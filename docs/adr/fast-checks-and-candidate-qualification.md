---
date: 2026-10-26
---

# Fast checks and candidate qualification

## Status

Accepted.

## Context

Ordinary CI repeated the same functional inventory on pull requests and main pushes.
It also required three operating systems, feature combinations, five MSRV builds, target builds, and nightly API generation before every push.
The summary jobs accepted skipped checks without establishing that the intended inventory succeeded.

The portability promises in [ADR 0004](0004-sans-io-protocol-layer.md) and the API snapshots in [ADR 0013](0013-unified-public-crate-and-explicit-interop.md) still need executable checks.
The repository remains intentionally unreleased and has no publication workflow.

## Decision

Use `just check` for ordinary Linux CI and local pre-push verification.
Keep formatting, strict linting, workspace tests and doctests, allocation and documentation checks, dependency policy, and architecture and repository checks in that inventory.
Run ordinary CI on pull requests and merge groups, with an explicit manual entry point.
Keep CodeQL's pull-request analysis and default-branch baseline without its weekly schedule.

Run Windows and macOS tests, feature combinations, bare-metal and WebAssembly compilation, pinned-nightly API snapshots, and five-crate MSRV verification during candidate qualification.
Every candidate job must verify that the requested complete commit ID matches its actual checkout before and after its checks.
Both identity checks reject changes to tracked source, including staged changes.
The existing xtask owns revision validation and strict phase summaries.
Summaries require the exact declared job inventory and successful results for every member.
The proof job imports the production CI core and checks every declared identity, inventory, and summary harness with Kani 0.68.0.
It also checks the actual arithmetic module imported by the unpublished allocation measurement helper.
The shared `ci-policy prove-source` command checks the complete harness inventory, reachable covers, positive verdicts, and a rejecting counterexample run.

Keep the measurement helper compatible with Rust 1.85.
Use a relaxed compare-and-exchange retry loop for saturated release accounting.
Use `AtomicU64::fetch_max` for peak tracking.
The release arithmetic harness quantifies every pair of unsigned 64-bit counts.
It checks that the production transition equals checked subtraction with a zero floor.
Native tests exercise local atomic counters at zero, exact-release, saturation, and `u64::MAX` boundaries.
Separate cases exercise concurrent releases and peak updates.
Those tests complement the arithmetic proof without proving every thread interleaving or eventual retry-loop completion.
The adapter relies on Rust's standard atomic semantics, and the allocator still delegates to `System` without changing the existing measurement ceilings.

Keep `just ci` as the complete compatibility inventory available on one developer host.
Use `just prove-ci` for both source-bound proof inventories on a supported verifier host.
Candidate qualification produces results without signing, tagging, or publishing.
A future publication workflow must consume successful qualification of its exact source commit.

## Consequences

Routine checks keep native behavior and dependency checks while avoiding repeated platform and compatibility campaigns.
Candidate checks remain mandatory evidence for a future release, but no present workflow enforces publication because no publisher exists.
Unrun candidate jobs provide no qualification evidence.
Deleting portability checks would lose supported-target evidence, and keeping them on every routine change would preserve the unnecessary execution cost.
The new decision changes when the earlier ADRs' portability checks run without changing their assertions.
