---
date: 2026-11-17
status: proposed
---

# Convert recurrence calendar dates at the Jiff boundary

## Context

[RFC 7529 section 3](https://www.rfc-editor.org/rfc/rfc7529.html#section-3) requires conversion between Gregorian dates and recurrence calendar coordinates.
Leap months and thirteen-month calendars can't use Gregorian month ordinals.

## Decision

Keep Jiff as the public Gregorian date boundary.
Use private ICU4X 2.4 conversion with compiled data and no default features.
Raise the shared Rust floor to 1.88 as required by that release.
Expose validated calendar dates and distinct regular and leap month identifiers.
Use ICU4X extended years consistently across both conversion directions.
Match Unicode calendar names and aliases explicitly.
Refuse ambiguous names, nonexistent dates, and results outside iCalendar Gregorian bounds.
Keep recurrence expansion gated until every selector uses the converted coordinates.

## Alternatives

Handwritten calendar algorithms add astronomical and calendar data maintenance.
ICU4X 2.1 preserves Rust 1.85 but requires a separate dependency version policy.
Gregorian month ordinals lose leap month identity.

## Consequences

Conversion adds a dependency and raises the compiler floor without exposing ICU4X types or adding I/O.

## Confirmation

Require RFC examples, bidirectional conversion, invalid dates, registered names, target builds, dependency boundaries, and compiler regressions.
Reconsider when calendar definitions or supported Rust targets change.
