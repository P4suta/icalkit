# Security Policy

## Scope

Calendar data is untrusted input that arrives by design: an `.ics` attachment in an email
from a stranger, a `REPORT` response from a server the user does not control, an iTIP
message inviting the user to a meeting. Callers embed this in mail clients and servers.

In scope:

- A panic, hang, or unbounded allocation on any input. A recurrence rule that exhausts the
  candidate budget must report that, not spin
  ([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)); a bypass of an injected limit is
  a vulnerability
- **Scheduling authorization failures.** An attendee changing an event's time through a
  `REPLY`, a `REPLY` from an address not on the attendee list being accepted, or a stale
  `SEQUENCE` overwriting a newer version. These are the positions where calendaring
  implementations have historically been exploited
  ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md))
- XML external entity or entity expansion handling in the CalDAV layer
- Data loss on round trip that a caller could not detect — silently dropping a property is
  a correctness bug, but silently dropping an `ATTENDEE` is closer to a security one

Out of scope:

- Accepting a calendar that violates RFC 5545. Real calendars do, constantly, and rejecting
  them is not a security posture ([ADR 0001](docs/adr/0001-lossless-round-trip.md))
- A time zone answer you disagree with when the embedded `VTIMEZONE` and IANA differ. Both
  are reported; choosing between them is the caller's
  ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md))

## What the scheduling gate proves, and what it does not

The bullet above says authorization failures are in scope. M3 shipped the gate, so this
section states what that sentence can honestly claim — a report is worth filing against these
guarantees and not against the ones nobody made.

**It never authenticates a sender.** `evaluate_message` judges the actor *the caller supplies*.
Establishing that the actor really sent the message is the transport's job: an authenticated
CalDAV session, or the RFC 6047 envelope checks behind the `imip` feature. Handing the gate an
address taken from a `From:` header nobody verified reproduces the whole of the classic forged
invitation, and no gate here can see that caller's code.

**What it compares the actor against depends on whether the caller holds the component.** When
it does, the actor is looked up in the caller's own copy — the `ORGANIZER` line a recipient
already has is the one statement about who runs this meeting that the sender did not write.
RFC 5546 lets exactly two methods act on nothing: `PUBLISH` and `REQUEST`, which exist to arrive
first. For those the actor is looked up in the message, so the gate proves the actor is a party
*the message names* and nothing more. A first invitation is therefore exactly as trustworthy as
the transport that delivered it. Every later message about the same identity is held against
what the caller already has.

**It proves no freshness.** An `Authorization` borrows both of its inputs and has no owned form,
so it cannot be encoded, stored in a session, or replayed — a caller that tries gets a compile
error rather than a forgeable token. What no lifetime can see is that the borrowed state is a
snapshot read minutes ago. A `Commitment` crosses a request boundary and deliberately carries
**no authority**: it is compared only to cause a refusal, its digest is a checksum and not a MAC,
and an attacker who forges one gains exactly the ability to decline to be told that the target
moved. The gate ran fresh either way. Binding a transition to an `ETag` is undesigned, so the
propose-and-confirm flow is not safe against a racing organizer update and must not be described
as if it were.

**Refusal is whole, and a refused message stays inspectable.** There is no partial success: a
message that overreaches on one property is denied entire, because applying its permitted half
would leave the caller holding a component no party ever described. `describe_message` hands a
caller what a denied message *tried* to do without handing it the ability to do it.

## Reporting

Report privately through GitHub's ["Report a vulnerability"][advisories] flow rather than a
public issue. Include the calendar or message that reproduces, reduced and with personal
data removed.

Expect an acknowledgement within seven days.

## Supported versions

While the project is pre-1.0, only the latest release receives fixes.

[advisories]: https://github.com/P4suta/icalkit/security/advisories/new
