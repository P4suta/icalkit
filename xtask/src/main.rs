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
//! The rule is also only as wide as the list it is applied to, so a crate under a member root
//! that declares `#![no_std]` without appearing in [`CORE_CRATES`] fails: the exemption
//! `ical-conform` holds is "not `no_std`", not "not listed". The membership walk covers every
//! root the workspace declares members under, `gates/` included, while the purity partition
//! stays over `crates/`: a directory that is not a crate must not be able to become one by
//! being somewhere this task does not look. The list is written twice, here and as the
//! `Justfile`'s `core_crates`, and the two are read against each other, because a crate in one
//! and not the other is either compiled for a bare-metal target with nothing checking what it
//! depends on or checked here and never compiled for one.
//!
//! # `purity`, second rule: the grammar layer
//!
//! `ical-core` absorbed `ical-grammar` (`docs/adr/0004`, D-0003), and inside one crate nothing
//! stops a file of `src/grammar/` from naming the model above it. `gates/grammar-layering`
//! compiles that directory with no dependencies at all and catches every spelling that names a
//! model item — `use crate::CivilDate;` fails there with a file and a line. What it cannot
//! catch is `crate::X` for an `X` the crate root re-exports *from the grammar*, because that
//! resolves in the gate too, and that is the spelling a contributor reaches for.
//!
//! So the remaining half is held here, textually: no path under `crates/ical-core/src/grammar/`
//! may resolve above the grammar root — in `mod.rs` neither `crate::` nor `super::`, in the
//! files beside it neither `crate::` nor `super::super::` — and the tree stays flat, because a
//! subdirectory changes the depth that arithmetic is stated in and a rule that quietly stops
//! applying is worse than no rule. It is in the same family as the golden-list scan and is
//! defeated by the same things: a macro, a generated path, a spelling it was not taught.
//!
//! This is hygiene about not routing a lateral import through the parent crate's public
//! surface. It is not the layering guarantee, and nothing here should be read as saying the
//! compiler enforces it.
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
//!
//! # `codes`
//!
//! `DiagnosticCode` is one vocabulary for the whole workspace and its meanings are frozen as
//! hard as its names (`docs/adr/0009`), because `docs/adr/0006`'s corpus asserts "this input
//! produces this code, on this channel" across releases. A meaning that only a doc comment
//! records is a meaning anybody can edit inside a diff nobody reads, so it is written twice —
//! once as the variant's first doc paragraph, once as a row of `docs/diagnostic-codes.md` —
//! and this task fails unless the two agree. Editing a meaning then means editing both files,
//! which is the review a frozen meaning is owed; improving the prose below a variant's first
//! paragraph stays free, because that prose is not the meaning.
//!
//! Exactly two committed files are read, `crates/ical-core/src/grammar/report.rs` and the golden
//! list, by the same kind of hand-rolled scan and for the same reason: a gate about
//! dependencies may not have any. The task fails on a code with no row, on a row no code
//! declares, on rows out of declaration order, on a meaning that drifted from either side, on
//! a channel that is not a `Severity` the source declares, and on a milestone `ROADMAP.md`
//! does not name. An addition that carries a row passes trivially, which is the one edit
//! `docs/adr/0009` allows without ceremony.
//!
//! The milestone column is the part that reads like bookkeeping and is not. `Severity` has a
//! variant and `Diagnostic` a constructor that no M0 code path reaches, nothing recorded that
//! they were waiting on M1 and M2 rather than left over from a design that moved, and both
//! were duly rediscovered as suspected dead API. So a severity that no row carries is a
//! violation here, and the milestone written against a code names who owes the emitter.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The crates that make up the sans-I/O core.
///
/// Mirrors `core_crates` in the `Justfile`, and [`recipe_violations`] reads the two against
/// each other, so a new core crate that reaches only one of them fails here rather than
/// quietly losing a gate. `ical-conform` is deliberately absent: the conformance runner is
/// allowed `std` and depends on every crate here, which is what makes it a consumer of the
/// core rather than part of it.
const CORE_CRATES: &[&str] = &[
    "ical-core",
    "ical-recur",
    "ical-tz",
    "ical-itip",
    "ical-dav",
];

/// The directories the root manifest declares members under.
///
/// The membership walk covers all of them; the purity partition covers `crates/` alone.
const MEMBER_ROOTS: [&str; 2] = ["crates", "gates"];

/// The recipe file that carries the second copy of [`CORE_CRATES`], relative to the root.
const JUSTFILE: &str = "Justfile";

/// The grammar layer's root, relative to the workspace root.
///
/// Written out rather than derived, because every message below is about positions relative to
/// this one directory: it is what `gates/grammar-layering` compiles alone.
const GRAMMAR_ROOT: &str = "crates/ical-core/src/grammar";

/// The committed golden list of diagnostic codes, relative to the workspace root.
const GOLDEN_LIST: &str = "docs/diagnostic-codes.md";

/// The declarations the golden list is checked against, relative to the workspace root.
const DIAGNOSTIC_SOURCE: &str = "crates/ical-core/src/grammar/report.rs";

/// The golden list's columns, in the order its cells are read.
const GOLDEN_COLUMNS: [&str; 4] = ["code", "meaning", "channel", "milestone"];

/// The milestones a row may name as owing the emitter. Mirrors `ROADMAP.md`.
///
/// A code whose milestone has not shipped has no emitter at all, and that is the honest
/// reading of the column rather than a defect in it.
const MILESTONES: &[&str] = &["M0", "M1", "M2", "M3", "M4", "M5"];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("purity") => report("purity", collect_purity_violations()),
        Some("codes") => report("codes", collect_codes_violations()),
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
    eprintln!("usage: cargo run -p xtask -- <purity|codes>");
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

/// Check that every core crate declares no outside dependency and stays `no_std`, that the
/// two copies of the core list agree, and that the grammar layer names nothing above itself.
fn collect_purity_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let root = workspace.join("crates");
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

    violations.extend(recipe_violations(&fs::read_to_string(
        workspace.join(JUSTFILE),
    )?));

    let mut unregistered = Vec::new();
    for member_root in MEMBER_ROOTS {
        unregistered.extend(unregistered_core_crates(&workspace.join(member_root))?);
    }
    unregistered.sort();
    violations.extend(unregistered);

    violations.extend(grammar_layer_violations(&workspace.join(GRAMMAR_ROOT))?);
    Ok(violations)
}

/// What the `Justfile`'s `core_crates` and [`CORE_CRATES`] disagree about.
///
/// The recipe is what `just no-std`, `just wasm` and the feature powerset are run over; the
/// constant is what the dependency rule is applied to. One decision, written twice, and neither
/// copy failing on its own is what would let them drift.
fn recipe_violations(justfile: &str) -> Vec<String> {
    let Some(named) = recipe_core_crates(justfile) else {
        return vec![format!(
            "{JUSTFILE}: no `core_crates := \"...\"` assignment was found, so the two copies of \
             the core list were compared against nothing (ADR 0004)"
        )];
    };

    let mut violations = Vec::new();
    for crate_name in CORE_CRATES {
        if !named.iter().any(|name| name == crate_name) {
            violations.push(format!(
                "{JUSTFILE}: `core_crates` does not name `{crate_name}`, which CORE_CRATES \
                 does; `just no-std` and `just wasm` would never compile it (ADR 0004)"
            ));
        }
    }
    for name in &named {
        if !CORE_CRATES.contains(&name.as_str()) {
            violations.push(format!(
                "{JUSTFILE}: `core_crates` names `{name}`, which CORE_CRATES does not, so no \
                 rule in this gate applies to it (ADR 0004)"
            ));
        }
    }
    violations
}

/// The crates the `Justfile`'s `core_crates` assignment names.
///
/// `None` when there is no such assignment at all, which is the difference between "the two
/// lists agree" and "one of them could not be found".
fn recipe_core_crates(justfile: &str) -> Option<Vec<String>> {
    let assignment = justfile
        .lines()
        .find(|line| line.trim_start().starts_with("core_crates"))?;
    let (_, value) = assignment.split_once(":=")?;
    let named = value.trim().strip_prefix('"')?.strip_suffix('"')?;

    let mut crates = Vec::new();
    let mut expecting = false;
    for token in named.split_whitespace() {
        if expecting {
            crates.push(token.to_owned());
        }
        expecting = token == "-p";
    }
    Some(crates)
}

/// Everything under the grammar layer that reaches out of it.
///
/// Textual, and stated over the committed directory rather than over a syntax tree, because a
/// gate about dependencies may not have any. What it costs is written in this file's header:
/// a macro, a generated path or a spelling it was not taught goes through.
fn grammar_layer_violations(root: &Path) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let directory = path.is_dir();
        let source = if directory || !is_rust_source(name) {
            String::new()
        } else {
            fs::read_to_string(&path)?
        };
        entries.push(LayerEntry {
            name: name.to_owned(),
            directory,
            source,
        });
    }
    Ok(layer_violations(&entries))
}

/// Whether a directory entry names a Rust source file.
fn is_rust_source(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

/// One entry of the grammar directory, as the rule has to see it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LayerEntry {
    /// The file or directory name, with no path in front of it.
    name: String,
    /// Whether the entry is a directory, which the flat-tree rule is about.
    directory: bool,
    /// The source of a `.rs` file, and empty for everything else.
    source: String,
}

/// The rule itself, over a listing rather than a directory.
fn layer_violations(entries: &[LayerEntry]) -> Vec<String> {
    let mut violations = Vec::new();
    let mut sources = 0usize;

    for entry in entries {
        let name = &entry.name;
        if entry.directory {
            violations.push(format!(
                "{GRAMMAR_ROOT}/{name}: the grammar tree is flat. A subdirectory changes the \
                 depth every path in this layer is stated in, and a check that quietly stops \
                 applying is worse than no check (ADR 0004)"
            ));
            continue;
        }
        if !is_rust_source(name) {
            continue;
        }
        sources = sources.saturating_add(1);
        violations.extend(file_path_violations(name, &entry.source));
    }

    if sources == 0 {
        violations.push(format!(
            "{GRAMMAR_ROOT}: no source file was found, so a scan that matched nothing would \
             have passed this rule for free (ADR 0004)"
        ));
    }
    violations.sort();
    violations
}

/// The paths in one file of the layer that resolve above the grammar root.
///
/// `mod.rs` *is* the root, so `super::` there names the crate; beside it `super::` names the
/// root and `super::super::` names the crate. Comments and string literals are removed first,
/// which is what lets a doc link keep writing `crate::Token`: the rendered documentation is
/// `ical-core`'s, and that link is how a reader reaches the item.
fn file_path_violations(name: &str, source: &str) -> Vec<String> {
    let climb = if name == "mod.rs" {
        "super::"
    } else {
        "super::super::"
    };
    let mut violations = Vec::new();
    for (number, code) in code_lines(source) {
        for path in ["crate::", climb] {
            if code.contains(path) {
                violations.push(format!(
                    "{GRAMMAR_ROOT}/{name}:{number}: `{path}` resolves above the grammar root; \
                     a path inside this layer names the layer or something under it, or the \
                     layer holds only by convention (ADR 0004)"
                ));
            }
        }
    }
    violations
}

/// One source file with its comments and literals removed, numbered from one.
///
/// A hand-rolled scan for the reason [`declared_dependencies`] gives about manifests. It reads
/// line comments, nested block comments, plain and raw string and byte-string literals, and
/// character literals — the last because `b'"'` is written in this very tree, and a scan that
/// took it for the start of a string would read everything after it in the wrong state.
fn code_lines(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut lines = Vec::new();
    let mut code = String::new();
    let mut number = 1usize;
    let mut at = 0usize;
    let mut comment = 0usize;
    let mut literal: Option<usize> = None;

    while let Some(&character) = chars.get(at) {
        if literal.is_some_and(|end| at >= end) {
            literal = None;
        }
        if character == '\n' {
            // Checked before every other state, so that a literal or a block comment spanning
            // lines does not merge the numbers a violation would be reported against.
            lines.push((number, std::mem::take(&mut code)));
            number = number.saturating_add(1);
            at = at.saturating_add(1);
        } else if literal.is_some() {
            at = at.saturating_add(1);
        } else if comment > 0 {
            let (step, depth) = block_comment_step(&chars, at, comment);
            comment = depth;
            at = at.saturating_add(step);
        } else if opens(&chars, at, '/') {
            at = at.saturating_add(1);
            while chars.get(at).is_some_and(|character| *character != '\n') {
                at = at.saturating_add(1);
            }
        } else if opens(&chars, at, '*') {
            comment = 1;
            at = at.saturating_add(2);
        } else if let Some(end) = literal_end(&chars, at) {
            literal = Some(end);
            at = at.saturating_add(1);
        } else {
            code.push(character);
            at = at.saturating_add(1);
        }
    }

    lines.push((number, code));
    lines
}

/// Whether a `/` at `at` is followed by `second`, which is what opens either comment.
fn opens(chars: &[char], at: usize, second: char) -> bool {
    chars.get(at) == Some(&'/') && chars.get(at.saturating_add(1)) == Some(&second)
}

/// How far to advance inside a block comment, and the nesting depth after doing so.
///
/// Rust's block comments nest, so a `/*` inside one is an opening rather than noise.
fn block_comment_step(chars: &[char], at: usize, depth: usize) -> (usize, usize) {
    if opens(chars, at, '*') {
        return (2, depth.saturating_add(1));
    }
    if chars.get(at) == Some(&'*') && chars.get(at.saturating_add(1)) == Some(&'/') {
        return (2, depth.saturating_sub(1));
    }
    (1, depth)
}

/// The index just past the literal beginning at `at`, or `None` if none begins there.
fn literal_end(chars: &[char], at: usize) -> Option<usize> {
    let mut index = at;
    if chars.get(index) == Some(&'b') {
        index = index.saturating_add(1);
    }
    let raw = chars.get(index) == Some(&'r');
    if raw {
        index = index.saturating_add(1);
    }
    let mut hashes = 0usize;
    while chars.get(index) == Some(&'#') {
        hashes = hashes.saturating_add(1);
        index = index.saturating_add(1);
    }
    match chars.get(index) {
        Some(&'"') => Some(string_end(chars, index.saturating_add(1), raw, hashes)),
        // A `'` opens a character literal or a lifetime, and only the first is a literal.
        Some(&'\'') if !raw && hashes == 0 => character_end(chars, index),
        _ => None,
    }
}

/// The index just past a string literal whose opening quote has already been passed.
fn string_end(chars: &[char], from: usize, raw: bool, hashes: usize) -> usize {
    let mut index = from;
    while let Some(&character) = chars.get(index) {
        if !raw && character == '\\' {
            index = index.saturating_add(2);
            continue;
        }
        index = index.saturating_add(1);
        if character == '"' && closed_by(chars, index, hashes) {
            return index.saturating_add(hashes);
        }
    }
    index
}

/// Whether `hashes` hash marks follow, which is what closes a raw literal opened with them.
fn closed_by(chars: &[char], from: usize, hashes: usize) -> bool {
    (0..hashes).all(|offset| chars.get(from.saturating_add(offset)) == Some(&'#'))
}

/// The index just past a character literal, or `None` when the `'` opened a lifetime.
fn character_end(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at.saturating_add(1)) == Some(&'\\') {
        // The escape's own length varies — `'\n'` against `'\u{1F600}'` — so the quote after
        // the escaped character is what ends it, and the escaped character is skipped first
        // because in `'\''` it is a quote.
        let mut index = at.saturating_add(3);
        while let Some(&character) = chars.get(index) {
            index = index.saturating_add(1);
            if character == '\'' {
                return Some(index);
            }
        }
        return None;
    }
    (chars.get(at.saturating_add(2)) == Some(&'\'')).then(|| at.saturating_add(3))
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

/// Crates under one member root that declare `#![no_std]` without being named in
/// [`CORE_CRATES`].
///
/// The rule is only as wide as the list it is applied to, so the list must not be able to go
/// stale behind a crate somebody added (ADR 0004). A crate that wants the exemption
/// `ical-conform` has takes it by not being `no_std`, which is what that exemption means.
/// Called for every root in [`MEMBER_ROOTS`], so that `gates/` is not a place to put one where
/// nothing looks.
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

/// Check the committed golden list against the codes the grammar declares.
fn collect_codes_violations() -> io::Result<Vec<String>> {
    let root = workspace_root()?;
    let declarations = fs::read_to_string(root.join(DIAGNOSTIC_SOURCE))?;
    let golden = fs::read_to_string(root.join(GOLDEN_LIST))?;
    Ok(codes_violations(&declarations, &golden))
}

/// Everything the declarations and the golden list disagree about.
///
/// The checks are staged, because a later one is only meaningful once the earlier one holds:
/// comparing meanings across a table whose rows and codes do not line up reports the same
/// mistake once per row and buries the one line that says what happened.
fn codes_violations(declarations: &str, golden: &str) -> Vec<String> {
    let variants = enum_variants(declarations, "DiagnosticCode");
    let arms = as_str_arms(declarations);
    let channels: Vec<String> = enum_variants(declarations, "Severity")
        .into_iter()
        .map(|(variant, _)| variant)
        .collect();

    let scanned = declaration_violations(&variants, &arms, &channels);
    if !scanned.is_empty() {
        return scanned;
    }

    let rows = match golden_rows(golden) {
        Ok(rows) => rows,
        Err(message) => return vec![message],
    };
    let codes = join_codes(&variants, &arms);
    let placement = placement_violations(&codes, &rows);
    if placement.is_empty() {
        content_violations(&codes, &rows, &channels)
    } else {
        placement
    }
}

/// What the scan of the declarations found before the golden list is opened at all.
///
/// A hand-rolled scan that stops matching reports nothing and passes, which is the failure
/// mode a gate cannot afford: an empty result here means the source moved out from under the
/// scan, not that the vocabulary is empty.
fn declaration_violations(
    variants: &[(String, String)],
    arms: &[(String, String)],
    channels: &[String],
) -> Vec<String> {
    let mut violations = Vec::new();
    if variants.is_empty() {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: no `DiagnosticCode` variant was found; a scan that matches \
             nothing would pass every remaining check for free"
        ));
    }
    if channels.is_empty() {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: no `Severity` variant was found, so every channel a row \
             names would be unrecognizable"
        ));
    }
    if !names(variants).eq(names(arms)) {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: the `DiagnosticCode` variants and the `as_str` arms are not \
             the same names in the same order; the golden list is keyed on the arms, so a \
             variant with no arm has no key to be listed under (ADR 0009)"
        ));
    }
    violations
}

/// Pair each declared variant with the key its `as_str` arm gives it.
///
/// Total only because [`declaration_violations`] has already established that the two lists
/// name the same variants in the same order.
fn join_codes(variants: &[(String, String)], arms: &[(String, String)]) -> Vec<DeclaredCode> {
    variants
        .iter()
        .zip(arms)
        .map(|((_, meaning), (_, key))| DeclaredCode {
            key: key.clone(),
            meaning: meaning.clone(),
        })
        .collect()
}

/// Which codes have a row, which rows have a code, and whether the two agree on order.
fn placement_violations(codes: &[DeclaredCode], rows: &[Row]) -> Vec<String> {
    let mut violations = Vec::new();
    for code in codes {
        let key = &code.key;
        match rows.iter().filter(|row| row.code == *key).count() {
            0 => violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is declared in {DIAGNOSTIC_SOURCE} and has no row; a \
                 code with no row is a meaning nothing holds still (ADR 0009)"
            )),
            1 => {},
            count => violations.push(format!(
                "{GOLDEN_LIST}: `{key}` has {count} rows; the row for a code is that code's \
                 meaning, and two of them are two meanings (ADR 0009)"
            )),
        }
    }
    for row in rows {
        let key = &row.code;
        if !codes.iter().any(|code| code.key == *key) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` has a row and no `DiagnosticCode` declares it; a \
                 removed code is a break, and a typo in this column quietly retires a live one \
                 (ADR 0009)"
            ));
        }
    }
    if violations.is_empty() && !row_codes(rows).eq(code_keys(codes)) {
        violations.push(format!(
            "{GOLDEN_LIST}: the rows are not in declaration order; the two files stay in one \
             order so that adding a code is one line of diff in each (ADR 0009)"
        ));
    }
    violations
}

/// What the rows claim, checked row by row against the code they are about.
///
/// Zipping is total here: [`placement_violations`] came back empty, so the rows are the codes
/// in the codes' own order.
fn content_violations(codes: &[DeclaredCode], rows: &[Row], channels: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    for (code, row) in codes.iter().zip(rows) {
        let key = &row.code;
        if row.meaning != code.meaning {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is listed as \"{listed}\" and documented as \
                 \"{documented}\"; a meaning is frozen as hard as a name, so changing one is a \
                 rename or a deprecation (ADR 0009)",
                listed = row.meaning,
                documented = code.meaning
            ));
        }
        if !channels.contains(&row.channel) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` travels on `{channel}`, which is not a `Severity` \
                 {DIAGNOSTIC_SOURCE} declares (ADR 0009)",
                channel = row.channel
            ));
        }
        if !MILESTONES.contains(&row.milestone.as_str()) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is owed by `{milestone}`, which ROADMAP.md does not \
                 name as a milestone",
                milestone = row.milestone
            ));
        }
    }
    violations.extend(unused_channel_violations(rows, channels));
    violations
}

/// Severities that no code travels on.
///
/// `Severity::LimitReached` spent the whole of M0 with no emitter and no row saying which
/// milestone owed it one, which is how it came to be rediscovered as suspected dead API. A
/// severity nothing carries is either unbuilt work with a milestone against it or a variant
/// nobody needs, and both answers belong in the table rather than in somebody's memory.
fn unused_channel_violations(rows: &[Row], channels: &[String]) -> Vec<String> {
    channels
        .iter()
        .filter(|channel| !rows.iter().any(|row| row.channel == **channel))
        .map(|channel| {
            format!(
                "{GOLDEN_LIST}: no code travels on `Severity::{channel}`; a severity with \
                 nothing on it is unbuilt work, and the milestone column is where that is \
                 written down (ADR 0009)"
            )
        })
        .collect()
}

/// One `DiagnosticCode` as the golden list has to see it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DeclaredCode {
    /// The `as_str` key, which is what the golden list is keyed on and what a conformance
    /// case names.
    key: String,
    /// The first paragraph of the variant's doc comment, as one line.
    meaning: String,
}

/// One row of the committed golden list.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    /// The `as_str` key this row is about.
    code: String,
    /// The one-line meaning, which is the variant's first doc paragraph verbatim.
    meaning: String,
    /// The `Severity` the emitter passes.
    channel: String,
    /// The milestone that owns the emission site.
    milestone: String,
}

impl Row {
    /// Read one row from its cells.
    fn read(cells: &[String]) -> Result<Self, String> {
        let [code, meaning, channel, milestone] = cells else {
            return Err(format!(
                "{GOLDEN_LIST}: a row has {count} cells rather than {expected}: {cells:?}. A \
                 meaning holding a `|` reads as two cells, so this is refused rather than \
                 guessed at",
                count = cells.len(),
                expected = GOLDEN_COLUMNS.len()
            ));
        };
        Ok(Self {
            code: code.clone(),
            meaning: meaning.clone(),
            channel: channel.clone(),
            milestone: milestone.clone(),
        })
    }
}

/// The rows of the golden list's table.
///
/// A hand-rolled scan rather than a Markdown parser, for the reason [`declared_dependencies`]
/// gives about manifests. The table is the first one in the file, and its header is checked
/// rather than skipped: the cells are read by position, so swapping two columns would
/// redefine every row at once and change nothing a reader would notice.
///
/// Structure is the error channel and content is the diagnostic one, which is the split the
/// crates themselves make: a table this cannot read at all is one message about the file
/// rather than one message per row of it.
fn golden_rows(document: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut in_table = false;

    for line in document.lines() {
        match table_cells(line) {
            None if in_table => break,
            None => header = None,
            Some(cells) if in_table => rows.push(Row::read(&cells)?),
            Some(cells) if is_rule_row(&cells) => {
                check_columns(header.as_deref())?;
                in_table = true;
            },
            Some(cells) => header = Some(cells),
        }
    }

    if !in_table {
        return Err(format!(
            "{GOLDEN_LIST}: no `{columns}` table was found, so the declarations would have been \
             compared against nothing",
            columns = GOLDEN_COLUMNS.join(" | ")
        ));
    }
    if rows.is_empty() {
        return Err(format!("{GOLDEN_LIST}: the table has a header and no rows"));
    }
    Ok(rows)
}

/// Check that the header names the columns this scan reads by position.
fn check_columns(header: Option<&[String]>) -> Result<(), String> {
    let Some(cells) = header else {
        return Err(format!(
            "{GOLDEN_LIST}: a table rule has no header row above it"
        ));
    };
    if cells.iter().map(String::as_str).eq(GOLDEN_COLUMNS) {
        return Ok(());
    }
    Err(format!(
        "{GOLDEN_LIST}: the table's columns are `{found}`, not `{expected}`; the cells are read \
         by position, so renaming or reordering one redefines every row",
        found = cells.join(" | "),
        expected = GOLDEN_COLUMNS.join(" | ")
    ))
}

/// The cells of one Markdown table row, or `None` when the line is not one.
fn table_cells(line: &str) -> Option<Vec<String>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    Some(
        inner
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect(),
    )
}

/// Whether a row is the `| --- | --- |` rule under a header rather than data.
fn is_rule_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            !cell.is_empty()
                && cell
                    .chars()
                    .all(|character| character == '-' || character == ':')
        })
}

/// The variants of one enum body, each with the first paragraph of its doc comment.
///
/// Hand-rolled over source this repository writes in one style: the enum bodies it reads hold
/// no braces of their own, so the first `}` closes the body.
fn enum_variants(source: &str, name: &str) -> Vec<(String, String)> {
    let header = format!("pub enum {name} {{");
    let mut variants = Vec::new();
    let mut meaning = Paragraph::default();
    let mut inside = false;

    for line in source.lines() {
        let line = line.trim();
        if !inside {
            inside = line == header;
        } else if line == "}" {
            break;
        } else if let Some(doc) = line.strip_prefix("///") {
            meaning.push(doc.trim());
        } else if let Some(variant) = variant_name(line) {
            variants.push((variant.to_owned(), meaning.take()));
        }
    }
    variants
}

/// A fieldless enum variant written as `Name,`.
fn variant_name(line: &str) -> Option<&str> {
    let name = line.strip_suffix(',')?;
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    plain.then_some(name)
}

/// The `variant -> key` pairs `DiagnosticCode::as_str` declares, in arm order.
///
/// Bounded to that one function: the impl block may grow another method with arms of its own,
/// and those arms are not keys.
fn as_str_arms(source: &str) -> Vec<(String, String)> {
    let mut arms = Vec::new();
    let mut inside = false;

    for line in source.lines() {
        if !inside {
            inside = line.contains("fn as_str(");
        } else if line == "}" || line.trim_start().starts_with("pub ") {
            break;
        } else if let Some(arm) = match_arm(line.trim()) {
            arms.push(arm);
        }
    }
    arms
}

/// Read `Self::Variant => "key",` as the pair it declares.
fn match_arm(line: &str) -> Option<(String, String)> {
    let (variant, rest) = line.strip_prefix("Self::")?.split_once("=>")?;
    let quoted = rest.trim().strip_suffix(',')?;
    let key = quoted.strip_prefix('"')?.strip_suffix('"')?;
    Some((variant.trim().to_owned(), key.to_owned()))
}

/// The first paragraph of a doc comment, gathered one `///` line at a time.
#[derive(Debug, Default)]
struct Paragraph {
    /// The lines gathered so far.
    text: Vec<String>,
    /// Whether a blank `///` has closed the paragraph.
    ended: bool,
}

impl Paragraph {
    /// Offer one `///` line, with the marker already stripped.
    ///
    /// Everything after the first blank line is dropped. A variant's later paragraphs are
    /// prose that gets improved, and freezing those would make an editorial fix a CI failure
    /// while doing nothing for the meaning `docs/adr/0009` actually freezes.
    fn push(&mut self, line: &str) {
        if line.is_empty() {
            self.ended = !self.text.is_empty();
        } else if !self.ended {
            self.text.push(line.to_owned());
        }
    }

    /// The paragraph as one line, leaving an empty one behind for the next variant.
    fn take(&mut self) -> String {
        self.ended = false;
        std::mem::take(&mut self.text).join(" ")
    }
}

/// The names of a scanned enum body or arm list, in order.
fn names(entries: &[(String, String)]) -> impl Iterator<Item = &str> {
    entries.iter().map(|(name, _)| name.as_str())
}

/// The keys of the declared codes, in declaration order.
fn code_keys(codes: &[DeclaredCode]) -> impl Iterator<Item = &str> {
    codes.iter().map(|code| code.key.as_str())
}

/// The codes the rows are about, in row order.
fn row_codes(rows: &[Row]) -> impl Iterator<Item = &str> {
    rows.iter().map(|row| row.code.as_str())
}

#[cfg(test)]
mod tests {
    use super::{
        LayerEntry, as_str_arms, codes_violations, collect_codes_violations,
        collect_purity_violations, declared_dependencies, declares, dependency_subtable_name,
        enum_variants, file_path_violations, golden_rows, is_dependency_table, layer_violations,
        manifest_violations, recipe_violations,
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

    /// One `.rs` file of the grammar layer, as the listing hands it over.
    fn source(name: &str, body: &str) -> LayerEntry {
        LayerEntry {
            name: name.to_owned(),
            directory: false,
            source: body.to_owned(),
        }
    }

    #[test]
    fn the_two_copies_of_the_core_list_are_read_against_each_other() {
        let recipe = format!(
            "core_crates := \"{}\"\n",
            super::CORE_CRATES
                .iter()
                .map(|name| format!("-p {name}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(recipe_violations(&recipe), Vec::<String>::new());

        let missing = recipe.replace("-p ical-tz ", "");
        assert!(
            recipe_violations(&missing)
                .iter()
                .any(|line| line.contains("does not name `ical-tz`")),
            "a core crate the recipe never compiles for a bare-metal target"
        );

        let extra = recipe.replace("core_crates := \"", "core_crates := \"-p ical-query ");
        assert!(
            recipe_violations(&extra)
                .iter()
                .any(|line| line.contains("names `ical-query`")),
            "a crate the recipe compiles and this gate states no rule about"
        );
    }

    #[test]
    fn a_recipe_with_no_core_list_is_reported_rather_than_compared_against_nothing() {
        assert!(
            recipe_violations("# a Justfile with no such assignment\n")
                .iter()
                .any(|line| line.contains("compared against nothing")),
            "the failure mode of a hand-rolled scan is matching nothing and passing"
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_grammar_layer_is_a_violation() {
        let violations = file_path_violations("token.rs", "use crate::tree::Component;\n");
        assert!(
            violations
                .iter()
                .any(|line| line.contains("token.rs:1") && line.contains("`crate::`")),
            "the file and the line are the whole point of reporting it: {violations:?}"
        );

        assert_eq!(
            file_path_violations("token.rs", "use super::Token;\n"),
            Vec::<String>::new(),
            "one `super::` beside the root names the root, which is inside the layer"
        );
        assert!(
            !file_path_violations("token.rs", "use super::super::Writer;\n").is_empty(),
            "two of them name the crate"
        );
        assert!(
            !file_path_violations("mod.rs", "use super::Writer;\n").is_empty(),
            "`mod.rs` is the root, so one `super::` there already names the crate"
        );
    }

    /// Everything the rule is stated to ignore, in the spellings this tree actually writes.
    const LAYER_PROSE: &str = r##"
// use crate::tree::Component;
/// A doc link to [`Token`](crate::Token), which is how a reader reaches the item.
/*
   crate::Token, inside a block comment /* that nests */ and closes here.
*/
const QUOTE: u8 = b'"';
const LIFETIME: fn(&str) -> &str = |value: &str| value;
const MESSAGE: &str = "crate::Token";
const RAW: &[u8] = br"crate::Token ends with \";
const HASHED: &str = r#"crate::Token "quoted" inside"#;
"##;

    #[test]
    fn a_path_written_in_a_comment_or_a_literal_is_not_a_path() {
        assert_eq!(
            file_path_violations("lexer.rs", LAYER_PROSE),
            Vec::<String>::new(),
            "the rendered documentation is `ical-core`'s, and its links have to name items"
        );
        assert!(
            !file_path_violations(
                "lexer.rs",
                &format!("{LAYER_PROSE}use crate::tree::Tree;\n")
            )
            .is_empty(),
            "and the scan must still be reading code after all of that"
        );
    }

    #[test]
    fn a_subdirectory_under_the_grammar_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\n"),
            LayerEntry {
                name: "value".to_owned(),
                directory: true,
                source: String::new(),
            },
        ];
        assert!(
            layer_violations(&entries)
                .iter()
                .any(|line| line.contains("value") && line.contains("flat")),
            "a nested file is one `super::` deeper, and the rule is stated in that arithmetic"
        );
    }

    #[test]
    fn an_empty_layer_is_reported_rather_than_passing_for_free() {
        assert!(
            layer_violations(&[])
                .iter()
                .any(|line| line.contains("no source file was found")),
            "a scan of nothing finds nothing, which is not the same as finding nothing wrong"
        );
        assert_eq!(
            layer_violations(&[source("mod.rs", "mod token;\n")]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_committed_tree_passes_every_leg_of_purity() {
        // The gate's own subject, for the reason the codes task runs against its own files:
        // the samples prove the scan reads the style they are written in, and only this proves
        // it reads the style the workspace is.
        assert_eq!(collect_purity_violations().unwrap(), Vec::<String>::new());
    }

    /// A stand-in for `report.rs`: two severities, two codes, one of them a note.
    const SAMPLE_DECLARATIONS: &str = r#"
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The specification was violated. The input was kept anyway.
    Violation,
}

#[non_exhaustive]
pub enum DiagnosticCode {
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A `^` was followed by an octet RFC 6868 gives no meaning.
    ///
    /// A note rather than a violation: RFC 6868 section 2 requires the pair to be left as it
    /// is, so the octets are what they were.
    UndefinedCaretEscape,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingValueSeparator => "missing-value-separator",
            Self::UndefinedCaretEscape => "undefined-caret-escape",
        }
    }
}
"#;

    /// The same stand-in with one code added, which is the edit ADR 0009 allows freely.
    const SAMPLE_ADDED: &str = r#"
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The specification was violated. The input was kept anyway.
    Violation,
}

#[non_exhaustive]
pub enum DiagnosticCode {
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A `^` was followed by an octet RFC 6868 gives no meaning.
    ///
    /// A note rather than a violation: RFC 6868 section 2 requires the pair to be left as it
    /// is, so the octets are what they were.
    UndefinedCaretEscape,
    /// An `END` arrived with no `BEGIN` open.
    UnmatchedEnd,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingValueSeparator => "missing-value-separator",
            Self::UndefinedCaretEscape => "undefined-caret-escape",
            Self::UnmatchedEnd => "unmatched-end",
        }
    }
}
"#;

    /// The golden list those declarations are owed.
    const SAMPLE_GOLDEN: &str = "\
# Diagnostic codes

Prose, and a `|` in it, above the table.

| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
";

    /// The sample list with one substring rewritten, which is how a drift case is written.
    fn edited(document: &str, from: &str, to: &str) -> String {
        assert!(
            document.contains(from),
            "the sample does not carry `{from}`"
        );
        document.replace(from, to)
    }

    /// The sample list with the row an added code would arrive with.
    fn golden_with_added_row() -> String {
        format!(
            "{SAMPLE_GOLDEN}| unmatched-end | An `END` arrived with no `BEGIN` open. | Violation \
             | M0 |\n"
        )
    }

    #[test]
    fn a_list_that_mirrors_the_declarations_is_clean() {
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, SAMPLE_GOLDEN),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_committed_list_matches_the_committed_declarations() {
        // The gate's own subject. Running it here rather than only in CI is what proves the
        // scan reads the style `report.rs` is written in, not the style the samples are.
        assert_eq!(collect_codes_violations().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn empty_inputs_are_reported_rather_than_passing_for_free() {
        let violations = codes_violations("", "");
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `DiagnosticCode` variant")),
            "a scan that matched nothing must say so: {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `Severity` variant")),
            "and must say so for the channel vocabulary too: {violations:?}"
        );
    }

    #[test]
    fn an_added_code_that_carries_a_row_passes_trivially() {
        assert_eq!(
            codes_violations(SAMPLE_ADDED, &golden_with_added_row()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_code_with_no_row_is_a_violation() {
        let violations = codes_violations(SAMPLE_ADDED, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`unmatched-end`") && line.contains("has no row")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_row_with_no_code_is_a_violation() {
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden_with_added_row());
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `DiagnosticCode` declares it")),
            "{violations:?}"
        );
    }

    #[test]
    fn an_edited_meaning_fails_from_whichever_side_it_was_edited() {
        let drifted = "so it has no value.";
        let listed = edited(SAMPLE_GOLDEN, "so it has a name and no value.", drifted);
        let documented = edited(
            SAMPLE_DECLARATIONS,
            "so it has a name and no value.",
            drifted,
        );
        for violations in [
            codes_violations(SAMPLE_DECLARATIONS, &listed),
            codes_violations(&documented, SAMPLE_GOLDEN),
        ] {
            assert!(
                violations
                    .iter()
                    .any(|line| line.contains("frozen as hard as a name")),
                "the two copies of a meaning are what freeze it: {violations:?}"
            );
        }
    }

    #[test]
    fn a_rename_carried_through_both_files_passes() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "\"undefined-caret-escape\"",
            "\"undefined-caret-escape-v2\"",
        );
        let golden = edited(
            SAMPLE_GOLDEN,
            "| undefined-caret-escape |",
            "| undefined-caret-escape-v2 |",
        );
        assert_eq!(
            codes_violations(&declarations, &golden),
            Vec::<String>::new(),
            "a rename is how a meaning is allowed to change (ADR 0009)"
        );
    }

    #[test]
    fn a_channel_that_is_not_a_severity_is_a_violation() {
        let golden = edited(SAMPLE_GOLDEN, "| Note | M0 |", "| Warning | M0 |");
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`Warning`") && line.contains("not a `Severity`")),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("Severity::Note")),
            "retiring the last row on a severity leaves that severity with no emitter: \
             {violations:?}"
        );
    }

    #[test]
    fn a_severity_no_code_travels_on_is_a_violation() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "    Violation,\n}",
            "    Violation,\n    /// Cut short at a caller-stated bound.\n    LimitReached,\n}",
        );
        let violations = codes_violations(&declarations, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("Severity::LimitReached")),
            "this is the debt the milestone column exists to keep closed: {violations:?}"
        );
    }

    #[test]
    fn a_milestone_the_roadmap_does_not_name_is_a_violation() {
        let golden = edited(SAMPLE_GOLDEN, "| Violation | M0 |", "| Violation | later |");
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`later`") && line.contains("ROADMAP.md")),
            "{violations:?}"
        );
    }

    #[test]
    fn rows_out_of_declaration_order_are_a_violation() {
        let golden = "\
| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
";
        let violations = codes_violations(SAMPLE_DECLARATIONS, golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("not in declaration order")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_wrapped_first_paragraph_is_one_meaning_and_the_rest_is_free_prose() {
        let source = "\
pub enum DiagnosticCode {
    /// A recurrence rule generated an instance whose date does not exist, so it was
    /// filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one.
    ///
    /// Reported so that the two answers stay different.
    NonexistentRecurrenceInstance,
}
";
        assert_eq!(
            enum_variants(source, "DiagnosticCode"),
            vec![(
                "NonexistentRecurrenceInstance".to_owned(),
                "A recurrence rule generated an instance whose date does not exist, so it was \
                 filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one."
                    .to_owned()
            )]
        );
    }

    #[test]
    fn the_arm_scan_stops_at_the_end_of_as_str() {
        let source = r#"
impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnmatchedEnd => "unmatched-end",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::UnmatchedEnd => "an END with no BEGIN",
        }
    }
}
"#;
        assert_eq!(
            as_str_arms(source),
            vec![("UnmatchedEnd".to_owned(), "unmatched-end".to_owned())],
            "only `as_str` produces golden-list keys; a later method's arms are prose"
        );
    }

    #[test]
    fn a_variant_whose_arm_is_missing_is_a_violation() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "            Self::UndefinedCaretEscape => \"undefined-caret-escape\",\n",
            "",
        );
        let violations = codes_violations(&declarations, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("same names in the same order")),
            "a variant with no key cannot be listed under one: {violations:?}"
        );
    }

    #[test]
    fn a_table_that_cannot_be_read_is_an_error_rather_than_a_row_of_diagnostics() {
        let swapped = edited(
            SAMPLE_GOLDEN,
            "| code | meaning | channel | milestone |",
            "| code | meaning | milestone | channel |",
        );
        assert!(
            golden_rows(&swapped).is_err(),
            "the cells are read by position, so a column swap is not a readable table"
        );

        let piped = edited(
            SAMPLE_GOLDEN,
            "so it has a name and no value.",
            "a name | no value.",
        );
        assert!(golden_rows(&piped).is_err(), "a five-cell row is not a row");
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &piped).len(),
            1,
            "a file that cannot be read is one message about the file, not one per row"
        );
    }
}
