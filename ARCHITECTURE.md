# Architecture

This document states the invariants. The reasoning behind each one lives in
[`docs/adr/`](docs/adr/); this file is the summary a reader needs before touching code.

## Shape

```text
        your application (has an HTTP client and a tzdb already)
                              ▲
   ical-dav ── ical-itip ── ical-recur ── ical-tz          ← semantics
                              ▲
                          ical-core                        ← model, parse, serialize
```

Nothing here opens a connection, reads a clock, or bundles time zone data. Everything
above `ical-core` is a separate crate so that a caller who only reads `.ics` files never
compiles scheduling or CalDAV.

## Invariants

Enforced by `just purity`, `just no-std`, and `just wasm` — not by convention. A change
that violates one fails CI.

1. **Nothing is lost on a round trip**
   ([ADR 0001](docs/adr/0001-lossless-round-trip.md)). Unknown properties, parameters, and
   components are preserved in position. Typed access is a view over preserved content,
   never the storage. `parse → serialize` is byte-identical across the whole corpus.

2. **Recurrence is lazy and doubly bounded**
   ([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)). The caller supplies a window;
   the search itself has a candidate budget whose exhaustion is a reported outcome. There
   is no function that expands a rule into a `Vec`.

3. **Time zone data comes from the caller**
   ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md)). No bundled tzdb, no system
   clock. Where the embedded `VTIMEZONE` and IANA disagree, that is reported, and every
   result names its source.

4. **The protocol layer is sans-I/O and `no_std`**
   ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)). `ical-dav` produces requests and
   interprets responses; the same code serves clients and servers.

5. **Scheduling is separate from the model**
   ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md)). `ical-itip` returns a
   described transition, not a mutation, and authorization is part of the semantics.

6. **The conformance corpus is a deliverable**
   ([ADR 0006](docs/adr/0006-conformance-corpus-as-artifact.md)). Real client exports,
   reduced and anonymized, runnable against any implementation.

## Crate boundaries

| Crate | Depends on | std | Reads a clock |
| --- | --- | --- | --- |
| `ical-core` | — | no | no |
| `ical-recur` | `ical-core` | no | no |
| `ical-tz` | `ical-core` | no | no |
| `ical-itip` | `ical-core`, `ical-recur`, `ical-tz` | no | no |
| `ical-dav` | `ical-core` | no | no |
| `ical-conform` | all of the above | yes | no |

"Reads a clock" is a column because a calendar library that quietly asks the OS for the
current time is untestable: the answer to "is this event in the past" must come from an
instant the caller passed in.

## What lives where

- **Preservation lives in `ical-core`.** Every layer above it operates on the preserved
  model and must not require reserializing through a lossy typed form.
- **Ambiguity is represented, not resolved.** Non-existent and repeated local times at DST
  transitions, disagreeing time zone sources, and specification violations are all values
  a caller can inspect, not errors that discard the input.
- **Diagnostics travel with the item they concern,** so a caller can accept a
  specification-violating calendar and still know what was wrong with it.
