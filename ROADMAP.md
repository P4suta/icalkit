# Roadmap

Everything here is text in, text out. No network, no clock, no hardware: every milestone
is verifiable by `cargo test` on `ubuntu-latest`.

## M0 — Model and round trip

`ical-core`: the RFC 5545 content line grammar, the component and property model, and
serialization. The milestone is complete when a corpus of real client exports parses and
serializes back byte-identically
([ADR 0001](docs/adr/0001-lossless-round-trip.md)).

Round-trip fidelity first, before anything is interpreted, because it is the property every
later layer has to preserve.

## M1 — Recurrence

`ical-recur`: `RRULE` expansion over a caller-supplied window, with `RDATE`, `EXDATE`, and
overridden instances applied inside the iterator. Bounded by a candidate budget, so a
hostile rule is a reported limit breach rather than a hang
([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)).

This is the milestone that makes a month view possible, and the one where every calendar
implementation historically gets things wrong.

## M2 — Time zones

`ical-tz`: `VTIMEZONE` interpretation against a caller-supplied source, DST transition
handling, and explicit representation of ambiguous and non-existent local times. Reporting
where the embedded rules and IANA disagree.

## M3 — Scheduling

`ical-itip`: RFC 5546 message semantics as described transitions, with authorization —
an attendee cannot move a meeting by replying. iMIP (RFC 6047) as a thin layer over the
same state machine.

## M4 — CalDAV

`ical-dav`: RFC 4791 requests and responses, sans-I/O, usable from both sides. Calendar
collections, `REPORT` queries, `ETag` conditional writes, and sync tokens.

At this point writing a calendar client or a self-hosted server in Rust becomes a
reasonable thing to attempt, which it currently is not.

## M5 — Interoperability evidence

`ical-conform` grown into a published differential corpus: what Google, Microsoft 365, and
Apple each emit and accept, where they disagree with the RFC and with each other, and what
this project chose. Runnable against any implementation, including ones that are not this
one.

## Non-goals

Bundling a time zone database or an HTTP client. Reading the system clock. vCard and
CardDAV — the same shape, a different specification, and a decision to make later rather
than a scope to assume now.
