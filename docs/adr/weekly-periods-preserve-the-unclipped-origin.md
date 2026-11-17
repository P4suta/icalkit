---
date: 2026-11-18
status: proposed
---

# Preserve weekly origins outside writable date views

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines seven-day weekly periods using `WKST`.
The week containing year zero's first date can begin outside the writable calendar.
Rejecting its civil anchor discards valid source-week candidates and later weeks.

## Decision

Keep weekly origins as signed epoch days.
Advance each indexed period by seven days times the interval.
Share explicit date extents with [week-year periods](week-year-periods-carry-their-date-extents.md).
Clip the writable anchor and candidate view without changing the stored origin.
Keep the existing clipping order before position selection.

## Alternatives

Keeping empty walks loses writable dates.
A clipped origin plus repeated week normalization can recover dates but makes period anchors depend on reconstruction.

## Consequences

Weekly periods reuse bounded extent storage without another allocation.
Counts, clocks, selection, and candidate charges remain in their existing layers.

## Confirmation

Compare independent weekly enumeration across week starts, intervals, resumed indices, and civil endpoints.
Check defaults, positions, clocks, limits, cursors, additions, exclusions, and budget exhaustion.
Reconsider if the writable date domain or weekly cadence changes.
