// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static CONFIGURATION_ID: AtomicUsize = AtomicUsize::new(0);
const SPELLING_FAULT: &str = "t\u{65}h";

struct HookConfiguration(PathBuf);

impl HookConfiguration {
    fn new(job: &str) -> io::Result<Self> {
        let source = include_str!("../../lefthook.yml");
        let command = format!("run: mise exec -- just {job}");
        if source.matches(&command).count() != 1 {
            return Err(io::Error::other(format!(
                "expected one maintained command for {job}"
            )));
        }
        // Check selection without staging files or recursively building xtask.
        let source = source.replacen(&command, "run: rustc --version", 1);
        for _ in 0..8 {
            let id = CONFIGURATION_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "icalkit-hook-scope-{}-{id}.yml",
                std::process::id()
            ));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    let configuration = Self(path);
                    let written = file.write_all(source.as_bytes());
                    drop(file);
                    written?;
                    return Ok(configuration);
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a temporary hook configuration",
        ))
    }

    fn matches(&self, job: &str, path: &str) -> io::Result<()> {
        let output = Command::new("lefthook")
            .current_dir(super::workspace_root()?)
            .env("LEFTHOOK_CONFIG", &self.0)
            .args([
                "run",
                "pre-commit",
                "--command",
                job,
                "--file",
                path,
                "--no-auto-install",
                "--no-stage-fixed",
                "--colors",
                "off",
            ])
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let executed = stdout.lines().any(|line| {
            line.strip_prefix("rustc ")
                .and_then(|version| version.as_bytes().first())
                .is_some_and(u8::is_ascii_digit)
        });
        if output.status.success() && executed {
            return Ok(());
        }
        Err(io::Error::other(format!(
            "{job} did not run for {path}:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

impl Drop for HookConfiguration {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0) {
            eprintln!("could not remove temporary hook configuration: {error}");
        }
    }
}

fn require_scope(job: &str, paths: &[&str]) -> io::Result<()> {
    let configuration = HookConfiguration::new(job)?;
    for path in paths {
        configuration.matches(job, path)?;
    }
    Ok(())
}

#[test]
fn repository_hook_covers_development_configuration_and_rust_sources() -> io::Result<()> {
    require_scope(
        "repository",
        &[
            "Justfile",
            "mise.toml",
            "rust-toolchain.toml",
            "lefthook.yml",
            "CONTRIBUTING.md",
            "docs/repository-policy.md",
            ".github/workflows/ci.yml",
            "xtask/src/main.rs",
            "xtask/src/ci.rs",
            "xtask/src/development.rs",
            "xtask/src/checks/development.rs",
        ],
    )
}

#[test]
fn architecture_hook_covers_root_manifests_and_rust_sources() -> io::Result<()> {
    require_scope(
        "architecture",
        &[
            "Cargo.toml",
            "crates/icalkit/Cargo.toml",
            "release-plz.toml",
            "xtask/src/main.rs",
            "xtask/src/checks/development.rs",
        ],
    )
}

#[test]
fn purity_hook_covers_root_and_nested_manifests_and_rust_sources() -> io::Result<()> {
    require_scope(
        "purity",
        &[
            "Cargo.toml",
            "crates/icalkit/Cargo.toml",
            "crates/icalkit/src/calendar.rs",
        ],
    )
}

#[test]
fn license_hook_covers_metadata_sources_and_capture_artifacts() -> io::Result<()> {
    require_scope(
        "reuse",
        &[
            "README.md",
            "Cargo.toml",
            ".github/workflows/ci.yml",
            "xtask/src/main.rs",
            "crates/icalkit-conformance/corpus/manifest.v1.jsonl",
            "crates/icalkit-conformance/tests/fixtures/captured/example.ics",
            "crates/icalkit-conformance/corpus/evidence/example.png.license",
        ],
    )
}

struct StagingRepository {
    directory: PathBuf,
    local_environment: Vec<String>,
}

impl StagingRepository {
    fn new() -> io::Result<Self> {
        let variables = Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()?;
        if !variables.status.success() || variables.stdout.is_empty() {
            return Err(io::Error::other(
                "Git could not identify its local environment",
            ));
        }
        let variables = String::from_utf8(variables.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut directory = None;
        for _ in 0..8 {
            let id = CONFIGURATION_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("icalkit-hook-stage-{}-{id}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    directory = Some(candidate);
                    break;
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error),
            }
        }
        let directory = directory.ok_or_else(|| io::Error::other("no unused hook directory"))?;
        let repository = Self {
            directory,
            local_environment: variables.lines().map(str::to_owned).collect(),
        };
        fs::write(repository.directory.join("empty.gitconfig"), "")?;
        fs::write(repository.directory.join("empty.mise.toml"), "\n")?;
        fs::write(
            repository.directory.join("mise.toml"),
            include_str!("../../mise.toml"),
        )?;
        fs::write(
            repository.directory.join("lefthook.yml"),
            include_str!("../../lefthook.yml"),
        )?;
        fs::write(
            repository.directory.join("Justfile"),
            include_str!("../../Justfile"),
        )?;
        fs::write(
            repository.directory.join("Cargo.toml"),
            "[package]\nname = \"hook-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::create_dir(repository.directory.join("src"))?;
        fs::write(
            repository.directory.join("src/lib.rs"),
            "#[path = \"source example.rs\"]\nmod example;\n",
        )?;
        fs::write(
            repository.directory.join("src/source example.rs"),
            "pub fn value() -> u8 {\n    1\n}\n",
        )?;
        repository.git(&["init", "--quiet", "--template="])?;
        repository.git(&[
            "-c",
            "user.name=Hook Fixture",
            "-c",
            "user.email=hook.fixture@example.test",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "test: initialize hook fixture",
        ])?;
        Ok(repository)
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.directory);
        for variable in &self.local_environment {
            command.env_remove(variable);
        }
        for (name, _) in
            std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("LEFTHOOK"))
        {
            command.env_remove(name);
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.directory.join("empty.gitconfig"))
            .env_remove("MISE_CONFIG_FILE")
            .env(
                "MISE_GLOBAL_CONFIG_FILE",
                self.directory.join("empty.mise.toml"),
            )
            .env("MISE_TRUSTED_CONFIG_PATHS", &self.directory)
            .env_remove("GIT_TEMPLATE_DIR");
        command
    }

    fn git(&self, arguments: &[&str]) -> io::Result<Vec<u8>> {
        let output = self.command("git").args(arguments).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "Git {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        Ok(output.stdout)
    }

    fn preserves(
        &self,
        job: &str,
        path: &str,
        staged: &str,
        working: &str,
        diagnostic: Option<&str>,
    ) -> io::Result<()> {
        let file = self.directory.join(path);
        fs::write(&file, staged)?;
        self.git(&["add", "--", path])?;
        fs::write(&file, working)?;
        let output = self
            .command("lefthook")
            .env("LEFTHOOK_CONFIG", self.directory.join("lefthook.yml"))
            .args([
                "run",
                "pre-commit",
                "--command",
                job,
                "--no-auto-install",
                "--no-tty",
                "--colors",
                "off",
            ])
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            self.git(&["show", &format!(":{path}")])?,
            staged.as_bytes(),
            "{job} changed staged content:\n{stdout}\n{stderr}"
        );
        assert_eq!(
            fs::read(&file)?,
            working.as_bytes(),
            "{job} changed working content:\n{stdout}\n{stderr}"
        );
        match diagnostic {
            Some(diagnostic) => {
                assert!(
                    !output.status.success(),
                    "{job} accepted an input needing repairs:\n{stdout}\n{stderr}"
                );
                assert!(
                    stdout.contains(diagnostic) || stderr.contains(diagnostic),
                    "{job} did not report the intended problem:\n{stdout}\n{stderr}"
                );
            },
            None => assert!(
                output.status.success(),
                "{job} refused a valid input:\n{stdout}\n{stderr}"
            ),
        }
        Ok(())
    }
}

impl Drop for StagingRepository {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("could not remove hook repository: {error}");
        }
    }
}

#[test]
fn rust_format_hook_preserves_partial_staging() -> io::Result<()> {
    StagingRepository::new()?.preserves(
        "cargo-fmt",
        "src/source example.rs",
        "pub fn value()->u8{1}\n\npub const SECOND: u8 = 1;\n",
        "pub fn value()->u8{1}\n\npub const SECOND: u8 = 2;\n",
        Some("Diff in"),
    )
}

#[test]
fn hook_fixture_selects_tools_without_global_mise_defaults() -> io::Result<()> {
    let repository = StagingRepository::new()?;
    let configuration = repository.directory.join("empty.mise.toml");
    fs::write(&configuration, "\n")?;
    let output = repository
        .command("mise")
        .env("MISE_GLOBAL_CONFIG_FILE", configuration)
        .args(["exec", "--", "just", "--list"])
        .output()?;
    assert!(
        output.status.success(),
        "fixture tools need project selection:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("fmt-check"));
    Ok(())
}

#[test]
fn toml_format_hook_preserves_partial_staging() -> io::Result<()> {
    StagingRepository::new()?.preserves(
        "taplo-fmt",
        "config example.toml",
        "value=\"staged\"\n\nsecond = 1\n",
        "value=\"staged\"\n\nsecond = 2\n",
        Some("value"),
    )
}

#[test]
fn spelling_hook_preserves_partial_staging() -> io::Result<()> {
    StagingRepository::new()?.preserves(
        "typos",
        "notes example.md",
        &format!("This is {SPELLING_FAULT} note.\n\nAnother line.\n"),
        &format!("This is {SPELLING_FAULT} note.\n\nAnother edited line.\n"),
        Some(SPELLING_FAULT),
    )
}

#[test]
fn valid_hook_inputs_preserve_selected_and_working_content() -> io::Result<()> {
    for (job, path, staged, working) in [
        (
            "cargo-fmt",
            "src/source example.rs",
            "pub fn value() -> u8 {\n    1\n}\n",
            "pub fn value() -> u8 {\n    2\n}\n",
        ),
        (
            "taplo-fmt",
            "config example.toml",
            "value = \"staged\"\n",
            "value = \"working\"\n",
        ),
        (
            "typos",
            "notes example.md",
            "Readable documentation.\n",
            "Readable edited documentation.\n",
        ),
    ] {
        StagingRepository::new()?.preserves(job, path, staged, working, None)?;
    }
    Ok(())
}

#[test]
fn spelling_hook_treats_command_options_as_file_names() -> io::Result<()> {
    StagingRepository::new()?.preserves(
        "typos",
        "--write-changes",
        &format!("This is {SPELLING_FAULT} note.\n\nAnother line.\n"),
        &format!("This is {SPELLING_FAULT} note.\n\nAnother edited line.\n"),
        Some(SPELLING_FAULT),
    )
}
