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
//! database. Declarations carry that promise, and this task reads them for every core
//! crate: the manifest links no package outside the core set and takes each one from inside
//! this workspace, and `lib.rs` declares `#![no_std]` and `extern crate alloc;`. No clock
//! and no network follow from having nothing to call.
//!
//! A dependency's key is a nickname its author chose; the name Cargo links is the `package`
//! field. `ical-dav = { package = "libm", version = "0.2" }` wrote a name from
//! [`CORE_CRATES`] and linked a third-party crate, and passed every leg of the gate for the
//! gate's entire life. Entries are therefore read for their linked name in both spellings,
//! the rename is a violation on its own, and a dependency that resolves from a registry
//! rather than from `workspace = true` or a path under `crates/` is one too.
//!
//! The rule is also only as wide as the list it is applied to, so a crate under `crates/`
//! that declares `#![no_std]` without appearing in [`CORE_CRATES`] fails: the exemption
//! `ical-conform` holds is "not `no_std`", not "not listed".
//!
//! `just no-std` and `just wasm` are the compile-time half: they prove the core builds for
//! `thumbv7em-none-eabi` and for `wasm32-unknown-unknown`. What they cannot express is the
//! dependency rule, because a `no_std` crate from outside compiles perfectly well while
//! still binding this core to someone else's MSRV, release cadence, and license. That rule
//! is a decision rather than a build property, so it needs a gate that reads what the
//! manifest declares.
//!
//! The reasoning is `docs/adr/0004` and `docs/adr/0007`, which the violations cite: the fix
//! is usually to move the code up a layer, not to add the dependency.

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
    "ical-grammar",
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
        violations.extend(manifest_violations(crate_name, &manifest));

        let lib = dir.join("src").join("lib.rs");
        if !lib.is_file() {
            violations.push(format!("{crate_name}: has no src/lib.rs"));
            continue;
        }
        let source = fs::read_to_string(&lib)?;
        if !declares(&source, "#![no_std]") {
            violations.push(format!(
                "{crate_name}: src/lib.rs does not declare `#![no_std]` (ADR 0004)"
            ));
        }
        if !declares(&source, "extern crate alloc;") {
            violations.push(format!(
                "{crate_name}: src/lib.rs does not declare `extern crate alloc;`; the core \
                 crates are `no_std` and `alloc`, and the declaration is the policy (ADR 0007)"
            ));
        }
    }

    violations.extend(unregistered_core_crates(&root)?);
    Ok(violations)
}

/// Everything a core crate's manifest declares that the rule forbids.
fn manifest_violations(crate_name: &str, manifest: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for dependency in declared_dependencies(manifest) {
        let Declared { key, package, .. } = &dependency;
        if dependency.renamed {
            violations.push(format!(
                "{crate_name}: `{key}` renames the package it links to `{package}`; a rename \
                 exists only to make the linked name differ from the written one (ADR 0004)"
            ));
        }
        if !CORE_CRATES.contains(&package.as_str()) {
            violations.push(format!(
                "{crate_name}: declares `{package}`; a core crate may depend only on other \
                 core crates (ADR 0004)"
            ));
        } else if !dependency.local {
            violations.push(format!(
                "{crate_name}: `{package}` does not resolve from inside this workspace; use \
                 `workspace = true` or a path under crates/ (ADR 0004)"
            ));
        }
    }
    violations
}

/// Crates that declare `#![no_std]` without being named in [`CORE_CRATES`].
///
/// The rule is only as wide as the list it is applied to, so the list must not be able to go
/// stale behind a crate somebody added (ADR 0004). A crate that wants the exemption
/// `ical-conform` has takes it by not being `no_std`, which is what that exemption means.
fn unregistered_core_crates(root: &Path) -> io::Result<Vec<String>> {
    let mut violations = Vec::new();
    for entry in fs::read_dir(root)? {
        let dir = entry?.path();
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if CORE_CRATES.contains(&name) {
            continue;
        }
        let lib = dir.join("src").join("lib.rs");
        if lib.is_file() && declares(&fs::read_to_string(&lib)?, "#![no_std]") {
            violations.push(format!(
                "{name}: declares `#![no_std]` but is absent from CORE_CRATES, so no rule in \
                 this gate applies to it (ADR 0004)"
            ));
        }
    }
    violations.sort();
    Ok(violations)
}

/// Whether a source file carries `declaration` as a declaration of its own.
///
/// Matched as a whole line rather than as a substring, so that naming an attribute in a doc
/// comment or a string literal does not satisfy the gate.
fn declares(source: &str, declaration: &str) -> bool {
    source.lines().any(|line| line.trim() == declaration)
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

/// One dependency entry, as the gate has to see it rather than as it was written.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Declared {
    /// The key to the left of `=`, or the last segment of a sub-table header. A nickname.
    key: String,
    /// The package Cargo links: the `package = "..."` value when one is given, the key
    /// otherwise.
    package: String,
    /// Whether the entry names a package other than its key.
    renamed: bool,
    /// Whether the entry resolves from inside this workspace rather than from a registry.
    local: bool,
}

impl Declared {
    /// Reads one entry from its key and the specification to the right of it.
    fn read(key: &str, spec: &str) -> Self {
        let package = inline_value(spec, "package");
        Self {
            key: key.to_owned(),
            package: package.unwrap_or(key).to_owned(),
            renamed: package.is_some_and(|package| package != key),
            local: inline_value(spec, "workspace") == Some("true")
                || inline_value(spec, "path").is_some(),
        }
    }
}

/// Collect the dependencies a manifest declares.
///
/// This is a deliberate hand-rolled scan rather than a TOML parser. The tool that enforces
/// "the core has no outside dependencies" should not itself have any, and it only ever
/// reads manifests this repository writes in one documented style. It understands both
/// `[dependencies]` tables and `[dependencies.name]` sub-tables, and treats `dev-` and
/// `build-` dependencies and `[target.'cfg(..)'.dependencies]` the same way, because a core
/// crate should not acquire those either.
///
/// A key is a nickname its author chose. `ical-dav = { package = "libm", version = "0.2" }`
/// writes a name from `CORE_CRATES` and links `libm`, so every entry is read for the
/// `package` field in both spellings — inside the inline table, and as a line of the
/// sub-table, which is why a sub-table is gathered into one specification before it is read.
fn declared_dependencies(manifest: &str) -> Vec<Declared> {
    let mut declared = Vec::new();
    let mut subtable: Option<(String, Vec<String>)> = None;
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
            flush_subtable(&mut subtable, &mut declared);
            let header = header.trim();
            match dependency_subtable_name(header) {
                Some(name) => {
                    subtable = Some((name.to_owned(), Vec::new()));
                    inside_dependency_table = false;
                },
                None => inside_dependency_table = is_dependency_table(header),
            }
            continue;
        }

        if let Some((_, body)) = subtable.as_mut() {
            body.push(line.to_owned());
        } else if inside_dependency_table {
            if let Some((key, spec)) = line.split_once('=') {
                let key = key.trim().trim_matches('"');
                if !key.is_empty() {
                    declared.push(Declared::read(key, spec));
                }
            }
        }
    }

    flush_subtable(&mut subtable, &mut declared);
    declared.sort();
    declared
}

/// Turn a gathered `[dependencies.name]` sub-table into one entry.
///
/// The body is rewritten as an inline table so that one reader handles both spellings; a
/// `package = "..."` line inside a sub-table is the same rename as the inline one, and
/// reading only the header is how it used to go unseen.
fn flush_subtable(subtable: &mut Option<(String, Vec<String>)>, declared: &mut Vec<Declared>) {
    let Some((name, body)) = subtable.take() else {
        return;
    };
    let spec = format!("{{ {} }}", body.join(", "));
    declared.push(Declared::read(&name, &spec));
}

/// The value of `key` in an inline table such as `{ package = "libm", version = "0.2" }`.
///
/// `None` for a bare version requirement, which is a specification with no keys at all.
fn inline_value<'a>(spec: &'a str, key: &str) -> Option<&'a str> {
    let inner = spec.trim().strip_prefix('{')?.strip_suffix('}')?;
    inner.split(',').find_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"'))
    })
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
        declared_dependencies, declares, dependency_subtable_name, is_dependency_table,
        manifest_violations,
    };

    /// The packages a manifest links, which is what every rule here is stated over.
    fn packages(manifest: &str) -> Vec<String> {
        declared_dependencies(manifest)
            .into_iter()
            .map(|dependency| dependency.package)
            .collect()
    }

    #[test]
    fn reads_dependencies_from_a_plain_table() {
        let manifest = r#"
[package]
name = "x"

[dependencies]
serde = "1"
log = { version = "0.4" }
"#;
        let linked = packages(manifest);
        assert!(linked.contains(&"serde".to_owned()));
        assert!(linked.contains(&"log".to_owned()));
        assert!(
            !linked.contains(&"name".to_owned()),
            "package keys are not dependencies"
        );
    }

    #[test]
    fn reads_dependencies_from_a_subtable_header() {
        let linked = packages(
            "[dependencies.serde]
version = \"1\"
",
        );
        assert_eq!(linked, vec!["serde".to_owned()]);
        assert!(
            !linked.contains(&"version".to_owned()),
            "keys inside a dependency subtable describe it, they are not further dependencies"
        );
    }

    #[test]
    fn an_inline_rename_is_read_as_the_package_it_links() {
        let manifest = r#"
[dependencies]
ical-dav = { package = "libm", version = "0.2" }
"#;
        assert_eq!(packages(manifest), vec!["libm".to_owned()]);

        let violations = manifest_violations("ical-tz", manifest);
        assert_eq!(violations.len(), 2, "the rename and the outside package");
        assert!(
            violations.iter().any(|line| line.contains("renames")),
            "a rename is itself a violation: {violations:?}"
        );
        assert!(
            violations.iter().any(|line| line.contains("`libm`")),
            "the linked package is named, not the key: {violations:?}"
        );
    }

    #[test]
    fn a_subtable_rename_is_read_as_the_package_it_links() {
        let manifest = r#"
[dependencies.ical-dav]
package = "libm"
version = "0.2"
"#;
        assert_eq!(packages(manifest), vec!["libm".to_owned()]);
        assert!(
            manifest_violations("ical-tz", manifest)
                .iter()
                .any(|line| line.contains("`libm`")),
            "a package line inside a subtable is the same rename as the inline one"
        );
    }

    #[test]
    fn an_honest_core_dependency_is_no_violation() {
        let manifest = r#"
[package]
name = "ical-itip"

[dependencies]
ical-core = { workspace = true }
ical-recur = { path = "../ical-recur" }

[dependencies.ical-tz]
workspace = true
"#;
        assert_eq!(
            manifest_violations("ical-itip", manifest),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_core_crate_taken_from_a_registry_is_a_violation() {
        let manifest = "[dependencies]
ical-core = \"0.1\"
";
        assert!(
            manifest_violations("ical-recur", manifest)
                .iter()
                .any(|line| line.contains("does not resolve from inside this workspace")),
            "the name is ours; the source is not"
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
        assert!(
            declared_dependencies(
                "[dependencies]
"
            )
            .is_empty()
        );
    }

    #[test]
    fn only_dependencies_from_outside_the_core_are_violations() {
        let manifest = r#"
[dependencies]
ical-core = { workspace = true }

[dev-dependencies]
proptest = "1"
"#;
        let violations = manifest_violations("ical-recur", manifest);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations
                .first()
                .is_some_and(|line| line.contains("proptest"))
        );
    }

    #[test]
    fn recognizes_a_crate_level_declaration() {
        assert!(declares(
            "//! docs

#![no_std]
",
            "#![no_std]"
        ));
        assert!(declares(
            "#![no_std]
extern crate alloc;
",
            "extern crate alloc;"
        ));
        assert!(!declares(
            "//! docs
",
            "#![no_std]"
        ));
        assert!(
            !declares(
                "//! This crate is `#![no_std]` throughout.
",
                "#![no_std]"
            ),
            "naming the attribute in prose is not declaring it"
        );
    }
}
