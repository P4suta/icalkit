---
date: 2026-11-18
status: proposed
---

# Keep earliest observances with bounded staging

## Context

The reader buffers every transition before applying the shared limit.
Truncation keeps unused vector capacity.
Tables select earliest UTC onsets, with complete values breaking ties.

## Decision

Use a bounded priority queue with the table's ordering key.
Keep the earliest candidates within the remaining budget plus one overflow witness.
Table admission charges the meter and reports truncation once.
Store admitted values in an immutable slice without spare capacity.
Track source readability independently of buffer length.

## Alternatives

Keeping a source prefix changes results with input order.
Sorting all candidates leaves unbounded staging.
Charging during reading duplicates admission responsibility.

## Consequences

Temporary storage depends on the remaining limit.
Reading still scans all source values.
Compacting a supplied vector can resize its allocation.
Reconsider when clock products need selection without visiting every candidate.

## Confirmation

Measure reader staging and supplied-vector retention.
Compare order permutations, compiled rules, spent and zero budgets, and unreadable values.
Run the shared ADR checker and maintained checks.
