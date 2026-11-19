---
date: 2026-11-17
status: proposed
---

# Compile weekday sets into calendar year masks

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) permits weekday lists and month-day intersections.
[Verified erratum 1913](https://errata.rfc-editor.org/eid1913) defines ordinal scope.
[Annual compilation](month-day-sets-as-ranked-annual-patterns.md) already bounds stored patterns.
Leap status and the weekday of January 1 determine Gregorian weekday placement.

## Decision

Normalize weekday terms into checked sets.
At admission, compile month date masks for fourteen year layouts.
Apply ordinal scope before month-day intersection and merge coincident dates.
Store the month, date masks, and rank in each pattern.
Preserve compact forms, anchors, clocks, end bounds, explicit dates, and meter charges.

## Alternatives

Evaluating a fixed term set per lookup needs less precomputation but repeats weekday and ordinal work.
General recurrence iteration changes the metering contract.
Refusal leaves valid lists unsupported.

## Consequences

Each rule emits at most 366 patterns.
Each general pattern stores fourteen 32-bit masks.
Lookup selects bits without recurrence date searches.

## Confirmation

Compare weekday subsets, signed ordinals, mixed lists, and intersections with independent date enumeration.
Check a Gregorian cycle, boundary years, source month binding, limits, and public CalDAV behavior.
Reconsider for other calendars or intervals.
