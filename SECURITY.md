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
CalDAV session, or an authenticated mail envelope. `Message::read_imip` checks the RFC 6047
`Content-Type` media type, charset declaration, and method agreement with the calendar body; it
does not authenticate `From` or turn it into an `Actor`. Handing the scheduling gate an address
taken from a `From:` header nobody verified reproduces the whole of the classic forged invitation,
and no gate here can see that caller's code.

**The iMIP body passed to icalkit must already be transfer-decoded.** The mail implementation must
remove Content-Transfer-Encoding before calling `Message::read_imip`. Passing the base64 or
quoted-printable wire spelling can make an all-ASCII wrapper appear consistent with an absent
charset while hiding non-ASCII decoded content. The strict calendar parser may reject such input,
but that is not a substitute for performing the transport decode at the MIME boundary.

**What it compares the actor against depends on whether the caller holds the component.** When
it does, the actor is looked up in the caller's own copy — the `ORGANIZER` line a recipient
already has is the one statement about who runs this meeting that the sender did not write.
RFC 5546 lets exactly two methods act on nothing: `PUBLISH` and `REQUEST`, which exist to arrive
first. For those the actor is looked up in the message, so the gate proves the actor is a party
*the message names* and nothing more. A first invitation is therefore exactly as trustworthy as
the transport that delivered it. Every later message about the same identity is held against
what the caller already has.

**It proves no storage freshness.** `AuthorizedChange` borrows the exact `Message` and `Calendar`
that were reviewed and is consumed by `apply`, so the capability cannot be encoded, stored, or
replayed. What no Rust lifetime can see is that the borrowed calendar is a snapshot read minutes
ago. The caller must bind that snapshot to an `icalkit::caldav::Revision` and use the resulting
strong `If-Match` or `If-None-Match` condition when writing. A failed precondition means reread,
review, and authorize again; applying the old result after a racing organizer update is unsafe.
`Revision` rejects weak entity tags because they cannot protect a state-changing write.

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
each answer was written at is recorded on the line it answers for. The built-in `Calendar` bridge
uses the private `X-ICALKIT-ANSWERED-AT` parameter; a caller that converts the calendar into
another storage model must preserve an equivalent value. **A state that records no such time
cannot order two answers and admits the second**: storage that discards the witness keeps the
change-of-mind case working and loses the defense against an attendee's own earlier answer being
replayed.

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

Until the first release, security fixes target the current `main` branch only. After releases
begin, only the latest released series will receive fixes unless a security advisory says
otherwise.

[advisories]: https://github.com/P4suta/icalkit/security/advisories/new
