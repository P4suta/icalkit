// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What RFC 5545 says a component is made of, and the typed access that follows from it.
//!
//! Specification: RFC 5545 section 3.4 and section 3.6.
//!
//! The tree knows nothing about component names, and that is deliberate: a `BEGIN` this crate
//! has never heard of keeps its entries, its order and its octets exactly as one it has
//! (`docs/adr/0001`). What this module adds is a *reading* of the ones section 3.6 defines —
//! which properties each may carry, which it must, and how often — as a lookup keyed on the
//! component's name and answering nothing at all for a name it does not know.
//!
//! Nothing here changes storage and nothing here refuses a document. A component carrying a
//! property section 3.6 does not define for it is a diagnostic; a component this crate has no
//! definition for allows everything, because "I have no schema" and "the schema forbids it"
//! are different answers and only the second may be reported.
//!
//! [`Component::audit`] is advisory and the caller decides when to run it. Running it inside
//! `parse` would make a reading of section 3.6 a condition of getting a tree back, and running
//! it inside `serialize` would let writing refuse; both contradict `docs/adr/0001` outright,
//! which is why the audit is a method a caller calls when it wants the answer and never a
//! stage anything else goes through. It reports about one component's own properties, so a
//! nested `VALARM` is audited by auditing it — exactly as a nested `DTSTART` is read by reading
//! it there.
//!
//! The accessors here are the same one accessor `access.rs` describes, called with a different
//! name. They are not gated on the component's kind: [`Component::due`] on a `VEVENT` answers
//! absence rather than an error, because a component that carries one is a component this crate
//! must still be able to read.

use ical_grammar::{
    Diagnostic, DiagnosticCode, DiagnosticSink, Location, Meter, Severity, report_diagnostic,
};

use crate::gregorian::{DateTimeValue, Duration, UtcOffset};
use crate::ident::PropertyId;
use crate::tree::{Component, PropertiesNamed, Property};
use crate::view::{TextValue, UriValue, ValueType, View};

/// One of the calendar components RFC 5545 defines.
///
/// `#[non_exhaustive]` because a later RFC may define another — `VAVAILABILITY` of RFC 7953 is
/// the one already published — and because a name this crate does not know is answered with
/// `None` rather than with a variant that would pretend to a schema it has not got.
///
/// The identity is normalized on the way in, as section 3.1 compares every name; the spelling
/// stays on the [`Boundary`](crate::Boundary), so a producer that wrote `begin:vevent` gets
/// `begin:vevent` back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ComponentKind {
    /// `VCALENDAR`, RFC 5545 section 3.4.
    Calendar,
    /// `VEVENT`, RFC 5545 section 3.6.1.
    Event,
    /// `VTODO`, RFC 5545 section 3.6.2.
    Todo,
    /// `VJOURNAL`, RFC 5545 section 3.6.3.
    Journal,
    /// `VFREEBUSY`, RFC 5545 section 3.6.4.
    FreeBusy,
    /// `VTIMEZONE`, RFC 5545 section 3.6.5.
    TimeZone,
    /// The `STANDARD` observance of a `VTIMEZONE`, RFC 5545 section 3.6.5.
    Standard,
    /// The `DAYLIGHT` observance of a `VTIMEZONE`, RFC 5545 section 3.6.5.
    Daylight,
    /// `VALARM`, RFC 5545 section 3.6.6.
    Alarm,
}

impl ComponentKind {
    /// Every kind this crate knows, paired with the name a `BEGIN` line spells.
    const SPELLINGS: [(Self, &'static [u8]); 9] = [
        (Self::Calendar, b"VCALENDAR"),
        (Self::Event, b"VEVENT"),
        (Self::Todo, b"VTODO"),
        (Self::Journal, b"VJOURNAL"),
        (Self::FreeBusy, b"VFREEBUSY"),
        (Self::TimeZone, b"VTIMEZONE"),
        (Self::Standard, b"STANDARD"),
        (Self::Daylight, b"DAYLIGHT"),
        (Self::Alarm, b"VALARM"),
    ];

    /// The name a `BEGIN` line spells this kind as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Calendar => b"VCALENDAR",
            Self::Event => b"VEVENT",
            Self::Todo => b"VTODO",
            Self::Journal => b"VJOURNAL",
            Self::FreeBusy => b"VFREEBUSY",
            Self::TimeZone => b"VTIMEZONE",
            Self::Standard => b"STANDARD",
            Self::Daylight => b"DAYLIGHT",
            Self::Alarm => b"VALARM",
        }
    }

    /// The kind `name` spells, or `None` for a component this crate has no definition for.
    ///
    /// `None` is the answer for every `X-` component and for every component from an RFC
    /// published after this code, and it means "no schema", never "not allowed".
    #[must_use]
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Self::SPELLINGS
            .iter()
            .find(|(_, spelling)| spelling.eq_ignore_ascii_case(name))
            .map(|(kind, _)| *kind)
    }

    /// How often this component may carry the property `name`.
    ///
    /// The name is compared as section 3.1 compares one, so a producer that wrote `summary`
    /// gets the answer it would have got for `SUMMARY`. A name this reading does not carry is
    /// [`Cardinality::NotDefined`], which is a statement about this crate and not about the
    /// file: section 3.8.8 lets a producer put an `X-` name or an IANA name in any component
    /// it likes, and so does every RFC published after this one.
    #[must_use]
    pub fn cardinality(self, name: &[u8]) -> Cardinality {
        let schema = self.schema();
        for (group, class) in [
            (schema.required_once, Cardinality::RequiredOnce),
            (schema.optional_once, Cardinality::OptionalOnce),
            (schema.repeatable, Cardinality::Repeatable),
        ] {
            if group.iter().any(|wanted| wanted.eq_ignore_ascii_case(name)) {
                return class;
            }
        }
        Cardinality::NotDefined
    }

    /// The three name lists section 3.6 gives this component.
    ///
    /// `STANDARD` and `DAYLIGHT` share one, because section 3.6.5 states the observance's
    /// property list once and both of its subcomponents are that list.
    const fn schema(self) -> ComponentSchema {
        match self {
            Self::Calendar => CALENDAR_SCHEMA,
            Self::Event => EVENT_SCHEMA,
            Self::Todo => TODO_SCHEMA,
            Self::Journal => JOURNAL_SCHEMA,
            Self::FreeBusy => FREE_BUSY_SCHEMA,
            Self::TimeZone => TIME_ZONE_SCHEMA,
            Self::Standard | Self::Daylight => OBSERVANCE_SCHEMA,
            Self::Alarm => ALARM_SCHEMA,
        }
    }
}

// ---------------------------------------------------------------------------------------
// The cardinality vocabulary, section 3.6
// ---------------------------------------------------------------------------------------

/// How often RFC 5545 section 3.6 lets one property name occur inside one component.
///
/// Four answers rather than three, and the fourth is the load-bearing one.
/// [`Cardinality::NotDefined`] says this crate has no reading of that name in that component,
/// which is what every `X-` name, every name from an RFC published after this code, and every
/// name inside a component with no [`ComponentKind`] resolves to. It does not say "forbidden":
/// a table that answered "not allowed" where it means "not known" would report a violation
/// against a file that has none, which is the one mistake a reading of an extensible format
/// must not make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Cardinality {
    /// Section 3.6 requires the name, and forbids a second occurrence.
    RequiredOnce,
    /// Section 3.6 permits the name, and forbids a second occurrence.
    OptionalOnce,
    /// Section 3.6 permits the name as often as a producer cares to write it.
    Repeatable,
    /// This crate has no reading of the name inside this component.
    NotDefined,
}

impl Cardinality {
    /// Whether this crate has a reading of the name here at all.
    #[must_use]
    pub const fn is_defined(self) -> bool {
        !matches!(self, Self::NotDefined)
    }

    /// Whether a component that carries no such property violates section 3.6.
    #[must_use]
    pub const fn is_required(self) -> bool {
        matches!(self, Self::RequiredOnce)
    }

    /// Whether a second occurrence is something this crate is willing to report.
    ///
    /// True for [`Cardinality::NotDefined`] as well as for [`Cardinality::Repeatable`], and for
    /// the reason the enum has a fourth variant at all: a name with no reading here is a name
    /// this crate may not count either.
    #[must_use]
    pub const fn allows_repeat(self) -> bool {
        matches!(self, Self::Repeatable | Self::NotDefined)
    }
}

// ---------------------------------------------------------------------------------------
// The tables, one per component of section 3.6
// ---------------------------------------------------------------------------------------

/// The property names one kind of component is a reading of, grouped by how often each occurs.
///
/// Three lists rather than one list of pairs, because both halves of the audit walk a class at
/// a time — the missing-property pass reads the first, the duplicate pass reads the first two —
/// and because section 3.6 itself states the class above a group of names rather than beside
/// each one. Keeping the shape the specification uses is what makes a table reviewable against
/// it.
#[derive(Clone, Copy, Debug)]
struct ComponentSchema {
    /// Names section 3.6 requires, each at most once.
    required_once: &'static [&'static [u8]],
    /// Names section 3.6 permits at most once.
    optional_once: &'static [&'static [u8]],
    /// Names section 3.6 permits any number of times.
    repeatable: &'static [&'static [u8]],
}

/// RFC 5545 section 3.4: what a `VCALENDAR` carries.
const CALENDAR_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"PRODID", b"VERSION"],
    optional_once: &[b"CALSCALE", b"METHOD"],
    // Section 3.4 defines no repeatable calendar property, and these five are not its. They
    // are RFC 7986's, which puts names section 3.8 defines for a component onto the calendar
    // itself — so a reader that knew only section 3.4 would report a `UID` beside `PRODID` as
    // a violation of a document RFC 7986 says is well formed, which is exactly the report the
    // code for it forbids. They are classified as repeatable rather than at-most-once because
    // that RFC admits a second `DESCRIPTION` under a second `LANGUAGE`, and because a count is
    // a claim this table has no reason to make about names it carries in order to stay silent
    // about them. None of them is a new name, so none of them widens what this crate treats as
    // known: that set is still exactly section 3.7's and section 3.8's.
    repeatable: &[
        b"CATEGORIES",
        b"DESCRIPTION",
        b"LAST-MODIFIED",
        b"UID",
        b"URL",
    ],
};

/// RFC 5545 section 3.6.1: what a `VEVENT` carries.
const EVENT_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"DTSTAMP", b"UID"],
    optional_once: &[
        // Section 3.6.1 requires a `DTSTART` of an event in a calendar that states no `METHOD`
        // and makes it optional in one that does. That condition is a property of the enclosing
        // `VCALENDAR`, which a reading of one component cannot see, so this table takes the
        // weaker of the two readings: a missing `DTSTART` is reported by nobody here rather
        // than reported wrongly against every scheduling message that legitimately has none.
        b"DTSTART",
        b"CLASS",
        b"CREATED",
        b"DESCRIPTION",
        b"GEO",
        b"LAST-MODIFIED",
        b"LOCATION",
        b"ORGANIZER",
        b"PRIORITY",
        b"SEQUENCE",
        b"STATUS",
        b"SUMMARY",
        b"TRANSP",
        b"URL",
        b"RECURRENCE-ID",
        // Section 3.6.1 says a second `RRULE` should not occur, where it says a second
        // `SUMMARY` must not. One class covers both: this crate reports the same thing about
        // each, since a caller handed two rules has the same problem either way and
        // `Component::get` already refuses to pick a winner between them.
        b"RRULE",
        // Section 3.6.1 admits either of these and forbids both together. That "and" is an
        // entailment between two properties rather than a count of one, so it belongs to the
        // audit `docs/adr/0001` describes and not to this table.
        b"DTEND",
        b"DURATION",
    ],
    repeatable: &[
        b"ATTACH",
        b"ATTENDEE",
        b"CATEGORIES",
        b"COMMENT",
        b"CONTACT",
        b"EXDATE",
        b"REQUEST-STATUS",
        b"RELATED-TO",
        b"RESOURCES",
        b"RDATE",
    ],
};

/// RFC 5545 section 3.6.2: what a `VTODO` carries.
const TODO_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"DTSTAMP", b"UID"],
    optional_once: &[
        b"CLASS",
        b"COMPLETED",
        b"CREATED",
        b"DESCRIPTION",
        b"DTSTART",
        b"GEO",
        b"LAST-MODIFIED",
        b"LOCATION",
        b"ORGANIZER",
        b"PERCENT-COMPLETE",
        b"PRIORITY",
        b"RECURRENCE-ID",
        b"SEQUENCE",
        b"STATUS",
        b"SUMMARY",
        b"URL",
        b"RRULE",
        // As in section 3.6.1, the pair is exclusive and the exclusion is an entailment. The
        // extra condition section 3.6.2 states — that a `DURATION` here needs a `DTSTART` to
        // measure from — is the same kind of claim and lives in the same place.
        b"DUE",
        b"DURATION",
    ],
    repeatable: &[
        b"ATTACH",
        b"ATTENDEE",
        b"CATEGORIES",
        b"COMMENT",
        b"CONTACT",
        b"EXDATE",
        b"REQUEST-STATUS",
        b"RELATED-TO",
        b"RESOURCES",
        b"RDATE",
    ],
};

/// RFC 5545 section 3.6.3: what a `VJOURNAL` carries.
const JOURNAL_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"DTSTAMP", b"UID"],
    optional_once: &[
        b"CLASS",
        b"CREATED",
        b"DTSTART",
        b"LAST-MODIFIED",
        b"ORGANIZER",
        b"RECURRENCE-ID",
        b"SEQUENCE",
        b"STATUS",
        b"SUMMARY",
        b"URL",
        b"RRULE",
    ],
    repeatable: &[
        b"ATTACH",
        b"ATTENDEE",
        b"CATEGORIES",
        b"COMMENT",
        b"CONTACT",
        // A journal entry may carry several descriptions where an event may carry one. The two
        // lists differ by exactly this name, which is the sort of difference a shared table
        // would have quietly averaged away.
        b"DESCRIPTION",
        b"EXDATE",
        b"RELATED-TO",
        b"RDATE",
        b"REQUEST-STATUS",
    ],
};

/// RFC 5545 section 3.6.4: what a `VFREEBUSY` carries.
const FREE_BUSY_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"DTSTAMP", b"UID"],
    // `CONTACT` is at most once here and repeatable in an event, and section 3.6.4 is where
    // that difference is stated.
    optional_once: &[b"CONTACT", b"DTSTART", b"DTEND", b"ORGANIZER", b"URL"],
    repeatable: &[b"ATTENDEE", b"COMMENT", b"FREEBUSY", b"REQUEST-STATUS"],
};

/// RFC 5545 section 3.6.5: what a `VTIMEZONE` carries.
///
/// The section also requires at least one `STANDARD` or `DAYLIGHT` subcomponent. That is a
/// claim about entries rather than about properties, and this reading does not make it: the
/// four codes the audit reports are all about property names.
const TIME_ZONE_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"TZID"],
    optional_once: &[b"LAST-MODIFIED", b"TZURL"],
    repeatable: &[],
};

/// RFC 5545 section 3.6.5: what a `STANDARD` or a `DAYLIGHT` observance carries.
const OBSERVANCE_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"DTSTART", b"TZOFFSETTO", b"TZOFFSETFROM"],
    optional_once: &[b"RRULE"],
    repeatable: &[b"COMMENT", b"RDATE", b"TZNAME"],
};

/// RFC 5545 section 3.6.6: what a `VALARM` carries.
///
/// Section 3.6.6 states three property lists rather than one, and the `ACTION` decides which of
/// them applies: a display alarm must carry a `DESCRIPTION` that an audio alarm must not, and
/// an email alarm must carry a `SUMMARY` and at least one `ATTENDEE` that neither of the others
/// may have. This table is their union, which is the weaker reading and the safe one — it
/// reports what all three forbid and stays silent where they disagree, so an audio alarm with a
/// `SUMMARY` earns no diagnostic here. Splitting the table by `ACTION` would make the audit's
/// answer depend on a value rather than on a name, and that belongs with the other cross-
/// property entailments `docs/adr/0001` names rather than here.
const ALARM_SCHEMA: ComponentSchema = ComponentSchema {
    required_once: &[b"ACTION", b"TRIGGER"],
    // Section 3.6.6 requires `DURATION` and `REPEAT` to appear together or not at all, which is
    // an entailment and not a count.
    optional_once: &[b"DESCRIPTION", b"DURATION", b"REPEAT", b"SUMMARY"],
    // `RELATED-TO` and `UID` are RFC 9074's addition to this component, carried for the reason
    // the calendar table carries RFC 7986's: an alarm that has one is well formed, and neither
    // is a name this crate did not already know.
    repeatable: &[b"ATTACH", b"ATTENDEE", b"RELATED-TO", b"UID"],
};

/// Whether RFC 5545 defines `name` as a property of anything at all.
///
/// This is the answer that decides silence, and it is derived from the tables rather than kept
/// beside them, because a second list of names is a second place to forget one. Every property
/// section 3.7 and section 3.8 define appears in at least one of the nine tables — a fact a
/// test checks rather than a claim this comment makes — and the later-RFC placements a table
/// carries introduce no name that was not already there, so this stays exactly RFC 5545's set.
///
/// A name outside it is an `X-` name, an IANA name from an RFC published after this code, or
/// something a producer invented, and this crate has nothing true to say about any of the
/// three.
fn defined_by_rfc5545(name: &[u8]) -> bool {
    ComponentKind::SPELLINGS
        .iter()
        .any(|(kind, _)| kind.cardinality(name).is_defined())
}

// ---------------------------------------------------------------------------------------
// The audit
// ---------------------------------------------------------------------------------------

/// Offer one diagnostic about a component to the sink.
///
/// The location is [`Location::NOWHERE`], for the reason every accessor gives it: a
/// [`Property`] owns fresh unfolded octets and not the offsets they were read from, so a span
/// produced here would address a buffer the caller never handed in, and a plausible-looking
/// offset into the wrong buffer is worse than admitting there is none.
///
/// The meter is here because a sink may refuse and the count of refusals has to live outside
/// the sink (`docs/adr/0009`). The audit charges it nothing else: it reads what is already
/// resident, allocates nothing, and an advisory reading that could fail on a budget would be a
/// reading a caller cannot rely on having.
fn report_here<S: DiagnosticSink + ?Sized>(
    sink: &mut S,
    meter: &mut Meter,
    code: DiagnosticCode,
    severity: Severity,
) {
    report_diagnostic(
        sink,
        meter,
        Diagnostic::new(code, severity, Location::NOWHERE),
    );
}

impl Component {
    /// Which of RFC 5545's components this is, or `None` for one with no definition here.
    #[must_use]
    pub fn kind(&self) -> Option<ComponentKind> {
        ComponentKind::from_name(self.name().as_bytes())
    }

    /// Report what RFC 5545 section 3.6 has to say about the properties this component carries.
    ///
    /// Four codes, and each of them is a claim this crate can defend:
    /// [`DiagnosticCode::MissingRequiredProperty`] for a name section 3.6 requires and this
    /// component has none of, [`DiagnosticCode::DuplicateProperty`] for a name it allows once
    /// and this component has two of, [`DiagnosticCode::PropertyNotAllowedHere`] for a name
    /// RFC 5545 defines somewhere and not here, and [`DiagnosticCode::UnknownValueType`] for a
    /// `VALUE` parameter naming a type this crate cannot read.
    ///
    /// Nothing is reported about a component with no [`Component::kind`], and nothing is
    /// reported about a property whose name RFC 5545 does not define. Both silences are the
    /// same silence: this crate has no reading of the thing, and a reading it does not have
    /// cannot be violated.
    ///
    /// Advisory, and it changes nothing. The document that goes in comes back out octet for
    /// octet whether the audit ran or not, which is what lets it be a reading rather than a
    /// gate — see `docs/adr/0001`, and the module documentation for why no other stage calls
    /// it.
    ///
    /// One component's own properties, in the order they arrived and then in the order
    /// section 3.6 lists the names. A nested component is audited by auditing it.
    pub fn audit<S: DiagnosticSink + ?Sized>(&self, meter: &mut Meter, sink: &mut S) {
        let Some(kind) = self.kind() else {
            // "I have no schema" and "the schema forbids it" are different answers, and only
            // the second is reportable. A component this crate has no definition for keeps
            // every entry it arrived with and earns no diagnostic about any of them.
            return;
        };
        self.audit_each_property(kind, meter, sink);
        self.audit_each_name(kind, meter, sink);
    }

    /// The pass over the properties as they arrived, in the order a producer wrote them.
    fn audit_each_property<S: DiagnosticSink + ?Sized>(
        &self,
        kind: ComponentKind,
        meter: &mut Meter,
        sink: &mut S,
    ) {
        for property in self.properties() {
            let name = property.name().as_bytes();
            if !defined_by_rfc5545(name) {
                // Section 3.8.8 puts an `X-` name, and an IANA name this crate has never heard
                // of, in whatever component their producer chose. Nothing here can say anything
                // true about either — including about a `VALUE` one of them carries, since a
                // property this crate cannot name is a property whose value type is its
                // producer's business.
                continue;
            }
            if let Some(Err(code)) = property.declared_value_type() {
                // A note rather than a violation. Section 3.2.20's grammar admits an `X-` name
                // and an IANA token, so a value type this crate does not carry is not by itself
                // a defect in the file; what it costs is this crate's ability to type the
                // value, which the caller is entitled to be told about.
                report_here(sink, meter, code, Severity::Note);
            }
            if !kind.cardinality(name).is_defined() {
                report_here(
                    sink,
                    meter,
                    DiagnosticCode::PropertyNotAllowedHere,
                    Severity::Violation,
                );
            }
        }
    }

    /// The pass over the names section 3.6 gives this component, in the order it gives them.
    ///
    /// Counted per name rather than per occurrence, so two `SUMMARY` lines are one report about
    /// a name and not two reports about lines. That is the same claim [`Component::get`] makes
    /// when it refuses to pick a winner between them, stated once for a whole component instead
    /// of once per read.
    fn audit_each_name<S: DiagnosticSink + ?Sized>(
        &self,
        kind: ComponentKind,
        meter: &mut Meter,
        sink: &mut S,
    ) {
        let schema = kind.schema();
        for (group, required) in [(schema.required_once, true), (schema.optional_once, false)] {
            for wanted in group {
                let seen = self
                    .properties()
                    .filter(|property| property.is_named(wanted))
                    .count();
                // Counted, not short-circuited: "none of them" and "two of them" are the two
                // answers section 3.6 has an opinion about, and they are not the same walk.
                if seen == 0 && required {
                    report_here(
                        sink,
                        meter,
                        DiagnosticCode::MissingRequiredProperty,
                        Severity::Violation,
                    );
                } else if seen > 1 {
                    report_here(
                        sink,
                        meter,
                        DiagnosticCode::DuplicateProperty,
                        Severity::Violation,
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The declared value type, section 3.2.20
// ---------------------------------------------------------------------------------------

impl Property {
    /// The value type this property's `VALUE` parameter declares, RFC 5545 section 3.2.20.
    ///
    /// Three answers, and they are three different things. `None` is no `VALUE` parameter at
    /// all, which is the ordinary case and means the default type section 3.8 states for that
    /// property applies — this method does not guess which, because the default is a fact about
    /// the property's name and this level is asking about its parameters. `Some(Ok(_))` is a
    /// type this crate knows. `Some(Err(_))` is [`DiagnosticCode::UnknownValueType`]: a name
    /// section 3.2.20's grammar admits, since it allows an `X-` name and an IANA token, and
    /// this crate's [`ValueType`] table does not carry — so nothing here can type the value.
    /// The octets are untouched in all three, and are written back in all three.
    ///
    /// The first occurrence, with a section 3.2 `DQUOTE` pair removed. A second `VALUE` on one
    /// line is a defect this level cannot report, since it answers with one type or one code;
    /// both stay in storage and reachable through [`Property::parameters_named`].
    ///
    /// A `VALUE` that arrived with no `=`, and one that arrived with nothing after it, both
    /// answer [`DiagnosticCode::UnknownValueType`]. One answer, because there is one question —
    /// what did the producer name — and neither of them named anything.
    #[must_use]
    pub fn declared_value_type(&self) -> Option<Result<ValueType, DiagnosticCode>> {
        let declared = self.parameters_named(b"VALUE").next()?;
        Some(ValueType::from_name(declared.unquoted()).ok_or(DiagnosticCode::UnknownValueType))
    }
}

// ---------------------------------------------------------------------------------------
// The per-component accessors, sections 3.7 and 3.8
// ---------------------------------------------------------------------------------------

/// The identities the repeatable accessors look up.
///
/// `static` rather than `const`, and the difference is not cosmetic:
/// [`Component::properties_named`] ties the identity's lifetime to the iterator it hands back,
/// and `&SOME_CONST` is a temporary that cannot be promoted to `'static` for a type that owns a
/// name. A `static` is the borrow the signature asks for.
static ATTENDEE: PropertyId = PropertyId::ATTENDEE;

/// The identity [`Component::freebusy`] looks up. See [`ATTENDEE`] for why it is a `static`.
static FREEBUSY: PropertyId = PropertyId::FREEBUSY;

/// The identity [`Component::due`] looks up.
///
/// [`PropertyId`]'s constants are a convenience over [`PropertyId::from_static`] and never a
/// closed set, so a name that list does not carry is reached the same way and reaches the same
/// storage.
static DUE: PropertyId = PropertyId::from_static(b"DUE");

/// The identity [`Component::trigger`] looks up. See [`DUE`] for why it is spelled out here.
static TRIGGER: PropertyId = PropertyId::from_static(b"TRIGGER");

impl Component {
    /// The `DTSTAMP`, RFC 5545 section 3.8.7.2.
    #[must_use]
    pub fn dtstamp(&self) -> View<'_, DateTimeValue<'_>> {
        self.get(&PropertyId::DTSTAMP)
    }

    /// The `DUE` of a to-do, RFC 5545 section 3.8.2.3.
    #[must_use]
    pub fn due(&self) -> View<'_, DateTimeValue<'_>> {
        self.get(&DUE)
    }

    /// The `DURATION`, RFC 5545 section 3.8.2.5.
    #[must_use]
    pub fn duration(&self) -> View<'_, Duration> {
        self.get(&PropertyId::DURATION)
    }

    /// The `TRIGGER` of an alarm, RFC 5545 section 3.8.6.3, in its relative form.
    ///
    /// Section 3.8.6.3 gives the property two value types: a span measured from the event by
    /// default, and an absolute instant under `VALUE=DATE-TIME`. This reads the default, so an
    /// alarm whose trigger is absolute reads as [`View::Malformed`] here and its value is one
    /// call away — `get::<DateTimeValue<'_>>` against the same name — with
    /// [`Property::declared_value_type`] saying which of the two arrived. Two accessors
    /// differing only in the type they decode would be one call written twice, and the caller
    /// has to ask the parameter either way.
    #[must_use]
    pub fn trigger(&self) -> View<'_, Duration> {
        self.get(&TRIGGER)
    }

    /// The `ORGANIZER`, RFC 5545 section 3.8.4.3.
    ///
    /// Section 3.3.3 defines a calendar address as a URI and adds no syntax of its own, which
    /// is why this reads as a [`UriValue`] rather than through a type that would be a second
    /// name for one grammar. Nothing is normalized: a scheme's case and a path's percent-
    /// encoding are exactly what a scheduling reply has to match against.
    #[must_use]
    pub fn organizer(&self) -> View<'_, UriValue<'_>> {
        self.get(&PropertyId::ORGANIZER)
    }

    /// Every `ATTENDEE`, RFC 5545 section 3.8.4.1, in the order they arrived.
    ///
    /// An iterator rather than a [`View`], because section 3.6 puts no limit on how many a
    /// component carries. A singular accessor here would silently keep the first, which is the
    /// data loss this crate exists to prevent, arriving through an accessor instead of through
    /// a parser.
    #[must_use]
    pub fn attendees(&self) -> PropertiesNamed<'_> {
        self.properties_named(&ATTENDEE)
    }

    /// The `STATUS`, RFC 5545 section 3.8.1.11.
    ///
    /// The octets, not one of section 3.8.1.11's enumerated tokens. Which tokens are legal
    /// depends on the component the property sits in, and a value outside the list is still a
    /// value this crate writes back; a caller comparing against `CONFIRMED` compares the bytes
    /// it was given.
    #[must_use]
    pub fn status(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::STATUS)
    }

    /// The `TRANSP`, RFC 5545 section 3.8.2.7.
    #[must_use]
    pub fn transp(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::TRANSP)
    }

    /// The `PRIORITY`, RFC 5545 section 3.8.1.9.
    ///
    /// The integer as written. Section 3.8.1.9 bounds it to zero through nine and maps it onto
    /// three bands, and neither the bound nor the mapping is applied here: a `PRIORITY:42` is a
    /// violation to report rather than octets to reinterpret.
    #[must_use]
    pub fn priority(&self) -> View<'_, i32> {
        self.get(&PropertyId::PRIORITY)
    }

    /// The `CLASS`, RFC 5545 section 3.8.1.3.
    #[must_use]
    pub fn class(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::CLASS)
    }

    /// The `LOCATION`, RFC 5545 section 3.8.1.7, with its escapes still in it.
    #[must_use]
    pub fn location(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::LOCATION)
    }

    /// The `DESCRIPTION`, RFC 5545 section 3.8.1.5, with its escapes still in it.
    ///
    /// A `VJOURNAL` may carry several, and section 3.6.3 is the only place that says so. This
    /// reads the one a component the specification allows one of has, and a journal entry with
    /// two of them reads as [`View::Malformed`] here while both stay reachable through
    /// [`Component::properties_named`].
    #[must_use]
    pub fn description(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::DESCRIPTION)
    }

    /// The `TZID` of a `VTIMEZONE`, RFC 5545 section 3.8.3.1.
    ///
    /// The zone this component *defines*, which is not the `TZID` parameter a date-time carries
    /// to name a zone it is *in*. The two are spelled alike and are different questions, and
    /// the parameter is read where the value is.
    #[must_use]
    pub fn tzid(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::TZID)
    }

    /// The `TZOFFSETFROM` of an observance, RFC 5545 section 3.8.3.3.
    #[must_use]
    pub fn tzoffsetfrom(&self) -> View<'_, UtcOffset> {
        self.get(&PropertyId::TZOFFSETFROM)
    }

    /// The `TZOFFSETTO` of an observance, RFC 5545 section 3.8.3.4.
    #[must_use]
    pub fn tzoffsetto(&self) -> View<'_, UtcOffset> {
        self.get(&PropertyId::TZOFFSETTO)
    }

    /// Every `FREEBUSY`, RFC 5545 section 3.8.2.6, in the order they arrived.
    ///
    /// An iterator for [`Component::attendees`]'s reason, and one more of its own: section
    /// 3.8.2.6 lets one line carry several periods and lets several lines carry more, so the
    /// count of busy spans is not the count of properties either way.
    #[must_use]
    pub fn freebusy(&self) -> PropertiesNamed<'_> {
        self.properties_named(&FREEBUSY)
    }

    /// The `RRULE`, RFC 5545 section 3.8.5.3, as the octets it arrived as.
    ///
    /// Section 3.3.10's grammar belongs to `ical-recur` and is not read here, so what comes
    /// back is a view over preserved text and [`TextValue::as_bytes`] is the whole of it. This
    /// accessor exists so that "this crate does not parse a recurrence rule" does not also mean
    /// "this crate cannot find one".
    #[must_use]
    pub fn rrule(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::RRULE)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use ical_grammar::{
        Diagnostic, DiagnosticCode, IgnoreDiagnostics, Limits, LineEnding, LineLayout, Meter,
        Severity,
    };

    use super::{Cardinality, ComponentKind, defined_by_rfc5545};
    use crate::ident::PropertyId;
    use crate::octets::RawText;
    use crate::tree::{Boundary, Component, Document, Item, Parameter, Property};
    use crate::view::{TextValue, ValueType, View};

    /// A closed component of the given name, carrying the given entries.
    fn component(name: &[u8], items: Vec<Item>) -> Component {
        let edge = |keyword: &[u8]| {
            Boundary::new(
                RawText::from_bytes(keyword),
                RawText::from_bytes(name),
                LineLayout::canonical(LineEnding::CANONICAL),
            )
        };
        Component::new(edge(b"BEGIN"), items, Some(edge(b"END")))
    }

    /// A content line as a well-behaved producer wrote it.
    fn line(name: &[u8], value: &[u8]) -> Item {
        Item::Property(decorated(name, &[], value))
    }

    /// A content line carrying parameters, in the order they were written.
    fn decorated(name: &[u8], parameters: &[(&[u8], &[u8])], value: &[u8]) -> Property {
        let written = parameters
            .iter()
            .map(|(key, text)| Parameter::new(RawText::from_bytes(key), RawText::from_bytes(text)))
            .collect();
        Property::new(
            RawText::from_bytes(name),
            written,
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    /// Every code the audit reports about every component of `document`, in document order.
    ///
    /// The walk is the caller's, because [`Component::audit`] reports about one component. An
    /// explicit stack rather than recursion, for the reason every other traversal in this crate
    /// uses one: the depth is a caller-tunable bound and a stack overflow is an abort.
    fn audit_every_component(document: &Document) -> Vec<DiagnosticCode> {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<DiagnosticCode> = Vec::new();
        let mut pending: Vec<&Component> = document.components().collect();
        pending.reverse();
        while let Some(component) = pending.pop() {
            let mut sink: Vec<Diagnostic> = Vec::new();
            component.audit(&mut meter, &mut sink);
            reported.extend(sink.iter().copied().map(Diagnostic::code));
            let mut nested: Vec<&Component> = component.components().collect();
            nested.reverse();
            pending.extend(nested);
        }
        reported
    }

    #[test]
    fn every_name_round_trips_however_the_producer_spelled_it() {
        for (kind, spelling) in ComponentKind::SPELLINGS {
            assert_eq!(ComponentKind::from_name(spelling), Some(kind));
            assert_eq!(kind.as_bytes(), spelling);
        }
        assert_eq!(
            ComponentKind::from_name(b"vevent"),
            Some(ComponentKind::Event)
        );
    }

    /// A component with no definition here is answered with absence, which is not a refusal:
    /// its entries are stored, ordered and written back exactly as a known one's are.
    #[test]
    fn a_component_with_no_definition_here_has_no_kind_and_keeps_everything() {
        assert_eq!(ComponentKind::from_name(b"X-VENDOR-BLOCK"), None);
        assert_eq!(ComponentKind::from_name(b"VAVAILABILITY"), None);
        assert_eq!(component(b"X-VENDOR-BLOCK", Vec::new()).kind(), None);
        assert_eq!(
            component(b"vevent", Vec::new()).kind(),
            Some(ComponentKind::Event)
        );
    }

    /// The audit, case by case: the octets a producer wrote, and what section 3.6 has to say
    /// about them.
    const CASES: [(&[u8], &[DiagnosticCode]); 13] = [
        // Nothing at all. No component, so nothing to have a schema for.
        (b"", &[]),
        // A calendar with every name section 3.6 requires of it.
        (
            b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//test//EN\r\n\
              BEGIN:VEVENT\r\nUID:1@example.test\r\nDTSTAMP:20260810T090000Z\r\n\
              DTSTART:20260810T100000Z\r\nSUMMARY:standup\r\nEND:VEVENT\r\n\
              END:VCALENDAR\r\n",
            &[],
        ),
        // A `UID` section 3.6.1 requires and this event has none of.
        (
            b"BEGIN:VEVENT\r\nDTSTAMP:20260810T090000Z\r\nEND:VEVENT\r\n",
            &[DiagnosticCode::MissingRequiredProperty],
        ),
        // Two of a name section 3.6.1 allows once, spelled two ways, counted as one name.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\nSUMMARY:a\r\n\
              summary:b\r\nEND:VEVENT\r\n",
            &[DiagnosticCode::DuplicateProperty],
        ),
        // A name RFC 5545 defines for a to-do and not for an event.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\n\
              DUE:20260810T110000Z\r\nEND:VEVENT\r\n",
            &[DiagnosticCode::PropertyNotAllowedHere],
        ),
        // A vendor name, a name from an RFC published after this code, and a name RFC 2445
        // defined and RFC 5545 does not. This crate has no reading of any of the three.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\nX-VENDOR-THING:1\r\n\
              CONFERENCE;VALUE=URI:https://example.test/call\r\nEXRULE:FREQ=DAILY\r\n\
              END:VEVENT\r\n",
            &[],
        ),
        // A `VALUE` naming a type this crate cannot read, on a name it knows and on one it
        // does not. Only the first is something it can say anything about.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\n\
              SUMMARY;VALUE=WIDGET:hi\r\nX-VENDOR;VALUE=WIDGET:hi\r\nEND:VEVENT\r\n",
            &[DiagnosticCode::UnknownValueType],
        ),
        // The boundary this reading is bounded by: no kind, so no schema, so no report — not
        // about the missing `UID`, the misplaced `DUE`, the duplicate, or the value type.
        (
            b"BEGIN:X-VENDOR-BLOCK\r\nDUE:20260810T110000Z\r\nSUMMARY;VALUE=WIDGET:a\r\n\
              SUMMARY:b\r\nEND:X-VENDOR-BLOCK\r\n",
            &[],
        ),
        // A nested component is audited by auditing it, and this alarm is missing its
        // `ACTION` while the event around it is complete.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\nBEGIN:VALARM\r\n\
              TRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n",
            &[DiagnosticCode::MissingRequiredProperty],
        ),
        // An observance with no `TZOFFSETTO`. The zone around it is complete.
        (
            b"BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nBEGIN:STANDARD\r\n\
              DTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nEND:STANDARD\r\n\
              END:VTIMEZONE\r\n",
            &[DiagnosticCode::MissingRequiredProperty],
        ),
        // RFC 7986 puts a `UID` and a `NAME` on the calendar itself. One is a name RFC 5545
        // defines elsewhere and one is a name it does not define at all, and neither is a
        // violation of anything.
        (
            b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//test//EN\r\n\
              UID:calendar-1@example.test\r\nNAME:Team\r\nEND:VCALENDAR\r\n",
            &[],
        ),
        // Both passes, in order: what arrived, and then what section 3.6 asks for.
        (
            b"BEGIN:VEVENT\r\nUID:1\r\nDTSTAMP:20260810T090000Z\r\n\
              DTSTART:20260810T100000Z\r\nDUE:20260810T110000Z\r\n\
              DTSTART:20260811T100000Z\r\nEND:VEVENT\r\n",
            &[
                DiagnosticCode::PropertyNotAllowedHere,
                DiagnosticCode::DuplicateProperty,
            ],
        ),
        // A file whose syntax is broken in four places: a bare `LF`, a blank line, a line with
        // no `:`, and an `END` that disagrees in case. None of the three names is one RFC 5545
        // defines, the audit says only what it can defend, and every octet comes back.
        (
            b"BEGIN:VEVENT\nDTSTAMP:20260810T090000Z\r\n\r\nGARBAGE\r\nEND:vevent\r\n",
            &[DiagnosticCode::MissingRequiredProperty],
        ),
    ];

    /// Every case asserts twice: the codes, and that the document writes back the octets it
    /// read. The second assertion is the one that would catch a reading that had quietly
    /// become an edit.
    #[test]
    fn what_section_3_6_says_about_a_calendar_and_what_it_does_to_it() {
        for (input, expected) in CASES {
            let mut at_parse: Vec<Diagnostic> = Vec::new();
            let document = Document::parse(input, Limits::DEFAULT, &mut at_parse).unwrap();
            assert_eq!(
                audit_every_component(&document),
                expected,
                "the audit of {input:?}"
            );
            assert_eq!(
                document.to_bytes(),
                input,
                "the audit read {input:?} and it must still write back"
            );
        }
    }

    /// A missing name is a violation, and a value type nobody here knows is a note: section
    /// 3.2.20's grammar admits the name, so the file is not wrong for carrying it.
    #[test]
    fn the_two_severities_say_different_things_and_are_not_interchangeable() {
        let event = component(
            b"VEVENT",
            vec![Item::Property(decorated(
                b"SUMMARY",
                &[(b"VALUE", b"WIDGET")],
                b"hi",
            ))],
        );
        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut meter = Meter::new(Limits::DEFAULT);
        event.audit(&mut meter, &mut sink);

        let reported: Vec<(DiagnosticCode, Severity)> = sink
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.severity()))
            .collect();
        assert_eq!(
            reported,
            vec![
                (DiagnosticCode::UnknownValueType, Severity::Note),
                (DiagnosticCode::MissingRequiredProperty, Severity::Violation),
                (DiagnosticCode::MissingRequiredProperty, Severity::Violation),
            ]
        );
    }

    /// Nothing runs the audit but a caller. A parse that reported section 3.6's requirements
    /// would make a reading of them a condition of getting a tree back, which is the posture
    /// `docs/adr/0001` refuses.
    #[test]
    fn no_stage_of_reading_or_writing_runs_the_audit() {
        let input: &[u8] = b"BEGIN:VEVENT\r\nEND:VEVENT\r\n";
        let mut at_parse: Vec<Diagnostic> = Vec::new();
        let document = Document::parse(input, Limits::DEFAULT, &mut at_parse).unwrap();
        assert!(
            !at_parse.iter().any(|diagnostic| matches!(
                diagnostic.code(),
                DiagnosticCode::MissingRequiredProperty
                    | DiagnosticCode::PropertyNotAllowedHere
                    | DiagnosticCode::DuplicateProperty
            )),
            "the parser reported a schema reading nobody asked it for"
        );
        assert_eq!(document.to_bytes(), input);
        assert_eq!(
            audit_every_component(&document),
            vec![
                DiagnosticCode::MissingRequiredProperty,
                DiagnosticCode::MissingRequiredProperty,
            ],
            "asked, it answers about both names section 3.6.1 requires"
        );
    }

    /// A sink is allowed to refuse, and a caller that loses *which* violations occurred must
    /// not also lose *that* they did (`docs/adr/0009`).
    #[test]
    fn a_refused_diagnostic_is_counted_against_the_meter() {
        let mut meter = Meter::new(Limits::DEFAULT);
        component(b"VEVENT", Vec::new()).audit(&mut meter, &mut IgnoreDiagnostics);
        assert_eq!(meter.diagnostics_dropped(), 2);
        assert!(!meter.is_exhausted(), "a reading spends no octet budget");
    }

    /// The table and the singular accessor make one claim, not two: a name classified at most
    /// once here is a name [`Component::get`] refuses two of.
    #[test]
    fn a_name_the_table_allows_once_is_a_name_the_accessor_refuses_two_of() {
        assert_eq!(
            ComponentKind::Event.cardinality(b"SUMMARY"),
            Cardinality::OptionalOnce
        );
        assert!(!Cardinality::OptionalOnce.allows_repeat());

        let twice = component(
            b"VEVENT",
            vec![line(b"SUMMARY", b"a"), line(b"summary", b"b")],
        );
        let view: View<'_, TextValue<'_>> = twice.get(&PropertyId::SUMMARY);
        assert_eq!(
            view.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::DuplicateProperty)
        );

        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut meter = Meter::new(Limits::DEFAULT);
        twice.audit(&mut meter, &mut sink);
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::DuplicateProperty)
        );
    }

    /// Every property RFC 5545 defines is known to at least one component, which is what lets
    /// the silence rule be derived from the tables instead of kept in a second list beside
    /// them.
    #[test]
    fn every_property_sections_3_7_and_3_8_define_is_known_to_some_component() {
        let defined: [&[u8]; 46] = [
            b"ACTION",
            b"ATTACH",
            b"ATTENDEE",
            b"CALSCALE",
            b"CATEGORIES",
            b"CLASS",
            b"COMMENT",
            b"COMPLETED",
            b"CONTACT",
            b"CREATED",
            b"DESCRIPTION",
            b"DTEND",
            b"DTSTAMP",
            b"DTSTART",
            b"DUE",
            b"DURATION",
            b"EXDATE",
            b"FREEBUSY",
            b"GEO",
            b"LAST-MODIFIED",
            b"LOCATION",
            b"METHOD",
            b"ORGANIZER",
            b"PERCENT-COMPLETE",
            b"PRIORITY",
            b"PRODID",
            b"RDATE",
            b"RECURRENCE-ID",
            b"RELATED-TO",
            b"REPEAT",
            b"REQUEST-STATUS",
            b"RESOURCES",
            b"RRULE",
            b"SEQUENCE",
            b"STATUS",
            b"SUMMARY",
            b"TRANSP",
            b"TRIGGER",
            b"TZID",
            b"TZNAME",
            b"TZOFFSETFROM",
            b"TZOFFSETTO",
            b"TZURL",
            b"UID",
            b"URL",
            b"VERSION",
        ];
        for name in defined {
            assert!(defined_by_rfc5545(name), "{name:?} is defined nowhere");
        }
    }

    /// The other half of the same rule. A name RFC 5545 does not define is a name this crate
    /// reports nothing about, wherever a producer put it — including the names later RFCs added
    /// beside the ones the tables carry for those RFCs' sake.
    #[test]
    fn a_name_no_section_of_rfc_5545_defines_is_not_this_crates_to_report() {
        let unknown: [&[u8]; 8] = [
            b"",
            b"X-MICROSOFT-CDO-BUSYSTATUS",
            b"COLOR",
            b"NAME",
            b"CONFERENCE",
            b"IMAGE",
            b"EXRULE",
            b"GARBAGE",
        ];
        assert!(Cardinality::NotDefined.allows_repeat());
        for name in unknown {
            assert!(!defined_by_rfc5545(name), "{name:?} is claimed by a table");
            assert_eq!(
                ComponentKind::Event.cardinality(name),
                Cardinality::NotDefined
            );
        }
    }

    /// Each table states one class per name, in the spelling the RFC uses. A name in two lists
    /// would make the class depend on which pass looked first.
    #[test]
    fn each_table_states_one_class_per_name_and_states_it_in_the_rfcs_spelling() {
        for (kind, _) in ComponentKind::SPELLINGS {
            let schema = kind.schema();
            let mut every: Vec<&[u8]> = Vec::new();
            for group in [
                schema.required_once,
                schema.optional_once,
                schema.repeatable,
            ] {
                for wanted in group {
                    assert!(
                        wanted.iter().all(|octet| !octet.is_ascii_lowercase()),
                        "{wanted:?} is not spelled as RFC 5545 spells it"
                    );
                    every.push(*wanted);
                }
            }
            let stated = every.len();
            every.sort_unstable();
            every.dedup();
            assert_eq!(
                every.len(),
                stated,
                "a name is classified twice for {kind:?}"
            );
        }
    }

    /// The classes are what section 3.6 says, read back through the two questions the audit
    /// asks of them.
    #[test]
    fn the_classes_are_the_ones_section_3_6_states() {
        assert!(ComponentKind::Event.cardinality(b"dtstamp").is_required());
        assert!(!ComponentKind::Event.cardinality(b"DTSTART").is_required());
        assert_eq!(
            ComponentKind::Journal.cardinality(b"DESCRIPTION"),
            Cardinality::Repeatable,
            "a journal entry may carry several where an event carries one"
        );
        assert_eq!(
            ComponentKind::Event.cardinality(b"DESCRIPTION"),
            Cardinality::OptionalOnce
        );
        assert_eq!(
            ComponentKind::FreeBusy.cardinality(b"CONTACT"),
            Cardinality::OptionalOnce,
            "at most once here, repeatable in an event"
        );
        assert_eq!(
            ComponentKind::Event.cardinality(b"CONTACT"),
            Cardinality::Repeatable
        );
        assert_eq!(
            ComponentKind::Standard.cardinality(b"TZOFFSETTO"),
            ComponentKind::Daylight.cardinality(b"TZOFFSETTO"),
            "section 3.6.5 states one property list and both observances are it"
        );
        assert_eq!(
            ComponentKind::TimeZone.cardinality(b"TZOFFSETTO"),
            Cardinality::NotDefined,
            "the offsets belong to the observance and not to the zone around it"
        );
    }

    /// The parameters a property carries, as name/value pairs, and the answer
    /// [`Property::declared_value_type`] owes for them.
    type DeclaredCase<'a> = (
        &'a [(&'a [u8], &'a [u8])],
        Option<Result<ValueType, DiagnosticCode>>,
    );

    /// What a `VALUE` parameter declares, and what it declares when it declares nothing.
    #[test]
    fn what_a_value_parameter_says_about_the_octets_beside_it() {
        let cases: [DeclaredCase<'_>; 7] = [
            (&[], None),
            (&[(b"VALUE", b"DATE")], Some(Ok(ValueType::Date))),
            (&[(b"value", b"date-time")], Some(Ok(ValueType::DateTime))),
            (&[(b"VALUE", b"\"TEXT\"")], Some(Ok(ValueType::Text))),
            (
                &[(b"TZID", b"Europe/Paris"), (b"VALUE", b"DATE")],
                Some(Ok(ValueType::Date)),
            ),
            (
                &[(b"VALUE", b"WIDGET")],
                Some(Err(DiagnosticCode::UnknownValueType)),
            ),
            (
                &[(b"VALUE", b"")],
                Some(Err(DiagnosticCode::UnknownValueType)),
            ),
        ];

        for (parameters, expected) in cases {
            let property = decorated(b"X-P", parameters, b"v");
            assert_eq!(
                property.declared_value_type(),
                expected,
                "{parameters:?} should declare {expected:?}"
            );
        }
    }

    /// A `VALUE` with no `=` names no type, exactly as an empty one does, and a second `VALUE`
    /// changes no answer — both stay in storage either way.
    #[test]
    fn a_value_parameter_that_names_nothing_and_one_that_names_twice() {
        let empty = Property::new(
            RawText::from_bytes(b"X-P"),
            vec![Parameter::without_value(RawText::from_bytes(b"VALUE"))],
            RawText::from_bytes(b"v"),
            LineLayout::canonical(LineEnding::CANONICAL),
        );
        assert_eq!(
            empty.declared_value_type(),
            Some(Err(DiagnosticCode::UnknownValueType))
        );

        let twice = decorated(b"X-P", &[(b"VALUE", b"DATE"), (b"VALUE", b"TEXT")], b"v");
        assert_eq!(twice.declared_value_type(), Some(Ok(ValueType::Date)));
        assert_eq!(twice.parameters_named(b"VALUE").count(), 2);
    }

    /// Every accessor is the general accessor called with a name, so each of them answers
    /// absence the way it does, and none of them reads a component's kind before answering.
    #[test]
    fn every_accessor_answers_absence_and_finds_what_it_names() {
        let empty = component(b"VEVENT", Vec::new());
        assert!(!empty.dtstamp().is_present());
        assert!(!empty.due().is_present());
        assert!(!empty.duration().is_present());
        assert!(!empty.trigger().is_present());
        assert!(!empty.organizer().is_present());
        assert!(!empty.status().is_present());
        assert!(!empty.transp().is_present());
        assert!(!empty.priority().is_present());
        assert!(!empty.class().is_present());
        assert!(!empty.location().is_present());
        assert!(!empty.description().is_present());
        assert!(!empty.tzid().is_present());
        assert!(!empty.tzoffsetfrom().is_present());
        assert!(!empty.tzoffsetto().is_present());
        assert!(!empty.rrule().is_present());
        assert_eq!(empty.attendees().count(), 0);
        assert_eq!(empty.freebusy().count(), 0);

        let filled = component(
            b"VEVENT",
            vec![
                line(b"DTSTAMP", b"20260810T090000Z"),
                line(b"DUE", b"20260810T110000Z"),
                line(b"DURATION", b"PT1H"),
                line(b"TRIGGER", b"-PT15M"),
                line(b"ORGANIZER", b"mailto:chair@example.test"),
                line(b"ATTENDEE", b"mailto:a@example.test"),
                line(b"attendee", b"mailto:b@example.test"),
                line(b"STATUS", b"CONFIRMED"),
                line(b"TRANSP", b"OPAQUE"),
                line(b"PRIORITY", b"5"),
                line(b"CLASS", b"PUBLIC"),
                line(b"LOCATION", b"Room 1\\, upstairs"),
                line(b"DESCRIPTION", b"the weekly one"),
                line(b"TZID", b"Europe/Paris"),
                line(b"TZOFFSETFROM", b"+0100"),
                line(b"TZOFFSETTO", b"+0200"),
                line(b"FREEBUSY", b"20260810T090000Z/PT1H"),
                line(b"RRULE", b"FREQ=DAILY;COUNT=3"),
            ],
        );
        assert!(filled.dtstamp().is_valid());
        assert!(filled.due().is_valid());
        assert!(filled.duration().is_valid());
        assert!(filled.trigger().is_valid());
        assert!(filled.tzoffsetfrom().is_valid());
        assert!(filled.tzoffsetto().is_valid());
        assert_eq!(filled.priority().value(), Some(5));
        assert_eq!(filled.attendees().count(), 2);
        assert_eq!(filled.freebusy().count(), 1);
        assert_eq!(
            filled.status().value().map(TextValue::as_bytes),
            Some(&b"CONFIRMED"[..])
        );
        assert_eq!(
            filled.transp().value().map(TextValue::as_bytes),
            Some(&b"OPAQUE"[..])
        );
        assert_eq!(
            filled.class().value().map(TextValue::as_bytes),
            Some(&b"PUBLIC"[..])
        );
        assert_eq!(
            filled.tzid().value().map(TextValue::as_bytes),
            Some(&b"Europe/Paris"[..])
        );
        assert_eq!(
            filled.location().value().map(TextValue::as_bytes),
            Some(&b"Room 1\\, upstairs"[..]),
            "the escapes are still in it, as they will be written back"
        );
        assert_eq!(
            filled.description().value().map(TextValue::as_bytes),
            Some(&b"the weekly one"[..])
        );
        assert_eq!(
            filled.rrule().value().map(TextValue::as_bytes),
            Some(&b"FREQ=DAILY;COUNT=3"[..]),
            "a rule is found here and read in `ical-recur`"
        );
    }
}
