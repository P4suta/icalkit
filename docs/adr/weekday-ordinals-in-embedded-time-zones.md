---
date: 2026-11-17
status: proposed
---

# Resolve weekday ordinals in embedded time zones

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines signed weekday positions.
[Verified erratum 1913](https://errata.rfc-editor.org/eid1913) clarifies month and year scope.
The embedded reader uses the start month for both scopes and refuses negative positions other than the last.
The fifth Sunday of a year can fall in February.

## Decision

Represent month patterns and year weekday patterns as distinct private variants.
Admit nonzero signed ordinals through checked construction.
For numeric `BYDAY`, select month scope when `BYMONTH` is present and year scope when absent.
Resolve each pattern through bounded date arithmetic without a candidate search.
Keep transition clocks, offsets, anchors, end bounds, and coverage on the existing path.

## Alternatives

Keeping the month fallback gives the wrong dates for a yearly selector.
Refusing valid ordinals avoids wrong dates but loses the yearly example in the RFC.
General recurrence searches need candidate budgets inside the zone lookup contract.

## Consequences

Public calendar and time zone interfaces keep their types.
Unsupported recurrence combinations keep their diagnostics.

## Confirmation

Require RFC examples, independent weekday enumeration, both signs, leap years, calendar bounds, anchors, end bounds, and public workflows.
Reconsider when the embedded reader admits more recurrence selectors.
