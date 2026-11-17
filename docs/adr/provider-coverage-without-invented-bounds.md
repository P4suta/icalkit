---
date: 2026-11-18
status: proposed
---

# Provider coverage without invented bounds

## Context

The public zone port reports coverage independently for local and offset answers.
The adapter labels every answer computed.
[RFC 7808 section 7.1](https://www.rfc-editor.org/rfc/rfc7808.html#section-7.1) distinguishes explicit validity bounds from transition dates.
A false coverage flag supplies no date.

## Decision

Represent unasserted coverage with a distinct internal answer basis without a boundary date.
Combine local coverage with every offset answer used by its resolution.
Preserve usable readings.
Expose a `time-zone-coverage-unasserted` note through `Session::issues()`.
Keep one note per session across recurrence, queries, projection, and scheduling, including failed operations.
Centralize source lifetimes and note collection in one session operation wrapper.

## Alternatives

Ignoring the flag misstates the provider's claim.
Refusing usable answers removes the caller's continuation choice.
Per-result reports require broader result types and storage.

## Consequences

The note describes the session's work and doesn't identify individual occurrences.
Internal answers keep their individual basis.
Note storage has a constant bound.
Each session retains its own notes.
Reconsider when consumers need per-result coverage metadata.

## Confirmation

Test independent flags, failure paths, repeated operations, session isolation, diagnostic channels, and existing dated continuation semantics.
Verify both API snapshots, the shared ADR gate, and the maintained project checks.
