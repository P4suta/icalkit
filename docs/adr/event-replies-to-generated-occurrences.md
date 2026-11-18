---
date: 2026-11-19
status: proposed
---

# Event replies to generated occurrences

## Context

[An attendee's reply](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.2.3) can address [one original recurrence key](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.7.1) after a move.

## Decision

Find the key and apply range changes under the session budget.
Compare stated times and parameters with the organizer's occurrence and keep omitted times.
Keep each date, floating time, UTC time, or named local time in its original form.
Refuse a local start or end when [the first occurrence of its wall time](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.5) would change its instant.
Stored and incoming time zone definitions must identify the same instant.
Reject a local recurrence identifier when the clock repeats that hour.
Update stored occurrences in place.
Preserve inherited attendee order when storing the answer.
Refuse an answer at the start of a stored [range](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.13) while its attendee state also covers later occurrences.

## Alternatives

The master can start on another day.
Copying the reply lets an attendee change the schedule.
Another component at the same key would duplicate the instance.

## Consequences

A review retains the fields needed for the answer.
Revisit range replies when storage can record participation at one key apart from the organizer's range.

## Confirmation

Tests cover repeated replies, inherited attendees, range participation, clocks, and exclusions.
