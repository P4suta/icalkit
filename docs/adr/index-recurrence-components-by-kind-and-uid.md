---
date: 2026-11-18
status: proposed
---

# Index recurrence components by kind and identifier

## Context

[`RECURRENCE-ID`](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4) identifies an instance of the series named by its [`UID` property](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.7).
Searching all siblings for each override takes quadratic work.

## Decision

Build a metered index for each sibling scope used for recurrence matching.
Group masters and overrides by component kind and decoded identifier length before comparing identifier text.
Preserve document order for equal keys.
An unreadable identifier among potential counterparts leaves the relationship undecided.

## Alternatives

Charging each sibling scan spends the budget on the same components for each override.
A hash table needs a collision policy and separate ordering for equal keys.

## Consequences

Index storage grows with the number of components and the decoded identifier bytes.
Reconsider if resources support incremental changes during a query.

## Confirmation

Exercise budget exhaustion, unreadable identities, missing masters, and lookup growth through calendar queries.
