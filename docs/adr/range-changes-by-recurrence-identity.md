---
date: 2026-11-16
status: proposed
---

# Range changes by recurrence identity

## Context

[RFC 5545 section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4) defines range membership by recurrence identity.
Range changes exclude instances defined in separate components.
[The recurrence decision](0002-bounded-lazy-recurrence.md) exempts literal additions from shifts and composes earlier changes into exact exceptions.

## Decision

Apply range shifts to rule instances and `RDATE` additions alike.
For exact exception components, use their stated changes without earlier range changes.
Build exact templates from the master and the exact component.
Keep cadence-key ordering, identity-based exclusions, bounded search, and the existing diff representation.

## Alternatives

Literal additions preserve their written starts but contradict the range definition.
Inheriting earlier range changes into exact exceptions preserves accumulated edits but contradicts the separate-component exclusion.
Full component snapshots would replace the borrowed diff model and add allocation costs.

## Consequences

Some starts and inherited properties change from prior behavior.
Input bytes and public signatures stay stable.
Revisit the composition model if a published extension defines another range mode.

## Confirmation

Tests must cover added dates, periods, exact exceptions, property changes, and window boundaries.
The shared merge and calendar projection must agree.
The maintained checks must pass.
