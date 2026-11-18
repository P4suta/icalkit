---
date: 2026-11-19
status: proposed
---

# Event replies to generated occurrences

## Context

An attendee can answer one occurrence of a recurring event.
The recurrence identifier keeps the original start after a move.
[Event replies](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.2.3) and [recurring components](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.7.1) define the attendee's role and the instance key.

## Decision

Find the recurrence key and apply the organizer's range changes under the session budget.
Compare each time in the reply with the organizer's projected property, including its parameters.
An omitted time keeps the organizer's value.
Keep each date, floating time, UTC time, or named local time in its original form.
Stored and incoming time zone definitions must identify the same instant.
Reject a local recurrence identifier when the clock repeats that hour.
Update an existing occurrence in place and keep its identifier.

## Alternatives

Comparing with the master rejects an echo of an occurrence on another day.
Copying times from the reply lets the attendee change the event's schedule.

## Consequences

A review holds the detached component until the caller applies the change.

## Confirmation

Scheduling tests compare explicit and omitted time fields for the same occurrence.
Tests cover later replies, range changes, time zones, and exclusions.
