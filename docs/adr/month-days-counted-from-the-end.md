---
date: 2026-11-17
status: proposed
---

# Resolve month days counted from the end

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines negative month-day positions from `-31` through `-1`.
The embedded reader accepts only the last day.
Every scalar position permits bounded arithmetic.

## Decision

Add a private selector variant with a nonzero positive position.
Position one names the month's final day.
Subtract the position from the month's length plus one with checked arithmetic.
Construct a date in that month, producing no date for an absent position.
Keep the last-day shorthand and reuse [all-month compilation](month-day-selectors-across-all-months.md).

## Alternatives

Refusing other negative positions preserves a specification gap.
Changing the positive day variant to a signed value changes existing construction.
General recurrence expansion adds a search unnecessary for a scalar position.

## Consequences

Month-scoped and unrestricted rules admit every negative scalar position.
Public interfaces, anchors, clocks, offsets, end bounds, and malformed-input diagnostics keep their behavior.

## Confirmation

Compare all signed scalar positions with independent date enumeration across months, leap years, and year bounds.
Check RFC example dates, missing positions, inclusive ends, public projection, and query matching.
Reconsider for lists or other selector combinations.
