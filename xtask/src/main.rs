// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Repository maintenance tasks for the icalkit workspace.
//!
//! Run with `cargo run -p xtask -- <task>`, or through the `Justfile` recipes.
//!
//! # `purity`
//!
//! Purity here means the core runs where calendars are actually rendered: a browser, an
//! embedded display, a server that already owns its HTTP client and its time zone
//! database. Two declarations carry that promise, and this task checks both for every core
//! crate — the manifest names no dependency outside the core set, and `lib.rs` declares
//! `#![no_std]`. No clock and no network follow from having nothing to call.
//!
//! `just no-std` and `just wasm` are the compile-time half: they prove the core builds for
//! `thumbv7em-none-eabi` and for `wasm32-unknown-unknown`. What they cannot express is the
//! dependency rule, because a `no_std` crate from outside compiles perfectly well while
//! still binding this core to someone else's MSRV, release cadence, and license. That rule
//! is a decision rather than a build property, so it needs a gate that reads what the
//! manifest declares.
//!
//! The reasoning is `docs/adr/0004`, which the violations cite: the fix is usually to move
//! the code up a layer, not to add the dependency.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The crates that make up the sans-I/O core.
///
/// Mirrors `core_crates` in the `Justfile`. A new core crate belongs in both.
/// `ical-conform` is deliberately absent: the conformance runner is allowed `std` and
/// depends on every crate here, which is what makes it a consumer of the core rather than
/// part of it.
const CORE_CRATES: &[&str] = &[
    "ical-core",
    "ical-recur",
    "ical-tz",
    "ical-itip",
    "ical-dav",
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("purity") => report("purity", collect_purity_violations()),
        Some(task) => {
            eprintln!("xtask: unknown task `{task}`");
            print_usage();
            ExitCode::FAILURE
        },
        None => {
            print_usage();
            ExitCode::FAILURE
        },
    }
}

/// Print the available tasks.
fn print_usage() {
    eprintln!("usage: cargo run -p xtask -- purity");
}

/// Turn a task's findings into output and an exit status.
///
/// Every violation is printed. A gate that stops at the first one turns a five-minute fix
/// into five CI rounds.
fn report(task: &str, outcome: io::Result<Vec<String>>) -> ExitCode {
    match outcome {
        Err(error) => {
            eprintln!("xtask: {task} could not run: {error}");
            ExitCode::FAILURE
        },
        Ok(violations) if violations.is_empty() => {
            println!("{task}: ok");
            ExitCode::SUCCESS
        },
        Ok(violations) => {
            for violation in &violations {
                eprintln!("{task}: {violation}");
            }
            eprintln!(
                "{task}: {count} violation(s). See docs/adr/",
                count = violations.len()
            );
            ExitCode::FAILURE
        },
    }
}

/// Check that every core crate declares no outside dependency and stays `no_std`.
fn collect_purity_violations() -> io::Result<Vec<String>> {
    let root = workspace_root()?.join("crates");
    let mut violations = Vec::new();

    for crate_name in CORE_CRATES {
        let dir = root.join(crate_name);
        let manifest = fs::read_to_string(dir.join("Cargo.toml"))?;
        for dependency in outside_dependencies(&manifest) {
            violations.push(format!(
                "{crate_name}: declares `{dependency}`; a core crate may depend only on \
                 other core crates (ADR 0004)"
            ));
        }

        let lib = dir.join("src").join("lib.rs");
        if !lib.is_file() {
            violations.push(format!("{crate_name}: has no src/lib.rs"));
            continue;
        }
        if !declares_no_std(&fs::read_to_string(&lib)?) {
            violations.push(format!(
                "{crate_name}: src/lib.rs does not declare `#![no_std]` (ADR 0004)"
            ));
        }
    }

    Ok(violations)
}

/// The dependencies a manifest declares that a core crate is not allowed to have.
fn outside_dependencies(manifest: &str) -> Vec<String> {
    declared_dependencies(manifest)
        .into_iter()
        .filter(|name| !CORE_CRATES.contains(&name.as_str()))
        .collect()
}

/// Whether a source file declares `#![no_std]`.
///
/// Matched as a whole line rather than as a substring, so that naming the attribute in a
/// doc comment or a string literal does not satisfy the gate.
fn declares_no_std(source: &str) -> bool {
    source.lines().any(|line| line.trim() == "#![no_std]")
}

/// Locate the workspace root relative to this crate.
fn workspace_root() -> io::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the xtask manifest directory has no parent",
            )
        })
}

/// Collect the dependency names a manifest declares.
///
/// This is a deliberate hand-rolled scan rather than a TOML parser. The tool that enforces
/// "the core has no outside dependencies" should not itself have any, and it only ever
/// reads manifests this repository writes in one documented style. It understands both
/// `[dependencies]` tables and `[dependencies.name]` sub-tables, and treats `dev-` and
/// `build-` dependencies and `[target.'cfg(..)'.dependencies]` the same way, because a core
/// crate should not acquire those either.
fn declared_dependencies(manifest: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut inside_dependency_table = false;

    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let header = header.trim();
            if let Some(name) = dependency_subtable_name(header) {
                names.insert(name.to_owned());
                inside_dependency_table = false;
            } else {
                inside_dependency_table = is_dependency_table(header);
            }
            continue;
        }

        if inside_dependency_table {
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim().trim_matches('"');
                if !key.is_empty() {
                    names.insert(key.to_owned());
                }
            }
        }
    }

    names
}

/// Whether a table header names a dependency table.
fn is_dependency_table(header: &str) -> bool {
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .any(|kind| header == *kind || header.ends_with(&format!(".{kind}")))
}

/// Extract `name` from a `[dependencies.name]` style header.
fn dependency_subtable_name(header: &str) -> Option<&str> {
    let (prefix, name) = header.rsplit_once('.')?;
    is_dependency_table(prefix).then(|| name.trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::{
        declared_dependencies, declares_no_std, dependency_subtable_name, is_dependency_table,
        outside_dependencies,
    };

    #[test]
    fn reads_dependencies_from_a_plain_table() {
        let manifest = r#"
[package]
name = "x"

[dependencies]
serde = "1"
log = { version = "0.4" }
"#;
        let names = declared_dependencies(manifest);
        assert!(names.contains("serde"));
        assert!(names.contains("log"));
        assert!(!names.contains("name"), "package keys are not dependencies");
    }

    #[test]
    fn reads_dependencies_from_a_subtable_header() {
        let names = declared_dependencies("[dependencies.serde]\nversion = \"1\"\n");
        assert!(names.contains("serde"));
        assert!(
            !names.contains("version"),
            "keys inside a dependency subtable describe it, they are not further dependencies"
        );
    }

    #[test]
    fn treats_dev_build_and_target_dependencies_as_dependencies() {
        assert!(is_dependency_table("dev-dependencies"));
        assert!(is_dependency_table("build-dependencies"));
        assert!(is_dependency_table("target.'cfg(windows)'.dependencies"));
        assert!(!is_dependency_table("package"));
        assert!(!is_dependency_table("lints.clippy"));
    }

    #[test]
    fn names_a_dependency_subtable_only_under_a_dependency_table() {
        assert_eq!(
            dependency_subtable_name("dependencies.serde"),
            Some("serde")
        );
        assert_eq!(
            dependency_subtable_name("target.'cfg(windows)'.dependencies"),
            None,
            "this is a dependency table, not a single dependency"
        );
        assert_eq!(dependency_subtable_name("workspace.package"), None);
    }

    #[test]
    fn an_empty_manifest_declares_nothing() {
        assert!(declared_dependencies("").is_empty());
        assert!(declared_dependencies("[dependencies]\n").is_empty());
    }

    #[test]
    fn only_dependencies_from_outside_the_core_are_violations() {
        let manifest = r#"
[dependencies]
ical-core = { workspace = true }

[dev-dependencies]
proptest = "1"
"#;
        assert_eq!(outside_dependencies(manifest), vec!["proptest".to_owned()]);
    }

    #[test]
    fn recognizes_a_crate_level_no_std_attribute() {
        assert!(declares_no_std("//! docs\n\n#![no_std]\n"));
        assert!(!declares_no_std("//! docs\n"));
        assert!(
            !declares_no_std("//! This crate is `#![no_std]` throughout.\n"),
            "naming the attribute in prose is not declaring it"
        );
    }
}
