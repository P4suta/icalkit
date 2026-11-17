---
date: 2026-11-17
status: proposed
---

# Select annual candidates before compiling month patterns

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines signed year days and `BYSETPOS` over the entire yearly interval.
[Month compilation](weekday-sets-as-calendar-year-masks.md) can't select positions across months independently.

## Decision

Build complete year date masks for fourteen Gregorian layouts at admission.
Intersect month, year-day, month-day, and weekday selectors before position selection.
Count positions after duplicate removal and invalid-date exclusion.
Pattern lookup enforces anchors and inclusive ends.
Split selected dates into month patterns with their month bindings, clocks, offsets, explicit dates, and meter charges.
Preserve compact rules when selection across months is unnecessary.

## Alternatives

Applying positions per month changes yearly scope.
Building the whole year during each month compilation avoids a buffer but repeats work.
General recurrence expansion changes the metering contract.

## Consequences

Admission uses a temporary buffer of 672 bytes.
One source rule still stores at most 366 patterns.
Lookup keeps fixed masks and bounded bit selection.

## Confirmation

Compare signed positions, intersections, leap cases, duplicates, and a Gregorian cycle with independent date enumeration.
Check defaults, bounds, resource charges, and public CalDAV behavior.
Reconsider for week-numbering years or rules that change transition clocks.
