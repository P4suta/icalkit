---
date: 2026-11-17
status: proposed
---

# Expand month-day selectors across all months

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) expands yearly `BYMONTHDAY` across months when `BYMONTH` is absent.
The reader instead uses the start month.
The [zone design](../design/ical-tz-api.md) resolves one onset per annual pattern without recurrence searches.

## Decision

Compile supported unrestricted month-day selectors into twelve month-specific annual patterns.
Represent single-pattern rules and all-month rules as distinct private variants.
Every pattern keeps the original anchor, offsets, clock, and end bound.
Store explicit dates once.
Charge each stored pattern to the existing observance meter.
Keep lookup arithmetic and caller-selected sources unchanged.

## Alternatives

Keeping the start month loses valid transitions.
Rejecting these selectors discards already accepted rules.
An onset iterator shares storage but changes the annual lookup contract.

## Consequences

One source rule can consume twelve stored observances.
Explicit months, default anniversaries, yearly weekday ordinals, and unsupported-rule diagnostics keep their behavior.

## Confirmation

Compare fixed days, last days, and weekday windows with independent date enumeration.
Check anchors, inclusive ends, explicit dates, exact meter charges, truncation, and public CalDAV behavior.
Reconsider for larger selector sets or other intervals.
