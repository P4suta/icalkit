// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5546 section 3's constraint tables, transcribed as data.
//!
//! Specification: RFC 5546 section 3.2 (`VEVENT`, eight methods), section 3.3 (`VFREEBUSY`,
//! three), section 3.4 (`VTODO`, eight) and section 3.5 (`VJOURNAL`, three). Twenty-two
//! tables, each stating for one method applied to one component type how often every property
//! and every component may appear.
//!
//! # How to check this file
//!
//! The rows below were extracted mechanically from the published text of RFC 5546, not typed
//! out, because a transcription error here is invisible until it accepts a message it should
//! have refused. Each `static` names the section its table is printed in, and the rows are in
//! the order that table prints them, so checking one is reading the two side by side.
//!
//! What is **not** from a table, and is the part a reviewer has to read prose for:
//!
//! - [`MethodRule::sender`] — RFC 5546 section 3's per-method prose names the permitted
//!   sender. No constraint table states it. It is carried on [`Method::sender`].
//! - [`MethodRule::prior_states`] — whether a method may act when the caller holds no such
//!   component. The specification states this in prose per method and tabulates it nowhere;
//!   these rows are a reading of that prose and are the likeliest place this file is wrong.
//!
//! # What a row means, and what it does not
//!
//! `IANA-PROPERTY` and `X-PROPERTY` are classes rather than names, and the same holds for
//! `IANA-COMPONENT` and `X-COMPONENT`. [`MethodRule::presence_of`] resolves a name it has no
//! row for onto the matching class row, so a vendor property arriving in a `REPLY` is answered
//! by the row RFC 5546 wrote for it rather than by a default nobody chose.
//!
//! A [`Presence`] is a statement about a well-formed message. It is not by itself an
//! authorization answer: a forbidden property in a message a caller is only inspecting is a
//! fact about the file, and the same property in a message an attendee sent is a denial. Which
//! of the two it is belongs to [`crate::internal::itip::authorize`].

use ical_core::ComponentKind;

use crate::internal::itip::method::{Method, SenderRule};

/// How often RFC 5546 section 3 permits one name inside one component of one message.
///
/// The five values its constraint tables print, and no sixth. [`Presence::Never`] is the `0`
/// row and is the one carrying the security weight: it is where the specification says a
/// property has no business in this message at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Presence {
    /// `1` — exactly one.
    ExactlyOnce,
    /// `1+` — at least one.
    OnceOrMore,
    /// `0 or 1` — at most one.
    AtMostOnce,
    /// `0+` — any number, none included.
    AnyNumber,
    /// `0` — none. The property or component may not appear at all.
    Never,
}

impl Presence {
    /// Whether `count` occurrences satisfy this row.
    #[must_use]
    pub const fn admits(self, count: usize) -> bool {
        match self {
            Self::ExactlyOnce => count == 1,
            Self::OnceOrMore => count >= 1,
            Self::AtMostOnce => count <= 1,
            Self::AnyNumber => true,
            Self::Never => count == 0,
        }
    }

    /// Whether this row requires at least one occurrence.
    #[must_use]
    pub const fn is_required(self) -> bool {
        matches!(self, Self::ExactlyOnce | Self::OnceOrMore)
    }

    /// Whether this row forbids every occurrence.
    #[must_use]
    pub const fn is_forbidden(self) -> bool {
        matches!(self, Self::Never)
    }
}

/// One row of a constraint table: a name and how often it may appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rule {
    /// The name the table prints, upper case as the specification writes it.
    name: &'static [u8],
    /// How often it may appear.
    presence: Presence,
}

impl Rule {
    /// A row naming `name` with `presence`.
    const fn new(name: &'static [u8], presence: Presence) -> Self {
        Self { name, presence }
    }

    /// The name this row is about.
    #[must_use]
    pub const fn name(self) -> &'static [u8] {
        self.name
    }

    /// How often it may appear.
    #[must_use]
    pub const fn presence(self) -> Presence {
        self.presence
    }

    /// Whether this row is about `name`, compared as RFC 5545 section 3.1 compares one.
    #[must_use]
    pub fn is_named(self, name: &[u8]) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// What the caller already holds about the identity a message names.
///
/// Two values, because that is the whole of what RFC 5546's prose distinguishes: either the
/// recipient has a component under this `UID` or it does not. A `REPLY` to an invitation nobody
/// sent, and a `CANCEL` of a meeting nobody has, are the shapes this refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PriorState {
    /// The caller holds no component under this identity.
    Absent,
    /// The caller holds one.
    Present,
}

/// Everything RFC 5546 section 3 states about one method applied to one component type.
///
/// The unit a reviewer diffs against the specification. [`MethodRule::section`] names the
/// subsection its rows were taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodRule {
    /// The method these rows are about.
    method: Method,
    /// The component type they are about.
    kind: ComponentKind,
    /// The RFC 5546 subsection the table is printed in, as `"3.2.3"`.
    section: &'static str,
    /// The properties of the addressed component, in table order.
    properties: &'static [Rule],
    /// The components nested inside the addressed component, in table order.
    subcomponents: &'static [Rule],
    /// The components at the top level of the message, in table order.
    components: &'static [Rule],
    /// Who may send it. From prose; no table states this.
    sender: SenderRule,
    /// The prior states it may act on. From prose; no table states this.
    prior_states: &'static [PriorState],
}

impl MethodRule {
    /// The rule for `method` applied to `kind`, or `None` where RFC 5546 defines no table.
    ///
    /// `None` is a refusal rather than a permission: a `REPLY` to a `VJOURNAL` has no stated
    /// semantics, and inventing them is how one implementation accepts what every other one
    /// rejects.
    #[must_use]
    pub fn lookup(method: Method, kind: ComponentKind) -> Option<Self> {
        RULES
            .iter()
            .find(|rule| rule.method == method && rule.kind == kind)
            .copied()
    }

    /// The method these rows are about.
    #[must_use]
    pub const fn method(self) -> Method {
        self.method
    }

    /// The component type they are about.
    #[must_use]
    pub const fn kind(self) -> ComponentKind {
        self.kind
    }

    /// The RFC 5546 subsection the constraint table is printed in.
    #[must_use]
    pub const fn section(self) -> &'static str {
        self.section
    }

    /// The property rows, in the order the table prints them.
    #[must_use]
    pub const fn properties(self) -> &'static [Rule] {
        self.properties
    }

    /// The rows for components nested inside the addressed one, in table order.
    #[must_use]
    pub const fn subcomponents(self) -> &'static [Rule] {
        self.subcomponents
    }

    /// The rows for components at the top level of the message, in table order.
    #[must_use]
    pub const fn components(self) -> &'static [Rule] {
        self.components
    }

    /// Who RFC 5546 section 3's prose permits to send this method.
    #[must_use]
    pub const fn sender(self) -> SenderRule {
        self.sender
    }

    /// The prior states RFC 5546 section 3's prose permits this method to act on.
    #[must_use]
    pub const fn prior_states(self) -> &'static [PriorState] {
        self.prior_states
    }

    /// Whether this method may act when the caller's state is `prior`.
    #[must_use]
    pub fn permits_prior(self, prior: PriorState) -> bool {
        self.prior_states.contains(&prior)
    }

    /// How often `name` may appear as a property of the addressed component.
    ///
    /// A name with no row of its own falls onto the class row RFC 5546 wrote for it —
    /// `X-PROPERTY` for a name beginning `X-`, `IANA-PROPERTY` otherwise. A table with no such
    /// row answers [`Presence::Never`], which is the closed direction: it refuses a name the
    /// specification did not admit rather than admitting one it never mentioned.
    #[must_use]
    pub fn presence_of(self, name: &[u8]) -> Presence {
        Self::resolve(self.properties, name, b"X-PROPERTY", b"IANA-PROPERTY")
    }

    /// How often `name` may appear as a component at the top level of the message.
    #[must_use]
    pub fn component_presence(self, name: &[u8]) -> Presence {
        Self::resolve(self.components, name, b"X-COMPONENT", b"IANA-COMPONENT")
    }

    /// How often `name` may appear as a component nested inside the addressed one.
    ///
    /// The tables list only `VALARM` here, so every other nested name falls through to
    /// [`Presence::Never`]: RFC 5546 section 3.1.3 restricts what an alarm may carry and says
    /// nothing at all about a second kind of nested component.
    #[must_use]
    pub fn subcomponent_presence(self, name: &[u8]) -> Presence {
        Self::resolve(self.subcomponents, name, b"X-COMPONENT", b"IANA-COMPONENT")
    }

    /// The presence `rows` state for `name`, falling onto the vendor or IANA class row.
    fn resolve(
        rows: &'static [Rule],
        name: &[u8],
        vendor: &'static [u8],
        registered: &'static [u8],
    ) -> Presence {
        if let Some(row) = rows.iter().find(|row| row.is_named(name)) {
            return row.presence;
        }
        let vendor_named = name
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"X-"));
        let class = if vendor_named { vendor } else { registered };
        rows.iter()
            .find(|row| row.is_named(class))
            .map_or(Presence::Never, |row| row.presence)
    }
}

/// The `PROPERTIES` rows of RFC 5546 section 3.2.1's constraint table.
static PUBLISH_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AtMostOnce),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.1's constraint table.
static PUBLISH_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.1's constraint table.
static PUBLISH_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::OnceOrMore),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.2's constraint table.
static REQUEST_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.2's constraint table.
static REQUEST_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.2's constraint table.
static REQUEST_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.3's constraint table.
static REPLY_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.3's constraint table.
static REPLY_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.3's constraint table.
static REPLY_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AtMostOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.4's constraint table.
static ADD_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"EXDATE", Presence::Never),
    Rule::new(b"RECURRENCE-ID", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RDATE", Presence::Never),
    Rule::new(b"RRULE", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.4's constraint table.
static ADD_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.4's constraint table.
static ADD_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.5's constraint table.
static CANCEL_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::AnyNumber),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.5's constraint table.
static CANCEL_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.5's constraint table.
static CANCEL_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.6's constraint table.
static REFRESH_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTACH", Presence::Never),
    Rule::new(b"CATEGORIES", Presence::Never),
    Rule::new(b"CLASS", Presence::Never),
    Rule::new(b"CONTACT", Presence::Never),
    Rule::new(b"CREATED", Presence::Never),
    Rule::new(b"DESCRIPTION", Presence::Never),
    Rule::new(b"DTEND", Presence::Never),
    Rule::new(b"DTSTART", Presence::Never),
    Rule::new(b"DURATION", Presence::Never),
    Rule::new(b"EXDATE", Presence::Never),
    Rule::new(b"GEO", Presence::Never),
    Rule::new(b"LAST-MODIFIED", Presence::Never),
    Rule::new(b"LOCATION", Presence::Never),
    Rule::new(b"PRIORITY", Presence::Never),
    Rule::new(b"RDATE", Presence::Never),
    Rule::new(b"RELATED-TO", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RESOURCES", Presence::Never),
    Rule::new(b"RRULE", Presence::Never),
    Rule::new(b"SEQUENCE", Presence::Never),
    Rule::new(b"STATUS", Presence::Never),
    Rule::new(b"SUMMARY", Presence::Never),
    Rule::new(b"TRANSP", Presence::Never),
    Rule::new(b"URL", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.6's constraint table.
static REFRESH_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.6's constraint table.
static REFRESH_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.7's constraint table.
static COUNTER_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.7's constraint table.
static COUNTER_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.7's constraint table.
static COUNTER_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.2.8's constraint table.
static DECLINECOUNTER_EVENT_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DTEND", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"TRANSP", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.2.8's constraint table.
static DECLINECOUNTER_EVENT_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.2.8's constraint table.
static DECLINECOUNTER_EVENT_COMPONENTS: &[Rule] = &[
    Rule::new(b"VEVENT", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.3.1's constraint table.
static PUBLISH_FREEBUSY_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"DTEND", Presence::ExactlyOnce),
    Rule::new(b"FREEBUSY", Presence::AnyNumber),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"DURATION", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.3.1's constraint table.
static PUBLISH_FREEBUSY_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.3.1's constraint table.
static PUBLISH_FREEBUSY_COMPONENTS: &[Rule] = &[
    Rule::new(b"VFREEBUSY", Presence::OnceOrMore),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTIMEZONE", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.3.2's constraint table.
static REQUEST_FREEBUSY_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTEND", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"FREEBUSY", Presence::Never),
    Rule::new(b"DURATION", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"URL", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.3.2's constraint table.
static REQUEST_FREEBUSY_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.3.2's constraint table.
static REQUEST_FREEBUSY_COMPONENTS: &[Rule] = &[
    Rule::new(b"VFREEBUSY", Presence::ExactlyOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTIMEZONE", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.3.3's constraint table.
static REPLY_FREEBUSY_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTEND", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"FREEBUSY", Presence::AnyNumber),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AtMostOnce),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"DURATION", Presence::Never),
    Rule::new(b"SEQUENCE", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.3.3's constraint table.
static REPLY_FREEBUSY_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.3.3's constraint table.
static REPLY_FREEBUSY_COMPONENTS: &[Rule] = &[
    Rule::new(b"VFREEBUSY", Presence::ExactlyOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VTIMEZONE", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.1's constraint table.
static PUBLISH_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"PRIORITY", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.1's constraint table.
static PUBLISH_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.1's constraint table.
static PUBLISH_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.2's constraint table.
static REQUEST_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"PRIORITY", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.2's constraint table.
static REQUEST_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.2's constraint table.
static REQUEST_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.3's constraint table.
static REPLY_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.3's constraint table.
static REPLY_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.3's constraint table.
static REPLY_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AtMostOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.4's constraint table.
static ADD_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"PRIORITY", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"EXDATE", Presence::Never),
    Rule::new(b"RECURRENCE-ID", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RDATE", Presence::Never),
    Rule::new(b"RRULE", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.4's constraint table.
static ADD_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.4's constraint table.
static ADD_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VJOURNAL", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.5's constraint table.
static CANCEL_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::AnyNumber),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.5's constraint table.
static CANCEL_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.5's constraint table.
static CANCEL_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AtMostOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.6's constraint table.
static REFRESH_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTACH", Presence::Never),
    Rule::new(b"CATEGORIES", Presence::Never),
    Rule::new(b"CLASS", Presence::Never),
    Rule::new(b"COMMENT", Presence::Never),
    Rule::new(b"COMPLETED", Presence::Never),
    Rule::new(b"CONTACT", Presence::Never),
    Rule::new(b"CREATED", Presence::Never),
    Rule::new(b"DESCRIPTION", Presence::Never),
    Rule::new(b"DTSTART", Presence::Never),
    Rule::new(b"DUE", Presence::Never),
    Rule::new(b"DURATION", Presence::Never),
    Rule::new(b"EXDATE", Presence::Never),
    Rule::new(b"GEO", Presence::Never),
    Rule::new(b"LAST-MODIFIED", Presence::Never),
    Rule::new(b"LOCATION", Presence::Never),
    Rule::new(b"ORGANIZER", Presence::Never),
    Rule::new(b"PERCENT-COMPLETE", Presence::Never),
    Rule::new(b"PRIORITY", Presence::Never),
    Rule::new(b"RDATE", Presence::Never),
    Rule::new(b"RELATED-TO", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RESOURCES", Presence::Never),
    Rule::new(b"RRULE", Presence::Never),
    Rule::new(b"SEQUENCE", Presence::Never),
    Rule::new(b"STATUS", Presence::Never),
    Rule::new(b"URL", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.6's constraint table.
static REFRESH_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.6's constraint table.
static REFRESH_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.7's constraint table.
static COUNTER_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"PRIORITY", Presence::ExactlyOnce),
    Rule::new(b"SUMMARY", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.7's constraint table.
static COUNTER_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.7's constraint table.
static COUNTER_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AtMostOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.4.8's constraint table.
static DECLINECOUNTER_TODO_PROPERTIES: &[Rule] = &[
    Rule::new(b"ATTENDEE", Presence::OnceOrMore),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"COMPLETED", Presence::AtMostOnce),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"DUE", Presence::AtMostOnce),
    Rule::new(b"DURATION", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"GEO", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"LOCATION", Presence::AtMostOnce),
    Rule::new(b"PERCENT-COMPLETE", Presence::AtMostOnce),
    Rule::new(b"PRIORITY", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::AnyNumber),
    Rule::new(b"RESOURCES", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.4.8's constraint table.
static DECLINECOUNTER_TODO_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.4.8's constraint table.
static DECLINECOUNTER_TODO_COMPONENTS: &[Rule] = &[
    Rule::new(b"VTODO", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.5.1's constraint table.
static PUBLISH_JOURNAL_PROPERTIES: &[Rule] = &[
    Rule::new(b"DESCRIPTION", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"SEQUENCE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.5.1's constraint table.
static PUBLISH_JOURNAL_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.5.1's constraint table.
static PUBLISH_JOURNAL_COMPONENTS: &[Rule] = &[
    Rule::new(b"VJOURNAL", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.5.2's constraint table.
static ADD_JOURNAL_PROPERTIES: &[Rule] = &[
    Rule::new(b"DESCRIPTION", Presence::ExactlyOnce),
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"DTSTART", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"EXDATE", Presence::Never),
    Rule::new(b"RECURRENCE-ID", Presence::Never),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
    Rule::new(b"RDATE", Presence::Never),
    Rule::new(b"RRULE", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.5.2's constraint table.
static ADD_JOURNAL_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::AnyNumber)];

/// The `COMPONENTS` rows of RFC 5546 section 3.5.2's constraint table.
static ADD_JOURNAL_COMPONENTS: &[Rule] = &[
    Rule::new(b"VJOURNAL", Presence::ExactlyOnce),
    Rule::new(b"VTIMEZONE", Presence::AtMostOnce),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// The `PROPERTIES` rows of RFC 5546 section 3.5.3's constraint table.
static CANCEL_JOURNAL_PROPERTIES: &[Rule] = &[
    Rule::new(b"DTSTAMP", Presence::ExactlyOnce),
    Rule::new(b"ORGANIZER", Presence::ExactlyOnce),
    Rule::new(b"SEQUENCE", Presence::ExactlyOnce),
    Rule::new(b"UID", Presence::ExactlyOnce),
    Rule::new(b"ATTACH", Presence::AnyNumber),
    Rule::new(b"ATTENDEE", Presence::Never),
    Rule::new(b"CATEGORIES", Presence::AnyNumber),
    Rule::new(b"CLASS", Presence::AtMostOnce),
    Rule::new(b"COMMENT", Presence::AnyNumber),
    Rule::new(b"CONTACT", Presence::AnyNumber),
    Rule::new(b"CREATED", Presence::AtMostOnce),
    Rule::new(b"DESCRIPTION", Presence::AtMostOnce),
    Rule::new(b"DTSTART", Presence::AtMostOnce),
    Rule::new(b"EXDATE", Presence::AnyNumber),
    Rule::new(b"LAST-MODIFIED", Presence::AtMostOnce),
    Rule::new(b"RDATE", Presence::AnyNumber),
    Rule::new(b"RECURRENCE-ID", Presence::AtMostOnce),
    Rule::new(b"RELATED-TO", Presence::AnyNumber),
    Rule::new(b"RRULE", Presence::AtMostOnce),
    Rule::new(b"STATUS", Presence::AtMostOnce),
    Rule::new(b"SUMMARY", Presence::AtMostOnce),
    Rule::new(b"URL", Presence::AtMostOnce),
    Rule::new(b"IANA-PROPERTY", Presence::AnyNumber),
    Rule::new(b"X-PROPERTY", Presence::AnyNumber),
    Rule::new(b"REQUEST-STATUS", Presence::Never),
];

/// The `SUBCOMPONENTS` rows of RFC 5546 section 3.5.3's constraint table.
static CANCEL_JOURNAL_SUBCOMPONENTS: &[Rule] = &[Rule::new(b"VALARM", Presence::Never)];

/// The `COMPONENTS` rows of RFC 5546 section 3.5.3's constraint table.
static CANCEL_JOURNAL_COMPONENTS: &[Rule] = &[
    Rule::new(b"VJOURNAL", Presence::OnceOrMore),
    Rule::new(b"VTIMEZONE", Presence::AnyNumber),
    Rule::new(b"IANA-COMPONENT", Presence::AnyNumber),
    Rule::new(b"X-COMPONENT", Presence::AnyNumber),
    Rule::new(b"VEVENT", Presence::Never),
    Rule::new(b"VFREEBUSY", Presence::Never),
    Rule::new(b"VTODO", Presence::Never),
];

/// Every (method, component kind) pair RFC 5546 section 3 states a constraint table for.
///
/// In the order the specification writes them: section 3.2's eight `VEVENT` tables, section
/// 3.3's three `VFREEBUSY` tables, section 3.4's eight `VTODO` tables, and section 3.5's
/// three `VJOURNAL` tables. A pair absent from this list is a pair the specification does
/// not define, which is a refusal rather than a permission.
pub(crate) static RULES: [MethodRule; 22] = [
    MethodRule {
        method: Method::Publish,
        kind: ComponentKind::Event,
        section: "3.2.1",
        properties: PUBLISH_EVENT_PROPERTIES,
        subcomponents: PUBLISH_EVENT_SUBCOMPONENTS,
        components: PUBLISH_EVENT_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Request,
        kind: ComponentKind::Event,
        section: "3.2.2",
        properties: REQUEST_EVENT_PROPERTIES,
        subcomponents: REQUEST_EVENT_SUBCOMPONENTS,
        components: REQUEST_EVENT_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Reply,
        kind: ComponentKind::Event,
        section: "3.2.3",
        properties: REPLY_EVENT_PROPERTIES,
        subcomponents: REPLY_EVENT_SUBCOMPONENTS,
        components: REPLY_EVENT_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Add,
        kind: ComponentKind::Event,
        section: "3.2.4",
        properties: ADD_EVENT_PROPERTIES,
        subcomponents: ADD_EVENT_SUBCOMPONENTS,
        components: ADD_EVENT_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Cancel,
        kind: ComponentKind::Event,
        section: "3.2.5",
        properties: CANCEL_EVENT_PROPERTIES,
        subcomponents: CANCEL_EVENT_SUBCOMPONENTS,
        components: CANCEL_EVENT_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Refresh,
        kind: ComponentKind::Event,
        section: "3.2.6",
        properties: REFRESH_EVENT_PROPERTIES,
        subcomponents: REFRESH_EVENT_SUBCOMPONENTS,
        components: REFRESH_EVENT_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Counter,
        kind: ComponentKind::Event,
        section: "3.2.7",
        properties: COUNTER_EVENT_PROPERTIES,
        subcomponents: COUNTER_EVENT_SUBCOMPONENTS,
        components: COUNTER_EVENT_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::DeclineCounter,
        kind: ComponentKind::Event,
        section: "3.2.8",
        properties: DECLINECOUNTER_EVENT_PROPERTIES,
        subcomponents: DECLINECOUNTER_EVENT_SUBCOMPONENTS,
        components: DECLINECOUNTER_EVENT_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Publish,
        kind: ComponentKind::FreeBusy,
        section: "3.3.1",
        properties: PUBLISH_FREEBUSY_PROPERTIES,
        subcomponents: PUBLISH_FREEBUSY_SUBCOMPONENTS,
        components: PUBLISH_FREEBUSY_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Request,
        kind: ComponentKind::FreeBusy,
        section: "3.3.2",
        properties: REQUEST_FREEBUSY_PROPERTIES,
        subcomponents: REQUEST_FREEBUSY_SUBCOMPONENTS,
        components: REQUEST_FREEBUSY_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Reply,
        kind: ComponentKind::FreeBusy,
        section: "3.3.3",
        properties: REPLY_FREEBUSY_PROPERTIES,
        subcomponents: REPLY_FREEBUSY_SUBCOMPONENTS,
        components: REPLY_FREEBUSY_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Publish,
        kind: ComponentKind::Todo,
        section: "3.4.1",
        properties: PUBLISH_TODO_PROPERTIES,
        subcomponents: PUBLISH_TODO_SUBCOMPONENTS,
        components: PUBLISH_TODO_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Request,
        kind: ComponentKind::Todo,
        section: "3.4.2",
        properties: REQUEST_TODO_PROPERTIES,
        subcomponents: REQUEST_TODO_SUBCOMPONENTS,
        components: REQUEST_TODO_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Reply,
        kind: ComponentKind::Todo,
        section: "3.4.3",
        properties: REPLY_TODO_PROPERTIES,
        subcomponents: REPLY_TODO_SUBCOMPONENTS,
        components: REPLY_TODO_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Add,
        kind: ComponentKind::Todo,
        section: "3.4.4",
        properties: ADD_TODO_PROPERTIES,
        subcomponents: ADD_TODO_SUBCOMPONENTS,
        components: ADD_TODO_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Cancel,
        kind: ComponentKind::Todo,
        section: "3.4.5",
        properties: CANCEL_TODO_PROPERTIES,
        subcomponents: CANCEL_TODO_SUBCOMPONENTS,
        components: CANCEL_TODO_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Refresh,
        kind: ComponentKind::Todo,
        section: "3.4.6",
        properties: REFRESH_TODO_PROPERTIES,
        subcomponents: REFRESH_TODO_SUBCOMPONENTS,
        components: REFRESH_TODO_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Counter,
        kind: ComponentKind::Todo,
        section: "3.4.7",
        properties: COUNTER_TODO_PROPERTIES,
        subcomponents: COUNTER_TODO_SUBCOMPONENTS,
        components: COUNTER_TODO_COMPONENTS,
        sender: SenderRule::Attendee,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::DeclineCounter,
        kind: ComponentKind::Todo,
        section: "3.4.8",
        properties: DECLINECOUNTER_TODO_PROPERTIES,
        subcomponents: DECLINECOUNTER_TODO_SUBCOMPONENTS,
        components: DECLINECOUNTER_TODO_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Publish,
        kind: ComponentKind::Journal,
        section: "3.5.1",
        properties: PUBLISH_JOURNAL_PROPERTIES,
        subcomponents: PUBLISH_JOURNAL_SUBCOMPONENTS,
        components: PUBLISH_JOURNAL_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Absent, PriorState::Present],
    },
    MethodRule {
        method: Method::Add,
        kind: ComponentKind::Journal,
        section: "3.5.2",
        properties: ADD_JOURNAL_PROPERTIES,
        subcomponents: ADD_JOURNAL_SUBCOMPONENTS,
        components: ADD_JOURNAL_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
    MethodRule {
        method: Method::Cancel,
        kind: ComponentKind::Journal,
        section: "3.5.3",
        properties: CANCEL_JOURNAL_PROPERTIES,
        subcomponents: CANCEL_JOURNAL_SUBCOMPONENTS,
        components: CANCEL_JOURNAL_COMPONENTS,
        sender: SenderRule::Organizer,
        prior_states: &[PriorState::Present],
    },
];

#[cfg(test)]
mod tests {
    use ical_core::ComponentKind;

    use super::{MethodRule, Presence, PriorState, RULES};
    use crate::internal::itip::method::{Method, SenderRule};

    /// Every table RFC 5546 section 3 prints is here exactly once, and nothing else is.
    #[test]
    fn the_twenty_two_tables_are_present_once_each() {
        assert_eq!(RULES.len(), 22);
        for (at, rule) in RULES.iter().enumerate() {
            let twin = RULES
                .iter()
                .position(|other| other.method() == rule.method() && other.kind() == rule.kind());
            assert_eq!(twin, Some(at), "two rules for one pair");
            assert!(
                !rule.properties().is_empty() && !rule.components().is_empty(),
                "section {} lost its rows in transcription",
                rule.section()
            );
        }
        let tallies = [
            (ComponentKind::Event, 8),
            (ComponentKind::FreeBusy, 3),
            (ComponentKind::Todo, 8),
            (ComponentKind::Journal, 3),
        ];
        for (kind, expected) in tallies {
            let seen = RULES.iter().filter(|rule| rule.kind() == kind).count();
            assert_eq!(seen, expected, "{kind:?} has the wrong number of tables");
        }
    }

    /// Rows read straight off the printed tables, one of each shape that matters.
    ///
    /// A spot check rather than a proof: what makes the rest trustworthy is that they were
    /// extracted from the published text rather than typed, and `section()` says where to look.
    #[test]
    fn the_rows_say_what_the_printed_tables_say() {
        let reply = MethodRule::lookup(Method::Reply, ComponentKind::Event).unwrap();
        assert_eq!(reply.section(), "3.2.3");
        assert_eq!(reply.presence_of(b"ATTENDEE"), Presence::ExactlyOnce);
        assert_eq!(reply.presence_of(b"RECURRENCE-ID"), Presence::AtMostOnce);
        assert_eq!(reply.subcomponent_presence(b"VALARM"), Presence::Never);
        assert_eq!(reply.component_presence(b"VTODO"), Presence::Never);

        let publish = MethodRule::lookup(Method::Publish, ComponentKind::Event).unwrap();
        assert_eq!(
            publish.presence_of(b"ATTENDEE"),
            Presence::Never,
            "section 3.2.1: attendees MUST NOT be present in a published component"
        );
        assert_eq!(publish.presence_of(b"SUMMARY"), Presence::ExactlyOnce);
        assert_eq!(
            publish.component_presence(b"VTIMEZONE"),
            Presence::AnyNumber
        );

        let cancel = MethodRule::lookup(Method::Cancel, ComponentKind::Journal).unwrap();
        assert_eq!(cancel.section(), "3.5.3");
        assert_eq!(cancel.sender(), SenderRule::Organizer);
        assert!(!cancel.permits_prior(PriorState::Absent));
        assert!(cancel.permits_prior(PriorState::Present));

        let request = MethodRule::lookup(Method::Request, ComponentKind::FreeBusy).unwrap();
        assert_eq!(request.section(), "3.3.2");
        assert_eq!(request.presence_of(b"FREEBUSY"), Presence::Never);
        assert_eq!(request.presence_of(b"ATTENDEE"), Presence::OnceOrMore);
    }

    /// A name with no row of its own is answered by the class row the specification wrote,
    /// which is what keeps a vendor property from arriving through a hole nobody named.
    #[test]
    fn an_unlisted_name_falls_onto_the_class_row_and_not_onto_a_default() {
        let reply = MethodRule::lookup(Method::Reply, ComponentKind::Event).unwrap();
        assert_eq!(
            reply.presence_of(b"X-MICROSOFT-CDO-BUSYSTATUS"),
            Presence::AnyNumber
        );
        assert_eq!(reply.presence_of(b"x-anything"), Presence::AnyNumber);
        assert_eq!(reply.presence_of(b"CONFERENCE"), Presence::AnyNumber);

        let publish = MethodRule::lookup(Method::Publish, ComponentKind::FreeBusy).unwrap();
        assert_eq!(
            publish.subcomponent_presence(b"VEVENT"),
            Presence::Never,
            "no row, no class row, and so no admission"
        );
    }

    /// The five printed shapes, as the arithmetic every conformance check runs.
    #[test]
    fn a_presence_admits_exactly_the_counts_its_row_prints() {
        assert!(Presence::ExactlyOnce.admits(1) && !Presence::ExactlyOnce.admits(0));
        assert!(Presence::OnceOrMore.admits(9) && !Presence::OnceOrMore.admits(0));
        assert!(Presence::AtMostOnce.admits(0) && !Presence::AtMostOnce.admits(2));
        assert!(Presence::AnyNumber.admits(0) && Presence::AnyNumber.admits(4096));
        assert!(Presence::Never.admits(0) && !Presence::Never.admits(1));
        assert!(Presence::OnceOrMore.is_required() && !Presence::AtMostOnce.is_required());
        assert!(Presence::Never.is_forbidden() && !Presence::AnyNumber.is_forbidden());
    }
}
