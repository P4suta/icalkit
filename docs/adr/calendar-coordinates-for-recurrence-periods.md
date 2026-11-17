---
date: 2026-11-17
status: proposed
---

# Advance recurrence periods in calendar coordinates

## Context

[RFC 7529 section 3](https://www.rfc-editor.org/rfc/rfc7529.html#section-3) advances the selected calendar before converting occurrences to Gregorian dates.
Gregorian month offsets lose leap months and thirteen-month years.

## Decision

Share calendar identities and validated coordinates between the private kernel and the public Jiff adapter.
Normalize calendar origins to the first day before ICU4X month and year arithmetic.
Compute resumed positions from the original coordinates and reuse boundaries during sequential walks.
Clip only the initial period boundary to Gregorian year zero and stop beyond year 9999.
The unpublished conformance helper compiles the same source and records its private ICU4X dependency.
Keep rule admission gated until every date selector and missing date correction uses the selected calendar.

## Alternatives

Gregorian offsets can't represent variable month counts.
Independent calendar adapters duplicate name and leap month mappings.
Unnormalized stepping can clamp days or lose leap month identity.
Recomputing every monthly position from the origin repeats calendar year traversal.

## Consequences

Period stepping gains calendar arithmetic without changing the public conversion API or enabling incomplete recurrence expansion.

## Confirmation

Require RFC examples, adjacent leap months, interval and resume equivalence, and Gregorian boundary regressions.
Reconsider when calendar arithmetic or conformance isolation changes.
