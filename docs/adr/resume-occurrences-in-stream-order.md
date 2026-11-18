---
date: 2026-11-19
status: proposed
---

# Resume occurrences in stream order

## Context

[RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3) includes additions and collapses duplicates.
An override can move a later addition ahead of an unseen addition.
The [engine](0002-bounded-lazy-recurrence.md) merges by recurrence identity.
Calendar queries order by effective start and identity.

## Decision

Count each rule key once under [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Replay counted keys still eligible for later windows.
Track delivered additions across gaps.
Charge the session before storing or copying history.
With unchanged inputs, calendar queries resume after the last start and identity.
On interruption, publish only entries known to precede unseen entries.
For UTC, the greatest visited rule or addition key minus the largest shift bounds unseen starts.

## Alternatives

The greatest delivered identity skips earlier unseen additions.
Sorting an incomplete batch can publish a later start first.
Array positions change between windows.

## Consequences

In-order additions need one counter.
Session budgets bound pending rule history.
Standalone rule cursors keep fixed-size state.
Floating and zoned queries wait for complete placement.
Revisit calendar positions if the output order changes.

## Confirmation

Test counted gaps, shifted additions, and retries after a refused history copy.
Check equal starts, empty windows, and adjacent windows.
