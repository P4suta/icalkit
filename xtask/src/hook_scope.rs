// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static CONFIGURATION_ID: AtomicUsize = AtomicUsize::new(0);

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
