# SPDX-FileCopyrightText: 2026 icalkit contributors
#
# SPDX-License-Identifier: MIT OR Apache-2.0

set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

export RUSTDOCFLAGS := "-D warnings"

# The sans-I/O core: no std, no clock, no network, no bundled time zone database
# (docs/adr/0003, docs/adr/0004).
core_crates := "-p ical-grammar -p ical-core -p ical-recur -p ical-tz -p ical-itip -p ical-dav"

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
test:
    cargo nextest run --workspace --all-features
    cargo test --workspace --doc --all-features

# Run the complete test suite with the non-fail-fast CI profile.
test-ci:
    cargo nextest run --profile ci --workspace --all-features
    cargo test --workspace --doc --all-features

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

# Reject std, clock, network, and bundled-tzdb dependencies in the core.
purity:
    cargo run --quiet -p xtask -- purity

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
    cargo msrv verify --path crates/ical-grammar
    cargo msrv verify --path crates/ical-core
    cargo msrv verify --path crates/ical-recur
    cargo msrv verify --path crates/ical-tz
    cargo msrv verify --path crates/ical-itip
    cargo msrv verify --path crates/ical-dav
    cargo msrv verify --path crates/ical-conform
    cargo msrv verify --path xtask

# Fast deterministic checks used during the edit/commit loop.
check: fmt-check toml-check typos lint purity codes shear reuse actionlint zizmor
    @echo "fast local checks passed"

# Every practical CI gate available on a developer machine.
ci: fmt-check toml-check typos lint feature-matrix test-ci doc no-std wasm purity codes deny shear reuse actionlint zizmor msrv
    @echo "local CI passed"
