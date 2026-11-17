// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 3 — applying the eight `BYxxx` parts, driven by the expand/limit table.
//!
//! # What this unit owns
//!
//! Turning one period into that period's candidate set, by walking [`RulePart::ALL`] in the RFC's own row order and asking [`crate::internal::recur::table::effect`] what each part does.
//! A part that expands multiplies the working set; a part that limits filters it; a part that is not applicable is skipped.
//! `BYSETPOS` is *not* applied here — it is unit 4's, and applying it in this pass is the single most common way this gets built wrong.
//!
//! # How the table drives it
//!
//! The engine is a selection over the days the period holds and the clock readings it admits,
//! and [`crate::internal::recur::table::effect`] decides four things about it, none of which is written down twice:
//!
//! 1. **[`PartEffect::NotApplicable`] drops the part entirely.** `BYMONTHDAY` under `FREQ=WEEKLY` is `N/A` in the printed table, so a weekly rule carrying one recurs as though it did not.
//!    Implementations that filter on it anyway disagree with the RFC here.
//! 2. **[`PartEffect::expands`] decides whether a part may move a candidate off `DTSTART`'s day.** RFC 5545 section 3.3.10 takes every field the rule leaves unstated from `DTSTART`,
//!    and a *limiting* part only removes days that default gave; an *expanding* part replaces the default outright.
//!    This is the whole of the difference the milestone brief names:
//!    `FREQ=MONTHLY;BYMONTHDAY=2,15` expands to two days in a month whose `DTSTART` names neither, and `FREQ=DAILY;BYMONTHDAY=2,15` keeps the one day the period already fixed only when it is the 2nd or the 15th.
//!    Read the `MONTHLY` cell as `Limit` and the first rule yields nothing at all.
//! 3. **[`PartEffect::ExpandWeekdays`] carries the scope a `BYDAY` ordinal counts within.** `-1MO` is the last Monday *of something*, and Note 1 and Note 2 decide what — which is why the table resolves the notes rather than this file.
//! 4. **[`PartEffect::expands`] also decides whether a missing day is reported.** Only a part that expands *generates* an instance, so only an expanding part can generate one whose date does not exist; a limiting part that matches nothing has removed a day rather than invented one.
//!
//! Two classifications appear here that are *not* the table and must not be mistaken for it.
//! [`Span`] says how much of the calendar one period pins before any part is read, which is the period walk's subject rather than section 3.3.10's.
//! `DATE_PARTS` and `TIME_PARTS` say which coordinate a part addresses, which is a fact about the part's name.
//! Neither says anything about expanding or limiting, and neither is consulted for that.
//!
//! # What this unit must not do
//!
//! - It must not hard-code any cell of the table.
//!   Every branch reads [`crate::internal::recur::table::effect`].
//! - It must not be one `match`.
//!   This crate's Clippy profile bounds a function at 100 lines and a cognitive complexity of 15; a `BYxxx` application written as one match fails both,
//!   which is the gate asking for the table-driven shape rather than obstructing it.
//! - It moves a missing date only for an explicit RFC 7529 `SKIP` policy.
//!   `FREQ=MONTHLY;BYMONTHDAY=31` without that extension has no February instance under RFC 5545 section 3.3.10.
//!   The skip is reported with [`DiagnosticCode::NonexistentRecurrenceInstance`], and the skipped candidate is still charged, because it was still generated (`docs/adr/0011`).
//! - It must not emit anything.
//!   Candidates leave this unit as a period-local set.
//!
//! # What it charges, and why the charge is where it is
//!
//! Every date the period contributes is charged as a candidate as it is enumerated, before any part has had a chance to reject it, and so is every clock reading a time part produces and every date-and-time pair that survives.
//! That is deliberately more than the count of candidates that come out: `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` yields nothing in any period, and a budget charged for what a period *produced* would let it walk the calendar forever without ever firing.
//! Charging what the period *cost* bounds the walk in exactly the cases the milestone brief names as hostile — a rule that matches rarely, and one that matches never.
//!
//! The period is not opened here.
//! `Meter::open_period` clears the per-period ceiling, and unit 7 owns where that happens so that the charges it makes on either side of an expansion land in the same period as this one's.
//! A caller expanding a period on its own opens it itself.

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::num::NonZeroI8;

use crate::internal::core::{
    CalendarMonth, CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, DiagnosticSink,
    LimitExceeded, Location, Meter, Severity, UtcOffset, Weekday, report_diagnostic,
};

use crate::internal::recur::calendar::{self, CalendarArithmeticError, CalendarFrame, DateSpan};
use crate::internal::recur::period::Period;
use crate::internal::recur::rule::{Freq, RecurrenceRule, RulePart, Skip, WeekdayNum};
use crate::internal::recur::table::{PartEffect, PartsPresent, WeekdayScope, effect};

/// The rule parts that name a coordinate of the date, in the table's row order.
///
/// A classification by what the part addresses, not by what it does: every one of these is `Expand` under some frequency and `Limit` under another, and which it is here is [`crate::internal::recur::table::effect`]'s answer and never this list's.
const DATE_PARTS: [RulePart; 5] = [
    RulePart::Month,
    RulePart::WeekNo,
    RulePart::YearDay,
    RulePart::MonthDay,
    RulePart::Day,
];

/// The rule parts that name a coordinate of the clock, in the table's row order.
const TIME_PARTS: [RulePart; 3] = [RulePart::Hour, RulePart::Minute, RulePart::Second];

/// Days in a week, which is the stride every weekday ordinal counts in.
const DAYS_PER_WEEK: u32 = 7;

/// The same stride, in the width a count of days in a period is kept in.
const DAYS_PER_WEEK_U16: u16 = 7;

/// One period's candidates, ascending and deduplicated.
///
/// Ascending because RFC 5545 section 3.3.10 counts `BYSETPOS` positions in chronological order rather than in the order the file wrote the parts, and unit 4 must not have to re-derive that.
/// Deduplicated because the recurrence set is a set: `BYWEEKNO` expands to a week and `BYDAY` then names one day of it, and the two arrive at that day once per day of the week they started from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CandidateSet {
    /// The candidates, ascending and without repeats.
    stamps: Vec<CivilDateTime>,
}

impl CandidateSet {
    /// The candidates, ascending.
    #[must_use]
    pub fn as_slice(&self) -> &[CivilDateTime] {
        &self.stamps
    }

    /// How many candidates the period produced.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stamps.len()
    }

    /// Whether the period produced none.
    ///
    /// Not the same statement as "the rule matched nothing": a period can cost a large budget and still be empty, which is what `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` does forever.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }

    /// A set holding exactly `stamps`, for a test that needs one it did not expand.
    ///
    /// `#[cfg(test)]` rather than a constructor on the public surface.
    /// The set remains ascending and deduplicated for [`crate::internal::recur::select`].
    /// [`expand_period`] establishes the invariant outside tests by sorting its candidates.
    /// Offering a door that takes any slice would let a caller state a set that is not one and get a `BYSETPOS=-1` answer that names the wrong instant.
    #[cfg(test)]
    pub(crate) fn from_ascending(stamps: &[CivilDateTime]) -> Self {
        let mut stamps = stamps.to_vec();
        stamps.sort_unstable();
        stamps.dedup();
        Self { stamps }
    }
}

/// Candidate generation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpansionError {
    /// Candidate generation reached a resource limit.
    Resource(LimitExceeded),
    /// Calendar coordinates could not be evaluated.
    Calendar(CalendarArithmeticError),
}

impl From<LimitExceeded> for ExpansionError {
    fn from(error: LimitExceeded) -> Self {
        Self::Resource(error)
    }
}

impl From<CalendarArithmeticError> for ExpansionError {
    fn from(error: CalendarArithmeticError) -> Self {
        Self::Calendar(error)
    }
}

struct DateContext<'a, S: DiagnosticSink + ?Sized> {
    frame: CalendarFrame,
    rule: &'a RecurrenceRule,
    anchor: CivilDateTime,
    meter: &'a mut Meter,
    sink: &'a mut S,
}

pub(super) fn candidate_lower_bound(
    period: Period,
    rule: &RecurrenceRule,
) -> Result<CivilDateTime, CalendarArithmeticError> {
    let anchor = period.anchor();
    let mut frame = CalendarFrame::new(rule.calendar_scale());
    let span = day_extent(rule, period, &mut frame)?;
    let (mut date, _) = span.writable()?.ok_or(CalendarArithmeticError)?;
    if rule.skip() != Skip::Omit && expands_year_days(rule, rule.parts_present()) {
        let year = frame.fields(anchor.date())?.year;
        let span = frame.year_span(year)?;
        let mut earliest = None;
        for value in rule.by_year_day().as_slice() {
            if let Some(candidate) = span
                .resolve(i32::from(*value), rule.skip())?
                .and_then(CivilDate::from_days_from_epoch)
            {
                earliest = Some(earliest.map_or(candidate, |held: CivilDate| held.min(candidate)));
            }
        }
        if let Some(earliest) = earliest {
            date = earliest;
        }
    }
    if rule.skip() == Skip::Backward && rule.has_part(RulePart::MonthDay) {
        let span = frame.fields(date)?.month_span;
        for value in rule.by_month_day().as_slice() {
            if let Some(candidate) = span
                .resolve(i32::from(*value), rule.skip())?
                .and_then(CivilDate::from_days_from_epoch)
            {
                date = date.min(candidate);
            }
        }
    }
    let clock = if date == anchor.date() {
        anchor.time()
    } else {
        CivilTime::MIDNIGHT
    };
    Ok(CivilDateTime::new(date, clock))
}

/// Generate a period's candidates in selector order, before position selection.
pub fn expand_period<S: DiagnosticSink + ?Sized>(
    period: Period,
    rule: &RecurrenceRule,
    dtstart: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<CandidateSet, ExpansionError> {
    let anchor = period.anchor();
    let present = rule.parts_present();
    let mut context = DateContext {
        frame: CalendarFrame::new(rule.calendar_scale()),
        rule,
        anchor,
        meter,
        sink,
    };
    let mut days = period_days(period, &mut context)?;
    apply_date_parts(&mut days, present, &mut context)?;
    if !has_expanding_day_part(rule, present) {
        apply_default_day(
            &mut days,
            effective_span(rule, present),
            dtstart.date(),
            &mut context,
        )?;
    }
    let clocks = period_times(
        rule,
        present,
        anchor.time(),
        dtstart.time(),
        &mut *context.meter,
    )?;
    Ok(assemble(&days, &clocks, &mut *context.meter)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    Year,
    Month,
    Week,
    Day,
}

impl Span {
    const fn depth(self) -> u8 {
        match self {
            Self::Year => 0,
            Self::Month => 1,
            Self::Week => 2,
            Self::Day => 3,
        }
    }

    const fn of(freq: Freq) -> Self {
        match freq {
            Freq::Yearly => Self::Year,
            Freq::Monthly => Self::Month,
            Freq::Weekly => Self::Week,
            Freq::Daily | Freq::Hourly | Freq::Minutely | Freq::Secondly => Self::Day,
        }
    }

    const fn narrowed_by(self, other: Self) -> Self {
        if other.depth() > self.depth() {
            other
        } else {
            self
        }
    }
}

fn effective_span(rule: &RecurrenceRule, present: PartsPresent) -> Span {
    let mut span = Span::of(rule.freq());
    for (part, named) in [
        (RulePart::Month, Span::Month),
        (RulePart::WeekNo, Span::Week),
    ] {
        if rule.has_part(part) && effect(rule.freq(), part, present).expands() {
            span = span.narrowed_by(named);
        }
    }
    span
}

fn has_expanding_day_part(rule: &RecurrenceRule, present: PartsPresent) -> bool {
    [RulePart::YearDay, RulePart::MonthDay, RulePart::Day]
        .into_iter()
        .any(|part| rule.has_part(part) && effect(rule.freq(), part, present).expands())
}

fn expands_year_days(rule: &RecurrenceRule, present: PartsPresent) -> bool {
    rule.has_part(RulePart::YearDay) && effect(rule.freq(), RulePart::YearDay, present).expands()
}

fn day_extent(
    rule: &RecurrenceRule,
    period: Period,
    frame: &mut CalendarFrame,
) -> Result<DateSpan, CalendarArithmeticError> {
    if let Some(extent) = period.date_extent() {
        return Ok(extent);
    }
    let anchor = period.anchor().date();
    let fields = frame.fields(anchor)?;
    match Span::of(rule.freq()) {
        Span::Year => Ok(fields.year_span),
        Span::Month => Ok(fields.month_span),
        Span::Week => Err(CalendarArithmeticError),
        Span::Day => DateSpan::new(anchor.days_from_epoch().ok_or(CalendarArithmeticError)?, 1),
    }
}

fn append_span<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    span: DateSpan,
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    let Some((first, length)) = span.writable()? else {
        return Ok(());
    };
    for offset in 0..length {
        let date = first
            .checked_add_days(i64::from(offset))
            .ok_or(CalendarArithmeticError)?;
        context.meter.try_charge_candidate()?;
        days.push(date);
    }
    Ok(())
}

fn period_days<S: DiagnosticSink + ?Sized>(
    period: Period,
    context: &mut DateContext<'_, S>,
) -> Result<Vec<CivilDate>, ExpansionError> {
    let span = day_extent(context.rule, period, &mut context.frame)?;
    let mut days = Vec::new();
    append_span(&mut days, span, context)?;
    Ok(days)
}

fn retain_dates<F>(days: &mut Vec<CivilDate>, mut keep: F) -> Result<(), CalendarArithmeticError>
where
    F: FnMut(CivilDate) -> Result<bool, CalendarArithmeticError>,
{
    let mut written = 0;
    for index in 0..days.len() {
        let date = *days.get(index).ok_or(CalendarArithmeticError)?;
        if keep(date)? {
            *days.get_mut(written).ok_or(CalendarArithmeticError)? = date;
            written = written.checked_add(1).ok_or(CalendarArithmeticError)?;
        }
    }
    days.truncate(written);
    Ok(())
}

fn apply_date_parts<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    present: PartsPresent,
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    let rule = context.rule;
    let scope = weekday_scope(rule.freq(), present);
    for part in RulePart::ALL {
        if !DATE_PARTS.contains(&part) || !rule.has_part(part) {
            continue;
        }
        let action = effect(rule.freq(), part, present);
        if action == PartEffect::NotApplicable {
            continue;
        }
        if part == RulePart::Month && action.expands() {
            select_months(days, rule.by_month().as_slice(), context)?;
            continue;
        }
        if action.expands()
            && rule.skip() != Skip::Omit
            && correct_date_part(days, part, present, context)?
        {
            continue;
        }
        if action.expands() {
            report_absent_days(days, part, context)?;
        }
        retain_dates(days, |date| {
            date_part_matches(part, date, rule, scope, &mut context.frame)
        })?;
    }
    Ok(())
}

fn distinct_years(
    days: &[CivilDate],
    frame: &mut CalendarFrame,
) -> Result<Vec<i32>, CalendarArithmeticError> {
    let mut years = Vec::new();
    for date in days {
        let year = frame.month_fields(*date)?.year;
        if years.last() != Some(&year) {
            years.push(year);
        }
    }
    Ok(years)
}

fn distinct_months(
    days: &[CivilDate],
    frame: &mut CalendarFrame,
) -> Result<Vec<(i32, CalendarMonth)>, CalendarArithmeticError> {
    let mut months = Vec::new();
    for date in days {
        let fields = frame.month_fields(*date)?;
        let key = (fields.year, fields.month);
        if months.last() != Some(&key) {
            months.push(key);
        }
    }
    Ok(months)
}

fn select_months<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    values: &[CalendarMonth],
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    let mut wanted: Vec<DateSpan> = Vec::new();
    let mut repairs: Vec<DateSpan> = Vec::new();
    for year in distinct_years(days, &mut context.frame)? {
        for value in values {
            let exact = context.frame.month_span(year, *value)?;
            let resolved = context
                .frame
                .resolve_month(year, *value, context.rule.skip())?;
            match resolved {
                Some(span) => {
                    if !wanted.iter().any(|held| held.first() == span.first()) {
                        wanted.push(span);
                    }
                    if exact.is_none() {
                        context.meter.try_charge_candidate()?;
                        if !repairs.iter().any(|held| held.first() == span.first()) {
                            repairs.push(span);
                        }
                    }
                },
                None => report_absent(context)?,
            }
        }
    }
    retain_dates(days, |date| {
        let day = date.days_from_epoch().ok_or(CalendarArithmeticError)?;
        Ok(wanted
            .iter()
            .any(|span| day >= span.first() && day < span.end()))
    })?;
    let mut added = Vec::new();
    for span in repairs {
        let Some((first, length)) = span.writable()? else {
            continue;
        };
        for offset in 0..length {
            let date = first
                .checked_add_days(i64::from(offset))
                .ok_or(CalendarArithmeticError)?;
            if days.binary_search(&date).is_err() {
                context.meter.try_charge_candidate()?;
                added.push(date);
            }
        }
    }
    days.extend(added);
    days.sort_unstable();
    days.dedup();
    Ok(())
}

fn correct_date_part<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    part: RulePart,
    present: PartsPresent,
    context: &mut DateContext<'_, S>,
) -> Result<bool, ExpansionError> {
    match part {
        RulePart::YearDay => *days = corrected_year_days(days, context)?,
        RulePart::MonthDay if expands_year_days(context.rule, present) => {
            retain_dates(days, |date| {
                month_day_matches(
                    date,
                    context.rule.by_month_day().as_slice(),
                    &mut context.frame,
                )
            })?;
        },
        RulePart::MonthDay => {
            *days = corrected_month_days(days, context.rule.by_month_day().as_slice(), context)?;
        },
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::Day
        | RulePart::Hour
        | RulePart::Minute
        | RulePart::Second
        | RulePart::SetPos => return Ok(false),
    }
    Ok(true)
}

enum DateSelection {
    Candidate(CivilDate),
    Filtered,
    Nonexistent,
}

fn select_year_day(
    days: &[CivilDate],
    year: i32,
    value: i16,
    rule: &RecurrenceRule,
    frame: &mut CalendarFrame,
) -> Result<DateSelection, CalendarArithmeticError> {
    let span = frame.year_span(year)?;
    let source = span.resolve(i32::from(value), Skip::Omit)?;
    let boundary = if value.is_negative() {
        span.first()
    } else {
        span.end().checked_sub(1).ok_or(CalendarArithmeticError)?
    };
    let held = CivilDate::from_days_from_epoch(source.unwrap_or(boundary));
    if held.is_none_or(|date| days.binary_search(&date).is_err()) {
        return Ok(DateSelection::Filtered);
    }
    if let Some(date) = source.and_then(CivilDate::from_days_from_epoch) {
        return Ok(DateSelection::Candidate(date));
    }
    if rule.has_part(RulePart::MonthDay) {
        return Ok(DateSelection::Filtered);
    }
    Ok(
        match span
            .resolve(i32::from(value), rule.skip())?
            .and_then(CivilDate::from_days_from_epoch)
        {
            Some(date) => DateSelection::Candidate(date),
            None => DateSelection::Nonexistent,
        },
    )
}

fn corrected_year_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    context: &mut DateContext<'_, S>,
) -> Result<Vec<CivilDate>, ExpansionError> {
    let mut corrected = Vec::new();
    for year in distinct_years(days, &mut context.frame)? {
        for value in context.rule.by_year_day().as_slice() {
            match select_year_day(days, year, *value, context.rule, &mut context.frame)? {
                DateSelection::Candidate(date) => {
                    context.meter.try_charge_candidate()?;
                    corrected.push(date);
                },
                DateSelection::Filtered => context.meter.try_charge_candidate()?,
                DateSelection::Nonexistent => report_absent(context)?,
            }
        }
    }
    corrected.sort_unstable();
    corrected.dedup();
    Ok(corrected)
}

fn weekday_scope(freq: Freq, present: PartsPresent) -> Option<WeekdayScope> {
    if !matches!(freq, Freq::Monthly | Freq::Yearly) {
        return None;
    }
    match effect(freq, RulePart::Day, present) {
        PartEffect::ExpandWeekdays(scope) => Some(scope),
        PartEffect::Limit => scope_without_day_parts(freq, present),
        PartEffect::NotApplicable | PartEffect::Expand => None,
    }
}

fn scope_without_day_parts(freq: Freq, present: PartsPresent) -> Option<WeekdayScope> {
    let reduced = RulePart::ALL
        .into_iter()
        .filter(|part| !matches!(part, RulePart::YearDay | RulePart::MonthDay))
        .filter(|part| present.has(*part))
        .fold(PartsPresent::NONE, PartsPresent::with);
    match effect(freq, RulePart::Day, reduced) {
        PartEffect::ExpandWeekdays(scope) => Some(scope),
        PartEffect::NotApplicable | PartEffect::Limit | PartEffect::Expand => None,
    }
}

fn date_part_matches(
    part: RulePart,
    date: CivilDate,
    rule: &RecurrenceRule,
    scope: Option<WeekdayScope>,
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    match part {
        RulePart::Month => Ok(rule
            .by_month()
            .as_slice()
            .contains(&frame.month_fields(date)?.month)),
        RulePart::WeekNo => week_no_matches(date, rule.by_week_no().as_slice(), rule.wkst(), frame),
        RulePart::YearDay => year_day_matches(date, rule.by_year_day().as_slice(), frame),
        RulePart::MonthDay => month_day_matches(date, rule.by_month_day().as_slice(), frame),
        RulePart::Day => weekday_matches(date, rule.by_day().as_slice(), scope, rule.wkst(), frame),
        RulePart::Hour | RulePart::Minute | RulePart::Second | RulePart::SetPos => Ok(true),
    }
}

fn month_day_matches(
    date: CivilDate,
    values: &[i8],
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    let fields = frame.month_fields(date)?;
    Ok(values.iter().any(|value| {
        position_named(i32::from(*value), u32::from(fields.days)) == Some(u32::from(fields.day))
    }))
}

fn year_day_matches(
    date: CivilDate,
    values: &[i16],
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    let fields = frame.fields(date)?;
    Ok(values.iter().any(|value| {
        position_named(i32::from(*value), u32::from(fields.year_span.days()))
            == Some(u32::from(fields.year_day))
    }))
}

fn week_no_matches(
    date: CivilDate,
    values: &[i8],
    wkst: Weekday,
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    let (year, number) = frame.week_of(date, wkst)?;
    let weeks = frame.week_year_span(year, wkst)?.days() / DAYS_PER_WEEK_U16;
    Ok(values.iter().any(|value| {
        position_named(i32::from(*value), u32::from(weeks)) == Some(u32::from(number))
    }))
}

fn weekday_matches(
    date: CivilDate,
    values: &[WeekdayNum],
    scope: Option<WeekdayScope>,
    wkst: Weekday,
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    let weekday = date.weekday().ok_or(CalendarArithmeticError)?;
    for value in values {
        if value.weekday() == weekday && ordinal_matches(date, value.ordinal(), scope, wkst, frame)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ordinal_matches(
    date: CivilDate,
    ordinal: Option<NonZeroI8>,
    scope: Option<WeekdayScope>,
    wkst: Weekday,
    frame: &mut CalendarFrame,
) -> Result<bool, CalendarArithmeticError> {
    let (Some(ordinal), Some(scope)) = (ordinal, scope) else {
        return Ok(true);
    };
    let (index, length) = weekday_position(date, scope, wkst, frame)?;
    Ok(position_named(i32::from(ordinal.get()), length) == Some(index))
}

fn weekday_position(
    date: CivilDate,
    scope: WeekdayScope,
    wkst: Weekday,
    frame: &mut CalendarFrame,
) -> Result<(u32, u32), CalendarArithmeticError> {
    let (offset, length) = match scope {
        WeekdayScope::Week => (
            u32::from(calendar::week_offset(
                date.weekday().ok_or(CalendarArithmeticError)?,
                wkst,
            )),
            DAYS_PER_WEEK,
        ),
        WeekdayScope::Month => {
            let fields = frame.month_fields(date)?;
            (
                u32::from(fields.day)
                    .checked_sub(1)
                    .ok_or(CalendarArithmeticError)?,
                u32::from(fields.days),
            )
        },
        WeekdayScope::Year => {
            let fields = frame.fields(date)?;
            (
                u32::from(fields.year_day)
                    .checked_sub(1)
                    .ok_or(CalendarArithmeticError)?,
                u32::from(fields.year_span.days()),
            )
        },
    };
    let index = offset
        .div_euclid(DAYS_PER_WEEK)
        .checked_add(1)
        .ok_or(CalendarArithmeticError)?;
    let first = offset.rem_euclid(DAYS_PER_WEEK);
    let count = length
        .checked_sub(first)
        .and_then(|value| value.checked_sub(1))
        .ok_or(CalendarArithmeticError)?
        .div_euclid(DAYS_PER_WEEK)
        .checked_add(1)
        .ok_or(CalendarArithmeticError)?;
    Ok((index, count))
}

fn position_named(value: i32, length: u32) -> Option<u32> {
    let length = i32::try_from(length).ok()?;
    let position = match value.cmp(&0) {
        Ordering::Greater => value,
        Ordering::Less => length.checked_add(value)?.checked_add(1)?,
        Ordering::Equal => return None,
    };
    if position >= 1 && position <= length {
        u32::try_from(position).ok()
    } else {
        None
    }
}

fn report_absent_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    part: RulePart,
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    match part {
        RulePart::MonthDay => {
            report_absent_month_days(days, context.rule.by_month_day().as_slice(), context)
        },
        RulePart::YearDay => {
            for year in distinct_years(days, &mut context.frame)? {
                let length = u32::from(context.frame.year_span(year)?.days());
                for value in context.rule.by_year_day().as_slice() {
                    if position_named(i32::from(*value), length).is_none() {
                        report_absent(context)?;
                    }
                }
            }
            Ok(())
        },
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::Day
        | RulePart::Hour
        | RulePart::Minute
        | RulePart::Second
        | RulePart::SetPos => Ok(()),
    }
}

fn report_absent_month_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    values: &[i8],
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    for (year, month) in distinct_months(days, &mut context.frame)? {
        let span = context
            .frame
            .month_span(year, month)?
            .ok_or(CalendarArithmeticError)?;
        for value in values {
            if position_named(i32::from(*value), u32::from(span.days())).is_none() {
                report_absent(context)?;
            }
        }
    }
    Ok(())
}

fn report_absent<S: DiagnosticSink + ?Sized>(
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    let code = DiagnosticCode::NonexistentRecurrenceInstance;
    let note = match context.anchor.at_offset(UtcOffset::UTC) {
        Some(instant) => Diagnostic::at_instant(code, Severity::Note, instant),
        None => Diagnostic::new(code, Severity::Note, Location::NOWHERE),
    };
    report_diagnostic(context.sink, context.meter, note);
    Ok(context.meter.try_charge_candidate()?)
}

fn corrected_month_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    values: &[i8],
    context: &mut DateContext<'_, S>,
) -> Result<Vec<CivilDate>, ExpansionError> {
    let mut corrected = Vec::new();
    for (year, month) in distinct_months(days, &mut context.frame)? {
        let span = context
            .frame
            .month_span(year, month)?
            .ok_or(CalendarArithmeticError)?;
        for value in values {
            match span
                .resolve(i32::from(*value), context.rule.skip())?
                .and_then(CivilDate::from_days_from_epoch)
            {
                Some(date) => {
                    context.meter.try_charge_candidate()?;
                    corrected.push(date);
                },
                None => report_absent(context)?,
            }
        }
    }
    corrected.sort_unstable();
    corrected.dedup();
    Ok(corrected)
}

fn apply_default_day<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    span: Span,
    start: CivilDate,
    context: &mut DateContext<'_, S>,
) -> Result<(), ExpansionError> {
    if matches!(span, Span::Year | Span::Month) {
        let seed = context.frame.month_fields(start)?;
        if span == Span::Year {
            select_months(days, &[seed.month], context)?;
        }
        let value = i8::try_from(seed.day).map_err(|_| CalendarArithmeticError)?;
        if context.rule.skip() == Skip::Omit {
            report_absent_month_days(days, &[value], context)?;
            retain_dates(days, |date| {
                Ok(context.frame.month_fields(date)?.day == seed.day)
            })?;
        } else {
            *days = corrected_month_days(days, &[value], context)?;
        }
    } else if span == Span::Week {
        let weekday = start.weekday().ok_or(CalendarArithmeticError)?;
        retain_dates(days, |date| Ok(date.weekday() == Some(weekday)))?;
    }
    Ok(())
}

#[cfg(test)]
fn week_of(date: CivilDate, wkst: Weekday) -> Option<(u16, u8)> {
    let (year, number) = CalendarFrame::new(crate::internal::core::CalendarScale::Gregorian)
        .week_of(date, wkst)
        .ok()?;
    Some((u16::try_from(year).ok()?, number))
}

#[cfg(test)]
fn week_offset(weekday: Weekday, wkst: Weekday) -> u8 {
    calendar::week_offset(weekday, wkst)
}
fn period_times(
    rule: &RecurrenceRule,
    present: PartsPresent,
    anchor: CivilTime,
    start: CivilTime,
    meter: &mut Meter,
) -> Result<Vec<CivilTime>, LimitExceeded> {
    let mut clocks = Vec::new();
    meter.try_charge_candidate()?;
    clocks.push(seed_time(rule.freq(), anchor, start));
    for part in RulePart::ALL {
        if !TIME_PARTS.contains(&part) || !rule.has_part(part) {
            continue;
        }
        let action = effect(rule.freq(), part, present);
        if action.limits() {
            clocks.retain(|time| {
                time_field(part, *time).is_some_and(|held| field_values(part, rule).contains(&held))
            });
        } else if action.expands() {
            clocks = expanded_times(&clocks, part, rule, meter)?;
        }
    }
    Ok(clocks)
}

/// The clock reading a period starts from before any time part is read.
///
/// RFC 5545 section 3.3.10 takes every unstated field from `DTSTART`, except the fields the period itself already fixed: an `HOURLY` period names its own hour and only the minute and the second below it fall back.
fn seed_time(freq: Freq, anchor: CivilTime, start: CivilTime) -> CivilTime {
    let (hour, minute, second) = match freq {
        Freq::Secondly => (anchor.hour(), anchor.minute(), anchor.second()),
        Freq::Minutely => (anchor.hour(), anchor.minute(), start.second()),
        Freq::Hourly => (anchor.hour(), start.minute(), start.second()),
        Freq::Daily | Freq::Weekly | Freq::Monthly | Freq::Yearly => {
            (start.hour(), start.minute(), start.second())
        },
    };
    CivilTime::from_hms(hour, minute, second).unwrap_or(start)
}

/// The working set with `part`'s field replaced by each value the rule lists.
fn expanded_times(
    clocks: &[CivilTime],
    part: RulePart,
    rule: &RecurrenceRule,
    meter: &mut Meter,
) -> Result<Vec<CivilTime>, LimitExceeded> {
    let mut widened = Vec::new();
    for time in clocks {
        for listed in field_values(part, rule) {
            // A value no clock can hold is dropped rather than folded onto a nearby one; the decoder already reported it out of range and kept the rest of the rule.
            let Some(moved) = with_field(*time, part, *listed) else {
                continue;
            };
            meter.try_charge_candidate()?;
            widened.push(moved);
        }
    }
    Ok(widened)
}

/// The values `part` lists, empty for a part that names no field of the clock.
fn field_values(part: RulePart, rule: &RecurrenceRule) -> &[u8] {
    match part {
        RulePart::Hour => rule.by_hour().as_slice(),
        RulePart::Minute => rule.by_minute().as_slice(),
        RulePart::Second => rule.by_second().as_slice(),
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::YearDay
        | RulePart::MonthDay
        | RulePart::Day
        | RulePart::SetPos => &[],
    }
}

/// The field of `time` that `part` names.
const fn time_field(part: RulePart, time: CivilTime) -> Option<u8> {
    match part {
        RulePart::Hour => Some(time.hour()),
        RulePart::Minute => Some(time.minute()),
        RulePart::Second => Some(time.second()),
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::YearDay
        | RulePart::MonthDay
        | RulePart::Day
        | RulePart::SetPos => None,
    }
}

/// `time` with the field `part` names set to `value`, or `None` when no clock reads that.
const fn with_field(time: CivilTime, part: RulePart, value: u8) -> Option<CivilTime> {
    match part {
        RulePart::Hour => CivilTime::from_hms(value, time.minute(), time.second()),
        RulePart::Minute => CivilTime::from_hms(time.hour(), value, time.second()),
        RulePart::Second => CivilTime::from_hms(time.hour(), time.minute(), value),
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::YearDay
        | RulePart::MonthDay
        | RulePart::Day
        | RulePart::SetPos => None,
    }
}

/// Pair every surviving day with every surviving clock reading, charged as each pair is made.
fn assemble(
    days: &[CivilDate],
    clocks: &[CivilTime],
    meter: &mut Meter,
) -> Result<CandidateSet, LimitExceeded> {
    let mut stamps: Vec<CivilDateTime> = Vec::new();
    for date in days {
        for time in clocks {
            meter.try_charge_candidate()?;
            stamps.push(CivilDateTime::new(*date, *time));
        }
    }
    // Chronological because `BYSETPOS` counts positions in that order and unit 4 must not have to re-derive it; deduplicated because the recurrence set is a set and two parts can name one instant.
    stamps.sort_unstable();
    stamps.dedup();
    Ok(CandidateSet { stamps })
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::{NonZeroI8, NonZeroU32};

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, LimitExceeded, Limits,
        Location, Meter, Severity, Weekday,
    };

    use super::{CandidateSet, expand_period, position_named, week_of, week_offset};
    use crate::internal::recur::period::PeriodWalk;
    use crate::internal::recur::rule::{
        ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RulePart, WeekdayNum,
    };
    use crate::internal::recur::table::{PartEffect, PartsPresent, WeekdayScope, effect};

    /// A local date and time, for a fixture that names one.
    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, minute, 0).unwrap();
        CivilDateTime::new(date, time)
    }

    /// A `BYDAY` entry.
    fn weekday(ordinal: Option<i8>, day: Weekday) -> WeekdayNum {
        WeekdayNum::new(ordinal.map(|count| NonZeroI8::new(count).unwrap()), day).unwrap()
    }

    /// Expand the `index`-th period of `rule`, answering the candidates and the diagnostics.
    ///
    /// The period comes from unit 2's walk rather than from a fixture, because that is how the engine composes the two and a period this file built itself would prove nothing about the pair.
    fn expand_at(
        rule: &RecurrenceRule,
        dtstart: CivilDateTime,
        index: usize,
    ) -> (Vec<CivilDateTime>, Vec<Diagnostic>) {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut kept: Vec<Diagnostic> = Vec::new();
        let period = PeriodWalk::new(dtstart, rule).nth(index).unwrap();
        meter.open_period();
        let set = expand_period(period, rule, dtstart, &mut meter, &mut kept).unwrap();
        (set.as_slice().to_vec(), kept)
    }

    /// The candidates of the `index`-th period, with the diagnostics dropped.
    fn candidates(
        rule: &RecurrenceRule,
        dtstart: CivilDateTime,
        index: usize,
    ) -> Vec<CivilDateTime> {
        expand_at(rule, dtstart, index).0
    }

    /// The expected column of a fixture, written as the RFC writes an occurrence list.
    fn expected(instants: &[(u16, u8, u8, u8, u8)]) -> Vec<CivilDateTime> {
        instants
            .iter()
            .map(|(year, month, day, hour, minute)| stamp(*year, *month, *day, *hour, *minute))
            .collect()
    }

    /// The row the milestone brief singles out, in both directions.
    ///
    /// The monthly rule names two days of a month whose `DTSTART` names neither, which is the fixture that tells expansion from limitation: read the `MONTHLY` cell as `Limit` and the answer is empty rather than two.
    #[test]
    fn by_month_day_expands_a_month_and_limits_a_day() {
        let start = stamp(1997, 9, 3, 9, 0);
        let monthly = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month_day(ByList::from_slice(&[2_i8, 15]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&monthly, start, 0),
            expected(&[(1997, 9, 2, 9, 0), (1997, 9, 15, 9, 0)])
        );

        let daily = RecurrenceRuleBuilder::new(Freq::Daily)
            .by_month_day(ByList::from_slice(&[2_i8, 15]))
            .build()
            .unwrap();
        assert!(
            candidates(&daily, start, 0).is_empty(),
            "September 3rd is neither the 2nd nor the 15th, and a limit adds no day"
        );
        assert_eq!(
            candidates(&daily, start, 12),
            expected(&[(1997, 9, 15, 9, 0)])
        );
    }

    /// The `N/A` cells are not filters.
    /// A weekly rule carrying `BYMONTHDAY` recurs as though it did not, which is the answer the printed table gives and several implementations do not.
    #[test]
    fn a_not_applicable_cell_drops_the_part_rather_than_filtering_on_it() {
        let start = stamp(1997, 9, 2, 9, 0);
        let weekly = RecurrenceRuleBuilder::new(Freq::Weekly)
            .by_month_day(ByList::from_slice(&[1_i8]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&weekly, start, 0),
            expected(&[(1997, 9, 2, 9, 0)]),
            "BYMONTHDAY is N/A under WEEKLY, so DTSTART's weekday still recurs"
        );

        let monthly = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_year_day(ByList::from_slice(&[1_i16]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&monthly, start, 0),
            expected(&[(1997, 9, 2, 9, 0)]),
            "BYYEARDAY is N/A under MONTHLY for the same reason"
        );
    }

    /// `BYMONTH`, both directions.
    /// The yearly rule is RFC 5545 section 3.8.5.3's "yearly in June and July", whose day comes from `DTSTART` because nothing else names one.
    #[test]
    fn by_month_expands_a_year_and_limits_a_month() {
        let start = stamp(1997, 6, 10, 9, 0);
        let yearly = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month(ByList::from_slice(&[6_u8, 7]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&yearly, start, 0),
            expected(&[(1997, 6, 10, 9, 0), (1997, 7, 10, 9, 0)])
        );

        let monthly = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month(ByList::from_slice(&[6_u8]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&monthly, start, 0),
            expected(&[(1997, 6, 10, 9, 0)])
        );
        assert!(
            candidates(&monthly, start, 1).is_empty(),
            "July is not June, and BYMONTH only limits under MONTHLY"
        );
    }

    /// `BYYEARDAY`, both directions.
    /// The yearly rule is section 3.8.5.3's "every 3rd year on the 1st, 100th and 200th day"; 1997 is common, so day 100 is April 10th and day 200 is July 19th.
    #[test]
    fn by_year_day_expands_a_year_and_limits_an_hour() {
        let start = stamp(1997, 1, 1, 9, 0);
        let interval = NonZeroU32::new(3).unwrap();
        let yearly = RecurrenceRuleBuilder::new(Freq::Yearly)
            .interval(interval)
            .by_year_day(ByList::from_slice(&[1_i16, 100, 200]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&yearly, start, 0),
            expected(&[(1997, 1, 1, 9, 0), (1997, 4, 10, 9, 0), (1997, 7, 19, 9, 0)])
        );

        let hourly = RecurrenceRuleBuilder::new(Freq::Hourly)
            .by_year_day(ByList::from_slice(&[2_i16]))
            .build()
            .unwrap();
        assert!(
            candidates(&hourly, start, 0).is_empty(),
            "January 1st is not the 2nd day, and a limit adds no day"
        );
    }

    /// `BYDAY` under `WEEKLY` expands the week, and under `DAILY` it limits the one day.
    #[test]
    fn by_day_expands_a_week_and_limits_a_day() {
        let start = stamp(1997, 9, 2, 9, 0);
        let weekly = RecurrenceRuleBuilder::new(Freq::Weekly)
            .by_day(ByList::from_slice(&[
                weekday(None, Weekday::Tuesday),
                weekday(None, Weekday::Thursday),
            ]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&weekly, start, 0),
            expected(&[(1997, 9, 2, 9, 0), (1997, 9, 4, 9, 0)])
        );

        let daily = RecurrenceRuleBuilder::new(Freq::Daily)
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Thursday)]))
            .build()
            .unwrap();
        assert_eq!(candidates(&daily, start, 0).as_slice(), []);
        assert_eq!(
            candidates(&daily, start, 2),
            expected(&[(1997, 9, 4, 9, 0)])
        );
    }

    /// Note 1, both branches.
    /// The first is section 3.8.5.3's "monthly on the first Friday";
    /// the second is its "every Friday the 13th", where `BYMONTHDAY` turns `BYDAY` into a limit and February 1998 is the first month that has one.
    #[test]
    fn note_one_expands_within_the_month_and_limits_beside_by_month_day() {
        let first_friday = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_day(ByList::from_slice(&[weekday(Some(1), Weekday::Friday)]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&first_friday, stamp(1997, 9, 5, 9, 0), 0),
            expected(&[(1997, 9, 5, 9, 0)])
        );

        let start = stamp(1997, 9, 2, 9, 0);
        let unlucky = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Friday)]))
            .by_month_day(ByList::from_slice(&[13_i8]))
            .build()
            .unwrap();
        assert!(
            candidates(&unlucky, start, 0).is_empty(),
            "September 13th 1997 is a Saturday"
        );
        assert_eq!(
            candidates(&unlucky, start, 5),
            expected(&[(1998, 2, 13, 9, 0)])
        );
    }

    /// Note 2, all four branches, each against a worked example.
    ///
    /// The limit branch is section 3.8.5.3's United States presidential election day.
    /// The week branch is its "Monday of week number 20".
    /// The month branch is its "every Thursday in March".
    /// The year branch is its "every 20th Monday of the year".
    #[test]
    fn note_two_resolves_every_branch_the_rfc_writes() {
        let election = RecurrenceRuleBuilder::new(Freq::Yearly)
            .interval(NonZeroU32::new(4).unwrap())
            .by_month(ByList::from_slice(&[11_u8]))
            .by_month_day(ByList::from_slice(&[2_i8, 3, 4, 5, 6, 7, 8]))
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Tuesday)]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&election, stamp(1996, 11, 5, 9, 0), 0),
            expected(&[(1996, 11, 5, 9, 0)])
        );

        let week_twenty = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_week_no(ByList::from_slice(&[20_i8]))
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Monday)]))
            .build()
            .unwrap();
        let spring = stamp(1997, 5, 12, 9, 0);
        assert_eq!(
            candidates(&week_twenty, spring, 0),
            expected(&[(1997, 5, 12, 9, 0)])
        );
        assert_eq!(
            candidates(&week_twenty, spring, 1),
            expected(&[(1998, 5, 11, 9, 0)])
        );

        let march = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month(ByList::from_slice(&[3_u8]))
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Thursday)]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&march, stamp(1997, 3, 13, 9, 0), 0),
            expected(&[
                (1997, 3, 6, 9, 0),
                (1997, 3, 13, 9, 0),
                (1997, 3, 20, 9, 0),
                (1997, 3, 27, 9, 0)
            ])
        );

        let twentieth = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_day(ByList::from_slice(&[weekday(Some(20), Weekday::Monday)]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&twentieth, stamp(1997, 5, 19, 9, 0), 0),
            expected(&[(1997, 5, 19, 9, 0)])
        );
    }

    /// Note 2's branch order is load-bearing: `BYWEEKNO` beats `BYMONTH`, so the Mondays are counted within week 20 and not within May.
    /// Held against the table directly as well as through an expansion, because the two could agree while both being wrong.
    #[test]
    fn by_week_no_beats_by_month_in_note_two() {
        let present = PartsPresent::NONE
            .with(RulePart::Month)
            .with(RulePart::WeekNo)
            .with(RulePart::Day);
        assert_eq!(
            effect(Freq::Yearly, RulePart::Day, present),
            PartEffect::ExpandWeekdays(WeekdayScope::Week)
        );

        let rule = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_week_no(ByList::from_slice(&[20_i8]))
            .by_month(ByList::from_slice(&[5_u8]))
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Monday)]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&rule, stamp(1997, 5, 12, 9, 0), 0),
            expected(&[(1997, 5, 12, 9, 0)]),
            "one Monday, the one in week 20, not the five Mondays of May"
        );
    }

    /// The rule `docs/adr/0011` names: candidates in every period and instances in none.
    ///
    /// February has no thirtieth in any Gregorian year, so the period costs a budget, says what it dropped and hands back nothing.
    /// A budget charged for what a period produced would let this rule walk the calendar forever.
    #[test]
    fn a_rule_no_year_can_satisfy_costs_a_budget_and_reports_what_it_dropped() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month(ByList::from_slice(&[2_u8]))
            .by_month_day(ByList::from_slice(&[30_i8]))
            .build()
            .unwrap();
        let start = stamp(2026, 2, 1, 9, 0);
        for index in 0..3 {
            let (stamps, notes) = expand_at(&rule, start, index);
            assert!(stamps.is_empty(), "February has no thirtieth");
            assert_eq!(
                notes.iter().map(|note| note.code()).collect::<Vec<_>>(),
                [DiagnosticCode::NonexistentRecurrenceInstance]
            );
        }

        let mut meter = Meter::new(Limits::DEFAULT);
        let mut kept: Vec<Diagnostic> = Vec::new();
        let period = PeriodWalk::new(start, &rule).next().unwrap();
        meter.open_period();
        expand_period(period, &rule, start, &mut meter, &mut kept).unwrap();
        assert!(
            meter.candidates_in_period() > 0,
            "a period that produced nothing still did the work of finding that out"
        );
    }

    /// The leap day.
    /// A `DTSTART` of February 29th names a date three years in four do not have, and section 3.3.10 says the instance is ignored rather than moved to the 28th.
    #[test]
    fn a_leap_day_start_skips_the_years_that_have_no_leap_day() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
        let start = stamp(2024, 2, 29, 9, 0);
        assert_eq!(
            candidates(&rule, start, 0),
            expected(&[(2024, 2, 29, 9, 0)])
        );

        let (stamps, notes) = expand_at(&rule, start, 1);
        assert!(stamps.is_empty(), "2025 has no February 29th");
        assert_eq!(
            notes.iter().map(|note| note.code()).collect::<Vec<_>>(),
            [DiagnosticCode::NonexistentRecurrenceInstance]
        );
        let note = notes.first().unwrap();
        assert_eq!(note.severity(), Severity::Note);
        assert_eq!(note.location(), Location::NOWHERE);
        assert!(
            note.instant().is_some(),
            "the diagnostic names the period that lost a candidate, since the candidate has \
             no instant of its own to name"
        );
        assert_eq!(
            candidates(&rule, start, 4),
            expected(&[(2028, 2, 29, 9, 0)])
        );
    }

    /// The month end, in both of the shapes that get it wrong.
    ///
    /// The first is section 3.8.5.3's "monthly on the first and last day of the month", where `BYMONTHDAY=-1` counts back from an end that moves.
    /// The second is a `DTSTART` on a 31st under `FREQ=MONTHLY`, which skips the months that have no 31st rather than clamping to their last day.
    #[test]
    fn the_month_end_is_counted_backwards_and_never_clamped() {
        let both_ends = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month_day(ByList::from_slice(&[1_i8, -1]))
            .build()
            .unwrap();
        let autumn = stamp(1997, 9, 30, 9, 0);
        assert_eq!(
            candidates(&both_ends, autumn, 0),
            expected(&[(1997, 9, 1, 9, 0), (1997, 9, 30, 9, 0)])
        );
        assert_eq!(
            candidates(&both_ends, autumn, 5),
            expected(&[(1998, 2, 1, 9, 0), (1998, 2, 28, 9, 0)]),
            "the last day of February is the 28th, and -1 finds it without naming it"
        );

        let plain = RecurrenceRuleBuilder::new(Freq::Monthly).build().unwrap();
        let year_end = stamp(1997, 1, 31, 9, 0);
        assert_eq!(
            candidates(&plain, year_end, 0),
            expected(&[(1997, 1, 31, 9, 0)])
        );
        let (stamps, notes) = expand_at(&plain, year_end, 1);
        assert!(stamps.is_empty(), "February has no thirty-first");
        assert_eq!(
            notes.iter().map(|note| note.code()).collect::<Vec<_>>(),
            [DiagnosticCode::NonexistentRecurrenceInstance]
        );
        assert_eq!(
            candidates(&plain, year_end, 2),
            expected(&[(1997, 3, 31, 9, 0)])
        );
    }

    /// The year boundary, which is where a week and a calendar year come apart.
    ///
    /// `BYWEEKNO` is `Expand` under `FREQ=YEARLY`, so period Y expands to week one *of year Y* — and week one of 2019 begins on Monday December 31st of 2018, which is a day of the calendar year before the one that numbers it.
    /// Each period therefore contributes exactly one Monday and no period contributes another's: the union is the same either way, which is why reading `BYWEEKNO` as a filter over the calendar year's own days survives every rule with `INTERVAL=1` and no `BYSETPOS`, and is wrong for every rule with either.
    #[test]
    fn a_week_straddling_new_year_belongs_to_the_year_that_numbers_it() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_week_no(ByList::from_slice(&[1_i8]))
            .by_day(ByList::from_slice(&[weekday(None, Weekday::Monday)]))
            .build()
            .unwrap();
        let start = stamp(2018, 1, 1, 9, 0);
        assert_eq!(
            candidates(&rule, start, 0),
            expected(&[(2018, 1, 1, 9, 0)]),
            "week one of 2018 opens on January 1st, and December 31st is week one of 2019"
        );
        assert_eq!(
            candidates(&rule, start, 1),
            expected(&[(2018, 12, 31, 9, 0)]),
            "week one of 2019 begins in December 2018 and belongs to the 2019 period"
        );
        assert_eq!(
            candidates(&rule, start, 2),
            expected(&[(2019, 12, 30, 9, 0)]),
            "and week one of 2020 begins in December 2019"
        );
    }

    /// Week 53 exists only in the years that have one, and its days may fall in the next.
    ///
    /// The other half of the same mechanism.
    /// Under `WKST=SU` the week-numbering year 2014 runs to Saturday January 3rd 2015, so week 53 of 2014 holds days of calendar 2015 — and 2015 itself holds 52 weeks, so `BYWEEKNO=53` names nothing at all in its period even though January 1st 2015 carries the number 53.
    #[test]
    fn week_fifty_three_belongs_to_the_year_that_has_one() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_week_no(ByList::from_slice(&[53_i8]))
            .by_day(ByList::from_slice(&[
                weekday(None, Weekday::Monday),
                weekday(None, Weekday::Thursday),
            ]))
            .wkst(Weekday::Sunday)
            .build()
            .unwrap();
        let start = stamp(2014, 12, 29, 9, 0);
        assert_eq!(
            candidates(&rule, start, 0),
            expected(&[(2014, 12, 29, 9, 0), (2015, 1, 1, 9, 0)]),
            "week 53 of 2014 runs from December 28th into January 3rd"
        );
        assert!(
            candidates(&rule, start, 1).is_empty(),
            "2015 has 52 weeks under WKST=SU, so its period names no week 53"
        );
    }

    /// A period is one `FREQ` unit wide however far apart `INTERVAL` puts two of them.
    ///
    /// The period walk carries an anchor and no upper edge, so the width a period contributes is this unit's answer alone.
    /// `FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,-1` skips February,
    /// and the period after January must offer March's two days and neither of February's — a period two months wide would put February's candidates where `BYSETPOS` would then select the wrong one of them.
    #[test]
    fn an_interval_that_skips_a_period_leaves_the_one_it_lands_on_the_same_width() {
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .interval(NonZeroU32::new(2).unwrap())
            .by_month_day(ByList::from_slice(&[1_i8, -1]))
            .build()
            .unwrap();
        let start = stamp(2026, 1, 15, 9, 0);
        assert_eq!(
            candidates(&rule, start, 0),
            expected(&[(2026, 1, 1, 9, 0), (2026, 1, 31, 9, 0)])
        );
        assert_eq!(
            candidates(&rule, start, 1),
            expected(&[(2026, 3, 1, 9, 0), (2026, 3, 31, 9, 0)]),
            "the period after January is March alone, and February is not folded into it"
        );
    }

    /// A `BYDAY` ordinal under a frequency that forbids one names its weekday and nothing more.
    ///
    /// RFC 5545 section 3.3.10 forbids the numeric form wherever `FREQ` is neither `MONTHLY` nor `YEARLY`, and gives no reading for a file that carries one anyway.
    /// All five of those frequencies answer the same way here — the weekday is kept and the ordinal ignored —
    /// because the alternative was that `FREQ=WEEKLY` resolved the ordinal inside a scope one week wide, where `2TU` matched nothing and silently emptied the entire series while `1TU` worked.
    #[test]
    fn a_forbidden_weekday_ordinal_keeps_its_weekday_under_every_frequency_that_forbids_one() {
        let start = stamp(2026, 8, 3, 9, 0);
        for ordinal in [1_i8, 2, -1, -2] {
            let weekly = RecurrenceRuleBuilder::new(Freq::Weekly)
                .by_day(ByList::from_slice(&[weekday(
                    Some(ordinal),
                    Weekday::Tuesday,
                )]))
                .build()
                .unwrap();
            assert_eq!(
                candidates(&weekly, start, 0),
                expected(&[(2026, 8, 4, 9, 0)]),
                "the Tuesday of the week beginning 2026-08-03, whatever ordinal was written"
            );

            let daily = RecurrenceRuleBuilder::new(Freq::Daily)
                .by_day(ByList::from_slice(&[weekday(
                    Some(ordinal),
                    Weekday::Monday,
                )]))
                .build()
                .unwrap();
            assert_eq!(
                candidates(&daily, start, 0),
                expected(&[(2026, 8, 3, 9, 0)]),
                "and a daily rule answers the same way, which is the point"
            );
        }
    }

    /// `WKST` moves the week a weekly period covers, which is what makes two rules differing only in it produce different days.
    /// Section 3.8.5.3 makes the same point with a pair of occurrence lists.
    #[test]
    fn wkst_decides_which_seven_days_a_weekly_period_holds() {
        let sunday = ByList::from_slice(&[weekday(None, Weekday::Sunday)]);
        let start = stamp(1997, 8, 5, 9, 0);
        let from_monday = RecurrenceRuleBuilder::new(Freq::Weekly)
            .by_day(sunday.clone())
            .wkst(Weekday::Monday)
            .build()
            .unwrap();
        let from_sunday = RecurrenceRuleBuilder::new(Freq::Weekly)
            .by_day(sunday)
            .wkst(Weekday::Sunday)
            .build()
            .unwrap();
        assert_eq!(
            candidates(&from_monday, start, 0),
            expected(&[(1997, 8, 10, 9, 0)]),
            "a week beginning Monday August 4th ends on Sunday the 10th"
        );
        assert_eq!(
            candidates(&from_sunday, start, 0),
            expected(&[(1997, 8, 3, 9, 0)]),
            "a week beginning Sunday August 3rd starts on it"
        );
    }

    /// The clock rows, both directions.
    /// The expanding fixture is section 3.8.5.3's "every 20 minutes from 9:00 AM to 4:40 PM", which is twenty-four readings of one day.
    #[test]
    fn the_clock_rows_expand_a_day_and_limit_an_hour() {
        let start = stamp(1997, 9, 2, 9, 0);
        let workday = RecurrenceRuleBuilder::new(Freq::Daily)
            .by_hour(ByList::from_slice(&[9_u8, 10, 11, 12, 13, 14, 15, 16]))
            .by_minute(ByList::from_slice(&[0_u8, 20, 40]))
            .build()
            .unwrap();
        let readings = candidates(&workday, start, 0);
        assert_eq!(readings.len(), 24);
        assert_eq!(readings.first(), Some(&stamp(1997, 9, 2, 9, 0)));
        assert_eq!(readings.last(), Some(&stamp(1997, 9, 2, 16, 40)));

        let ninth_hour = RecurrenceRuleBuilder::new(Freq::Hourly)
            .by_hour(ByList::from_slice(&[9_u8]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&ninth_hour, start, 0),
            expected(&[(1997, 9, 2, 9, 0)])
        );
        assert!(
            candidates(&ninth_hour, start, 1).is_empty(),
            "the tenth hour is not the ninth, and BYHOUR only limits under HOURLY"
        );
    }

    /// `BYSETPOS` is unit 4's.
    /// This period must hold every Tuesday, Wednesday and Thursday of September 1997 — thirteen of them — and not the third one alone, which is what section 3.8.5.3's "third instance into the month" rule finally emits.
    #[test]
    fn by_set_pos_is_not_applied_here() {
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_day(ByList::from_slice(&[
                weekday(None, Weekday::Tuesday),
                weekday(None, Weekday::Wednesday),
                weekday(None, Weekday::Thursday),
            ]))
            .by_set_pos(ByList::from_slice(&[3_i16]))
            .build()
            .unwrap();
        let stamps = candidates(&rule, stamp(1997, 9, 4, 9, 0), 0);
        assert_eq!(stamps.len(), 13);
        assert_eq!(
            stamps.get(2),
            Some(&stamp(1997, 9, 4, 9, 0)),
            "the third candidate is what BYSETPOS=3 will select, once unit 4 selects it"
        );
    }

    /// `BYMONTHDAY` under `YEARLY` expands over the whole year when no `BYMONTH` pins a month,
    /// which is what "Expand" means in a cell whose period is a year.
    /// With a `BYMONTH` beside it the month is pinned and one day survives.
    #[test]
    fn by_month_day_under_yearly_ranges_over_the_year_until_by_month_pins_it() {
        let start = stamp(1997, 9, 2, 9, 0);
        let every_month = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month_day(ByList::from_slice(&[15_i8]))
            .build()
            .unwrap();
        let stamps = candidates(&every_month, start, 0);
        assert_eq!(stamps.len(), 12);
        assert_eq!(stamps.first(), Some(&stamp(1997, 1, 15, 9, 0)));

        let pinned = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month(ByList::from_slice(&[6_u8]))
            .by_month_day(ByList::from_slice(&[15_i8]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&pinned, start, 0),
            expected(&[(1997, 6, 15, 9, 0)])
        );
    }

    /// Everything the rule leaves unstated comes from `DTSTART`, at whatever granularity the period and the expanding span parts left free.
    #[test]
    fn every_unstated_field_comes_from_dtstart() {
        let start = stamp(1997, 5, 12, 14, 30);
        let yearly = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
        assert_eq!(
            candidates(&yearly, start, 1),
            expected(&[(1998, 5, 12, 14, 30)]),
            "no BYMONTH means DTSTART's month and no BYHOUR means DTSTART's hour"
        );

        let by_week = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_week_no(ByList::from_slice(&[20_i8]))
            .build()
            .unwrap();
        assert_eq!(
            candidates(&by_week, start, 0),
            expected(&[(1997, 5, 12, 14, 30)]),
            "a week-sized span leaves the weekday free, and DTSTART fills it"
        );
    }

    /// A budget that binds is reported rather than exceeded, in both of its dimensions.
    #[test]
    fn a_period_that_costs_more_than_the_budget_is_refused_and_not_truncated() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
        let start = stamp(1997, 9, 2, 9, 0);

        let mut kept: Vec<Diagnostic> = Vec::new();
        let tight = Limits::DEFAULT.with_candidates_per_period(4);
        let mut counted = Meter::new(tight);
        let period = PeriodWalk::new(start, &rule).next().unwrap();
        counted.open_period();
        assert_eq!(
            expand_period(period, &rule, start, &mut counted, &mut kept),
            Err(super::ExpansionError::Resource(LimitExceeded::Candidates))
        );

        let mut spent = Meter::with_budget(Limits::DEFAULT, 8);
        let same = PeriodWalk::new(start, &rule).next().unwrap();
        spent.open_period();
        assert_eq!(
            expand_period(same, &rule, start, &mut spent, &mut kept),
            Err(super::ExpansionError::Resource(LimitExceeded::Budget))
        );
        assert!(spent.is_exhausted(), "the shared ledger latches");
    }

    /// The set is a set: two parts naming one instant produce it once, and the order is chronological because that is the order `BYSETPOS` counts in.
    #[test]
    fn the_candidate_set_is_ascending_and_holds_no_repeat() {
        let rule = RecurrenceRuleBuilder::new(Freq::Daily)
            .by_hour(ByList::from_slice(&[17_u8, 9, 9]))
            .build()
            .unwrap();
        let stamps = candidates(&rule, stamp(1997, 9, 2, 9, 0), 0);
        assert_eq!(stamps, expected(&[(1997, 9, 2, 9, 0), (1997, 9, 2, 17, 0)]));
    }

    /// An empty set is a value rather than an absence, and `CandidateSet` says which it is without a caller reaching for the slice.
    #[test]
    fn an_empty_candidate_set_reports_itself() {
        let empty = CandidateSet::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.as_slice(), []);
    }

    /// The arithmetic the negative forms all share, at both ends and at the value RFC 5545 section 3.3.10 excludes from every one of them.
    #[test]
    fn a_negative_position_counts_back_from_the_end_and_zero_names_nothing() {
        let cases: [(i32, u32, Option<u32>); 8] = [
            (1, 31, Some(1)),
            (31, 31, Some(31)),
            (32, 31, None),
            (-1, 31, Some(31)),
            (-31, 31, Some(1)),
            (-32, 31, None),
            (0, 31, None),
            (-1, 0, None),
        ];
        for (value, length, wanted) in cases {
            assert_eq!(position_named(value, length), wanted, "{value} of {length}");
        }
    }

    /// Week numbering, against dates whose ISO week is known independently, and under a `WKST` other than the Monday ISO 8601 fixes.
    #[test]
    fn week_numbers_follow_the_four_day_rule_from_wkst() {
        let cases: [(u16, u8, u8, u16, u8); 5] = [
            (1997, 5, 12, 1997, 20),
            (1998, 5, 11, 1998, 20),
            (2018, 12, 31, 2019, 1),
            (2019, 12, 30, 2020, 1),
            (2021, 1, 1, 2020, 53),
        ];
        for (year, month, day, holder, number) in cases {
            let date = CivilDate::from_ymd(year, month, day).unwrap();
            assert_eq!(
                week_of(date, Weekday::Monday),
                Some((holder, number)),
                "{year}-{month}-{day}"
            );
        }

        let new_year = CivilDate::from_ymd(2021, 1, 1).unwrap();
        assert_eq!(
            week_of(new_year, Weekday::Sunday),
            Some((2020, 53)),
            "a week beginning Sunday still belongs to the year holding four of its days"
        );
    }

    /// The offset a `WKST` puts a weekday at, over every pair.
    #[test]
    fn a_weekday_sits_at_its_distance_from_the_start_of_the_week() {
        for start in Weekday::ALL {
            let mut seen: Vec<u8> = Weekday::ALL
                .into_iter()
                .map(|day| week_offset(day, start))
                .collect();
            assert_eq!(week_offset(start, start), 0);
            seen.sort_unstable();
            assert_eq!(seen, [0, 1, 2, 3, 4, 5, 6], "{start:?}");
        }
    }
}
