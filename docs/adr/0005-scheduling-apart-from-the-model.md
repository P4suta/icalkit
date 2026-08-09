# ADR-0005: scheduling semantics live apart from the data model

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

## Context

iTIP (RFC 5546) is a state machine over messages: an organizer publishes a `REQUEST`, attendees
return a `REPLY`, the organizer sends a `CANCEL` or a partial update, sequence numbers arbitrate
which version wins, and the whole exchange has rules about who is permitted to change what.

Those rules are not properties of an `.ics` file. They are properties of a *conversation* between
parties, and they need state the file does not carry: who am I in this exchange, what did I last
see, and is this message authorized to make the change it is asking for.

Folding scheduling into the calendar model means every caller who just wants to read an `.ics`
file drags it along, and it means the model acquires a notion of identity that parsing has no
business having.

## Decision

`ical-core` knows about components and properties. It does not know what a `METHOD` means, who the
organizer is relative to the current user, or whether a `SEQUENCE` bump should be accepted.

`ical-itip` is a separate crate that takes an incoming message, the current state of the event,
and the identity of the party applying it, and returns what changes — as a description of the
transition, not as a mutated object. Applying it is the caller's decision. It sits above
`ical-core`, `ical-recur` and `ical-tz` in the crate graph, with nothing but the conformance crate
above it, and `xtask` checks that direction so the type coupling described below cannot invert.

Authorization is part of the semantics, not an afterthought: an attendee cannot change an event's
time by replying, and a `REPLY` from an address that is not on the attendee list is a rejected
message rather than a silent participant addition. Those are exactly the positions where
scheduling implementations have historically been exploited.

Concretely: the transition reuses `ical-core`'s per-property change vocabulary rather than
inventing a parallel one — `pub struct Transition { changes: BTreeMap<PropertyId, ProposedChange>,
reason: TransitionReason }`, a map rather than a `Vec`, so two conflicting changes to one property
cannot both be constructed. Identity is a borrowed CAL-ADDRESS, `pub struct PartyId<'a>(&'a str)`,
matched against the attendee list by RFC 5321 local-part rules including addresses relayed via
`SENT-BY`, not by a blanket ASCII case fold. The evaluating function is

`pub fn evaluate_message(message: &ItipMessage, current: &Component, actor: PartyId<'_>) ->
Result<AuthorizedTransition, AuthorizationDenied>`

and `apply_transition` accepts `AuthorizedTransition`, never a bare `Transition`, so an unvetted
transition cannot be applied by construction rather than by convention. `AuthorizationDenied` is
`#[non_exhaustive]`, with variants including `UnknownAttendee`, `OrganizerMismatch`,
`SequenceStale { have: u32 }` and `MethodForbidsField(PropertyId)`. A failure therefore appears as
an `Err` at evaluation time, before any transition value exists to apply — the concrete shape of
the rejected message above.

The wrapper is not generic and is not shared. It is `ical-itip`'s own `pub struct
AuthorizedTransition(Transition)`, with no public constructor, no public field, no `From`,
`Default`, or from-parts `Clone`, and no equivalent in any other crate; `ical-core`'s mutation API
produces it never and consumes it never. A wrapper parametrized only on what changed would prove
that some sealed constructor ran somewhere, not that RFC 5546's attendee-list, field-permission
and `SEQUENCE` checks ran for this value, and Rust has no crate-family privacy, so a wrapper
shared with `ical-core` would need a public constructor and would not be sealed at all. What the
two crates share is the vocabulary for describing a change, never the capability to apply one.

It has no serialized form either, and that is deliberate. ADR-0004 leaves this library no session
state, so a propose-then-confirm exchange crosses a request boundary; a wire encoding of
`AuthorizedTransition` would be a forgeable one, and the sealed constructor would then attest to
nothing but the transport. What the caller carries across the boundary is the message, not the
authorization, and `evaluate_message` runs again at the confirming turn against freshly read
state. The conformance corpus carries that two-turn exchange as a fixture. Implementers should
also keep a denied `REPLY`'s attempted changes inspectable — an `attempted: Transition` on the
denial, or an equivalent diagnostic path — rather than discarding them to a bare code, since
showing a change before it is applied is this ADR's own goal. That is a recommendation, not a
requirement.

## Consequences

A caller who only reads calendars never compiles the scheduling crate — now a checked claim rather
than an asserted one, since `xtask` compiles a minimal usage example per crate.

The transition being a value rather than a mutation means it can be shown to a user before being
applied, which is what a mail client actually needs when it displays "this meeting was moved —
accept?".

iMIP (RFC 6047), which carries the same messages over email, becomes a thin layer over the same
state machine rather than a second implementation of it.

Reusing `ical-core`'s change vocabulary couples the crates in the one place this ADR's own text
wanted kept clear: every newly mutable property `ical-core` learns must now also decide whether it
is iTIP-relevant. The dissent is worth keeping: nobody designing this fresh reached for the
unification, only reviewers weighing the tradeoff afterward did, so revisit it if that vocabulary
grows to where most properties are never iTIP-relevant.

Re-evaluating at the confirming turn closes forgery but not staleness. Nothing forces the second
call to read the component fresh rather than replay a snapshot from the first, and a genuine
`AuthorizedTransition` over a stale snapshot is still wrong. Binding a transition to an `ETag` or
sync-token is ADR-0004 territory and undesigned, so this remains a caller obligation with a
fixture behind it: the propose-and-confirm flow is not safe against a racing organizer update and
should not be described as if it were.

Two questions stay open underneath the gate. Whether a field diff compares preserved octets or
parsed values is settled nowhere; if it is octet-level, a CP1252-mangled value could report
"unchanged" for an organizer-only field an attendee touched, and the field-permission check has a
hole beneath it. The CAL-ADDRESS / `SENT-BY` / `SCHEDULE-AGENT` delegation rules are gestured at
above rather than specified. Beyond both, `RANGE=THISANDFUTURE` splitting, VALUE=DATE-safe
`DTSTART`, negative `BYSETPOS` and a fold across a codepoint remain unexercised through the reused
type: `ical-itip` is not RFC-5546-complete, and nothing here entitles anyone to say it is.
