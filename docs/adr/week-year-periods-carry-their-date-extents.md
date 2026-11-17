---
date: 2026-11-18
status: proposed
---

# Carry explicit week-year extents in recurrence periods

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines numbered weeks using four days and `WKST`.
[Selected calendars](calendar-coordinates-for-recurrence-periods.md) can place the source in an adjacent week-numbering year.
Calendar-year origins shift intervals and omit remaining source-week candidates.

## Decision

Normalize yearly week rules to the week-numbering year containing `DTSTART` in the selected calendar.
Keep civil periods and explicit week-year extents as distinct states.
Advance numbered-year labels by the interval, including resumed indices.
Use the same extent for generation and search bounds.
Clip only the writable date view before position selection.

## Alternatives

Keeping calendar-year origins preserves the mismatched phase.
Inferring labels from clipped anchors loses boundary years.
Padding expansion by adjacent years overlaps periods and complicates counting.

## Consequences

Week periods carry a bounded date span alongside their writable anchor.
Calendar conversion, candidate charging, and cursor selection keep their existing boundaries.

## Confirmation

Compare independent four-day boundaries across supported calendars, week starts, intervals, and resumed indices.
Check counts, positions, additions, exclusions, and both civil endpoints.
Reconsider when period domains or calendar arithmetic change.
