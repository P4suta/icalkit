---
date: 2026-11-18
status: proposed
---

# Compile annual clock products as bounded patterns

## Context

[RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) applies positions and bounds after clock expansion.
Materializing every date and clock can exceed thirty million candidates per year.
The existing lookup expects one date and clock per scalar annual pattern.

## Decision

Rank complete date-time products before projecting scalar patterns.
Reuse calendar and week-year summaries with wide counts.
Group clocks by date positions and bound cutoffs.
Keep only the earliest patterns within the remaining admission budget plus one witness, using the table's complete ordering.
Keep raw leap-second fields until existing instant conversion.

## Alternatives

Accepting only matching clocks leaves valid rules unsupported.
Expanding products consumes input-dependent storage.
A time-set lookup changes every transition search and its coverage contract.

## Consequences

Lookup remains scalar.
Admission adds bounded ranking and sorting work.
Reconsider when nonannual observance rules need a different lookup model.

## Confirmation

Compare independent products, global bounds, week-year edges, and admission prefixes.
Measure full clock products with small budgets.
Run the shared ADR checker and maintained checks.
