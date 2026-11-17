---
date: 2026-11-17
status: proposed
---

# Preserve month-day intersections in embedded time zones

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) applies `BYMONTHDAY` before `BYDAY` and ignores nonexistent dates.
The reader's month-end shortcut selects February 22 in 2026 for days 25 through 31.
The [zone design](../design/ical-tz-api.md) requires bounded arithmetic.

## Decision

Compile positive consecutive seven-day windows as the first matching weekday on or after their lower bound.
A weekday shift is at most six days, preserving the upper bound without candidate searches.
Existing date construction rejects results outside the month.
Keep the negative final-week window as the last matching weekday.

## Alternatives

Keeping the month-end shortcut loses the lower bound.
An explicit range variant preserves both bounds but adds state unnecessary for seven-day windows.
General recurrence expansion needs a meter inside zone lookup.

## Consequences

Some short months have no computed onset for positive windows.
Observance anchors, clocks, offsets, end bounds, and unsupported-rule diagnostics keep their existing behavior.

## Confirmation

Compare every positive seven-day window and weekday with independent date-set enumeration across months, leap years, and calendar bounds.
Check negative windows and public CalDAV behavior.
Reconsider for other window lengths or nonconsecutive sets.
