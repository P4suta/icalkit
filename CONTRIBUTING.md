# Contributing

## Setup

```sh
mise install        # toolchain and gate tooling
mise run hooks      # install the git hooks
just                # list the available commands
```

## The loop

```sh
just check          # fast deterministic gates
just ci             # full compatibility checks available on this host
```

The pre-push hook runs `just check`, matching the ordinary Linux CI inventory.
The ordinary gate includes workspace tests and doctests, allocation and documentation checks, strict linting, dependency policy, and repository checks.
`just ci` also runs feature combinations, target builds, nightly API snapshots, and MSRV verification.
The candidate qualification workflow runs those checks and the Windows and macOS tests on one complete commit ID.
It also verifies the actual CI decision core with Kani 0.68.0 and the declared rejecting probe.
`just prove-ci` runs that source-bound proof inventory on Linux or macOS with the pinned verifier and the shared `ci-policy` helper.

## Test-driven changes

Start by adding the smallest test that expresses the missing behavior and confirm that it fails for the intended reason.
Add the behavior, run the focused test until it passes, then run `just check` and the relevant portability or conformance gates.
Before pushing, run `just check`.
Use candidate qualification to verify portability and compatibility.
Candidate qualification records verification results without publishing a release.

Work on a topic branch and open a draft pull request while the change is still evolving.
Keep generated artifacts, public API snapshots, corpus evidence, and documentation in the same pull request as the source change that requires them.
The complete hosting and merge rules are in the [repository policy](docs/repository-policy.md).

## Required boundaries

Don't add `allow` or `ignore` to suppress a gate finding.
If a lint doesn't fit the codebase, change the shared configuration and explain the reason in the commit message.

`icalkit` is the only production crate.
Its crate root remains `#![no_std]`, and its private kernel must not gain a clock, transport, store, caller policy, or bundled time zone database.
The `std` and `system-tz` features activate only the documented adapter boundary.
`just purity`, `just no-std`, and `just wasm` enforce the boundary.
A zone answer names its caller-supplied source, as defined in [ADR 0003](docs/adr/0003-caller-supplied-time-zones.md).
The caller supplies the current instant, as defined in [ADR 0004](docs/adr/0004-sans-io-protocol-layer.md).
`just purity` checks the linked package identity, so renaming a dependency key can't hide a new boundary.
Jiff and xmlparser are the two recorded third-party boundaries.
Another dependency requires an architecture decision.

The production crate declares `extern crate alloc;`, and `just purity` checks for the declaration.
An allocation-free tier would require a new crate and lint profile under [ADR 0007](docs/adr/0007-allocation-policy.md).
Charge each byte when appending it and refuse a value at the octet that crosses its budget.
A refusal must not truncate the value.

Every entry point that reads untrusted input takes the shared `Limits` and `&mut Meter` from [ADR 0010](docs/adr/0010-shared-resource-limits.md).
Charge the supplied meter throughout the operation.
Creating a fresh meter inside a fan-out loop defeats the total bound.
`Meter` implements neither `Copy` nor `Default`.

A specification violation is an issue.
An error means the operation couldn't build a validated value.
[ADR 0009](docs/adr/0009-error-and-diagnostic-model.md) fixes the channel per code.
Keep the meanings of internal diagnostic codes and the public `IssueCode` string newtype stable.
A sink refusing retention must not change how much input the parser validates.

Strict validation, lossless import, and editing use the same private content-line tokenizer under [ADR 0008](docs/adr/0008-parser-layering-and-pull-api.md).
Keep token payloads as octets internally and require UTF-8 only for typed views that need it.

`crates/icalkit/src/internal/core/grammar/` remains a flat private module tree that names no higher layer.
`gates/grammar-layering` compiles the same sources without the model.
The textual checks in `just purity` supplement compilation under amendments 17 and 18 of [ADR 0004](docs/adr/0004-sans-io-protocol-layer.md).
Inside `mod.rs`, use neither `crate::` nor `super::`.
Sibling files permit neither `crate::` nor `super::super::`.
The layer permits no retired crate name, `extern crate`, or `#[path]`, and `mod.rs` must declare every sibling `.rs` file.
The textual check doesn't resolve macros or generated paths.

Use checked time arithmetic with `checked_*`, `div_euclid`, and `rem_euclid`.
No `Duration` carries years or months.
Filter recurrence instances with nonexistent dates or local times under RFC 5545 section 3.3.10.
Don't move the instances to nearby values.
[ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md) defines the arithmetic and resolution types.

A supported property's parse and serialization round trip must preserve every byte, including uninterpreted parameters, casing, and ordering.
Typed access remains a view over preserved text under [ADR 0001](docs/adr/0001-lossless-round-trip.md).
An accessor must not replace the original text.

Recurrence expansion is a lazy iterator over a caller-supplied window.
Report candidate-budget exhaustion as an outcome under [ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md).
Don't collect an entire rule into a `Vec` or bypass the budget in a search path.

Every implemented rule needs a case in `icalkit-conformance` citing its RFC section under [ADR 0006](docs/adr/0006-conformance-corpus-as-artifact.md).
Where implementations disagree, record each observed behavior and the project's choice.

## The corpus is real, which is an obligation

Use real-client calendar exports to check fidelity with external input.
Before accepting a case, reduce the export to the smallest form that preserves the behavior.
Replace names, addresses, locations, and identifiers with anonymous values that preserve the input's shape.
Record the client and version that produced the original.

Reject a case if anonymization loses the demonstrated behavior.
Describe the behavior in prose and construct a synthetic case that demonstrates it.

## Code and comments are in English

Write code comments and documentation in American English, matching the `en-us` locale in `typos`.
Property and component names keep the RFC's spelling in prose.
`typos.toml` records the names as vocabulary.

## Commits

Conventional Commits, validated by `committed` in the commit-msg hook:

```text
feat(core): preserve unknown parameters in their original order
fix(recur): apply EXDATE inside the iterator instead of after it
docs(adr): record the caller-supplied time zone decision
```

## Where disagreement belongs

Resolve a disputed RFC 5545 rule with a conformance case citing the relevant section.
Where the RFC permits alternatives, expose a caller-visible option and record every permitted outcome in a case.
