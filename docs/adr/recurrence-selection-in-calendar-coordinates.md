---
date: 2026-11-17
status: proposed
---

# Select recurrence dates in calendar coordinates

## Context

[RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4) changes calendar field ranges and missing date processing.
The [calendar control decision](recurrence-calendar-controls-before-expansion.md) refuses unimplemented selectors.
The [period arithmetic decision](calendar-coordinates-for-recurrence-periods.md) supplies shared calendar coordinates.
Period stepping alone leaves Gregorian selectors with different occurrence dates.

## Decision

Keep one selector pipeline driven by the existing expansion table.
Read date fields and spans through a calendar adapter with bounded year metadata.
Represent regular and leap month selectors with validated month identities.
Apply month correction before day selection and day correction before weekday selection.
Preserve Gregorian recurrence keys, ordering, counts, budgets, and cursor semantics.
Report arithmetic failure as an incomplete search with its own terminal result.
Refuse week correction combinations while their specified behavior remains unresolved.

## Alternatives

Independent selector engines can drift in processing order and accounting.
Converting only final dates leaves period selection in the wrong calendar.
Treating arithmetic failure as exhaustion or completion hides the cause.

## Consequences

Internal date helpers become fallible and calendar aware.
Public conversion types and Gregorian identifiers keep their contracts.

## Confirmation

Require RFC examples, every supported calendar, correction order, selector combinations, boundaries, counts, and resumed searches.
Reconsider when calendar algorithms or recurrence requirements change.
