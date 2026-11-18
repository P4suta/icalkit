// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const CARGO_PROBE: &str = r#"
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    println!("icalkit-cargo-probe\t{}", arguments.join("\t"));
    let nextest = arguments.first().is_some_and(|argument| argument == "nextest");
    let suite = if arguments.iter().any(|argument| argument == "--no-default-features") {
        "core"
    } else {
        "workspace"
    };
    if nextest && std::env::var("ICALKIT_CARGO_PROBE_FAIL").as_deref() == Ok(suite) {
        ExitCode::from(37)
    } else {
        ExitCode::SUCCESS
    }
}
"#;

struct RecipeFixture(PathBuf);

impl RecipeFixture {
    fn new() -> io::Result<Self> {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let directory = env::temp_dir().join(format!(
            "icalkit nextest output {} {}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory)?;
        let fixture = Self(directory);
        let bin = fixture.0.join("bin");
        fs::create_dir(&bin)?;
        fs::copy(
            super::workspace_root()?.join("Justfile"),
            fixture.0.join("Justfile"),
        )?;
        let source = fixture.0.join("cargo_probe.rs");
        fs::write(&source, CARGO_PROBE)?;
        let output = Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "cargo_probe"])
            .arg(source)
            .arg("-o")
            .arg(bin.join(format!("cargo{}", env::consts::EXE_SUFFIX)))
            .output()?;
        assert!(
            output.status.success(),
            "probe compilation failed: {output:?}"
        );
        Ok(fixture)
    }

    fn run(&self, recipe: &str, status: &str, failure: &str) -> io::Result<Output> {
        let inherited = env::var_os("PATH");
        let paths = std::iter::once(self.0.join("bin"))
            .chain(inherited.as_deref().into_iter().flat_map(env::split_paths));
        Command::new("just")
            .current_dir(&self.0)
            .arg("--justfile")
            .arg(self.0.join("Justfile"))
            .arg(recipe)
            .env("PATH", env::join_paths(paths).map_err(io::Error::other)?)
            .env("NEXTEST_STATUS_LEVEL", status)
            .env("NEXTEST_FINAL_STATUS_LEVEL", "fail")
            .env("ICALKIT_CARGO_PROBE_FAIL", failure)
            .output()
    }
}

impl Drop for RecipeFixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove recipe fixture: {error}");
        }
    }
}

fn invocations(output: &Output) -> Vec<Vec<String>> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("icalkit-cargo-probe\t"))
        .map(|line| line.split('\t').map(String::from).collect())
        .collect()
}

fn shows_leak_status(arguments: &[String]) -> bool {
    arguments
        .split(|argument| argument == "--")
        .next()
        .is_some_and(|options| {
            !options.iter().any(|option| option == "--no-run")
                && options
                    .windows(2)
                    .any(|pair| pair == ["--status-level", "leak"])
        })
}

#[test]
fn test_arguments_cannot_supply_nextest_status_options() {
    let arguments = ["nextest", "run", "--", "--status-level", "leak"].map(String::from);
    assert!(!shows_leak_status(&arguments));
}

#[test]
fn compile_only_invocations_cannot_verify_executed_tests() {
    let run = ["nextest", "run", "--status-level", "leak", "--workspace"].map(String::from);
    assert!(shows_leak_status(&run));
    let mut compile_only = run.to_vec();
    compile_only.push("--no-run".to_owned());
    assert!(!shows_leak_status(&compile_only));
    let mut test_arguments = run.to_vec();
    test_arguments.extend(["--", "--no-run"].map(String::from));
    assert!(shows_leak_status(&test_arguments));
}

#[test]
fn maintained_recipes_keep_leak_status_visible_under_sparse_output() -> io::Result<()> {
    let fixture = RecipeFixture::new()?;
    for recipe in ["test", "test-core", "test-ci"] {
        for status in ["fail", "none"] {
            let output = fixture.run(recipe, status, "")?;
            assert!(output.status.success(), "{recipe}: {output:?}");
            let commands = invocations(&output);
            let nextest: Vec<_> = commands
                .iter()
                .filter(|arguments| arguments.first().is_some_and(|value| value == "nextest"))
                .collect();
            assert_eq!(
                nextest.len(),
                if recipe == "test-core" { 1 } else { 2 },
                "{recipe}: {output:?}"
            );
            for arguments in nextest {
                assert!(
                    shows_leak_status(arguments),
                    "{recipe} hides leak status when NEXTEST_STATUS_LEVEL={status}: {arguments:?}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn a_failing_core_suite_stops_every_test_recipe() -> io::Result<()> {
    let fixture = RecipeFixture::new()?;
    for recipe in ["test", "test-core", "test-ci"] {
        let output = fixture.run(recipe, "fail", "core")?;
        assert!(!output.status.success(), "{recipe}: {output:?}");
        let commands = invocations(&output);
        assert_eq!(commands.len(), 1, "{recipe}: {output:?}");
        assert_eq!(commands[0][..2], ["nextest", "run"]);
        assert!(
            commands[0]
                .iter()
                .any(|value| value == "--no-default-features")
        );
    }
    Ok(())
}

#[test]
fn a_failing_workspace_suite_stops_before_workspace_doctests() -> io::Result<()> {
    let fixture = RecipeFixture::new()?;
    for recipe in ["test", "test-ci"] {
        let output = fixture.run(recipe, "fail", "workspace")?;
        assert!(!output.status.success(), "{recipe}: {output:?}");
        let commands = invocations(&output);
        assert_eq!(commands.len(), 3, "{recipe}: {output:?}");
        assert_eq!(commands[0][..2], ["nextest", "run"]);
        assert_eq!(commands[1][0], "test");
        assert!(commands[1].iter().any(|value| value == "--doc"));
        assert_eq!(commands[2][..2], ["nextest", "run"]);
        assert!(commands[2].iter().any(|value| value == "--workspace"));
    }
    Ok(())
}
