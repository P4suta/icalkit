// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The eight methods of RFC 5546, and who is entitled to send one.
//!
//! Specification: RFC 5546 section 1.4 for the vocabulary, section 3 for what each one means
//! per component type.
//!
//! `METHOD` is not a switch. Each of the eight has its own required properties, its own
//! permitted sender, and its own legal prior states, stated per component type across pages of
//! section 3. This module carries the identity and the sender rule; [`crate::internal::itip::table`] carries
//! the property constraints as transcribed data, so that a reviewer checks a table against the
//! specification rather than reading control flow.

use ical_core::ComponentKind;

/// One of the eight scheduling methods RFC 5546 section 1.4 defines.
///
/// A closed enum, unlike [`ical_core::PropertyId`], because RFC 5546 closes this set: a
/// `METHOD` value outside it is a message whose semantics nothing here knows, which is a
/// refusal rather than an extension point. `#[non_exhaustive]` all the same, because a later
/// specification may define a ninth and adding it must not be a major version here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Method {
    /// Post notification of a component. RFC 5546 section 3.2.1.
    Publish,
    /// Invite attendees, or update or reschedule an existing invitation. Section 3.2.2.
    Request,
    /// Answer an invitation. Section 3.2.3.
    Reply,
    /// Add instances to an existing component. Section 3.2.4.
    Add,
    /// Cancel a component or some of its instances. Section 3.2.5.
    Cancel,
    /// Ask the organizer to resend the latest version. Section 3.2.6.
    Refresh,
    /// Counter a request with an alternative proposal. Section 3.2.7.
    Counter,
    /// Decline a counter proposal. Section 3.2.8.
    DeclineCounter,
}

impl Method {
    /// Every method, in the order RFC 5546 section 3.2's summary table writes them.
    pub const ALL: [Self; 8] = [
        Self::Publish,
        Self::Request,
        Self::Reply,
        Self::Add,
        Self::Cancel,
        Self::Refresh,
        Self::Counter,
        Self::DeclineCounter,
    ];

    /// The method `value` names, or `None` for a value RFC 5546 does not define.
    ///
    /// Compared without case, as RFC 5545 section 3.1 compares every property value that is
    /// drawn from an enumerated set. `None` is the present-and-unusable state
    /// [`DiagnosticCode::SchedulingMethodUnknown`] reports, and is a different fact from a
    /// `METHOD` that is absent — an `.ics` with no `METHOD` is an ordinary calendar.
    ///
    /// [`DiagnosticCode::SchedulingMethodUnknown`]: ical_core::DiagnosticCode::SchedulingMethodUnknown
    #[must_use]
    pub fn read(value: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.as_bytes().eq_ignore_ascii_case(value))
    }

    /// The value a `METHOD` property spells this method as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Publish => b"PUBLISH",
            Self::Request => b"REQUEST",
            Self::Reply => b"REPLY",
            Self::Add => b"ADD",
            Self::Cancel => b"CANCEL",
            Self::Refresh => b"REFRESH",
            Self::Counter => b"COUNTER",
            Self::DeclineCounter => b"DECLINECOUNTER",
        }
    }

    /// Who RFC 5546 section 3 permits to send this method.
    ///
    /// Read from each section's prose rather than from its constraint table, which states
    /// property presence and never says who may send. The rows are the same for `VEVENT`,
    /// `VTODO` and `VJOURNAL`; `VFREEBUSY` defines only three methods and agrees with them.
    #[must_use]
    pub const fn sender(self) -> SenderRule {
        match self {
            Self::Publish | Self::Request | Self::Add | Self::Cancel | Self::DeclineCounter => {
                SenderRule::Organizer
            },
            Self::Reply | Self::Refresh | Self::Counter => SenderRule::Attendee,
        }
    }

    /// Whether the organizer is the party who authors this method.
    ///
    /// The same fact [`Method::sender`] states, as the predicate a call site most often wants.
    #[must_use]
    pub const fn is_organizer_authored(self) -> bool {
        matches!(self.sender(), SenderRule::Organizer)
    }

    /// Whether RFC 5546 section 3 defines this method for `kind`.
    ///
    /// `VJOURNAL` has three methods and `VFREEBUSY` has three others; asking for the rest is
    /// asking for semantics the specification does not state, which is a refusal.
    #[must_use]
    pub fn is_defined_for(self, kind: ComponentKind) -> bool {
        crate::internal::itip::table::MethodRule::lookup(self, kind).is_some()
    }
}

/// Which side of a scheduling exchange RFC 5546 permits to send a method.
///
/// Two values rather than a party list, because the specification names roles and the roles
/// are what a component states. Whether a particular actor *is* that role is
/// [`ActorRole`]'s question, and the agent cases are why the two are separate types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SenderRule {
    /// Only the organizer of the component, or an agent sending on their behalf.
    Organizer,
    /// Only an attendee of the component, or an agent sending on theirs.
    Attendee,
}

/// What one actor is to the component a message is being judged against.
///
/// `SENT-BY` is answered separately from identity, so that "the assistant sent this" never
/// becomes "the organizer sent this": an agent satisfies the same [`SenderRule`] its principal
/// does and is a distinct value, so a caller that wants to show a person who actually sent a
/// message still can.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ActorRole {
    /// The address the component's `ORGANIZER` names.
    Organizer,
    /// An address named by an `ORGANIZER`'s `SENT-BY` parameter.
    OrganizerAgent,
    /// An address the component's `ATTENDEE` list names.
    Attendee,
    /// An address named by an `ATTENDEE`'s `SENT-BY` parameter.
    AttendeeAgent,
    /// An attendee reached only through another attendee's `DELEGATED-TO`.
    ///
    /// RFC 5546 section 2.1.2 lets an attendee delegate, and the delegate is on the list by the
    /// time a well-formed delegation has been applied. This value is for the window before
    /// that: a reply from a delegate the organizer's copy names only as a `DELEGATED-TO` value.
    Delegate,
}

impl ActorRole {
    /// Whether this actor satisfies `rule`.
    ///
    /// An agent satisfies its principal's rule, which is the whole reason `SENT-BY` exists. A
    /// delegate satisfies the attendee rule, because a delegation that has been accepted makes
    /// the delegate an attendee and RFC 5546 section 2.1.2's reply comes from them.
    #[must_use]
    pub const fn satisfies(self, rule: SenderRule) -> bool {
        match rule {
            SenderRule::Organizer => matches!(self, Self::Organizer | Self::OrganizerAgent),
            SenderRule::Attendee => {
                matches!(self, Self::Attendee | Self::AttendeeAgent | Self::Delegate)
            },
        }
    }

    /// Whether this actor acts for the organizer rather than for an attendee.
    #[must_use]
    pub const fn is_organizer_side(self) -> bool {
        matches!(self, Self::Organizer | Self::OrganizerAgent)
    }

    /// Whether this actor is an agent rather than the party itself.
    #[must_use]
    pub const fn is_agent(self) -> bool {
        matches!(self, Self::OrganizerAgent | Self::AttendeeAgent)
    }
}

#[cfg(test)]
mod tests {
    use ical_core::ComponentKind;

    use super::{ActorRole, Method, SenderRule};

    #[test]
    fn a_method_is_read_without_case_and_an_undefined_one_is_none() {
        assert_eq!(Method::read(b"reply"), Some(Method::Reply));
        assert_eq!(
            Method::read(b"DECLINECOUNTER"),
            Some(Method::DeclineCounter)
        );
        assert_eq!(Method::read(b"INVITE"), None);
        assert_eq!(Method::read(b""), None);
    }

    /// The two directions of RFC 5546's sender rule, and the reason it is a rule and not a
    /// convention: an attendee sending a `CANCEL` cancels somebody else's meeting.
    #[test]
    fn the_sender_rule_separates_the_two_sides_and_admits_their_agents() {
        assert!(Method::Cancel.is_organizer_authored());
        assert!(!Method::Reply.is_organizer_authored());
        assert!(ActorRole::OrganizerAgent.satisfies(SenderRule::Organizer));
        assert!(!ActorRole::Attendee.satisfies(SenderRule::Organizer));
        assert!(ActorRole::Delegate.satisfies(SenderRule::Attendee));
        assert!(!ActorRole::Organizer.satisfies(SenderRule::Attendee));
        assert!(ActorRole::AttendeeAgent.is_agent());
        assert!(!ActorRole::Delegate.is_agent());
        assert!(ActorRole::Organizer.is_organizer_side());
    }

    /// `VJOURNAL` has three methods and `VFREEBUSY` three others, so "defined" is a question
    /// about the pair rather than about the method.
    #[test]
    fn a_method_is_defined_per_component_kind_rather_than_on_its_own() {
        assert!(Method::Reply.is_defined_for(ComponentKind::Event));
        assert!(!Method::Reply.is_defined_for(ComponentKind::Journal));
        assert!(Method::Cancel.is_defined_for(ComponentKind::Journal));
        assert!(!Method::Cancel.is_defined_for(ComponentKind::FreeBusy));
        assert!(!Method::Publish.is_defined_for(ComponentKind::Alarm));
    }
}
