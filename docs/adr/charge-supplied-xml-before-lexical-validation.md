---
date: 2026-11-18
status: proposed
---

# Charge supplied bodies before lexical validation

## Context

The [shared limits decision](0010-shared-resource-limits.md) requires hostile input to consume one running budget.
XML lexical validation scans the supplied body before protocol parsing can reject its root.
Charging delivered spans leaves failed scans outside the budget.

## Decision

Admit the complete body length before lexical validation.
Carry a private checked body type in the content phase.
Decode text spans from that body without charging wire bytes again.
Keep standalone text decoding, text width caps, and storage charges metered.

## Alternatives

Current span charging misses failed scans.
Incremental lexical charging needs common progress tracking across both parsing passes.

## Consequences

Protocol refusal leaves the admitted body length charged.
Input-budget refusal precedes syntax and storage work.
Reconsider when lexical validation becomes incremental.

## Confirmation

Reproduce repeated malformed calls and unread tails.
Test exact byte ceilings, references, comments, text caps, storage charges, and error precedence.
Run maintained gates.
