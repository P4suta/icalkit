// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 3 — applying the eight `BYxxx` parts, driven by the expand/limit table.
//!
//! # What this unit owns
//!
//! Turning one period into that period's candidate set, by walking [`RulePart::ALL`] in the
//! RFC's own row order and asking [`crate::table::effect`] what each part does. A part that
//! expands multiplies the working set; a part that limits filters it; a part that is not
//! applicable is skipped. `BYSETPOS` is *not* applied here — it is unit 4's, and applying it in
//! this pass is the single most common way this gets built wrong.
//!
//! # How the table drives it
//!
//! The engine is a selection over the days the period holds and the clock readings it admits,
//! and [`crate::table::effect`] decides four things about it, none of which is written down
//! twice:
//!
//! 1. **[`PartEffect::NotApplicable`] drops the part entirely.** `BYMONTHDAY` under
//!    `FREQ=WEEKLY` is `N/A` in the printed table, so a weekly rule carrying one recurs as
//!    though it did not. Implementations that filter on it anyway disagree with the RFC here.
//! 2. **[`PartEffect::expands`] decides whether a part may move a candidate off `DTSTART`'s
//!    day.** RFC 5545 section 3.3.10 takes every field the rule leaves unstated from `DTSTART`,
//!    and a *limiting* part only removes days that default gave; an *expanding* part replaces
//!    the default outright. This is the whole of the difference the milestone brief names:
//!    `FREQ=MONTHLY;BYMONTHDAY=2,15` expands to two days in a month whose `DTSTART` names
//!    neither, and `FREQ=DAILY;BYMONTHDAY=2,15` keeps the one day the period already fixed
//!    only when it is the 2nd or the 15th. Read the `MONTHLY` cell as `Limit` and the first
//!    rule yields nothing at all.
//! 3. **[`PartEffect::ExpandWeekdays`] carries the scope a `BYDAY` ordinal counts within.**
//!    `-1MO` is the last Monday *of something*, and Note 1 and Note 2 decide what — which is
//!    why the table resolves the notes rather than this file.
//! 4. **[`PartEffect::expands`] also decides whether a missing day is reported.** Only a part
//!    that expands *generates* an instance, so only an expanding part can generate one whose
//!    date does not exist; a limiting part that matches nothing has removed a day rather than
//!    invented one.
//!
//! Two classifications appear here that are *not* the table and must not be mistaken for it.
//! [`Span`] says how much of the calendar one period pins before any part is read, which is
//! the period walk's subject rather than section 3.3.10's. `DATE_PARTS` and `TIME_PARTS` say
//! which coordinate a part addresses, which is a fact about the part's name. Neither says
//! anything about expanding or limiting, and neither is consulted for that.
//!
//! # What this unit must not do
//!
//! - It must not hard-code any cell of the table. Every branch reads [`crate::table::effect`].
//! - It must not be one `match`. This crate's Clippy profile bounds a function at 100 lines
//!   and a cognitive complexity of 15; a `BYxxx` application written as one match fails both,
//!   which is the gate asking for the table-driven shape rather than obstructing it.
//! - It must not clamp an invalid date. `FREQ=MONTHLY;BYMONTHDAY=31` has no February instance
//!   and section 3.3.10 says such an instance MUST be ignored. The skip is reported with
//!   [`DiagnosticCode::NonexistentRecurrenceInstance`], and the skipped candidate is still
//!   charged, because it was still generated (`docs/adr/0011`).
//! - It must not emit anything. Candidates leave this unit as a period-local set.
//!
//! # What it charges, and why the charge is where it is
//!
//! Every date the period contributes is charged as a candidate as it is enumerated, before
//! any part has had a chance to reject it, and so is every clock reading a time part produces
//! and every date-and-time pair that survives. That is deliberately more than the count of
//! candidates that come out: `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` yields nothing in any
//! period, and a budget charged for what a period *produced* would let it walk the calendar
//! forever without ever firing. Charging what the period *cost* bounds the walk in exactly the
//! cases the milestone brief names as hostile — a rule that matches rarely, and one that
//! matches never.
//!
//! The period is not opened here. `Meter::open_period` clears the per-period ceiling, and unit
//! 7 owns where that happens so that the charges it makes on either side of an expansion land
//! in the same period as this one's. A caller expanding a period on its own opens it itself.

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::num::NonZeroI8;

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, DiagnosticSink, LimitExceeded,
    Location, Meter, Severity, UtcOffset, Weekday, report_diagnostic,
};

use crate::period::Period;
use crate::rule::{Freq, RecurrenceRule, RulePart, WeekdayNum};
use crate::table::{PartEffect, PartsPresent, WeekdayScope, effect};

/// The rule parts that name a coordinate of the date, in the table's row order.
///
/// A classification by what the part addresses, not by what it does: every one of these is
/// `Expand` under some frequency and `Limit` under another, and which it is here is
/// [`crate::table::effect`]'s answer and never this list's.
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
/// Ascending because RFC 5545 section 3.3.10 counts `BYSETPOS` positions in chronological
/// order rather than in the order the file wrote the parts, and unit 4 must not have to
/// re-derive that. Deduplicated because the recurrence set is a set: `BYWEEKNO` expands to a
/// week and `BYDAY` then names one day of it, and the two arrive at that day once per day of
/// the week they started from.
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
    /// Not the same statement as "the rule matched nothing": a period can cost a large budget
    /// and still be empty, which is what `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` does forever.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }

    /// A set holding exactly `stamps`, for a test that needs one it did not expand.
    ///
    /// `#[cfg(test)]` rather than a constructor on the public surface. The ascending-and-
    /// deduplicated invariant is what [`crate::select`] reads positions against, and the only
    /// thing entitled to establish it outside a test is [`expand_period`], which builds it by
    /// sorting. Offering a door that takes any slice would let a caller state a set that is not
    /// one and get a `BYSETPOS=-1` answer that names the wrong instant.
    #[cfg(test)]
    pub(crate) fn from_ascending(stamps: &[CivilDateTime]) -> Self {
        let mut stamps = stamps.to_vec();
        stamps.sort_unstable();
        stamps.dedup();
        Self { stamps }
    }
}

/// One period's candidate set, with every `BYxxx` part but `BYSETPOS` applied.
///
/// `dtstart` is an argument because RFC 5545 section 3.3.10 takes every field the rule leaves
/// unstated from it: a `FREQ=YEARLY` rule with no `BYMONTH` recurs in `DTSTART`'s month, and a
/// rule with no `BYHOUR` recurs at `DTSTART`'s hour. `period` is read for its anchor alone —
/// the anchor names the year, month, week or day the period covers, and every extent this unit
/// needs follows from that plus the frequency, so nothing here depends on where unit 2 chose to
/// put a period's upper edge.
pub fn expand_period<S: DiagnosticSink + ?Sized>(
    period: Period,
    rule: &RecurrenceRule,
    dtstart: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<CandidateSet, LimitExceeded> {
    let anchor = period.anchor();
    let present = rule.parts_present();
    let mut days = period_days(rule, present, anchor.date(), meter)?;
    apply_date_parts(&mut days, rule, present, anchor, meter, sink)?;
    if !has_expanding_day_part(rule, present) {
        // Nothing expanded at the granularity of a day, so section 3.3.10's fallback stands
        // and the day is the one `DTSTART` names within whatever the period and the expanding
        // span parts left free.
        let span = effective_span(rule, present);
        apply_default_day(&mut days, span, dtstart.date(), anchor, meter, sink)?;
    }
    let clocks = period_times(rule, present, anchor.time(), dtstart.time(), meter)?;
    assemble(&days, &clocks, meter)
}

/// How much of the calendar is fixed before any `BYxxx` part is read.
///
/// This is the period walk's subject rather than the expand/limit table's, and it is here
/// because section 3.3.10's fallback to `DTSTART` needs to know which field is still free: a
/// monthly period leaves the day of the month open, a weekly one leaves the weekday open, and
/// a yearly one leaves both the month and the day open.
///
/// Unrelated to `ical_core::Span`, which is a range of octets in a file. This one is private to
/// this module and never crosses it, so the two cannot meet; naming both after the same English
/// word is the cost of that word being right for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    /// A whole calendar year.
    Year,
    /// One calendar month.
    Month,
    /// Seven days beginning on `WKST`.
    Week,
    /// One day, which is what every frequency below `DAILY` also spans.
    Day,
}

impl Span {
    /// How narrow this span is, larger being narrower.
    ///
    /// A week is narrower than a month, which is why `BYWEEKNO` beats `BYMONTH` in Note 2 and
    /// why the same ordering settles it here without a second statement of that rule.
    const fn depth(self) -> u8 {
        match self {
            Self::Year => 0,
            Self::Month => 1,
            Self::Week => 2,
            Self::Day => 3,
        }
    }

    /// The span one period of `freq` covers.
    const fn of(freq: Freq) -> Self {
        match freq {
            Freq::Yearly => Self::Year,
            Freq::Monthly => Self::Month,
            Freq::Weekly => Self::Week,
            // A period below a day is still inside one day, and the day is what a date-level
            // fallback would have to choose from.
            Freq::Daily | Freq::Hourly | Freq::Minutely | Freq::Secondly => Self::Day,
        }
    }

    /// The narrower of the two.
    const fn narrowed_by(self, other: Self) -> Self {
        if other.depth() > self.depth() {
            other
        } else {
            self
        }
    }
}

/// The span left once every expanding span-naming part has narrowed the period's own.
///
/// `BYMONTH` and `BYWEEKNO` name a stretch of the calendar rather than a day, so an expanding
/// one narrows what `DTSTART` still has to fill in: `FREQ=YEARLY;BYMONTH=6,7` recurs on
/// `DTSTART`'s day of June and of July, not on `DTSTART`'s month and day. A *limiting*
/// `BYMONTH` narrows nothing, because a limit removes candidates rather than choosing where
/// they sit, and that difference is read from the table rather than from the frequency.
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

/// Whether any part that names a day expands, which is what suppresses the `DTSTART` fallback.
fn has_expanding_day_part(rule: &RecurrenceRule, present: PartsPresent) -> bool {
    [RulePart::YearDay, RulePart::MonthDay, RulePart::Day]
        .into_iter()
        .any(|part| rule.has_part(part) && effect(rule.freq(), part, present).expands())
}

/// Every date the period covers, charged as it is enumerated.
///
/// At most 371 dates, and charged one candidate each. The enumeration is deliberately not
/// narrowed by what the rule is about to ask for: the charge is what bounds the work, so work
/// that is done has to be charged even when the rule rejects all of it.
fn period_days(
    rule: &RecurrenceRule,
    present: PartsPresent,
    anchor: CivilDate,
    meter: &mut Meter,
) -> Result<Vec<CivilDate>, LimitExceeded> {
    let (start, length) = day_extent(rule, present, anchor);
    let mut days = Vec::new();
    for offset in 0..length {
        // A period at the very end of the years RFC 5545 section 3.3.4 can write runs off the
        // end of the calendar; the days past it are not dates and are not charged for.
        let Some(date) = start.checked_add_days(i64::from(offset)) else {
            break;
        };
        meter.try_charge_candidate()?;
        days.push(date);
    }
    Ok(days)
}

/// Where the days this period contributes begin, and how many there are.
///
/// The frequency's own span, except under an expanding `BYWEEKNO` — which the table prints for
/// `FREQ=YEARLY` and for nothing else. There, section 3.3.10 expands the period to *the weeks
/// of that year*, and a week-numbering year is not a calendar year: week one of 2020 begins on
/// Monday December 30th 2019, and week 53 of 2014 runs to January 3rd 2015. Enumerating the
/// calendar year instead attributes each of those days to the neighboring period, which is
/// invisible at `INTERVAL=1` — the two readings partition the same union — and wrong the moment
/// a period is skipped or a `BYSETPOS` selects within one.
///
/// Week-numbering years tile the timeline exactly as calendar years do, each beginning where
/// the last ended, so two periods still never offer the same day and candidates still ascend
/// across periods.
fn day_extent(rule: &RecurrenceRule, present: PartsPresent, anchor: CivilDate) -> (CivilDate, u16) {
    let expands_weeks =
        rule.has_part(RulePart::WeekNo) && effect(rule.freq(), RulePart::WeekNo, present).expands();
    let named = expands_weeks
        .then(|| week_year_extent(anchor.year(), rule.wkst()))
        .flatten();
    named.unwrap_or_else(|| period_extent(Span::of(rule.freq()), rule.wkst(), anchor))
}

/// Where week-numbering year `year` begins, and how many days it holds.
///
/// `None` at the edges of the calendar, where the week before or the week after is not a date
/// RFC 5545 section 3.3.4 can write; the calendar year is used there, which is the same set of
/// days give or take the three at each end that no year can number.
fn week_year_extent(year: u16, wkst: Weekday) -> Option<(CivilDate, u16)> {
    let start = week_one_start(year, wkst)?;
    let weeks = weeks_in_week_year(year, wkst)?;
    let length = u16::from(weeks).checked_mul(DAYS_PER_WEEK_U16)?;
    Some((start, length))
}

/// The first day of week one of `year`, counting weeks from `wkst`.
///
/// A week belongs to the year holding four or more of its days, so week one is the week holding
/// January 4th: whichever weekday that date falls on, the week containing it has at least four
/// days in the year and the week before it has at most three.
fn week_one_start(year: u16, wkst: Weekday) -> Option<CivilDate> {
    let fourth = CivilDate::from_ymd(year, 1, 4)?;
    let offset = i64::from(week_offset(fourth.weekday()?, wkst));
    fourth.checked_add_days(offset.checked_neg()?)
}

/// Where the period holding `anchor` starts, and how many days it holds.
fn period_extent(span: Span, wkst: Weekday, anchor: CivilDate) -> (CivilDate, u16) {
    match span {
        Span::Year => {
            let start = CivilDate::from_ymd(anchor.year(), 1, 1).unwrap_or(anchor);
            (start, days_in_year(anchor.year()))
        },
        Span::Month => {
            let length = CivilDate::days_in_month(anchor.year(), anchor.month()).unwrap_or(1);
            let start = CivilDate::from_ymd(anchor.year(), anchor.month(), 1).unwrap_or(anchor);
            (start, u16::from(length))
        },
        Span::Week => {
            // A week begins on `WKST` whatever weekday the anchor fell on, which is the whole
            // of section 3.3.10's `WKST` rule part and the reason two rules differing only in
            // it produce different days.
            let back = anchor
                .weekday()
                .map_or(0, |weekday| week_offset(weekday, wkst));
            let start = i64::from(back)
                .checked_neg()
                .and_then(|shift| anchor.checked_add_days(shift))
                .unwrap_or(anchor);
            (start, 7)
        },
        Span::Day => (anchor, 1),
    }
}

/// Apply every date part the rule carries, in the table's row order.
fn apply_date_parts<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    rule: &RecurrenceRule,
    present: PartsPresent,
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    let scope = weekday_scope(rule.freq(), present);
    for part in RulePart::ALL {
        if !DATE_PARTS.contains(&part) || !rule.has_part(part) {
            continue;
        }
        let action = effect(rule.freq(), part, present);
        if action == PartEffect::NotApplicable {
            continue;
        }
        if action.expands() {
            // Only a part that expands generates an instance, so only a part that expands can
            // generate one whose date does not exist.
            report_absent_days(days, part, rule, anchor, meter, sink)?;
        }
        days.retain(|date| date_part_matches(part, *date, rule, scope));
    }
    Ok(())
}

/// The scope a `BYDAY` ordinal counts within for this rule, or `None` when it counts in none.
///
/// The expanding cells carry their scope, because that is what Note 1 and Note 2 resolve to.
/// The limiting ones do not: the RFC prints `Limit` for `BYDAY` exactly when a day-naming part
/// is already present, and says nothing about what an ordinal would then mean. Rather than
/// invent an answer, this asks the same two notes what they would have said had those parts
/// been absent — which is the scope the RFC itself derives from the remaining parts.
///
/// Section 3.3.10 forbids an ordinal outright wherever `FREQ` is neither `MONTHLY` nor
/// `YEARLY`, and one answer covers all five of those frequencies: none. An ordinal in a rule
/// that may not carry one is ignored and its weekday kept, which is all such an entry can be
/// read to have meant. The alternative was worse in exactly one cell — `FREQ=WEEKLY` prints
/// `Expand` rather than `Limit`, so a scope one week wide came back from the table and resolved
/// `BYDAY=2TU` against a run of one, silently emptying the whole series including `DTSTART`
/// while `BYDAY=1TU` worked. Same forbidden construct, two answers, and the quiet one lost
/// everything; the decoder reports the construct on
/// [`DiagnosticCode::RecurrenceRulePartOutOfRange`] so it is not merely tolerated.
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

/// What the two notes would give for `BYDAY` if no `BYYEARDAY` and no `BYMONTHDAY` were there.
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

/// Whether `date` is one of the days `part` names.
fn date_part_matches(
    part: RulePart,
    date: CivilDate,
    rule: &RecurrenceRule,
    scope: Option<WeekdayScope>,
) -> bool {
    match part {
        RulePart::Month => rule.by_month().as_slice().contains(&date.month()),
        RulePart::WeekNo => week_no_matches(date, rule.by_week_no().as_slice(), rule.wkst()),
        RulePart::YearDay => year_day_matches(date, rule.by_year_day().as_slice()),
        RulePart::MonthDay => month_day_matches(date, rule.by_month_day().as_slice()),
        RulePart::Day => weekday_matches(date, rule.by_day().as_slice(), scope, rule.wkst()),
        // The three clock rows and `BYSETPOS` say nothing about a date. The walk never offers
        // them here, and answering "this date is not excluded" is the only honest reply if it
        // ever did.
        RulePart::Hour | RulePart::Minute | RulePart::Second | RulePart::SetPos => true,
    }
}

/// Whether `date`'s day of the month is one of the days `BYMONTHDAY` names.
fn month_day_matches(date: CivilDate, values: &[i8]) -> bool {
    let Some(length) = CivilDate::days_in_month(date.year(), date.month()) else {
        return false;
    };
    let wanted = u32::from(date.day());
    values
        .iter()
        .any(|listed| position_named(i32::from(*listed), u32::from(length)) == Some(wanted))
}

/// Whether `date`'s day of the year is one of the days `BYYEARDAY` names.
fn year_day_matches(date: CivilDate, values: &[i16]) -> bool {
    let Some(wanted) = day_of_year(date).map(u32::from) else {
        return false;
    };
    let length = u32::from(days_in_year(date.year()));
    values
        .iter()
        .any(|listed| position_named(i32::from(*listed), length) == Some(wanted))
}

/// Whether the week holding `date` is one of the weeks `BYWEEKNO` names.
///
/// The comparison is against the number of the week `date` belongs to, and it needs no second
/// question about which year numbers that week: [`day_extent`] already offered this period the
/// days of *its own* week-numbering year, so every date reaching here is numbered by the period
/// it came from. A negative value counts back through that year's own 52 or 53 weeks, which is
/// why the length is asked of the week year and never of the calendar one.
fn week_no_matches(date: CivilDate, values: &[i8], wkst: Weekday) -> bool {
    let Some((week_year, number)) = week_of(date, wkst) else {
        return false;
    };
    let Some(length) = weeks_in_week_year(week_year, wkst) else {
        return false;
    };
    let wanted = u32::from(number);
    values
        .iter()
        .any(|listed| position_named(i32::from(*listed), u32::from(length)) == Some(wanted))
}

/// Whether `date` is one of the days `BYDAY` names.
fn weekday_matches(
    date: CivilDate,
    values: &[WeekdayNum],
    scope: Option<WeekdayScope>,
    wkst: Weekday,
) -> bool {
    let Some(weekday) = date.weekday() else {
        return false;
    };
    values.iter().any(|listed| {
        listed.weekday() == weekday && ordinal_matches(date, listed.ordinal(), scope, wkst)
    })
}

/// Whether `date` is the occurrence of its own weekday that `ordinal` names within `scope`.
fn ordinal_matches(
    date: CivilDate,
    ordinal: Option<NonZeroI8>,
    scope: Option<WeekdayScope>,
    wkst: Weekday,
) -> bool {
    let Some(ordinal) = ordinal else {
        return true;
    };
    let Some(scope) = scope else {
        // RFC 5545 section 3.3.10 forbids an ordinal wherever the table prints a plain
        // `Limit`, and a file carrying one anyway is kept rather than refused: the entry names
        // its weekday, which is all the rule can be read to have meant.
        return true;
    };
    let Some((index, length)) = weekday_position(date, scope, wkst) else {
        return false;
    };
    position_named(i32::from(ordinal.get()), length) == Some(index)
}

/// Which occurrence of its own weekday `date` is within `scope`, and how many that scope holds.
fn weekday_position(date: CivilDate, scope: WeekdayScope, wkst: Weekday) -> Option<(u32, u32)> {
    let (offset, length) = match scope {
        WeekdayScope::Week => (u32::from(week_offset(date.weekday()?, wkst)), DAYS_PER_WEEK),
        WeekdayScope::Month => (
            u32::from(date.day()).checked_sub(1)?,
            u32::from(CivilDate::days_in_month(date.year(), date.month())?),
        ),
        WeekdayScope::Year => (
            u32::from(day_of_year(date)?).checked_sub(1)?,
            u32::from(days_in_year(date.year())),
        ),
    };
    let index = offset.div_euclid(DAYS_PER_WEEK).checked_add(1)?;
    // Every occurrence of one weekday sits a whole number of weeks from the first, so the
    // first is at the remainder and the count is how many strides of seven fit above it.
    let first = offset.rem_euclid(DAYS_PER_WEEK);
    let count = length
        .checked_sub(first)?
        .checked_sub(1)?
        .div_euclid(DAYS_PER_WEEK)
        .checked_add(1)?;
    Some((index, count))
}

/// The position `value` names in a run of `length` positions, counting back from the end when
/// it is negative, or `None` when the run has no such position.
///
/// The one place this crate turns a `BYxxx` value into a place in a calendar, so that the
/// negative form — `BYMONTHDAY=-1`, `BYDAY=-2MO`, `BYWEEKNO=-1` — is written once. Zero names
/// nothing: RFC 5545 section 3.3.10 excludes it from every one of those productions.
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

/// How far `weekday` sits into a week beginning on `wkst`.
fn week_offset(weekday: Weekday, wkst: Weekday) -> u8 {
    let reached = i16::from(weekday.index());
    let begins = i16::from(wkst.index());
    // Both indices are under seven, so the difference is inside `i16` and the Euclidean
    // remainder puts a weekday before the week's start at the end of it rather than below zero.
    let offset = reached.saturating_sub(begins).rem_euclid(7);
    u8::try_from(offset).unwrap_or(0)
}

/// The calendar year numbering the week that holds `date`, and that week's number.
///
/// RFC 5545 section 3.3.10 numbers weeks as ISO 8601 does but from `WKST` rather than always
/// from Monday: a week belongs to the year holding at least four of its days, which is the
/// year its fourth day falls in, and week one is the first such week.
fn week_of(date: CivilDate, wkst: Weekday) -> Option<(u16, u8)> {
    let offset = i64::from(week_offset(date.weekday()?, wkst));
    let pivot = date.checked_add_days(3_i64.checked_sub(offset)?)?;
    let reached = day_of_year(pivot)?;
    let number = u32::from(reached)
        .checked_sub(1)?
        .div_euclid(DAYS_PER_WEEK)
        .checked_add(1)?;
    Some((pivot.year(), u8::try_from(number).ok()?))
}

/// How many weeks the week-numbering year `year` holds, which is 52 or 53.
fn weeks_in_week_year(year: u16, wkst: Weekday) -> Option<u8> {
    let last = CivilDate::from_ymd(year, 12, 31)?;
    let (holder, number) = week_of(last, wkst)?;
    if holder == year {
        return Some(number);
    }
    // December 31st already belongs to week one of the following year, so the week before it
    // is this year's last.
    let earlier = last.checked_add_days(-7)?;
    let (_, previous) = week_of(earlier, wkst)?;
    Some(previous)
}

/// Which day of the year `date` is, counting January 1st as one.
fn day_of_year(date: CivilDate) -> Option<u16> {
    let start = CivilDate::from_ymd(date.year(), 1, 1)?;
    let elapsed = date
        .days_from_epoch()?
        .checked_sub(start.days_from_epoch()?)?;
    u16::try_from(elapsed.checked_add(1)?).ok()
}

/// How many days `year` holds.
const fn days_in_year(year: u16) -> u16 {
    if CivilDate::is_leap_year(year) {
        366
    } else {
        365
    }
}

/// Report every day an expanding part named that the calendar does not have.
///
/// A `BYMONTHDAY` of 31 in a 30-day month and a `BYYEARDAY` of 366 in a common year are the
/// two shapes of that. RFC 5545 section 3.3.10 says such an instance MUST be ignored, and
/// `docs/adr/0011` says it is nonetheless charged, because generating it is what discovered it
/// was not there.
fn report_absent_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    part: RulePart,
    rule: &RecurrenceRule,
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    match part {
        RulePart::MonthDay => {
            report_absent_month_days(days, rule.by_month_day().as_slice(), anchor, meter, sink)
        },
        RulePart::YearDay => {
            report_absent_year_days(days, rule.by_year_day().as_slice(), anchor, meter, sink)
        },
        // No other part can name a day the calendar lacks. A month, a week number and a
        // weekday each name something every period either has or does not reach, and neither
        // is a date that failed to exist.
        RulePart::Month
        | RulePart::WeekNo
        | RulePart::Day
        | RulePart::Hour
        | RulePart::Minute
        | RulePart::Second
        | RulePart::SetPos => Ok(()),
    }
}

/// Report each `BYMONTHDAY` value that names no day of a month the working set still holds.
fn report_absent_month_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    values: &[i8],
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    for (year, month) in distinct_months(days) {
        let Some(length) = CivilDate::days_in_month(year, month) else {
            continue;
        };
        for listed in values {
            if position_named(i32::from(*listed), u32::from(length)).is_none() {
                report_absent(anchor, meter, sink)?;
            }
        }
    }
    Ok(())
}

/// Report each `BYYEARDAY` value that names no day of a year the working set still holds.
fn report_absent_year_days<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    values: &[i16],
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    for year in distinct_years(days) {
        let length = u32::from(days_in_year(year));
        for listed in values {
            if position_named(i32::from(*listed), length).is_none() {
                report_absent(anchor, meter, sink)?;
            }
        }
    }
    Ok(())
}

/// Charge one generated candidate and say that its date does not exist.
///
/// The instant the diagnostic carries is the *period's anchor* and never the missing instance:
/// the whole claim is that the instance has no date, so it has no instant either, and naming
/// one would invent exactly the nearby answer section 3.3.10 forbids. The anchor is what a
/// caller can act on, because it says which period lost a candidate, and it is read at UTC
/// because this layer has no zone and `docs/adr/0003` makes resolving one the caller's step. A
/// period at the very edge of the timeline has no instant at all, and there the diagnostic
/// carries none rather than a saturated one.
///
/// Reporting comes before charging so that the last candidate a budget admits is still
/// explained rather than swallowed by its own exhaustion.
fn report_absent<S: DiagnosticSink + ?Sized>(
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    let code = DiagnosticCode::NonexistentRecurrenceInstance;
    let note = match anchor.at_offset(UtcOffset::UTC) {
        Some(instant) => Diagnostic::at_instant(code, Severity::Note, instant),
        None => Diagnostic::new(code, Severity::Note, Location::NOWHERE),
    };
    report_diagnostic(sink, meter, note);
    meter.try_charge_candidate()
}

/// Each distinct month the working set still holds, in ascending order.
fn distinct_months(days: &[CivilDate]) -> Vec<(u16, u8)> {
    let mut covered: Vec<(u16, u8)> = Vec::new();
    for date in days {
        let reached = (date.year(), date.month());
        // The working set is ascending, so one month's days are contiguous and the last entry
        // is the only one a repeat can equal.
        if covered.last() != Some(&reached) {
            covered.push(reached);
        }
    }
    covered
}

/// Each distinct year the working set still holds, in ascending order.
fn distinct_years(days: &[CivilDate]) -> Vec<u16> {
    let mut covered: Vec<u16> = Vec::new();
    for date in days {
        if covered.last() != Some(&date.year()) {
            covered.push(date.year());
        }
    }
    covered
}

/// Keep only the day of each span that `DTSTART` names, reporting the spans that lack it.
fn apply_default_day<S: DiagnosticSink + ?Sized>(
    days: &mut Vec<CivilDate>,
    span: Span,
    start: CivilDate,
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    report_absent_default(days, span, start, anchor, meter, sink)?;
    days.retain(|date| default_day_matches(span, *date, start));
    Ok(())
}

/// Report each span whose free coordinate `DTSTART` fills with a day the span does not have.
fn report_absent_default<S: DiagnosticSink + ?Sized>(
    days: &[CivilDate],
    span: Span,
    start: CivilDate,
    anchor: CivilDateTime,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<(), LimitExceeded> {
    match span {
        // A month leaves the day of the month free, so a `DTSTART` on the 31st names nothing
        // in a 30-day month and a `FREQ=MONTHLY` rule skips that month rather than moving.
        Span::Month => {
            // A day of the month is at most 31 and so always fits, but the conversion is the
            // one that says so rather than a comment claiming it.
            let Ok(named) = i8::try_from(start.day()) else {
                return Ok(());
            };
            report_absent_month_days(days, &[named], anchor, meter, sink)
        },
        // A year leaves both the month and the day free, which is how a `DTSTART` of February
        // 29th names nothing at all in a common year.
        Span::Year => {
            for year in distinct_years(days) {
                if CivilDate::from_ymd(year, start.month(), start.day()).is_none() {
                    report_absent(anchor, meter, sink)?;
                }
            }
            Ok(())
        },
        // A week leaves the weekday free and holds all seven; a day leaves nothing free.
        Span::Week | Span::Day => Ok(()),
    }
}

/// Whether `date` is the day `DTSTART` names within a span of this size.
fn default_day_matches(span: Span, date: CivilDate, start: CivilDate) -> bool {
    match span {
        Span::Year => date.month() == start.month() && date.day() == start.day(),
        Span::Month => date.day() == start.day(),
        Span::Week => date.weekday() == start.weekday(),
        Span::Day => true,
    }
}

/// Every clock reading the period admits, with the three time parts applied in row order.
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
/// RFC 5545 section 3.3.10 takes every unstated field from `DTSTART`, except the fields the
/// period itself already fixed: an `HOURLY` period names its own hour and only the minute and
/// the second below it fall back.
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
            // A value no clock can hold is dropped rather than folded onto a nearby one; the
            // decoder already reported it out of range and kept the rest of the rule.
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
    // Chronological because `BYSETPOS` counts positions in that order and unit 4 must not have
    // to re-derive it; deduplicated because the recurrence set is a set and two parts can name
    // one instant.
    stamps.sort_unstable();
    stamps.dedup();
    Ok(CandidateSet { stamps })
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::{NonZeroI8, NonZeroU32};

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, LimitExceeded, Limits,
        Location, Meter, Severity, Weekday,
    };

    use super::{CandidateSet, expand_period, position_named, week_of, week_offset};
    use crate::period::PeriodWalk;
    use crate::rule::{ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RulePart, WeekdayNum};
    use crate::table::{PartEffect, PartsPresent, WeekdayScope, effect};

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
    /// The period comes from unit 2's walk rather than from a fixture, because that is how the
    /// engine composes the two and a period this file built itself would prove nothing about
    /// the pair.
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
    /// The monthly rule names two days of a month whose `DTSTART` names neither, which is the
    /// fixture that tells expansion from limitation: read the `MONTHLY` cell as `Limit` and
    /// the answer is empty rather than two.
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

    /// The `N/A` cells are not filters. A weekly rule carrying `BYMONTHDAY` recurs as though
    /// it did not, which is the answer the printed table gives and several implementations do
    /// not.
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

    /// `BYMONTH`, both directions. The yearly rule is RFC 5545 section 3.8.5.3's "yearly in
    /// June and July", whose day comes from `DTSTART` because nothing else names one.
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

    /// `BYYEARDAY`, both directions. The yearly rule is section 3.8.5.3's "every 3rd year on
    /// the 1st, 100th and 200th day"; 1997 is common, so day 100 is April 10th and day 200 is
    /// July 19th.
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
        assert!(candidates(&daily, start, 0).is_empty());
        assert_eq!(
            candidates(&daily, start, 2),
            expected(&[(1997, 9, 4, 9, 0)])
        );
    }

    /// Note 1, both branches. The first is section 3.8.5.3's "monthly on the first Friday";
    /// the second is its "every Friday the 13th", where `BYMONTHDAY` turns `BYDAY` into a
    /// limit and February 1998 is the first month that has one.
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
    /// The limit branch is section 3.8.5.3's U.S. presidential election day. The week branch
    /// is its "Monday of week number 20". The month branch is its "every Thursday in March".
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

    /// Note 2's branch order is load-bearing: `BYWEEKNO` beats `BYMONTH`, so the Mondays are
    /// counted within week 20 and not within May. Held against the table directly as well as
    /// through an expansion, because the two could agree while both being wrong.
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
    /// February has no thirtieth in any Gregorian year, so the period costs a budget, says
    /// what it dropped and hands back nothing. A budget charged for what a period produced
    /// would let this rule walk the calendar forever.
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

    /// The leap day. A `DTSTART` of February 29th names a date three years in four do not
    /// have, and section 3.3.10 says the instance is ignored rather than moved to the 28th.
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
    /// The first is section 3.8.5.3's "monthly on the first and last day of the month", where
    /// `BYMONTHDAY=-1` counts back from an end that moves. The second is a `DTSTART` on a 31st
    /// under `FREQ=MONTHLY`, which skips the months that have no 31st rather than clamping to
    /// their last day.
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
    /// `BYWEEKNO` is `Expand` under `FREQ=YEARLY`, so period Y expands to week one *of year Y*
    /// — and week one of 2019 begins on Monday December 31st of 2018, which is a day of the
    /// calendar year before the one that numbers it. Each period therefore contributes exactly
    /// one Monday and no period contributes another's: the union is the same either way, which
    /// is why reading `BYWEEKNO` as a filter over the calendar year's own days survives every
    /// rule with `INTERVAL=1` and no `BYSETPOS`, and is wrong for every rule with either.
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
    /// The other half of the same mechanism. Under `WKST=SU` the week-numbering year 2014 runs
    /// to Saturday January 3rd 2015, so week 53 of 2014 holds days of calendar 2015 — and 2015
    /// itself holds 52 weeks, so `BYWEEKNO=53` names nothing at all in its period even though
    /// January 1st 2015 carries the number 53.
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
    /// The period walk carries an anchor and no upper edge, so the width a period contributes
    /// is this unit's answer alone. `FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,-1` skips February,
    /// and the period after January must offer March's two days and neither of February's — a
    /// period two months wide would put February's candidates where `BYSETPOS` would then
    /// select the wrong one of them.
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
    /// RFC 5545 section 3.3.10 forbids the numeric form wherever `FREQ` is neither `MONTHLY`
    /// nor `YEARLY`, and gives no reading for a file that carries one anyway. All five of those
    /// frequencies answer the same way here — the weekday is kept and the ordinal ignored —
    /// because the alternative was that `FREQ=WEEKLY` resolved the ordinal inside a scope one
    /// week wide, where `2TU` matched nothing and silently emptied the entire series while
    /// `1TU` worked.
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

    /// `WKST` moves the week a weekly period covers, which is what makes two rules differing
    /// only in it produce different days. Section 3.8.5.3 makes the same point with a pair of
    /// occurrence lists.
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

    /// The clock rows, both directions. The expanding fixture is section 3.8.5.3's "every 20
    /// minutes from 9:00 AM to 4:40 PM", which is twenty-four readings of one day.
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

    /// `BYSETPOS` is unit 4's. This period must hold every Tuesday, Wednesday and Thursday of
    /// September 1997 — thirteen of them — and not the third one alone, which is what section
    /// 3.8.5.3's "third instance into the month" rule finally emits.
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
    /// which is what "Expand" means in a cell whose period is a year. With a `BYMONTH` beside
    /// it the month is pinned and one day survives.
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

    /// Everything the rule leaves unstated comes from `DTSTART`, at whatever granularity the
    /// period and the expanding span parts left free.
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
            Err(LimitExceeded::Candidates)
        );

        let mut spent = Meter::with_budget(Limits::DEFAULT, 8);
        let same = PeriodWalk::new(start, &rule).next().unwrap();
        spent.open_period();
        assert_eq!(
            expand_period(same, &rule, start, &mut spent, &mut kept),
            Err(LimitExceeded::Budget)
        );
        assert!(spent.is_exhausted(), "the shared ledger latches");
    }

    /// The set is a set: two parts naming one instant produce it once, and the order is
    /// chronological because that is the order `BYSETPOS` counts in.
    #[test]
    fn the_candidate_set_is_ascending_and_holds_no_repeat() {
        let rule = RecurrenceRuleBuilder::new(Freq::Daily)
            .by_hour(ByList::from_slice(&[17_u8, 9, 9]))
            .build()
            .unwrap();
        let stamps = candidates(&rule, stamp(1997, 9, 2, 9, 0), 0);
        assert_eq!(stamps, expected(&[(1997, 9, 2, 9, 0), (1997, 9, 2, 17, 0)]));
    }

    /// An empty set is a value rather than an absence, and `CandidateSet` says which it is
    /// without a caller reaching for the slice.
    #[test]
    fn an_empty_candidate_set_reports_itself() {
        let empty = CandidateSet::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(empty.as_slice().is_empty());
    }

    /// The arithmetic the negative forms all share, at both ends and at the value RFC 5545
    /// section 3.3.10 excludes from every one of them.
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

    /// Week numbering, against dates whose ISO week is known independently, and under a `WKST`
    /// other than the Monday ISO 8601 fixes.
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
