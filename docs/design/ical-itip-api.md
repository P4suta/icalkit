# `ical-itip` public API

- Status: accepted
- Date: 2026-08-10
- Implements: [ADR 0005](../adr/0005-scheduling-apart-from-the-model.md), with
  [0007](../adr/0007-allocation-policy.md), [0009](../adr/0009-error-and-diagnostic-model.md),
  [0010](../adr/0010-shared-resource-limits.md)
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below

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
    fn component_kind(&self) -> ComponentKind;
    fn method(&self) -> Option<Method>;
    fn uid(&self) -> Option<&[u8]>;
    fn sequence(&self) -> u32;
    fn dtstamp(&self) -> Option<Instant>;
    fn dtstart(&self) -> Option<Instant>;
    fn dtend(&self) -> Option<Instant>;
    fn recurrence_id(&self) -> Option<InstanceRef>;
    fn organizer(&self) -> Option<Party<'_>>;
    fn attendee_count(&self) -> usize;
    fn attendee(&self, index: usize) -> Option<Attendee<'_>>;
    fn attendee_property_id(&self, index: usize) -> Option<PropertyId>;
    fn property_count(&self) -> usize;
    fn property_id(&self, index: usize) -> Option<PropertyId>;
    fn property_bytes(&self, id: &PropertyId) -> Option<&[u8]>;
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

`ScheduledComponent` is how ADR-0005's `current: &Component` is spelled. `ical-itip` ships
`impl ScheduledComponent for ical_core::Component`, so the literal call
`evaluate_message(&message, &component, actor)` compiles unchanged — `&Component` unsizes to
`&dyn ScheduledComponent` at the argument position. Naming the trait buys two things: a CalDAV
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
pub struct Transition { changes: BTreeMap<PropertyId, ProposedChange>, reason: TransitionReason }

impl Transition {
    pub const fn reason(&self) -> TransitionReason;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn change(&self, property: &PropertyId) -> Option<&ProposedChange>;
    pub fn changes(&self) -> Changes<'_>;   // Iterator<Item = (&PropertyId, &ProposedChange)>
}

pub enum TransitionReason {
    Published, Created, Updated, Rescheduled, InstancesAdded,
    ParticipationChanged, Cancelled, RefreshRequested, CounterProposed, CounterDeclined,
}

pub fn describe_message(message: &ItipMessage<'_>, current: &dyn ScheduledComponent) -> Transition;
```

**Invariants.** `Transition` reuses `ical-core`'s `ProposedChange` rather than inventing a second
change vocabulary, keyed by `PropertyId` so two conflicting changes to one property occurrence
cannot both exist. It is inert: no method on it reaches a component, and the only route to
`apply_transition` is through `evaluate_message`. `describe_message` therefore hands a caller
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
    fn write_change(&mut self, property: &PropertyId, change: &ProposedChange)
        -> Result<(), WriteRejected>;
}

pub enum WriteRejected { UnknownProperty, ValueTypeMismatch, ReadOnly }
pub struct RejectedChange { /* property, reason */ }
pub struct ApplyReport { /* applied: u32, rejected: Vec<RejectedChange> */ }

pub fn apply_transition(
    target: &mut dyn ScheduleTarget,
    authorized: AuthorizedTransition,
) -> ApplyReport;
```

**Invariants.** `ical-itip` ships `impl ScheduleTarget for ical_core::Component`, routing each
change through the scoped `PropertyMut` guard of ADR-0001; a server whose storage is a row
implements the trait against its rows instead. A partial application is reported, never hidden:
this crate owns no transaction and cannot roll one back, so a caller that needs all-or-nothing
checks `ApplyReport::is_complete` before committing its own storage.

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

    let declared = MediaTypeParams::read(content_type);
    if !declared.agrees_with(&message) {
        return Ok(Prompt::Refused(AuthorizationDenied::UnknownAttendee));
    }
    if !sender_is_named(envelope_sender, current) {
        return Ok(Prompt::Refused(AuthorizationDenied::UnknownAttendee));
    }
    match evaluate_message(&message, current, envelope_sender) {
        Ok(authorized) => Ok(Prompt::Confirm(authorized.reason())),
        Err(denied) => Ok(Prompt::Refused(denied)),
    }
}
```

## What this makes worse

The `ScheduledComponent` trait is seventeen methods, and `ical-core` has to implement all of them
before this crate does anything at all. That is the price of not requiring a `Component` to
exist, and if no second implementation ever appears the trait earns its cost as insurance rather
than as demonstrated demand — the same bet the crate decomposition made on `ical-grammar`.

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
