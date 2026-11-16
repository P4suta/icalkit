# The decision docket

## Context

The earlier docket describes retired packages as published contracts and completed work as outstanding.
[ADR 0014](../adr/0014-private-kernel-and-conformance-isolation.md) defines the single production crate and unpublished conformance boundary.

## Decision

[Architecture](../../ARCHITECTURE.md) defines the package, feature, and effect boundaries.
[Roadmap](../../ROADMAP.md) records implemented milestones, remaining observations, and the publication boundary.
[Specification validation](../specification-validation.md) connects supported behavior to public specifications and regression tests.
[Contributing](../../CONTRIBUTING.md) defines TDD and the pinned verification commands.

## Alternatives

Keeping the earlier work queues as current instructions contradicts the maintained architecture.
A status note alone keeps the duplicated obligations.

## Consequences

The [archived docket](https://github.com/P4suta/icalkit/blob/2714a2b817cf75b07634d4986db9e3edae4b1ea2/docs/decisions/DOCKET.md) preserves the earlier questions, verdicts, and work queues.
[Capture intake](../../crates/icalkit-conformance/CAPTURE.md) defines provenance, reduction, and anonymization rules.
Producer observations can change gap and delegation policies only at the roadmap's thresholds.
Synthetic fixtures establish no observed producer behavior.

## Confirmation

Check the archive's Git blob, local links, and `prose check --channel document docs/decisions/DOCKET.md`.
