// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::io;
use std::path::Path;
use std::process::Command;

pub(super) fn collect(workspace: &Path) -> io::Result<Vec<String>> {
    let mise = configured_value(workspace, "mise.toml", "tools.rust.version", "value")?;
    let rustup = configured_value(
        workspace,
        "rust-toolchain.toml",
        "toolchain.channel",
        "value",
    )?;
    let mut violations = toolchain_violations(&mise, &rustup);
    let shellcheck = configured_value(workspace, "mise.toml", "tools.shellcheck", "value")?;
    if !exact_version(&shellcheck) {
        violations.push("mise.toml: actionlint requires an exact shellcheck version".to_owned());
    }
    for (file, table) in [
        ("mise.toml", "tools.rust"),
        ("rust-toolchain.toml", "toolchain"),
    ] {
        let profile = configured_value(workspace, file, &format!("{table}.profile"), "value")?;
        if profile != "minimal" {
            violations.push(format!(
                "{file}: Rust must use the minimal installation profile"
            ));
        }
        let components = configured_value(workspace, file, &format!("{table}.components"), "json")?;
        let components = component_names(&components).ok_or_else(|| {
            io::Error::other(format!(
                "{file}: Rust components must be an array of component names"
            ))
        })?;
        for required in ["rustfmt", "clippy"] {
            if !components.contains(&required) {
                violations.push(format!("{file}: Rust installation requires {required}"));
            }
        }
    }
    violations.extend(recipe_violations(&std::fs::read_to_string(
        workspace.join("Justfile"),
    )?));
    Ok(violations)
}

fn configured_value(workspace: &Path, file: &str, key: &str, format: &str) -> io::Result<String> {
    let output = Command::new("taplo")
        .args([
            "get",
            "--strip-newline",
            "--output-format",
            format,
            "--file-path",
        ])
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

fn component_names(json: &str) -> Option<Vec<&str>> {
    let entries = json.trim().strip_prefix('[')?.strip_suffix(']')?;
    if entries.trim().is_empty() {
        return Some(Vec::new());
    }
    entries
        .split(',')
        .map(|entry| {
            let name = entry.trim().strip_prefix('"')?.strip_suffix('"')?;
            (!name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
            .then_some(name)
        })
        .collect()
}

fn toolchain_violations(mise: &str, rustup: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (file, version) in [("mise.toml", mise), ("rust-toolchain.toml", rustup)] {
        if !exact_version(version) {
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

fn exact_version(version: &str) -> bool {
    version.split('.').count() == 3
        && version.split('.').all(|part| {
            part.parse::<u32>()
                .is_ok_and(|value| value.to_string() == part)
        })
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
        let dependencies = header.split_once('#').map_or(header, |(body, _)| body);
        if !dependencies
            .split_whitespace()
            .take_while(|word| *word != "&&")
            .any(|word| word == "test-core")
        {
            violations.push(format!("Justfile: {recipe} requires test-core"));
        }
    }
    let core = recipe_body(justfile, "test-core");
    for command in [
        "cargo nextest run --locked --status-level leak --profile ci -p icalkit --no-default-features",
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
            .take_while(|word| *word != "--" && !word.starts_with('#'))
            .collect();
        let operations = match words.as_slice() {
            ["cargo", selector, operations @ ..] if selector.starts_with('+') => operations,
            ["cargo", operations @ ..] => operations,
            _ => continue,
        };
        let resolves = matches!(
            operations,
            ["build" | "check" | "clippy" | "test" | "doc" | "run", ..]
                | ["nextest", "run", ..]
                | ["hack", "check", ..]
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
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{collect, recipe_violations, toolchain_violations};

    const TEST_RECIPES: &str = "
test: test-core

test-ci: test-core

test-core:
    cargo nextest run --locked --status-level leak --profile ci -p icalkit --no-default-features
    cargo test --locked -p icalkit --doc --no-default-features
";

    const INSTALLATION: &str = "[tools]\nrust = { version = \"1.99.0\", profile = \"minimal\", components = [\"rustfmt\", \"clippy\"] }\nshellcheck = \"0.11.0\"\n";
    const RUSTUP: &str = "[toolchain]\nchannel = \"1.99.0\"\nprofile = \"minimal\"\ncomponents = [\"rustfmt\", \"clippy\"]\n";

    struct ConfigurationFixture(PathBuf);

    impl ConfigurationFixture {
        fn new(mise: &str, rustup: &str) -> io::Result<Self> {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "icalkit-development-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            let fixture = Self(path);
            fs::write(fixture.0.join("mise.toml"), mise)?;
            fs::write(fixture.0.join("rust-toolchain.toml"), rustup)?;
            fs::write(fixture.0.join("Justfile"), TEST_RECIPES)?;
            Ok(fixture)
        }
    }

    impl Drop for ConfigurationFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove owned configuration fixture");
        }
    }

    #[test]
    fn a_version_without_component_installation_is_refused() -> io::Result<()> {
        let fixture = ConfigurationFixture::new("[tools]\nrust = \"1.99.0\"\n", RUSTUP)?;
        assert!(
            !matches!(collect(&fixture.0), Ok(violations) if violations.is_empty()),
            "the version alone does not install rustfmt and clippy"
        );
        Ok(())
    }

    #[test]
    fn explicit_matching_component_installations_are_admitted() -> io::Result<()> {
        let fixture = ConfigurationFixture::new(INSTALLATION, RUSTUP)?;
        assert_eq!(collect(&fixture.0)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn actionlint_requires_an_exact_shellcheck_installation() -> io::Result<()> {
        for version in ["", "latest", "0.11", "0.011.0"] {
            let mise = if version.is_empty() {
                INSTALLATION.replace("shellcheck = \"0.11.0\"\n", "")
            } else {
                INSTALLATION.replace("0.11.0", version)
            };
            let fixture = ConfigurationFixture::new(&mise, RUSTUP)?;
            assert!(
                !matches!(collect(&fixture.0), Ok(violations) if violations.is_empty()),
                "actionlint's helper lacks an exact installation: {version:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn either_missing_component_is_refused_in_both_configurations() -> io::Result<()> {
        for removed in ["\"rustfmt\", ", ", \"clippy\""] {
            for (mise, rustup) in [
                (INSTALLATION.replace(removed, ""), RUSTUP.to_owned()),
                (INSTALLATION.to_owned(), RUSTUP.replace(removed, "")),
            ] {
                let fixture = ConfigurationFixture::new(&mise, &rustup)?;
                assert!(
                    !matches!(collect(&fixture.0), Ok(violations) if violations.is_empty()),
                    "missing component admitted: {mise:?} {rustup:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn component_order_and_toml_escapes_preserve_installation_identity() -> io::Result<()> {
        let mise = INSTALLATION.replace("\"rustfmt\", \"clippy\"", "\"clipp\\u0079\", \"rustfmt\"");
        let fixture = ConfigurationFixture::new(&mise, RUSTUP)?;
        assert_eq!(collect(&fixture.0)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn each_rust_component_requires_its_own_array_entry() -> io::Result<()> {
        for components in [
            "[\"rustfmt\\nclippy\"]",
            "\"rustfmt\\nclippy\"",
            "[[\"rustfmt\", \"clippy\"]]",
            "[\"rustfmt\", \"clippy\", 1]",
        ] {
            let replace = |configuration: &str| {
                configuration.replace("[\"rustfmt\", \"clippy\"]", components)
            };
            for (mise, rustup) in [
                (replace(INSTALLATION), RUSTUP.to_owned()),
                (INSTALLATION.to_owned(), replace(RUSTUP)),
            ] {
                let fixture = ConfigurationFixture::new(&mise, &rustup)?;
                assert!(
                    !matches!(collect(&fixture.0), Ok(violations) if violations.is_empty()),
                    "invalid component entries were admitted: {components}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn extra_named_rust_components_preserve_required_entries() -> io::Result<()> {
        let extend =
            |configuration: &str| configuration.replace("\"clippy\"]", "\"clippy\", \"rust-src\"]");
        let fixture = ConfigurationFixture::new(&extend(INSTALLATION), &extend(RUSTUP))?;
        assert_eq!(collect(&fixture.0)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn mismatched_installation_profiles_are_refused() -> io::Result<()> {
        let mise = INSTALLATION.replace("minimal", "default");
        let fixture = ConfigurationFixture::new(&mise, RUSTUP)?;
        assert!(
            !matches!(collect(&fixture.0), Ok(violations) if violations.is_empty()),
            "different installation profiles were admitted"
        );
        Ok(())
    }

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
    fn comments_do_not_satisfy_standalone_test_dependencies() {
        for recipe in ["test", "test-ci"] {
            let broken = TEST_RECIPES.replace(
                &format!("{recipe}: test-core"),
                &format!("{recipe}: # test-core"),
            );
            assert!(
                !recipe_violations(&broken).is_empty(),
                "a comment replaced {recipe}'s dependency"
            );
            let valid = TEST_RECIPES.replace(
                &format!("{recipe}: test-core"),
                &format!("{recipe}: test-core # required coverage"),
            );
            assert_eq!(recipe_violations(&valid), Vec::<String>::new());
        }
    }

    #[test]
    fn standalone_core_must_be_a_prior_recipe_dependency() {
        for recipe in ["test", "test-ci"] {
            let broken = TEST_RECIPES.replace(
                &format!("{recipe}: test-core"),
                &format!("{recipe}: && test-core"),
            );
            assert!(
                !recipe_violations(&broken).is_empty(),
                "a later dependency replaced {recipe}'s prerequisite"
            );
            let valid = TEST_RECIPES.replace(
                &format!("{recipe}: test-core"),
                &format!("{recipe}: test-core && unrelated"),
            );
            assert_eq!(recipe_violations(&valid), Vec::<String>::new());
        }
    }

    #[test]
    fn toolchain_qualified_commands_require_the_committed_lockfile() {
        for operation in [
            "build",
            "check",
            "clippy",
            "test",
            "doc",
            "run",
            "nextest run",
            "hack check",
        ] {
            let command = format!("cargo +1.100.0 {operation}");
            for arguments in ["--workspace", "--workspace -- --locked"] {
                let broken = format!("{TEST_RECIPES}\nprobe:\n    {command} {arguments}\n");
                assert!(
                    !recipe_violations(&broken).is_empty(),
                    "a toolchain selector bypassed --locked: {command} {arguments}"
                );
            }
            let valid = format!("{TEST_RECIPES}\nprobe:\n    {command} --locked --workspace\n");
            assert_eq!(recipe_violations(&valid), Vec::<String>::new());
        }
    }

    #[test]
    fn comments_and_test_arguments_do_not_lock_dependency_resolution() {
        for command in [
            "cargo check --workspace # --locked",
            "cargo test --workspace -- --locked",
            "cargo nextest run --workspace -- --locked",
            "cargo clippy --workspace -- -D warnings # --locked",
        ] {
            let broken = format!("{TEST_RECIPES}\nprobe:\n    {command}\n");
            assert!(
                !recipe_violations(&broken).is_empty(),
                "a non-Cargo argument satisfied the lockfile check: {command}"
            );
        }
        let valid = format!(
            "{TEST_RECIPES}\nprobe:\n    cargo clippy --locked --workspace -- -D warnings # checked\n"
        );
        assert_eq!(recipe_violations(&valid), Vec::<String>::new());
    }

    #[test]
    fn removing_either_standalone_test_command_is_refused() {
        for command in [
            "cargo nextest run --locked --status-level leak --profile ci -p icalkit --no-default-features",
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
