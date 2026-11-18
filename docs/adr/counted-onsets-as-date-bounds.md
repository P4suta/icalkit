---
date: 2026-11-18
status: proposed
---

# Bound counted observances by their final onset

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) applies `COUNT` after date selection and counts `DTSTART` first.
Explicit recurrence dates remain independent.
Month patterns need one shared endpoint.

## Decision

Represent unlimited, dated, and counted ends as exclusive alternatives.
Find the final onset from complete yearly masks using bounded summaries of 400 aligned periods.
Gregorian dates repeat under the [400-year leap rule](https://aa.usno.navy.mil/faq/leap_years).
Clip stored masks after the anchor through a representable endpoint.
Keep source anchors and explicit dates independent.
An endpoint beyond civil years 0000–9999 leaves patterns without a finite bound inside that range.
A counted clock series with no remaining civil onsets retains its anchor and coverage through year 9999.

## Alternatives

Runtime counting changes lookup state and storage.
Enumerating every occurrence makes admission depend on the count's value.
Counting per month changes the recurrence set.

## Consequences

Counted rules use date masks even for scalar selectors.
Admission uses bounded summaries and temporary masks.
Lookup and pattern charges keep their existing contracts.

## Confirmation

Compare forward enumeration, large counts, interval phases, aliases, leap dates, civil boundaries, resource limits, and CalDAV results.
Reconsider for a wider civil range or recurrence clocks that vary within a year.
