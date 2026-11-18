// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Read time zone definitions under RFC 5545 section 3.6.5.
//!
//! A `STANDARD` or `DAYLIGHT` component supplies its `DTSTART` anchor and explicit `RDATE` transitions.
//! RFC 5545 sections 3.8.3.3 and 3.8.3.4 define the offsets.
//! Sections 3.8.5.2 and 3.8.5.3 define recurrence dates and rules.
//! An annual clock can use up to 366 calendar patterns or 372 week patterns.
//! Explicit dates and computed onsets contribute to the same transition set.
//!
//! Definition identifiers use decoded `TEXT` values and preserve literal quotes.
//! Lookup preserves case and vendor prefixes as `docs/adr/0003` requires.
//! A definition needs one nonempty identifier in valid UTF-8.
//!
//! [`Component::audit`] reports absent required properties.
//! An observance with all required properties but no usable transition produces `vtimezone-observance-unreadable`.
//! An unsupported rule leaves its anchor and explicit dates available.
//! A zone without observance subcomponents keeps its identifier but has no transition data.
//!
//! The caller's limits bound zone definitions and observances.
//! Staging keeps the earliest patterns within the remaining budget and one overflow witness.
//! [`TransitionTable::new`] charges admitted observances and reports truncation.
//! The reader uses an explicit worklist to find undefined identifiers throughout nested components.
//!
//! [`Component::audit`]: crate::internal::core::Component::audit
//!
//! [`TransitionTable::new`]: crate::internal::tz::TransitionTable::new

use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::num::{NonZeroU8, NonZeroU32};
use core::str;

use crate::internal::core::codec::decode_single_scalar_text;
use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Component, DateTimeValue, DecodeValue as _, Diagnostic,
    DiagnosticCode, DiagnosticSink, Location, Meter, PropertyId, Severity, Subject, UtcOffset,
    ValueType, Weekday, report_diagnostic,
};

use crate::internal::tz::annual_clocks::{
    self, ClockLimit, ClockRules, ClockSet, read_clock_field,
};
use crate::internal::tz::annual_counts::{CountEnd, counted_end};
use crate::internal::tz::annual_dates::{DateSelectors, MonthSet, YearDateMasks};
use crate::internal::tz::annual_positions::AnnualPositions;
use crate::internal::tz::earliest_observances::EarliestObservances;
use crate::internal::tz::model::{
    NthWeek, Observance, ObservanceReader, RuleDay, StagedDefinition, TransitionTable,
    VtimezoneSet, YearlyRule, ZoneSetError,
};
use crate::internal::tz::month_days::MonthDaySet;
use crate::internal::tz::week_years::{WeekNumbers, WeekYearMasks};
use crate::internal::tz::weekdays::{WeekdayScope, WeekdaySet, WeekdayTerm};
use crate::internal::tz::year_intervals::YearInterval;

/// A weekday window spans seven consecutive `BYMONTHDAY` values.
const RUN_LENGTH: u32 = 7;

/// Read the calendar's `VTIMEZONE` definitions and report undefined identifiers.
///
/// The set compares identifiers by their exact bytes.
/// [`VtimezoneSet::definitions`] exposes duplicate definitions, and the reader reports [`DiagnosticCode::DuplicateTimeZoneIdentifier`].
/// The reader reports each undefined identifier once.
///
/// The meter bounds both zone and observance counts.
/// A zone-count refusal names the definition and prevents a second diagnostic for an undefined identifier.
///
/// [`VtimezoneSet::definitions`]: crate::internal::tz::VtimezoneSet::definitions
#[must_use]
pub fn read_calendar_zones<S: DiagnosticSink + ?Sized>(
    calendar: &Component,
    meter: &mut Meter,
    sink: &mut S,
) -> VtimezoneSet {
    let mut zones = VtimezoneSet::new();
    let mut refused: BTreeSet<Box<str>> = BTreeSet::new();
    for component in calendar.components() {
        if !component.is_named(b"VTIMEZONE") {
            continue;
        }
        let Some(tzid) = identifier(component) else {
            continue;
        };
        let reservation = match VtimezoneSet::reserve(meter) {
            Ok(reservation) => reservation,
            Err(breach) => {
                let named = Subject::new(tzid.as_bytes());
                refused.insert(tzid);
                report(
                    sink,
                    meter,
                    ZoneSetError::code_for_limit(breach),
                    Severity::LimitReached,
                    Some(named),
                );
                continue;
            },
        };
        let mut observances = Vec::new();
        read_observances(component, meter, sink, &mut observances, &tzid);
        let table = TransitionTable::new(tzid, observances, meter, sink);
        // Capture the identifier before transferring the table to the set.
        let named = Subject::new(table.tzid().as_str().as_bytes());
        let admission = zones.insert_reserved(table, reservation);
        if let Some(code) = admission.diagnostic_code() {
            report(sink, meter, code, Severity::Violation, Some(named));
        }
    }
    report_undefined(calendar, &zones, &refused, meter, sink);
    zones
}

impl ObservanceReader for Component {
    fn read_vtimezone(
        &self,
        meter: &mut Meter,
        sink: &mut dyn DiagnosticSink,
        out: &mut Vec<Observance>,
    ) -> Option<StagedDefinition> {
        read_definition(self, meter, sink, out)
    }
}

/// Append one definition's staged observances and return its identifier and overflow status.
fn read_definition<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut Vec<Observance>,
) -> Option<StagedDefinition> {
    let tzid = identifier(component)?;
    let exceeds_limit = read_observances(component, meter, sink, out, &tzid);
    Some(StagedDefinition {
        tzid,
        exceeds_limit,
    })
}

fn read_observances<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut Vec<Observance>,
    tzid: &str,
) -> bool {
    let mut staged = EarliestObservances::new(meter);
    let mut declared = 0_usize;
    for inner in component.components() {
        if !inner.is_named(b"STANDARD") && !inner.is_named(b"DAYLIGHT") {
            continue;
        }
        declared = declared.saturating_add(1);
        read_observance(inner, meter, sink, &mut staged);
    }
    // Count declared subcomponents, including those with unreadable values.
    if declared == 0 {
        report(
            sink,
            meter,
            DiagnosticCode::VtimezoneWithoutObservance,
            Severity::Violation,
            Some(Subject::new(tzid.as_bytes())),
        );
    }
    let exceeds_limit = staged.has_overflow_witness();
    out.extend(staged.into_observances());
    exceeds_limit
}

/// The definition's nonempty scalar `TEXT` identifier, decoded once.
fn identifier(component: &Component) -> Option<Box<str>> {
    let text = decode_single_scalar_text(component, &PropertyId::TZID)?;
    (!text.is_empty()).then(|| text.into_owned().into_boxed_str())
}

/// Stage one `STANDARD` or `DAYLIGHT` observance.
/// Report [`DiagnosticCode::VtimezoneObservanceUnreadable`] when every required property exists but none supplies a usable transition.
fn read_observance<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut EarliestObservances,
) {
    if !read_transitions(component, meter, sink, out) && states_every_required_property(component) {
        report(
            sink,
            meter,
            DiagnosticCode::VtimezoneObservanceUnreadable,
            Severity::Violation,
            None,
        );
    }
}

/// Whether the observance has every property RFC 5545 section 3.6.5 requires.
/// [`Component::audit`] reports missing properties.
fn states_every_required_property(component: &Component) -> bool {
    [
        PropertyId::DTSTART,
        PropertyId::TZOFFSETFROM,
        PropertyId::TZOFFSETTO,
    ]
    .iter()
    .all(|wanted| component.properties_named(wanted).next().is_some())
}

/// Append every transition one `STANDARD` or `DAYLIGHT` subcomponent states.
fn read_transitions<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut EarliestObservances,
) -> bool {
    // The subcomponent name determines daylight status even where daylight time has the smaller offset.
    let daylight = component.is_named(b"DAYLIGHT");
    let Some(offset_from) = component
        .get::<UtcOffset>(&PropertyId::TZOFFSETFROM)
        .value()
    else {
        return false;
    };
    let Some(offset_to) = component.get::<UtcOffset>(&PropertyId::TZOFFSETTO).value() else {
        return false;
    };
    // Only `DTSTART` anchors the rule.
    // Each `RDATE` contributes a single transition.
    let start = component
        .dtstart()
        .value()
        .and_then(|value| wall_clock(value, offset_from));
    if let Some(start) = start {
        let rule = read_rule(component, start, offset_from, meter, sink, out.capacity());
        let continues = matches!(
            rule,
            Some(YearlyRules {
                patterns: YearlyPatterns::Clocks(ClockRules::NoRemainingCivilOnsets),
                ..
            })
        );
        let mut rules = rule
            .into_iter()
            .flat_map(YearlyRules::into_rules)
            .peekable();
        if rules.peek().is_some() {
            out.extend(
                rules.map(|rule| {
                    Observance::new(start, offset_from, offset_to, daylight, Some(rule))
                }),
            );
        } else {
            let anchor = Observance::new(start, offset_from, offset_to, daylight, None);
            out.push(if continues {
                anchor.with_count_beyond_civil_range()
            } else {
                anchor
            });
        }
    }
    let dated = push_dated_transitions(component, offset_from, offset_to, daylight, out);
    start.is_some() || dated
}

/// Append one transition per readable `RDATE` field across all property lines.
/// [`TransitionTable::new`] orders the result.
fn push_dated_transitions(
    component: &Component,
    offset_from: UtcOffset,
    offset_to: UtcOffset,
    daylight: bool,
    out: &mut EarliestObservances,
) -> bool {
    let mut readable = false;
    let wanted = PropertyId::RDATE;
    for property in component.properties_named(&wanted) {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let Ok(value) = DateTimeValue::decode_property_field(property, field) else {
                continue;
            };
            let Some(start) = wall_clock(value, offset_from) else {
                continue;
            };
            readable = true;
            out.push(Observance::new(
                start,
                offset_from,
                offset_to,
                daylight,
                None,
            ));
        }
    }
    readable
}

/// Read a transition on the clock in effect before the change.
/// For a local time, use its written fields.
/// For a UTC value, convert its instant using `TZOFFSETFROM`.
/// A date alone supplies no transition time.
fn wall_clock(value: DateTimeValue<'_>, offset_from: UtcOffset) -> Option<CivilDateTime> {
    match value {
        DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => Some(stamp),
        DateTimeValue::Utc(stamp) => {
            let moment = stamp.at_offset(UtcOffset::UTC)?;
            CivilDateTime::from_instant(moment, offset_from)
        },
        DateTimeValue::Date(_) => None,
    }
}

#[derive(Debug)]
struct YearlyRules {
    patterns: YearlyPatterns,
    interval: YearInterval,
}

impl YearlyRules {
    fn into_rules(self) -> impl Iterator<Item = YearlyRule> {
        let Self { patterns, interval } = self;
        patterns
            .into_rules()
            .map(move |rule| rule.with_year_interval(interval))
    }
}

#[derive(Debug)]
enum YearlyPatterns {
    Clocks(ClockRules),
    Single(YearlyRule),
    Months {
        months: Option<MonthSet>,
        day: RuleDay,
        at: CivilTime,
        through: Option<CivilDate>,
    },
    DaySets {
        dates: Box<YearDateMasks>,
        at: CivilTime,
        through: Option<CivilDate>,
    },
    WeekDates {
        dates: Box<WeekYearMasks>,
        at: CivilTime,
        through: Option<CivilDate>,
    },
}

impl YearlyPatterns {
    fn into_rules(self) -> impl Iterator<Item = YearlyRule> {
        let (single, monthly, compiled, weeks, clocks) = match self {
            Self::Clocks(ClockRules::Patterns(rules)) => (None, None, None, None, Some(rules)),
            Self::Clocks(ClockRules::NoRemainingCivilOnsets) => (None, None, None, None, None),
            Self::Single(rule) => (Some(rule), None, None, None, None),
            Self::Months {
                months,
                day,
                at,
                through,
            } => (None, Some((months, day, at, through)), None, None, None),
            Self::DaySets { dates, at, through } => {
                (None, None, Some((dates, at, through)), None, None)
            },
            Self::WeekDates { dates, at, through } => {
                (None, None, None, Some((dates, at, through)), None)
            },
        };
        single
            .into_iter()
            .chain(clocks.into_iter().flatten())
            .chain(monthly.into_iter().flat_map(|(months, day, at, through)| {
                (1..=12)
                    .filter(move |month| months.is_none_or(|selected| selected.contains(*month)))
                    .filter_map(move |month| YearlyRule::new(month, day, at, through))
            }))
            .chain(
                compiled
                    .into_iter()
                    .flat_map(|(dates, at, through)| dates.into_rules(at, through)),
            )
            .chain(
                weeks
                    .into_iter()
                    .flat_map(|(dates, at, through)| dates.into_rules(at, through)),
            )
    }
}

/// Compile the observance rule or report an unsupported recurrence.
/// More than one `RRULE` property leaves no unique cadence.
fn read_rule<S: DiagnosticSink + ?Sized>(
    component: &Component,
    start: CivilDateTime,
    offset_from: UtcOffset,
    meter: &mut Meter,
    sink: &mut S,
    capacity: usize,
) -> Option<YearlyRules> {
    let wanted = PropertyId::RRULE;
    let mut written = component.properties_named(&wanted);
    let first = written.next()?;
    let rule = if written.next().is_some()
        || !matches!(
            first.declared_value_type(),
            None | Some(Ok(ValueType::Recur))
        ) {
        None
    } else {
        parse_rule(first.value_text().as_bytes(), start, offset_from, capacity)
    };
    if rule.is_none() {
        // The diagnostic applies to one unnamed observance within the zone.
        report(
            sink,
            meter,
            DiagnosticCode::VtimezoneRuleUnsupported,
            Severity::Note,
            None,
        );
    }
    rule
}

/// Compile an observance `RRULE` into annual patterns.
///
/// Numeric `BYDAY` selects positions within a month or year.
/// `BYWEEKNO` accepts weekdays without ordinals.
/// A supported `BYMONTHDAY` without `BYMONTH` selects across all months.
/// Default day selection uses the anchor's weekday for `BYWEEKNO` and its month day otherwise.
/// Missing clock fields use `DTSTART` under RFC 5545 section 3.3.10.
fn parse_rule(
    text: &[u8],
    start: CivilDateTime,
    offset_from: UtcOffset,
    capacity: usize,
) -> Option<YearlyRules> {
    let parts = collect_parts(text, offset_from)?;
    if parts.has_clocks() {
        let clocks = ClockSet::new(parts.hours, parts.minutes, parts.seconds, start.time());
        let unpositioned = RuleParts {
            set_positions: None,
            ..parts
        };
        let end = match parts.end {
            RuleEnd::Unlimited => ClockLimit::Unlimited,
            RuleEnd::Until(last) => ClockLimit::Until(last),
            RuleEnd::Count(count) => ClockLimit::Count(count),
        };
        let compiled = if parts.week_numbers.is_some() {
            annual_clocks::compile(
                &compile_week_masks(&unpositioned, start)?,
                clocks,
                parts.set_positions,
                start,
                parts.interval,
                end,
                capacity,
            )?
        } else {
            annual_clocks::compile(
                &compile_date_masks(&unpositioned, start)?,
                clocks,
                parts.set_positions,
                start,
                parts.interval,
                end,
                capacity,
            )?
        };
        return Some(YearlyRules {
            patterns: YearlyPatterns::Clocks(compiled),
            interval: parts.interval,
        });
    }
    validate_until(&parts, start, offset_from)?;
    Some(YearlyRules {
        patterns: compile_patterns(&parts, start)?,
        interval: parts.interval,
    })
}

/// Require `UNTIL` to name a generated onset, per RFC 5545 section 3.6.5.
fn validate_until(parts: &RuleParts, start: CivilDateTime, offset_from: UtcOffset) -> Option<()> {
    let RuleEnd::Until(last) = parts.end else {
        return Some(());
    };
    let expected = CivilDateTime::new(last.date(), start.time());
    if expected < start || expected.at_offset(offset_from)? != last.at_offset(offset_from)? {
        return None;
    }
    if parts.week_numbers.is_some() {
        let dates = compile_week_masks(parts, start)?;
        return (parts.interval.contains(
            dates.period_year(start.date())?,
            dates.period_year(last.date())?,
        ) && dates.contains(last.date())?)
        .then_some(());
    }
    if !parts
        .interval
        .contains(start.date().year(), last.date().year())
    {
        return None;
    }
    let dates = compile_date_masks(parts, start)?;
    let position = dates.count_through(last.date())?;
    (dates.date_at(last.date().year(), position) == Some(last.date())).then_some(())
}

fn compile_patterns(parts: &RuleParts, start: CivilDateTime) -> Option<YearlyPatterns> {
    if parts.week_numbers.is_some() {
        return compile_week_patterns(parts, start);
    }
    if let RuleEnd::Count(count) = parts.end {
        return compile_counted(parts, start, count);
    }
    if parts.year_days.is_some() || parts.set_positions.is_some() {
        return compile_date_sets(parts, start);
    }
    if parts.month.is_none()
        && parts.month_days.is_none()
        && let Some(term) = parts.weekdays.and_then(WeekdaySet::single)
        && let Some(ordinal) = term.ordinal()
    {
        return Some(YearlyPatterns::Single(YearlyRule::for_year_weekday(
            term.weekday(),
            ordinal,
            start.time(),
            parts.end.until(),
        )));
    }
    let day = day_form(parts, start.date());
    if day.is_none() {
        return compile_date_sets(parts, start);
    }
    let day = day?;
    if parts.month.is_some() || parts.month_days.is_some() {
        return Some(YearlyPatterns::Months {
            months: parts.month,
            day,
            at: start.time(),
            through: parts.end.until(),
        });
    }
    YearlyRule::new(start.date().month(), day, start.time(), parts.end.until())
        .map(YearlyPatterns::Single)
}

fn compile_week_patterns(parts: &RuleParts, start: CivilDateTime) -> Option<YearlyPatterns> {
    let mut dates = compile_week_masks(parts, start)?;
    let through = if let RuleEnd::Count(count) = parts.end {
        match counted_end(&dates, start.date(), parts.interval, count)? {
            CountEnd::Within(last) => {
                dates.retain_after(start.date(), last, parts.interval)?;
                Some(last)
            },
            CountEnd::BeyondCivilRange => None,
        }
    } else {
        parts.end.until()
    };
    Some(YearlyPatterns::WeekDates {
        dates: Box::new(dates),
        at: start.time(),
        through,
    })
}

fn compile_week_masks(parts: &RuleParts, start: CivilDateTime) -> Option<WeekYearMasks> {
    if parts.numeric_weekdays {
        return None;
    }
    let default = DateSelectors::Weekdays(WeekdaySet::singleton(WeekdayTerm::new(
        start.date().weekday()?,
        None,
    )?)?);
    let selectors = date_selectors(parts, default);
    let scope = if parts.month.is_some() {
        WeekdayScope::Month
    } else {
        WeekdayScope::Year
    };
    let filters = YearDateMasks::compile(parts.month, selectors, scope, None)?;
    WeekYearMasks::compile(
        &filters,
        parts.week_numbers?,
        parts.week_start,
        parts.set_positions,
    )
}

fn compile_counted(
    parts: &RuleParts,
    start: CivilDateTime,
    count: NonZeroU32,
) -> Option<YearlyPatterns> {
    let mut dates = compile_date_masks(parts, start)?;
    let through = match counted_end(&dates, start.date(), parts.interval, count)? {
        CountEnd::Within(last) => {
            dates.retain_after(start.date(), last, parts.interval)?;
            Some(last)
        },
        CountEnd::BeyondCivilRange => None,
    };
    Some(YearlyPatterns::DaySets {
        dates: Box::new(dates),
        at: start.time(),
        through,
    })
}

fn compile_date_sets(parts: &RuleParts, start: CivilDateTime) -> Option<YearlyPatterns> {
    Some(YearlyPatterns::DaySets {
        dates: Box::new(compile_date_masks(parts, start)?),
        at: start.time(),
        through: parts.end.until(),
    })
}

fn compile_date_masks(parts: &RuleParts, start: CivilDateTime) -> Option<YearDateMasks> {
    let default_day =
        parts.year_days.is_none() && parts.month_days.is_none() && parts.weekdays.is_none();
    if default_day && parts.month.is_none() && parts.set_positions.is_some() && !parts.has_clocks()
    {
        return None;
    }
    let months = if default_day && parts.month.is_none() {
        Some(MonthSet::singleton(start.date().month())?)
    } else {
        parts.month
    };
    let default = DateSelectors::MonthDays {
        days: MonthDaySet::singleton(i8::try_from(start.date().day()).ok()?)?,
        weekdays: None,
    };
    let selectors = date_selectors(parts, default);
    let scope = if parts.month.is_some() {
        WeekdayScope::Month
    } else {
        WeekdayScope::Year
    };
    YearDateMasks::compile(months, selectors, scope, parts.set_positions)
}

fn date_selectors(parts: &RuleParts, default: DateSelectors) -> DateSelectors {
    if let Some(days) = parts.year_days {
        DateSelectors::YearDays {
            days,
            month_days: parts.month_days,
            weekdays: parts.weekdays,
        }
    } else {
        match (parts.month_days, parts.weekdays) {
            (Some(days), weekdays) => DateSelectors::MonthDays { days, weekdays },
            (None, Some(weekdays)) => DateSelectors::Weekdays(weekdays),
            (None, None) => default,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum RuleEnd {
    Unlimited,
    Until(CivilDateTime),
    Count(NonZeroU32),
}

impl RuleEnd {
    const fn until(self) -> Option<CivilDate> {
        match self {
            Self::Until(stamp) => Some(stamp.date()),
            Self::Unlimited | Self::Count(_) => None,
        }
    }

    fn take(&mut self, end: Option<Self>) -> bool {
        if !matches!(self, Self::Unlimited) {
            return false;
        }
        match end {
            Some(end) => {
                *self = end;
                true
            },
            None => false,
        }
    }
}

/// What a `RECUR` value said, as far as this crate reads one.
#[derive(Clone, Copy, Debug)]
struct RuleParts {
    /// Whether the rule specifies `FREQ=YEARLY`.
    yearly: bool,
    interval: YearInterval,
    week_start: Weekday,
    week_numbers: Option<WeekNumbers>,
    /// The explicit `BYMONTH` set, absent when the rule states none.
    month: Option<MonthSet>,
    /// The nonempty `BYDAY` set, absent when the rule states none.
    weekdays: Option<WeekdaySet>,
    numeric_weekdays: bool,
    /// The nonempty `BYMONTHDAY` set, absent when the rule states none.
    month_days: Option<MonthDaySet>,
    /// The nonempty `BYYEARDAY` set, absent when the rule states none.
    year_days: Option<AnnualPositions>,
    /// Positions within the complete yearly candidate set.
    set_positions: Option<AnnualPositions>,
    hours: Option<u64>,
    minutes: Option<u64>,
    seconds: Option<u64>,
    end: RuleEnd,
}

impl RuleParts {
    /// A reading with nothing in it yet.
    const EMPTY: Self = Self {
        yearly: false,
        interval: YearInterval::YEARLY,
        week_start: Weekday::Monday,
        week_numbers: None,
        month: None,
        weekdays: None,
        numeric_weekdays: false,
        month_days: None,
        year_days: None,
        set_positions: None,
        hours: None,
        minutes: None,
        seconds: None,
        end: RuleEnd::Unlimited,
    };

    const fn has_clocks(self) -> bool {
        self.hours.is_some() || self.minutes.is_some() || self.seconds.is_some()
    }
}

#[derive(Clone, Copy, Debug)]
enum RulePartName {
    Frequency,
    Interval,
    WeekStart,
    WeekNumber,
    Month,
    Weekday,
    MonthDay,
    YearDay,
    SetPosition,
    Hour,
    Minute,
    Second,
    Count,
    Until,
}

impl RulePartName {
    const ALL: [Self; 14] = [
        Self::Frequency,
        Self::Interval,
        Self::WeekStart,
        Self::WeekNumber,
        Self::Month,
        Self::Weekday,
        Self::MonthDay,
        Self::YearDay,
        Self::SetPosition,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::Count,
        Self::Until,
    ];

    fn read(name: &[u8]) -> Option<(usize, Self)> {
        Self::ALL
            .into_iter()
            .enumerate()
            .find(|(_, part)| part.as_bytes().eq_ignore_ascii_case(name))
    }

    const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Frequency => b"FREQ",
            Self::Interval => b"INTERVAL",
            Self::WeekStart => b"WKST",
            Self::WeekNumber => b"BYWEEKNO",
            Self::Month => b"BYMONTH",
            Self::Weekday => b"BYDAY",
            Self::MonthDay => b"BYMONTHDAY",
            Self::YearDay => b"BYYEARDAY",
            Self::SetPosition => b"BYSETPOS",
            Self::Hour => b"BYHOUR",
            Self::Minute => b"BYMINUTE",
            Self::Second => b"BYSECOND",
            Self::Count => b"COUNT",
            Self::Until => b"UNTIL",
        }
    }
}

/// Refuse unknown parts rather than evaluating only part of a recurrence rule.
fn collect_parts(text: &[u8], offset_from: UtcOffset) -> Option<RuleParts> {
    let mut parts = RuleParts::EMPTY;
    let mut seen = [false; RulePartName::ALL.len()];
    for term in crate::internal::recur::parts(text) {
        let (index, name) = RulePartName::read(term.name())?;
        let slot = seen.get_mut(index)?;
        if *slot {
            return None;
        }
        *slot = true;
        if !take_part(&mut parts, name, term.value(), offset_from) {
            return None;
        }
    }
    parts.yearly.then_some(parts)
}

/// Validate one unique supported part before compiling any transition.
fn take_part(
    parts: &mut RuleParts,
    name: RulePartName,
    value: &[u8],
    offset_from: UtcOffset,
) -> bool {
    match name {
        RulePartName::Frequency => {
            parts.yearly = value.eq_ignore_ascii_case(b"YEARLY");
            parts.yearly
        },
        RulePartName::Interval => match crate::internal::recur::decode_positive_decimal(value) {
            Ok(interval) => {
                parts.interval = YearInterval::new(interval);
                true
            },
            Err(_) => false,
        },
        RulePartName::WeekStart => match weekday_of(value) {
            Some(start) => {
                parts.week_start = start;
                true
            },
            None => false,
        },
        RulePartName::WeekNumber => {
            parts.week_numbers = read_week_numbers(value);
            parts.week_numbers.is_some()
        },
        RulePartName::Month => {
            parts.month = read_months(value);
            parts.month.is_some()
        },
        RulePartName::Weekday => match read_weekdays(value) {
            Some(reading) => {
                parts.weekdays = Some(reading.days);
                parts.numeric_weekdays = reading.numeric;
                true
            },
            None => false,
        },
        RulePartName::MonthDay => {
            parts.month_days = read_month_days(value);
            parts.month_days.is_some()
        },
        RulePartName::YearDay => {
            parts.year_days = read_annual_positions(value);
            parts.year_days.is_some()
        },
        RulePartName::SetPosition => {
            parts.set_positions = read_annual_positions(value);
            parts.set_positions.is_some()
        },
        RulePartName::Hour => {
            parts.hours = read_clock_field(value, 23);
            parts.hours.is_some()
        },
        RulePartName::Minute => {
            parts.minutes = read_clock_field(value, 59);
            parts.minutes.is_some()
        },
        RulePartName::Second => {
            parts.seconds = read_clock_field(value, 60);
            parts.seconds.is_some()
        },
        RulePartName::Count => parts.end.take(
            crate::internal::recur::decode_positive_decimal(value)
                .ok()
                .map(RuleEnd::Count),
        ),
        RulePartName::Until => parts
            .end
            .take(until_clock(value, offset_from).map(RuleEnd::Until)),
    }
}

fn read_months(value: &[u8]) -> Option<MonthSet> {
    let mut months: Option<MonthSet> = None;
    for field in value.split(|octet| *octet == b',') {
        let month = month_number(field)?;
        months = Some(match months {
            None => MonthSet::singleton(month)?,
            Some(months) => months.with_month(month)?,
        });
    }
    months
}

fn read_annual_positions(value: &[u8]) -> Option<AnnualPositions> {
    let mut positions: Option<AnnualPositions> = None;
    for field in value.split(|octet| *octet == b',') {
        let number = signed_number::<3>(field)?;
        positions = Some(match positions {
            None => AnnualPositions::singleton(number)?,
            Some(positions) => positions.with_position(number)?,
        });
    }
    positions
}

/// Read a nonempty `BYMONTHDAY` set without retaining repeated spellings.
fn read_month_days(value: &[u8]) -> Option<MonthDaySet> {
    let mut days: Option<MonthDaySet> = None;
    for field in value.split(|octet| *octet == b',') {
        let day = month_day_number(field)?;
        days = Some(match days {
            Some(selected) => selected.with_day(day)?,
            None => MonthDaySet::singleton(day)?,
        });
    }
    days
}

fn read_week_numbers(value: &[u8]) -> Option<WeekNumbers> {
    let mut weeks: Option<WeekNumbers> = None;
    for field in value.split(|octet| *octet == b',') {
        let number = signed_number::<2>(field)?;
        weeks = Some(match weeks {
            None => WeekNumbers::singleton(number)?,
            Some(weeks) => weeks.with_number(number)?,
        });
    }
    weeks
}

struct WeekdayReading {
    days: WeekdaySet,
    numeric: bool,
}

fn read_weekdays(value: &[u8]) -> Option<WeekdayReading> {
    let mut weekdays: Option<WeekdaySet> = None;
    let mut numeric = false;
    for field in value.split(|octet| *octet == b',') {
        let term = weekday_term(field)?;
        numeric |= term.ordinal().is_some();
        weekdays = Some(match weekdays {
            None => WeekdaySet::singleton(term)?,
            Some(weekdays) => weekdays.with_term(term)?,
        });
    }
    Some(WeekdayReading {
        days: weekdays?,
        numeric,
    })
}

/// Determine the day selector for one month, or return `None` for an unsupported form.
fn day_form(parts: &RuleParts, start: CivilDate) -> Option<RuleDay> {
    if let Some(weekdays) = parts.weekdays {
        let term = weekdays.single()?;
        return match parts.month_days {
            None => nth_form(term),
            Some(days) => window_form(term, days),
        };
    }
    match parts.month_days {
        // RFC 5545 section 3.3.10 supplies the month day from `DTSTART`.
        // A February 29 anchor can leave common years without an onset.
        None => Some(RuleDay::DayOfMonth(start.day())),
        Some(days) if days.len() == 1 => fixed_form(days),
        Some(_) => None,
    }
}

/// `BYDAY=2SU` and `BYDAY=-1SU`, which name one weekday of the month outright.
fn nth_form(term: WeekdayTerm) -> Option<RuleDay> {
    let week = term.ordinal()?;
    Some(RuleDay::Nth {
        weekday: term.weekday(),
        week,
    })
}

/// One weekday inside a contiguous seven-day month-day window.
///
/// Positive windows keep their lower bound through [`RuleDay::OnOrAfter`].
/// Its weekday shift is at most six days, and date construction rejects results outside the month.
/// Negative days `-7` through `-1` select the month's last matching weekday.
fn window_form(term: WeekdayTerm, month_days: MonthDaySet) -> Option<RuleDay> {
    if term.ordinal().is_some() || month_days.len() != RUN_LENGTH {
        return None;
    }
    let (low, highest) = month_days.contiguous_run()?;
    if highest == -1 {
        return Some(RuleDay::Nth {
            weekday: term.weekday(),
            week: NthWeek::Last,
        });
    }
    if low < 1 {
        return None;
    }
    Some(RuleDay::OnOrAfter {
        weekday: term.weekday(),
        day: u8::try_from(low).ok()?,
    })
}

/// One positive or negative `BYMONTHDAY` position with no `BYDAY` beside it.
fn fixed_form(month_days: MonthDaySet) -> Option<RuleDay> {
    let (day, _) = month_days.contiguous_run()?;
    if day == -1 {
        return Some(RuleDay::LastDayOfMonth);
    }
    if day.is_negative() {
        return NonZeroU8::new(day.unsigned_abs()).map(RuleDay::FromMonthEnd);
    }
    u8::try_from(day).ok().map(RuleDay::DayOfMonth)
}

/// Read the UTC bound on the clock in use before the transition.
/// Reject bounds outside the civil range.
fn until_clock(value: &[u8], offset_from: UtcOffset) -> Option<CivilDateTime> {
    let written = DateTimeValue::decode_value(value).ok()?;
    match written {
        DateTimeValue::Utc(_) => wall_clock(written, offset_from),
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Zoned { .. } => None,
    }
}

/// One unsigned `BYMONTH` value, `1` through `12`.
fn month_number(value: &[u8]) -> Option<u8> {
    if !value.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let number = signed_number::<2>(value)?;
    if !(1..=12).contains(&number) {
        return None;
    }
    u8::try_from(number).ok()
}

/// One `BYMONTHDAY` value, `1` through `31` or `-31` through `-1`.
fn month_day_number(value: &[u8]) -> Option<i8> {
    let number = signed_number::<2>(value)?;
    if !(-31..=31).contains(&number) {
        return None;
    }
    i8::try_from(number).ok()
}

/// One optional ordinal and its two-letter weekday.
fn weekday_term(value: &[u8]) -> Option<WeekdayTerm> {
    let split = value.len().checked_sub(2)?;
    let (prefix, name) = value.split_at_checked(split)?;
    let weekday = weekday_of(name)?;
    if prefix.is_empty() {
        return WeekdayTerm::new(weekday, None);
    }
    WeekdayTerm::new(
        weekday,
        Some(i8::try_from(signed_number::<2>(prefix)?).ok()?),
    )
}

/// The weekday RFC 5545 section 3.3.10 spells `name`, compared as that section compares one.
fn weekday_of(name: &[u8]) -> Option<Weekday> {
    Weekday::ALL
        .into_iter()
        .find(|day| name.eq_ignore_ascii_case(day.as_bytes()))
}

/// One optionally signed nonzero decimal within the declared digit width.
fn signed_number<const DIGITS: usize>(bytes: &[u8]) -> Option<i16> {
    let (negative, digits) = match bytes.split_first() {
        Some((&b'-', tail)) => (true, tail),
        Some((&b'+', tail)) => (false, tail),
        _ => (false, bytes),
    };
    if digits.is_empty() || digits.len() > DIGITS || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut total: i16 = 0;
    for octet in digits {
        let place = i16::from(octet.wrapping_sub(b'0'));
        total = total.checked_mul(10)?.checked_add(place)?;
    }
    if total == 0 {
        return None;
    }
    if negative {
        total.checked_neg()
    } else {
        Some(total)
    }
}

/// Report each undefined `TZID` once with the identifier as its subject.
/// Traverse nested components with an explicit worklist.
fn report_undefined<S: DiagnosticSink + ?Sized>(
    calendar: &Component,
    zones: &VtimezoneSet,
    refused: &BTreeSet<Box<str>>,
    meter: &mut Meter,
    sink: &mut S,
) {
    let mut undefined: BTreeSet<&[u8]> = BTreeSet::new();
    let mut pending: Vec<&Component> = Vec::new();
    pending.push(calendar);
    while let Some(component) = pending.pop() {
        for property in component.properties() {
            for parameter in property.parameters_named(b"TZID") {
                let named = parameter.zone_identifier();
                // An empty `TZID` names no zone, which is the reading `ical-core` already gives it when it declines to make such a value a zoned one.
                if !named.is_empty() && !is_defined(zones, refused, named) {
                    undefined.insert(named);
                }
            }
        }
        pending.extend(component.components());
    }
    for named in &undefined {
        report(
            sink,
            meter,
            DiagnosticCode::MissingTimeZoneDefinition,
            Severity::Violation,
            Some(Subject::new(named)),
        );
    }
}

/// Whether the calendar defines the exact identifier, including a definition that exceeded the zone limit.
fn is_defined(zones: &VtimezoneSet, refused: &BTreeSet<Box<str>>, named: &[u8]) -> bool {
    str::from_utf8(named).is_ok_and(|text| zones.table(text).is_some() || refused.contains(text))
}

/// Report a diagnostic with an optional subject and charge any sink refusal.
/// The component tree has no input spans, so the diagnostic uses [`Location::NOWHERE`].
fn report<S: DiagnosticSink + ?Sized>(
    sink: &mut S,
    meter: &mut Meter,
    code: DiagnosticCode,
    severity: Severity,
    subject: Option<Subject>,
) {
    let diagnostic = Diagnostic::new(code, severity, Location::NOWHERE);
    report_diagnostic(
        sink,
        meter,
        match subject {
            Some(named) => diagnostic.about(named),
            None => diagnostic,
        },
    );
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Component, Diagnostic, DiagnosticCode, Document,
        IgnoreDiagnostics, Limits, Meter, Severity, UtcOffset, Weekday,
    };

    use super::read_calendar_zones;
    use crate::internal::tz::model::{
        NthWeek, Observance, ObservanceReader, RuleDay, VtimezoneSet, YearlyRule,
    };

    /// `America/New_York` as every major client exports it since the 2007 rule change.
    const NEW_YORK: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
TZNAME:EDT
DTSTART:20070311T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
TZNAME:EST
DTSTART:20071104T020000
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU
END:STANDARD
END:VTIMEZONE
";

    /// Both Berlin transitions occur at 01:00 UTC and have different local times.
    const BERLIN: &str = "\
BEGIN:VTIMEZONE
TZID:Europe/Berlin
BEGIN:DAYLIGHT
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
TZNAME:CEST
DTSTART:19700329T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:+0200
TZOFFSETTO:+0100
TZNAME:CET
DTSTART:19701025T030000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU
END:STANDARD
END:VTIMEZONE
";

    /// `Australia/Lord_Howe`, whose daylight saving moves the clock by thirty minutes.
    const LORD_HOWE: &str = "\
BEGIN:VTIMEZONE
TZID:Australia/Lord_Howe
BEGIN:STANDARD
TZOFFSETFROM:+1100
TZOFFSETTO:+1030
DTSTART:20270404T020000
RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=1SU
END:STANDARD
BEGIN:DAYLIGHT
TZOFFSETFROM:+1030
TZOFFSETTO:+1100
DTSTART:20271003T020000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=1SU
END:DAYLIGHT
END:VTIMEZONE
";

    /// Explicit New York transitions cover the second Sunday in March and the first Sunday in November from 2027 through 2029.
    const NEW_YORK_DATED: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:20270314T020000
RDATE:20280312T020000,20290311T020000
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:20271107T020000
RDATE:20281105T020000,20291104T020000
END:STANDARD
END:VTIMEZONE
";

    /// New York rules on both sides of the 2007 change.
    const NEW_YORK_BOTH_RULES: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:19671029T020000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;UNTIL=20061029T060000Z
END:STANDARD
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:19870405T020000
RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=1SU;UNTIL=20060402T070000Z
END:DAYLIGHT
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:20070311T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:20071104T020000
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU
END:STANDARD
END:VTIMEZONE
";

    /// One observance as the real zone's published rules describe it.
    #[derive(Clone, Copy, Debug)]
    struct Expected {
        /// The local transition time under `offset_from`.
        start: CivilDateTime,
        /// Seconds east of UTC before it.
        offset_from: i32,
        /// Seconds east of UTC from it.
        offset_to: i32,
        /// Whether this is the zone's daylight observance.
        daylight: bool,
        /// Which day of its month the rule names.
        day: RuleDay,
    }

    /// Convert the fixture's line endings to RFC 5545 section 3.1 terminators.
    fn crlf(text: &str) -> String {
        let mut out = String::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            out.push_str(line);
            out.push_str("\r\n");
        }
        out
    }

    /// One calendar wrapping `body`.
    fn calendar_text(body: &str) -> String {
        let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//EN\r\n");
        out.push_str(&crlf(body));
        out.push_str("END:VCALENDAR\r\n");
        out
    }

    /// Read the fixture under the supplied limits and return its definitions and diagnostics.
    fn read_under(body: &str, limits: Limits) -> (VtimezoneSet, Vec<Diagnostic>) {
        let text = calendar_text(body);
        let document = Document::parse(text.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let mut meter = Meter::new(limits);
        let mut reported = Vec::new();
        let zones = read_calendar_zones(calendar, &mut meter, &mut reported);
        (zones, reported)
    }

    /// Read `body` under the default policy.
    fn read(body: &str) -> (VtimezoneSet, Vec<Diagnostic>) {
        read_under(body, Limits::DEFAULT)
    }

    /// The observances of the one zone `body` declares under `tzid`.
    fn observances_of(body: &str, tzid: &str) -> Vec<Observance> {
        let (zones, _) = read(body);
        zones.table(tzid).unwrap().observances().to_vec()
    }

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().copied().map(Diagnostic::code).collect()
    }

    fn first_severity(reported: &[Diagnostic]) -> Option<Severity> {
        reported.first().copied().map(Diagnostic::severity)
    }

    /// The Sunday selector used by the zone fixtures.
    fn sunday(week: NthWeek) -> RuleDay {
        RuleDay::Nth {
            weekday: Weekday::Sunday,
            week,
        }
    }

    /// A minimal definition under `tzid`, for the tests that are about the identifier alone.
    fn definition(tzid: &str) -> String {
        format!(
            "BEGIN:VTIMEZONE\nTZID:{tzid}\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\n\
             TZOFFSETTO:+0100\nDTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n"
        )
    }

    /// The fixture anchors its rule at `20070325T020000`.
    fn ruled(rule: &str) -> String {
        format!(
            "BEGIN:VTIMEZONE\nTZID:Z\nBEGIN:DAYLIGHT\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0200\n\
             DTSTART:20070325T020000\nRRULE:{rule}\nEND:DAYLIGHT\nEND:VTIMEZONE\n"
        )
    }

    /// Assert that `body` reads as the observances a real zone's rules describe.
    fn assert_reads_as(body: &str, tzid: &str, expected: &[Expected]) {
        let (zones, reported) = read(body);
        let table = zones.table(tzid).unwrap();
        assert!(codes(&reported).is_empty(), "{tzid}");
        assert_eq!(table.observances().len(), expected.len(), "{tzid}");
        assert_eq!(
            table.coverage_end(),
            None,
            "{tzid} repeats by rules with no UNTIL, so it knows the future"
        );
        for (observance, wanted) in table.observances().iter().zip(expected) {
            assert_eq!(observance.start(), wanted.start, "{tzid}");
            assert_eq!(
                observance.offset_from(),
                offset(wanted.offset_from),
                "{tzid}"
            );
            assert_eq!(observance.offset_to(), offset(wanted.offset_to), "{tzid}");
            assert_eq!(observance.daylight(), wanted.daylight, "{tzid}");
            let rule = observance.rule().unwrap();
            assert_eq!(rule.day(), wanted.day, "{tzid}");
            assert_eq!(rule.at(), wanted.start.time(), "{tzid}");
            assert_eq!(rule.through(), None, "{tzid}");
        }
    }

    /// Count the readings for which the candidate offset governs the resulting instant.
    fn readings_of(transition: Observance, local: CivilDateTime) -> usize {
        let Some(moment) = transition.start().at_offset(transition.offset_from()) else {
            return 0;
        };
        let earlier = local
            .at_offset(transition.offset_from())
            .filter(|instant| *instant < moment);
        let later = local
            .at_offset(transition.offset_to())
            .filter(|instant| *instant >= moment);
        usize::from(earlier.is_some()).saturating_add(usize::from(later.is_some()))
    }

    #[test]
    fn the_united_states_rule_reads_as_the_two_sundays_it_names() {
        assert_reads_as(
            NEW_YORK,
            "America/New_York",
            &[
                Expected {
                    start: stamp(2007, 3, 11, 2, 0),
                    offset_from: -18_000,
                    offset_to: -14_400,
                    daylight: true,
                    day: sunday(NthWeek::Second),
                },
                Expected {
                    start: stamp(2007, 11, 4, 2, 0),
                    offset_from: -14_400,
                    offset_to: -18_000,
                    daylight: false,
                    day: sunday(NthWeek::First),
                },
            ],
        );
    }

    #[test]
    fn the_european_rule_reads_as_the_last_sundays_it_names() {
        assert_reads_as(
            BERLIN,
            "Europe/Berlin",
            &[
                Expected {
                    start: stamp(1970, 3, 29, 2, 0),
                    offset_from: 3_600,
                    offset_to: 7_200,
                    daylight: true,
                    day: sunday(NthWeek::Last),
                },
                Expected {
                    start: stamp(1970, 10, 25, 3, 0),
                    offset_from: 7_200,
                    offset_to: 3_600,
                    daylight: false,
                    day: sunday(NthWeek::Last),
                },
            ],
        );
    }

    /// A zone whose daylight saving is half an hour, which a reader that assumed an hour reads as an offset nobody wrote.
    #[test]
    fn a_zone_whose_daylight_saving_is_half_an_hour_reads_as_half_an_hour() {
        assert_reads_as(
            LORD_HOWE,
            "Australia/Lord_Howe",
            &[
                Expected {
                    start: stamp(2027, 4, 4, 2, 0),
                    offset_from: 39_600,
                    offset_to: 37_800,
                    daylight: false,
                    day: sunday(NthWeek::First),
                },
                Expected {
                    start: stamp(2027, 10, 3, 2, 0),
                    offset_from: 37_800,
                    offset_to: 39_600,
                    daylight: true,
                    day: sunday(NthWeek::First),
                },
            ],
        );
    }

    /// The two local transition hours map to the same UTC hour.
    #[test]
    fn a_zone_that_transitions_at_one_utc_hour_reads_back_as_that_hour() {
        for observance in observances_of(BERLIN, "Europe/Berlin") {
            let moment = observance
                .start()
                .at_offset(observance.offset_from())
                .unwrap();
            let utc = CivilDateTime::from_instant(moment, UtcOffset::UTC).unwrap();
            assert_eq!(utc.time(), CivilTime::from_hms(1, 0, 0).unwrap());
            assert_eq!(utc.date().year(), 1970);
        }
    }

    /// Real transitions supply independent expectations for the gap and fold.
    #[test]
    fn an_hour_that_repeats_and_an_hour_that_does_not_are_both_in_what_was_read() {
        let eastern = observances_of(NEW_YORK_DATED, "America/New_York");
        let spring = *eastern.first().unwrap();
        let fall = *eastern.get(1).unwrap();
        assert_eq!(spring.start(), stamp(2027, 3, 14, 2, 0));
        assert_eq!(fall.start(), stamp(2027, 11, 7, 2, 0));
        assert_eq!(
            readings_of(fall, stamp(2027, 11, 7, 1, 30)),
            2,
            "01:30 on the morning the United States falls back names two instants"
        );
        assert_eq!(
            readings_of(spring, stamp(2027, 3, 14, 2, 30)),
            0,
            "02:30 on the morning it springs forward names none"
        );
        assert_eq!(readings_of(spring, stamp(2027, 3, 14, 3, 30)), 1);
        assert_eq!(readings_of(fall, stamp(2027, 11, 7, 3, 30)), 1);
    }

    /// Lord Howe's fold is thirty minutes wide, so an hour-wide reading of it reports an ambiguity at 01:15 where the zone has none.
    #[test]
    fn a_half_hour_fold_is_half_an_hour_wide_and_not_an_hour() {
        let howe = observances_of(LORD_HOWE, "Australia/Lord_Howe");
        let ends = *howe.first().unwrap();
        let begins = *howe.get(1).unwrap();
        assert_eq!(readings_of(ends, stamp(2027, 4, 4, 1, 45)), 2);
        assert_eq!(readings_of(ends, stamp(2027, 4, 4, 1, 15)), 1);
        assert_eq!(readings_of(begins, stamp(2027, 10, 3, 2, 15)), 0);
        assert_eq!(readings_of(begins, stamp(2027, 10, 3, 1, 45)), 1);
    }

    /// The table of dates that runs out, which is the input this whole crate turns on.
    #[test]
    fn a_table_written_as_dates_stops_where_its_dates_do() {
        let (zones, reported) = read(NEW_YORK_DATED);
        let table = zones.table("America/New_York").unwrap();
        assert_eq!(codes(&reported).as_slice(), []);
        assert!(!table.is_truncated());
        let starts: Vec<CivilDate> = table
            .observances()
            .iter()
            .map(|observance| observance.start().date())
            .collect();
        assert_eq!(
            starts,
            alloc::vec![
                date(2027, 3, 14),
                date(2027, 11, 7),
                date(2028, 3, 12),
                date(2028, 11, 5),
                date(2029, 3, 11),
                date(2029, 11, 4),
            ],
            "one observance for the DTSTART and one for each RDATE value, sorted"
        );
        assert!(
            table
                .observances()
                .iter()
                .all(|observance| observance.rule().is_none()),
            "a date states no cadence, so no RDATE observance carries the rule"
        );
        assert_eq!(
            table.coverage_end(),
            Some(date(2029, 11, 4)),
            "an event in 2035 is past everything this file knows"
        );
        assert!(table.coverage_end() < Some(date(2035, 6, 1)));
    }

    /// Each historical rule uses its observance's offset to interpret the UTC `UNTIL`.
    #[test]
    fn a_zone_that_changed_its_rules_keeps_both_and_dates_each_on_its_own_clock() {
        let (zones, reported) = read(NEW_YORK_BOTH_RULES);
        let table = zones.table("America/New_York").unwrap();
        assert_eq!(codes(&reported).as_slice(), []);
        assert_eq!(table.observances().len(), 4);
        let through: Vec<Option<CivilDate>> = table
            .observances()
            .iter()
            .map(|observance| observance.rule().and_then(YearlyRule::through))
            .collect();
        assert_eq!(
            through,
            alloc::vec![Some(date(2006, 10, 29)), Some(date(2006, 4, 2)), None, None,],
            "UNTIL:20061029T060000Z is 02:00 on the 29th read against -0400, not 06:00 UTC"
        );
        assert_eq!(
            table.coverage_end(),
            None,
            "the rules written for 2007 onwards have no UNTIL, so the zone knows the future"
        );
    }

    /// Calendar identifiers can include vendor prefixes.
    #[test]
    fn an_identifier_keeps_its_literal_characters() {
        let written = [
            "America/New_York",
            "W. Europe Standard Time",
            "/mozilla.org/20050126_1/Europe/Berlin",
            "Customized Time Zone",
            "GMT+9",
            "\"Europe/Paris\"",
        ];
        for tzid in written {
            let (zones, reported) = read(&definition(tzid));
            assert!(codes(&reported).is_empty(), "{tzid}");
            let filed = tzid;
            assert!(
                zones.table(filed).is_some(),
                "{tzid} should be filed under {filed}"
            );
            assert_eq!(zones.len(), 1, "{tzid}");
        }
        let (folded, _) = read(&definition("W. Europe Standard Time"));
        assert!(
            folded.table("w. europe standard time").is_none(),
            "lookup is by exact bytes, which is what keeps aliasing the caller's step"
        );
        let (prefixed, _) = read(&definition("/mozilla.org/20050126_1/Europe/Berlin"));
        assert!(
            prefixed.table("Europe/Berlin").is_none(),
            "nothing goes looking for an IANA name inside a vendor identifier"
        );
    }

    /// A definition without observances keeps its identifier and supplies no offset.
    #[test]
    fn a_definition_with_no_observance_is_reported_and_still_a_zone() {
        let (zones, reported) = read("BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nEND:VTIMEZONE\n");
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::VtimezoneWithoutObservance]
        );
        assert_eq!(first_severity(&reported), Some(Severity::Violation));
        let table = zones.table("Europe/Berlin").unwrap();
        assert!(table.is_empty());
        assert_eq!(table.coverage_end(), None);
    }

    /// A rule outside the closed form costs the rule and not the observance.
    #[test]
    fn a_rule_this_crate_does_not_evaluate_is_a_note_and_the_dtstart_still_stands() {
        let refused = [
            "FREQ=MONTHLY;BYMONTHDAY=1",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=SUX",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;BYSETPOS=0",
            "FREQ=YEARLY;BYMONTH=3,13;BYDAY=-1SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;COUNT=0",
            "FREQ=YEARLY;INTERVAL=0;BYMONTH=3;BYDAY=2SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=-2SU,-54SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=SU;BYMONTHDAY=8,9,32",
            "FREQ=YEARLY;BYMONTH=3;BYWEEKNO=0",
            "BYMONTH=3;BYDAY=2SU",
        ];
        for rule in refused {
            let (zones, reported) = read(&ruled(rule));
            assert_eq!(
                codes(&reported),
                alloc::vec![DiagnosticCode::VtimezoneRuleUnsupported],
                "{rule}"
            );
            assert_eq!(first_severity(&reported), Some(Severity::Note), "{rule}");
            let table = zones.table("Z").unwrap();
            assert_eq!(table.observances().len(), 1, "{rule}");
            let observance = *table.observances().first().unwrap();
            assert_eq!(observance.rule(), None, "{rule}");
            assert_eq!(observance.start(), stamp(2007, 3, 25, 2, 0), "{rule}");
            assert_eq!(
                table.coverage_end(),
                Some(date(2007, 3, 25)),
                "the observance stands alone and covers its own date, {rule}"
            );
        }
    }

    /// The day forms the corpus actually writes, including the two window shapes Exchange and older tz database releases emit instead of a `BYDAY` ordinal.
    #[test]
    fn every_day_form_the_corpus_writes_reads_as_the_one_it_names() {
        let cases = [
            (
                "FREQ=YEARLY;BYMONTH=10;BYDAY=SU;BYMONTHDAY=25,26,27,28,29,30,31",
                10_u8,
                RuleDay::OnOrAfter {
                    weekday: Weekday::Sunday,
                    day: 25,
                },
            ),
            (
                "FREQ=YEARLY;BYMONTH=4;BYDAY=SU;BYMONTHDAY=1,2,3,4,5,6,7",
                4,
                RuleDay::OnOrAfter {
                    weekday: Weekday::Sunday,
                    day: 1,
                },
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYDAY=SU;BYMONTHDAY=-1,-2,-3,-4,-5,-6,-7",
                3,
                sunday(NthWeek::Last),
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=-1",
                3,
                RuleDay::LastDayOfMonth,
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15",
                3,
                RuleDay::DayOfMonth(15),
            ),
            ("freq=yearly;bymonth=3;byday=5su", 3, sunday(NthWeek::Fifth)),
            // The anchor supplies both the month and month day.
            ("FREQ=YEARLY", 3, RuleDay::DayOfMonth(25)),
        ];
        for (written, month, expected) in cases {
            let (zones, reported) = read(&ruled(written));
            assert!(codes(&reported).is_empty(), "{written}");
            let observance = *zones.table("Z").unwrap().observances().first().unwrap();
            let rule = observance.rule().unwrap();
            assert_eq!(rule.day(), expected, "{written}");
            assert_eq!(rule.month(), Some(month), "{written}");
            assert_eq!(
                rule.at(),
                CivilTime::from_hms(2, 0, 0).unwrap(),
                "{written}"
            );
            assert_eq!(rule.through(), None, "{written}");
        }
    }

    /// Two or more references can name the same identifier.
    #[test]
    fn an_identifier_no_definition_backs_is_reported_once() {
        let mut body = String::from(BERLIN);
        body.push_str(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=America/Denver:20260810T090000\n\
             DTEND;TZID=America/Denver:20260810T100000\n\
             RDATE;TZID=Europe/Berlin:20260811T090000\nEND:VEVENT\n",
        );
        body.push_str(
            "BEGIN:VEVENT\nUID:b\nDTSTART;TZID=America/Denver:20260812T090000\nEND:VEVENT\n",
        );
        let (zones, reported) = read(&body);
        assert!(zones.table("Europe/Berlin").is_some());
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::MissingTimeZoneDefinition],
            "three properties named one undefined zone, and the defined one said nothing"
        );
    }

    /// A calendar declaring one zone twice reports it and keeps the first, rather than preferring the later definition out of sight.
    #[test]
    fn a_second_definition_of_one_zone_is_reported_and_not_chosen_between() {
        let mut body = String::from(BERLIN);
        body.push_str(NEW_YORK_DATED);
        body.push_str(&definition("Europe/Berlin"));
        let (zones, reported) = read(&body);
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::DuplicateTimeZoneIdentifier]
        );
        assert_eq!(zones.len(), 2);
        assert_eq!(
            zones.table("Europe/Berlin").unwrap().observances().len(),
            2,
            "the definition that arrived first is the one that stayed"
        );
    }

    /// A bound nobody charges is decoration: the table that owns the truncation decision is the one that reports it, and this unit calls it rather than repeating it.
    #[test]
    fn observances_past_the_bound_are_dropped_by_the_table_that_owns_the_decision() {
        let limits = Limits::DEFAULT.with_max_vtimezone_observances(4);
        let (zones, reported) = read_under(NEW_YORK_DATED, limits);
        let table = zones.table("America/New_York").unwrap();
        assert!(table.is_truncated());
        assert_eq!(table.observances().len(), 4);
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::VtimezoneObservancesTruncated]
        );
        assert_eq!(
            table.coverage_end(),
            Some(date(2028, 11, 5)),
            "coverage ends earlier rather than the table acquiring a hole"
        );
    }

    /// The zone count limit applies to the full calendar.
    #[test]
    fn a_calendar_declaring_more_zones_than_the_policy_admits_keeps_the_ones_that_fit() {
        let limits = Limits::DEFAULT.with_max_vtimezone_components(1);
        let mut body = String::from(BERLIN);
        body.push_str(NEW_YORK);
        let (zones, reported) = read_under(&body, limits);
        assert_eq!(zones.len(), 1);
        assert!(zones.table("Europe/Berlin").is_some());
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::VtimezoneComponentsTruncated],
            "a definition the caller's own bound refused is reported as that, and named"
        );
        assert_eq!(
            reported
                .first()
                .and_then(|entry| entry.subject())
                .map(|named| named.as_bytes().to_vec()),
            Some(b"America/New_York".to_vec()),
            "the report says which definition was turned back"
        );
    }

    /// The trait is the other door, and a caller holding one component and no calendar uses it.
    #[test]
    fn one_definition_can_be_read_through_the_trait_and_an_erased_sink() {
        let text = calendar_text(NEW_YORK);
        let document =
            Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let vtimezone: &Component = calendar
            .components()
            .find(|component| component.is_named(b"VTIMEZONE"))
            .unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut out = Vec::new();
        let tzid = vtimezone.read_vtimezone(&mut meter, &mut reported, &mut out);
        let staged = tzid.unwrap();
        assert_eq!(staged.tzid.as_ref(), "America/New_York");
        assert!(!staged.exceeds_limit);
        assert_eq!(out.len(), 2);
        assert_eq!(reported.as_slice(), []);
        assert_eq!(
            meter.vtimezone_observances(),
            0,
            "the trait reads; admitting the observances is the table's charge and its call"
        );
    }

    /// A definition needs a usable identifier before it can enter the set.
    #[test]
    fn a_definition_with_no_usable_identifier_is_filed_nowhere() {
        let bodies = [
            "BEGIN:VTIMEZONE\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nTZID:Europe/Paris\nBEGIN:STANDARD\n\
             TZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nDTSTART:19700101T000000\nEND:STANDARD\n\
             END:VTIMEZONE\n",
        ];
        for body in bodies {
            let (zones, _) = read(body);
            assert!(zones.is_empty(), "{body}");
        }
    }

    /// An unreadable offset yields no transition.
    /// The definition still contains an observance subcomponent.
    #[test]
    fn an_observance_with_no_readable_offset_yields_nothing_and_no_code_here() {
        let (zones, reported) = read(
            "BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nBEGIN:STANDARD\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
        );
        assert!(
            codes(&reported).is_empty(),
            "section 3.6's required-property reading is Component::audit's, not this unit's"
        );
        assert!(zones.table("Europe/Berlin").unwrap().is_empty());
    }

    /// The meter tracks zone definitions and observances as separate dimensions.
    #[test]
    fn the_bounds_this_unit_charges_are_the_two_a_zone_definition_has() {
        let limits = Limits::DEFAULT;
        assert_eq!(limits.max_vtimezone_observances(), 4096);
        assert_eq!(limits.max_vtimezone_components(), 256);
        assert!(Limits::GENEROUS.max_vtimezone_observances() > limits.max_vtimezone_observances());

        let text = calendar_text(NEW_YORK);
        let document = Document::parse(text.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let mut meter = Meter::new(limits);
        let zones = read_calendar_zones(calendar, &mut meter, &mut IgnoreDiagnostics);
        assert_eq!(zones.len(), 1);
        assert_eq!(meter.vtimezone_observances(), 2);
        assert_eq!(meter.vtimezone_components(), 1);
    }
}
