# ADR-0005: scheduling semantics live apart from the data model

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-11 (three times)

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

## Amendments

**1. A transition addresses a property occurrence, and the address grows a second door rather
than widening the first.** This document settled the *vocabulary* — `ProposedChange` and
`ParameterEdit` are `ical-core`'s and are reused — and left the *address* stated as
`BTreeMap<PropertyId, ProposedChange>`. `PropertyId` names an identity, and M0 made every
`Component::apply` variant identity-addressed for a reason this amendment does not disturb: a
caller naming an identity that two properties carry must not get a half-applied `Replace`,
because the identity would then carry two values with no way to see it. ROADMAP M3 requires the
other thing, because a `REPLY` changes one `ATTENDEE` among many.

Both are true, and they are questions about different addresses. So the map is keyed on
`ical_itip::PropertyOccurrence` — an `ical_core::PropertyId` plus a zero-based index among the
properties of that name directly inside one component — and `ical-core` gains
`Component::apply_to_occurrence(&PropertyId, usize, &ProposedChange, Limits)` beside
`Component::apply`, which is unchanged. No shared type is widened, no lookup below `ical-itip`
narrows, and the vocabulary is still one vocabulary. `ProposedChange::Add` has no occurrence to
name yet, so its index must be the append position and any other index is
`MutationError::Absent`: an addition landing anywhere else renumbers every occurrence after it.

**2. `AuthorizedTransition` is `Authorization<'a>`, and what it guarantees across bytes is
nothing, enforced rather than documented.** This document says the wrapper "has no serialized
form either, and that is deliberate", and then rests that on nobody writing one. A red-team
round found what is underneath: a sealed wrapper whose only guarantee is "this crate built it"
guarantees nothing once a propose-then-confirm exchange crosses a request boundary, because
whatever the caller encodes to carry it there is forgeable there, and the seal then attests to
the transport.

So the type *borrows both of its inputs*: `Authorization<'a>` holds `&'a ItipMessage<'a>` and
`&'a dyn ScheduledComponent`. There is no owned form to encode and no owned form to
reconstruct, so a caller that tries to carry one across a request gets a compile error instead
of a forgeable token. `apply_transition` takes it by value, so it is single-use. What that
still does not prove is freshness, which no lifetime can see; `Commitment` is the one value
designed to cross bytes, it carries no authority, it is compared only to cause a refusal, and
its digest is a checksum and not a MAC. An attacker who forges one gains exactly the ability to
decline to be told that the target moved, and the gate ran fresh either way. Nothing may be
changed to grant on a `Commitment`.

**3. Instance identity carries a fold side, and an ambiguous match is a denial.** M2 left
`RECURRENCE-ID` on a zoned series ambiguous: the two halves of a repeated hour are one cadence
key on `ical_tz::seam`'s nominal timeline, so a `REPLY` or a `CANCEL` naming one of them cannot
be told from one naming the other (ADR-0011 amendment 3). `ical_itip::InstanceRef` therefore
carries an `ical_itip::FoldSide` derived from an `ical_tz::LocalResolution` the caller already
holds, comparison is the three-valued `InstanceMatch`, and `evaluate_message` refuses
`InstanceMatch::Ambiguous` as `AuthorizationDenied::AmbiguousInstance`. A guess between the two
halves cancels somebody else's meeting; M2 bounded that damage and this refuses it.

**4. The party a message is from is looked up in the message when the caller holds nothing, and
that is a stated cost rather than an oversight.** This document has the gate judge the actor
against `current`, the component the caller already holds, and that is right for every message
about something a recipient has: an `ORGANIZER` line already in the recipient's copy is the one
statement about who runs this meeting that the sender did not write. RFC 5546 admits exactly two
methods against an absent prior state — `PUBLISH` (section 3.2.1) and `REQUEST` (section 3.2.2)
— and both exist to arrive before the recipient has anything. Looking the actor up in state that
names nobody answers `None` for both, so as first written the gate refused every invitation
`OrganizerMismatch` and `TransitionReason::Created` was unreachable: the two methods the
protocol is *for* could never succeed.

So the lookup falls back to the matched payload when, and only when, the prior state is absent.
What this buys is that a first invitation can be accepted at all. What it costs is that for a
first message the gate proves the supplied actor is a party *the message names* and nothing
more, because there is nothing else in the room to compare it to. That is not a weakening of the
model — nothing was ever being compared for a component the caller does not hold — but it is a
place where the whole of the trust rests on the transport, and `SECURITY.md` now says so in the
same words. Every later message about the same identity is judged against what the caller holds.

**5. The constraint tables' `SUBCOMPONENTS` rows are part of the gate, and a forbidden component
gets its own refusal.** The conformance corpus, written from RFC 5546 rather than from this
implementation, found that the gate counted a payload's properties against section 3 and never
its components — so section 3.2.3's `VALARM: 0` row was unenforced and an attendee's `REPLY`
could install a component the recipient's client will act on. The refusal is
`AuthorizationDenied::MethodForbidsComponent(ComponentKind)` and not the existing
`MethodForbidsField` carrying a `PropertyOccurrence` that spells `VALARM`: a nested component is
not a property occurrence, and a caller told otherwise would look the name up among the
payload's properties and find nothing there. Only the forbidden direction is read, because every
`SUBCOMPONENTS` row section 3 prints is `0` or `0+` — machine-checked against the transcribed
tables rather than asserted here, so a row transcribed later that does require a nested
component fails a test instead of going quietly unenforced. The `COMPONENTS` rows stay unread by
the gate: `ItipMessage::read` already refuses a second payload kind and a payload the tables
never nest at the top level, earlier and for the whole message, and two gates over one rule are
two places for the answer to drift.

**6. A method that states no revision is not ordered against one, and a `REFRESH` describes
nothing.** Sections 2.1.4 and 2.1.5 order versions, and this document applies them to every
message. Section 3.2.6's `REFRESH` table gives `SEQUENCE` the value `0`: a refresh asks for the
latest version and states no version of its own, so reading a revision out of it yields the
absent-is-zero reading and makes every refresh stale against every held revision above zero. The
revision gate therefore runs only for a method whose own table admits a `SEQUENCE` — read from
the transcribed table, not special-cased on the method — because a message that states no
revision overwrites nothing and there is nothing for those two sections to order.

The same method needed the second half. A `REFRESH` diffed as a restatement of the component
describes the removal of every property its four lines do not echo — the organizer's `DTSTART`,
`RRULE` and attendee list — and the field rule then refuses the attendee for removals the diff
invented rather than for anything the attendee wrote. `describe_payload` answers an empty
transition for it, the way it already answered one attendee's line for a `REPLY`, and for the
same reason: RFC 5546 says what these two methods are about, and a general octet diff says
something else.
