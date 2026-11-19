---
date: 2026-11-17
status: proposed
---

# Stream conformance exchanges

## Context

The conformance subject flushes one response per request.
Sending every request before reading responses can deadlock on [bounded pipes](https://doc.rust-lang.org/std/process/struct.Stdio.html#method.piped).
Rust's [`Child`](https://doc.rust-lang.org/std/process/struct.Child.html) leaves cleanup to its caller.

## Decision

Send and flush one request, then consume its response before sending another.
Close input after the last request and reject missing, malformed, or extra responses.
Keep child ownership until waiting succeeds.
On an early return, stop and wait for the unfinished subject while preserving the primary error.
Identify failed phases without recording request contents.

## Alternatives

A concurrent input pump permits pipelining but needs cancellation, joining, and error coordination.
Writing all requests first preserves the cyclic wait.

## Consequences

The client adds serial round trips without changing the protocol.
Cleanup covers the direct subject, which creates no descendants.
Revisit if the protocol permits out-of-order responses or needs pipelining.

## Confirmation

A bounded transport model checks ordering.
Native tests check large exchanges and cleanup on early returns without sleeps.
Framing and injected input/output failures check diagnostics.
Maintained gates must pass.
