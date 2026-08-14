// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Whether the party applying a message is entitled to the change it asks for.
//!
//! Specification: RFC 5546 section 1.3 (the two roles), section 2.1 (who a message is between),
//! section 2.1.4 (`SEQUENCE`), section 2.1.5 (`DTSTAMP`), section 3 (per-method restrictions).
//!
//! Authorization is the first half of the semantics, not a layer above them. The positions
//! where scheduling implementations have historically been exploited — a reply that moves a
//! meeting, a reply from an address nobody invited, a stale `SEQUENCE` overwriting a newer
//! one — are all positions where the message and the identity have to be judged together or
//! not at all.
//!
//! # The gate, in order
//!
//! [`evaluate_message`] runs a fixed order and a denial names the first reason a caller can
//! act on: **identity**, then **sender**, then **method conformance**, then **revision**, then
//! **fields**. There is no partial success. A message that overreaches on one property is
//! denied whole, because applying its permitted half would leave the caller holding a
//! component no party ever described.
//!
//! Conformance sits ahead of the revision because it is the half of the judgment that is
//! about the message alone. A `REPLY` carrying no `DTSTAMP` is a message RFC 5546 section
//! 3.2.3's table refuses; ordering it first instead would report it as *stale*, since a
//! revision with no timestamp loses the tie to one that has a timestamp — a true statement
//! about the comparison and the wrong thing to tell a caller about a message whose own table
//! it never satisfied.
//!
//! # What survives the byte boundary, stated plainly
//!
//! ADR-0004 leaves this library no session, so a propose-then-confirm exchange crosses a
//! request boundary. A wrapper whose only guarantee is "this crate built it" guarantees
//! **nothing** across that boundary: encode it and a forged copy is indistinguishable from a
//! genuine one, and the sealed constructor then attests to the transport rather than to the
//! gate.
//!
//! So [`Authorization`] is not encodable, and that is enforced by a borrow rather than by a
//! naming convention. It holds `&'a ItipMessage` and `&'a dyn ScheduledComponent`, so it
//! cannot outlive either, cannot be stored in a session, and cannot be reconstructed from
//! bytes — there is no owned form to reconstruct. A caller that tries to carry one across a
//! request gets a compile error instead of a forgeable token, which is the whole of the
//! improvement over a wrapper that would merely have been *documented* as unserializable.
//! [`apply_transition`] then takes it **by value**, so a vetted transition is a single-use
//! capability rather than something replayable against a second target.
//!
//! What that still does not prove is **freshness**. The borrowed state may be a snapshot the
//! caller read minutes ago, and a genuine `Authorization` over a stale snapshot is wrong in a
//! way no lifetime can see. Binding a transition to an `ETag` is ADR-0004 territory and
//! undesigned. [`Commitment`] is the one value here designed to cross bytes, and it is
//! deliberately not a capability: it carries **no authority**, it is compared only to cause a
//! *refusal*, and its digest is a checksum rather than a MAC. An attacker who forges one gains
//! exactly one thing — the ability to decline to be told that the target moved — and the gate
//! below ran fresh either way. Nothing here should ever be changed to grant on a `Commitment`.

use crate::internal::core::{ComponentKind, Instant, PropertyId, ProposedChange};

use crate::internal::itip::diff::{attendee_occurrence_of, describe_payload, reason_for};
use crate::internal::itip::identity::{MessageIdentity, Revision, Uid};
use crate::internal::itip::message::ItipMessage;
use crate::internal::itip::method::{ActorRole, Method};
use crate::internal::itip::party::PartyId;
use crate::internal::itip::state::{PropertyOccurrence, ScheduledComponent, property_value};
use crate::internal::itip::table::{MethodRule, PriorState};
use crate::internal::itip::transition::{
    ApplyReport, FieldRule, ScheduleTarget, Transition, TransitionReason, field_rule,
};

/// Why a message was refused.
///
/// `#[non_exhaustive]`, so a caller's `match` keeps a `_` arm and a new refusal is not a major
/// version. Every variant is a whole-message refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizationDenied {
    /// The message carries no payload about the component being judged.
    UidMismatch,
    /// The message names an instance the local copy does not have.
    NoMatchingInstance,
    /// The message names an instance nothing could tell from its neighbor.
    ///
    /// The two halves of a repeated hour are one cadence key, and a guess between them
    /// cancels or moves somebody else's meeting. See [`crate::internal::itip::FoldSide`].
    AmbiguousInstance,
    /// The sender is on neither the attendee list nor the organizer line.
    UnknownAttendee,
    /// A `CAL-ADDRESS` the decision turns on was present and identifies nobody.
    ///
    /// RFC 5546 section 3.2.3 makes the `ATTENDEE` of a `REPLY` the address of the attendee
    /// replying, so a value that does not decode — or that is empty — is an answer on behalf of
    /// no party. It is a refusal rather than a transition describing nothing, because the two
    /// look identical to a caller and only one of them means the attendee's answer was dropped.
    /// [`DiagnosticCode::SchedulingCalendarAddressUnreadable`] is the same fact reported.
    ///
    /// [`DiagnosticCode::SchedulingCalendarAddressUnreadable`]: crate::internal::core::DiagnosticCode::SchedulingCalendarAddressUnreadable
    CalendarAddressUnreadable,
    /// The sender is not the organizer this component names.
    OrganizerMismatch,
    /// RFC 5546 does not permit this party to send this method.
    MethodForbidsSender(ActorRole),
    /// The message's `SEQUENCE` was present and unreadable, so it has no revision.
    SequenceUnreadable,
    /// The message is an older revision than the one already held.
    SequenceStale {
        /// The `SEQUENCE` the caller already holds.
        have: u32,
    },
    /// The message is the same revision with an older `DTSTAMP`.
    DtstampStale {
        /// The `DTSTAMP` the caller already holds.
        have: Instant,
    },
    /// RFC 5546 does not permit this method to act on what the caller holds.
    PriorStateForbidden(PriorState),
    /// A `RECURRENCE-ID` reached further than this method may.
    RangeNotPermitted,
    /// A `CANCEL` carried `STATUS` with a value other than `CANCELLED`.
    CancellationStatusInvalid,
    /// A `CANCEL` named neither an instance, an entire component, nor an affected attendee.
    CancellationTargetMissing,
    /// The message states a property RFC 5546 forbids it, or one this sender may not change.
    MethodForbidsField(PropertyOccurrence),
    /// The message lacks a property RFC 5546 requires of it.
    MethodRequiresField(PropertyId),
    /// The message nests a component RFC 5546 forbids inside its payload.
    ///
    /// A [`ComponentKind`] rather than a [`PropertyOccurrence`] naming the same octets: a
    /// nested `VALARM` is not a property, and a caller matching on this and looking the name up
    /// among the payload's properties would find nothing there.
    MethodForbidsComponent(ComponentKind),
}

/// A transition that has passed every gate, for this message against this state.
///
/// Sealed: no public constructor, no public field, no `From`, no `Default`, no `Clone`. It is
/// `ical-itip`'s alone and not a generic `Authorized<T>` shared with `ical-core`, because a
/// wrapper parametrized only on *what changed* would prove that some sealed constructor ran
/// somewhere rather than that RFC 5546's checks ran for this value.
///
/// It borrows both inputs, which is what makes "not encodable" a property of the type rather
/// than a promise in prose: see this module's own documentation for what that does and does
/// not buy.
#[derive(Debug)]
pub struct Authorization<'a> {
    /// The message the decision was made about.
    message: &'a ItipMessage<'a>,
    /// The state it was judged against.
    current: &'a dyn ScheduledComponent,
    /// What the actor turned out to be.
    actor: ActorRole,
    /// What the message is about.
    identity: MessageIdentity,
    /// What it would change.
    transition: Transition,
}

impl<'a> Authorization<'a> {
    /// The message this decision was made about.
    #[must_use]
    pub const fn message(&self) -> &'a ItipMessage<'a> {
        self.message
    }

    /// The state it was judged against.
    #[must_use]
    pub const fn current(&self) -> &'a dyn ScheduledComponent {
        self.current
    }

    /// What the actor turned out to be.
    #[must_use]
    pub const fn actor(&self) -> ActorRole {
        self.actor
    }

    /// What the message is about.
    #[must_use]
    pub const fn identity(&self) -> &MessageIdentity {
        &self.identity
    }

    /// What kind of change this is.
    #[must_use]
    pub const fn reason(&self) -> TransitionReason {
        self.transition.reason()
    }

    /// What would change.
    #[must_use]
    pub const fn transition(&self) -> &Transition {
        &self.transition
    }

    /// The transition, taken out of its authorization.
    ///
    /// What is left is inert, which is the point: a caller that wants to keep the description
    /// after applying it keeps a value that cannot apply anything.
    #[must_use]
    pub fn into_transition(self) -> Transition {
        self.transition
    }

    /// Whether this decision is about the same thing `commitment` recorded.
    ///
    /// Compared to *refuse*, never to grant: a caller confirming an action a user approved
    /// asks this after re-evaluating, so that a target which moved in between is caught rather
    /// than silently overwritten. Answering `true` grants nothing on its own.
    #[must_use]
    pub fn honors(&self, commitment: &Commitment) -> bool {
        Commitment::of(self) == *commitment
    }
}

/// What a user was shown, in a form that can cross a request boundary.
///
/// Carries **no authority**. It exists so that a confirm turn can notice that the thing being
/// confirmed is no longer the thing that was described — a racing organizer update, a second
/// message about the same identity — and refuse. Its digest is a non-cryptographic checksum
/// and is not a MAC: an attacker who can choose one gains only the ability to skip that
/// staleness refusal, and [`evaluate_message`] ran fresh regardless.
///
/// A `Commitment` is `Clone` and owns everything it holds, so a caller serializes it with
/// whatever encoding it already has. This crate ships none, because shipping one would invite
/// exactly the reading this paragraph exists to prevent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitment {
    /// What the message was about.
    identity: MessageIdentity,
    /// The revision the message carried.
    offered: Revision,
    /// The revision the state carried, absent when the caller held nothing readable.
    held: Option<Revision>,
    /// What kind of change was described.
    reason: TransitionReason,
    /// How many occurrences it touched.
    changes: u32,
    /// A checksum over those occurrences and their changes. Not a MAC.
    digest: u64,
}

impl Commitment {
    /// What `authorization` described, recorded so a later turn can notice a difference.
    #[must_use]
    pub fn of(authorization: &Authorization<'_>) -> Self {
        let payload = authorization
            .message
            .payload_for(authorization.current)
            .or_else(|| authorization.message.payload(0));
        let offered = payload
            .and_then(|component| Revision::read(component.sequence(), component.dtstamp()))
            .unwrap_or_else(|| Revision::new(0, None));
        Self {
            identity: authorization.identity.clone(),
            offered,
            held: revision_of(authorization.current),
            reason: authorization.transition.reason(),
            changes: u32::try_from(authorization.transition.len()).unwrap_or(u32::MAX),
            digest: digest_of(&authorization.transition),
        }
    }

    /// What the message was about.
    #[must_use]
    pub const fn identity(&self) -> &MessageIdentity {
        &self.identity
    }

    /// The revision the message carried.
    #[must_use]
    pub const fn offered(&self) -> Revision {
        self.offered
    }

    /// The revision the state carried when the description was made.
    #[must_use]
    pub const fn held(&self) -> Option<Revision> {
        self.held
    }

    /// What kind of change was described.
    #[must_use]
    pub const fn reason(&self) -> TransitionReason {
        self.reason
    }

    /// How many occurrences it touched.
    #[must_use]
    pub const fn changes(&self) -> u32 {
        self.changes
    }

    /// The checksum over those occurrences. Not a MAC; see this type's own documentation.
    #[must_use]
    pub const fn digest(&self) -> u64 {
        self.digest
    }
}

/// FNV-1a over every occurrence and change, in the order a transition iterates them.
///
/// Non-cryptographic on purpose and documented as such where it is used. It detects a target
/// that moved between two turns, which is an accident rather than an adversary.
fn digest_of(transition: &Transition) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut eat = |octets: &[u8]| {
        for octet in octets {
            hash ^= u64::from(*octet);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for (at, change) in transition.changes() {
        eat(at.name());
        eat(&at.index().to_le_bytes());
        match change {
            ProposedChange::Add(line) => {
                eat(b"+");
                eat(line.as_bytes());
            },
            ProposedChange::Replace(line) => {
                eat(b"=");
                eat(line.as_bytes());
            },
            ProposedChange::SetParameters(edits) => {
                eat(b"p");
                for edit in edits {
                    eat(edit.name());
                    eat(edit.value().unwrap_or(b""));
                }
            },
            ProposedChange::Remove => eat(b"-"),
        }
    }
    hash
}

/// What `actor` is to `current`, or `None` when they are neither party to it.
///
/// The organizer line is consulted before the attendee list, because RFC 5546 section 1.3 lets
/// one calendar user be both and the organizer's permissions are the wider ones.
#[must_use]
pub fn actor_role(current: &dyn ScheduledComponent, actor: PartyId<'_>) -> Option<ActorRole> {
    if let Some(role) = current
        .organizer()
        .and_then(|party| party.role_of(actor, ActorRole::Organizer, ActorRole::OrganizerAgent))
    {
        return Some(role);
    }
    (0..current.attendee_count())
        .find_map(|index| current.attendee(index).and_then(|who| who.role_of(actor)))
}

/// Which `ATTENDEE` of `current` is `who`, if any.
#[must_use]
pub fn attendee_index(current: &dyn ScheduledComponent, who: PartyId<'_>) -> Option<usize> {
    (0..current.attendee_count()).find(|index| {
        current
            .attendee(*index)
            .is_some_and(|found| found.party().is(who))
    })
}

/// The revision `component` states, or `None` when its `SEQUENCE` could not be read.
fn revision_of(component: &dyn ScheduledComponent) -> Option<Revision> {
    Revision::read(component.sequence(), component.dtstamp())
}

/// Whether `current` is a component the caller actually holds.
///
/// Asked of the component as a whole and never of its `UID`. A `UID` that does not read is a
/// component whose *identity* cannot be compared, which is a reason to refuse the message in
/// [`matching_payload`]; reading it as "the caller holds nothing" is a reason to look the
/// sending party up in the attacker's own message instead of in the recipient's copy, and a
/// stranger is then authorized to rewrite the organizer line of a meeting the caller is holding
/// while this function answers that the caller holds no such meeting.
///
/// So absence is absence: a component that states no property, carries no attendee and nests
/// nothing is the placeholder a caller passes when it has nothing, and everything else is
/// something it has.
fn prior_state(current: &dyn ScheduledComponent) -> PriorState {
    let empty = current.property_count() == 0
        && current.attendee_count() == 0
        && current.child_count() == 0;
    if empty {
        PriorState::Absent
    } else {
        PriorState::Present
    }
}

/// Judge `message` against `current` on behalf of `actor`.
///
/// # Errors
///
/// [`AuthorizationDenied`], naming the first gate that refused. There is no partial success;
/// see this module's own documentation for the order and for why.
pub fn evaluate_message<'a>(
    message: &'a ItipMessage<'a>,
    current: &'a dyn ScheduledComponent,
    actor: PartyId<'_>,
) -> Result<Authorization<'a>, AuthorizationDenied> {
    let constraints = message.rule();
    let prior = prior_state(current);
    if !constraints.permits_prior(prior) {
        return Err(AuthorizationDenied::PriorStateForbidden(prior));
    }
    let payload = matching_payload(message, current, prior)?;
    evaluate_selected(message, current, actor, prior, payload)
}

/// Judge one payload of a multi-component message against an absent state.
///
/// The ordinary entry point selects the first payload when nothing is held. A facade applying
/// an initial recurring object must authorize every master and detached component before it
/// inserts any of them, so this entry point makes that selection explicit while retaining the
/// same conformance, sender, range, field, and revision gates.
pub fn evaluate_initial_payload<'a>(
    message: &'a ItipMessage<'a>,
    current: &'a dyn ScheduledComponent,
    payload_index: usize,
    actor: PartyId<'_>,
) -> Result<Authorization<'a>, AuthorizationDenied> {
    let constraints = message.rule();
    let prior = prior_state(current);
    if prior != PriorState::Absent || !constraints.permits_prior(prior) {
        return Err(AuthorizationDenied::PriorStateForbidden(prior));
    }
    let payload = message
        .payload(payload_index)
        .ok_or(AuthorizationDenied::UidMismatch)?;
    evaluate_selected(message, current, actor, prior, payload)
}

/// Judge one explicitly selected payload as a not-yet-materialized instance of a held master.
///
/// This is the narrow composition seam used by the public workflow after it has proved that
/// the payload's `RECURRENCE-ID` belongs to the master's recurrence set. The low-level
/// [`evaluate_message`] keeps its exact-component identity contract; callers cannot
/// accidentally turn an arbitrary instance reference into stored state through it.
pub(crate) fn evaluate_new_instance_payload<'a>(
    message: &'a ItipMessage<'a>,
    current: &'a dyn ScheduledComponent,
    payload_index: usize,
    actor: PartyId<'_>,
) -> Result<Authorization<'a>, AuthorizationDenied> {
    let constraints = message.rule();
    let prior = prior_state(current);
    if prior != PriorState::Present || !constraints.permits_prior(prior) {
        return Err(AuthorizationDenied::PriorStateForbidden(prior));
    }
    if current.recurrence_id().is_some()
        || !message
            .uid()
            .matches(&Uid::new(current.uid().unwrap_or(&[])))
    {
        return Err(AuthorizationDenied::UidMismatch);
    }
    let payload = message
        .payload(payload_index)
        .filter(|payload| payload.recurrence_id().is_some())
        .ok_or(AuthorizationDenied::NoMatchingInstance)?;
    evaluate_selected(message, current, actor, prior, payload)
}

fn evaluate_selected<'a>(
    message: &'a ItipMessage<'a>,
    current: &'a dyn ScheduledComponent,
    actor: PartyId<'_>,
    prior: PriorState,
    payload: &'a dyn ScheduledComponent,
) -> Result<Authorization<'a>, AuthorizationDenied> {
    let constraints = message.rule();
    let role = sender_role(message, sender_state(current, payload, prior), actor)?;
    check_conformance(constraints, payload)?;
    check_nesting(constraints, payload)?;
    check_range(message.method(), payload)?;
    check_cancellation(message.method(), payload)?;
    if !constraints.presence_of(b"SEQUENCE").is_forbidden() {
        check_revision(payload, current)?;
    }
    if message.method() == Method::Reply {
        check_answer(payload, current)?;
    }

    let described = describe_payload(message.method(), payload, current);
    check_fields(&described, current, role, actor)?;
    let transition = settle(message.method(), prior, (payload, current), described);
    Ok(Authorization {
        message,
        current,
        actor: role,
        identity: MessageIdentity::new(message.uid().clone(), payload.recurrence_id()),
        transition,
    })
}

/// The payload this message carries about `current`.
///
/// For a method acting on nothing the caller holds, the first payload is the one being
/// created. Otherwise the identity has to match exactly: an ambiguous instance is a denial
/// rather than a pick, which is what closes M2's repeated-hour question here.
fn matching_payload<'a>(
    message: &ItipMessage<'a>,
    current: &dyn ScheduledComponent,
    prior: PriorState,
) -> Result<&'a dyn ScheduledComponent, AuthorizationDenied> {
    if prior == PriorState::Absent {
        return message.payload(0).ok_or(AuthorizationDenied::UidMismatch);
    }
    if !message
        .uid()
        .matches(&Uid::new(current.uid().unwrap_or(&[])))
    {
        return Err(AuthorizationDenied::UidMismatch);
    }
    if let Some(payload) = message.payload_for(current) {
        return Ok(payload);
    }
    // A THISANDFUTURE payload is the anchor that creates a detached component when the caller
    // currently holds only the series master. It therefore has no exact component identity to
    // match until this authorization is applied. Match exactly one such anchor to the master;
    // the caller still has to materialize the split, and every sender, revision and field gate
    // below runs against the master before it may do so.
    if current.recurrence_id().is_none() && message.method() == Method::Request {
        let mut split = None;
        for index in 0..message.payload_count() {
            let Some(payload) = message.payload(index) else {
                continue;
            };
            if !payload
                .recurrence_id()
                .is_some_and(crate::internal::itip::InstanceRef::is_this_and_future)
            {
                continue;
            }
            if split.is_some() {
                return Err(AuthorizationDenied::NoMatchingInstance);
            }
            split = Some(payload);
        }
        if let Some(payload) = split {
            return Ok(payload);
        }
    }
    let target = MessageIdentity::new(message.uid().clone(), current.recurrence_id());
    let ambiguous = (0..message.payload_count()).any(|index| {
        message.payload_identity(index).is_some_and(|identity| {
            identity.matches(&target) == crate::internal::itip::InstanceMatch::Ambiguous
        })
    });
    if ambiguous {
        Err(AuthorizationDenied::AmbiguousInstance)
    } else {
        Err(AuthorizationDenied::NoMatchingInstance)
    }
}

/// Which component the sending party is looked up in.
///
/// The state the caller holds, whenever it holds one: an `ORGANIZER` line a recipient already
/// has is the only statement about who runs this meeting that the sender did not write.
///
/// When the prior state is absent there is no such line, and the two methods RFC 5546 lets act
/// on nothing — `PUBLISH` (section 3.2.1) and `REQUEST` (section 3.2.2) — exist precisely to
/// arrive before the recipient has one. Looking the party up in the state would answer `None`
/// for both, so an invitation could never be accepted and
/// [`TransitionReason::Created`] could never be reached. So the payload answers instead, and
/// what that costs is stated plainly: for a first message this gate proves that the actor the
/// caller named is a party **the message names**, and nothing more. Whether that actor really
/// sent it is the transport's answer — an authenticated CalDAV session, or the iMIP envelope
/// checks in [`crate::internal::itip::imip`] — and `SECURITY.md` says so in the same words.
const fn sender_state<'a>(
    current: &'a dyn ScheduledComponent,
    payload: &'a dyn ScheduledComponent,
    prior: PriorState,
) -> &'a dyn ScheduledComponent {
    match prior {
        PriorState::Present => current,
        PriorState::Absent => payload,
    }
}

/// What `actor` is, refused when RFC 5546 does not let that party send this method.
fn sender_role(
    message: &ItipMessage<'_>,
    current: &dyn ScheduledComponent,
    actor: PartyId<'_>,
) -> Result<ActorRole, AuthorizationDenied> {
    let role = actor_role(current, actor).ok_or({
        if message.method().is_organizer_authored() {
            AuthorizationDenied::OrganizerMismatch
        } else {
            AuthorizationDenied::UnknownAttendee
        }
    })?;
    if role.satisfies(message.rule().sender()) {
        Ok(role)
    } else {
        Err(AuthorizationDenied::MethodForbidsSender(role))
    }
}

/// RFC 5546 sections 2.1.4 and 2.1.5: an older version never overwrites a newer one.
///
/// Run only for a method whose table admits a `SEQUENCE`, which is the caller's check above and
/// not a special case here. Section 3.2.6's `REFRESH` table gives `SEQUENCE` the value `0`: a
/// refresh asks for the latest version and states no version of its own, so reading one out of
/// it yields the absent-is-zero reading and makes every refresh stale against every held
/// revision above zero. A method that states no revision overwrites nothing, so there is
/// nothing for these two sections to order.
fn check_revision(
    payload: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    let offered = revision_of(payload).ok_or(AuthorizationDenied::SequenceUnreadable)?;
    let Some(held) = revision_of(current) else {
        // The caller's own copy has no readable revision, so there is nothing to be stale
        // against. Refusing here would make an unreadable local `SEQUENCE` reject every
        // message about it, which is a denial of service against the recipient.
        return Ok(());
    };
    if !offered.is_stale_against(held) {
        return Ok(());
    }
    if offered.sequence() < held.sequence() {
        return Err(AuthorizationDenied::SequenceStale {
            have: held.sequence(),
        });
    }
    held.dtstamp().map_or(Ok(()), |have| {
        Err(AuthorizationDenied::DtstampStale { have })
    })
}

/// RFC 5546 section 2.1.5, applied to the two messages it was written for: one attendee's
/// answers.
///
/// The revision gate above orders a message against the component; two replies from one
/// attendee are one revision of that component answered twice, so nothing it compares can tell
/// them apart. What can is the time the answer already on the line was written at, and
/// [`ScheduledComponent::attendee_answered_at`] is where a state keeps it.
///
/// A state that keeps none admits the second answer, which is the direction that lets an
/// attendee change its mind; a state that keeps one refuses an answer that is not newer, which
/// is the direction that stops an attendee's own earlier answer, replayed, from reverting the
/// current one. A reply whose `DTSTAMP` cannot be read is refused against a recorded time for
/// the same reason a message with no tie-break does not win a tie.
fn check_answer(
    payload: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    let Some(answer) = payload.attendee(0) else {
        // Section 3.2.3's `1` row has already refused a reply with no `ATTENDEE`, so this arm
        // is a `ScheduledComponent` whose count and whose lookup disagree.
        return Ok(());
    };
    let Some(who) = answer.party().address() else {
        return Err(AuthorizationDenied::CalendarAddressUnreadable);
    };
    let Some(have) = attendee_index(current, who).and_then(|at| current.attendee_answered_at(at))
    else {
        return Ok(());
    };
    if payload.dtstamp().is_some_and(|stated| stated > have) {
        Ok(())
    } else {
        Err(AuthorizationDenied::DtstampStale { have })
    }
}

/// The transition as it stands once RFC 5546 section 2.1.4 has been read.
///
/// Section 2.1.4 requires the organizer to increment `SEQUENCE` for every update it sends, so
/// an organizer-authored message that does not supersede the revision the caller holds is not a
/// newer version of it — it is the version already held, restated, whatever else its lines say.
/// Two messages at one revision are one version, and one of them is not the one the organizer
/// sent; describing the second as a change would let a captured message replayed with an edited
/// time move the meeting without any revision moving.
///
/// So it describes nothing rather than being refused, because the commonest message of this
/// shape is a message already applied arriving twice, and a caller shown "no change" for a
/// duplicate is being told the truth. What the message *claimed* stays reachable through
/// [`describe_message`](crate::internal::itip::describe_message), which is ADR-0005's own recommendation.
///
/// Attendee-authored methods are left alone: a `REPLY` or a `COUNTER` restates the revision it
/// answers and never claims to be a newer one, so there is nothing here for section 2.1.4 to
/// say about it. So is a caller that holds nothing: the two methods RFC 5546 admits against an
/// absent prior state exist to arrive first, and there is no revision of anything for a first
/// message to fail to supersede.
///
/// The two components arrive as a pair rather than as two parameters because they are the two
/// sides of one comparison, and `clippy::similar_names` is right that `payload` and `current`
/// read alike at a call site with four arguments.
fn settle(
    method: Method,
    prior: PriorState,
    versions: (&dyn ScheduledComponent, &dyn ScheduledComponent),
    described: Transition,
) -> Transition {
    let (payload, current) = versions;
    if !method.is_organizer_authored() || described.is_empty() || prior == PriorState::Absent {
        return described;
    }
    let advanced = match (revision_of(payload), revision_of(current)) {
        (Some(offered), Some(held)) => offered.supersedes(held),
        // Either side stating no readable revision leaves section 2.1.4 nothing to order, and
        // the message stands on whatever the gates above made of it.
        _ => true,
    };
    if advanced {
        described
    } else {
        Transition::new(reason_for(method, current, false))
    }
}

/// RFC 5546 section 3.2.3: a `REPLY` answers one instance, not every later one.
fn check_range(
    method: Method,
    payload: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    let reaching = payload
        .recurrence_id()
        .is_some_and(crate::internal::itip::InstanceRef::is_this_and_future);
    if reaching && matches!(method, Method::Reply | Method::Refresh) {
        return Err(AuthorizationDenied::RangeNotPermitted);
    }
    Ok(())
}

/// RFC 5546 sections 3.2.5, 3.4.5, and 3.5.3's conditional `CANCEL` rows.
fn check_cancellation(
    method: Method,
    payload: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    if method != Method::Cancel {
        return Ok(());
    }
    let status = property_value(payload, b"STATUS");
    if status.is_some_and(|value| !value.eq_ignore_ascii_case(b"CANCELLED")) {
        return Err(AuthorizationDenied::CancellationStatusInvalid);
    }
    if matches!(
        payload.component_kind(),
        Some(ComponentKind::Event | ComponentKind::Todo)
    ) && payload.recurrence_id().is_none()
        && status.is_none()
        && payload.attendee_count() == 0
    {
        return Err(AuthorizationDenied::CancellationTargetMissing);
    }
    Ok(())
}

/// The transcribed section 3 table, evaluated against one payload.
///
/// Counted per name over the payload's own properties, so a `0` row is refused however many
/// times it appears and a `1` row is refused when it appears twice. The `SUBCOMPONENTS` rows
/// are read the same way by [`check_nesting`].
///
/// Every row is read and not only the ones that require something. A `0 or 1` row left unread
/// is a row that says a name means one thing when it appears and something else when it
/// appears twice: section 3.2.5 gives `CANCEL` a `RECURRENCE-ID` of `0 or 1`, so a message
/// stating it twice satisfies neither the instance reading nor the series reading — and the
/// duplicate widens the message's reach from one occurrence to the whole recurring event,
/// which is the shape a reader must never resolve by picking.
///
/// The `COMPONENTS` rows are deliberately **not** read here. They state which top-level
/// components a message may carry, and [`crate::internal::itip::ItipMessage::read`] already refuses a second
/// payload kind (`MixedPayloadKinds`) and a payload the tables never nest at the top level
/// (`UnsupportedPayload`) — earlier than this gate and for the whole message rather than for
/// one payload. A second reading here would be a weaker restatement of a refusal that already
/// happened, and two gates over one rule are two places for the answer to drift.
fn check_conformance(
    constraints: MethodRule,
    payload: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    let mut counts: alloc::collections::BTreeMap<&[u8], usize> =
        alloc::collections::BTreeMap::new();
    for index in 0..payload.property_count() {
        let Some(name) = payload.property_name(index) else {
            continue;
        };
        if constraints.presence_of(name).is_forbidden() {
            let seen = counts.get(name).copied().unwrap_or(0);
            return Err(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(name, seen),
            ));
        }
        let seen = counts.entry(name).or_insert(0);
        *seen = seen.saturating_add(1);
    }
    for row in constraints.properties() {
        let seen = counts
            .iter()
            .find(|(name, _)| row.is_named(name))
            .map_or(0, |(_, count)| *count);
        if row.presence().admits(seen) {
            continue;
        }
        if seen == 0 {
            return Err(AuthorizationDenied::MethodRequiresField(
                PropertyId::from_name(row.name()),
            ));
        }
        // Every row that fails with something present fails for having too much of it, and the
        // occurrence named is the first one the row does not admit.
        return Err(AuthorizationDenied::MethodForbidsField(
            PropertyOccurrence::named(row.name(), seen.saturating_sub(1)),
        ));
    }
    Ok(())
}

/// The `SUBCOMPONENTS` rows of the same table, evaluated against the payload's own children.
///
/// Section 3.2.3 gives `VALARM` the value `0` for a `REPLY`, and an alarm arriving on an
/// attendee's answer is a component the recipient's client will act on — which is why this is a
/// refusal and not a diagnostic.
///
/// Only the forbidden direction, because there is nothing else to read: every `SUBCOMPONENTS`
/// row RFC 5546 section 3 prints is `0` or `0+`, so no nested component is ever required. That
/// is machine-checked below rather than trusted, so a row added later cannot slip past.
///
/// A child whose name this workspace has no schema for is not judged. `ComponentKind::from_name`
/// answering `None` means "no schema" and never "not allowed" — its own words — so refusing on
/// it would put a meaning into that answer that `ical-core` does not give it.
fn check_nesting(
    constraints: MethodRule,
    payload: &dyn ScheduledComponent,
) -> Result<(), AuthorizationDenied> {
    for index in 0..payload.child_count() {
        let Some(kind) = payload
            .child(index)
            .and_then(ScheduledComponent::component_kind)
        else {
            continue;
        };
        if constraints
            .subcomponent_presence(kind.as_bytes())
            .is_forbidden()
        {
            return Err(AuthorizationDenied::MethodForbidsComponent(kind));
        }
    }
    Ok(())
}

/// Which of the described changes this sender is entitled to make.
///
/// An organizer-side actor may write anything the method's table admits. An attendee-side one
/// may write only [`FieldRule::EitherParty`] properties and its own `ATTENDEE` line — which is
/// the rule that stops a reply from moving a meeting, the first attack `SECURITY.md` names.
///
/// "Its own" is two questions and not one. The occurrence has to be the line this actor sits
/// at, **and** the line the change would leave behind has to still name this actor: a `COUNTER`
/// replacing the sender's own `ATTENDEE` line with a party the meeting never invited passes the
/// first question and is the substitution of a stranger for the sender, which takes the sender
/// off the list and puts somebody nobody asked for on it, on one authorized change.
fn check_fields(
    transition: &Transition,
    current: &dyn ScheduledComponent,
    role: ActorRole,
    actor: PartyId<'_>,
) -> Result<(), AuthorizationDenied> {
    if role.is_organizer_side() {
        return Ok(());
    }
    let own = attendee_occurrence_of(current, actor);
    for (at, change) in transition.changes() {
        let permitted = match field_rule(at.name()) {
            FieldRule::EitherParty => true,
            FieldRule::AttendeeOwn => own.as_ref() == Some(at) && keeps_the_actor(change, actor),
            FieldRule::OrganizerOnly => false,
        };
        if !permitted {
            return Err(AuthorizationDenied::MethodForbidsField(at.clone()));
        }
    }
    Ok(())
}

/// Whether the line `change` would leave behind still names `actor`.
///
/// A parameter edit does not reach the value, so the line names whoever it named. A whole line
/// written in has to name the actor itself, and a removal names nobody: an attendee stating
/// that it is no longer a participant is `PARTSTAT=DECLINED`, not the deletion of the line the
/// organizer is tracking that answer on.
fn keeps_the_actor(change: &ProposedChange, actor: PartyId<'_>) -> bool {
    match *change {
        ProposedChange::SetParameters(_) => true,
        ProposedChange::Add(ref line) | ProposedChange::Replace(ref line) => {
            address_of(line.as_bytes()).is_some_and(|written| written.matches(actor))
        },
        ProposedChange::Remove => false,
    }
}

/// The `CAL-ADDRESS` a written content line states, or `None` when it states none.
///
/// The value is what follows the first `:` outside a quoted parameter value, which is RFC 5545
/// section 3.1's own division of a content line and is the same one
/// [`crate::internal::itip::ScheduledView`] assembles a line by.
fn address_of(line: &[u8]) -> Option<PartyId<'_>> {
    let mut quoted = false;
    let cut = line.iter().position(|octet| match *octet {
        b'"' => {
            quoted = !quoted;
            false
        },
        b':' => !quoted,
        _ => false,
    })?;
    PartyId::from_bytes(line.get(cut..)?.get(1..)?)
}

/// Write an authorized transition to `target`, reporting what it took.
///
/// Takes the authorization **by value**, so a vetted transition is a single-use capability
/// rather than something a caller can replay against a second target after the state it was
/// vetted against has moved.
#[must_use]
pub fn apply_transition(
    target: &mut dyn ScheduleTarget,
    authorized: Authorization<'_>,
) -> ApplyReport {
    let mut report = ApplyReport::new();
    // Consumed rather than borrowed, which is what makes the capability single-use: the
    // description survives the call only if the caller asked for it back.
    let transition = authorized.into_transition();
    for (at, change) in transition.changes() {
        match target.write_change(at, change) {
            Ok(()) => report.note_applied(),
            Err(reason) => report.note_rejected(at.clone(), reason),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{ParameterEdit, ProposedChange, RawText};

    use super::{address_of, keeps_the_actor};
    use crate::internal::itip::party::PartyId;
    use crate::internal::itip::table::RULES;

    /// The second half of "its own `ATTENDEE` line": a line the actor no longer appears on is
    /// not the actor's line, however it is addressed.
    #[test]
    fn a_change_keeps_the_actor_only_when_the_line_it_leaves_behind_names_them() {
        let bo = PartyId::new("mailto:bo@example.com");
        let substitution = ProposedChange::Replace(RawText::from_bytes(
            b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:eve@example.com",
        ));
        assert!(!keeps_the_actor(&substitution, bo));

        let answer = ProposedChange::Replace(RawText::from_bytes(
            b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:bo@example.com",
        ));
        assert!(keeps_the_actor(&answer, bo));

        // A parameter edit does not reach the value, so it cannot move the line to anybody.
        let edited = ProposedChange::SetParameters(alloc::vec![ParameterEdit::set(
            b"PARTSTAT",
            b"DECLINED"
        )]);
        assert!(keeps_the_actor(&edited, bo));
        assert!(!keeps_the_actor(&ProposedChange::Remove, bo));
    }

    /// A content line's value is what follows the first `:` outside a quoted parameter, which
    /// is where a `SENT-BY` full of colons would otherwise end the search early.
    #[test]
    fn the_address_of_a_line_is_read_the_way_the_grammar_divides_one() {
        let quoted =
            address_of(b"ATTENDEE;SENT-BY=\"mailto:pa@example.com\":mailto:bo@example.com");
        assert_eq!(quoted.map(PartyId::as_str), Some("mailto:bo@example.com"));
        assert_eq!(
            address_of(b"ATTENDEE:").map(PartyId::as_str),
            Some(""),
            "an empty value is a value, and it is the one that identifies nobody"
        );
        assert_eq!(address_of(b"ATTENDEE"), None);
    }

    /// [`super::check_nesting`] reads only the forbidden direction of the `SUBCOMPONENTS` rows,
    /// and that is only correct while no such row requires anything. Checked against the
    /// transcribed tables rather than asserted in prose, so a row transcribed later from an RFC
    /// that does require a nested component fails here instead of being quietly unenforced.
    #[test]
    fn no_transcribed_subcomponent_row_requires_a_nested_component() {
        for rule in &RULES {
            for row in rule.subcomponents() {
                assert!(
                    !row.presence().is_required(),
                    "{} nests a required component, which the gate does not read",
                    rule.section()
                );
            }
        }
    }
}
