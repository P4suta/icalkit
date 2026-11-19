// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [RFC 5545 section 3.6.5](https://www.rfc-editor.org/rfc/rfc5545#section-3.6.5) defines the `VTIMEZONE` component.
//! Sections 3.8.3.3 and 3.8.3.4 define the UTC offsets before and after a transition.
//! Sections 3.8.5.2 and 3.8.5.3 define recurrence dates and recurrence rules.
//!
//! `RRULE` supplies computed transitions and `RDATE` supplies explicit transitions.
//! Answers before [`TransitionTable::coverage_start`] carry [`AnswerBasis::BeforeKnownTransitions`](crate::internal::tz::AnswerBasis::BeforeKnownTransitions).
//! Answers after [`TransitionTable::coverage_end`] carry [`AnswerBasis::BeyondKnownTransitions`](crate::internal::tz::AnswerBasis::BeyondKnownTransitions).
//! Coverage markers distinguish continuation of the nearest observance from a computed transition.
//!
//! # Closed-form rules
//!
//! [`YearlyRule`] represents a month-scoped day selector or a weekday ordinal counted within the year.
//! Each pattern includes a wall clock, a nonzero year interval, and an optional inclusive last date.
//! Arithmetic over the selected period answers each rule lookup without iterating candidate dates.
//! [`ZoneSource::resolve`](crate::internal::tz::ZoneSource::resolve) can query these patterns without a recurrence meter.
//!
//! A rule outside the supported patterns produces [`DiagnosticCode::VtimezoneRuleUnsupported`](crate::internal::core::DiagnosticCode::VtimezoneRuleUnsupported).
//! Its observance's `DTSTART` remains an explicit transition.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::num::{NonZeroI8, NonZeroU8};

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant,
    LimitExceeded, Location, Meter, Severity, UtcOffset, Weekday, report_diagnostic,
};

use crate::internal::tz::annual_dates::AnnualDateSelection;
use crate::internal::tz::ident::Tzid;
use crate::internal::tz::year_intervals::YearInterval;
use crate::internal::tz::year_layouts::YearOffset;

/// A signed weekday occurrence within the selected month or year.
///
/// For month-scoped rules, [`NthWeek::Fifth`] is absent in a month with four matching weekdays, while [`NthWeek::Last`] still names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NthWeek {
    /// The first such weekday in the selected period.
    First,
    /// The second.
    Second,
    /// The third.
    Third,
    /// The fourth.
    Fourth,
    /// The fifth.
    Fifth,
    /// The last, whichever ordinal that turns out to be.
    Last,
    /// A signed position within the selected month or year.
    Ordinal(NonZeroI8),
}

impl NthWeek {
    pub(super) fn from_ordinal(position: i8) -> Option<Self> {
        let position = NonZeroI8::new(position)?;
        if position.get().unsigned_abs() > 53 {
            return None;
        }
        Some(match position.get() {
            1 => Self::First,
            2 => Self::Second,
            3 => Self::Third,
            4 => Self::Fourth,
            5 => Self::Fifth,
            -1 => Self::Last,
            _ => Self::Ordinal(position),
        })
    }

    pub(super) const fn number(self) -> i8 {
        match self {
            Self::First => 1,
            Self::Second => 2,
            Self::Third => 3,
            Self::Fourth => 4,
            Self::Fifth => 5,
            Self::Last => -1,
            Self::Ordinal(position) => position.get(),
        }
    }
}

/// A day selector evaluated within a month or year.
///
/// Only weekday ordinals admit year scope.
/// An invalid date or missing ordinal position evaluates to `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RuleDay {
    /// A fixed day of the month, as `BYMONTHDAY=n` writes one.
    DayOfMonth(u8),
    /// The last day of the month, as `BYMONTHDAY=-1` writes it.
    LastDayOfMonth,
    /// The *n*th or last given weekday, as `BYDAY=2SU` or `BYDAY=-1SU` writes one.
    Nth {
        /// The weekday.
        weekday: Weekday,
        /// Which occurrence of it.
        week: NthWeek,
    },
    /// The first given weekday falling on or after a day, as `BYDAY=SU;BYMONTHDAY=8,..,14` writes one.
    OnOrAfter {
        /// The weekday.
        weekday: Weekday,
        /// The earliest day of the month it may fall on.
        day: u8,
    },
    /// The last given weekday falling on or before a month-day bound.
    OnOrBefore {
        /// The weekday.
        weekday: Weekday,
        /// The inclusive upper day of the month.
        day: u8,
    },
    /// A day counted backward from the month's final day, with position one naming that day.
    FromMonthEnd(NonZeroU8),
    /// A distinct date selected from a compiled Gregorian year layout.
    SelectedDates(AnnualDateSelection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum RuleDate {
    Month { month: u8, day: RuleDay },
    YearWeekday { weekday: Weekday, week: NthWeek },
}

/// A closed-form yearly transition pattern.
///
/// RFC 5545 section 3.6.5 uses the offset before the transition, `TZOFFSETFROM`, to interpret the local onset time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearlyRule {
    date: RuleDate,
    interval: YearInterval,
    /// The local onset time at `TZOFFSETFROM`.
    at: CivilTime,
    /// The inclusive rule end, absent when no finite end falls in the civil date range.
    through: Option<CivilDate>,
}

impl YearlyRule {
    /// A rule for `month`, or `None` when there is no such month.
    /// Compiled date masks must carry the same month.
    #[must_use]
    pub const fn new(
        month: u8,
        day: RuleDay,
        at: CivilTime,
        through: Option<CivilDate>,
    ) -> Option<Self> {
        if month == 0 || month > 12 {
            return None;
        }
        if let RuleDay::SelectedDates(selection) = day
            && selection.month() != month
        {
            return None;
        }
        Some(Self {
            date: RuleDate::Month { month, day },
            interval: YearInterval::YEARLY,
            at,
            through,
        })
    }

    pub(super) const fn for_year_weekday(
        weekday: Weekday,
        week: NthWeek,
        at: CivilTime,
        through: Option<CivilDate>,
    ) -> Self {
        Self {
            date: RuleDate::YearWeekday { weekday, week },
            interval: YearInterval::YEARLY,
            at,
            through,
        }
    }

    pub(super) const fn date_pattern(self) -> RuleDate {
        self.date
    }

    pub(super) const fn with_clock(mut self, at: CivilTime) -> Self {
        self.at = at;
        self
    }

    pub(super) const fn with_year_interval(mut self, interval: YearInterval) -> Self {
        self.interval = interval;
        self
    }

    pub(super) const fn year_interval(self) -> YearInterval {
        self.interval
    }

    pub(super) fn first_year(self, anchor: CivilDate) -> Option<i32> {
        match self.date {
            RuleDate::Month {
                day: RuleDay::SelectedDates(dates),
                ..
            } => dates.first_year(anchor),
            RuleDate::Month { .. } | RuleDate::YearWeekday { .. } => Some(i32::from(anchor.year())),
        }
    }

    fn period_year(self, calendar: u16) -> Option<i32> {
        match self.date {
            RuleDate::Month {
                day: RuleDay::SelectedDates(dates),
                ..
            } => dates.period_year(calendar),
            RuleDate::Month { .. } | RuleDate::YearWeekday { .. } => Some(i32::from(calendar)),
        }
    }

    fn calendar_year(self, period: i32) -> Option<u16> {
        match self.date {
            RuleDate::Month {
                day: RuleDay::SelectedDates(dates),
                ..
            } => dates.calendar_year(period),
            RuleDate::Month { .. } | RuleDate::YearWeekday { .. } => {
                YearOffset::Same.calendar_year(period)
            },
        }
    }

    pub(super) fn aligned_in(self, anchor: CivilDate, calendar: u16) -> bool {
        self.first_year(anchor)
            .zip(self.period_year(calendar))
            .is_some_and(|(first, year)| self.year_interval().contains(first, year))
    }

    pub(super) fn prior_calendar_years(
        self,
        anchor: CivilDate,
        limit: u16,
    ) -> impl Iterator<Item = u16> {
        self.first_year(anchor)
            .zip(self.period_year(limit))
            .into_iter()
            .flat_map(move |(first, last)| self.interval.prior_years(first, last))
            .filter_map(move |year| self.calendar_year(year))
    }

    pub(super) fn boundary_calendar_years(
        self,
        anchor: CivilDate,
        limit: u16,
    ) -> impl Iterator<Item = u16> {
        let bounds = self.first_year(anchor).zip(self.period_year(limit));
        let years = match self.date {
            RuleDate::Month {
                day: RuleDay::SelectedDates(dates),
                ..
            } => dates.boundary_years(),
            RuleDate::Month { .. } | RuleDate::YearWeekday { .. } => None,
        };
        years
            .into_iter()
            .flatten()
            .rev()
            .filter(move |year| {
                bounds.is_some_and(|(first, last)| {
                    *year >= first && *year <= last && self.interval.contains(first, *year)
                })
            })
            .filter_map(move |year| self.calendar_year(year))
    }

    /// The compiled pattern's month, absent for a direct yearly weekday ordinal.
    #[must_use]
    pub const fn month(self) -> Option<u8> {
        match self.date {
            RuleDate::Month { month, .. } => Some(month),
            RuleDate::YearWeekday { .. } => None,
        }
    }

    /// The selector naming the date within the selected month or year.
    #[must_use]
    pub const fn day(self) -> RuleDay {
        match self.date {
            RuleDate::Month { day, .. } => day,
            RuleDate::YearWeekday { weekday, week } => RuleDay::Nth { weekday, week },
        }
    }

    /// The wall clock the transition happens at.
    #[must_use]
    pub const fn at(self) -> CivilTime {
        self.at
    }

    /// The last date the rule applies to within civil years 0000–9999.
    ///
    /// An absent bound covers unlimited rules and counted ends beyond the civil range.
    #[must_use]
    pub const fn through(self) -> Option<CivilDate> {
        self.through
    }
}

/// One annual pattern or explicit date from a `STANDARD` or `DAYLIGHT` subcomponent.
///
/// Compiled annual patterns share the source observance's original anchor.
/// Each `RDATE` contributes an explicit transition with the offsets from its subcomponent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Observance {
    /// The local onset time at `offset_from`.
    start: CivilDateTime,
    /// The offset in force before it, from `TZOFFSETFROM`.
    offset_from: UtcOffset,
    /// The offset in force from it, from `TZOFFSETTO`.
    offset_to: UtcOffset,
    /// Whether this is the zone's daylight observance.
    daylight: bool,
    /// Recurrence and coverage remaining after the anchor.
    recurrence: ObservanceRecurrence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ObservanceRecurrence {
    Dated,
    CountBeyondCivilRange,
    Annual(YearlyRule),
}

impl Observance {
    /// An observance beginning at `start`.
    #[must_use]
    pub const fn new(
        start: CivilDateTime,
        offset_from: UtcOffset,
        offset_to: UtcOffset,
        daylight: bool,
        rule: Option<YearlyRule>,
    ) -> Self {
        Self {
            start,
            offset_from,
            offset_to,
            daylight,
            recurrence: match rule {
                Some(rule) => ObservanceRecurrence::Annual(rule),
                None => ObservanceRecurrence::Dated,
            },
        }
    }

    pub(super) const fn with_count_beyond_civil_range(self) -> Self {
        Self {
            recurrence: ObservanceRecurrence::CountBeyondCivilRange,
            ..self
        }
    }

    /// The local onset time at [`Self::offset_from`].
    #[must_use]
    pub const fn start(self) -> CivilDateTime {
        self.start
    }

    /// The offset in force before it.
    #[must_use]
    pub const fn offset_from(self) -> UtcOffset {
        self.offset_from
    }

    /// The offset in force from it.
    #[must_use]
    pub const fn offset_to(self) -> UtcOffset {
        self.offset_to
    }

    /// Whether this is the zone's daylight observance.
    #[must_use]
    pub const fn daylight(self) -> bool {
        self.daylight
    }

    /// The rule repeating it, absent for a date-driven transition.
    #[must_use]
    pub const fn rule(self) -> Option<YearlyRule> {
        match self.recurrence {
            ObservanceRecurrence::Annual(rule) => Some(rule),
            ObservanceRecurrence::Dated | ObservanceRecurrence::CountBeyondCivilRange => None,
        }
    }

    /// Whether the offsets on either side of this observance differ.
    ///
    /// A definition can establish its base offset with an observance whose two offsets match.
    /// A transition with equal offsets creates neither a gap nor a fold.
    #[must_use]
    pub const fn moves_the_clock(self) -> bool {
        self.offset_from.seconds() != self.offset_to.seconds()
    }

    /// The last supported date, absent when its rule covers the remaining civil range.
    ///
    /// A date-driven observance covers its own date.
    /// A rule-driven observance uses its inclusive end from `UNTIL` or `COUNT`.
    #[must_use]
    pub fn covered_through(self) -> Option<CivilDate> {
        match self.recurrence {
            ObservanceRecurrence::Dated => Some(self.start.date()),
            ObservanceRecurrence::CountBeyondCivilRange => None,
            ObservanceRecurrence::Annual(rule) => rule.through(),
        }
    }
}

/// An immutable table of one zone's transitions in onset order.
///
/// Construction charges the observance budget once.
/// [`ZoneSource`](crate::internal::tz::ZoneSource) can query the resulting table without a meter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionTable {
    /// The identifier this table answers to, compared by exact bytes.
    tzid: Box<str>,
    /// The observances, ascending by the instant they begin.
    observances: Box<[Observance]>,
    /// All recurring observances in onset order.
    ///
    /// An explicit transition can separate a rule's anchor from a later occurrence of the same rule.
    /// Indexing the rules keeps each one available for lookup.
    rules: Vec<Observance>,
    /// Whether the resource limits cut the list short.
    truncated: bool,
    /// The final date of finite coverage.
    coverage_end: Option<CivilDate>,
    /// The date of the earliest transition this table records, absent when it records none.
    coverage_start: Option<CivilDate>,
}

impl TransitionTable {
    /// Build a table for `tzid` and charge the meter for each admitted observance.
    ///
    /// Rule anchors and `RDATE` values can arrive in any order across subcomponents.
    /// Sort each local `DTSTART` by its instant at `TZOFFSETFROM`.
    /// Equal local times with different offsets can identify different instants.
    /// The full observance breaks ties between equal instants and makes the order independent of the input.
    ///
    /// Keep the earliest onsets that fit the remaining budget.
    /// Report [`DiagnosticCode::VtimezoneObservancesTruncated`] when the observance limit cuts the list short.
    /// Report [`DiagnosticCode::ResourceBudgetExhausted`] when the shared budget runs out.
    #[must_use]
    pub fn new<S: DiagnosticSink + ?Sized>(
        tzid: Box<str>,
        observances: Vec<Observance>,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Self {
        let mut kept = observances;
        kept.sort_unstable_by_key(|observance| OrderedObservance::new(*observance));
        let (admitted, breach) = admitted_count(kept.len(), meter);
        let truncated = admitted < kept.len();
        kept.truncate(admitted);
        if truncated {
            report_diagnostic(
                sink,
                meter,
                Diagnostic::new(
                    if matches!(breach, Some(LimitExceeded::Exhausted)) {
                        DiagnosticCode::ResourceBudgetExhausted
                    } else {
                        DiagnosticCode::VtimezoneObservancesTruncated
                    },
                    Severity::LimitReached,
                    Location::NOWHERE,
                ),
            );
        }
        let coverage_end = if truncated {
            kept.last().map(|last| last.start().date())
        } else {
            coverage_end_of(&kept)
        };
        let coverage_start = kept.first().map(|first| first.start().date());
        let rules = kept
            .iter()
            .filter(|observance| observance.rule().is_some())
            .copied()
            .collect();
        Self {
            tzid,
            observances: kept.into_boxed_slice(),
            rules,
            truncated,
            coverage_end,
            coverage_start,
        }
    }

    /// The identifier this table answers to.
    #[must_use]
    pub fn tzid(&self) -> Tzid<'_> {
        Tzid::new(&self.tzid)
    }

    /// The observances, ascending by the instant they begin.
    #[must_use]
    pub fn observances(&self) -> &[Observance] {
        &self.observances
    }

    /// The observances that repeat by a rule, in the same order.
    ///
    /// Each query checks all recurring patterns, including rules with anchors before intervening explicit transitions.
    /// The observance meter bounds the number of compiled patterns, including month-specific patterns from one source rule.
    /// Each pattern contributes at most one computed onset per year.
    #[must_use]
    pub fn rules(&self) -> &[Observance] {
        &self.rules
    }

    /// Whether the meter refused any offered observance.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// The date at which known transitions end.
    ///
    /// Truncation limits coverage to the last admitted anchor date.
    /// Omitted transitions can fall on the same date.
    /// A complete table with observances has open coverage when each represented side covers the remaining civil years.
    /// An empty table has no end date and supplies no transition.
    /// Answers after a finite end carry [`AnswerBasis::BeyondKnownTransitions`].
    ///
    /// [`AnswerBasis::BeyondKnownTransitions`]: crate::internal::tz::AnswerBasis::BeyondKnownTransitions
    #[must_use]
    pub const fn coverage_end(&self) -> Option<CivilDate> {
        self.coverage_end
    }

    /// The first admitted anchor date, or `None` for an empty table.
    ///
    /// Before the first anchor, the table extends the first observance's `TZOFFSETFROM` to the past.
    /// Answers carry [`AnswerBasis::BeforeKnownTransitions`](crate::internal::tz::AnswerBasis::BeforeKnownTransitions).
    #[must_use]
    pub const fn coverage_start(&self) -> Option<CivilDate> {
        self.coverage_start
    }

    /// Whether this table declares no observance at all.
    ///
    /// RFC 5545 section 3.6.5 requires at least one, so such a table came from a file that violated it, and it answers nothing rather than answering UTC.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observances.is_empty()
    }
}

/// Charge one observance at a time and return the accepted count and the first refusal.
fn admitted_count(offered: usize, meter: &mut Meter) -> (usize, Option<LimitExceeded>) {
    let mut admitted = 0_usize;
    while admitted < offered {
        if let Err(breach) = meter.try_charge_vtimezone_observances(1) {
            return (admitted, Some(breach));
        }
        admitted = admitted.saturating_add(1);
    }
    (admitted, None)
}

/// Interpret an observance's local `DTSTART` using its `TZOFFSETFROM` under RFC 5545 section 3.6.5.
/// An onset outside the timeline sorts before every representable instant.
fn onset_of(observance: Observance) -> Option<Instant> {
    observance.start().at_offset(observance.offset_from())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OrderedObservance {
    onset: Option<Instant>,
    observance: Observance,
}

impl OrderedObservance {
    pub(super) fn new(observance: Observance) -> Self {
        Self {
            onset: onset_of(observance),
            observance,
        }
    }

    pub(super) const fn into_observance(self) -> Observance {
        self.observance
    }
}

/// The final date of coverage for a complete table.
///
/// Coverage remains open when each represented side has an open rule or a counted end beyond the civil range.
/// If either kind runs out, coverage ends at the last finite date that any observance supports.
/// For example, an endless daylight rule and a finite list of standard onsets cover the zone through the last standard onset.
fn coverage_end_of(observances: &[Observance]) -> Option<CivilDate> {
    if sides_of(observances)
        .into_iter()
        .filter(|side| side.present)
        .all(|side| side.endless)
    {
        return None;
    }
    let mut latest: Option<CivilDate> = None;
    for covered in observances.iter().filter_map(|held| held.covered_through()) {
        latest = Some(match latest {
            Some(known) if known >= covered => known,
            _ => covered,
        });
    }
    latest
}

/// Coverage for either the `STANDARD` or the `DAYLIGHT` observances in a definition.
#[derive(Clone, Copy, Debug)]
struct Side {
    /// Whether the definition carries an observance of this kind at all.
    present: bool,
    /// Whether one of them repeats by a rule with no `UNTIL`.
    endless: bool,
}

/// The two sides of `observances`, as [`Side`] reads them.
fn sides_of(observances: &[Observance]) -> [Side; 2] {
    let mut sides = [Side {
        present: false,
        endless: false,
    }; 2];
    for observance in observances {
        let Some(side) = sides.get_mut(usize::from(observance.daylight())) else {
            continue;
        };
        side.present = true;
        side.endless |= observance.covered_through().is_none();
    }
    sides
}

/// The limit that prevented admission of a `VTIMEZONE` definition.
///
/// The error holds the full table so the caller can inspect the refused definition.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZoneSetError {
    /// A prior resource refusal prevented this table's admission.
    Exhausted(TransitionTable),
    /// The set already holds as many zones as the caller's policy admits.
    TooMany(TransitionTable, LimitExceeded),
}

impl ZoneSetError {
    /// The table that exceeded the admission limit.
    #[must_use]
    pub const fn table(&self) -> &TransitionTable {
        match self {
            Self::Exhausted(table) | Self::TooMany(table, _) => table,
        }
    }

    /// The diagnostic code for the resource limit that refused the definition.
    ///
    /// A resource refusal can make a declared zone unavailable even when the calendar defines its identifier.
    #[must_use]
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        match self {
            Self::Exhausted(_) => DiagnosticCode::ResourceBudgetExhausted,
            Self::TooMany(_, breach) => Self::code_for_limit(*breach),
        }
    }

    pub(super) const fn code_for_limit(breach: LimitExceeded) -> DiagnosticCode {
        match breach {
            LimitExceeded::Exhausted => DiagnosticCode::ResourceBudgetExhausted,
            _ => DiagnosticCode::VtimezoneComponentsTruncated,
        }
    }
}

/// The result of admitting a time zone definition.
///
/// Under RFC 5545 section 3.8.3.1, a `TZID` identifies one definition within its calendar.
/// The set keeps every definition under a repeated identifier so callers can inspect the conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ZoneAdmission {
    /// The only definition of its identifier so far.
    Sole,
    /// A second or later definition of an identifier already in the set.
    /// Both stay reachable.
    Repeated,
}

impl ZoneAdmission {
    /// The code an emitter reports this admission under, absent for the ordinary case.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Sole => None,
            Self::Repeated => Some(DiagnosticCode::DuplicateTimeZoneIdentifier),
        }
    }
}

/// The zone definitions one calendar carries.
///
/// The set orders identifiers by their exact bytes for binary search.
/// Callers handle identifier aliases as specified in `docs/adr/0003`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VtimezoneSet {
    /// The tables, ascending by identifier.
    tables: Vec<TransitionTable>,
}

pub(super) struct ZoneReservation(());

impl VtimezoneSet {
    /// A set holding nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { tables: Vec::new() }
    }

    /// Admit `table` and charge the meter for one definition.
    ///
    /// The set preserves input order among definitions with the same identifier.
    /// [`ZoneAdmission::Repeated`] identifies a duplicate, including one after an empty placeholder.
    /// Resource exhaustion or the zone count limit returns the offered table in a [`ZoneSetError`].
    pub fn insert(
        &mut self,
        table: TransitionTable,
        meter: &mut Meter,
    ) -> Result<ZoneAdmission, ZoneSetError> {
        let reservation = match Self::reserve(meter) {
            Ok(reservation) => reservation,
            Err(LimitExceeded::Exhausted) => return Err(ZoneSetError::Exhausted(table)),
            Err(breach) => return Err(ZoneSetError::TooMany(table, breach)),
        };
        Ok(self.insert_reserved(table, reservation))
    }

    pub(super) fn reserve(meter: &mut Meter) -> Result<ZoneReservation, LimitExceeded> {
        meter.try_charge_vtimezone_component()?;
        Ok(ZoneReservation(()))
    }

    pub(super) fn insert_reserved(
        &mut self,
        table: TransitionTable,
        _: ZoneReservation,
    ) -> ZoneAdmission {
        let held = self.definitions(table.tzid().as_str()).len();
        let admission = if held == 0 {
            ZoneAdmission::Sole
        } else {
            ZoneAdmission::Repeated
        };
        // Insert after all definitions with the same identifier to preserve their input order.
        let index = self
            .tables
            .partition_point(|held| held.tzid().as_str() <= table.tzid().as_str());
        self.tables.insert(index, table);
        admission
    }

    /// Find `tzid` by exact bytes and return its first definition with a transition.
    ///
    /// If all matching definitions are empty, return the first one in file order.
    /// [`VtimezoneSet::definitions`] returns every matching definition.
    #[must_use]
    pub fn table(&self, tzid: &str) -> Option<&TransitionTable> {
        let held = self.definitions(tzid);
        held.iter()
            .find(|table| !table.is_empty())
            .or_else(|| held.first())
    }

    /// Every definition of `tzid`, in the order the calendar wrote them.
    #[must_use]
    pub fn definitions(&self, tzid: &str) -> &[TransitionTable] {
        let first = self
            .tables
            .partition_point(|table| table.tzid().as_str() < tzid);
        let past = self
            .tables
            .partition_point(|table| table.tzid().as_str() <= tzid);
        self.tables.get(first..past).unwrap_or(&[])
    }

    /// All definitions in identifier order, preserving file order within each identifier.
    #[must_use]
    pub fn tables(&self) -> &[TransitionTable] {
        &self.tables
    }

    /// The number of distinct zone identifiers.
    ///
    /// Duplicate definitions count once here and each appears in [`VtimezoneSet::tables`].
    #[must_use]
    pub fn len(&self) -> usize {
        let mut counted = 0_usize;
        let mut previous: Option<&str> = None;
        for table in &self.tables {
            let name = table.tzid().as_str();
            if previous != Some(name) {
                counted = counted.saturating_add(1);
                previous = Some(name);
            }
        }
        counted
    }

    /// Whether this set holds no zone at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

/// The identifier and admission status of one staged time zone definition.
#[derive(Debug, PartialEq, Eq)]
pub struct StagedDefinition {
    /// Decoded identifier from the definition.
    pub tzid: Box<str>,
    /// Whether the staged candidates include a witness beyond the remaining limit.
    pub exceeds_limit: bool,
}

/// Read a `VTIMEZONE` from a calendar component or another source.
pub trait ObservanceReader {
    /// Append the earliest observances within the remaining limit and one overflow witness if present.
    /// [`TransitionTable::new`] charges the meter and reports truncation.
    ///
    /// Return `None` when the definition lacks a usable `TZID`.
    /// Report unsupported rules and missing observances through `sink`.
    fn read_vtimezone(
        &self,
        meter: &mut Meter,
        sink: &mut dyn DiagnosticSink,
        out: &mut Vec<Observance>,
    ) -> Option<StagedDefinition>;
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics, Limits,
        Meter, UtcOffset, Weekday,
    };

    use super::{
        NthWeek, Observance, RuleDay, TransitionTable, VtimezoneSet, YearlyRule, ZoneAdmission,
        ZoneSetError,
    };

    fn two_am() -> CivilTime {
        CivilTime::from_hms(2, 0, 0).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8) -> CivilDateTime {
        CivilDateTime::new(CivilDate::from_ymd(year, month, day).unwrap(), two_am())
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    fn dated(year: u16, month: u8, day: u8) -> Observance {
        Observance::new(
            stamp(year, month, day),
            offset(3600),
            offset(7200),
            true,
            None,
        )
    }

    fn ruled(through: Option<CivilDate>) -> Observance {
        let rule = YearlyRule::new(
            3,
            RuleDay::Nth {
                weekday: Weekday::Sunday,
                week: NthWeek::Last,
            },
            two_am(),
            through,
        )
        .unwrap();
        Observance::new(
            stamp(2007, 3, 25),
            offset(3600),
            offset(7200),
            true,
            Some(rule),
        )
    }

    fn table(observances: Vec<Observance>, meter: &mut Meter) -> TransitionTable {
        TransitionTable::new(
            Box::from("Europe/Berlin"),
            observances,
            meter,
            &mut IgnoreDiagnostics,
        )
    }

    #[test]
    fn a_month_no_year_has_is_not_a_rule() {
        assert_eq!(
            YearlyRule::new(13, RuleDay::LastDayOfMonth, two_am(), None),
            None
        );
        assert_eq!(
            YearlyRule::new(0, RuleDay::LastDayOfMonth, two_am(), None),
            None
        );
        assert!(YearlyRule::new(12, RuleDay::DayOfMonth(31), two_am(), None).is_some());
    }

    /// Table construction sorts explicit transitions by onset.
    #[test]
    fn a_table_sorts_what_it_is_handed() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let built = table(
            alloc::vec![dated(2029, 3, 25), dated(2027, 3, 28), dated(2028, 3, 26)],
            &mut meter,
        );
        let starts: Vec<u16> = built
            .observances()
            .iter()
            .map(|observance| observance.start().date().year())
            .collect();
        assert_eq!(starts, alloc::vec![2027, 2028, 2029]);
        assert!(!built.is_truncated());
        assert_eq!(built.tzid().as_str(), "Europe/Berlin");
    }

    /// Explicit dates and rule end dates bound coverage.
    #[test]
    fn a_date_driven_table_stops_where_its_dates_do_and_a_rule_driven_one_does_not() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let finite = table(
            alloc::vec![dated(2027, 3, 28), dated(2029, 3, 25)],
            &mut meter,
        );
        assert_eq!(
            finite.coverage_end(),
            CivilDate::from_ymd(2029, 3, 25),
            "an explicit table covers its last date and nothing after it"
        );

        let endless = table(alloc::vec![dated(2027, 3, 28), ruled(None)], &mut meter);
        assert_eq!(
            endless.coverage_end(),
            None,
            "one rule with no UNTIL means the zone knows the future"
        );

        let bounded = CivilDate::from_ymd(2035, 12, 31);
        let expiring = table(alloc::vec![ruled(bounded), dated(2027, 3, 28)], &mut meter);
        assert_eq!(expiring.coverage_end(), bounded);
    }

    /// Truncation keeps the earliest onsets and reports the observance limit.
    #[test]
    fn observances_past_the_caller_s_bound_are_dropped_from_the_end_and_reported() {
        let limits = Limits::DEFAULT.with_max_vtimezone_observances(2);
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let built = TransitionTable::new(
            Box::from("Europe/Berlin"),
            alloc::vec![dated(2029, 3, 25), dated(2027, 3, 28), dated(2028, 3, 26)],
            &mut meter,
            &mut reported,
        );
        assert_eq!(built.observances().len(), 2);
        assert!(built.is_truncated());
        assert_eq!(
            built.coverage_end(),
            CivilDate::from_ymd(2028, 3, 26),
            "coverage ends earlier rather than the table acquiring a hole"
        );
        assert_eq!(
            reported.first().map(|entry| entry.code()),
            Some(DiagnosticCode::VtimezoneObservancesTruncated)
        );
    }

    /// Admission reports a duplicate identifier and preserves both definitions.
    #[test]
    fn a_second_definition_of_one_zone_is_kept_beside_the_first_and_reported() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut set = VtimezoneSet::new();
        assert!(set.is_empty());
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Sole)
        );
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Repeated),
            "the second definition is admitted beside the first, not handed back"
        );
        assert_eq!(
            ZoneAdmission::Repeated.diagnostic_code(),
            Some(DiagnosticCode::DuplicateTimeZoneIdentifier)
        );
        assert_eq!(set.len(), 1, "one identifier");
        assert_eq!(
            set.definitions("Europe/Berlin").len(),
            2,
            "two readings of it, both reachable"
        );
        assert_eq!(set.tables().len(), 2);
        assert!(set.table("Europe/Berlin").is_some());
        assert!(
            set.table("europe/berlin").is_none(),
            "lookup is by exact bytes, which is what keeps aliasing the caller's step"
        );
    }

    /// A definition carrying nothing may not shadow one carrying the zone's own rules, whatever order the file wrote the two in.
    #[test]
    fn an_empty_definition_does_not_shadow_a_definition_that_holds_transitions() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut set = VtimezoneSet::new();
        assert!(
            set.insert(table(Vec::new(), &mut meter), &mut meter)
                .is_ok()
        );
        let real = table(alloc::vec![dated(2027, 3, 28)], &mut meter);
        assert!(set.insert(real, &mut meter).is_ok());
        assert_eq!(
            set.table("Europe/Berlin").map(TransitionTable::is_empty),
            Some(false),
            "the placeholder was written first and the definition is what answers"
        );
        assert_eq!(set.definitions("Europe/Berlin").len(), 2);
    }

    #[test]
    fn a_zone_count_past_the_caller_s_bound_is_refused_under_its_own_dimension() {
        let limits = Limits::DEFAULT.with_max_vtimezone_components(1);
        let mut meter = Meter::new(limits);
        let mut set = VtimezoneSet::new();
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Sole)
        );
        let second = TransitionTable::new(
            Box::from("America/New_York"),
            Vec::new(),
            &mut meter,
            &mut IgnoreDiagnostics,
        );
        let refused = set.insert(second, &mut meter);
        assert!(matches!(refused, Err(ZoneSetError::TooMany(_, _))));
        assert_eq!(set.len(), 1);
    }
}
