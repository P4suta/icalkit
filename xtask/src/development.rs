// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::io;
use std::path::Path;
use std::process::Command;

pub(super) fn collect(workspace: &Path) -> io::Result<Vec<String>> {
    let mise = configured_toolchain(workspace, "mise.toml", "tools.rust")?;
    let rustup = configured_toolchain(workspace, "rust-toolchain.toml", "toolchain.channel")?;
    let mut violations = toolchain_violations(&mise, &rustup);
    violations.extend(recipe_violations(&std::fs::read_to_string(
        workspace.join("Justfile"),
    )?));
    Ok(violations)
}

fn configured_toolchain(workspace: &Path, file: &str, key: &str) -> io::Result<String> {
    let output = Command::new("taplo")
        .args(["get", "--strip-newline", "--file-path"])
        .arg(workspace.join(file))
        .arg(key)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "taplo could not read {file} {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn toolchain_violations(mise: &str, rustup: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (file, version) in [("mise.toml", mise), ("rust-toolchain.toml", rustup)] {
        if version.split('.').count() != 3
            || !version.split('.').all(|part| {
                part.parse::<u32>()
                    .is_ok_and(|value| value.to_string() == part)
            })
        {
            violations.push(format!(
                "{file}: Rust must use an exact major.minor.patch version"
            ));
        }
    }
    if mise != rustup {
        violations
            .push("mise.toml and rust-toolchain.toml must select the same Rust version".to_owned());
    }
    violations
}

fn recipe_body<'a>(justfile: &'a str, name: &str) -> Vec<&'a str> {
    justfile
        .split_once(&format!("\n{name}:\n"))
        .map_or_else(Vec::new, |(_, body)| {
            body.lines()
                .take_while(|line| {
                    line.starts_with(' ') || line.starts_with('#') || line.trim().is_empty()
                })
                .map(str::trim)
                .collect()
        })
}

fn recipe_violations(justfile: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for recipe in ["test", "test-ci"] {
        let header = justfile
            .lines()
            .find(|line| line.starts_with(&format!("{recipe}:")))
            .unwrap_or("");
        if !header.split_whitespace().any(|word| word == "test-core") {
            violations.push(format!("Justfile: {recipe} requires test-core"));
        }
    }
    let core = recipe_body(justfile, "test-core");
    for command in [
        "cargo nextest run --locked --profile ci -p icalkit --no-default-features",
        "cargo test --locked -p icalkit --doc --no-default-features",
    ] {
        if !core.contains(&command) {
            violations.push(format!("Justfile: test-core is missing {command}"));
        }
    }
    for (index, line) in justfile.lines().enumerate() {
        let words: Vec<_> = line
            .trim()
            .trim_start_matches('@')
            .split_whitespace()
            .collect();
        let resolves = matches!(
            words.as_slice(),
            [
                "cargo",
                "build" | "check" | "clippy" | "test" | "doc" | "run",
                ..
            ] | ["cargo", "nextest", "run", ..]
                | ["cargo", "hack", "check", ..]
        );
        if resolves && !words.contains(&"--locked") {
            violations.push(format!(
                "Justfile:{}: dependency-resolving commands require --locked",
                index.saturating_add(1)
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::{recipe_violations, toolchain_violations};

    const TEST_RECIPES: &str = "
test: test-core

test-ci: test-core

test-core:
    cargo nextest run --locked --profile ci -p icalkit --no-default-features
    cargo test --locked -p icalkit --doc --no-default-features
";

    #[test]
    fn exact_matching_toolchains_are_admitted() {
        assert_eq!(
            toolchain_violations("1.99.0", "1.99.0"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn moving_partial_and_mismatched_toolchains_are_refused() {
        for version in [
            "stable", "beta", "nightly", "1", "1.99", "1.099.0", "", "1.99.0.1", "1.99.0\n",
            " 1.99.0", "1.+99.0",
        ] {
            assert!(
                !toolchain_violations(version, version).is_empty(),
                "admitted moving or malformed toolchain {version:?}"
            );
        }
        assert_ne!(
            toolchain_violations("1.99.0", "1.98.0"),
            Vec::<String>::new()
        );
        assert_ne!(
            toolchain_violations("stable", "1.99.0"),
            Vec::<String>::new()
        );
        assert_ne!(
            toolchain_violations("1.99.0", "stable"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn both_test_entrypoints_require_the_standalone_core_suite() {
        assert_eq!(recipe_violations(TEST_RECIPES), Vec::<String>::new());
        for recipe in ["test", "test-ci"] {
            let broken =
                TEST_RECIPES.replace(&format!("{recipe}: test-core"), &format!("{recipe}:"));
            assert!(
                !recipe_violations(&broken).is_empty(),
                "{recipe} omitted the standalone suite"
            );
        }
    }

    #[test]
    fn removing_either_standalone_test_command_is_refused() {
        for command in [
            "cargo nextest run --locked --profile ci -p icalkit --no-default-features",
            "cargo test --locked -p icalkit --doc --no-default-features",
        ] {
            let broken = TEST_RECIPES.replace(command, "");
            assert!(!recipe_violations(&broken).is_empty(), "removed {command}");
        }
    }

    #[test]
    fn dependency_resolving_commands_require_the_committed_lockfile() {
        for command in [
            "cargo build",
            "cargo check",
            "cargo clippy",
            "cargo test",
            "cargo doc",
            "cargo run",
            "cargo nextest run",
            "cargo hack check",
        ] {
            let unlocked = format!("{TEST_RECIPES}\nprobe:\n    {command} --workspace\n");
            assert!(
                !recipe_violations(&unlocked).is_empty(),
                "unlocked {command}"
            );
            let locked = unlocked.replace(
                &format!("{command} --workspace"),
                &format!("{command} --locked --workspace"),
            );
            assert!(recipe_violations(&locked).is_empty(), "locked {command}");
        }
    }
}
