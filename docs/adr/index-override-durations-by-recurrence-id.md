---
date: 2026-11-19
status: proposed
---

# Index override durations by recurrence identifier

## Context

Expansion and overlap checks need a duration for each occurrence.
Scanning all exceptions for each occurrence repeats work outside the [shared budget](0010-shared-resource-limits.md).

## Decision

Sort exceptions by recurrence identifier and source position.
Keep the first exception for each identifier.
Record the applicable range override at each position if one supplies a duration.
Look up each occurrence by binary search and charge the caller's meter for comparisons.
Charge storage and sorting before building the index.

## Alternatives

A cursor requires ascending requests, but scheduling can request individual occurrences.
A separate range index needs a second search.

## Consequences

Storage grows linearly with the number of exceptions.
An exact exception without a stated end uses the [base occurrence duration](occurrence-duration-semantics.md).
Reconsider the index if requests can edit a series between lookups.

## Confirmation

Measure lookup work for occurrences before later exceptions and require an explicit result on exhaustion.
