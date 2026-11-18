// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Diagnostics describe input that the reader keeps.
//!
//! The workspace shares one vocabulary under `docs/adr/0004` and one code registry under `docs/adr/0009`.
//! A fixed-capacity sink can refuse a diagnostic and [`IgnoreDiagnostics`] refuses every diagnostic.
//! The reader continues after a refusal.
//! [`report_diagnostic`] charges each refusal to the caller's meter.

use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter};

use super::{Instant, Location, Meter};

// Named only by this file's tests, for the reason `budget.rs` states beside the same import.
#[cfg(test)]
use super::Limits;

/// The significance of a diagnostic.
/// Strict validation can reject a [`Severity::Violation`].
/// A [`Severity::Note`] describes an observation and [`Severity::LimitReached`] identifies a resource limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The input violates the specification.
    /// The reader keeps the input.
    Violation,
    /// A caller limit stopped further work.
    /// The reader keeps the input it has read.
    LimitReached,
}

/// Stable diagnostic codes for the workspace.
///
/// New codes can extend the vocabulary.
/// Changing a code's meaning requires renaming or deprecating it under `docs/adr/0009`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// A typed view encountered invalid UTF-8 and kept the original octets.
    InvalidUtf8Text,
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A content line had an empty property name.
    /// A blank line is the degenerate case.
    EmptyPropertyName,
    /// A `BEGIN` or `END` line carried parameters that the component boundary syntax forbids.
    ParametersOnComponentBoundary,
    /// An `END` arrived with no `BEGIN` open.
    UnmatchedEnd,
    /// An `END` named a different component than the `BEGIN` it closed.
    MismatchedEndName,
    /// A `BEGIN` was never closed before the input ended.
    UnclosedComponent,
    /// A line ended with a bare `LF` instead of the `CRLF` that RFC 5545 requires.
    BareLineFeed,
    /// A line ended with a bare `CR` instead of the `CRLF` that RFC 5545 requires.
    BareCarriageReturn,
    /// The last line of the input carried no terminator at all.
    MissingFinalLineBreak,
    /// A physical line exceeded the 75 octets that RFC 5545 section 3.1 recommends.
    ///
    /// The count excludes the terminator and treats each folded segment as a separate physical line.
    LineTooLong,
    /// A value or parameter held a control character RFC 5545 section 3.1 excludes.
    ControlCharacterInText,
    /// A `DQUOTE`-delimited parameter value was never closed.
    UnterminatedQuotedParameter,
    /// An octet following `^` had no meaning under RFC 6868.
    ///
    /// Section 2 requires the reader to keep both octets.
    UndefinedCaretEscape,
    /// A parameter arrived with a name and no `=`.
    ParameterWithoutValue,
    /// A property the specification declares at most once occurred more than once.
    ///
    /// The singular accessor reports a malformed value.
    /// The general lookup returns every occurrence.
    DuplicateProperty,
    /// A `DATE` value violated RFC 5545 section 3.3.4.
    MalformedDate,
    /// A `DATE-TIME` value violated RFC 5545 section 3.3.5.
    MalformedDateTime,
    /// A `TIME` value violated RFC 5545 section 3.3.12.
    MalformedTime,
    /// A `DURATION` value violated RFC 5545 section 3.3.6.
    MalformedDuration,
    /// A `PERIOD` value violated RFC 5545 section 3.3.9.
    MalformedPeriod,
    /// A `UTC-OFFSET` value violated RFC 5545 section 3.3.14.
    MalformedUtcOffset,
    /// A `GEO` value lacked the valid `FLOAT;FLOAT` pair that RFC 5545 section 3.8.1.6 requires.
    MalformedGeo,
    /// An `INTEGER` value exceeded the supported range or fell outside the syntax of RFC 5545 section 3.3.8.
    MalformedInteger,
    /// A `FLOAT` value violated RFC 5545 section 3.3.7.
    MalformedFloat,
    /// A `BOOLEAN` value was neither `TRUE` nor `FALSE` in any casing.
    MalformedBoolean,
    /// A `BINARY` value had invalid base64 encoding under RFC 5545 section 3.3.1.
    MalformedBinary,
    /// A `URI` or `CAL-ADDRESS` value had invalid URI syntax under RFC 5545 sections 3.3.13 and 3.3.3.
    ///
    /// Calendar addresses share the syntax for resource identifiers.
    MalformedUri,
    /// A property declared an empty, repeated, or malformed `VALUE` parameter.
    MalformedValueTypeParameter,
    /// A `VALUE` parameter named an unknown value type.
    UnknownValueType,
    /// A component lacked a property that RFC 5545 section 3.6 requires.
    MissingRequiredProperty,
    /// A component carried a property outside its definition in RFC 5545 section 3.6.
    ///
    /// The audit skips extension names, properties from later specifications, and unfamiliar component types.
    PropertyNotAllowedHere,
    /// A component carried a pair of properties that RFC 5545 section 3.6 forbids together.
    ///
    /// An event can have `DTEND` or `DURATION`.
    MutuallyExclusiveProperties,
    /// An earlier resource refusal prevented further admission.
    ResourceBudgetExhausted,
    /// A recurrence search stopped at the candidate budget rather than at the rule's end.
    ///
    /// The search can reach its candidate limit before it reaches `UNTIL`.
    RecurrenceBudgetExhausted,
    /// The recurrence search reached the last representable date before the rule ended.
    ///
    /// RFC 5545 section 3.3.4 limits the civil year even for a rule with a larger `COUNT` or no endpoint.
    RecurrenceCalendarEnded,
    /// A recurrence rule generated a nonexistent date, so expansion skipped that instance under RFC 5545 section 3.3.10.
    NonexistentRecurrenceInstance,
    /// A `RECUR` value had no usable rule under RFC 5545 section 3.3.10.
    ///
    /// A missing or unrecognized `FREQ` leaves no frequency to expand.
    /// An invalid part of an otherwise usable rule has a separate diagnostic.
    MalformedRecurrenceRule,
    /// A `RECUR` value repeated a rule part that RFC 5545 section 3.3.10 permits once.
    ///
    /// The last occurrence wins.
    DuplicateRecurrenceRulePart,
    /// A `RECUR` value named a rule part outside RFC 5545 section 3.3.10.
    ///
    /// The reader keeps the rest of the rule.
    UnknownRecurrenceRulePart,
    /// A `RECUR` value requires unsupported `RSCALE` or `SKIP` semantics.
    UnsupportedRecurrenceSemantics,
    /// A `RECUR` value included a part outside its RFC 5545 section 3.3.10 range, and the reader kept the remaining parts.
    ///
    /// For example, `BYMONTHDAY=32` names no day in a month.
    RecurrenceRulePartOutOfRange,
    /// An `UNTIL` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.3.10 requires to agree.
    ///
    /// The caller supplies the resolved instants for comparison.
    RecurrenceUntilValueTypeMismatch,
    /// A `RECUR` value included `BYSETPOS` without another `BYxxx` part, contrary to RFC 5545 section 3.3.10.
    BySetPosWithoutByRule,
    /// A `RECUR` value carried both `UNTIL` and `COUNT`, which RFC 5545 section 3.3.10 forbids in one recur.
    ///
    /// The later part supplies the limit.
    MutuallyExclusiveRuleParts,
    /// A component supplied more than one `RRULE` property, and expansion used the first.
    ///
    /// RFC 5545 section 3.8.5.3 discourages extra rules.
    /// RFC 2445 permitted them.
    ExtraRecurrenceRuleIgnored,
    /// An `EXDATE` and a `RECURRENCE-ID` named the same instant, and the exclusion won.
    ///
    /// Excluding a `RANGE=THISANDFUTURE` anchor removes one occurrence.
    /// Its changes still apply to later candidates.
    ExdateShadowsOverride,
    /// An override moved an occurrence's start outside the window its cadence key fell in.
    ///
    /// The window includes an occurrence when either its cadence key or its effective start falls inside.
    OverrideLeftWindow,
    /// An override shifted an occurrence outside the representable timeline, so expansion skipped that occurrence.
    ///
    /// [`DiagnosticCode::NonexistentRecurrenceInstance`] identifies invalid calendar dates generated by a rule.
    OverrideShiftNotRepresentable,
    /// A `TZID` named a zone no supplied source could resolve.
    UnknownTimeZone,
    /// A `TZID` parameter named a zone with no `VTIMEZONE` in the same calendar.
    MissingTimeZoneDefinition,
    /// A local time occurs twice under its zone, at the end of a daylight saving period.
    AmbiguousLocalTime,
    /// A local time fell inside a gap at the start of a daylight saving period.
    NonexistentLocalTime,
    /// An embedded `VTIMEZONE` and the caller's other zone source disagreed about an offset.
    TimeZoneSourceDisagreement,
    /// A `VTIMEZONE` carried neither a `STANDARD` nor a `DAYLIGHT` subcomponent, which RFC 5545 section 3.6.5 requires at least one of.
    ///
    /// The reader keeps the component and its zone identifier.
    VtimezoneWithoutObservance,
    /// An observance supplied an `RRULE` outside the supported yearly forms, so the reader derived no transition from that rule.
    ///
    /// RFC 5545 section 3.6.5 permits other recurrence forms.
    VtimezoneRuleUnsupported,
    /// A `VTIMEZONE` exceeded the caller's observance limit, so the reader omitted the excess observances.
    VtimezoneObservancesTruncated,
    /// More than one `VTIMEZONE` declared the same `TZID`.
    ///
    /// Lookup selects the first definition with a transition or the first empty definition if all are empty.
    /// Every definition remains available in source order.
    DuplicateTimeZoneIdentifier,
    /// A lookup beyond the final known transition continued the zone's final observance.
    TimeZoneCoverageExhausted,
    /// A local `UNTIL` used the `DTSTART` zone where RFC 5545 section 3.3.10 requires UTC.
    ///
    /// [`DiagnosticCode::RecurrenceUntilValueTypeMismatch`] identifies a difference in value types.
    RecurrenceUntilNotUtc,
    /// An `EXDATE` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.8.5.1 requires to agree.
    ///
    /// The caller's exclusion policy determines how a date applies to a date-time series.
    ExdateValueTypeMismatch,
    /// A `RECURRENCE-ID` had no matching series occurrence, so the override changed no instance.
    OverrideMatchesNoInstance,
    /// A zone source recognized a `TZID` and holds no transition for it, so no wall clock names an instant under it.
    ///
    /// An empty `VTIMEZONE` recognizes its identifier but supplies no offset.
    TimeZoneWithoutTransitions,
    /// A lookup before the first known transition continued the offset in force before that transition.
    ///
    /// The earliest observance supplies `TZOFFSETFROM`.
    TimeZoneBeforeKnownTransitions,
    /// A zone provider returned an answer without claiming complete coverage for the requested time.
    TimeZoneCoverageUnasserted,
    /// A calendar exceeded the caller's `VTIMEZONE` count limit, so the reader omitted the excess definitions.
    ///
    /// A refused definition still identifies a zone that the input supplied.
    VtimezoneComponentsTruncated,
    /// An observance's required value was present and unreadable, so it stated no transition at all.
    ///
    /// A malformed `TZOFFSETTO` or a `DTSTART` with only a date leaves no usable transition.
    VtimezoneObservanceUnreadable,
    /// An `EXDATE` written in UTC named no cadence key because no source recognized the series' zone, so it excluded nothing.
    ///
    /// The caller can recover the UTC instant from `ResolvedExclusions::unplaced`.
    ExdateZoneUnknown,
    /// The zone provider found an unreadable boundary for a whole-day `EXDATE`.
    ExdateBoundaryUnreadable,
    /// A `METHOD` was present and named no method RFC 5546 defines, so the message states no scheduling semantics at all.
    ///
    /// A calendar without `METHOD` is a distinct case.
    SchedulingMethodUnknown,
    /// An `ORGANIZER` or `ATTENDEE` had an unreadable `CAL-ADDRESS` and identified no party.
    SchedulingCalendarAddressUnreadable,
    /// A `SEQUENCE` value had no integer revision number.
    ///
    /// RFC 5546 section 3.2 assigns zero only when the property is absent.
    SchedulingSequenceUnreadable,
    /// A scheduling payload carried a property RFC 5546 section 3 forbids for its `METHOD` and component type.
    ///
    /// The method constraint table assigns the property a presence of zero.
    /// Authorization decides whether to accept the message.
    SchedulingPropertyNotAllowed,
    /// A scheduling payload lacked a property RFC 5546 section 3 requires for its `METHOD` and component type.
    ///
    /// The presence `1` and `1+` rows of the section 3 constraint tables, and the sibling of [`DiagnosticCode::SchedulingPropertyNotAllowed`] at the other end of the same row.
    SchedulingRequiredPropertyMissing,
    /// A `CANCEL` payload carried `STATUS` with a value other than `CANCELLED`.
    ///
    /// RFC 5546 sections 3.2.5, 3.4.5, and 3.5.3 make this a method-specific value constraint rather than an iCalendar value-type error.
    SchedulingCancellationStatusInvalid,
    /// A `RECURRENCE-ID` named a wall clock its series' zone repeats, and nothing said which of the two instants it addresses.
    ///
    /// A fold can map one nominal recurrence key to two instants.
    SchedulingInstanceAmbiguous,
    /// A `RECURRENCE-ID` carried `RANGE=THISANDFUTURE` under a `METHOD` that RFC 5546 forbids for that range.
    ///
    /// For example, a `REPLY` addresses one instance.
    SchedulingRangeNotPermitted,
    /// A scheduling message addressed a component with an unplaced exclusion, leaving its instance set unresolved.
    ///
    /// A caller can inspect the unresolved keys in `ResolvedExclusions::unplaced`.
    SchedulingExclusionUnplaced,
    /// An instance identity used a zone answer that continued an observance beyond the source's known transitions.
    ///
    /// The diagnostic identifies uncertainty in the addressed instance.
    SchedulingZoneContinued,
    /// The sender lacked permission to send the message's `METHOD` under RFC 5546 section 3.
    ///
    /// For example, an attendee sent a `CANCEL` or an organizer sent a `REPLY`.
    SchedulingSenderNotPermitted,
    /// A calendar stated more than one `METHOD`, so the verb of the whole message is two claims rather than one.
    ///
    /// RFC 5545 section 3.7.2 permits one `METHOD` per calendar.
    SchedulingMethodAmbiguous,
    /// A `RECURRENCE-ID` fell in a local time gap, and the caller's policy dropped the instance.
    ///
    /// A gap names no instant until the caller chooses a reading.
    SchedulingInstanceNonexistent,
    /// The reader skipped an unfamiliar element outside the `DAV:` and CalDAV vocabulary and skipped its descendants.
    ///
    /// RFC 4918 section 17 permits extensions.
    DavForeignElementSkipped,
    /// The reader copied a `calendar-data` payload that it couldn't borrow from the body.
    ///
    /// A character reference or a `CDATA` boundary can require an allocation to assemble the payload.
    DavCalendarDataCopied,
    /// A `calendar-data` payload lost carriage returns to XML 1.0 section 2.11 line-ending normalization.
    ///
    /// RFC 4791 section 9.6 permits normalization.
    /// Writing the result back can change the stored line endings and the `ETag`.
    ///
    /// The diagnostic also applies to `CALDAV:calendar-timezone` and `CALDAV:timezone` under RFC 4791 sections 5.2.2 and 9.5.
    DavCalendarDataLineEndingsFolded,
    /// The reader kept the octets of a property with an unmodeled value.
    DavPropertyUnmodeled,
    /// A `DAV:status` element lacked a valid status line under RFC 4918 section 14.28.
    ///
    /// The response leaves the outcome unknown for the affected properties.
    DavStatusUnreadable,
    /// A `DAV:response` carried no `href`, so it names no resource.
    ///
    /// RFC 4918 section 14.24 requires at least one `href` to identify the resource.
    DavResponseWithoutHref,
    /// A multistatus exceeded the caller's response limit, so the reader omitted the excess responses.
    DavResponsesTruncated,
    /// A property mixed text and child elements, and the reader dropped the child markup.
    ///
    /// Earlier readers emitted this diagnostic.
    /// The current reader preserves mixed content in document order.
    DavPropertyMarkupDropped,
    /// The reader withheld a synchronization token because the response was incomplete.
    ///
    /// RFC 6578 section 3.4 ties the token to the complete report.
    /// Saving a token from a partial report could skip changes.
    DavSyncTokenWithheld,
    /// A `calendar-data` selection produced a reduced or expanded representation of the stored calendar.
    ///
    /// A client that writes the result back can overwrite information it didn't request.
    QueryCalendarDataReduced,
    /// The reader lacked enough information to decide whether the resource matched the filter.
    ///
    /// For example, a floating start needs a zone for comparison with a UTC range.
    QueryFilterUndecidable,
}

impl DiagnosticCode {
    /// The stable registry key for this diagnostic.
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
            Self::MalformedValueTypeParameter => "malformed-value-type-parameter",
            Self::UnknownValueType => "unknown-value-type",
            Self::MissingRequiredProperty => "missing-required-property",
            Self::PropertyNotAllowedHere => "property-not-allowed-here",
            Self::MutuallyExclusiveProperties => "mutually-exclusive-properties",
            Self::ResourceBudgetExhausted => "resource-budget-exhausted",
            Self::RecurrenceBudgetExhausted => "recurrence-budget-exhausted",
            Self::RecurrenceCalendarEnded => "recurrence-calendar-ended",
            Self::NonexistentRecurrenceInstance => "nonexistent-recurrence-instance",
            Self::MalformedRecurrenceRule => "malformed-recurrence-rule",
            Self::DuplicateRecurrenceRulePart => "duplicate-recurrence-rule-part",
            Self::UnknownRecurrenceRulePart => "unknown-recurrence-rule-part",
            Self::UnsupportedRecurrenceSemantics => "unsupported-recurrence-semantics",
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
            Self::TimeZoneCoverageUnasserted => "time-zone-coverage-unasserted",
            Self::VtimezoneComponentsTruncated => "vtimezone-components-truncated",
            Self::VtimezoneObservanceUnreadable => "vtimezone-observance-unreadable",
            Self::ExdateZoneUnknown => "exdate-zone-unknown",
            Self::ExdateBoundaryUnreadable => "exdate-boundary-unreadable",
            Self::SchedulingMethodUnknown => "scheduling-method-unknown",
            Self::SchedulingCalendarAddressUnreadable => "scheduling-calendar-address-unreadable",
            Self::SchedulingSequenceUnreadable => "scheduling-sequence-unreadable",
            Self::SchedulingPropertyNotAllowed => "scheduling-property-not-allowed",
            Self::SchedulingRequiredPropertyMissing => "scheduling-required-property-missing",
            Self::SchedulingCancellationStatusInvalid => "scheduling-cancellation-status-invalid",
            Self::SchedulingInstanceAmbiguous => "scheduling-instance-ambiguous",
            Self::SchedulingRangeNotPermitted => "scheduling-range-not-permitted",
            Self::SchedulingExclusionUnplaced => "scheduling-exclusion-unplaced",
            Self::SchedulingZoneContinued => "scheduling-zone-continued",
            Self::SchedulingSenderNotPermitted => "scheduling-sender-not-permitted",
            Self::SchedulingMethodAmbiguous => "scheduling-method-ambiguous",
            Self::SchedulingInstanceNonexistent => "scheduling-instance-nonexistent",
            Self::DavForeignElementSkipped => "dav-foreign-element-skipped",
            Self::DavCalendarDataCopied => "dav-calendar-data-copied",
            Self::DavCalendarDataLineEndingsFolded => "dav-calendar-data-line-endings-folded",
            Self::DavPropertyUnmodeled => "dav-property-unmodeled",
            Self::DavStatusUnreadable => "dav-status-unreadable",
            Self::DavResponseWithoutHref => "dav-response-without-href",
            Self::DavResponsesTruncated => "dav-responses-truncated",
            Self::DavPropertyMarkupDropped => "dav-property-markup-dropped",
            Self::DavSyncTokenWithheld => "dav-sync-token-withheld",
            Self::QueryCalendarDataReduced => "query-calendar-data-reduced",
            Self::QueryFilterUndecidable => "query-filter-undecidable",
        }
    }
}

impl Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The inline name of a diagnostic subject.
///
/// A subject identifies a name such as a `TZID` even when the input has no usable byte span.
/// The buffer holds the first [`Subject::CAPACITY`] octets.
/// [`Subject::is_truncated`] reports whether the name extends beyond the buffer.
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
    /// Number of name octets the subject can store.
    pub const CAPACITY: usize = 64;

    /// Store the first [`Subject::CAPACITY`] octets of the name.
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

    /// The name octets in the buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.octets.get(..usize::from(self.len)).unwrap_or(&[])
    }

    /// Whether the name exceeds the buffer capacity.
    #[must_use]
    pub const fn is_truncated(self) -> bool {
        self.truncated
    }
}

impl Display for Subject {
    /// Display invalid UTF-8 octets with replacement characters.
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

/// A diagnostic code, severity, and location.
///
/// A fixed-capacity sink can store the value without allocation.
/// Use the constructors so future versions can add fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Diagnostic {
    /// What was wrong.
    code: DiagnosticCode,
    /// How much it claims.
    severity: Severity,
    /// The location within the input.
    location: Location,
    /// The occurrence instant, when applicable.
    instant: Option<Instant>,
    /// The subject name, when applicable.
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
    /// Without this constructor each would have invented a reporting channel of its own, and a caller would have three places to look instead of one sink.
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

    /// Attach a subject name while preserving the diagnostic's location and instant.
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

    /// The input location, or [`Location::NOWHERE`] for a diagnostic without an input location.
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

/// Whether the sink accepted the diagnostic.
/// Readers continue when the sink refuses a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SinkOutcome {
    /// The sink kept the diagnostic.
    Accepted,
    /// The sink dropped it.
    /// The count of drops lives outside the sink.
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
/// Push-only and object-safe, so `&mut dyn DiagnosticSink` is a legal argument: an allocating caller passes `&mut Vec<Diagnostic>`, and a caller with no allocator passes [`IgnoreDiagnostics`] or a fixed-capacity sink of its own.
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

/// A sink that declines every diagnostic.
/// [`report_diagnostic`] still counts each refusal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IgnoreDiagnostics;

impl DiagnosticSink for IgnoreDiagnostics {
    fn push(&mut self, _diagnostic: Diagnostic) -> SinkOutcome {
        SinkOutcome::Refused
    }
}

/// Offer a diagnostic to `sink` and charge any refusal to `meter`.
/// Readers continue regardless of the sink's capacity.
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
        Diagnostic, DiagnosticCode, DiagnosticSink, IgnoreDiagnostics, Instant, Limits, Location,
        Meter, Severity, SinkOutcome, Subject, report_diagnostic,
    };

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

    /// Subject names distinguish diagnostics that share a code.
    /// Truncation remains visible to the caller.
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

        let long = [b'x'; Subject::CAPACITY + 1];
        let overflowing = Subject::new(&long);
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
