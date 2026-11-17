---
date: 2026-11-18
status: proposed
---

# Project week-numbered observances into calendar patterns

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines numbered weeks using `WKST` and four days in the numbered year.
Week periods can cross calendar-year boundaries.
[Calendar alignment](year-aligned-embedded-transitions.md) alone can omit dates from the source week.

## Decision

Align week intervals from the week-numbering year containing `DTSTART`.
Compile 28 Gregorian layouts distinguished by January weekday and neighboring leap years.
Use four separate rows for civil-boundary periods.
Exclude unwritable dates before selecting positions across each complete period.
Project selected dates into month patterns carrying their generating-year offsets.
Share bounded count ranking, with cycle skips confined to periodic years.
Validate UTC ends within the generating period.

## Alternatives

Calendar-year filtering splits week periods.
General recurrence expansion changes lookup metering.
Storing 400 layouts per pattern increases memory.

## Consequences

Admission holds 4608 bytes of week date masks.
One rule stores at most 372 patterns.
Week patterns use fixed day tables and bounded lookup.

## Confirmation

Compare forward enumeration across Gregorian cycles, week starts, neighboring leap years, limits, and civil boundaries.
Check explicit dates, resource charges, and CalDAV results.
Reconsider for nonannual frequencies or variable transition clocks.
