---
date: 2026-11-18
status: proposed
---

# Release live counters after owned decoding

## Context

Decoders own parser state but borrow a shared meter.
Rejecting a body can drop its reader while leaving live depth and namespace counts charged.

## Decision

Wrap owned decoding in a scope guard that snapshots live component depth, XML depth, and prefix bindings.
Restore only those counters on scope exit, using Rust's [destructor semantics](https://doc.rust-lang.org/std/ops/trait.Drop.html).
Keep cumulative charges, diagnostics, and exhaustion unchanged.

## Alternatives

Manual cleanup can miss early returns.
Draining rejected input can fail or charge more work.
Fresh meters lose the shared work budget.

## Consequences

Owned decoding releases temporary nesting and bindings before the next operation.
Reconsider when decoding state must outlive one call.

## Confirmation

Test rejected roots, entry live counts, preserved charges, and exhaustion after cleanup.
Run maintained gates.
