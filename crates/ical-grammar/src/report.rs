// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The channel for "this is wrong and the file is still here".
//!
//! One vocabulary for the whole workspace, defined at the bottom of it. Rust has no
//! extensible enum, so the choice was one set of codes defined here or one set per crate to
//! reconcile at every seam; `docs/adr/0004` forbids the second, which is why this crate
//! enumerates codes only `ical-tz` or `ical-recur` can emit. That is also why the golden
//! list `docs/adr/0009` requires is a workspace artifact rather than a per-crate one.
//!
//! A sink is allowed to refuse. A device with no allocator takes a fixed-capacity sink or
//! [`IgnoreDiagnostics`], and neither can accept an unbounded number of violations from a
//! file that repeats one. No reader may treat a refusal as a reason to stop reading, and no
//! refusal is silent: [`report_diagnostic`] charges the refusal to the caller's meter, so
//! "no violation was found" stays distinguishable from "violations were found and could not
//! be delivered".

use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter};

use crate::budget::Meter;
use crate::instant::Instant;
use crate::location::Location;

/// How much a diagnostic claims.
///
/// The distinction is not decoration. A caller enforcing strictness rejects on
/// [`Severity::Violation`] and would reject half the calendars in the world if it also
/// rejected on [`Severity::Note`]; a caller showing a progress bar cares only about
/// [`Severity::LimitReached`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The specification was violated. The input was kept anyway.
    Violation,
    /// Work was cut short at a caller-stated bound, and what was already read is intact.
    ///
    /// This is where the graduated response genuinely lives: a recurrence search that ran
    /// out of candidate budget can be abandoned without losing input, which is exactly what
    /// an oversized value cannot be.
    LimitReached,
}

/// The workspace-wide vocabulary of things that can be wrong.
///
/// Stable in meaning as well as in name. A variant may be added; the meaning of one that
/// exists may not be edited without a rename or a deprecation, because `docs/adr/0006`'s
/// corpus asserts "this input produces this code" across releases and an edited meaning
/// would break that claim while every doc comment sat still.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// Text that a typed view had to decode was not valid UTF-8. The octets are preserved.
    InvalidUtf8Text,
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A content line had an empty property name. A blank line is the degenerate case.
    EmptyPropertyName,
    /// A `BEGIN` or `END` line carried parameters, which a component boundary cannot hold.
    ParametersOnComponentBoundary,
    /// An `END` arrived with no `BEGIN` open.
    UnmatchedEnd,
    /// An `END` named a different component than the `BEGIN` it closed.
    MismatchedEndName,
    /// A `BEGIN` was never closed before the input ended.
    UnclosedComponent,
    /// A line was terminated by a bare `LF`, where RFC 5545 requires `CRLF`.
    BareLineFeed,
    /// A line was terminated by a bare `CR`, where RFC 5545 requires `CRLF`.
    BareCarriageReturn,
    /// The last line of the input carried no terminator at all.
    MissingFinalLineBreak,
    /// A physical line ran past the 75 octets RFC 5545 section 3.1 allows one.
    ///
    /// Counted over the octets of one physical line, terminator excluded, which is what
    /// section 3.1 bounds — a folded content line is as long as its producer wanted and each
    /// of its continuations is a separate physical line with its own answer.
    LineTooLong,
    /// A value or parameter held a control character RFC 5545 section 3.1 excludes.
    ControlCharacterInText,
    /// A `DQUOTE`-delimited parameter value was never closed.
    UnterminatedQuotedParameter,
    /// A `^` was followed by an octet RFC 6868 gives no meaning.
    ///
    /// A note rather than a violation: RFC 6868 section 2 requires such a pair to be left as
    /// it is, so the octets are what they were and the caller is told that a producer may have
    /// meant something by them.
    UndefinedCaretEscape,
    /// A parameter arrived with a name and no `=`.
    ParameterWithoutValue,
    /// A property the specification declares at most once occurred more than once.
    ///
    /// The singular accessor resolves to malformed rather than picking a winner; the
    /// occurrences stay reachable through the general lookup.
    DuplicateProperty,
    /// A `DATE` value did not match RFC 5545 section 3.3.4.
    MalformedDate,
    /// A `DATE-TIME` value did not match RFC 5545 section 3.3.5.
    MalformedDateTime,
    /// A `TIME` value did not match RFC 5545 section 3.3.12.
    MalformedTime,
    /// A `DURATION` value did not match RFC 5545 section 3.3.6.
    MalformedDuration,
    /// A `PERIOD` value did not match RFC 5545 section 3.3.9.
    MalformedPeriod,
    /// A `UTC-OFFSET` value did not match RFC 5545 section 3.3.14.
    MalformedUtcOffset,
    /// A `GEO` value was not the `FLOAT;FLOAT` pair RFC 5545 section 3.8.1.6 requires.
    MalformedGeo,
    /// An `INTEGER` value did not match RFC 5545 section 3.3.8, or did not fit.
    MalformedInteger,
    /// A `FLOAT` value did not match RFC 5545 section 3.3.7.
    MalformedFloat,
    /// A `BOOLEAN` value was neither `TRUE` nor `FALSE` in any casing.
    MalformedBoolean,
    /// A `BINARY` value was not the base 64 RFC 5545 section 3.3.1 requires.
    MalformedBinary,
    /// A `URI` value did not match RFC 5545 section 3.3.13, or section 3.3.3's `CAL-ADDRESS`.
    ///
    /// One code for both, because section 3.3.3 defines a calendar address as a URI and adds
    /// no syntax of its own; what distinguishes them is the property, which the caller holds.
    MalformedUri,
    /// A `VALUE` parameter named a value type this workspace does not know.
    UnknownValueType,
    /// A component did not carry a property RFC 5545 section 3.6 requires of it.
    MissingRequiredProperty,
    /// A component carried a property RFC 5545 section 3.6 does not define for it.
    ///
    /// Never reported for an `X-` name or for one from a later RFC: section 3.8.8 allows both
    /// anywhere, and a component this crate has no definition for allows everything.
    PropertyNotAllowedHere,
    /// A component carried two properties RFC 5545 section 3.6 does not allow together.
    ///
    /// An entailment between two names rather than a count of one, which is why it is a code
    /// of its own: `DTEND` and `DURATION` are each permitted once in a `VEVENT` and forbidden
    /// as a pair, so a reading stated per name has nothing to report about the pair.
    MutuallyExclusiveProperties,
    /// A recurrence search stopped at the candidate budget rather than at the rule's end.
    ///
    /// Reported so that "cut short at the limit" and "the rule ended at `UNTIL`" are
    /// different answers; the second would otherwise arrive dressed as the first.
    RecurrenceBudgetExhausted,
    /// A recurrence search reached the end of the calendar RFC 5545 section 3.3.4 can write
    /// while the rule it was expanding had reached neither its `COUNT` nor its `UNTIL`.
    ///
    /// A note rather than a violation: the file is legal, the answer holds every instance the
    /// calendar can express, and nothing was dropped. What it says is that the series stopped
    /// for a reason outside the rule, so a caller resuming past 9999-12-31 gets nothing however
    /// far it asks — the one terminal state that neither the rule, the window nor the budget
    /// explains.
    RecurrenceCalendarEnded,
    /// A recurrence rule generated an instance whose date does not exist, so it was
    /// filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one.
    NonexistentRecurrenceInstance,
    /// A `RECUR` value did not match the grammar of RFC 5545 section 3.3.10.
    ///
    /// Raised where nothing usable could be read at all — a missing `FREQ`, a `FREQ` naming
    /// no frequency. A part that is merely out of range keeps the rule and travels on
    /// [`DiagnosticCode::RecurrenceRulePartOutOfRange`] instead.
    MalformedRecurrenceRule,
    /// A `RECUR` value named one rule part more than once, which RFC 5545 section 3.3.10
    /// allows at most once.
    ///
    /// The last occurrence wins, because a producer that repeats a part is more plausibly
    /// appending a correction than restating a value it already wrote.
    DuplicateRecurrenceRulePart,
    /// A `RECUR` value named a rule part RFC 5545 section 3.3.10 does not define.
    ///
    /// A note rather than a violation: section 3.3.10's grammar is closed, but an
    /// unrecognized part is indistinguishable from one a later specification added, and
    /// discarding the rest of a rule over it would lose a series this crate can still expand.
    UnknownRecurrenceRulePart,
    /// A `RECUR` rule part carried a value outside the range RFC 5545 section 3.3.10 gives
    /// it, and the rest of the rule was kept.
    ///
    /// `BYMONTHDAY=32` names no day of any month. The part is dropped rather than clamped, for
    /// the reason `docs/adr/0011` gives about instances: a nearby answer is not the answer.
    RecurrenceRulePartOutOfRange,
    /// An `UNTIL` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545
    /// section 3.3.10 requires to agree.
    ///
    /// Reported rather than refused because half the clients in the corpus emit it, and
    /// because the comparison still has to happen in some named clock. Which clock that is
    /// belongs to the caller, who resolved both instants before offering them.
    RecurrenceUntilValueTypeMismatch,
    /// A `RECUR` value carried `BYSETPOS` with no other `BYxxx` rule part to select from,
    /// which RFC 5545 section 3.3.10 forbids.
    BySetPosWithoutByRule,
    /// A `RECUR` value carried both `UNTIL` and `COUNT`, which RFC 5545 section 3.3.10 forbids
    /// in one recur.
    ///
    /// The two name one bound and a rule holds one, so the part written later in the value
    /// wins. That is what a reader applying each pair as it arrives does anyway, and stating it
    /// here makes it a decision rather than an artifact of the walk — but a decision resolved
    /// silently is still a series the caller may not have asked for, which is why it travels.
    MutuallyExclusiveRuleParts,
    /// A component offered more than one `RRULE` and only the first was expanded.
    ///
    /// RFC 5545 section 3.8.5.3 says `SHOULD NOT`, RFC 2445 permitted it, and files with two
    /// exist. Merging them would make `COUNT` ambiguous across the union, so the extras are
    /// dropped loudly rather than silently unioned.
    ExtraRecurrenceRuleIgnored,
    /// An `EXDATE` and a `RECURRENCE-ID` named the same instant, and the exclusion won.
    ///
    /// A note rather than a violation, and scoped to the instant rather than to the override
    /// object: a redundant `EXDATE` landing on a `RANGE=THISANDFUTURE` anchor removes that one
    /// occurrence and leaves the anchor's diff in force for every later candidate.
    ExdateShadowsOverride,
    /// An override moved an occurrence's start outside the window its cadence key fell in.
    ///
    /// Not a defect in the search. A window admits an occurrence whose cadence key falls in it
    /// **or** whose effective start does, so a `THISANDFUTURE` time shift never loses an
    /// occurrence it moved *into* the window and never hides one it moved out: the occurrence is
    /// still emitted, and this says its start is somewhere the caller did not ask about.
    OverrideLeftWindow,
    /// An override moved an occurrence's start off the representable timeline, so the
    /// occurrence was filtered rather than moved to a nearby instant.
    ///
    /// The shift is a number a file supplies and it may name half the timeline, so the sum is
    /// checked. Its own code rather than
    /// [`DiagnosticCode::NonexistentRecurrenceInstance`]: that one names a date RFC 5545
    /// section 3.3.10 defines away, which is a legal file describing fewer instances than it
    /// looks like, and this one names an override asking for an instant no calendar can hold.
    OverrideShiftNotRepresentable,
    /// A `TZID` named a zone no supplied source could resolve.
    UnknownTimeZone,
    /// A `TZID` parameter named a zone with no `VTIMEZONE` in the same calendar.
    MissingTimeZoneDefinition,
    /// A local time occurs twice under its zone, at the end of a daylight saving period.
    AmbiguousLocalTime,
    /// A local time does not occur under its zone, at the start of a daylight saving period.
    NonexistentLocalTime,
    /// An embedded `VTIMEZONE` and the caller's other zone source disagreed about an offset.
    TimeZoneSourceDisagreement,
    /// A `VTIMEZONE` carried neither a `STANDARD` nor a `DAYLIGHT` subcomponent, which RFC 5545 section 3.6.5 requires at least one of.
    ///
    /// The component is kept, because `docs/adr/0001` forbids discarding it, and the zone it
    /// declares answers nothing: a table with no observance has no offset to report and says
    /// so through the absence of an answer rather than through a default of UTC.
    VtimezoneWithoutObservance,
    /// An observance carried an `RRULE` outside the yearly form this crate evaluates in closed form, so no transition was derived from it.
    ///
    /// A note rather than a violation: RFC 5545 section 3.6.5 permits any `RECUR` value on an
    /// observance and the file is legal. What is missing is here, not there. The closed form
    /// covers every rule the tz database and the major producers generate; anything else would
    /// need a search, and a search inside a zone lookup is the unbounded work `docs/adr/0010`
    /// refuses. The observance's own `DTSTART` still stands as one transition.
    VtimezoneRuleUnsupported,
    /// A `VTIMEZONE` declared more observances than the caller's policy admits, and the ones past the bound were dropped.
    ///
    /// Reported so that a zone answered from a truncated table is distinguishable from one
    /// answered from a whole one. A million `RDATE` transitions is a file somebody can write.
    VtimezoneObservancesTruncated,
    /// A calendar declared two `VTIMEZONE` components under one `TZID`, and the second was not admitted.
    ///
    /// The definition is handed back rather than dropped, so a caller that wants the later one
    /// can decide that for itself. Silently preferring either is how a file with two readings
    /// acquires one nobody chose.
    DuplicateTimeZoneIdentifier,
    /// A zone was asked about a time later than the last transition it actually knows, so the answer continues its final observance.
    ///
    /// A note rather than a violation: an embedded `VTIMEZONE` whose transitions are explicit
    /// `RDATE` lines through 2029 is a legal file, and an event in 2035 is a legal event.
    /// Continuing the last observance is the defensible thing for such a source to do and a
    /// dishonest thing to do quietly, which is what this code and the answer's own basis field
    /// exist to prevent between them.
    TimeZoneCoverageExhausted,
    /// An `UNTIL` was written as a local time where RFC 5545 section 3.3.10 requires UTC, and it was read in `DTSTART`'s own zone.
    ///
    /// Distinct from [`DiagnosticCode::RecurrenceUntilValueTypeMismatch`], which is about
    /// `DATE` against `DATE-TIME`: this one is about the clock. Google emits it, the reading
    /// that recovers the producer's intent is `DTSTART`'s own zone, and a series whose end was
    /// guessed rather than read is a fact the caller is owed.
    RecurrenceUntilNotUtc,
    /// An `EXDATE` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.8.5.1 requires to agree.
    ///
    /// The sibling of [`DiagnosticCode::RecurrenceUntilValueTypeMismatch`] on the exclusion
    /// list, and the more damaging of the two: a `DATE` exclusion read at midnight names an
    /// instant a date-timed series does not have, so it removes nothing at all and the
    /// exception the producer wrote disappears without a word.
    ExdateValueTypeMismatch,
    /// A `RECURRENCE-ID` named an instant the series does not generate, so the override modified nothing.
    ///
    /// Clients emit these routinely: an instance is edited, then the rule beneath it is
    /// rewritten, and the override is left addressing a cadence key that no longer exists. The
    /// file then carries a meeting the user sees in the client that wrote it and the expanded
    /// series does not have.
    OverrideMatchesNoInstance,
    /// A zone source recognized a `TZID` and holds no transition for it, so no wall clock names an instant under it.
    ///
    /// The other half of [`DiagnosticCode::UnknownTimeZone`], and a different fact: the
    /// identifier was supplied and the data was not. A `VTIMEZONE` with no usable observance is
    /// how a file produces one, and answering `None` for it — the value a source uses to say
    /// "I have never heard of this identifier" — is what made the two indistinguishable.
    TimeZoneWithoutTransitions,
    /// A zone was asked about a time earlier than the first transition it knows, so the answer continues the offset in force before it.
    ///
    /// [`DiagnosticCode::TimeZoneCoverageExhausted`] at the other end of the table, and a note
    /// for the same reason: a `VTIMEZONE` whose `RDATE` lines begin in 2027 is a legal file and
    /// an event in 2020 is a legal event. The offset reported is the earliest observance's own
    /// `TZOFFSETFROM`, extended backwards, which is all the file states about that era.
    TimeZoneBeforeKnownTransitions,
    /// A calendar declared more `VTIMEZONE` components than the caller's policy admits, and the ones past the bound were dropped.
    ///
    /// The zone-count sibling of [`DiagnosticCode::VtimezoneObservancesTruncated`]. Without it
    /// a refused definition left the identifiers it defines looking like identifiers the file
    /// never defined, which is a claim about the file that the caller's own bound made false.
    VtimezoneComponentsTruncated,
    /// An observance's required value was present and unreadable, so it stated no transition at all.
    ///
    /// `TZOFFSETTO:+9999` is a `UTC-OFFSET` the value layer refuses and RFC 5545 section 3.6's
    /// own audit counts as present, and `DTSTART;VALUE=DATE` inside an observance carries no
    /// hour for a transition to happen at. Neither is a missing property and neither leaves a
    /// usable observance, so without this code a `VTIMEZONE` the file wrote in full was
    /// indistinguishable from one it never wrote.
    VtimezoneObservanceUnreadable,
    /// An `EXDATE` written in UTC named no cadence key because no source recognized the series' zone, so it excluded nothing.
    ///
    /// A `Z`-terminated value on a zoned series has to be placed through the zone's offset at
    /// it, and a series whose `TZID` nothing defines has no offset to place it through. The
    /// exception is kept as the real instant it names, reachable through
    /// `ical_tz::ResolvedExclusions::unplaced`, because an exception that removes nothing and
    /// says nothing is the silent no-op that layer exists to refuse.
    ExdateZoneUnknown,
}

impl DiagnosticCode {
    /// The stable key this code is known by outside the type system.
    ///
    /// This is what the golden list is keyed on and what a conformance case names, so it is
    /// frozen exactly as hard as the variant is. It is not a message: a message is prose
    /// that gets improved, and improving prose must not break a corpus assertion.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUtf8Text => "invalid-utf8-text",
            Self::MissingValueSeparator => "missing-value-separator",
            Self::EmptyPropertyName => "empty-property-name",
            Self::ParametersOnComponentBoundary => "parameters-on-component-boundary",
            Self::UnmatchedEnd => "unmatched-end",
            Self::MismatchedEndName => "mismatched-end-name",
            Self::UnclosedComponent => "unclosed-component",
            Self::BareLineFeed => "bare-line-feed",
            Self::BareCarriageReturn => "bare-carriage-return",
            Self::MissingFinalLineBreak => "missing-final-line-break",
            Self::LineTooLong => "line-too-long",
            Self::ControlCharacterInText => "control-character-in-text",
            Self::UnterminatedQuotedParameter => "unterminated-quoted-parameter",
            Self::UndefinedCaretEscape => "undefined-caret-escape",
            Self::ParameterWithoutValue => "parameter-without-value",
            Self::DuplicateProperty => "duplicate-property",
            Self::MalformedDate => "malformed-date",
            Self::MalformedDateTime => "malformed-date-time",
            Self::MalformedTime => "malformed-time",
            Self::MalformedDuration => "malformed-duration",
            Self::MalformedPeriod => "malformed-period",
            Self::MalformedUtcOffset => "malformed-utc-offset",
            Self::MalformedGeo => "malformed-geo",
            Self::MalformedInteger => "malformed-integer",
            Self::MalformedFloat => "malformed-float",
            Self::MalformedBoolean => "malformed-boolean",
            Self::MalformedBinary => "malformed-binary",
            Self::MalformedUri => "malformed-uri",
            Self::UnknownValueType => "unknown-value-type",
            Self::MissingRequiredProperty => "missing-required-property",
            Self::PropertyNotAllowedHere => "property-not-allowed-here",
            Self::MutuallyExclusiveProperties => "mutually-exclusive-properties",
            Self::RecurrenceBudgetExhausted => "recurrence-budget-exhausted",
            Self::RecurrenceCalendarEnded => "recurrence-calendar-ended",
            Self::NonexistentRecurrenceInstance => "nonexistent-recurrence-instance",
            Self::MalformedRecurrenceRule => "malformed-recurrence-rule",
            Self::DuplicateRecurrenceRulePart => "duplicate-recurrence-rule-part",
            Self::UnknownRecurrenceRulePart => "unknown-recurrence-rule-part",
            Self::RecurrenceRulePartOutOfRange => "recurrence-rule-part-out-of-range",
            Self::RecurrenceUntilValueTypeMismatch => "recurrence-until-value-type-mismatch",
            Self::BySetPosWithoutByRule => "by-set-pos-without-by-rule",
            Self::MutuallyExclusiveRuleParts => "mutually-exclusive-rule-parts",
            Self::ExtraRecurrenceRuleIgnored => "extra-recurrence-rule-ignored",
            Self::ExdateShadowsOverride => "exdate-shadows-override",
            Self::OverrideLeftWindow => "override-left-window",
            Self::OverrideShiftNotRepresentable => "override-shift-not-representable",
            Self::UnknownTimeZone => "unknown-time-zone",
            Self::MissingTimeZoneDefinition => "missing-time-zone-definition",
            Self::AmbiguousLocalTime => "ambiguous-local-time",
            Self::NonexistentLocalTime => "nonexistent-local-time",
            Self::TimeZoneSourceDisagreement => "time-zone-source-disagreement",
            Self::VtimezoneWithoutObservance => "vtimezone-without-observance",
            Self::VtimezoneRuleUnsupported => "vtimezone-rule-unsupported",
            Self::VtimezoneObservancesTruncated => "vtimezone-observances-truncated",
            Self::DuplicateTimeZoneIdentifier => "duplicate-time-zone-identifier",
            Self::TimeZoneCoverageExhausted => "time-zone-coverage-exhausted",
            Self::RecurrenceUntilNotUtc => "recurrence-until-not-utc",
            Self::ExdateValueTypeMismatch => "exdate-value-type-mismatch",
            Self::OverrideMatchesNoInstance => "override-matches-no-instance",
            Self::TimeZoneWithoutTransitions => "time-zone-without-transitions",
            Self::TimeZoneBeforeKnownTransitions => "time-zone-before-known-transitions",
            Self::VtimezoneComponentsTruncated => "vtimezone-components-truncated",
            Self::VtimezoneObservanceUnreadable => "vtimezone-observance-unreadable",
            Self::ExdateZoneUnknown => "exdate-zone-unknown",
        }
    }
}

impl Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The name of the thing a diagnostic is about, carried inline.
///
/// A diagnostic about a `TZID` nothing defines is worth nothing if it does not say which
/// identifier: three undefined zones reported as three equal values tell a caller that
/// something is missing and not what to go and find. A [`Location`] cannot say it — a
/// component that owns unfolded octets has no span back into the caller's buffer — and a
/// borrowed `&str` cannot either, because a `Diagnostic` is `Copy` and outlives the tree it
/// was read from.
///
/// So the name travels by value, in a fixed buffer, and a name longer than
/// [`Subject::CAPACITY`] is kept as its first octets with [`Subject::is_truncated`] saying so.
/// The capacity is a compromise stated rather than hidden: it holds every IANA identifier and
/// `/mozilla.org/20050126_1/Europe/Berlin` besides, and it costs every `Diagnostic` its size
/// whether one is carried or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subject {
    /// The name's first octets, of which the first `len` are live.
    octets: [u8; Subject::CAPACITY],
    /// How many are live.
    len: u8,
    /// Whether the name offered was longer than the buffer.
    truncated: bool,
}

impl Subject {
    /// How many octets of a name are carried.
    pub const CAPACITY: usize = 48;

    /// The name `octets` spells, truncated to [`Subject::CAPACITY`] if it is longer.
    #[must_use]
    pub fn new(octets: &[u8]) -> Self {
        let mut kept = [0_u8; Self::CAPACITY];
        let taken = octets.get(..Self::CAPACITY).unwrap_or(octets);
        let Some(room) = kept.get_mut(..taken.len()) else {
            // Unreachable: `taken` is `octets` clipped to the buffer's own length.
            return Self {
                octets: kept,
                len: 0,
                truncated: true,
            };
        };
        room.copy_from_slice(taken);
        Self {
            octets: kept,
            len: u8::try_from(taken.len()).unwrap_or(0),
            truncated: taken.len() < octets.len(),
        }
    }

    /// The octets of the name, as far as they were kept.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.octets.get(..usize::from(self.len)).unwrap_or(&[])
    }

    /// Whether the name offered was longer than what is carried.
    #[must_use]
    pub const fn is_truncated(self) -> bool {
        self.truncated
    }
}

impl Display for Subject {
    /// The name as text, with any octets that are not UTF-8 written as `U+FFFD`.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        for chunk in self.as_bytes().utf8_chunks() {
            formatter.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                formatter.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

/// One thing that was wrong, and where.
///
/// `Copy` and free of allocation so that a fixed-capacity sink on a device with no allocator
/// stores it by value. `#[non_exhaustive]` so that a field can be added without a major
/// version; the accessors are the API, not the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Diagnostic {
    /// What was wrong.
    code: DiagnosticCode,
    /// How much it claims.
    severity: Severity,
    /// Where in the input, when it is in the input.
    location: Location,
    /// Which occurrence, when it is an occurrence rather than octets.
    instant: Option<Instant>,
    /// What it was about, when a name is what identifies it.
    subject: Option<Subject>,
}

impl Diagnostic {
    /// A diagnostic about octets in the input.
    #[must_use]
    pub const fn new(code: DiagnosticCode, severity: Severity, location: Location) -> Self {
        Self {
            code,
            severity,
            location,
            instant: None,
            subject: None,
        }
    }

    /// A diagnostic about an occurrence that exists at no offset in any file.
    ///
    /// `ical-recur` and `ical-tz` report about expanded instances and zone transitions.
    /// Without this constructor each would have invented a reporting channel of its own, and
    /// a caller would have three places to look instead of one sink.
    #[must_use]
    pub const fn at_instant(code: DiagnosticCode, severity: Severity, instant: Instant) -> Self {
        Self {
            code,
            severity,
            location: Location::NOWHERE,
            instant: Some(instant),
            subject: None,
        }
    }

    /// The same diagnostic, saying what it is about.
    ///
    /// A builder rather than a fourth constructor, because the subject is orthogonal to where
    /// a diagnostic points: an identifier can be named about octets, about an occurrence, and
    /// about neither.
    #[must_use]
    pub const fn about(self, subject: Subject) -> Self {
        Self {
            subject: Some(subject),
            ..self
        }
    }

    /// What was wrong.
    #[must_use]
    pub const fn code(self) -> DiagnosticCode {
        self.code
    }

    /// How much this diagnostic claims.
    #[must_use]
    pub const fn severity(self) -> Severity {
        self.severity
    }

    /// Where in the input, [`Location::NOWHERE`] when it is not in the input.
    #[must_use]
    pub const fn location(self) -> Location {
        self.location
    }

    /// The occurrence concerned, when the diagnostic is about one.
    #[must_use]
    pub const fn instant(self) -> Option<Instant> {
        self.instant
    }

    /// What the diagnostic is about, when a name is what identifies it.
    #[must_use]
    pub const fn subject(self) -> Option<Subject> {
        self.subject
    }
}

impl Display for Diagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())?;
        if let Some(subject) = self.subject {
            write!(formatter, " about {subject}")?;
        }
        match self.location.span() {
            Some(span) => write!(formatter, " at octets {}..{}", span.start(), span.end()),
            None => match self.instant {
                Some(instant) => write!(formatter, " at instant {}", instant.unix_seconds()),
                None => Ok(()),
            },
        }
    }
}

/// Whether a sink took the diagnostic it was handed.
///
/// Refusal is a normal answer, not a failure. It is also not the reader's business: the
/// reader keeps reading either way, which is what makes "a violation never discards the
/// file" hold with no allocator linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SinkOutcome {
    /// The sink kept the diagnostic.
    Accepted,
    /// The sink dropped it. The count of drops lives outside the sink.
    Refused,
}

impl SinkOutcome {
    /// Whether the sink kept the diagnostic.
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// Where diagnostics go.
///
/// Push-only and object-safe, so `&mut dyn DiagnosticSink` is a legal argument: an
/// allocating caller passes `&mut Vec<Diagnostic>`, and a caller with no allocator passes
/// [`IgnoreDiagnostics`] or a fixed-capacity sink of its own.
pub trait DiagnosticSink {
    /// Offer a diagnostic to the sink, which may refuse it.
    fn push(&mut self, diagnostic: Diagnostic) -> SinkOutcome;
}

impl DiagnosticSink for Vec<Diagnostic> {
    fn push(&mut self, diagnostic: Diagnostic) -> SinkOutcome {
        Self::push(self, diagnostic);
        SinkOutcome::Accepted
    }
}

impl<S: DiagnosticSink + ?Sized> DiagnosticSink for &mut S {
    fn push(&mut self, diagnostic: Diagnostic) -> SinkOutcome {
        (**self).push(diagnostic)
    }
}

/// A sink that keeps nothing.
///
/// The honest shape for a caller that wants the model and not the report. It still counts,
/// because [`report_diagnostic`] charges every refusal to the meter — a caller using this
/// loses which violations occurred, never that they did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IgnoreDiagnostics;

impl DiagnosticSink for IgnoreDiagnostics {
    fn push(&mut self, _diagnostic: Diagnostic) -> SinkOutcome {
        SinkOutcome::Refused
    }
}

/// Offer a diagnostic to `sink`, charging a refusal to `meter`.
///
/// Every emission site in this workspace goes through this function rather than calling
/// [`DiagnosticSink::push`] directly, because the count of refused diagnostics has to live
/// outside the sink — a sink that keeps nothing cannot also remember how much it did not
/// keep. There is no return value on purpose: a reader has nothing to decide here, and a
/// reader that could branch on refusal is a reader that could stop reading because a buffer
/// filled up.
pub fn report_diagnostic<S>(sink: &mut S, meter: &mut Meter, diagnostic: Diagnostic)
where
    S: DiagnosticSink + ?Sized,
{
    if sink.push(diagnostic) == SinkOutcome::Refused {
        meter.note_dropped_diagnostic();
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec::Vec;

    use super::{
        Diagnostic, DiagnosticCode, DiagnosticSink, IgnoreDiagnostics, Severity, SinkOutcome,
        Subject, report_diagnostic,
    };
    use crate::budget::{Limits, Meter};
    use crate::instant::Instant;
    use crate::location::Location;

    #[test]
    fn every_code_has_a_distinct_stable_key() {
        let codes = [
            DiagnosticCode::InvalidUtf8Text,
            DiagnosticCode::MissingValueSeparator,
            DiagnosticCode::UnmatchedEnd,
            DiagnosticCode::MismatchedEndName,
            DiagnosticCode::UnclosedComponent,
        ];
        let mut keys: Vec<&str> = codes.iter().map(|code| code.as_str()).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "two codes share a golden-list key");
    }

    /// A name is what tells two reports of one code apart, so it has to survive into the value
    /// a sink stores and to say when it did not survive whole.
    #[test]
    fn a_diagnostic_can_name_what_it_is_about() {
        let missing = |zone: &str| {
            Diagnostic::new(
                DiagnosticCode::MissingTimeZoneDefinition,
                Severity::Violation,
                Location::NOWHERE,
            )
            .about(Subject::new(zone.as_bytes()))
        };
        let denver = missing("America/Denver");
        let kolkata = missing("Asia/Kolkata");
        assert_ne!(
            denver, kolkata,
            "two undefined zones may not arrive as two equal values"
        );
        assert_eq!(
            denver.subject().map(|named| named.as_bytes().to_vec()),
            Some(b"America/Denver".to_vec())
        );
        assert!(format!("{denver}").contains("America/Denver"));
        assert!(denver.subject().is_some_and(|named| !named.is_truncated()));

        let long = "/mozilla.org/20050126_1/Europe/Berlin/and/then/some/more/of/it";
        let overflowing = Subject::new(long.as_bytes());
        assert!(overflowing.is_truncated(), "a longer name says it was cut");
        assert_eq!(overflowing.as_bytes().len(), Subject::CAPACITY);
        assert_eq!(
            Diagnostic::new(
                DiagnosticCode::UnknownTimeZone,
                Severity::Violation,
                Location::NOWHERE
            )
            .subject(),
            None,
            "a diagnostic about nothing in particular names nothing"
        );
    }

    #[test]
    fn a_diagnostic_about_an_occurrence_has_no_span() {
        let diagnostic = Diagnostic::at_instant(
            DiagnosticCode::NonexistentLocalTime,
            Severity::Violation,
            Instant::from_unix_seconds(1_700_000_000),
        );
        assert_eq!(diagnostic.location(), Location::NOWHERE);
        assert!(diagnostic.instant().is_some());
        assert!(format!("{diagnostic}").contains("1700000000"));
    }

    #[test]
    fn a_vec_accepts_and_the_ignoring_sink_refuses() {
        let diagnostic = Diagnostic::new(
            DiagnosticCode::BareLineFeed,
            Severity::Violation,
            Location::at_offset(12),
        );
        let mut kept: Vec<Diagnostic> = Vec::new();
        assert_eq!(
            DiagnosticSink::push(&mut kept, diagnostic),
            SinkOutcome::Accepted
        );
        assert_eq!(IgnoreDiagnostics.push(diagnostic), SinkOutcome::Refused);
    }

    #[test]
    fn a_refusal_is_counted_outside_the_sink() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let diagnostic = Diagnostic::new(
            DiagnosticCode::BareLineFeed,
            Severity::Violation,
            Location::at_offset(0),
        );
        report_diagnostic(&mut IgnoreDiagnostics, &mut meter, diagnostic);
        report_diagnostic(&mut IgnoreDiagnostics, &mut meter, diagnostic);
        assert_eq!(meter.diagnostics_dropped(), 2);

        let mut kept: Vec<Diagnostic> = Vec::new();
        report_diagnostic(&mut kept, &mut meter, diagnostic);
        assert_eq!(
            meter.diagnostics_dropped(),
            2,
            "an accepted diagnostic is not a drop"
        );
        assert_eq!(kept.len(), 1);
    }
}
