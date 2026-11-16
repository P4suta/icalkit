---
date: 2026-11-16
status: proposed
---

# Occurrence duration semantics

## Context

One wall-clock span erases the duration distinction in [RFC 5545 section 3.8.5.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3).
Periods and exceptions can state lengths that differ from the master.

## Decision

Keep calendar days separate from elapsed seconds in the private query model.
Resolve timed `DTEND` and `DUE` differences as elapsed time.
Add duration days before time parts under [section 3.3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.6).
Associate period and exception lengths with recurrence identifiers.
Use the same placement logic for expansion, overlap, and derived results.

## Alternatives

A shared wall-clock span preserves calendar days but changes exact lengths.
A shared elapsed span preserves exact lengths but loses calendar-day semantics.
Expanding all instances before filtering would evade the bounded search contract.

## Consequences

Search bounds must cover the longest applicable span.
Zone resolution stays caller-supplied, and unreadable values remain explicit outcomes.
Reconsider the representation if recurrence gains fractional-second windows.

## Confirmation

Required tests cover time changes, mixed durations, period inclusions, exclusions, exception lengths, and window boundaries.
The maintained checks must pass with default features and without default features.
