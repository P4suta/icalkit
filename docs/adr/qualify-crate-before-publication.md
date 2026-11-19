---
date: 2026-11-19
status: proposed
---

# Qualify the crate before publication

## Context

Cargo's packaging build missed test data outside the crate.
[Cargo package](https://doc.rust-lang.org/cargo/commands/cargo-package.html)

Trusted Publishing requires a published crate.
[crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing)

## Decision

Test the archive Cargo extracts in an isolated directory and compare its license texts and mirrored fixtures with the repository sources.
Run the package check within candidate qualification for the selected commit.
Use release-plz to prepare draft version and changelog proposals.
Require separate authorization for version changes and publication.
[Release-plz CLI](https://release-plz.dev/docs/usage)

## Alternatives

An ordinary workspace build can resolve sibling files that Cargo excludes from the crate.
Publishing when a version proposal merges removes an independent choice of publication timing.

## Consequences

Candidate qualification pays for a separate package build and test run.
The first publication requires a separate registry operation.
Revisit registry automation when the crate has a published baseline and a Trusted Publishing binding.

## Confirmation

The archived test suite must pass and byte comparisons must reject missing or changed files.
The proposal configuration must reject publication options and preserve private workspace members.
