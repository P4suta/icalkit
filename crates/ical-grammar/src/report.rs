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
    /// A recurrence rule generated an instance whose date does not exist, so it was
    /// filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one.
    NonexistentRecurrenceInstance,
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
            Self::NonexistentRecurrenceInstance => "nonexistent-recurrence-instance",
            Self::UnknownTimeZone => "unknown-time-zone",
            Self::MissingTimeZoneDefinition => "missing-time-zone-definition",
            Self::AmbiguousLocalTime => "ambiguous-local-time",
            Self::NonexistentLocalTime => "nonexistent-local-time",
            Self::TimeZoneSourceDisagreement => "time-zone-source-disagreement",
        }
    }
}

impl Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
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
}

impl Display for Diagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())?;
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
        report_diagnostic,
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
