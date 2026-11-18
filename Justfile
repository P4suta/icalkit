# SPDX-FileCopyrightText: 2026 icalkit contributors
#
# SPDX-License-Identifier: MIT OR Apache-2.0

set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

export RUSTDOCFLAGS := "-D warnings"

# The core has no clock, network, or bundled time zone database (docs/adr/0003 and docs/adr/0004).
core_crates := "-p icalkit"

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

# Lint all workspace features and the standalone core without default features.
lint:
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo clippy --locked -p icalkit --all-targets --no-default-features -- -D warnings

# Run workspace tests and doctests, including standalone core coverage.
test: test-core
    cargo nextest run --locked --status-level leak --workspace --all-features
    cargo test --locked --workspace --doc --all-features --exclude ical-grammar-layering --exclude ical-xml-layering

# Exercise the standalone core without workspace feature unification.
test-core:
    cargo nextest run --locked --status-level leak --profile ci -p icalkit --no-default-features
    cargo test --locked -p icalkit --doc --no-default-features

# Run CI-profile suites after the standalone prerequisites pass.
test-ci: test-core
    cargo nextest run --locked --status-level leak --profile ci --workspace --all-features
    cargo test --locked --workspace --doc --all-features --exclude ical-grammar-layering --exclude ical-xml-layering

# Check allocation ceilings in a dedicated process.
allocation:
    cargo run --locked --quiet -p icalkit-conformance --bin allocation_gate

# Build public documentation with warnings denied.
doc:
    cargo doc --locked --workspace --all-features --no-deps

# Compile no-default, every individual feature, and representative feature pairs.
feature-matrix:
    cargo hack check --locked --workspace --all-targets --each-feature
    cargo hack check --locked {{ core_crates }} --feature-powerset --depth 2

# Build the core for bare-metal ARM without default features.
no-std:
    rustup target add thumbv7em-none-eabi
    cargo build --locked {{ core_crates }} --target thumbv7em-none-eabi --no-default-features

# Check the core for WebAssembly without default features.
wasm:
    rustup target add wasm32-unknown-unknown
    cargo check --locked {{ core_crates }} --target wasm32-unknown-unknown --no-default-features

# Check core dependencies and private implementation boundaries.
purity:
    cargo run --locked --quiet -p xtask -- purity

# Check the public crate, feature vocabulary, and private implementation boundaries.
architecture:
    cargo run --locked --quiet -p xtask -- architecture

# Check repository policy, toolchain pins, and verification recipes.
repository:
    cargo run --locked --quiet -p xtask -- repository

# Freeze the sole production crate's canonical API with default and no default features.
public-api:
    cargo run --locked --quiet -p xtask -- public-api

# Verify the actual CI and allocation arithmetic cores with their declared harnesses.
prove-ci:
    ci-policy prove-source --source xtask/src/ci.rs --require-json '["production::proofs::candidate_identity_is_complete_and_exact","production::proofs::phase_inventory_is_complete_and_ordered","production::proofs::summaries_require_every_job_to_succeed"]' --counterexample production::proofs::counterexample_skipped_candidate_is_successful
    ci-policy prove-source --source crates/icalkit-conformance/src/bin/allocation_gate/arithmetic.rs --require-json '["production::proofs::released_count_is_saturating"]' --counterexample production::proofs::counterexample_release_underflows

# Check diagnostic codes against their committed meanings and channels.
codes:
    cargo run --locked --quiet -p xtask -- codes

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

# Check high-severity GitHub Actions security findings without network access.
zizmor:
    zizmor --offline --persona regular --min-severity high .

# Verify every workspace crate at the shared declared MSRV.
msrv:
    cargo msrv verify --path crates/icalkit
    cargo msrv verify --path crates/icalkit-conformance
    cargo msrv verify --path gates/grammar-layering
    cargo msrv verify --path gates/xml-layering
    cargo msrv verify --path xtask

# Fast deterministic checks used during the edit/commit loop.
check: fmt-check toml-check typos lint test-ci allocation doc purity architecture repository codes deny shear reuse actionlint zizmor
    @echo "fast local checks passed"

# Every practical CI gate available on a developer machine.
ci: fmt-check toml-check typos lint feature-matrix test-ci allocation doc no-std wasm purity architecture repository public-api codes deny shear reuse actionlint zizmor msrv
    @echo "local CI passed"
