---
date: 2026-11-17
status: proposed
---

# Preserve year day selection before Gregorian correction

## Context

[RFC 7529 section 4.1](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1) includes missing year days and places day correction after `BYMONTHDAY`.
The [month day implementation](gregorian-month-day-adjustments.md) can merge overlapping periods but rejects non-`OMIT` year days.

## Decision

Apply month selection to the source year day before correction.
Keep `BYYEARDAY` and `BYMONTHDAY` as intersecting constraints on the source date.
A missing year day has no valid month day to match.
Resolve surviving days with the same signed span arithmetic as month days.
Apply weekday selection after correction.
Derive yearly candidate bounds from resolved year days and preserve their originating periods.

## Alternatives

Keeping the gate rejects patterns that need no calendar conversion.
Correcting before month day selection can admit a different source day.
Expanding month days again loses the year day constraint.

## Consequences

Gregorian year day corrections can cross year boundaries without duplicate counts or lost continuation.
Non-`OMIT` week numbers and leap month selectors remain unsupported.

## Confirmation

Require signed ordinal, filter order, boundary, budget, interval, and continuation regressions.
Reconsider when week numbers, leap months, or calendar conversion change the source date model.
