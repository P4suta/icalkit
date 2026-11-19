---
date: 2026-11-17
status: proposed
---

# Align embedded recurrence years with their source anchor

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines yearly interval spacing from `DTSTART`.
The [64-year search](0003-caller-supplied-time-zones.md) misses sparse interval rules.
Gregorian dates repeat every 400 years under the [leap rule](https://aa.usno.navy.mil/faq/leap_years).

## Decision

Store a nonzero interval on every compiled pattern.
Align computed onsets and backward probes with the source anchor year.
Probe at most 403 aligned years: one Gregorian cycle and three initial candidates for UTC lookahead, year offsets, and a later local onset.
Keep anchors, explicit dates, clocks, inclusive ends, and admission charges independent of interval spacing.

## Alternatives

Keeping unit intervals rejects specified input.
The fixed 64-year scan misses distant transitions.
General recurrence expansion changes metering.
Scanning intervening years increases lookup work.

## Consequences

Each stored pattern carries an interval.
Lookup can examine 403 candidate years per pattern without allocating recurrence candidates.

## Confirmation

Compare all interval residues with independent forward enumeration.
Check leap centuries, date intersections, end bounds, year boundaries, resource limits, and public CalDAV workflows.
Reconsider for nonannual frequencies or week-numbering years.
