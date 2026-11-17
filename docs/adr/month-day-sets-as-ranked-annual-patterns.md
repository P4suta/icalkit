---
date: 2026-11-17
status: proposed
---

# Compile month-day sets into ranked annual patterns

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) permits signed month-day lists and weekday intersections.
[Annual compilation](selected-month-sets-as-annual-patterns.md) stores one onset per pattern without recurrence searches.
Positive and negative positions can name the same date.

## Decision

Keep checked signed positions in a nonempty bit set.
Resolve positions into valid month dates, merge coincident dates, and intersect an optional nonnumeric weekday.
Compile ranks within the resulting date set into annual patterns.
Bound stored ranks by each month's largest possible set over common and leap years and weekday alignments.
Preserve compact scalar and window patterns, original anchors, explicit dates, and observance accounting.

## Alternatives

One pattern per possible date avoids rank selection but can store more records for weekday intersections.
A general onset iterator changes lookup and metering contracts.
Continued refusal leaves valid lists unsupported.

## Consequences

One source rule can store at most 366 patterns.
Lookup uses fixed-width masks and at most thirty bit removals.
Source bytes remain unchanged.

## Confirmation

Compare signed pairs, duplicates, intersections, and a Gregorian cycle with independent date enumeration.
Check limits, bounds, anchors, and public CalDAV behavior.
Reconsider for other calendar domains or rules that change the transition clock.
