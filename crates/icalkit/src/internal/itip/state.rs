// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The state a transition is judged against, and how one property occurrence is addressed.
//!
//! Two things live here. [`PropertyOccurrence`] is the address a scheduling transition uses —
//! the second `ATTENDEE`, not `ATTENDEE` — and [`ScheduledComponent`] is how a caller offers
//! the state to judge against, whether that state is an [`crate::internal::core::Component`] or a database
//! row a server never turns into one.

use core::fmt::Debug;

use crate::internal::core::{ComponentKind, Instant, PropertyId};

use crate::internal::itip::identity::{InstanceRef, SequenceRead};
use crate::internal::itip::party::{Attendee, Party};

/// One property *occurrence*: an identity and which of the properties carrying it.
///
/// `ical-core`'s [`PropertyId`] identifies a name, which is what `properties_named` and `get`
/// look up with and what makes two `ATTENDEE` lines deliberately share one key. A scheduling
/// transition needs the other thing — a message changes one attendee among many — so the
/// index lives here rather than widening a type every lookup below this crate uses.
///
/// The index counts properties of that name **directly inside one component**, from zero, in
/// document order. Nested components are not counted, matching every reading and writing path
/// in `ical-core`: a `DTSTART` inside a `VALARM` belongs to the alarm, and counting it would
/// make one occurrence number address two different lines depending on what is nested.
///
/// `Ord` sorts by name and then by index, so a [`crate::internal::itip::Transition`] keyed on this iterates in
/// an order that does not depend on which side of a diff a change was found on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropertyOccurrence {
    /// The property name's identity.
    id: PropertyId,
    /// Which of the properties carrying it, from zero.
    index: usize,
}

impl PropertyOccurrence {
    /// The `index`th occurrence of `id`.
    #[must_use]
    pub const fn new(id: PropertyId, index: usize) -> Self {
        Self { id, index }
    }

    /// The `index`th occurrence of the property `name` spells.
    ///
    /// The name is normalized the way [`PropertyId::from_name`] normalizes one, so an
    /// occurrence read from a message and one read from a local copy are one key even when the
    /// two producers disagreed about case.
    #[must_use]
    pub fn named(name: &[u8], index: usize) -> Self {
        Self::new(PropertyId::from_name(name), index)
    }

    /// The property name's identity.
    #[must_use]
    pub const fn id(&self) -> &PropertyId {
        &self.id
    }

    /// Which of the properties carrying that name, from zero.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// The normalized name's octets.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        self.id.as_bytes()
    }
}

/// One component, as much of it as scheduling has to read.
///
/// This is how ADR-0005's `current: &Component` is spelled, and it is a trait rather than a
/// concrete type for one reason worth the cost: a CalDAV server whose current state is a
/// database row must not have to build an `crate::internal::core::Component` in order to answer "who may
/// change this". `ical-itip` ships an implementation for [`crate::internal::core::Component`], so a caller
/// that does hold one passes `&component` and never names the trait.
///
/// Deliberately object-safe — index accessors rather than iterators, no generics, no
/// associated types — matching the posture `crate::internal::tz::ZoneSource` already takes, and costing
/// one vtable rather than a monomorphized copy per state carrier on a `thumbv7em` target.
///
/// # What an implementation owes
///
/// - Every octet slice handed back is a **value**: RFC 6868's caret encoding is resolved
///   before it reaches [`Party`] or [`Attendee`], per [`crate::internal::itip::party`]'s own contract.
/// - [`ScheduledComponent::property_line`] hands back the whole content line — name,
///   parameters and value together, unfolded, with no terminator — because that is the unit
///   [`crate::internal::core::ProposedChange::Replace`] takes and the unit a diff compares.
/// - Properties are reported in document order, and only those directly inside this
///   component. [`ScheduledComponent::child`] reaches the nested ones.
/// - Nothing here allocates per call on the hot paths a diff walks, because a diff walks every
///   property of both sides.
pub trait ScheduledComponent: Debug {
    /// What kind of component this is, `None` for a name RFC 5545 does not define.
    fn component_kind(&self) -> Option<ComponentKind>;

    /// The `METHOD` value, absent when there is no such property.
    ///
    /// Raw rather than a [`crate::internal::itip::Method`], so that "absent" and "present and not a method
    /// RFC 5546 defines" stay two answers. [`crate::internal::itip::Method::read`] makes the second one.
    fn method(&self) -> Option<&[u8]>;

    /// The `UID` value, absent when there is no such property.
    fn uid(&self) -> Option<&[u8]>;

    /// What the `SEQUENCE` property was.
    fn sequence(&self) -> SequenceRead;

    /// The `DTSTAMP`, absent when there is none or it did not read.
    fn dtstamp(&self) -> Option<Instant>;

    /// The `RECURRENCE-ID`, absent when this is about a whole series.
    ///
    /// The [`crate::internal::itip::FoldSide`] of what is returned is whatever the implementation could
    /// resolve, which for one that holds no zone is [`crate::internal::itip::FoldSide::Unresolved`].
    fn recurrence_id(&self) -> Option<InstanceRef>;

    /// The `ORGANIZER`, absent when there is no such property.
    fn organizer(&self) -> Option<Party<'_>>;

    /// How many `ATTENDEE` properties this component carries.
    fn attendee_count(&self) -> usize;

    /// The `index`th `ATTENDEE`, in document order.
    fn attendee(&self, index: usize) -> Option<Attendee<'_>>;

    /// When the `index`th `ATTENDEE` last answered, as this state records it.
    ///
    /// RFC 5546 section 2.1.5 orders two messages at one `SEQUENCE` by `DTSTAMP`, and two
    /// replies from one attendee are exactly that pair: the same revision, answered twice. The
    /// component's own `DTSTAMP` cannot order them — it is the organizer's, it is older than
    /// both, and reading a reply against it would refuse one attendee's answer because a
    /// *different* attendee answered later. So the fact has to sit on the line it is about.
    ///
    /// The default is `None`, which says the state records nothing and is not a claim that the
    /// attendee never answered. A gate handed `None` cannot order two answers and admits the
    /// second, which is the direction that keeps a legitimate change of mind working; a store
    /// that does record the time gets the refusal instead. [`crate::internal::itip::ANSWERED_AT`] is how the
    /// shipped bridge spells it, and a store keeping its own column answers from that.
    fn attendee_answered_at(&self, index: usize) -> Option<Instant> {
        let _ = index;
        None
    }

    /// Which occurrence the `index`th `ATTENDEE` is, as a property occurrence.
    ///
    /// The two indexes agree for a component whose only repeated name is `ATTENDEE` and would
    /// not be worth stating if that were guaranteed. It is not: an implementation is free to
    /// order its attendee list however it likes, and a `REPLY` has to name the line it changed
    /// in the vocabulary a transition is keyed on.
    fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence>;

    /// How many properties sit directly inside this component.
    fn property_count(&self) -> usize;

    /// The name of the `index`th property, in document order.
    fn property_name(&self, index: usize) -> Option<&[u8]>;

    /// The whole content line of the `index`th property, unfolded and unterminated.
    fn property_line(&self, index: usize) -> Option<&[u8]>;

    /// How many components sit directly inside this one.
    fn child_count(&self) -> usize;

    /// The `index`th nested component, in document order.
    fn child(&self, index: usize) -> Option<&dyn ScheduledComponent>;
}

/// The first value carried by a directly contained property named `name`.
///
/// The separator is the first colon outside a quoted parameter value, matching RFC 5545's
/// content-line grammar. This small helper keeps method-specific gates from each inventing a
/// less complete split.
#[must_use]
pub fn property_value<'a>(component: &'a dyn ScheduledComponent, name: &[u8]) -> Option<&'a [u8]> {
    for index in 0..component.property_count() {
        if !component
            .property_name(index)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let line = component.property_line(index)?;
        let mut quoted = false;
        for (at, octet) in line.iter().enumerate() {
            match *octet {
                b'"' => quoted = !quoted,
                b':' if !quoted => return line.get(at.saturating_add(1)..),
                _ => {},
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::internal::core::PropertyId;

    use super::PropertyOccurrence;

    /// The address a transition is keyed on: a name and which line of it.
    #[test]
    fn an_occurrence_is_a_name_and_an_index_and_sorts_by_both() {
        let first = PropertyOccurrence::named(b"attendee", 0);
        let second = PropertyOccurrence::new(PropertyId::ATTENDEE, 1);
        assert_eq!(first.id(), &PropertyId::ATTENDEE);
        assert_eq!(first.name(), b"ATTENDEE");
        assert_eq!(first.index(), 0);
        assert_ne!(first, second);
        assert!(first < second);
        assert!(
            PropertyOccurrence::named(b"ATTENDEE", 9) < PropertyOccurrence::named(b"DTSTART", 0)
        );
        assert_eq!(first, PropertyOccurrence::named(b"ATTENDEE", 0));
    }
}
