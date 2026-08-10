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

**An attendee-side message may write only its own participation, and nothing that decides who
may write next.** `ORGANIZER` and `SEQUENCE` are the organizer's: an attendee-authored message
may restate either and may not change either, because the first is who runs the meeting (RFC
5546 section 3.2.7 gives an attendee no authority over it) and the second is the number this
protocol's whole replay defense reads. "Its own `ATTENDEE` line" is two questions — the line has
to be the one the sender sits at, and the line the change leaves behind has to still name the
sender — so a `COUNTER` substituting a party the meeting never invited for the sender is
refused rather than treated as an edit to the sender's own line.

**Version ordering refuses what it cannot order, and a reply is ordered against the answer it
replaces.** At an equal `SEQUENCE` a revision that states a readable `DTSTAMP` supersedes one
that does not, so a message whose `DTSTAMP` is written in a spelling nothing can read has not
won the tie it declined to offer. An organizer-authored message that is neither newer nor older
than the state describes nothing, because RFC 5546 section 2.1.4 requires an update to increment
`SEQUENCE` and two messages at one revision are one version. Two replies from one attendee are
one revision answered twice and the component's own `DTSTAMP` cannot order them, so the time
each answer was written at is recorded on the line it answers for — `ical_itip::ANSWERED_AT` in
the shipped bridge, `ScheduledComponent::attendee_answered_at` for a store keeping its own
column. **A state that records no such time cannot order two answers and admits the second**: a
caller whose storage discards that parameter keeps the change-of-mind case working and loses the
defense against an attendee's own earlier answer being replayed.

**Absence is absence, and identity is compared or the message is refused.** A component that
states anything at all is a component the caller holds, so the payload fallback above is reached
only for a state that is genuinely empty — a `UID` that cannot be read no longer downgrades a
held meeting into an absent one, which is how a stranger was once authorized to rewrite the
organizer line of a component the caller was holding.

**The work of judging a message is bounded before the message exists.** `ItipMessage::read`
counts and charges a payload's properties against `Limits::max_payload_properties` as well as
its attendees, components and depth, because `evaluate_message` takes no ledger and describes a
transition per property occurrence. Without that, one message read for four units could cost a
hundred thousand allocations to judge, and an inbox sharing one meter would be bounded in the
number of messages it read rather than in the work they cost.

## Reporting

Report privately through GitHub's ["Report a vulnerability"][advisories] flow rather than a
public issue. Include the calendar or message that reproduces, reduced and with personal
data removed.

Expect an acknowledgement within seven days.

## Supported versions

While the project is pre-1.0, only the latest release receives fixes.

[advisories]: https://github.com/P4suta/icalkit/security/advisories/new
