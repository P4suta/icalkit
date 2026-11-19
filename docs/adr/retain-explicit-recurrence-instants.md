---
date: 2026-11-19
status: proposed
---

# Keep explicit recurrence instants

## Context

During a backward clock change, two UTC instants can share a local time.
[RFC 5545 section 3.3.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.5) assigns the first reading to a repeated local time.
An explicit UTC value identifies its own instant.
[ADR 0011](0011-civil-time-arithmetic-and-resolution-types.md) records the resulting collision between nominal keys.

## Decision

Keep civil keys for rule generation.
Keep the original instant of an explicit date when resolving its civil key would change the instant.
Combine inclusions, exclusions, and override identities at their original starts under [section 3.8.5.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.2).
The shared meter bounds both sets.
A cursor needs an ordering across both sets.
Refuse a range anchor when its nominal key would also move an earlier occurrence.

## Alternatives

Using only civil keys merges distinct starts.
Generating every rule in UTC shifts local appointments across offset changes.

## Consequences

Explicit inclusions can add a second occurrence in a repeated hour.
Conflicting periods at one instant still have no unique duration.
Revisit the split if the recurrence engine supports distinct readings of a civil key.

## Confirmation

Check expansion, filtering, recurrence identity, exclusions, override effects, and shared limits across a repeated hour.
