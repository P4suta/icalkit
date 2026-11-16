---
date: 2026-11-16
status: proposed
---

# Gate recurrence calendar controls before expansion

## Context

[RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4) makes calendar controls affect occurrence dates.
The native engine enumerates valid Gregorian dates.

## Decision

Validate `RSCALE` and `SKIP` before typed construction.
Resolve frequency before applying parts, independent of their written order.
Admit [Gregorian names](https://raw.githubusercontent.com/unicode-org/cldr/main/common/bcp47/calendar.xml) with absent or `OMIT` skipping.
Refuse unsupported calendars, other skipping modes, and leap month selectors.
Malformed or repeated controls also fail.
Imports keep original bytes.
Unrelated unknown parts keep their notes.

## Alternatives

Keeping only recognized rule parts can compute different dates.
Ignoring the whole rule loses its recurrence pattern.
Filtering unsupported components and associated overrides would change lossless calendar membership.
Calendar conversion and complete skipping need engine changes and capability negotiation.

## Consequences

Typed workflows report unsupported semantics instead of generating a different series.
Calendar promotion rejects such rules under [compatibility option 1](https://www.rfc-editor.org/rfc/rfc7529.html#section-6).
Full RFC 7529 support remains open.

## Confirmation

Require public and kernel regressions for aliases, malformed controls, lossless import, and sinks that reject reports.
Reconsider when calendar conversion, corrected dates, and CalDAV negotiation have maintained implementations.
