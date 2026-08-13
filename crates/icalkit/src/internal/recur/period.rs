// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The period walk: stepping the base cadence, once for all seven frequencies.
//!
//! Specification: RFC 5545 section 3.3.10's `FREQ`, `INTERVAL` and `WKST`.
//!
//! A *period* is the span one `FREQ` names — a second, a minute, an hour, a day, a week, a
//! month, a year — and it is the unit the whole of section 3.3.10 is written in: the `BYxxx`
//! parts produce candidates *within* one period, and `BYSETPOS` selects from what one period
//! produced. This module produces the periods and nothing else. It applies no `BYxxx` part,
//! resolves no local time against a zone, and charges no meter, because a period anchor is not
//! a candidate and only a candidate is any of those things.
//!
//! # The anchor is the period, not the instance
//!
//! A period's anchor is where the period *begins*, not where `DTSTART` falls inside it. A
//! `FREQ=MONTHLY` rule whose `DTSTART` is the 31st of January anchors on the 1st of January,
//! and its next anchor is the 1st of February — not the 28th, which is the clamp
//! `docs/adr/0011` forbids, and not the 31st of March, which is February's period deleted for
//! want of a 31st day. February's period has to exist: `FREQ=MONTHLY;BYMONTHDAY=1` under that
//! same `DTSTART` has an instance on the 1st of February, and a walk that skipped the period
//! leaves nothing downstream able to recover it. Whether a period holds any *instance* is
//! decided per candidate, one layer up, where a date that does not exist is filtered rather
//! than moved.
//!
//! The fields no rule part states go the same way. A period anchored at midnight does not say
//! the rule recurs at midnight: section 3.3.10 takes every unstated field from `DTSTART`, and
//! the expansion is handed `DTSTART` for exactly that. For the same reason the first period
//! usually begins *before* `DTSTART` — instances before it are not in the recurrence set, and
//! dropping them belongs to the search, because a first period that began at `DTSTART` would
//! misplace every anchor after it too.
//!
//! # A period spans one frequency, and `INTERVAL` separates two of them
//!
//! `FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,15` recurs twice in January and not at all in
//! February, so a period is one `FREQ` unit wide and `INTERVAL` is the distance between two
//! anchors. A period two months wide would put February's candidates into January's set, where
//! `BYSETPOS=-1` would then select the wrong one of them.
//!
//! # Three arithmetics, not seven
//!
//! The seven frequencies are three kinds of stepping — a multiple of a second, a multiple of a
//! day, a multiple of a month — and [`Cadence`] is that observation as data. Written as seven
//! generators, the month arithmetic gets written twice and the week origin three times, and the
//! copies drift apart at a leap day. Which of the three a frequency uses is not cosmetic: a
//! `FREQ=DAILY` rule recurs at the same wall clock on the next date, which is 86,400 seconds
//! later here and is not once a caller resolves the result against a zone, so a day is stepped
//! on the calendar and never as a count of seconds.
//!
//! Every anchor is computed from the origin rather than from the anchor before it, so
//! `resume_at(dtstart, rule, n)` and `n` calls to `next` agree by construction instead of by
//! argument.
//!
//! # A period is named by where it begins, and by nothing else
//!
//! A period carries its anchor alone. It spans one `FREQ` unit, and every consumer that needs
//! that span recomputes it from the anchor and the frequency — `byparts::expand_period` says so
//! itself, that a period "is read for its anchor alone". A stored upper edge was therefore a
//! field nothing read, and storing it cost the last period of the calendar: the daily period
//! anchored 9999-12-31 would end on 10000-01-01, which RFC 5545 section 3.3.4 cannot write, so
//! requiring both bounds to exist deleted a period whose every instance is representable and
//! legal. The RFC's own answer for `FREQ=DAILY` from 9999-12-28 includes December 31st.
//!
//! # Where the walk ends
//!
//! At `None`, when the next period's *anchor* is not one the calendar can express: there is no
//! year 10000 to anchor in. Saturating instead would report a period no file can name, and
//! repeating the final anchor forever would hang the search that `docs/adr/0002`'s budget
//! exists to bound.

use core::iter::FusedIterator;
use core::num::NonZeroU32;

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Duration, MonthAddOutcome, Weekday,
};

use crate::internal::recur::rule::{Freq, RecurrenceRule};

/// Seconds in a minute, which is what one `FREQ=MINUTELY` period spans.
const SECONDS_PER_MINUTE: i64 = 60;

/// Seconds in an hour, which is what one `FREQ=HOURLY` period spans.
const SECONDS_PER_HOUR: i64 = 3_600;

/// Days in a week: what one `FREQ=WEEKLY` period spans, and the modulus a week origin counts
/// a weekday within.
const DAYS_PER_WEEK: i64 = 7;

/// Months in a year, which is what one `FREQ=YEARLY` period spans.
const MONTHS_PER_YEAR: i32 = 12;

/// The largest second a civil time carries once RFC 5545 section 3.3.12's leap second is folded
/// onto the second before it, as `ical-core`'s arithmetic already folds it.
const LAST_SECOND_OF_MINUTE: u8 = 59;

/// What one period of a frequency spans, in the arithmetic that span is exact in.
///
/// Three shapes rather than seven. A month is not a fixed number of days in any calendar, which
/// is why the third exists at all; a day is not a fixed number of seconds under a zone, which
/// is why the second is not folded into the first even though nothing at this layer has a zone
/// to tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cadence {
    /// Whole seconds: `SECONDLY`, `MINUTELY`, `HOURLY`.
    Seconds(i64),
    /// Whole days: `DAILY`, `WEEKLY`.
    Days(i64),
    /// Whole months: `MONTHLY`, `YEARLY`.
    Months(i32),
}

/// What one period of `freq` spans.
const fn cadence(freq: Freq) -> Cadence {
    match freq {
        Freq::Secondly => Cadence::Seconds(1),
        Freq::Minutely => Cadence::Seconds(SECONDS_PER_MINUTE),
        Freq::Hourly => Cadence::Seconds(SECONDS_PER_HOUR),
        Freq::Daily => Cadence::Days(1),
        Freq::Weekly => Cadence::Days(DAYS_PER_WEEK),
        Freq::Monthly => Cadence::Months(1),
        Freq::Yearly => Cadence::Months(MONTHS_PER_YEAR),
    }
}

/// One period of the base cadence, named by where it begins.
///
/// Consecutive periods tile the timeline without overlapping — no instant belongs to two of
/// them and none falls between two — so the anchor of the next period is where this one ends
/// and there is nothing for a second field to hold. The span itself is one `FREQ` unit and is
/// recomputed from the anchor wherever it is needed, which is the expansion and nowhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Period {
    /// Where the period begins.
    anchor: CivilDateTime,
}

impl Period {
    /// Where the period begins.
    #[must_use]
    pub const fn anchor(self) -> CivilDateTime {
        self.anchor
    }
}

/// The periods of one rule, in order, from the one holding `DTSTART`.
///
/// An `Iterator` rather than a collection for `docs/adr/0002`'s reason: a rule with neither
/// `COUNT` nor `UNTIL` has no last period, and `FREQ=SECONDLY` is legal.
#[derive(Clone, Debug)]
#[must_use = "a period walk computes nothing until it is iterated"]
pub struct PeriodWalk {
    /// The anchor of period 0 — `DTSTART`'s own period, normalized to its start — or `None`
    /// when that start is not a date RFC 5545 section 3.3.4 can write, which is a walk with no
    /// periods in it rather than a walk that begins somewhere else.
    origin: Option<CivilDateTime>,
    /// What one period spans.
    cadence: Cadence,
    /// How many cadence units separate one anchor from the next.
    interval: NonZeroU32,
    /// The index of the period the next call to [`Iterator::next`] will produce.
    index: u64,
    /// Whether the walk has run out. Latched, so that [`FusedIterator`] is a property of the
    /// state machine rather than an argument about the calendar being monotone.
    finished: bool,
}

impl PeriodWalk {
    /// The periods of `rule`, starting with the one `dtstart` falls in.
    pub fn new(dtstart: CivilDateTime, rule: &RecurrenceRule) -> Self {
        Self::resume_at(dtstart, rule, 0)
    }

    /// The periods of `rule` from `index`, counted as [`PeriodWalk::index`] counts.
    ///
    /// `dtstart` is still required rather than a stored anchor, because an anchor alone cannot
    /// say which week `WKST` began or which day of a month `DTSTART` sat in, and a resumed walk
    /// that guessed either would drift away from the walk it claims to continue.
    pub fn resume_at(dtstart: CivilDateTime, rule: &RecurrenceRule, index: u64) -> Self {
        let freq = rule.freq();
        Self {
            origin: period_start(dtstart, freq, rule.wkst()),
            cadence: cadence(freq),
            interval: rule.interval(),
            index,
            finished: false,
        }
    }

    /// The index of the period the next call to [`Iterator::next`] will produce.
    ///
    /// Zero for a fresh walk, so that `resume_at(dtstart, rule, walk.index())` continues a walk
    /// exactly where it was left. A walk that has run out reports the index it could not reach,
    /// and resuming there produces nothing, which is the same answer twice rather than two.
    #[must_use]
    pub const fn index(&self) -> u64 {
        self.index
    }

    /// The period at `index`, or `None` when it is not one this calendar can express.
    ///
    /// Expressible is a claim about the anchor and never about the edge past it. A period whose
    /// anchor is a date RFC 5545 section 3.3.4 can write holds only instants at or after that
    /// anchor, and every one of them is representable however far the following anchor would
    /// be — which is why the year 9999 has a yearly period and its December 31st has a daily
    /// one.
    fn period_at(&self, index: u64) -> Option<Period> {
        let steps = i64::try_from(index)
            .ok()?
            .checked_mul(i64::from(self.interval.get()))?;
        let anchor = advance(self.origin?, self.cadence, steps)?;
        Some(Period { anchor })
    }
}

impl Iterator for PeriodWalk {
    type Item = Period;

    fn next(&mut self) -> Option<Period> {
        if self.finished {
            return None;
        }
        let Some(period) = self.period_at(self.index) else {
            self.finished = true;
            return None;
        };
        match self.index.checked_add(1) {
            Some(following) => self.index = following,
            // Out of reach for a walk that started at zero, since the calendar runs out long
            // before the counter does; latching rather than wrapping keeps that true for a walk
            // resumed at an index the caller chose.
            None => self.finished = true,
        }
        Some(period)
    }
}

impl FusedIterator for PeriodWalk {}

/// Where the period of `freq` holding `dtstart` begins, or `None` when that is not a date RFC
/// 5545 section 3.3.4 can write.
///
/// This is the one place the seven frequencies differ before the arithmetic, and each answer is
/// a truncation towards the start of the span the frequency names — never a `BYxxx` part and
/// never a zone.
fn period_start(dtstart: CivilDateTime, freq: Freq, wkst: Weekday) -> Option<CivilDateTime> {
    let date = dtstart.date();
    let clock = dtstart.time();
    match freq {
        // A leap second is folded here for the reason `ical-core`'s arithmetic folds it: the
        // timeline has no room for it, and an anchor that kept it would name a second the
        // period after this one also names.
        Freq::Secondly => at_time(
            date,
            clock.hour(),
            clock.minute(),
            clock.second().min(LAST_SECOND_OF_MINUTE),
        ),
        Freq::Minutely => at_time(date, clock.hour(), clock.minute(), 0),
        Freq::Hourly => at_time(date, clock.hour(), 0, 0),
        Freq::Daily => Some(midnight(date)),
        Freq::Weekly => Some(midnight(week_origin(date, wkst)?)),
        Freq::Monthly => Some(midnight(CivilDate::from_ymd(date.year(), date.month(), 1)?)),
        Freq::Yearly => Some(midnight(CivilDate::from_ymd(date.year(), 1, 1)?)),
    }
}

/// The most recent `wkst` on or before `date`, or `None` when it falls before the first date
/// RFC 5545 section 3.3.4 can write.
///
/// RFC 5545 section 3.3.10 gives `WKST` for exactly this: a `FREQ=WEEKLY` period is the week
/// `WKST` starts, not the seven days following `DTSTART`, and the two differ for every rule
/// whose `DTSTART` is not already on its week's first day.
fn week_origin(date: CivilDate, wkst: Weekday) -> Option<CivilDate> {
    let elapsed = i64::from(date.weekday()?.index())
        .checked_sub(i64::from(wkst.index()))?
        .rem_euclid(DAYS_PER_WEEK);
    date.checked_add_days(elapsed.checked_neg()?)
}

/// Midnight on `date`, where every period a day or longer begins.
const fn midnight(date: CivilDate) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::MIDNIGHT)
}

/// `date` at that time of day, or `None` when there is no such time.
fn at_time(date: CivilDate, hour: u8, minute: u8, second: u8) -> Option<CivilDateTime> {
    Some(CivilDateTime::new(
        date,
        CivilTime::from_hms(hour, minute, second)?,
    ))
}

/// `from` moved `count` cadence units, or `None` when the result leaves the years RFC 5545
/// section 3.3.4 can write.
///
/// The whole of the walk's arithmetic, so that `SECONDLY` and `YEARLY` are the same code with a
/// different unit and there is no second copy for one of them to be wrong in.
fn advance(from: CivilDateTime, cadence: Cadence, count: i64) -> Option<CivilDateTime> {
    match cadence {
        Cadence::Seconds(unit) => {
            let span = Duration::new(0, unit.checked_mul(count)?);
            from.checked_add_duration(span)
        },
        Cadence::Days(unit) => {
            let span = Duration::new(unit.checked_mul(count)?, 0);
            from.checked_add_duration(span)
        },
        Cadence::Months(unit) => {
            let months = i64::from(unit).checked_mul(count)?;
            shifted_by_months(from, i32::try_from(months).ok()?)
        },
    }
}

/// `from` moved `months` months, keeping its time of day, or `None` when there is no such
/// local date and time.
///
/// A clamped outcome answers `None` rather than handing back the date it carries. It is out of
/// reach — every anchor of a month-based cadence is the first of its month, and day 1 exists in
/// every month of every year — and taking the clamped date would be the coercion RFC 5545
/// section 3.3.10 and `docs/adr/0011` forbid, so the branch that cannot happen answers with the
/// same "no such period" an overflow does instead of with a plausible wrong one.
fn shifted_by_months(from: CivilDateTime, months: i32) -> Option<CivilDateTime> {
    let MonthAddOutcome::Exact(date) = from.date().add_months(months) else {
        return None;
    };
    Some(CivilDateTime::new(date, from.time()))
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::num::NonZeroU32;

    use crate::internal::core::{CivilDate, CivilDateTime, CivilTime, UtcOffset, Weekday};

    use super::{Period, PeriodWalk};
    use crate::internal::recur::rule::{
        ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, WeekdayNum,
    };

    /// A local date and time, spelled as the tables below spell one.
    fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> CivilDateTime {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        CivilDateTime::new(date, CivilTime::from_hms(hour, minute, second).unwrap())
    }

    /// Midnight on a date, which is what every anchor of a day or longer is.
    fn day_start(year: u16, month: u8, day: u8) -> CivilDateTime {
        at(year, month, day, 0, 0, 0)
    }

    /// A rule carrying only what a period walk reads.
    fn walk_rule(freq: Freq, interval: u32, wkst: Weekday) -> RecurrenceRule {
        RecurrenceRuleBuilder::new(freq)
            .interval(NonZeroU32::new(interval).unwrap())
            .wkst(wkst)
            .build()
            .unwrap()
    }

    /// The first `count` anchors of a walk.
    fn anchors_of(
        dtstart: CivilDateTime,
        rule: &RecurrenceRule,
        count: usize,
    ) -> Vec<CivilDateTime> {
        PeriodWalk::new(dtstart, rule)
            .take(count)
            .map(Period::anchor)
            .collect()
    }

    /// How many days separate each of the first `count` anchors of a walk from the next.
    ///
    /// Measured between two anchors rather than read off a stored upper edge, because a period
    /// carries none: consecutive periods tile the timeline, so at `INTERVAL=1` the next anchor
    /// *is* where this period ends, and the difference is what tells a leap year from a common
    /// one.
    fn spans_of(dtstart: CivilDateTime, rule: &RecurrenceRule, count: usize) -> Vec<i64> {
        let anchors = anchors_of(dtstart, rule, count.saturating_add(1));
        anchors
            .windows(2)
            .map(|pair| {
                let opens = pair[0].date().days_from_epoch().unwrap();
                let closes = pair[1].date().days_from_epoch().unwrap();
                closes.checked_sub(opens).unwrap()
            })
            .collect()
    }

    /// One `DTSTART`, seven frequencies: the period holding it begins where that frequency's
    /// span begins and never at `DTSTART` itself unless `DTSTART` is already on the boundary,
    /// and the period after it begins one span later.
    ///
    /// The second column is asserted through the next anchor rather than through an upper edge
    /// the period no longer carries, which is the same claim seen from the side that tiles.
    #[test]
    fn the_period_holding_a_dtstart_begins_where_its_frequency_begins() {
        // A Wednesday, at a time of day that is on no boundary at all.
        let dtstart = at(2026, 8, 5, 13, 47, 29);
        let cases: [(Freq, CivilDateTime, CivilDateTime); 7] = [
            (
                Freq::Secondly,
                at(2026, 8, 5, 13, 47, 29),
                at(2026, 8, 5, 13, 47, 30),
            ),
            (
                Freq::Minutely,
                at(2026, 8, 5, 13, 47, 0),
                at(2026, 8, 5, 13, 48, 0),
            ),
            (
                Freq::Hourly,
                at(2026, 8, 5, 13, 0, 0),
                at(2026, 8, 5, 14, 0, 0),
            ),
            (Freq::Daily, day_start(2026, 8, 5), day_start(2026, 8, 6)),
            (Freq::Weekly, day_start(2026, 8, 3), day_start(2026, 8, 10)),
            (Freq::Monthly, day_start(2026, 8, 1), day_start(2026, 9, 1)),
            (Freq::Yearly, day_start(2026, 1, 1), day_start(2027, 1, 1)),
        ];
        for (freq, opens, closes) in cases {
            let rule = walk_rule(freq, 1, Weekday::Monday);
            assert_eq!(anchors_of(dtstart, &rule, 2), vec![opens, closes]);
        }
    }

    /// The month-end case the milestone singles out. February is a period of this walk and its
    /// anchor is the 1st: `FREQ=MONTHLY;BYMONTHDAY=1` under this same `DTSTART` has an instance
    /// on the 1st of February, so a walk that clamped to the 28th or skipped to March would
    /// delete an instance that nothing downstream could put back.
    #[test]
    fn a_monthly_walk_from_the_thirty_first_keeps_february_and_reaches_march() {
        let dtstart = at(2026, 1, 31, 9, 0, 0);
        let rule = walk_rule(Freq::Monthly, 1, Weekday::Monday);
        let expected = vec![
            day_start(2026, 1, 1),
            day_start(2026, 2, 1),
            day_start(2026, 3, 1),
            day_start(2026, 4, 1),
            day_start(2026, 5, 1),
            day_start(2026, 6, 1),
            day_start(2026, 7, 1),
            day_start(2026, 8, 1),
        ];
        assert_eq!(anchors_of(dtstart, &rule, 8), expected);

        // The second period is the whole of February and not one day of it: it opens on the
        // 1st, the period after it opens on March 1st, and the 28 days between are February's.
        let february = PeriodWalk::new(dtstart, &rule).nth(1).unwrap();
        assert_eq!(february.anchor(), day_start(2026, 2, 1));
        assert_eq!(spans_of(dtstart, &rule, 2), vec![31_i64, 28]);
    }

    /// `INTERVAL` moves the anchors apart and leaves the origin where it was.
    ///
    /// That a period stays one `FREQ` unit wide however large the interval is no longer a claim
    /// this file can check — a period carries an anchor and the width belongs to
    /// `byparts::period_extent`, which asserts it against the days a skipped-over month
    /// contributes.
    #[test]
    fn an_interval_separates_anchors_without_widening_a_period() {
        let dtstart = at(2026, 1, 31, 9, 0, 0);
        let rule = walk_rule(Freq::Monthly, 2, Weekday::Monday);
        let expected = vec![
            day_start(2026, 1, 1),
            day_start(2026, 3, 1),
            day_start(2026, 5, 1),
            day_start(2026, 7, 1),
            day_start(2026, 9, 1),
            day_start(2026, 11, 1),
            day_start(2027, 1, 1),
            day_start(2027, 3, 1),
        ];
        assert_eq!(anchors_of(dtstart, &rule, 8), expected);
    }

    /// Every `WKST` the RFC admits, against one `DTSTART` that is none of them.
    #[test]
    fn a_weekly_walk_begins_on_the_week_start_the_rule_names() {
        // 2026-08-05 is a Wednesday, so only the Wednesday row anchors on `DTSTART`'s own date.
        let dtstart = at(2026, 8, 5, 8, 0, 0);
        let cases: [(Weekday, CivilDateTime); 7] = [
            (Weekday::Monday, day_start(2026, 8, 3)),
            (Weekday::Tuesday, day_start(2026, 8, 4)),
            (Weekday::Wednesday, day_start(2026, 8, 5)),
            (Weekday::Thursday, day_start(2026, 7, 30)),
            (Weekday::Friday, day_start(2026, 7, 31)),
            (Weekday::Saturday, day_start(2026, 8, 1)),
            (Weekday::Sunday, day_start(2026, 8, 2)),
        ];
        for (wkst, opens) in cases {
            let rule = walk_rule(Freq::Weekly, 1, wkst);
            let walked = anchors_of(dtstart, &rule, 8);
            assert_eq!(walked.first(), Some(&opens));
            for pair in walked.windows(2) {
                let following = pair[0].date().checked_add_days(7).unwrap();
                assert_eq!(pair[1].date(), following);
            }
        }
    }

    /// The eight anchors of one of those seven written out, so that the stride above is
    /// checked against dates and not only against itself.
    #[test]
    fn a_weekly_walk_crosses_a_month_boundary_a_week_at_a_time() {
        let dtstart = at(2026, 8, 5, 8, 0, 0);
        let rule = walk_rule(Freq::Weekly, 1, Weekday::Wednesday);
        let expected = vec![
            day_start(2026, 8, 5),
            day_start(2026, 8, 12),
            day_start(2026, 8, 19),
            day_start(2026, 8, 26),
            day_start(2026, 9, 2),
            day_start(2026, 9, 9),
            day_start(2026, 9, 16),
            day_start(2026, 9, 23),
        ];
        assert_eq!(anchors_of(dtstart, &rule, 8), expected);
    }

    /// A `FREQ=YEARLY` walk over 2100, which the Gregorian century rule makes a common year,
    /// and over 2000, which the quadricentennial rule makes a leap year. The lengths are the
    /// assertion that matters: a walk stepping 365 days instead of a year would drift a day per
    /// leap year and would agree with this test's anchors for a while first.
    #[test]
    fn a_yearly_walk_crosses_a_common_century_and_a_leap_one() {
        let rule = walk_rule(Freq::Yearly, 1, Weekday::Monday);

        let before_2100 = at(2097, 3, 15, 12, 0, 0);
        let over_the_century: Vec<CivilDateTime> =
            (2097_u16..2105).map(|year| day_start(year, 1, 1)).collect();
        assert_eq!(anchors_of(before_2100, &rule, 8), over_the_century);
        let common = vec![365_i64, 365, 365, 365, 365, 365, 365, 366];
        assert_eq!(spans_of(before_2100, &rule, 8), common);

        // A `DTSTART` on a leap day: 2001 through 2003 have no 29th of February, and their
        // periods are still walked. Dropping the instance is the candidate filter's business.
        let leap_day = at(2000, 2, 29, 0, 0, 0);
        let over_the_leap: Vec<CivilDateTime> =
            (2000_u16..2008).map(|year| day_start(year, 1, 1)).collect();
        assert_eq!(anchors_of(leap_day, &rule, 8), over_the_leap);
        let leaping = vec![366_i64, 365, 365, 365, 366, 365, 365, 365];
        assert_eq!(spans_of(leap_day, &rule, 8), leaping);
    }

    /// An `INTERVAL` past what an `i32` holds, walked eight times over, checked against Unix
    /// seconds rather than against dates this file would otherwise be grading itself on.
    #[test]
    fn a_secondly_interval_past_the_reach_of_an_i32_still_steps() {
        let dtstart = at(1970, 1, 1, 0, 0, 0);
        // Half again as large as `i32::MAX`, and eight steps of it is over six centuries.
        let interval = 3_000_000_000_u32;
        let rule = walk_rule(Freq::Secondly, interval, Weekday::Monday);
        let utc = UtcOffset::UTC;
        for (step, period) in PeriodWalk::new(dtstart, &rule).take(8).enumerate() {
            let elapsed = i64::try_from(step)
                .unwrap()
                .checked_mul(i64::from(interval))
                .unwrap();
            let opens = period.anchor().at_offset(utc).unwrap();
            assert_eq!(opens.unix_seconds(), elapsed);
        }
    }

    /// A `DAILY` walk steps onto the leap day rather than over it, and a `HOURLY` one carries
    /// the date across the year boundary.
    #[test]
    fn a_walk_crosses_a_leap_day_and_a_year_boundary_by_stepping_the_calendar() {
        let leap_year = at(2024, 2, 27, 6, 30, 0);
        let daily = walk_rule(Freq::Daily, 1, Weekday::Monday);
        let over_the_leap_day = vec![
            day_start(2024, 2, 27),
            day_start(2024, 2, 28),
            day_start(2024, 2, 29),
            day_start(2024, 3, 1),
            day_start(2024, 3, 2),
            day_start(2024, 3, 3),
            day_start(2024, 3, 4),
            day_start(2024, 3, 5),
        ];
        assert_eq!(anchors_of(leap_year, &daily, 8), over_the_leap_day);

        let new_year_eve = at(2026, 12, 31, 21, 30, 0);
        let hourly = walk_rule(Freq::Hourly, 2, Weekday::Monday);
        let over_the_year = vec![
            at(2026, 12, 31, 21, 0, 0),
            at(2026, 12, 31, 23, 0, 0),
            at(2027, 1, 1, 1, 0, 0),
            at(2027, 1, 1, 3, 0, 0),
            at(2027, 1, 1, 5, 0, 0),
            at(2027, 1, 1, 7, 0, 0),
            at(2027, 1, 1, 9, 0, 0),
            at(2027, 1, 1, 11, 0, 0),
        ];
        assert_eq!(anchors_of(new_year_eve, &hourly, 8), over_the_year);
    }

    /// The walk reads `FREQ`, `INTERVAL` and `WKST` and nothing else: adding three `BYxxx`
    /// parts, one of which selects the last candidate of a period, moves no anchor at all.
    #[test]
    fn a_by_part_does_not_move_a_period_anchor() {
        let dtstart = at(2026, 1, 31, 9, 0, 0);
        let plain = walk_rule(Freq::Monthly, 1, Weekday::Monday);
        let friday = WeekdayNum::new(None, Weekday::Friday).unwrap();
        let loaded = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month_day(ByList::from_slice(&[-1_i8]))
            .by_day(ByList::from_slice(&[friday]))
            .by_set_pos(ByList::from_slice(&[-1_i16]))
            .build()
            .unwrap();
        assert_eq!(
            anchors_of(dtstart, &plain, 8),
            anchors_of(dtstart, &loaded, 8)
        );
    }

    /// The end of the walk, which is the last anchor RFC 5545 section 3.3.4 can write.
    ///
    /// The year 9999 has a yearly period and its December has a monthly one, because both
    /// anchors are dates a file can name and every instant they hold is representable. What
    /// does not exist is the period after each, whose anchor would fall in the year 10000, and
    /// both walks say so by stopping rather than by saturating. Requiring an upper edge instead
    /// deleted these two periods and every instance in them, which RFC 5545 section 3.8.5.3's
    /// own reading of a `FREQ=YEARLY` rule contradicts.
    #[test]
    fn a_walk_stops_where_the_writable_calendar_stops() {
        let yearly = walk_rule(Freq::Yearly, 1, Weekday::Monday);
        let mut last_year = PeriodWalk::new(at(9999, 6, 15, 0, 0, 0), &yearly);
        assert_eq!(
            last_year.next().map(Period::anchor),
            Some(day_start(9999, 1, 1))
        );
        assert_eq!(last_year.next(), None);
        assert_eq!(last_year.next(), None);

        let monthly = walk_rule(Freq::Monthly, 1, Weekday::Monday);
        let mut last_months = PeriodWalk::new(at(9999, 11, 15, 0, 0, 0), &monthly);
        let november = last_months.next().unwrap();
        assert_eq!(november.anchor(), day_start(9999, 11, 1));
        let december = last_months.next().unwrap();
        assert_eq!(december.anchor(), day_start(9999, 12, 1));
        assert_eq!(last_months.next(), None);
        // `FusedIterator` is a promise, so asking again is defined and answers the same.
        assert_eq!(last_months.next(), None);
    }

    /// The last day, hour, minute and second of the calendar each have a period of their own.
    ///
    /// One assertion per sub-monthly cadence, because the defect this replaces dropped every
    /// one of them: the daily period anchored 9999-12-31 would have ended on 10000-01-01, and
    /// so would the hourly one anchored at 23:00 and the secondly one at 23:59:59.
    #[test]
    fn the_last_period_of_each_cadence_exists_although_nothing_follows_it() {
        let cases: [(Freq, CivilDateTime); 4] = [
            (Freq::Daily, day_start(9999, 12, 31)),
            (Freq::Hourly, at(9999, 12, 31, 23, 0, 0)),
            (Freq::Minutely, at(9999, 12, 31, 23, 59, 0)),
            (Freq::Secondly, at(9999, 12, 31, 23, 59, 59)),
        ];
        for (freq, last) in cases {
            let rule = walk_rule(freq, 1, Weekday::Monday);
            let mut walk = PeriodWalk::new(last, &rule);
            assert_eq!(walk.next().map(Period::anchor), Some(last), "{freq:?}");
            assert_eq!(walk.next(), None, "{freq:?}");
        }
    }

    /// The other end. 0000-01-01 is a Saturday, so a Monday-start week begins five days before
    /// the first date this calendar has and the walk is empty, while a Saturday-start week
    /// begins on the day itself.
    #[test]
    fn a_week_origin_before_the_first_writable_day_is_an_empty_walk() {
        let dtstart = at(0, 1, 1, 0, 0, 0);
        let from_monday = walk_rule(Freq::Weekly, 1, Weekday::Monday);
        let mut empty = PeriodWalk::new(dtstart, &from_monday);
        assert_eq!(empty.next(), None);
        assert_eq!(empty.next(), None);

        let from_saturday = walk_rule(Freq::Weekly, 1, Weekday::Saturday);
        assert_eq!(
            anchors_of(dtstart, &from_saturday, 2),
            vec![day_start(0, 1, 1), day_start(0, 1, 8)]
        );
    }

    /// Resuming and walking are the same sequence, because both are computed from the origin.
    #[test]
    fn resuming_at_an_index_lands_where_walking_to_it_lands() {
        let dtstart = at(2026, 1, 31, 9, 0, 0);
        let rule = walk_rule(Freq::Monthly, 3, Weekday::Monday);
        let mut walked = PeriodWalk::new(dtstart, &rule);
        assert_eq!(walked.index(), 0);
        assert_eq!(walked.by_ref().take(5).count(), 5);
        assert_eq!(walked.index(), 5);

        let mut resumed = PeriodWalk::resume_at(dtstart, &rule, 5);
        assert_eq!(resumed.index(), 5);
        for _ in 0..3 {
            assert_eq!(resumed.next(), walked.next());
        }
        assert_eq!(resumed.index(), walked.index());

        // Five periods of three months each past January 2026 is April 2027, stated here
        // rather than read back off the walk this test is checking.
        let mut fifth = PeriodWalk::resume_at(dtstart, &rule, 5);
        assert_eq!(
            fifth.next().map(Period::anchor),
            Some(day_start(2027, 4, 1))
        );
    }

    /// An index no calendar can reach is the ordinary end of a walk and not an overflow.
    #[test]
    fn an_index_past_the_calendar_yields_nothing_rather_than_wrapping() {
        let dtstart = at(2026, 1, 31, 9, 0, 0);
        let rule = walk_rule(Freq::Monthly, 1, Weekday::Monday);
        let mut far = PeriodWalk::resume_at(dtstart, &rule, u64::MAX);
        assert_eq!(far.next(), None);
        assert_eq!(far.index(), u64::MAX);
        let mut past = PeriodWalk::resume_at(dtstart, &rule, 100_000);
        assert_eq!(past.next(), None);
    }

    /// A `DTSTART` on a leap second anchors on the second before it, which is where
    /// `ical-core`'s arithmetic folds it. An anchor that kept the 60 would name a second the
    /// following period also names.
    #[test]
    fn a_leap_second_dtstart_anchors_on_the_second_before_it() {
        let dtstart = at(2026, 6, 30, 23, 59, 60);
        let rule = walk_rule(Freq::Secondly, 1, Weekday::Monday);
        assert_eq!(
            anchors_of(dtstart, &rule, 2),
            vec![at(2026, 6, 30, 23, 59, 59), day_start(2026, 7, 1)]
        );
    }
}
