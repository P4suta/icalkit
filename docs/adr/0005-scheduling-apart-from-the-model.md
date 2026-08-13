# ADR-0005: scheduling semantics live apart from the data model

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-11 (fourteen amendments)

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

A caller who only reads calendars never compiles the scheduling crate. That is still an asserted
claim rather than a checked one: `xtask` runs `purity` and `codes` and nothing else, no crate
carries a minimal-usage example, and the crate graph is all that stands behind the sentence.

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
sync-token was ADR-0004 territory and undesigned, and M4 designed it: `ical_dav::Revision`
carries what the first turn read into the `Precondition` the second turn writes under. What that
closes is the plumbing and not the guarantee — the freshness a caller gets is the freshness the
*server* enforces when it compares the `If-Match` — so this remains a caller obligation with a
fixture behind it, and the propose-and-confirm flow is still not safe against a racing organizer
update on its own.

Two questions stay open underneath the gate, and one of them is now half answered. A field diff
compares preserved octets — `diff.rs` chose that in M3 on the ground that its failure direction
is refusal rather than permission, and amendments 6 and 7 below reason from it. The hole beneath
it is exactly the one named here and is not closed: a CP1252-mangled value can report
"unchanged" for an organizer-only field an attendee touched, and no gate above the diff sees it.
**Amendment 14 closes it on the iMIP path only and narrows this sentence rather than striking
it — every other route into this crate still reaches the diff with octets nobody has vouched
for.** The CAL-ADDRESS / `SENT-BY` / `SCHEDULE-AGENT` delegation rules are gestured at above
rather than specified. **Amendment 13 specifies them, and one third of that sentence turns out
to be a category error: `SCHEDULE-AGENT` can never reach this gate.** The single public
`icalkit` facade now exercises organizer `REQUEST` splitting for `RANGE=THISANDFUTURE`,
including recurrence-set membership, transactional detached-component creation, later-anchor
updates and zone-gap refusal. The low-level transition crate still owns no container, and other
method-specific range behavior, VALUE=DATE-safe `DTSTART`, negative `BYSETPOS` and a fold across
a codepoint remain unexercised through the reused type: `ical-itip` is not RFC-5546-complete,
and nothing here entitles anyone to say it is.

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

**7. The attendee side may write its own participation and nothing that decides who writes
next.** Amendment 1 settled the *address* of a change and this document settled the
*vocabulary*; what neither settled is that a field permission is a statement about an octet
diff, and a diff records only what differs. `field_rule` gave `ORGANIZER` and `SEQUENCE`
`EitherParty` on the true observation that an attendee's `COUNTER` legally restates both — but a
restatement produces no entry and is never asked about, so the permission bought only the case
it was not written for: an attendee-authored line naming somebody else as `ORGANIZER`, which
hands the meeting away, and a `SEQUENCE` an attendee raises, which is the number the revision
gate then refuses every genuine organizer update against. Both are `OrganizerOnly`. In the same
place, `AttendeeOwn` was read as "the occurrence this actor sits at" and is now also "and the
line this change leaves behind still names this actor", because replacing one's own `ATTENDEE`
line with a stranger's satisfies the first reading exactly.

What this cannot reach is a state that already names the wrong party. A component whose
`ORGANIZER` line names an attendee says that attendee organizes it, and section 1.3 lets one
calendar user be both — every real invitation lists the organizer on the attendee list — so no
rule readable from that file alone can refuse them without refusing every organizer who attends
their own meeting. The defense is that no message may write that line, and the corpus asserts
the state is unreachable rather than asserting a reading of it that cannot exist.

**8. A component that states anything is a component the caller holds.** Amendment 4 made the
sending party's lookup fall back to the message when the prior state is absent, and priced that
honestly. What it did not say is how *absent* is decided, and the answer was "the `UID` did not
read" — which is a different question, and the wrong one. A held copy whose `UID` line appears
twice is a component the caller plainly has, and reading it as one the caller has nothing about
sent the sender's lookup into the attacker's own message, where the attacker is the `ORGANIZER`.
Absence is now the absence of everything: no property, no attendee, no nested component. A
`UID` that cannot be compared is a reason to refuse the message on identity, never a reason to
believe the recipient is holding nothing.

Underneath it, the bridge stopped refusing a name stated twice with byte-identical lines. Two
identical claims have no winner to pick between them, and refusing them was conservative about
the reading while being permissive about the consequence.

**9. Ordering is refused where it cannot be done, and a reply is ordered against the answer it
replaces.** Sections 2.1.4 and 2.1.5 are this protocol's whole replay defense, and three of
their edges were open. At an equal `SEQUENCE`, a revision stating a `DTSTAMP` now supersedes one
stating none — the tie is broken towards refusal in both directions, because the spelling of a
`DTSTAMP` is the sender's to choose and an unreadable one is a tie-break declined rather than an
accident. An organizer-authored message that supersedes nothing describes nothing, because
section 2.1.4 requires an update to increment `SEQUENCE`: two messages at one revision are one
version, and the second is not the one the organizer sent.

The third needed the state model to grow, which is why it is an amendment and not a fix. Two
replies from one attendee are one revision answered twice, and nothing on a component can order
them: its `DTSTAMP` is the organizer's, it is older than both, and advancing it on a reply would
refuse one attendee's answer because a *different* attendee answered later.
`ScheduledComponent::attendee_answered_at` is where that fact lives, the reply diff writes it
onto the line it answers for as `ical_itip::ANSWERED_AT`, and a state that records nothing
admits the second answer rather than refusing it — the direction that keeps a change of mind
working, stated in `SECURITY.md` as a defense a caller can discard by discarding the parameter.
No parameter RFC 5545 registers carries this, and an `x-param` on the line it is about is the
only conformant place for it.

**10. A gap is the caller's reading, and an identity that names no instant is not an ambiguous
one.** Amendment 3 gave instance identity a fold side and stopped there, at the half of the
question the octets can raise. The other half is a wall clock a zone sprang over, which names no
instant until `ical_tz::GapPolicy` says otherwise — and `resolve_instance` was reaching past
that policy to the raw resolution, so an identity in a gap came back identical under all three
readings, with an empty report, and the message was then refused `AmbiguousInstance`: a claim
that two meetings could not be told apart, about an hour in which the zone showed none.
`FoldSide::Nowhere` is that state, it compares `Different` rather than `Ambiguous` so the
refusal is `NoMatchingInstance`, and `scheduling-instance-nonexistent` says when a reading
dropped an identity. `FoldPolicy` stays deliberately unread here: a policy for *rendering* a
repeated hour must not decide which of two meetings a message is about.

**11. The work of judging a message is cardinality, and cardinality is charged at `read`.**
`ItipMessage` means "already checked and already charged" and `evaluate_message` takes no ledger
on the strength of it. The list that was never counted is the one a transition is described
over: a payload of a hundred thousand properties read for four units and cost a hundred thousand
allocations to judge, so a shared meter bounded how many messages an inbox read and not what
reading one cost. `Limits::max_payload_properties` bounds it and `read` charges it, beside the
attendee list it already charged.

**12. The field-permission table acquires a per-method dimension, and a payload stating a change
its author may not make is refused rather than dropped.** `field_rule` answers on a name alone,
which is a dozen lines standing in for RFC 5546's section 3 restriction tables, and M3 recorded
two failures with one cause: a legitimate `COUNTER` is refused, and an attendee's `REPLY`
carrying a moved `DTSTART` is *ignored* — the transition holds only the sender's own `ATTENDEE`
line, so the gate's guarantee holds, but a caller that applies `Authorization::message`'s payload
instead of the transition moves the meeting.

So `field_rule` takes the method as well as the name, and the per-method rows are read off the
tables already committed as data rather than transcribed a second time: for an attendee-authored
method, a name that method's own table prints is writable by an attendee-side actor under that
method only. In practice exactly one method moves. Under `COUNTER`, section 3.2.7's scheduling and
descriptive rows — `DTSTART`, `DTEND`, `DURATION`, `DUE`, `RRULE`, `RDATE`, `EXDATE`, `LOCATION`,
`SUMMARY`, `DESCRIPTION`, `PRIORITY` — read as either party's. `ORGANIZER` and `SEQUENCE` stay
organizer-only under every method, which is Amendment 7's finding and is not reopened, and
`ATTENDEE` stays the actor's own under every method. Under `REPLY` nothing moves: section 3.2.3's
table prints those rows only because a reply echoes them unchanged, and echoing is not writing.
Permitting them under `COUNTER` is not a hole, because a `COUNTER` travels attendee to organizer,
so the recipient evaluating it is the actor who may write those fields anyway; what it authorizes
is a *proposal* the organizer's client shows and chooses to accept, after which the organizer
sends the `REQUEST` that actually moves the meeting.

The silent drop becomes a denial through one gate placed before the payload is described, and
three properties of it are load-bearing. It is one-directional — a property the payload omits is
silence and never a removal, the same reasoning that makes `REFRESH` short-circuit in Amendment 6,
and without which every ordinary four-line `REPLY` would be refused for the `DTSTART` it does not
echo. It reads only lines whose octets differ from the held ones, so a legitimate echo is not a
change. And it skips every name whose rule is the actor's own line, because positional occurrence
identity is not line identity for those: a reply's `ATTENDEE` occurrence 0 is not the held copy's
occurrence 0, and only the address match inside the reply diff knows which line is meant. The
existing occurrence-shaped check is retained beside it, and that is a division of labor rather
than a backstop — it owns the question no per-method table can answer, which is whether this is
the actor's own line and whether the line the change leaves behind still names them. It has a
committed live path today, where an invited attendee replying on another attendee's behalf is
refused at the recipient's own numbering; a second path arrives with Amendment 13's agent-aware
lookup.

Three costs. The conservative default the design document explicitly preferred is given up for one
method, so a row transcribed wrong in section 3.2.7 is now a permission rather than a refusal —
the failure direction flips for exactly the table with the most rows — and a caller that
auto-applies an authorized `COUNTER` writes an attendee's proposed `DTSTART` into its own copy,
which M3 refused at the gate and which now becomes caller policy that nothing here can enforce.
The reply overreach gate converts M3's silence into refusal and inherits the octet diff's known
noise as the difference between an accepted and a rejected message: a client that echoes a
re-folded `DESCRIPTION` or a reordered parameter list on its `REPLY` is now refused outright,
which is a real interoperability regression across every reply, it will be reported, and the only
defense is that the comparison is one-directional and reads only differing octets. And one denial
now has three producers over three different comparisons, so a reader debugging a refusal has three
places to look, and drift between the last two — split along a boundary invisible in the error —
is caught only by tests.

Three alternatives rejected. Keeping the method-blind table and answering the `COUNTER` report with
documentation is the design document's own stated preference, and it loses because the roadmap now
records the refusal as observed rather than predicted, and a preference stated before the report is
not a decision taken after it. Widening the reply diff so the transition itself carries the reply's
`DTSTART` — one gate instead of two — loses because it fixes the refusal by reopening the write:
an *accepted* reply's transition would then carry every property the replying client echoed,
destroying the narrowing that preserves the recipient's own `X-` parameters and that section
3.2.3's "MUST NOT differ" makes meaningful. And the reading that the occurrence-shaped check has no
live path until delegation supplies one is rejected on committed evidence, since it is reached
through `evaluate_message` today.

**13. The delegation rules are specified, `SCHEDULE-AGENT` leaves this sentence entirely, and the
hold a delegate's reply lands in is named rather than silent.** The Consequences gesture at three
things at once, and reading the specifications apart shows they are not one subject.

`SCHEDULE-AGENT` is a category error and is struck rather than answered. RFC 6638 section 7.1 says
servers MUST NOT include the parameter in any scheduling message they send and clients MUST NOT
include it in any they send, so a parameter both sides are forbidden to put in a message can never
reach an iTIP message gate; section 1 of the same document excludes delegating from its scope
outright. That `ical-itip` names it nowhere is therefore correct rather than a hole. It governs a
stored scheduling object resource and belongs to `ical-dav`'s vocabulary — and because the crate
graph forbids `ical-dav` from naming this crate's types, no joint enforcement exists or is planned:
a caller wiring both owns that join, and this ADR says so rather than leaving a reader to wonder
where the term went.

`SENT-BY` is not vague, it is half-applied, and the gap is a live defect. An agent satisfies its
principal's sender rule, and then the field check computes the actor's own occurrence by matching
the address value only and never the `SENT-BY` parameter — so the occurrence is absent, every
change to the actor's own line fails, and an assistant's reply is refused for making the one change
a reply exists to make. It is untested because every `SENT-BY` in the corpus sits on `ORGANIZER`,
where that check short-circuits on role. The rule: the actor's own occurrence is found by an
agent-aware lookup, and if two `ATTENDEE` lines name the same agent the lookup answers nothing —
the same conservative direction the reader already takes for multi-valued parameters — so an agent
for one party can never reach another's line. The line the change leaves behind must still carry
the *principal's* address, so an agent may change its principal's participation status and may
never write its own address into a party line, and the actor role keeps the agent distinct, so
"the assistant sent this" still never reads as "the attendee is the assistant".

A delegate's `REPLY` is held, in one turn, and the hold is named. RFC 5546 section 5.2.2 offers two
acceptable resolutions and declines to choose; this project chooses hold, because accepting the
party crasher overturns this document's own opening position verbatim and section 3.2.2.6 makes
admission the organizer's decision rather than the library's, while composing the two turns from
the delegate's own `DELEGATED-FROM` would make an attendee-side actor author the addition of an
`ATTENDEE` line on the sender's own word. But the hold as shipped fails on its own terms twice, and
both are fixed here. It was silent — an authorized empty transition is indistinguishable from an
authorized no-op, the precise conflation this crate introduced a denial variant elsewhere to avoid
— so `TransitionReason` gains a held-for-delegation value, which is a description and not a
permission and which costs no major version. And it was permanent while the documents said
otherwise: the corpus fixture that *is* the post-delegator-reply state still describes nothing,
because the delegator's reply writes parameter edits and never adds the delegate's own `ATTENDEE`
line. Under the rules decided here the sole release is an organizer `REQUEST` naming the delegate,
and the amendment states that as the documented path so a caller is told what it is waiting for.

Two smaller rules close the same neighborhood. A forwarded delegation `REQUEST` is admitted only
where there is nothing to overwrite: against an absent prior state an attendee-side role satisfies
`REQUEST`, because nothing is held, the only reachable reason is creation, and Amendment 4 already
concedes that for a first message the gate proves only that the actor is a party the message names;
against a held copy it stays a sender refusal, which is the first attack `SECURITY.md` names and
which does not move. And a multi-valued `DELEGATED-TO` matches nobody, promoted from a code comment
to a rule: the reader does not split on comma, so a delegation to several calendar users — which
section 2.1.2 permits — leaves each delegate a stranger whose reply is refused as an unknown
attendee rather than held. That is a stated limitation with a fixture rather than a silent one.

Five things this makes worse. The caller wanting one turn still does not get one and now gets a
named reason for a wait this decision cannot end: if the organizer never sends a `REQUEST` naming
the delegate, the hold repeats forever, and the library reports the wait while supplying no queue,
no timeout and no implementation of section 5.2.2's third sentence. The `REQUEST` admission widens
a path — against an absent prior state a `REQUEST` is now accepted from any party the payload names
as an attendee, and the payload's own delegation parameters are written by the forwarder and
cross-check nothing. The agent rule lets an address that appears only inside a parameter change its
principal's participation status, which section 2.1.3 authorizes nowhere; it defines what `SENT-BY`
means and states no permission, so a producer emitting a forged one has its word honored, bounded
only by "one line, and the value stays the principal's". Striking `SCHEDULE-AGENT` leaves this
workspace with no answer for a server that must honor a client-managed scheduling object, and the
crate graph forbids ever enforcing it jointly with this gate. And the single-`ATTENDEE` rule stands
until measured, so a `REPLY` shaped like RFC 5546 section 4.2.6's own printed example is still
refused by an implementation that claims to transcribe that RFC.

That last one is the only sub-question reading cannot settle, because RFC 5546 contradicts itself —
section 3.2.3's table prints one `ATTENDEE`, section 4.2.5 item B says a second one MUST be
included, and sections 4.2.6 and 4.2.7 print two while 4.2.5's own example prints one — so it gets a
measurement and a threshold fixed now, before any capture. **The measurement.** For each of Google
Calendar, Microsoft 365, Apple Calendar and Thunderbird, capture two iMIP payloads from a real
account: the `REPLY` the client generates when an attendee delegates, and the `REPLY` the delegate's
client generates when the delegate accepts. For each captured component record the count of
`ATTENDEE` properties, the exact spelling and quoting of `DELEGATED-TO` and `DELEGATED-FROM` and
whether each value is single or comma-joined, the participation status on each line, and whether
`SEQUENCE` was incremented, with producer and `PRODID` recorded beside each capture. **The source.**
Live iMIP messages from accounts on each of the four services, exported and committed under ADR
0006's provenance rules; it needs a real account per service and a second account to delegate to,
and no CalDAV server, no time zone data and no network service of this project's own. **The
threshold.** If one or more of the four producers emits either capture with two or more `ATTENDEE`
properties in a single component, the single-`ATTENDEE` row is an observed break and the
delegator-authored addition is adopted: the `REPLY` row admits a second `ATTENDEE` only when the two
lines form a delegation pair, and the field check gains exactly one exception — an attendee-side
actor at its own occurrence may author the addition of one `ATTENDEE` line whose address equals the
`DELEGATED-TO` value on that same actor's line. Every other reply keeps a count of one. If zero of
the four emits a second line in either capture, the row stands unamended, the composing options are
closed, and the organizer `REQUEST` is recorded as the sole release. The count is per producer, not
per capture, and a capture that cannot be obtained counts as zero for that producer and is recorded
as not-obtained, never as a negative. **If it cannot be obtained at all**, the row stands by default
— refusal rather than permission, the direction M3 already chose — and this amendment records that
it was never tested against a real producer, so a later interoperability report about a refused
delegation reply reopens this rather than being treated as a new discovery. The default may not be
flipped by argument, only by a capture meeting the threshold above.

The strongest rejected alternative is composing the two turns in the gate, restricted to the state
where the held copy already carries the delegator's line with a delegated status and a
`DELEGATED-TO` naming the sender. In that state the addition is authorized by the organizer's own
held copy rather than by the sender's word, which is a genuinely strong argument and must not be
rediscovered from scratch. Two things beat it: it needs an attendee-side actor to author the
addition of an `ATTENDEE` line, which is verbatim the write this document forbids and which the two
field checks were built to stop; and decisively, it is the wrong author, since section 2.1.2 says
the *delegating* attendee informs the organizer. If the measurement shows producers emit the
two-`ATTENDEE` reply, the delegator-authored addition supersedes this alternative and is the better
version of it.

**14. The charset is a fact about the envelope, so it is judged at the door and never in the diff.**
The Consequences name a hole with the failure direction this crate does not accept — permission —
and the diff is the wrong place to close it. `ical-itip` does not become charset-aware: the octet
comparison is unchanged, it gains no cannot-compare outcome, and Amendments 6 and 7, which reason
from octet equality, are undisturbed. The hole is closed one layer up, at the iMIP door, where a
charset is actually stated.

`MediaTypeParams` gains one predicate over the part's body octets, cited to RFC 6047 section 2.4
throughout and to RFC 5545 section 3.1.4 nowhere. A stated `utf-8` is accepted unconditionally. A
stated `us-ascii` is accepted only when every octet is below `0x80`, since a high octet makes the
declaration a statement the body contradicts. An *absent* charset is accepted only when every octet
is below `0x80`, because the part is a `text/*` entity whose RFC 2046 default is US-ASCII and
section 2.4 requires the parameter to be present and to read UTF-8 whenever the object carries
characters US-ASCII cannot represent — so absent-plus-high-octet is itself a section 2.4 violation
and lands on the same answer a declared `windows-1252` gets, by a stronger argument. Everything
else is refused, with no alias table and no unsupported-encoding diagnostic: the door refuses.
Well-formedness under a UTF-8 declaration is deliberately not checked here, because section 2.4
governs the parameter rather than the encoding's validity, and a validity sweep is
[ADR 0001](0001-lossless-round-trip.md) Amendment 9's subject. The door stays three separate
questions composed by the caller — the media type, the method agreement, the charset — shipped as a
documented recipe and a fixture rather than as one call, because this module's whole framing is
that the envelope and the object are two statements from two parties.

What that buys the diff, stated as the scope of the guarantee rather than left to be assumed: every
payload reaching the describe step *through iMIP* arrived as octets declared and consistent with
UTF-8 or US-ASCII, so exactly one charset is in force and octet equality coincides with text
equality for anything that decodes. The construction this document names has no entry route through
iMIP. It keeps every other route — a caller feeding this crate from CalDAV, from a file or from its
own store holds no envelope and gets none of this.

Six costs, and two of them are sharp. The predicate can no longer be answered from a header alone,
which dents the module's dual of envelope statements on one side and object statements on the
other; the only mitigation is that the method-agreement question already had that shape, which is
consistency rather than a defense. And it depends on the caller having performed
Content-Transfer-Encoding decoding and cannot verify that it happened: a caller passing the base64
text of the part sees nothing but ASCII and gets acceptance for an absent charset over a body full
of high octets. This amendment creates that hole and can only document it, so it belongs in
`SECURITY.md` as a caller obligation and not only in a doc comment. Beyond those: the cost goes from
constant to linear in the part's length on a call that takes no meter and charges nothing, which is
tolerated because the octets are ones the caller already holds and paid to receive, and which a
later hostile-input review is entitled to reopen; refusing a declared `windows-1252` and an absent
charset over high octets drops real mail from shipping clients, which is lost interoperability
bought with safety rather than safety for free; the guarantee is narrower than the sentence it
answers, so the Consequences above are narrowed rather than struck and the roadmap keeps the hole
listed with the iMIP path carved out; and the predicate's name under-describes what it answers,
since it is also a section 2.4 conformance check about whether the declaration and the octets can
both be true.

The alternative rejected is strictly wider and is not weak: compare decoded text in the diff and
refuse a comparison that cannot be decoded, which would close the hole for CalDAV inputs, file
inputs and any caller-supplied component, and would need no cooperation about transfer decoding. It
loses on two grounds. It puts the decode in the one place with no charset to decode against, so an
encoding failure would surface as a scheduling refusal — right direction, wrong reason, at the depth
where a caller can least diagnose it. And it forces lazy decoding to become eager at the gate, which
is ADR 0001's question, and answering that as a side effect of this one is the wrong door. If an
eager sweep is adopted there, this door stays anyway: it refuses before anything is parsed, the
sweep refuses after, and the cheaper refusal is worth its own line. Also rejected, so it is not
proposed as a simplification: answering false for an absent charset unconditionally, which would
keep the predicate header-only. RFC 2046 makes an absent charset over an all-ASCII body fully
conformant and RFC 6047's own examples run that way, so it would drop conforming mail for no gain.
