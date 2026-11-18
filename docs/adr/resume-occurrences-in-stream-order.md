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

Keep the count and last committed rule key.
Track the delivered addition prefix and indices beyond gaps.
Charge the session before allocating or copying marks.
Calendar queries resume after the last start and identity for unchanged calendar, identifier, and zone inputs.

## Alternatives

The greatest delivered identity skips earlier unseen additions.
Each partial search can leave gaps outside the last window.
Array positions change between windows.

## Consequences

In-order additions need one counter.
Input length bounds extra storage.
Standalone rule cursors keep fixed-size state.
Revisit calendar positions if the output order changes.

## Confirmation

Test shifted additions, budget refusal, and replay.
Check equal starts, empty windows, and adjacent windows.
