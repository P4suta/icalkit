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
//! `icalkit-conformance` holds is "private tool", not "not listed". The membership walk covers every
//! root the workspace declares members under, `gates/` included, while the purity partition
//! stays over `crates/`: a directory that is not a crate must not be able to become one by
//! being somewhere this task does not look. The list is written twice, here and as the
//! `Justfile`'s `core_crates`, and the two are read against each other, because a crate in one
//! and not the other is either compiled for a bare-metal target with nothing checking what it
//! depends on or checked here and never compiled for one.
//!
//! # `purity`, second rule: the grammar layer
//!
//! The private `icalkit` kernel absorbed `ical-grammar` (`docs/adr/0004`, D-0003), and inside
//! one crate nothing stops a file of `internal/core/grammar/` from naming the model above it.
//! `gates/grammar-layering`
//! compiles that directory with no dependencies at all and catches every spelling that names a
//! model item — `use crate::CivilDate;` fails there with a file and a line. What it cannot
//! catch is `crate::X` for an `X` the crate root re-exports *from the grammar*, because that
//! resolves in the gate too, and that is the spelling a contributor reaches for.
//!
//! So the remaining half is held here, textually: no path under
//! `crates/icalkit/src/internal/core/grammar/`
//! may resolve above the grammar root — in `mod.rs` neither `crate::` nor `super::`, in the
//! files beside it neither `crate::` nor `super::super::` — and the tree stays flat, because a
//! subdirectory changes the depth that arithmetic is stated in and a rule that quietly stops
//! applying is worse than no rule.
//!
//! Four spellings were walked around before they were rules. Whitespace: `use crate ::Token;` is
//! the same import and a substring match does not see it, so every line is read with its
//! whitespace removed rather than being left to `cargo fmt`. The crate's own name: `extern crate
//! self as icalkit;` gives the layer a name for the crate root that is neither `crate::` nor
//! `super::`, so `icalkit::` is refused as a path and `extern crate` is refused outright —
//! nothing in the layer needs one, since `alloc` is declared by each root that compiles it.
//! `#[path]`: it maps a module of the layer onto a file this scan never opens, so the layer would
//! hold code no rule here applies to. And the module tree: the rule reads a directory while the
//! compiler reads `mod.rs`, so a `.rs` file the module root does not declare is invisible to
//! `gates/grammar-layering`, and the two sets are held equal in both directions.
//!
//! It is still in the same family as the golden-list scan and is still defeated by the same
//! things: a macro, a generated path, a spelling it was not taught.
//!
//! This is hygiene about not routing a lateral import through the parent crate's public
//! surface. It is not the layering guarantee, and nothing here should be read as saying the
//! compiler enforces it.
//!
//! # `purity`, third rule: the members that make the layers facts
//!
//! The compile half of a layering guarantee is a workspace member, and a member is deletable.
//! Before this rule, a pull request that dropped `gates/grammar-layering` from the member list
//! and moved its directory away passed every gate in this repository: a stale `--exclude` for a
//! package that no longer exists is not an error, and the membership walk below only ever
//! reports a directory that declares `#![no_std]`, which such a member deliberately does not. So
//! each member is named here — the member line, the package name, `publish = false`, `[lib] doc`
//! and `test`, and the `#[path]` that reaches the real sources — by string equality, which
//! `docs/adr/0004` calls narrower than a name scanner and not zero.
//!
//! There are two of them now and [`LAYERING_GATES`] is a table rather than a second pair of
//! constants, because the second layer arrived and copying the rule would have made "what a
//! layering gate is" a fact stated twice. The second is `gates/xml-layering` over
//! `crates/icalkit/src/internal/dav/xml/`, which `docs/adr/0012` requires may not name a CalDAV type: that
//! boundary is what keeps the `webdav-core` extraction that ADR declines to publish a file move
//! plus a manifest rather than a redesign. Both the textual rule above and the member rule here
//! are stated over the table, so a third layer is three strings.
//!
//! # `purity`, fourth rule: a wildcard arm over `Token`
//!
//! `Token` is `#[non_exhaustive]`, which binds outside this workspace and means nothing inside
//! it, so `unreachable_patterns = "deny"` was written down as what stops a wildcard arm from
//! being added back. It does not: that lint fires on a catch-all after every variant is already
//! covered, and a match that omits one variant and adds `_` is a *reachable* wildcard the lint
//! is silent about. It is also the only shape that loses data, and the only shape a hand
//! remembering the old cross-crate rule would write. So the arms are read here: a `match` whose
//! arm patterns name `Token::` may not also carry a `_` arm. `SyncToken::` ends in the same
//! seven characters and is a different type, so the name is matched at its boundary.
//!
//! # `purity`, fifth rule: the crate set's other copies
//!
//! `release-plz.toml` is the one file describing this workspace that no gate compiled, ran, or
//! read, and it named `ical-grammar` for a full landing after the crate ceased to exist. So the
//! published members are read out of the root manifest and held against it: one `[[package]]`
//! block each, no block for a package the workspace does not build, and `changelog_include`
//! naming every published member but the one that carries the changelog.
//!
//! The published list is also held against [`PUBLIC_CRATES`]. ADR 0013 makes `icalkit` the
//! single future registry contract. The six retired implementation package names are rejected,
//! and the conformance helper must remain explicitly classified and unpublished.
//!
//! # `architecture`
//!
//! The architecture task gives that release boundary a discoverable name and also freezes the
//! facade's Cargo feature vocabulary to `std` and `system-tz`, both enabled by default. It
//! deliberately checks declarations rather than publishability: implementation lives in
//! private modules, but publishing remains deferred until an explicit release decision.
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
//! Exactly two committed files are read,
//! `crates/icalkit/src/internal/core/grammar/report.rs` and the golden list, by the same kind of
//! hand-rolled scan and for the same reason: a gate about
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
//!
//! # `public-api`
//!
//! The facade is the workspace's sole future semver contract, so its rustdoc JSON surface is
//! committed twice: once with default features and once with no features. `cargo-public-api`
//! generates both lists and this task compares them exactly. An addition, removal, move, or
//! duplicate canonical path therefore needs an intentional snapshot edit in the same review.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The crates that make up the sans-I/O core.
///
/// Mirrors `core_crates` in the `Justfile`, and [`recipe_violations`] reads the two against
/// each other, so a new core crate that reaches only one of them fails here rather than
/// quietly losing a gate. `icalkit-conformance` is deliberately absent: the conformance runner is
/// allowed `std` and depends on every crate here, which is what makes it a consumer of the
/// core rather than part of it.
const CORE_CRATES: &[&str] = &["icalkit"];

/// The sole crate allowed to carry a future registry release contract.
const PUBLIC_CRATES: &[&str] = &["icalkit"];

/// Temporary implementation crates whose APIs are not production semver contracts.
const PRIVATE_IMPLEMENTATION: &[&str] = &[];

/// Package boundaries whose sources have moved behind the facade and may not be recreated.
const RETIRED_IMPLEMENTATION: &[&str] = &[
    "ical-core",
    "ical-dav",
    "ical-itip",
    "ical-query",
    "ical-recur",
    "ical-tz",
];

/// Narrow third-party boundaries required by the unified public facade.
///
/// This is intentionally keyed by both owner and package. Adding an external dependency to
/// another core crate, or adding another package to `icalkit`, remains a gate failure until
/// the architecture records the new boundary explicitly.
const ALLOWED_EXTERNAL_DEPENDENCIES: &[(&str, &str)] =
    &[("icalkit", "jiff"), ("icalkit", "xmlparser")];

/// Private tools isolated from the production API and release graph.
///
/// Kept separate from [`PRIVATE_IMPLEMENTATION`] so an isolation helper cannot quietly become
/// part of the production implementation graph merely because both categories are unpublished.
const PRIVATE_TOOLS: &[&str] = &["icalkit-conformance"];

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
const GRAMMAR_ROOT: &str = "crates/icalkit/src/internal/core/grammar";

/// The crate the grammar layer sits in, spelled as a path spells it.
///
/// A path may name this crate from inside the layer only by way of `extern crate self as`, which
/// is why both that declaration and this prefix are refused there.
const GRAMMAR_OWNER: &str = "icalkit";

/// One member that compiles another crate's layer with nothing above that layer in scope.
///
/// A table rather than a second pair of constants. The rule was written for exactly one member
/// and a second one arrived; copying it would have made "what a layering gate is" a fact stated
/// twice, and the third would have been stated three times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayeringGate {
    /// The member directory, relative to the workspace root.
    member: &'static str,
    /// The package name that member declares.
    package: &'static str,
    /// The layer root it compiles, relative to the workspace root.
    root: &'static str,
    /// The crate that layer sits in, spelled as a path spells it.
    owner: &'static str,
}

/// Every member that makes a layer a fact rather than a directory.
///
/// `gates/grammar-layering` compiles `icalkit`'s private content-line grammar with no model in scope
/// (`docs/adr/0004`, D-0003). `gates/xml-layering` compiles `icalkit`'s private WebDAV XML module with
/// no CalDAV vocabulary in scope, which is what keeps the extraction `docs/adr/0012` deferred a
/// file move rather than a redesign.
const LAYERING_GATES: [LayeringGate; 2] = [
    LayeringGate {
        member: "gates/grammar-layering",
        package: "ical-grammar-layering",
        root: GRAMMAR_ROOT,
        owner: GRAMMAR_OWNER,
    },
    LayeringGate {
        member: "gates/xml-layering",
        package: "ical-xml-layering",
        root: XML_ROOT,
        owner: XML_OWNER,
    },
];

/// The WebDAV XML layer's root, relative to the workspace root.
///
/// Written out for the reason [`GRAMMAR_ROOT`] is: it is what `gates/xml-layering` compiles
/// alone, and every message about it is about a position relative to this one directory.
const XML_ROOT: &str = "crates/icalkit/src/internal/dav/xml";

/// The crate the WebDAV XML layer sits in, spelled as a path spells it.
const XML_OWNER: &str = "icalkit";

/// The type a wildcard match arm would silently swallow a variant of.
const GUARDED_ENUM: &str = "Token";

/// The root manifest, relative to the workspace root.
const ROOT_MANIFEST: &str = "Cargo.toml";

/// The release configuration, relative to the workspace root.
const RELEASE_CONFIG: &str = "release-plz.toml";

/// The published member whose changelog carries the whole stack's history.
const CHANGELOG_OWNER: &str = "icalkit";

/// The committed golden list of diagnostic codes, relative to the workspace root.
const GOLDEN_LIST: &str = "docs/diagnostic-codes.md";

/// The declarations the golden list is checked against, relative to the workspace root.
const DIAGNOSTIC_SOURCE: &str = "crates/icalkit/src/internal/core/grammar/report.rs";

/// The facade's committed public API with default features.
const PUBLIC_API_DEFAULT: &str = "api/icalkit.default.txt";

/// The facade's committed public API with every default feature disabled.
const PUBLIC_API_NO_DEFAULT: &str = "api/icalkit.no-default.txt";

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
        Some("architecture") => report("architecture", collect_architecture_violations()),
        Some("codes") => report("codes", collect_codes_violations()),
        Some("public-api") => report("public-api", collect_public_api_violations()),
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
    eprintln!("usage: cargo run -p xtask -- <purity|architecture|codes|public-api>");
}

/// Generate and compare both feature profiles of the sole public crate.
fn collect_public_api_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let profiles = [
        ("default", PUBLIC_API_DEFAULT, false),
        ("no-default", PUBLIC_API_NO_DEFAULT, true),
    ];
    let mut violations = Vec::new();
    for (profile, snapshot, no_default_features) in profiles {
        let expected = normalize_api(&fs::read_to_string(workspace.join(snapshot))?);
        let generated = generate_public_api(&workspace, no_default_features)?;
        violations.extend(snapshot_violations(profile, &expected, &generated));
    }
    Ok(violations)
}

/// Ask cargo-public-api for one deterministic facade surface.
fn generate_public_api(workspace: &Path, no_default_features: bool) -> io::Result<String> {
    let mut command = Command::new("cargo");
    command.args(["public-api", "-p", "icalkit", "-sss", "--color", "never"]);
    if no_default_features {
        command.arg("--no-default-features");
    }
    let output = command
        .current_dir(workspace)
        .env("RUSTDOCFLAGS", "-D warnings")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "cargo-public-api failed for {} features: {}",
            if no_default_features {
                "no default"
            } else {
                "default"
            },
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8(output.stdout).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cargo-public-api emitted non-UTF-8 output: {error}"),
        )
    })?;
    Ok(normalize_api(&text))
}

/// Normalize host line endings and keep exactly one final line ending.
fn normalize_api(text: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    format!("{}\n", unix.trim_end_matches(['\r', '\n']))
}

/// Compare one generated surface with its committed review boundary.
fn snapshot_violations(profile: &str, expected: &str, generated: &str) -> Vec<String> {
    if expected == generated {
        return Vec::new();
    }
    let mut expected_lines = expected.lines();
    let mut generated_lines = generated.lines();
    let mut line = 1_usize;
    loop {
        let expected_line = expected_lines.next();
        let generated_line = generated_lines.next();
        if expected_line != generated_line {
            return vec![format!(
                "api/icalkit.{profile}.txt: public API differs at line {line}: expected `{expected}`, generated `{generated}`",
                expected = expected_line.unwrap_or("<end of snapshot>"),
                generated = generated_line.unwrap_or("<end of generated API>"),
            )];
        }
        if expected_line.is_none() {
            return Vec::new();
        }
        line = line.saturating_add(1);
    }
}

/// Check the single-crate release boundary and the facade's fixed feature vocabulary.
fn collect_architecture_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let mut violations = release_config_violations(&workspace)?;
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let facade = fs::read_to_string(workspace.join("crates/icalkit/Cargo.toml"))?;
    let readme = fs::read_to_string(workspace.join("README.md"))?;
    let architecture = fs::read_to_string(workspace.join("ARCHITECTURE.md"))?;
    let roadmap = fs::read_to_string(workspace.join("ROADMAP.md"))?;
    let internal_modules =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/mod.rs"))?;
    let scheduling_chapter =
        fs::read_to_string(workspace.join("crates/icalkit-conformance/src/itip.rs"))?;
    let example_path = workspace.join("crates/icalkit/examples/golden_path.rs");
    let request_writer =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/dav/write_request.rs"))?;
    let response_writer =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/dav/write_response.rs"))?;
    violations.extend(retired_crate_violations(&root, &facade));
    violations.extend(documentation_violations(&readme, &architecture));
    violations.extend(closure_status_violations(
        &roadmap,
        &internal_modules,
        &scheduling_chapter,
    ));
    violations.extend(xml_writer_violations(&request_writer, &response_writer));
    match fs::read_to_string(&example_path) {
        Ok(example) => violations.extend(basic_example_violations(&example)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => violations.push(
            "crates/icalkit/examples/golden_path.rs: missing external-consumer compile fixture"
                .to_owned(),
        ),
        Err(error) => return Err(error),
    }
    let mut features = table_keys(&facade, "features");
    features.sort();
    let mut expected = vec![
        "default".to_owned(),
        "std".to_owned(),
        "system-tz".to_owned(),
    ];
    expected.sort();
    if features != expected {
        violations.push(format!(
            "crates/icalkit/Cargo.toml: features are {features:?}, expected only {expected:?} \
             (ADR 0013)"
        ));
    }
    if !declares(&facade, "default = [\"std\", \"system-tz\"]") {
        violations.push(
            "crates/icalkit/Cargo.toml: default must enable exactly std and system-tz (ADR 0013)"
                .to_owned(),
        );
    }
    Ok(violations)
}

/// Keep current-facing status documents separate from the historical milestone narrative.
fn closure_status_violations(roadmap: &str, internal: &str, scheduling: &str) -> Vec<String> {
    const LEDGER: &str = "## Current closure ledger — 2026-08-14";
    const RETIRED_ROADMAP_PROSE: &[&str] = &[
        "still owed: a hostile input of 200,000",
        "which it currently is not",
        "no outside XML crate may be added",
        "remaining method-specific range behavior is still work",
        "One piece of internal debt is named rather than left",
    ];

    let mut violations = Vec::new();
    if !roadmap.contains(LEDGER) {
        violations.push(format!(
            "ROADMAP.md: missing current closure marker `{LEDGER}`"
        ));
    }
    for stale in RETIRED_ROADMAP_PROSE {
        if roadmap.contains(stale) {
            violations.push(format!(
                "ROADMAP.md: completed work remains described as current debt: `{stale}`"
            ));
        }
    }
    if internal.contains("including units not yet reached") {
        violations.push(
            "crates/icalkit/src/internal/mod.rs: migrated query code is still described as \
             unconnected"
                .to_owned(),
        );
    }
    for stale in [
        "# What is still owed here",
        "# Where the three big clients disagree with the table",
        "The corpus records what was observed",
    ] {
        if scheduling.contains(stale) {
            violations.push(format!(
                "crates/icalkit-conformance/src/itip.rs: stale debt or unsupported capture claim \
                 remains: `{stale}`"
            ));
        }
    }
    violations
}

/// Keep DAV request and response grammars on the one stack-balanced XML primitive.
fn xml_writer_violations(request: &str, response: &str) -> Vec<String> {
    const SHARED_IMPORT: &str = "use crate::internal::dav::writer::XmlWriter;";
    const FORBIDDEN_HELPERS: &[&str] = &[
        "fn write_root_declarations(",
        "fn write_name(",
        "fn open_extension(",
        "fn close_extension(",
        "fn empty_extension(",
    ];

    let mut violations = Vec::new();
    for (path, source) in [
        ("crates/icalkit/src/internal/dav/write_request.rs", request),
        (
            "crates/icalkit/src/internal/dav/write_response.rs",
            response,
        ),
    ] {
        if !source.lines().any(|line| line.trim() == SHARED_IMPORT) {
            violations.push(format!(
                "{path}: DAV body encoder does not use the shared stack-balanced XmlWriter"
            ));
        }
        for helper in FORBIDDEN_HELPERS {
            if source.contains(helper) {
                violations.push(format!(
                    "{path}: duplicate XML structural helper {helper} bypasses XmlWriter"
                ));
            }
        }
    }
    violations
}

/// Keep the first consumer example at the workflow layer rather than exposing implementation
/// machinery a normal application never needs to name.
fn basic_example_violations(example: &str) -> Vec<String> {
    const INTERNAL_VOCABULARY: &[&str] = &[
        "Token",
        "Xml",
        "XML",
        "Limits",
        "Meter",
        "Sink",
        "DiagnosticSink",
        "RFC table",
    ];
    INTERNAL_VOCABULARY
        .iter()
        .filter(|word| example.contains(*word))
        .map(|word| {
            format!(
                "crates/icalkit/examples/golden_path.rs: basic example exposes internal vocabulary `{word}` (ADR 0014)"
            )
        })
        .collect()
}

/// Hold the public guide to the same typestate and workflow order as the API.
fn documentation_violations(readme: &str, architecture: &str) -> Vec<String> {
    const GOLDEN_PATH: &[&str] = &[
        "## Strict parsing",
        "## Explicit normalization",
        "## Transactional editing",
        "## DST-aware recurrence",
        "## iTIP scheduling",
        "## CalDAV sync and server workflows",
    ];
    const RETIRED_PROSE: &[&str] = &[
        "one temporary path dependency",
        "unpublished scaffolding",
        "temporary compatibility harness",
        "remaining `ical-core` package",
    ];

    let mut violations = Vec::new();
    let mut previous = None;
    for heading in GOLDEN_PATH {
        let Some(at) = readme.find(heading) else {
            violations.push(format!(
                "README.md: missing golden-path heading `{heading}` (ADR 0014)"
            ));
            continue;
        };
        if previous.is_some_and(|before| at <= before) {
            violations.push(format!(
                "README.md: `{heading}` is out of golden-path order (ADR 0014)"
            ));
        }
        previous = Some(at);
    }
    for stale in RETIRED_PROSE {
        if architecture.contains(stale) || readme.contains(stale) {
            violations.push(format!(
                "README.md/ARCHITECTURE.md: retired migration prose `{stale}` remains (ADR 0014)"
            ));
        }
    }
    violations
}

/// References that would recreate a retired implementation package boundary.
fn retired_crate_violations(root: &str, facade: &str) -> Vec<String> {
    let workspace_members = members(root);
    let dependencies = declared_dependencies(facade);
    let mut violations = Vec::new();
    for retired in RETIRED_IMPLEMENTATION {
        let old_member = format!("crates/{retired}");
        if workspace_members.iter().any(|member| member == &old_member) {
            violations.push(format!(
                "{ROOT_MANIFEST}: retired implementation package `{retired}` remains a \
                 workspace member (ADR 0013)"
            ));
        }
        if dependencies
            .iter()
            .any(|dependency| dependency.package == *retired)
        {
            violations.push(format!(
                "crates/icalkit/Cargo.toml: facade still depends on retired implementation \
                 package `{retired}` (ADR 0013)"
            ));
        }
    }
    violations
}

/// Assignment keys in one top-level TOML table.
fn table_keys(document: &str, table: &str) -> Vec<String> {
    let wanted = format!("[{table}]");
    let mut inside = false;
    let mut keys = Vec::new();
    for line in document.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == wanted;
        } else if inside {
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim();
                if !key.is_empty() {
                    keys.push(key.to_owned());
                }
            }
        }
    }
    keys
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

    for gate in LAYERING_GATES {
        violations.extend(layer_root_violations(gate, &workspace.join(gate.root))?);
        violations.extend(layering_member_violations(&workspace, gate)?);
    }
    violations.extend(guarded_match_violations(&workspace)?);
    violations.extend(release_config_violations(&workspace)?);
    Ok(violations)
}

/// What the workspace says about the member that compiles the grammar alone.
///
/// That member is the layering guarantee. Deleted, the grammar is a directory inside `ical-core`
/// again and `use crate::CivilDate;` from inside it compiles; nothing else here would notice,
/// because every other mention of it is a comment, an `--exclude` that is not an error once the
/// package is gone, or a membership walk that reports only a directory declaring `#![no_std]`.
/// So each part the guarantee rests on is named and compared: the member line, the package name,
/// `publish = false`, the two `[lib]` switches that keep the grammar's tests and docs from being
/// counted twice, and the `#[path]` that reaches the real sources rather than a copy.
fn layering_member_violations(workspace: &Path, gate: LayeringGate) -> io::Result<Vec<String>> {
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let directory = workspace.join(gate.member);
    let manifest_path = directory.join("Cargo.toml");
    let lib_path = directory.join("src").join("lib.rs");
    let member = if manifest_path.is_file() && lib_path.is_file() {
        Some((
            fs::read_to_string(&manifest_path)?,
            fs::read_to_string(&lib_path)?,
        ))
    } else {
        None
    };
    Ok(layering_violations(
        gate,
        &root,
        member
            .as_ref()
            .map(|(manifest, lib)| (manifest.as_str(), lib.as_str())),
    ))
}

/// The rule itself, over the three files' text rather than over the workspace.
fn layering_violations(
    gate: LayeringGate,
    root: &str,
    member: Option<(&str, &str)>,
) -> Vec<String> {
    let LayeringGate {
        member: directory,
        package,
        root: layer,
        ..
    } = gate;
    let mut violations = Vec::new();
    if !members(root).iter().any(|name| name == directory) {
        violations.push(format!(
            "{ROOT_MANIFEST}: does not register `{directory}`; that member is what makes \
             `{layer}` a layer rather than a directory, and it is nothing once it is not built \
             (ADR 0004)"
        ));
    }

    let Some((manifest, lib)) = member else {
        violations.push(format!(
            "{directory}: has no Cargo.toml and src/lib.rs; the compile half of the layering \
             guarantee over `{layer}` is this member and nothing else (ADR 0004)"
        ));
        return violations;
    };

    for declaration in [
        format!("name = \"{package}\""),
        "publish = false".to_owned(),
        "doc = false".to_owned(),
        "test = false".to_owned(),
    ] {
        if !declares(manifest, &declaration) {
            violations.push(format!(
                "{directory}/Cargo.toml: does not declare `{declaration}`; the member is a gate \
                 rather than a crate, and without both `[lib]` switches the layer's tests and \
                 documentation are counted twice (ADR 0004)"
            ));
        }
    }

    // Three `../`, not two: `#[path]` on a `mod` in `src/lib.rs` resolves relative to `src/`.
    let expected = format!("#[path = \"../../../{layer}/mod.rs\"]");
    if !declares(lib, &expected) {
        violations.push(format!(
            "{directory}/src/lib.rs: does not declare `{expected}`; a gate that compiles \
             anything other than the shipped sources proves something about a copy (ADR 0004)"
        ));
    }
    violations
}

/// Every wildcard arm over the guarded type, across the crates.
///
/// Stated over `crates/` rather than over `ical-core` alone: the type is re-exported at that
/// crate's root, so any crate that names it can write the arm this rule is about.
fn guarded_match_violations(workspace: &Path) -> io::Result<Vec<String>> {
    let mut violations = Vec::new();
    let mut sources = Vec::new();
    collect_rust_sources(&workspace.join("crates"), &mut sources)?;
    sources.sort();
    for path in sources {
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(match_arm_violations(&label, &fs::read_to_string(&path)?));
    }
    Ok(violations)
}

/// Every `.rs` file under `root`, at any depth, gathered into `sources`.
fn collect_rust_sources(root: &Path, sources: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_sources(&path, sources)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_rust_source)
        {
            sources.push(path);
        }
    }
    Ok(())
}

/// One `match` block whose arms are being read.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenMatch {
    /// The brace depth this block's arms sit at.
    arms: usize,
    /// Whether an arm pattern names the guarded type.
    guarded: bool,
    /// The lines a wildcard arm was written on.
    wildcards: Vec<usize>,
}

/// The wildcard arms of every match over the guarded type in one file.
///
/// Brace depth is counted over the code with comments and literals already removed, and an arm is
/// a line that begins at the depth the block's arms sit at. What that costs is a match whose
/// scrutinee is written across lines, because the rule finds a block by the `match` and the brace
/// arriving together; what it buys is a rule with no parser and no dependency, which is the trade
/// every scan in this file makes.
fn match_arm_violations(label: &str, source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let mut open: Vec<OpenMatch> = Vec::new();
    let mut depth = 0usize;

    for (number, code) in code_lines(source) {
        while open.last().is_some_and(|block| depth < block.arms) {
            if let Some(block) = open.pop() {
                violations.extend(closed_match_violations(label, &block));
            }
        }
        let line = code.trim();
        if let Some(block) = open.last_mut() {
            if depth == block.arms {
                let pattern = line.split("=>").next().unwrap_or(line).trim();
                block.guarded |= names_path(pattern, &format!("{GUARDED_ENUM}::"));
                if is_wildcard(pattern) {
                    block.wildcards.push(number);
                }
            }
        }

        let inner = depth
            .saturating_add(code.matches('{').count())
            .saturating_sub(code.matches('}').count());
        if inner > depth && line.contains("match ") && line.ends_with('{') {
            open.push(OpenMatch {
                arms: inner,
                guarded: false,
                wildcards: Vec::new(),
            });
        }
        depth = inner;
    }

    while let Some(block) = open.pop() {
        violations.extend(closed_match_violations(label, &block));
    }
    violations.sort();
    violations
}

/// What one finished match block is owed, once every arm of it has been read.
fn closed_match_violations(label: &str, block: &OpenMatch) -> Vec<String> {
    if !block.guarded {
        return Vec::new();
    }
    block
        .wildcards
        .iter()
        .map(|number| {
            format!(
                "{label}:{number}: a wildcard arm over `{GUARDED_ENUM}`. The attribute that makes \
                 adding a variant a minor release binds outside this workspace and means nothing \
                 inside it, so a `_` here is a variant whose payload is dropped with no compiler \
                 error rather than forward compatibility (ADR 0004)"
            )
        })
        .collect()
}

/// Whether a match arm's pattern is a catch-all.
fn is_wildcard(pattern: &str) -> bool {
    pattern == "_"
        || pattern
            .strip_prefix("_ ")
            .is_some_and(|guard| guard.trim_start().starts_with("if "))
}

/// What `release-plz.toml` and the workspace's own members disagree about.
fn release_config_violations(workspace: &Path) -> io::Result<Vec<String>> {
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let mut published = Vec::new();
    let mut private_tools = Vec::new();
    let mut violations = Vec::new();
    for member in members(&root) {
        let manifest = fs::read_to_string(workspace.join(&member).join("Cargo.toml"))?;
        match package_name(&manifest) {
            Some(name) if declares(&manifest, "publish = false") => {
                if Path::new(&member).starts_with("crates") {
                    private_tools.push(name);
                }
            },
            Some(name) => published.push(name),
            None => violations.push(format!(
                "{member}/Cargo.toml: declares no package name, so the release configuration \
                 could not be read against it"
            )),
        }
    }
    violations.extend(core_list_violations(&published));
    violations.extend(private_crate_violations(&private_tools));
    violations.extend(release_violations(
        &published,
        &fs::read_to_string(workspace.join(RELEASE_CONFIG))?,
    ));
    Ok(violations)
}

/// What [`PUBLIC_CRATES`] and the workspace's own published members disagree about.
fn core_list_violations(published: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    for name in published {
        if !PUBLIC_CRATES.contains(&name.as_str()) {
            violations.push(format!(
                "{ROOT_MANIFEST}: publishes {name}, but ADR 0013 permits only the icalkit facade"
            ));
        }
    }
    for name in PUBLIC_CRATES {
        if !published.iter().any(|member| member == *name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: does not leave {name} as the sole future public crate (ADR 0013)"
            ));
        }
    }
    violations.sort();
    violations
}

/// What the two explicit private categories and unpublished members under `crates/` disagree
/// about.
fn private_crate_violations(private: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    let expected: Vec<&str> = PRIVATE_TOOLS
        .iter()
        .chain(PRIVATE_IMPLEMENTATION)
        .copied()
        .collect();

    for name in PRIVATE_TOOLS {
        if CORE_CRATES.contains(name) || PUBLIC_CRATES.contains(name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: `{name}` is both a private tool and a published crate; those \
                 release contracts are mutually exclusive"
            ));
        }
    }
    for name in &expected {
        if !private.iter().any(|member| member == *name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: private member `{name}` is missing or publishable (ADR 0013)"
            ));
        }
    }

    for name in private {
        if !expected.contains(&name.as_str()) {
            violations.push(format!(
                "{ROOT_MANIFEST}: private crate `{name}` is unclassified; add it to the explicit \
                 private implementation or tool set"
            ));
        }
    }

    violations.sort();
    violations
}

/// The rule itself, over the two lists rather than the two files.
///
/// The release path is the one path this repository never runs, so its configuration is the one
/// place a deleted crate can survive a whole landing. Every published member gets one entry and
/// one line of the changelog list; a package the workspace does not build gets neither.
fn release_violations(published: &[String], config: &str) -> Vec<String> {
    let released = release_packages(config);
    let included = quoted_values(array_value(config, "changelog_include").unwrap_or_default());
    let mut violations = Vec::new();

    for name in published {
        match released.iter().filter(|entry| *entry == name).count() {
            1 => {},
            0 => violations.push(format!(
                "{RELEASE_CONFIG}: `{name}` is a published member with no `[[package]]` block; \
                 the crates move as one version group, and a member outside it is a member with \
                 its own release story"
            )),
            count => violations.push(format!(
                "{RELEASE_CONFIG}: `{name}` has {count} `[[package]]` blocks; the last one wins \
                 silently, which is how a setting comes to be read by nobody"
            )),
        }
        let listed = included.iter().any(|entry| entry == name);
        if name == CHANGELOG_OWNER && listed {
            violations.push(format!(
                "{RELEASE_CONFIG}: `changelog_include` names `{CHANGELOG_OWNER}`, which is the \
                 crate carrying the changelog rather than a crate folded into it"
            ));
        } else if name != CHANGELOG_OWNER && !listed {
            violations.push(format!(
                "{RELEASE_CONFIG}: `changelog_include` does not name `{name}`, so that crate's \
                 history would be missing from the one changelog this stack has"
            ));
        }
    }

    for name in released.iter().chain(&included) {
        if !published.contains(name) {
            violations.push(format!(
                "{RELEASE_CONFIG}: names `{name}`, which this workspace does not publish; a \
                 release configuration describing a crate set that no longer exists is a release \
                 nobody can run"
            ));
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

/// The packages `release-plz.toml` declares a `[[package]]` block for, in file order.
fn release_packages(config: &str) -> Vec<String> {
    let mut packages = Vec::new();
    let mut inside = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[[package]]";
        } else if inside {
            if let Some(name) = quoted_after(line, "name") {
                packages.push(name.to_owned());
                inside = false;
            }
        }
    }
    packages
}

/// The `name = "..."` a `[package]` table declares.
fn package_name(manifest: &str) -> Option<String> {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[package]";
        } else if inside {
            if let Some(name) = quoted_after(line, "name") {
                return Some(name.to_owned());
            }
        }
    }
    None
}

/// The value of `key = "..."` on one line, or `None` when the line assigns something else.
fn quoted_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = line.split_once('=')?;
    (name.trim() == key).then(|| value.trim().trim_matches('"'))
}

/// The members the root manifest registers.
fn members(manifest: &str) -> Vec<String> {
    quoted_values(array_value(manifest, "members").unwrap_or_default())
}

/// The text of a `key = [ ... ]` array, however many lines it is written across.
fn array_value<'a>(document: &'a str, key: &str) -> Option<&'a str> {
    let at = document.find(&format!("{key} = ["))?;
    let rest = document.get(at..)?;
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    rest.get(open.saturating_add(1)..close)
}

/// Every double-quoted string in one piece of text.
fn quoted_values(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
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
fn layer_root_violations(gate: LayeringGate, root: &Path) -> io::Result<Vec<String>> {
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
    Ok(layer_violations(gate, &entries))
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
fn layer_violations(gate: LayeringGate, entries: &[LayerEntry]) -> Vec<String> {
    let layer = gate.root;
    let mut violations = Vec::new();
    let mut sources = 0usize;
    let mut root = None;
    let mut beside = Vec::new();

    for entry in entries {
        let name = &entry.name;
        if entry.directory {
            violations.push(format!(
                "{layer}/{name}: a layer's tree is flat. A subdirectory changes the depth every \
                 path in this layer is stated in, and a check that quietly stops applying is \
                 worse than no check (ADR 0004)"
            ));
            continue;
        }
        if !is_rust_source(name) {
            continue;
        }
        sources = sources.saturating_add(1);
        violations.extend(file_path_violations(gate, name, &entry.source));
        if name == "mod.rs" {
            root = Some(entry.source.as_str());
        } else {
            beside.push(name.as_str());
        }
    }

    if sources == 0 {
        violations.push(format!(
            "{layer}: no source file was found, so a scan that matched nothing would have passed \
             this rule for free (ADR 0004)"
        ));
    } else {
        violations.extend(module_tree_violations(layer, root, &beside));
    }
    violations.sort();
    violations
}

/// What the module root declares against what the directory holds.
///
/// Two gates read this layer and they read it differently: `gates/grammar-layering` compiles what
/// `mod.rs` declares, and the rule above scans what the directory holds. A `.rs` file the module
/// root never declares is therefore in exactly one of them — the textual half sees it, the
/// compile half cannot — and a file pulled in from elsewhere by `#[path]` is in the other. Both
/// are closed by holding the two sets equal in both directions; `#[path]` itself is refused a few
/// lines below, because it is what lets the equality hold while the layer still gains a file.
fn module_tree_violations(layer: &str, root: Option<&str>, beside: &[&str]) -> Vec<String> {
    let Some(source) = root else {
        return vec![format!(
            "{layer}: has no mod.rs, so the files beside it were compared against no module tree \
             at all (ADR 0004)"
        )];
    };

    let declared = declared_modules(source);
    let mut violations = Vec::new();
    for name in beside {
        let stem = name.strip_suffix(".rs").unwrap_or(name);
        if !declared.iter().any(|module| module == stem) {
            violations.push(format!(
                "{layer}/{name}: mod.rs declares no `mod {stem};`, so the file is part of this \
                 layer for one of the two gates that read it and invisible to the other \
                 (ADR 0004)"
            ));
        }
    }
    for module in &declared {
        if !beside.iter().any(|name| *name == format!("{module}.rs")) {
            violations.push(format!(
                "{layer}/mod.rs: declares `mod {module};` and there is no {module}.rs beside it; \
                 the module resolves somewhere this rule does not read (ADR 0004)"
            ));
        }
    }
    violations
}

/// The modules one file declares as files of its own directory.
fn declared_modules(source: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for (_, code) in code_lines(source) {
        let Some(declaration) = code.trim().strip_suffix(';') else {
            continue;
        };
        let named = declaration
            .trim_start_matches("pub(crate)")
            .trim_start_matches("pub")
            .trim_start();
        if let Some(name) = named.strip_prefix("mod ") {
            modules.push(name.trim().to_owned());
        }
    }
    modules
}

/// The paths in one file of the layer that resolve above the grammar root.
///
/// `mod.rs` *is* the root, so `super::` there names the crate; beside it `super::` names the
/// root and `super::super::` names the crate. Comments and string literals are removed first,
/// which is what lets a doc link keep writing `crate::Token`: the rendered documentation is
/// `ical-core`'s, and that link is how a reader reaches the item.
///
/// Each line is then read with the whitespace around its `::` closed up, because `use crate
/// ::Token;` is the same import as `use crate::Token;` and only `cargo fmt` stood between the two.
/// The whitespace elsewhere on the line stays, because it is what tells `crate::` apart from the
/// tail of `subcrate::`. Two declarations are refused outright beside the paths: `extern crate`,
/// which is how the layer would acquire a name for the crate above it that is spelled like
/// neither of them, and `#[path]`, which is how it would acquire a file this scan never opens.
fn file_path_violations(gate: LayeringGate, name: &str, source: &str) -> Vec<String> {
    let LayeringGate {
        root: layer, owner, ..
    } = gate;
    let climb = if name == "mod.rs" {
        "super::"
    } else {
        "super::super::"
    };
    let prefix = format!("{owner}::");
    let mut violations = Vec::new();
    for (number, line) in code_lines(source) {
        let text = spaced(&line);
        let code = tightened(&text);
        for path in ["crate::", climb, prefix.as_str()] {
            if names_path(&code, path) {
                violations.push(format!(
                    "{layer}/{name}:{number}: `{path}` resolves above this layer's root; a path \
                     inside the layer names the layer or something under it, or the layer holds \
                     only by convention (ADR 0004)"
                ));
            }
        }
        if text.contains("extern crate") {
            violations.push(format!(
                "{layer}/{name}:{number}: `extern crate` gives this layer a second name for \
                 something above it — `extern crate self as {owner};` names the crate root in a \
                 spelling no path rule sees. `alloc` is declared by each root that compiles these \
                 sources, so nothing here needs one (ADR 0004)"
            ));
        }
        if text.replace(' ', "").contains("#[path") {
            violations.push(format!(
                "{layer}/{name}:{number}: `#[path]` maps a module of this layer onto a file \
                 outside the directory this rule reads, so the layer would hold code no rule here \
                 applies to (ADR 0004)"
            ));
        }
    }
    violations
}

/// One line with every run of whitespace closed up to a single space.
///
/// `extern  crate` is `extern crate`, and a rule that reads the second and not the first is a
/// rule held by `cargo fmt`.
fn spaced(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One spaced line with the whitespace around each `::` removed.
///
/// `use crate ::Token;` compiles and means what `use crate::Token;` means. Only the space beside
/// the separator goes: the rest is what distinguishes `use crate::x` from `use subcrate::x`, and
/// removing all of it would make every path the tail of the word in front of it.
fn tightened(spaced: &str) -> String {
    spaced.replace(" ::", "::").replace(":: ", "::")
}

/// Whether `needle` occurs in `code` as something other than the tail of a longer name.
///
/// `SyncToken::` ends in `Token::` and is a different type. `$crate::` does not end in a name
/// character, and is the macro spelling of the path being refused rather than a different one.
fn names_path(code: &str, needle: &str) -> bool {
    code.match_indices(needle).any(|(at, _)| {
        code.get(..at)
            .and_then(|before| before.chars().next_back())
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
    })
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
        let is_core = CORE_CRATES.contains(&package.as_str());
        let is_recorded_boundary =
            ALLOWED_EXTERNAL_DEPENDENCIES.contains(&(crate_name, package.as_str()));
        if !is_core && !is_recorded_boundary {
            violations.push(format!(
                "{crate_name}: declares `{package}`; a core crate may depend only on other \
                 core crates or an explicitly recorded facade boundary (ADR 0013)"
            ));
        } else if is_core && !dependency.local {
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
/// stale behind a crate somebody added (ADR 0004). `icalkit-conformance` is instead admitted by
/// [`PRIVATE_TOOLS`]; it uses `std` and is not part of the production or release graph.
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
    let mut inline_table: Option<(String, String)> = None;
    let mut inside_dependency_table = false;

    for line in manifest.lines() {
        let line = line.trim();
        let continuing_inline_table = inline_table.is_some();
        let mut completed_inline_table = false;
        if let Some((_, spec)) = inline_table.as_mut() {
            if !line.is_empty() && !line.starts_with('#') {
                spec.push(' ');
                spec.push_str(line);
            }
            if inline_table_is_complete(spec) {
                completed_inline_table = true;
            }
        }
        if completed_inline_table {
            if let Some((key, spec)) = inline_table.take() {
                declared.push(Declared::read(&key, &spec));
            }
        }
        if continuing_inline_table {
            continue;
        }
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
                    if spec.trim_start().starts_with('{') && !inline_table_is_complete(spec) {
                        inline_table = Some((key.to_owned(), spec.trim().to_owned()));
                    } else {
                        declared.push(Declared::read(key, spec));
                    }
                }
            }
        }
    }

    if let Some((key, spec)) = inline_table {
        declared.push(Declared::read(&key, &spec));
    }
    flush_subtable(&mut subtable, &mut declared);
    declared.sort();
    declared
}

/// Whether a possibly wrapped inline TOML table has reached its closing brace.
fn inline_table_is_complete(spec: &str) -> bool {
    let mut braces = 0usize;
    let mut quoted = false;
    let mut literal = false;
    let mut escaped = false;
    for character in spec.chars() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        if literal {
            if character == '\'' {
                literal = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '\'' => literal = true,
            '{' => braces = braces.saturating_add(1),
            '}' => braces = braces.saturating_sub(1),
            _ => {},
        }
    }
    braces == 0
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
        LAYERING_GATES, LayerEntry, LayeringGate, as_str_arms, basic_example_violations,
        closure_status_violations, codes_violations, collect_architecture_violations,
        collect_codes_violations, collect_purity_violations, core_list_violations,
        declared_dependencies, declares, dependency_subtable_name, documentation_violations,
        enum_variants, file_path_violations, golden_rows, is_dependency_table, layer_violations,
        layering_violations, manifest_violations, match_arm_violations, private_crate_violations,
        recipe_violations, release_violations, retired_crate_violations, snapshot_violations,
        table_keys, xml_writer_violations,
    };

    /// The grammar layer's gate, which every layer rule below is stated over.
    ///
    /// The rule takes the gate rather than reading a constant, so the tests name one: what they
    /// assert is the rule, and the second gate differs from this one only in its three strings.
    const GATE: LayeringGate = LAYERING_GATES[0];

    /// [`file_path_violations`] over the grammar gate.
    fn file_paths(name: &str, source: &str) -> Vec<String> {
        file_path_violations(GATE, name, source)
    }

    /// [`layer_violations`] over the grammar gate.
    fn layers(entries: &[LayerEntry]) -> Vec<String> {
        layer_violations(GATE, entries)
    }

    /// [`layering_violations`] over the grammar gate.
    fn layering(root: &str, member: Option<(&str, &str)>) -> Vec<String> {
        layering_violations(GATE, root, member)
    }

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
    fn recorded_facade_dependencies_are_no_violation() {
        let manifest = r#"
[package]
name = "icalkit"

[dependencies]
jiff = { version = "0.2.35", default-features = false }
xmlparser = { version = "0.13.6", default-features = false }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_formatted_multiline_path_dependency_is_still_local() {
        let manifest = r#"
[dependencies]
icalkit = { path = "../icalkit", version = "0.0.0", features = [
  "std",
] }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "taplo may wrap a local feature-bearing inline table without changing its provenance"
        );
    }

    #[test]
    fn only_the_facade_may_expose_the_jiff_boundary() {
        let manifest = r#"
[dependencies]
jiff = { version = "0.2.35", default-features = false, features = ["alloc"] }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "the unified facade owns the public civil-time boundary"
        );
        assert!(
            manifest_violations("icalkit-conformance", manifest)
                .iter()
                .any(|line| line.contains("jiff")),
            "an isolation tool must not acquire a second public time boundary"
        );
    }

    #[test]
    fn only_the_facade_may_own_the_private_xml_lexer() {
        let manifest = r#"
[dependencies]
xmlparser = { version = "0.13.6", default-features = false }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "the facade owns the private lexer wrapper"
        );
        assert!(
            manifest_violations("icalkit-conformance", manifest)
                .iter()
                .any(|line| line.contains("xmlparser")),
            "an isolation tool must record its dependency independently"
        );
    }

    #[test]
    fn a_core_crate_taken_from_a_registry_is_a_violation() {
        let manifest = "[dependencies]
icalkit = \"0.1\"
";
        assert!(
            manifest_violations("icalkit", manifest)
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
icalkit = { path = "../icalkit" }

[dev-dependencies]
proptest = "1"
"#;
        let violations = manifest_violations("icalkit", manifest);
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

        let missing = recipe.replace("-p icalkit", "");
        assert!(
            recipe_violations(&missing)
                .iter()
                .any(|line| line.contains("does not name `icalkit`")),
            "a core crate the recipe never compiles for a bare-metal target"
        );

        // `ical-query` used to be the crate that did not exist here and now does, which is the
        // drift this leg exists to catch: a name in one copy of the list and not the other.
        let extra = recipe.replace("core_crates := \"", "core_crates := \"-p ical-carddav ");
        assert!(
            recipe_violations(&extra)
                .iter()
                .any(|line| line.contains("names `ical-carddav`")),
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
        let violations = file_paths("token.rs", "use crate::tree::Component;\n");
        assert!(
            violations
                .iter()
                .any(|line| line.contains("token.rs:1") && line.contains("`crate::`")),
            "the file and the line are the whole point of reporting it: {violations:?}"
        );

        assert_eq!(
            file_paths("token.rs", "use super::Token;\n"),
            Vec::<String>::new(),
            "one `super::` beside the root names the root, which is inside the layer"
        );
        assert!(
            !file_paths("token.rs", "use super::super::Writer;\n").is_empty(),
            "two of them name the crate"
        );
        assert!(
            !file_paths("mod.rs", "use super::Writer;\n").is_empty(),
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
            file_paths("lexer.rs", LAYER_PROSE),
            Vec::<String>::new(),
            "the rendered documentation is `ical-core`'s, and its links have to name items"
        );
        assert!(
            !file_paths(
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
            source("token.rs", ""),
            LayerEntry {
                name: "value".to_owned(),
                directory: true,
                source: String::new(),
            },
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("value") && line.contains("flat")),
            "a nested file is one `super::` deeper, and the rule is stated in that arithmetic"
        );
    }

    #[test]
    fn an_empty_layer_is_reported_rather_than_passing_for_free() {
        assert!(
            layers(&[])
                .iter()
                .any(|line| line.contains("no source file was found")),
            "a scan of nothing finds nothing, which is not the same as finding nothing wrong"
        );
        assert_eq!(
            layers(&[source("mod.rs", "mod token;\n"), source("token.rs", "")]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_file_the_module_root_does_not_declare_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\n"),
            source("token.rs", ""),
            source("sneak.rs", "pub(crate) const HIDDEN: u8 = 0;\n"),
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("sneak.rs") && line.contains("mod sneak;")),
            "an undeclared file is scanned by this rule and never compiled by the layering member"
        );
    }

    #[test]
    fn a_module_the_directory_does_not_hold_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\nmod launder;\n"),
            source("token.rs", ""),
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("mod launder;") && line.contains("launder.rs")),
            "a module with no file beside it resolves somewhere this rule does not read"
        );
    }

    #[test]
    fn a_path_attribute_inside_the_layer_is_a_violation() {
        assert!(
            file_paths("mod.rs", "#[path = \"../launder.rs\"]\nmod launder;\n")
                .iter()
                .any(|line| line.contains("`#[path]`")),
            "`#[path]` pulls a file into the layer that the directory listing never shows"
        );
    }

    #[test]
    fn an_extern_crate_declaration_inside_the_layer_is_a_violation() {
        let violations = file_paths(
            "token.rs",
            "extern crate self as icalkit;\nuse icalkit::Token as Laundered;\n",
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`extern crate`")),
            "the declaration is what makes the spelling below available: {violations:?}"
        );
        assert!(
            violations.iter().any(|line| line.contains("`icalkit::`")),
            "and the crate's own name is a path above the layer like any other: {violations:?}"
        );
    }

    #[test]
    fn whitespace_does_not_hide_a_path_that_climbs_out() {
        assert!(
            !file_paths("lexer.rs", "use crate ::Token as Laundered;\n").is_empty(),
            "the same import with one space in it, which only `cargo fmt` was catching"
        );
        assert!(
            !file_paths("token.rs", "use super :: super ::tree::Component;\n").is_empty(),
            "and the climbing spelling, which rustfmt would also have normalized"
        );
    }

    /// A match over the guarded type, in the shape `parse.rs` and `mutate.rs` write one.
    const EXHAUSTIVE_MATCH: &str = "\
fn take(token: Token<'_>) -> usize {
    match token {
        Token::Name(bytes) => bytes.len(),
        Token::Parameter {
            name,
            value,
            has_value,
        } => name.len().saturating_add(value.len()).saturating_add(usize::from(has_value)),
        Token::Value { bytes, .. } => bytes.len(),
        Token::EndOfLine { folds, .. } => folds.len(),
    }
}
";

    #[test]
    fn a_wildcard_arm_over_the_guarded_type_is_a_violation() {
        assert_eq!(
            match_arm_violations("parse.rs", EXHAUSTIVE_MATCH),
            Vec::<String>::new(),
            "naming every variant is the shape this rule exists to keep"
        );

        let swallowed = EXHAUSTIVE_MATCH.replace(
            "        Token::Value { bytes, .. } => bytes.len(),\n",
            "        _ => 0,\n",
        );
        assert!(
            match_arm_violations("parse.rs", &swallowed)
                .iter()
                .any(|line| line.contains("parse.rs:") && line.contains("a wildcard arm over")),
            "a match that omits a variant and adds `_` is the shape `unreachable_patterns` is \
             silent about, and the only shape that loses data"
        );

        let guarded = EXHAUSTIVE_MATCH.replace(
            "        Token::Value { bytes, .. } => bytes.len(),\n",
            "        _ if true => 0,\n",
        );
        assert!(
            !match_arm_violations("parse.rs", &guarded).is_empty(),
            "a guard does not make a catch-all name a variant"
        );
    }

    #[test]
    fn a_wildcard_arm_over_another_type_is_not_this_rule() {
        let source = "\
fn label(severity: Severity) -> &'static str {
    match severity {
        Severity::Note => \"note\",
        _ => \"else\",
    }
}
";
        assert_eq!(
            match_arm_violations("report.rs", source),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_type_whose_name_ends_in_the_guarded_one_is_a_different_type() {
        let source = "\
fn stale(token: SyncToken) -> bool {
    match token {
        SyncToken::Weak => true,
        _ => false,
    }
}
";
        assert_eq!(
            match_arm_violations("freshness.rs", source),
            Vec::<String>::new(),
            "`SyncToken::` ends in `Token::` and is a type this rule states nothing about"
        );
    }

    #[test]
    fn a_wildcard_beside_a_match_over_the_guarded_type_is_read_at_its_own_depth() {
        let source = "\
fn take(token: Token<'_>, kind: Kind) -> usize {
    match kind {
        Kind::Line => match token {
            Token::Name(bytes) => bytes.len(),
            Token::Parameter { .. } => 1,
            Token::Value { .. } => 2,
            Token::EndOfLine { .. } => 3,
        },
        _ => 0,
    }
}
";
        assert_eq!(
            match_arm_violations("mutate.rs", source),
            Vec::<String>::new(),
            "the outer match is over another type and the inner one names every variant"
        );
    }

    /// The root manifest, the member's manifest and the member's root file, as committed.
    const LAYERING_ROOT: &str = "\
[workspace]
members = [
  \"crates/ical-core\",
  \"gates/grammar-layering\",
  \"xtask\",
]
";

    /// The member manifest, with everything the rule is stated over.
    const LAYERING_MANIFEST: &str = "\
[package]
name = \"ical-grammar-layering\"
publish = false

[lib]
doc = false
test = false
";

    /// The member's root file, which reaches the shipped grammar rather than a copy of it.
    const LAYERING_LIB: &str = "\
#[path = \"../../../crates/icalkit/src/internal/core/grammar/mod.rs\"]
mod grammar;
";

    #[test]
    fn the_member_that_makes_the_layer_a_fact_is_held_by_string_equality() {
        let committed = Some((LAYERING_MANIFEST, LAYERING_LIB));
        assert_eq!(layering(LAYERING_ROOT, committed), Vec::<String>::new());

        let dropped = LAYERING_ROOT.replace("  \"gates/grammar-layering\",\n", "");
        assert!(
            layering(&dropped, committed)
                .iter()
                .any(|line| line.contains("does not register")),
            "a pull request that deleted this member used to pass every gate in the repository"
        );

        assert!(
            layering(LAYERING_ROOT, None)
                .iter()
                .any(|line| line.contains("has no Cargo.toml")),
            "and one that deleted the files rather than the member line, likewise"
        );

        let copied = LAYERING_LIB.replace("/grammar/", "/grammar-copy/");
        assert!(
            layering(LAYERING_ROOT, Some((LAYERING_MANIFEST, &copied)))
                .iter()
                .any(|line| line.contains("shipped sources")),
            "a gate compiling a copy proves something about the copy"
        );

        let counted_twice = LAYERING_MANIFEST.replace("test = false\n", "");
        assert!(
            layering(LAYERING_ROOT, Some((&counted_twice, LAYERING_LIB)))
                .iter()
                .any(|line| line.contains("test = false")),
            "without it the grammar's tests run twice and its coverage is counted twice"
        );
    }

    #[test]
    fn only_the_facade_carries_a_future_release_contract() {
        let published = ["icalkit".to_owned()];
        assert_eq!(
            core_list_violations(&published),
            Vec::<String>::new(),
            "the committed workspace"
        );

        let mut added = published.to_vec();
        added.push("ical-core".to_owned());
        assert!(
            core_list_violations(&added)
                .iter()
                .any(|line| line.contains("ical-core")),
            "an implementation crate must not regain a separate semver contract"
        );

        assert!(
            core_list_violations(&[])
                .iter()
                .any(|line| line.contains("sole future public crate")),
            "the facade is the one production release boundary"
        );
    }

    #[test]
    fn private_workspace_crates_are_explicit_and_never_published() {
        let private = ["icalkit-conformance".to_owned()];
        assert_eq!(
            private_crate_violations(&private),
            Vec::<String>::new(),
            "the committed private tool set"
        );

        assert!(
            private_crate_violations(&[])
                .iter()
                .any(|line| line.contains("`icalkit-conformance`") && line.contains("missing")),
            "a named isolation tool must remain a workspace member"
        );
        assert!(
            private_crate_violations(&["unclassified-helper".to_owned()])
                .iter()
                .any(|line| line.contains("`unclassified-helper`") && line.contains("unclassified")),
            "a private crate under crates/ must be admitted deliberately"
        );
    }

    #[test]
    fn feature_keys_are_read_only_from_the_requested_top_level_table() {
        let manifest = r#"
[package]
name = "facade"

[features]
default = [
    "std",
]
std = []
system-tz = ["std"]

[dependencies]
jiff = "0.2"
"#;
        assert_eq!(
            table_keys(manifest, "features"),
            ["default", "std", "system-tz"]
        );
    }

    #[test]
    fn a_retired_package_cannot_return_as_a_member_or_facade_dependency() {
        let root = r#"
[workspace]
members = [
  "crates/icalkit",
  "crates/ical-core",
  "crates/ical-dav",
  "crates/ical-itip",
  "crates/ical-query",
  "crates/ical-recur",
  "crates/ical-tz",
]
"#;
        let facade = r#"
[dependencies]
ical-core = { path = "../ical-core" }
ical-dav = { path = "../ical-dav" }
ical-itip = { path = "../ical-itip" }
ical-query = { path = "../ical-query" }
ical-recur = { path = "../ical-recur" }
ical-tz = { path = "../ical-tz" }
"#;
        let violations = retired_crate_violations(root, facade);
        assert_eq!(violations.len(), 12);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("workspace member"))
        );
        assert!(violations.iter().any(|line| line.contains("still depends")));

        assert_eq!(
            retired_crate_violations(
                "[workspace]\nmembers = [\"crates/icalkit\"]",
                "[dependencies]\njiff = \"0.2\"",
            ),
            Vec::<String>::new()
        );
    }

    /// A release configuration for the sole public facade.
    const SAMPLE_RELEASE: &str = "\
[[package]]
name = \"icalkit\"
";

    /// The members that configuration is owed.
    fn published() -> Vec<String> {
        vec!["icalkit".to_owned()]
    }

    #[test]
    fn the_release_configuration_is_read_against_the_workspace_members() {
        assert_eq!(
            release_violations(&published(), SAMPLE_RELEASE),
            Vec::<String>::new()
        );

        let stale = format!("{SAMPLE_RELEASE}\n[[package]]\nname = \"ical-grammar\"\n");
        assert!(
            release_violations(&published(), &stale)
                .iter()
                .any(|line| line.contains("`ical-grammar`") && line.contains("does not publish")),
            "the collapse deleted the crate and this file named it for a whole landing"
        );

        let mut added = published();
        added.push("ical-query".to_owned());
        let violations = release_violations(&added, SAMPLE_RELEASE);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `[[package]]` block")),
            "a new crate is released outside the version group until it has one: {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`changelog_include` does not name")),
            "and its history is missing from the one changelog this stack has: {violations:?}"
        );
    }

    #[test]
    fn the_committed_tree_passes_every_leg_of_purity() {
        // The gate's own subject, for the reason the codes task runs against its own files:
        // the samples prove the scan reads the style they are written in, and only this proves
        // it reads the style the workspace is.
        assert_eq!(collect_purity_violations().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn the_committed_tree_has_one_public_crate_and_two_features() {
        assert_eq!(
            collect_architecture_violations().unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn dav_body_encoders_cannot_recreate_xml_structure_helpers() {
        let shared = "use crate::internal::dav::writer::XmlWriter;\n";
        assert!(xml_writer_violations(shared, shared).is_empty());

        let violations = xml_writer_violations(
            "fn write_name() {}\n",
            "use crate::internal::dav::writer::XmlWriter;\nfn open_extension() {}\n",
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("does not use"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("write_name"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("open_extension"))
        );
    }

    #[test]
    fn current_status_rejects_historical_debt_as_live_work() {
        let current_roadmap = "## Current closure ledger — 2026-08-14\n";
        assert!(
            closure_status_violations(
                current_roadmap,
                "private modules are connected through workflows",
                "# Current evidence boundary"
            )
            .is_empty()
        );

        let violations = closure_status_violations(
            "Two are still owed: a hostile input of 200,000",
            "including units not yet reached",
            "# What is still owed here",
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("missing current closure marker"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("completed work"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unconnected"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unsupported capture claim"))
        );
    }

    #[test]
    fn public_guidance_follows_the_typestate_and_workflow_order() {
        let readme = "## Strict parsing\n## Explicit normalization\n## Transactional editing\n\
                      ## DST-aware recurrence\n## iTIP scheduling\n\
                      ## CalDAV sync and server workflows\n";
        assert!(documentation_violations(readme, "private implementation modules").is_empty());

        let stale = documentation_violations(
            "## Explicit normalization\n## Strict parsing\n",
            "one temporary path dependency",
        );
        assert!(
            stale
                .iter()
                .any(|violation| violation.contains("out of golden-path order"))
        );
        assert!(
            stale
                .iter()
                .any(|violation| violation.contains("retired migration prose"))
        );
    }

    #[test]
    fn the_basic_example_keeps_internal_vocabulary_out_of_the_golden_path() {
        assert!(basic_example_violations("use icalkit::Calendar;").is_empty());
        let violations = basic_example_violations(
            "use icalkit::Calendar; fn hidden(_: Limits, _: &mut Meter) {}",
        );
        assert_eq!(violations.len(), 2);
        assert!(violations.iter().any(|line| line.contains("`Limits`")));
        assert!(violations.iter().any(|line| line.contains("`Meter`")));
    }

    #[test]
    fn a_public_api_snapshot_is_exact_and_names_the_first_changed_line() {
        assert_eq!(
            snapshot_violations("default", "pub struct A;\n", "pub struct A;\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            snapshot_violations(
                "no-default",
                "pub struct A;\npub struct B;\n",
                "pub struct A;\npub struct C;\n",
            ),
            vec![
                "api/icalkit.no-default.txt: public API differs at line 2: expected `pub struct B;`, generated `pub struct C;`"
                    .to_owned()
            ]
        );
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
