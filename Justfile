# SPDX-FileCopyrightText: 2026 icalkit contributors
#
# SPDX-License-Identifier: MIT OR Apache-2.0

set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

export RUSTDOCFLAGS := "-D warnings"

# The sans-I/O core: no std, no clock, no network, no bundled time zone database
# (docs/adr/0003, docs/adr/0004).
core_crates := "-p icalkit -p ical-core -p ical-recur -p ical-tz -p ical-itip -p ical-dav"

# List the available development commands.
default:
    @just --list

# Format the workspace.
fmt:
    cargo fmt --all

# Check formatting without modifying files.
fmt-check:
    cargo fmt --all --check

# Check TOML formatting.
toml-check:
    taplo fmt --check --diff

# Run Clippy with and without default features across every target.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo clippy --workspace --all-targets --no-default-features -- -D warnings

# Run the workspace suite. Nextest runs normal tests process-per-test; Cargo
# separately runs doctests, which nextest does not currently support.
#
# The two layering gates are excluded from the doctests and only from them. Each compiles
# another crate's layer a second time, so the doc examples on those items would be compiled
# inside a crate that declares almost no dependencies and would fail there. `[lib] doctest =
# false` reads like the fix and is not one: cargo reports the member's doctests disabled and the
# merged doctest runner runs them anyway (docs/adr/0004, amendment 17).
test:
    cargo nextest run --workspace --all-features
    cargo test --workspace --doc --all-features --exclude ical-grammar-layering --exclude ical-xml-layering

# Run the complete test suite with the non-fail-fast CI profile.
test-ci:
    cargo nextest run --profile ci --workspace --all-features
    cargo test --workspace --doc --all-features --exclude ical-grammar-layering --exclude ical-xml-layering

# Build public documentation with warnings denied.
doc:
    cargo doc --workspace --all-features --no-deps

# Compile no-default, every individual feature, and representative feature pairs.
feature-matrix:
    cargo hack check --workspace --all-targets --each-feature
    cargo hack check {{core_crates}} --feature-powerset --depth 2

# Prove the core stays no_std. A calendar library that needs an OS cannot run in the
# places calendars are actually rendered (docs/adr/0004).
no-std:
    rustup target add thumbv7em-none-eabi
    cargo build {{core_crates}} --target thumbv7em-none-eabi --no-default-features

# The same core has to work in a browser, which is where most calendar UIs live.
wasm:
    rustup target add wasm32-unknown-unknown
    cargo check {{core_crates}} --target wasm32-unknown-unknown --no-default-features

# Five rules, one task, because it already walks this tree and already holds docs/adr/0004's
# structural rules. First: reject std, clock, network, and bundled-tzdb dependencies in the
# core, and hold this file's core_crates and xtask's CORE_CRATES to each other. Second: reject
# a path inside ical-core's grammar layer that resolves above the layer's root, a subdirectory
# under it, an `extern crate` or `#[path]` inside it, and a file the layer's mod.rs does not
# declare. Third: hold gates/grammar-layering to what the ADR says that member must be, since
# deleting it deletes the compile half of the layering guarantee and nothing else noticed.
# Fourth: reject a wildcard match arm over `Token`, which the lint that claimed to do it cannot
# see. Fifth: hold release-plz.toml to the workspace's published members. A contributor grepping
# for a gate called "layering" finds nothing, which is the price of not adding four more recipes
# and eight CI lines to read files that are already open (docs/adr/0004, amendment 18).
purity:
    cargo run --quiet -p xtask -- purity

# Hold the single public crate boundary and the facade's two-feature vocabulary
# (docs/adr/0013).
architecture:
    cargo run --quiet -p xtask -- architecture

# Freeze the sole production crate's canonical API with default and no default features.
public-api:
    cargo run --quiet -p xtask -- public-api

# Hold every diagnostic code's meaning, channel, and owing milestone to the committed
# golden list docs/adr/0009 requires.
codes:
    cargo run --quiet -p xtask -- codes

# Spell-check the repository.
typos:
    typos

# Check dependency advisories, bans, licenses, and sources.
deny:
    cargo deny --all-features check advisories bans licenses sources

# Reject unused, misplaced, and unlinked Cargo dependencies or source files.
shear:
    cargo shear --deny-warnings

# Check REUSE/SPDX compliance.
reuse:
    uvx --with charset-normalizer==3.4.9 reuse==6.2.0 lint

# Validate GitHub Actions workflows.
actionlint:
    actionlint -color

# Reject high-severity GitHub Actions and Dependabot security findings without
# granting the auditor network or repository credentials.
zizmor:
    zizmor --offline --persona regular --min-severity high .

# Verify every workspace crate at the shared declared MSRV.
msrv:
    cargo msrv verify --path crates/icalkit
    cargo msrv verify --path crates/ical-core
    cargo msrv verify --path crates/ical-recur
    cargo msrv verify --path crates/ical-tz
    cargo msrv verify --path crates/ical-itip
    cargo msrv verify --path crates/ical-dav
    cargo msrv verify --path crates/icalkit-conformance
    cargo msrv verify --path gates/grammar-layering
    cargo msrv verify --path gates/xml-layering
    cargo msrv verify --path xtask

# Fast deterministic checks used during the edit/commit loop.
check: fmt-check toml-check typos lint purity architecture public-api codes shear reuse actionlint zizmor
    @echo "fast local checks passed"

# Every practical CI gate available on a developer machine.
ci: fmt-check toml-check typos lint feature-matrix test-ci doc no-std wasm purity architecture public-api codes deny shear reuse actionlint zizmor msrv
    @echo "local CI passed"
