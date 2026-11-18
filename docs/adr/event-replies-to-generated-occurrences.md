---
date: 2026-11-19
status: proposed
---

# Event replies to generated occurrences

## Context

[An attendee's reply](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.2.3) can address [one original recurrence key](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.7.1) after a move.

## Decision

Resolve keys and ranges within budget.
Compare stated times and parameters with the organizer's occurrence and keep omitted times.
Preserve dates, floating times, UTC times, and named local times.
Refuse a local start or end when [the first occurrence of its wall time](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.5) would change its instant.
Both time zone definitions must identify the same instant.
Reject a local recurrence identifier when the clock repeats that hour.
Update stored occurrences in place and preserve inherited attendee order.
Compare [reply timestamps](https://www.rfc-editor.org/rfc/rfc5546.html#section-2.1.5) for the same attendee, recurrence key, and revision.
Advancing the master expires timestamps on untouched occurrences with no `SEQUENCE`.
Refuse replies at a stored [range](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.13) anchor whose attendance covers later occurrences.

## Alternatives

The master can start on another day.
Copying the reply lets an attendee change the schedule.
Another component at the same key would duplicate the instance.

## Consequences

A review retains the fields needed for the answer.
Revisit range replies when storage separates individual attendance from the organizer's range.

## Confirmation

Tests cover repeated replies, inherited attendees, range participation, clocks, and exclusions.
