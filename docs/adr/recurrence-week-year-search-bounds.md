---
date: 2026-11-17
status: proposed
---

# Bound recurrence searches by candidate placement

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines weeks using the rule's week start and four days in the numbered year.
A week-numbering year can begin before or after January 1.
The stream stops at calendar anchors while generation uses week-numbering years.

## Decision

Derive the candidate lower bound from the generation date extent.
Use that bound before opening a period.
Keep the anchor's clock when the date extent begins on the anchor's date.
Return completed counts before opening another period.
Preserve the last visited period in cursors when resuming generates nothing.
Keep period selection, window exclusion, continuation, and candidate charges at their existing boundaries.

## Alternatives

Calendar anchors omit candidates before January 1.
Padding every window spends work outside the candidate domain.
A separate week calculation can diverge from generation.

## Consequences

Year-end searches include candidates from the next week-numbering year.
Completed counts need no later period.
Revisit the bound when generation permits other dates outside calendar periods.

## Confirmation

Regressions cover week starts, intervals, selection, limits, continuation, stored calendars, and short-frequency charges.
Maintained gates must pass.
