// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a message would change, described and not made.
//!
//! A transition is a value. Nothing on it reaches a component, and applying one is a separate
//! call taking a separate type — which is what lets a mail client render "this meeting was
//! moved — accept?" before anything touches the user's calendar (ADR-0005).
//!
//! # The vocabulary, and the address
//!
//! The words are `ical-core`'s: [`ProposedChange`] and
//! [`ParameterEdit`](ical_core::ParameterEdit), reused rather than reinvented, which is the
//! coupling DP-13 bought. The *address* is this crate's, because
//! `ical-core`'s [`PropertyId`](ical_core::PropertyId) names an identity and a scheduling
//! message changes one attendee among many. [`crate::internal::itip::PropertyOccurrence`] is that address, and
//! [`ical_core::Component::apply_to_occurrence`] is the door it is applied through — a second
//! door beside the identity-addressed [`ical_core::Component::apply`], not a widening of it.
//!
//! The map is keyed on the occurrence, so two conflicting changes to one line cannot both
//! exist. A `Vec` would admit them and leave the resolution to whoever iterated last.

use alloc::collections::BTreeMap;
use alloc::collections::btree_map;
use alloc::vec::Vec;
use core::fmt::Debug;

use ical_core::ProposedChange;

use crate::internal::itip::state::PropertyOccurrence;

/// What kind of change a message describes, in RFC 5546's own terms.
///
/// A caller renders a prompt from this rather than from the method, because two methods can
/// describe one thing to a person: an updated `REQUEST` that moved the time and one that only
/// fixed a typo are `Rescheduled` and `Updated`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TransitionReason {
    /// An unsolicited posting. RFC 5546 section 3.2.1.
    Published,
    /// An invitation to a component the caller did not have. Section 3.2.2.
    Created,
    /// An update to one it did, leaving the time alone. Section 3.2.2.
    Updated,
    /// An update that moved the time. Section 3.2.2.1.
    Rescheduled,
    /// Instances added to an existing component. Section 3.2.4.
    InstancesAdded,
    /// One attendee's answer. Section 3.2.3.
    ParticipationChanged,
    /// The component, or some of its instances, cancelled. Section 3.2.5.
    Cancelled,
    /// A request for the latest version. Section 3.2.6.
    RefreshRequested,
    /// An alternative proposal from an attendee. Section 3.2.7.
    CounterProposed,
    /// An organizer declining one. Section 3.2.8.
    CounterDeclined,
}

/// Every change one message would make, keyed by the occurrence it would make it to.
///
/// Inert. No method here reaches a component, so a transition can be shown, stored, counted
/// and thrown away, and none of that applies anything. Applying one needs a
/// [`crate::internal::itip::Authorization`], which cannot be built from a transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// What kind of change this is.
    reason: TransitionReason,
    /// The changes, one per addressed occurrence.
    changes: BTreeMap<PropertyOccurrence, ProposedChange>,
}

impl Transition {
    /// A transition of kind `reason` that changes nothing yet.
    ///
    /// Public because a transition is inert: it describes and never acts, so nothing rests on
    /// who built one. What rests on provenance is [`crate::internal::itip::Authorization`], which this cannot
    /// become.
    #[must_use]
    pub const fn new(reason: TransitionReason) -> Self {
        Self {
            reason,
            changes: BTreeMap::new(),
        }
    }

    /// Record `change` against `at`, answering with whatever it displaced.
    ///
    /// A displacement is a caller describing one occurrence twice, and the later description
    /// wins — the same rule `ical_recur::PropertyDiff` states about restating a property
    /// inside one diff.
    pub fn record(
        &mut self,
        at: PropertyOccurrence,
        change: ProposedChange,
    ) -> Option<ProposedChange> {
        self.changes.insert(at, change)
    }

    /// What kind of change this is.
    #[must_use]
    pub const fn reason(&self) -> TransitionReason {
        self.reason
    }

    /// How many occurrences would change.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether nothing would change.
    ///
    /// A message that changes nothing is a normal outcome, not an error: a `REPLY` restating
    /// an answer already recorded is one, and a caller shows "no change" rather than a prompt.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The change to `at`, absent when that occurrence would not change.
    #[must_use]
    pub fn change(&self, at: &PropertyOccurrence) -> Option<&ProposedChange> {
        self.changes.get(at)
    }

    /// Every change, in occurrence order.
    #[must_use]
    pub fn changes(&self) -> Changes<'_> {
        Changes(self.changes.iter())
    }
}

/// Every change one transition would make, in occurrence order.
#[derive(Clone, Debug)]
pub struct Changes<'a>(btree_map::Iter<'a, PropertyOccurrence, ProposedChange>);

impl<'a> Iterator for Changes<'a> {
    type Item = (&'a PropertyOccurrence, &'a ProposedChange);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for Changes<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Which party a property belongs to, for the purposes of RFC 5546's field restrictions.
///
/// The generalization of section 3.2.2's and section 3.2.3's restriction tables, which state
/// per method that an attendee's reply may not change the organizer's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FieldRule {
    /// Only the organizer may change it. The default, including for unknown names.
    OrganizerOnly,
    /// An attendee may change it, on their own `ATTENDEE` line and nowhere else.
    AttendeeOwn,
    /// Either party may state it.
    EitherParty,
}

/// Which party may change the property `name`.
///
/// The default is [`FieldRule::OrganizerOnly`], `X-` names included. An unrecognized property
/// arriving in a `REPLY` is exactly the shape of an attendee smuggling state into an
/// organizer's copy, and a permissive default there is a hole no test written against the
/// properties we know will ever find. The price is the failure mode we prefer: the first real
/// interoperability report is "this legitimate `COUNTER` was refused", not a silent write.
///
/// # `ORGANIZER` and `SEQUENCE` are the organizer's, and echoing one is not changing it
///
/// Both were [`FieldRule::EitherParty`] on the reading that an attendee's `COUNTER` legally
/// *restates* them. That is true and is not what the rule decides: a transition holds only
/// occurrences whose octets differ from the ones held, so a restatement produces no entry and
/// is never asked about. What the permission actually bought was the other case — an
/// attendee-authored message whose `ORGANIZER` line names somebody else, which RFC 5546
/// section 3.2.7 gives an attendee no authority over and which hands the meeting away, and one
/// whose `SEQUENCE` is a number section 2.1.4 makes the *organizer's* to increment and which
/// the revision gate then reads back as the version to refuse everything older than.
#[must_use]
pub fn field_rule(name: &[u8]) -> FieldRule {
    for (spelling, rule) in [
        (&b"ATTENDEE"[..], FieldRule::AttendeeOwn),
        (b"REQUEST-STATUS", FieldRule::EitherParty),
        (b"COMMENT", FieldRule::EitherParty),
        (b"DTSTAMP", FieldRule::EitherParty),
        (b"UID", FieldRule::EitherParty),
        (b"RECURRENCE-ID", FieldRule::EitherParty),
    ] {
        if spelling.eq_ignore_ascii_case(name) {
            return rule;
        }
    }
    FieldRule::OrganizerOnly
}

/// Whether `name` is a property that says when something happens.
///
/// What separates [`TransitionReason::Rescheduled`] from [`TransitionReason::Updated`], and
/// the list an attendee may never write: moving a meeting by replying to it is the first
/// attack `SECURITY.md` names.
#[must_use]
pub fn is_time_property(name: &[u8]) -> bool {
    [
        &b"DTSTART"[..],
        b"DTEND",
        b"DUE",
        b"DURATION",
        b"RRULE",
        b"RDATE",
        b"EXDATE",
        b"RECURRENCE-ID",
    ]
    .iter()
    .any(|spelling| spelling.eq_ignore_ascii_case(name))
}

/// Where an authorized transition is written.
///
/// A trait because `ical-itip` owns no storage. It ships an implementation for
/// [`ical_core::Component`], routing each change through
/// [`ical_core::Component::apply_to_occurrence`]; a server whose storage is a row implements
/// this against its rows instead.
pub trait ScheduleTarget: Debug {
    /// Write one change, or say why this target will not.
    ///
    /// # Errors
    ///
    /// [`WriteRejected`], which is a report and never a reason to stop: a partial application
    /// is reported rather than hidden, because this crate owns no transaction and cannot roll
    /// one back.
    fn write_change(
        &mut self,
        at: &PropertyOccurrence,
        change: &ProposedChange,
    ) -> Result<(), WriteRejected>;
}

/// Why a target refused one change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WriteRejected {
    /// The target has no such property occurrence.
    UnknownProperty,
    /// The change's octets are not something this target can store.
    ValueTypeMismatch,
    /// The target will not have that property written at all.
    ReadOnly,
}

/// One change a target refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedChange {
    /// The occurrence the change addressed.
    at: PropertyOccurrence,
    /// Why the target refused it.
    reason: WriteRejected,
}

impl RejectedChange {
    /// A refusal of the change to `at`, for `reason`.
    #[must_use]
    pub const fn new(at: PropertyOccurrence, reason: WriteRejected) -> Self {
        Self { at, reason }
    }

    /// The occurrence the change addressed.
    #[must_use]
    pub const fn at(&self) -> &PropertyOccurrence {
        &self.at
    }

    /// Why the target refused it.
    #[must_use]
    pub const fn reason(&self) -> WriteRejected {
        self.reason
    }
}

/// What applying a transition actually did.
///
/// A partial application is reported, never hidden. A caller that needs all-or-nothing checks
/// [`ApplyReport::is_complete`] before committing its own storage, because this crate owns no
/// transaction and cannot roll one back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// How many changes the target took.
    applied: u32,
    /// The ones it refused, in occurrence order.
    rejected: Vec<RejectedChange>,
}

impl ApplyReport {
    /// A report of nothing applied and nothing refused.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            applied: 0,
            rejected: Vec::new(),
        }
    }

    /// Record that the target took a change.
    pub const fn note_applied(&mut self) {
        self.applied = self.applied.saturating_add(1);
    }

    /// Record that the target refused the change to `at`, for `reason`.
    pub fn note_rejected(&mut self, at: PropertyOccurrence, reason: WriteRejected) {
        self.rejected.push(RejectedChange::new(at, reason));
    }

    /// How many changes the target took.
    #[must_use]
    pub const fn applied(&self) -> u32 {
        self.applied
    }

    /// The changes the target refused.
    #[must_use]
    pub fn rejected(&self) -> &[RejectedChange] {
        &self.rejected
    }

    /// Whether every change was written.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.rejected.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use ical_core::{ProposedChange, RawText};

    use super::{
        ApplyReport, FieldRule, Transition, TransitionReason, WriteRejected, field_rule,
        is_time_property,
    };
    use crate::internal::itip::state::PropertyOccurrence;

    fn line(text: &[u8]) -> ProposedChange {
        ProposedChange::Replace(RawText::from_bytes(text))
    }

    /// A map keyed on the occurrence, so one line cannot carry two conflicting descriptions.
    #[test]
    fn one_occurrence_carries_one_change_and_the_later_one_wins() {
        let mut transition = Transition::new(TransitionReason::ParticipationChanged);
        assert!(transition.is_empty());

        let at = PropertyOccurrence::named(b"ATTENDEE", 1);
        assert_eq!(
            transition.record(at.clone(), line(b"ATTENDEE:mailto:bo@x")),
            None
        );
        assert_eq!(
            transition.record(at.clone(), line(b"ATTENDEE:mailto:cy@x")),
            Some(line(b"ATTENDEE:mailto:bo@x"))
        );
        assert_eq!(transition.len(), 1);
        assert_eq!(transition.change(&at), Some(&line(b"ATTENDEE:mailto:cy@x")));
        assert_eq!(transition.reason(), TransitionReason::ParticipationChanged);
    }

    /// Changes come back in occurrence order, whichever side of a diff found them.
    #[test]
    fn changes_iterate_in_occurrence_order() {
        let mut transition = Transition::new(TransitionReason::Updated);
        transition.record(
            PropertyOccurrence::named(b"SUMMARY", 0),
            ProposedChange::Remove,
        );
        transition.record(
            PropertyOccurrence::named(b"ATTENDEE", 1),
            ProposedChange::Remove,
        );
        transition.record(
            PropertyOccurrence::named(b"ATTENDEE", 0),
            ProposedChange::Remove,
        );

        let seen: alloc::vec::Vec<(&[u8], usize)> = transition
            .changes()
            .map(|(at, _)| (at.name(), at.index()))
            .collect();
        assert_eq!(
            seen,
            alloc::vec![
                (&b"ATTENDEE"[..], 0),
                (&b"ATTENDEE"[..], 1),
                (&b"SUMMARY"[..], 0)
            ]
        );
        assert_eq!(transition.changes().len(), 3);
    }

    /// The default is the closed one, and the exceptions are named rather than inferred.
    #[test]
    fn an_unknown_property_belongs_to_the_organizer() {
        assert_eq!(field_rule(b"X-VENDOR-THING"), FieldRule::OrganizerOnly);
        assert_eq!(field_rule(b"SUMMARY"), FieldRule::OrganizerOnly);
        assert_eq!(field_rule(b"DTSTART"), FieldRule::OrganizerOnly);
        assert_eq!(field_rule(b"attendee"), FieldRule::AttendeeOwn);
        assert_eq!(field_rule(b"REQUEST-STATUS"), FieldRule::EitherParty);
    }

    /// The two properties an attendee may echo and may not write: who runs the meeting, and
    /// which revision of it this is.
    #[test]
    fn the_organizer_line_and_the_revision_are_not_an_attendees_to_change() {
        assert_eq!(field_rule(b"ORGANIZER"), FieldRule::OrganizerOnly);
        assert_eq!(field_rule(b"organizer"), FieldRule::OrganizerOnly);
        assert_eq!(field_rule(b"SEQUENCE"), FieldRule::OrganizerOnly);
        assert_eq!(
            field_rule(b"DTSTAMP"),
            FieldRule::EitherParty,
            "a reply states when it was written, and section 2.1.5 orders replies by it"
        );
    }

    /// The list an attendee may never write, which is the first attack `SECURITY.md` names.
    #[test]
    fn the_properties_that_move_a_meeting_are_named() {
        assert!(is_time_property(b"DTSTART"));
        assert!(is_time_property(b"rrule"));
        assert!(is_time_property(b"DURATION"));
        assert!(!is_time_property(b"SUMMARY"));
        assert!(!is_time_property(b"X-DTSTART"));
    }

    /// A partial application is a reported outcome rather than a hidden one.
    #[test]
    fn a_report_says_what_was_written_and_what_was_not() {
        let mut report = ApplyReport::new();
        assert!(report.is_complete() && report.applied() == 0);
        report.note_applied();
        report.note_rejected(
            PropertyOccurrence::named(b"DTSTART", 0),
            WriteRejected::ReadOnly,
        );
        assert_eq!(report.applied(), 1);
        assert!(!report.is_complete());
        assert_eq!(report.rejected().len(), 1);
        assert_eq!(report.rejected()[0].reason(), WriteRejected::ReadOnly);
        assert_eq!(report.rejected()[0].at().name(), b"DTSTART");
        assert_eq!(ApplyReport::default(), ApplyReport::new());
    }
}
