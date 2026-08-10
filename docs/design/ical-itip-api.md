# `ical-itip` public API

- Status: accepted
- Date: 2026-08-10
- Implements: [ADR 0005](../adr/0005-scheduling-apart-from-the-model.md), with
  [0007](../adr/0007-allocation-policy.md), [0009](../adr/0009-error-and-diagnostic-model.md),
  [0010](../adr/0010-shared-resource-limits.md)
- Amended: 2026-08-11 (M3 shipped)
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" and "What M3 shipped" below

## Responsibility

`ical-itip` answers one question — *what would this scheduling message change, and is the party
applying it entitled to make that change* — and answers it as a value. It takes an RFC 5546
message, the component the caller currently holds, and the identity of the actor, and returns
either a described transition or the reason the message was refused. It mutates nothing, opens
nothing, reads no clock, and holds no session: the propose step and the confirm step are two
independent calls over state the caller supplies each time. Authorization is not a layer above
the semantics but the first half of them, because the positions where scheduling implementations
have historically been exploited — a reply that moves a meeting, a reply from an address nobody
invited, a stale `SEQUENCE` overwriting a newer one — are all positions where the message and the
identity have to be judged together or not at all.

## The surface

Every signature below is compiled: the skeleton beside this document passes `cargo clippy`
with the workspace lint table and `-D warnings`, `cargo fmt --check`, and `cargo doc` under
`RUSTDOCFLAGS=-D warnings`, in all four combinations of its two features.

### Identity

```rust
pub struct PartyId<'a>(&'a str);

impl<'a> PartyId<'a> {
    pub const fn new(address: &'a str) -> Self;
    pub fn from_bytes(address: &'a [u8]) -> Option<Self>;
    pub const fn as_str(self) -> &'a str;
    pub fn matches(self, other: PartyId<'_>) -> bool;
}

pub struct Party<'a> { /* raw, address, sent_by */ }
pub struct Attendee<'a> { /* party, part_stat, part_stat_text, role, delegated_from/to */ }
pub enum PartStat { NeedsAction, Accepted, Declined, Tentative, Delegated, Other }
pub enum Role { Chair, RequiredParticipant, OptionalParticipant, NonParticipant, Other }
```

**Invariants.** A `PartyId` holds valid UTF-8; a CAL-ADDRESS that does not decode is not a
`PartyId` and therefore matches nobody, which is the conservative direction — the alternative is
an address that compares equal to something it is not. `matches` folds the `mailto:` scheme and
the domain case-insensitively and compares the local part exactly, per RFC 5321 §2.4; the blanket
ASCII fold every naive implementation reaches for would silently merge `J.Doe@example.com` with
`j.doe@example.com`, which is the receiving host's decision and not ours. `Party::is_agent_of`
answers `SENT-BY` separately from identity, so "the assistant sent this" never becomes "the
organizer sent this". `Attendee::part_stat_text` keeps a `PARTSTAT` value we do not interpret
reachable, so `PartStat::Other` loses nothing.

### The message

```rust
pub trait ScheduledComponent: core::fmt::Debug {
    fn component_kind(&self) -> Option<ComponentKind>;
    fn method(&self) -> Option<&[u8]>;
    fn uid(&self) -> Option<&[u8]>;
    fn sequence(&self) -> SequenceRead;
    fn dtstamp(&self) -> Option<Instant>;
    fn recurrence_id(&self) -> Option<InstanceRef>;
    fn organizer(&self) -> Option<Party<'_>>;
    fn attendee_count(&self) -> usize;
    fn attendee(&self, index: usize) -> Option<Attendee<'_>>;
    fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence>;
    fn property_count(&self) -> usize;
    fn property_name(&self, index: usize) -> Option<&[u8]>;
    fn property_line(&self, index: usize) -> Option<&[u8]>;
    fn child_count(&self) -> usize;
    fn child(&self, index: usize) -> Option<&dyn ScheduledComponent>;
}

pub struct ItipMessage<'a> { /* calendar, method, first_payload, payload_count */ }

impl<'a> ItipMessage<'a> {
    pub fn read(
        calendar: &'a dyn ScheduledComponent,
        limits: &Limits,
        meter: &mut Meter,
        sink: &mut dyn DiagnosticSink,
    ) -> Result<Self, MessageError>;

    pub const fn method(&self) -> Method;
    pub const fn calendar(&self) -> &'a dyn ScheduledComponent;
    pub const fn payload_count(&self) -> usize;
    pub fn payload(&self, index: usize) -> Option<&'a dyn ScheduledComponent>;
    pub fn payload_for(&self, current: &dyn ScheduledComponent)
        -> Option<&'a dyn ScheduledComponent>;
    pub fn uid(&self) -> Option<&'a [u8]>;
}
```

**Invariants.** `ItipMessage` is the type that means *already checked and already charged*: the
`METHOD` is one RFC 5546 defines, at least one scheduling payload exists, every payload shares
one `UID`, every attendee list is within `Limits`, and the work of establishing that was debited
from the caller's `Meter`. Nothing downstream re-checks any of it, which is why `read` is the
only constructor. This is also where ADR-0010's `&Limits, &mut Meter` pair enters:
`evaluate_message` keeps the three parameters ADR-0005 gives it, because by the time a message
exists its cardinality is already bounded.

`ScheduledComponent` is how ADR-0005's `current: &Component` is spelled. `ical-itip` bridges an
`ical_core::Component` onto it with `ScheduledView::of(&component)`, which is a value rather
than an impl on `Component` itself; "What M3 shipped" gives the two reasons that is forced
and what it costs. Naming the trait buys two things: a CalDAV
server whose current state is a database row never has to build a `Component` to answer "who may
change this", and the crate's demand on `ical-core` becomes one readable list instead of an
inference from call sites. The trait is deliberately object-safe — index accessors rather than
iterators, no generics, no associated types — matching the posture `ical-tz`'s `ZoneSource`
already takes, and costing one vtable rather than a monomorphized copy per state carrier on a
`thumbv7em` target.

### Limits travel on the error channel

```rust
pub enum MessageError {
    MissingMethod, UnknownMethod, NoPayload, MissingUid, MixedUids,
    UnsupportedPayload(ComponentKind), TooManyAttendees, TooManyComponents, BudgetExhausted,
}
```

ADR-0009 routes a limit breach on an otherwise parseable value to the diagnostic channel, and
this crate does the opposite for every one of them, deliberately. A truncated attendee list is
not a degraded answer, it is a *different* authorization answer: dropping the 513th attendee
turns "this party may reply" into "this party is unknown", and an attacker who can pad a list
past the threshold picks which of those two the server believes. Truncate-and-flag is a safe
policy for a 40 KB `DESCRIPTION` and an unsafe one for anything the authorization decision reads.

### The transition

```rust
pub struct Transition {
    changes: BTreeMap<PropertyOccurrence, ProposedChange>,
    reason: TransitionReason,
}

impl Transition {
    pub const fn reason(&self) -> TransitionReason;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn change(&self, at: &PropertyOccurrence) -> Option<&ProposedChange>;
    // Iterator<Item = (&PropertyOccurrence, &ProposedChange)>
    pub fn changes(&self) -> Changes<'_>;
}

pub enum TransitionReason {
    Published, Created, Updated, Rescheduled, InstancesAdded,
    ParticipationChanged, Cancelled, RefreshRequested, CounterProposed, CounterDeclined,
}

pub fn describe_message(message: &ItipMessage<'_>, current: &dyn ScheduledComponent) -> Transition;
```

**Invariants.** `Transition` reuses `ical-core`'s `ProposedChange` rather than inventing a second
change vocabulary, keyed by `PropertyOccurrence` so two conflicting changes to one property
occurrence cannot both exist. It is inert: no method on it reaches a component, and the only
route to `apply_transition` is through `evaluate_message`. `describe_message` therefore hands a caller
what a *denied* message tried to do without handing it the ability to do it — which is ADR-0005's
recommendation that a rejected reply stay inspectable, discharged without giving
`AuthorizationDenied` an allocated field on every rejection path.

The diff compares the preserved bytes of a whole property occurrence, name and parameters and
value together, and expresses a difference as `ProposedChange::Replace`. ADR-0005 leaves
octet-versus-typed comparison open and warns about the hole under it; this is the octet answer,
chosen because its failure direction is safe. Byte-identical means untouched, so no organizer-only
field an attendee edited can report "unchanged"; the cost lands on the other side, where a
re-fold or a parameter reorder reports a change a semantic diff would not. A `REPLY` is the
exception: it is matched to the local attendee list by CAL-ADDRESS and expressed as
`ProposedChange::SetParameters`, so the recipient's own `X-` parameters on that `ATTENDEE` line
survive an answer to it.

### Authorization

```rust
pub struct AuthorizedTransition(Transition);   // no pub field, no From, no Default, no Clone

impl AuthorizedTransition {
    pub const fn transition(&self) -> &Transition;
    pub const fn reason(&self) -> TransitionReason;
    pub fn into_transition(self) -> Transition;
}

pub enum AuthorizationDenied {
    UnknownAttendee,
    OrganizerMismatch,
    SequenceStale { have: u32 },
    DtstampStale { have: Instant },
    UidMismatch,
    MethodForbidsField(PropertyId),
    NoMatchingInstance,
}

pub enum ActorRole { Organizer, OrganizerAgent, Attendee, AttendeeAgent, Delegate }
pub enum FieldRule { OrganizerOnly, AttendeeOwn, EitherParty }

pub fn actor_role(current: &dyn ScheduledComponent, actor: PartyId<'_>) -> Option<ActorRole>;
pub fn attendee_index(current: &dyn ScheduledComponent, who: PartyId<'_>) -> Option<usize>;
pub fn field_rule(name: &[u8]) -> FieldRule;
pub fn is_time_property(name: &[u8]) -> bool;

pub fn evaluate_message(
    message: &ItipMessage<'_>,
    current: &dyn ScheduledComponent,
    actor: PartyId<'_>,
) -> Result<AuthorizedTransition, AuthorizationDenied>;
```

**Invariants.** `AuthorizedTransition` is sealed and is `ical-itip`'s alone — not a generic
`Authorized<T>` shared with `ical-core`. A wrapper parametrized only on what changed would prove
that *some* sealed constructor ran somewhere, not that RFC 5546's attendee-list, field-permission
and `SEQUENCE` checks ran for *this* value, and since Rust has no crate-family privacy a shared
wrapper would need a public constructor and would not be sealed at all. It has no serialized form
either: a wire encoding would be forgeable, and the sealed constructor would then attest to
nothing but the transport. `apply_transition` takes it **by value**, so a vetted transition is a
single-use capability rather than something a caller can replay against a second target after the
state it was vetted against has moved.

The gate runs in a fixed order — identity, then revision, then fields — so a denial names the
first reason a caller can act on. There is no partial success: a message that overreaches on one
property is denied whole, because applying its permitted half would leave the caller holding a
component no party ever described. `field_rule`'s default is `OrganizerOnly`, including for `X-`
properties, because an unrecognized property arriving in a `REPLY` is exactly the shape of an
attendee smuggling state into an organizer's copy, and a permissive default there is a hole that
no test written against the properties we know will ever find.

### Application

```rust
pub trait ScheduleTarget: core::fmt::Debug {
    fn write_change(&mut self, at: &PropertyOccurrence, change: &ProposedChange)
        -> Result<(), WriteRejected>;
}

pub enum WriteRejected { UnknownProperty, ValueTypeMismatch, ReadOnly }
pub struct RejectedChange { /* at, reason */ }
pub struct ApplyReport { /* applied: u32, rejected: Vec<RejectedChange> */ }

pub fn apply_transition(
    target: &mut dyn ScheduleTarget,
    authorized: Authorization<'_>,
) -> ApplyReport;
```

**Invariants.** `ical-itip` ships `impl ScheduleTarget for ical_core::Component`, routing each
change through `Component::apply_to_occurrence` — the occurrence-addressed door ADR-0001
amendment 5 adds, not ADR-0001's scoped `PropertyMut` guard, which addresses an identity and
would answer for every `ATTENDEE` at once. `ComponentTarget` is the same door carrying the
caller's `Limits`; see "What M3 shipped" for why there are two. A server whose storage
is a row implements the trait against its rows instead. A partial application is reported, never
hidden: this crate owns no transaction and cannot roll one back, so a caller that needs
all-or-nothing checks `ApplyReport::is_complete` before committing its own storage.

### What this needs from `ical-core`

Three things, and they are the whole coupling DP-13 bought:

1. `PropertyId: Ord + Clone + Debug`, identifying one property *occurrence* — upper-cased name
   plus zero-based index among properties of that name — so that the same key addresses the same
   property in the incoming message and in the local copy, and `fn name(&self) -> &[u8]`.
2. `ProposedChange` constructible as: replace a whole occurrence, add an occurrence, remove an
   occurrence, and **set a list of parameters on one occurrence**. The list is not decoration:
   RFC 5546 §2.1.2 delegation writes `PARTSTAT=DELEGATED` and `DELEGATED-TO` on the same
   `ATTENDEE` line, so a single-parameter edit type cannot express a legal reply.
3. `Limits` must not be `Copy`. At the repository's `trivial-copy-size-limit = 128`, a small
   `Copy` policy value trips `clippy::trivially_copy_pass_by_ref` on every `&Limits` parameter
   ADR-0010 mandates, and CONTRIBUTING.md forbids the `#[allow]` that would buy it back. This was
   found by compiling, not by reading.

## Type to specification map

| Item | Serves |
| --- | --- |
| `Method`, `Method::is_organizer_authored` | RFC 5546 §1.4 |
| `ComponentKind`, `MessageError::UnsupportedPayload` | RFC 5546 §3.2–§3.5 |
| `ItipMessage::read`, `MessageError::MixedUids` | RFC 5546 §3.1.1 |
| `PartyId`, `PartyId::matches` | RFC 5545 §3.3.3; RFC 5321 §2.4 |
| `Party::sent_by`, agent roles | RFC 5545 §3.2.18; RFC 5546 §2.1.3 |
| `ActorRole::{Organizer, Attendee}` | RFC 5546 §1.3 |
| `Attendee`, `Role` | RFC 5545 §3.8.4.1, §3.2.16; RFC 5546 §3.7.2 |
| `PartStat`, `Attendee::part_stat_text` | RFC 5545 §3.2.12 |
| `Attendee::delegated_from/to`, `ActorRole::Delegate` | RFC 5545 §3.2.4/§3.2.5; §2.1.2 |
| `InstanceRef`, `payload_for` | RFC 5545 §3.8.4.4, §3.2.13; RFC 5546 §3.7.1 |
| `AuthorizationDenied::SequenceStale` | RFC 5546 §2.1.4 |
| `AuthorizationDenied::DtstampStale` | RFC 5546 §2.1.5 |
| `AuthorizationDenied::OrganizerMismatch` | RFC 5546 §2.1, §1.3 |
| `FieldRule`, `field_rule`, `MethodForbidsField` | RFC 5546 §3.2.2, §3.2.3 |
| `FieldRule::EitherParty` (`REQUEST-STATUS`) | RFC 5546 §3.6 |
| `TransitionReason::Published` | RFC 5546 §3.2.1 |
| `TransitionReason::{Created, Updated, Rescheduled}` | RFC 5546 §3.2.2 |
| `TransitionReason::ParticipationChanged` | RFC 5546 §3.2.3 |
| `TransitionReason::InstancesAdded` | RFC 5546 §3.2.4 |
| `TransitionReason::Cancelled` | RFC 5546 §3.2.5 |
| `TransitionReason::RefreshRequested` | RFC 5546 §3.2.6 |
| `TransitionReason::{CounterProposed, CounterDeclined}` | RFC 5546 §3.2.7, §3.2.8 |
| `freebusy::requested_window` | RFC 5546 §3.3 |
| `imip::MediaTypeParams` | RFC 6047 §2.4 |
| `imip::sender_is_named` | RFC 6047 §2.5 |
| `Transition`, `AuthorizedTransition`, `ScheduleTarget` | no RFC; ADR-0005 and DP-07 |

## Deliberately rejected

**A mutating `apply` that takes the message.** The whole point of ADR-0005 is that a mail client
needs to render "this meeting was moved — accept?" before anything touches the user's calendar.
Cost: two calls where one would do, and the caller carries the state between them.

**A generic `Authorized<T>` shared with `ical-core`.** It type-checks a confused deputy: a value
minted by a self-authorized local edit would satisfy a surface expecting an iTIP-vetted one,
because the parameter names what changed and not which gate ran.

**A serialized `AuthorizedTransition`.** Under ADR-0004 there is no session, so a
propose-then-confirm exchange crosses a request boundary; anything encodable there is forgeable
there. The caller carries the *message* across and evaluates again. Cost: the second evaluation
must read fresh state, and nothing in the type system forces it to — see below.

**`Vec<ProposedChange>`.** A list admits two changes to one property and leaves the resolution to
whoever iterates it last.

**A blanket ASCII case-fold on CAL-ADDRESS.** Rejected under RFC 5321 §2.4; see `PartyId::matches`.

**Truncating oversized lists to a diagnostic.** Covered above: it lets an attacker choose the
authorization answer.

**Generating outbound messages.** Building the `REPLY` an attendee should send is not here. It is
a real need and a named follow-up — the natural shape is a `Transition` against the actor's own
copy, which the caller then serializes through `ical-core` — but shipping it now would mean this
crate acquiring an opinion about `DTSTAMP`, and it owns no clock.

**A `serde` feature.** The five core crates may declare zero dependencies of any kind. There is
no `alloc` feature either: ADR-0007 makes `alloc` mandatory, not optional, and a feature that
could turn it off would be a second, untested crate.

## Feature flags

Neither feature is on by default, so `--no-default-features` and the default set are the same
build.

`imip` adds the `imip` module: `MediaTypeParams::read` over a `Content-Type` header value,
`agrees_with` comparing the envelope's `method` parameter against the body's `METHOD`, and
`sender_is_named` answering whether a `From` address appears on the component at all. It adds no
dependency and changes no evaluation result; without it, those checks are the caller's to write.

`freebusy` accepts `VFREEBUSY` payloads (RFC 5546 §3.3) and adds `freebusy::requested_window`.
Without it a `VFREEBUSY` payload is `MessageError::UnsupportedPayload(ComponentKind::FreeBusy)`
at `read` — refused, never silently ignored, because a scheduling message a build cannot reason
about is not a message it may accept.

All four combinations are compiled by the feature-matrix gate; `--no-default-features` is the
same as the default set, since no feature is on by default.

## Using it

A mail client, showing the change before applying it. The `Prompt` value is the caller's, not
this crate's:

```rust
fn on_incoming_message(
    calendar: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
    me: PartyId<'_>,
    meter: &mut Meter,
) -> Result<Prompt, MessageError> {
    let limits = Limits::CONSERVATIVE;
    let mut diagnostics = DiscardDiagnostics;
    let message = ItipMessage::read(calendar, &limits, meter, &mut diagnostics)?;

    match evaluate_message(&message, current, me) {
        Ok(authorized) if authorized.transition().is_empty() => Ok(Prompt::NoChange),
        Ok(authorized) => Ok(Prompt::Confirm(authorized.reason())),
        Err(denied) => Ok(Prompt::Refused(denied)),
    }
}

fn confirm(
    calendar: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
    me: PartyId<'_>,
    target: &mut dyn ScheduleTarget,
    meter: &mut Meter,
) -> Result<ApplyReport, MessageError> {
    let limits = Limits::CONSERVATIVE;
    let mut diagnostics = DiscardDiagnostics;
    let message = ItipMessage::read(calendar, &limits, meter, &mut diagnostics)?;
    match evaluate_message(&message, current, me) {
        Ok(authorized) => Ok(apply_transition(target, authorized)),
        Err(_) => Ok(ApplyReport::default()),
    }
}
```

The confirm turn re-reads and re-evaluates. That is the whole defense against a forged
authorization crossing the request boundary, and note what it does not defend: `current` here
must be freshly read, and no type forces that.

A server refusing an attendee that overreaches, and still able to say what the reply tried to do:

```rust
struct Review { denial: AuthorizationDenied, attempted: Transition }

fn review_reply(
    message: &ItipMessage<'_>,
    current: &dyn ScheduledComponent,
    sender: PartyId<'_>,
) -> Result<(), Review> {
    if message.method() != Method::Reply {
        return Ok(());
    }
    match evaluate_message(message, current, sender) {
        Ok(_) => Ok(()),
        Err(denial) => Err(Review { denial, attempted: describe_message(message, current) }),
    }
}

fn describe_denial(review: &Review) -> (&'static str, usize) {
    let label = match review.denial {
        AuthorizationDenied::UnknownAttendee => "sender is not on the attendee list",
        AuthorizationDenied::MethodForbidsField(_) => "sender may not change that property",
        AuthorizationDenied::SequenceStale { .. } => "reply answers an older invitation",
        _ => "reply rejected",
    };
    (label, review.attempted.len())
}
```

`attempted` is a `Transition`, so it can be rendered and cannot be applied. The `_` arm is
required and will stay required: `AuthorizationDenied` is `#[non_exhaustive]`.

One meter across a whole inbox, which is the amplification ADR-0010 exists to bound — 5,000
individually bounded messages are bounded in aggregate only if they share a ledger:

```rust
fn process_inbox(
    inbox: &[&dyn ScheduledComponent],
    current: &dyn ScheduledComponent,
    me: PartyId<'_>,
) -> (u32, bool) {
    let limits = Limits::CONSERVATIVE;
    let mut meter = Meter::with_budget(1_000_000);
    let mut diagnostics = DiscardDiagnostics;
    let mut accepted = 0u32;

    for calendar in inbox {
        let message = match ItipMessage::read(*calendar, &limits, &mut meter, &mut diagnostics) {
            Ok(message) => message,
            Err(MessageError::BudgetExhausted) => return (accepted, false),
            Err(_) => continue,
        };
        if evaluate_message(&message, current, me).is_ok() {
            accepted = accepted.saturating_add(1);
        }
    }
    (accepted, true)
}
```

Moving `Meter::with_budget` inside the loop reproduces the attack exactly, and no gate here sees
that caller's code.

And iMIP, where the envelope makes a claim that has to be checked against the body rather than
trusted (feature `imip`):

```rust
fn from_mail(
    content_type: &[u8],
    calendar: &dyn ScheduledComponent,
    envelope_sender: PartyId<'_>,
    current: &dyn ScheduledComponent,
    meter: &mut Meter,
) -> Result<Prompt, MessageError> {
    use ical_itip::imip::{MediaTypeParams, sender_is_named};

    let limits = Limits::CONSERVATIVE;
    let mut diagnostics = DiscardDiagnostics;
    let message = ItipMessage::read(calendar, &limits, meter, &mut diagnostics)?;

    // The header is bounded and charged like anything else off the wire, and an unclosed
    // quoted value is a refusal rather than a truncation: see "What M3 shipped".
    // Both envelope answers are the *caller's* refusals and not `AuthorizationDenied`: the
    // gate below never ran, and reporting one as though it had would tell a user that a
    // message was refused on scheduling grounds when it was refused on postal ones.
    let Ok(declared) = MediaTypeParams::read(content_type, limits, meter) else {
        return Ok(Prompt::EnvelopeUnreadable);
    };
    if !declared.is_calendar() || !declared.agrees_with(&message) {
        return Ok(Prompt::EnvelopeDisagrees);
    }
    if !sender_is_named(envelope_sender, current) {
        return Ok(Prompt::EnvelopeNamesNobody);
    }
    match evaluate_message(&message, current, envelope_sender) {
        Ok(authorized) => Ok(Prompt::Confirm(authorized.reason())),
        Err(denied) => Ok(Prompt::Refused(denied)),
    }
}
```

## What this makes worse

The `ScheduledComponent` trait is sixteen methods, and something has to implement all of them
before this crate does anything at all. That is the price of not requiring a `Component` to
exist, and M3 paid it twice over: `ScheduledView` bridges an `ical_core::Component` and the
conformance corpus wrote a second implementation over its own `.ics` reader, so the trait is
demonstrated demand rather than insurance. The bill arrived elsewhere. Because the bridge is a
value that owns the reconstructed content lines and the RFC 6868-resolved parameter values, a
caller holding a `Component` pays one build pass over it before the first question is asked,
where an impl on `Component` would have paid nothing — and could not have existed.

Reusing `ical-core`'s change vocabulary couples the crates where ADR-0005's own text wanted them
clear: every newly mutable property `ical-core` learns must now also decide whether it is
iTIP-relevant, and this document adds a second obligation on top — the parameter-list form of
`ProposedChange`, which `ical-core` needs for nobody but us.

Re-evaluating at the confirm turn closes forgery and not staleness. Nothing forces the second
call to read fresh state rather than replay a snapshot, and a genuine `AuthorizedTransition` over
a stale snapshot is still wrong. Binding a transition to an `ETag` is ADR-0004 territory and
undesigned, so the propose-and-confirm flow is not safe against a racing organizer update and
must not be described as if it were.

The octet-level diff is a decision this document makes that ADR-0005 left open, and it makes
`Replace` noisier than it should be: a message that only refolds a long `DESCRIPTION` reports a
change. The failure direction is the safe one, but a caller diffing for display will see it.

`RANGE=THISANDFUTURE` is *represented* here and not *implemented*: `InstanceRef` carries the
range, `payload_for` matches one instance, and nothing splits a series or composes anchors the
way ADR-0002 requires of `ical-recur`. VALUE=DATE-safe `DTSTART`, negative `BYSETPOS`, and a fold
across a codepoint are likewise unexercised through these types. `ical-itip` is not
RFC-5546-complete, and this API does not entitle anyone to say it is.

Finally: the field-permission table is a dozen lines of `field_rule` standing in for RFC 5546's
per-method restriction tables, which run to pages. The conservative default keeps the gaps
closed rather than open, which means the first real interoperability report is likely to be
"this legitimate `COUNTER` was refused", not a security hole — a failure mode we prefer, but a
failure mode.

## What the first compile changed

The stand-in modules in the skeleton were deleted and replaced with the real crates, which
surfaced one genuine design collision and three spellings.

`PropertyId` was the collision. This crate's version identified a property *occurrence* — a name
plus a zero-based index among the properties sharing it — because a transition has to say which
`ATTENDEE` changed. `ical-core`'s version identifies a name, and is what `properties_named` and
`get` look up with, so two `ATTENDEE` properties deliberately share one. Widening the shared type
would have narrowed every lookup below this crate, so the occurrence index stays here as
`PropertyOccurrence`, wrapping an `ical_core::PropertyId` and the index. Everything this document
says about the change map is unchanged; the key type has a different name and one more field.

`Instant` comes from `ical-core`, not from `ical-tz`. This crate compares two of them and resolves
nothing, so it does not depend on `ical-tz` for that — which also means a caller with a floating
time resolves it before handing the message over, exactly as before.

`SinkStatus` is `SinkOutcome`, the workspace's one name for the answer
[ADR 0009](../adr/0009-error-and-diagnostic-model.md) requires. `Meter::charge(u32)` is
`Meter::try_charge(u64)`, the shared ledger's own primitive. `Limits::CONSERVATIVE` is
`Limits::DEFAULT`: `ical-core`'s default policy already is the conservative one, and two names for
one value is how a caller learns to distrust both.

## What M3 shipped

The crate behind the surface above was built in M3 and is no longer a proposal. Its eight units
were written against this document and compiled together for the first time afterwards. Nothing
in the responsibility or the threat model moved. Five spellings did, and each is a place where
this document promised something the frozen signatures could not deliver.

**`ScheduledComponent` is bridged by a value, not by an impl on `Component`.** This document
said `ical-itip` ships `impl ScheduledComponent for ical_core::Component`. It cannot, for two
independent reasons and neither is stylistic. `property_line` hands back a whole content line as
`&[u8]`, and a `Component` stores the name, the ordered parameter list, the value and the
recorded folds separately — the line as one contiguous run of octets exists nowhere in the tree,
and a borrow can only point at octets something already owns. Second, ADR-0001 amendment 3
requires every parameter value handed to `Party` or `Attendee` to be RFC 6868-decoded first, and
resolving `^'` into `"` *produces an octet the file does not contain*, while `Party<'a>` and
`Attendee<'a>` are `Copy` over `&'a [u8]` and cannot hold a `Cow`. Both derivations need storage
with the `&self` lifetime, and a `Component` has none. So the bridge is
`ScheduledView::of(&component)`, which borrows the component and owns exactly the two things the
component does not store. The alternative cost three frozen files — `property_line` returning a
`Cow`, the diff's map, and `Party`/`Attendee` losing `Copy` — to buy one line at one call site.

**`ScheduleTarget` has two doors, because a policy is not part of a transition.**
`write_change` takes no `Limits` and `Component::apply_to_occurrence` needs one, since a
replacement is octets off the wire read through the same content-line reader a file goes
through. `impl ScheduleTarget for ical_core::Component` therefore writes under `Limits::DEFAULT`,
which is safe for the ordinary caller because those octets came out of an `ItipMessage` already
read under that caller's own bounds. `Transition::new` and `Transition::record` are public, so a
hand-built transition's octets have cleared nothing — and such a caller uses `ComponentTarget`,
which carries the caller's `Limits`. Both route through one private writer so the two cannot come
to disagree about which occurrence a change addresses.

**`current` and the target cannot be the same value, and the type system says so.**
`Authorization<'a>` holds `&'a dyn ScheduledComponent`, so the immutable borrow of the state a
decision was made against overlaps the `&mut dyn ScheduleTarget` that `apply_transition` writes
to. A caller applies into its own storage, or into a separate value. The propose/confirm example
above takes `current` and `target` as separate parameters and so happens to compile; this
paragraph is why it must.

**`MediaTypeParams::read` is bounded, charged and fallible.** The sketch spelled
`read(content_type)`. ADR-0010 requires the header to be held under `Limits::max_header_bytes`
and charged to the meter, and an unclosed quoted value has to be a refusal rather than a
truncation — truncating lets an attacker choose where a value ends and therefore which method
the envelope appears to declare. Those cannot hold in an infallible one-argument constructor, so
the signature is `read(header, limits, meter) -> Result<Self, MediaTypeError>`, matching
`ItipMessage::read`'s own shape. `agrees_with` covers the `method` parameter only; RFC 6047
section 2.4 also requires the media type itself, which is `is_calendar()`, and a caller asks
both.

**Two surfaces gained a parameter or a return type the sketch did not name.**
`resolve_instance` answers a `ResolvedInstance` rather than an `InstanceRef`, because the same
paragraph asks for `nearest_known()` on what comes back and `InstanceRef` is frozen with nowhere
to keep an `AnswerBasis`; `ResolvedInstance::reference()` and `From<ResolvedInstance> for
InstanceRef` hand back exactly the value the sketch named. `inspect_message` takes
`Option<PartyId<'_>>`, because it is required to report `scheduling-sender-not-permitted` for a
supplied actor and the actor has to arrive somehow; `None` is the ordinary inbox case, where a
file is being inspected rather than a sender judged.

### Three gaps the corpus found in the gate

The conformance chapter was written from RFC 5546 rather than from the implementation, and
landed with three cases the gate failed. All three were the specification's reading and all
three are now closed; the alternative — editing the corpus to agree — would have retired the
only instrument that can find this class of defect.

1. **Section 3's `SUBCOMPONENTS` rows were never read.** `check_conformance` counted properties
   and nothing else, so a `REPLY` carrying a `VALARM` — a `0` row — was accepted whole, and an
   attendee's answer could install a component the recipient's client will act on. The gate now
   runs `check_nesting`, and the refusal is `AuthorizationDenied::MethodForbidsComponent(kind)`
   rather than a `PropertyOccurrence` carrying a component's name, because a nested `VALARM` is
   not a property and a caller looking that name up among the payload's properties would find
   nothing. The `COMPONENTS` rows are deliberately still unread here: `ItipMessage::read` already
   refuses a second payload kind and a payload the tables do not nest, earlier and for the whole
   message.
2. **`PUBLISH` and `REQUEST` could never create anything.** The sending party was resolved
   against the state the caller holds, which for the two methods whose whole purpose is to
   arrive first names nobody — so both were refused `OrganizerMismatch` and
   `TransitionReason::Created` was unreachable. The lookup now falls back to the payload when the
   prior state is absent. What that costs is stated in `SECURITY.md` and in `authorize.rs`: for a
   first message this gate proves the actor is a party *the message names*, and the claim that
   the actor really sent it rests entirely on the transport.
3. **A `REFRESH` described the removal of the organizer's calendar.** It was diffed as a
   restatement of the component, so it stated a removal for every property its four lines do not
   echo, and the field rule then refused the attendee for removals the diff had invented.
   `describe_payload` now answers an empty transition for it. The revision gate is skipped for
   any method whose table forbids `SEQUENCE` — read from the table rather than special-cased —
   because such a method states no version of its own and the absent-is-zero reading would make
   every refresh stale against every held revision above zero.
