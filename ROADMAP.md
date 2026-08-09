# Roadmap

Everything here is text in, text out. No network, no clock, no hardware: every milestone
is verifiable by `cargo test` on `ubuntu-latest`.

## M0 — Model and round trip

`ical-grammar` and `ical-core`: the RFC 5545 content line grammar, the component and property
model, and serialization. The milestone is complete when a corpus of real client exports
parses and serializes back byte-identically
([ADR 0001](docs/adr/0001-lossless-round-trip.md)).

Round-trip fidelity first, before anything is interpreted, because it is the property every
later layer has to preserve.

What now binds this milestone. The token layer is the parser and `Document::parse` is one
consumer of the same public path, with no private fast path
([ADR 0008](docs/adr/0008-parser-layering-and-pull-api.md)). Storage is owned bytes charged
against the caller's budget as they are appended, never sliced out of pre-unfold input
([ADR 0007](docs/adr/0007-allocation-policy.md)). A violation is a `Diagnostic` on a sink that
may refuse, and an error means no item could be built at all
([ADR 0009](docs/adr/0009-error-and-diagnostic-model.md)).

Gates this milestone owes, because they cannot be written before the code they read: the
`DiagnosticCode` golden list and its diff check, a round-trip property test over the corpus, a
fold that splits a UTF-8 codepoint, a CP1252 `SUMMARY`, a hostile input of 200,000 one-byte
properties, a peak-allocation ceiling as a multiple of input size, and a structural test that
`Document` is built from the public token path.

## M1 — Recurrence

`ical-recur`: `RRULE` expansion over a caller-supplied window, with `RDATE`, `EXDATE`, and
overridden instances applied inside the iterator. Bounded by a candidate budget, so a
hostile rule is a reported limit breach rather than a hang
([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)).

This is the milestone that makes a month view possible, and the one where every calendar
implementation historically gets things wrong.

What now binds it. The budget is a field of the shared `Limits` and is charged per candidate
*generated*, not per instance emitted, against a `Meter` whose lifetime is the caller's
([ADR 0010](docs/adr/0010-shared-resource-limits.md)). An instance whose date does not exist
is filtered rather than clamped, and it still debits the meter
([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)).

Gates this milestone owes: a `FusedIterator` test that calls `next` past the end, a
`COUNT`-bounded resume matching a from-scratch expansion, a negative `BYSETPOS` that cannot
outspend its budget inside one `next`, a `RANGE=THISANDFUTURE` override that changes only a
non-time property, and an instant present in both an `EXDATE` and an override table.

## M2 — Time zones

`ical-tz`: `VTIMEZONE` interpretation against a caller-supplied source, DST transition
handling, and explicit representation of ambiguous and non-existent local times. Reporting
where the embedded rules and IANA disagree.

What now binds it. The civil-time primitives are `ical-core`'s and every operation on them is
checked ([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)); this crate
owns the resolution types and the source trait, and every caller-facing outcome enum is
`#[non_exhaustive]`. Transition search takes the shared limits and meter
([ADR 0010](docs/adr/0010-shared-resource-limits.md)).

Gates this milestone owes: a compiled `ZoneSource` sketch before implementation starts, and a
spring-forward and fall-back case per resolution outcome.

## M3 — Scheduling

`ical-itip`: RFC 5546 message semantics as described transitions, with authorization —
an attendee cannot move a meeting by replying. iMIP (RFC 6047) as a thin layer over the
same state machine.

What now binds it. The change vocabulary is `ical-core`'s, so the dependency runs one way and
cannot invert ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md)). Public error
enums crossing this crate's boundary are `#[non_exhaustive]`. A transition addresses a
property *occurrence*, not a property name, because a message changes one attendee among
many.

## M4 — CalDAV

`ical-dav`: RFC 4791 requests and responses, sans-I/O, usable from both sides. Calendar
collections, `REPORT` queries, `ETag` conditional writes, and sync tokens.

At this point writing a calendar client or a self-hosted server in Rust becomes a
reasonable thing to attempt, which it currently is not.

What now binds it. The XML tokenizer is this crate's own, namespace-resolving and bounded, and
no outside XML crate may be added ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)).
Reading a multi-status is an incremental decoder holding one `DavResponse` at a time and
writing one is an incremental encoder, with the owned `MultiStatus` as one optional consumer.
Per-property status is a `PropStat` list, a `time-range` has two independently optional
bounds, and a collection field is private behind a capped push.

Gates this milestone owes: compile-checked examples for those three shapes, and the
incremental codec pair compiling under `no_std` on `thumbv7em-none-eabi`, which is the part
this design has never proved.

## M5 — Interoperability evidence

`ical-conform` grown into a published differential corpus: what Google, Microsoft 365, and
Apple each emit and accept, where they disagree with the RFC and with each other, and what
this project chose. Runnable against any implementation, including ones that are not this
one.

What now binds it. A case is addressed to a specification section and evaluated through the
subject trait, and it states the `Limits` policy it ran under, because an outcome that depends
on a budget is not reproducible without one
([ADR 0010](docs/adr/0010-shared-resource-limits.md)). A case asserting a diagnostic asserts a
`DiagnosticCode` and its channel, which is what the golden list of
[ADR 0009](docs/adr/0009-error-and-diagnostic-model.md) exists to keep stable.

Gates this milestone owes: the foreign-implementation bridge job, which needs an external
runtime in CI and a kill wrapper around the child process — neither exists today, and until
both do the bridge is a best-effort check rather than a gate.

## Non-goals

Bundling a time zone database or an HTTP client. Reading the system clock. vCard and
CardDAV — the same shape, a different specification, and a decision to make later rather
than a scope to assume now.

An allocation-free tier is a named gap rather than a non-goal: it belongs to a future crate
with its own lint profile, not to a feature flag on these
([ADR 0007](docs/adr/0007-allocation-policy.md)). `just no-std` proves these crates build for
`thumbv7em-none-eabi`, not that they build without a global allocator.
