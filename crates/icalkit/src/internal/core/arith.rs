// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checked arithmetic over the civil-time types.
//!
//! Specification: RFC 5545 section 3.3.4, section 3.3.5, and section 3.3.10's rule about
//! instances that do not exist.
//!
//! Every value reaching this module was validated when it was constructed, so nothing here
//! has to decide what an impossible input means. What it does have to decide is what an
//! impossible *result* means, and it answers that in one of two shapes. An operation whose
//! answer is not expressible returns `None`. Adding months has three answers rather than
//! two, because "the 31st of a 30-day month" and "the year 12000" are different failures and
//! a recurrence rule treats them differently: the first is [`MonthAddOutcome::Clamped`],
//! which carries the day that was asked for so a caller obeying RFC 5545 section 3.3.10 can
//! drop the instance and still say why it vanished, and the second is
//! [`MonthAddOutcome::Overflow`].
//!
//! Nothing below coerces. A date is never moved to a nearby one, an offset is never assumed,
//! and no clock is read. The one value folded rather than refused is a leap second, which
//! section 3.3.12 writes and the Unix timeline has no room for: the arithmetic uses `59`
//! while the preserved text keeps the `60` its producer wrote, so the fold is invisible to
//! the round trip `docs/adr/0001` promises.
//!
//! The date conversions count from the era beginning on March 1st of a year divisible by
//! 400. Starting the year in March puts the leap day at its end, which makes every era of
//! 146097 days identical and takes the leap-year branching out of both directions; the
//! `153/5` terms are a closed form of that year's cumulative month lengths, exact over the
//! whole domain rather than an approximation.

use crate::internal::core::Instant;

use crate::internal::core::gregorian::{
    CivilDate, CivilDateTime, CivilTime, Duration, MonthAddOutcome, UtcOffset, Weekday,
};

/// Days from 0000-03-01, the era origin, to 1970-01-01, the epoch results are reported
/// against.
const DAYS_FROM_ERA_TO_EPOCH: i64 = 719_468;

/// Days in one 400-year era, the cycle the Gregorian leap rule repeats on.
const DAYS_PER_ERA: i64 = 146_097;

/// Years in one era.
const YEARS_PER_ERA: i64 = 400;

/// Days in a common year, before the leap corrections are applied.
const DAYS_PER_COMMON_YEAR: i64 = 365;

/// Months in a year, which is the width of the mixed radix `add_months` counts in.
const MONTHS_PER_YEAR: i32 = 12;

/// Seconds in a day. RFC 5545 puts no leap second on the timeline, so this is a constant
/// rather than a lookup.
const SECONDS_PER_DAY: i64 = 86_400;

/// Seconds in an hour.
const SECONDS_PER_HOUR: i64 = 3_600;

/// Seconds in a minute.
const SECONDS_PER_MINUTE: i64 = 60;

/// Minutes in an hour.
const MINUTES_PER_HOUR: i64 = 60;

impl CivilDate {
    /// Days from 1970-01-01, negative for a date before it.
    ///
    /// `None` is unreachable for a date that exists: the widest span this type admits is
    /// under four million days and every step below runs in `i64`. The checked chain is
    /// written anyway, because a bound that holds today is not a bound the compiler checks,
    /// and an operator here would report a wrapped answer as a successful one.
    #[must_use]
    pub fn days_from_epoch(self) -> Option<i64> {
        let year = i64::from(self.year());
        let month = i64::from(self.month());
        let day_of_month = i64::from(self.day());
        let shifted_year = if month <= 2 {
            // January and February belong to the March-based year before them.
            year.checked_sub(1)?
        } else {
            year
        };
        // The divisor is a positive constant, so neither of these is the one division `i64`
        // cannot perform, and the remainder lands in `0..400`.
        let era = shifted_year.div_euclid(YEARS_PER_ERA);
        let year_of_era = shifted_year.rem_euclid(YEARS_PER_ERA);
        let day_of_year = day_of_year_from_march(month, day_of_month)?;
        let day_of_era = year_of_era
            .checked_mul(DAYS_PER_COMMON_YEAR)?
            .checked_add(year_of_era.div_euclid(4))?
            .checked_sub(year_of_era.div_euclid(100))?
            .checked_add(day_of_year)?;
        era.checked_mul(DAYS_PER_ERA)?
            .checked_add(day_of_era)?
            .checked_sub(DAYS_FROM_ERA_TO_EPOCH)
    }

    /// The date `days` after 1970-01-01, or `None` when that is not a date RFC 5545 section
    /// 3.3.4 can write back.
    ///
    /// The bound is the format's rather than the integer's: a year past 9999 has no
    /// four-digit spelling, so producing one would build a value that cannot be serialized.
    #[must_use]
    pub fn from_days_from_epoch(days: i64) -> Option<Self> {
        let from_era_origin = days.checked_add(DAYS_FROM_ERA_TO_EPOCH)?;
        // A positive divisor again, so the remainder lands in `0..146_097` and the quotient
        // names the era holding the day however far before the epoch it falls.
        let era = from_era_origin.div_euclid(DAYS_PER_ERA);
        let day_of_era = from_era_origin.rem_euclid(DAYS_PER_ERA);
        let year_of_era = year_of_era_holding(day_of_era)?;
        let leap_days = year_of_era
            .div_euclid(4)
            .checked_sub(year_of_era.div_euclid(100))?;
        let day_of_year = day_of_era
            .checked_sub(year_of_era.checked_mul(DAYS_PER_COMMON_YEAR)?)?
            .checked_sub(leap_days)?;
        let era_start_year = era.checked_mul(YEARS_PER_ERA)?;
        let shifted_year = era_start_year.checked_add(year_of_era)?;
        from_march_year(shifted_year, day_of_year)
    }

    /// The day of the week this date falls on.
    ///
    /// `None` only where [`CivilDate::days_from_epoch`] is, which is nowhere a constructed
    /// date can reach.
    #[must_use]
    pub fn weekday(self) -> Option<Weekday> {
        // 1970-01-01 was a Thursday, which is index 3 counted from Monday, so shifting by
        // three moves the epoch onto the origin `Weekday::index` counts from.
        let index = self.days_from_epoch()?.checked_add(3)?.rem_euclid(7);
        // A remainder against a positive divisor is in `0..7`, which `from_index` accepts,
        // so neither the narrowing nor the lookup is the step that can fail here.
        Weekday::from_index(u8::try_from(index).ok()?)
    }

    /// The date `count` months later, saying which of three things happened.
    ///
    /// A day the target month does not have is [`MonthAddOutcome::Clamped`] carrying the day
    /// that was asked for, and never a nearby date: RFC 5545 section 3.3.10 requires such a
    /// recurrence instance to be ignored, and a caller cannot ignore what it was never told
    /// about. A target year outside the four digits section 3.3.4 writes is
    /// [`MonthAddOutcome::Overflow`], as is a `count` so large that the month index itself
    /// does not fit.
    #[must_use]
    pub fn add_months(self, count: i32) -> MonthAddOutcome {
        let Some(target) = self.first_of_shifted_month(count) else {
            // Day 1 exists in every month, so the only thing that can have failed in there
            // is the year, which is exactly what `Overflow` names.
            return MonthAddOutcome::Overflow;
        };
        let Some(length) = Self::days_in_month(target.year(), target.month()) else {
            // Unreachable: `target` came out of `from_ymd`, which accepts only a month that
            // has a length. Answered rather than asserted, because nothing here may panic.
            return MonthAddOutcome::Overflow;
        };
        let requested_day = self.day();
        if requested_day <= length {
            match Self::from_ymd(target.year(), target.month(), requested_day) {
                Some(date) => MonthAddOutcome::Exact(date),
                // Unreachable for the same reason, and answered the same way.
                None => MonthAddOutcome::Overflow,
            }
        } else {
            match Self::from_ymd(target.year(), target.month(), length) {
                Some(date) => MonthAddOutcome::Clamped {
                    date,
                    requested_day,
                },
                None => MonthAddOutcome::Overflow,
            }
        }
    }

    /// The date `days` later, or `None` when it leaves the years RFC 5545 section 3.3.4 can
    /// write.
    ///
    /// Days rather than a [`Duration`], because a duration also carries seconds and a date
    /// has no clock for them to move.
    #[must_use]
    pub fn checked_add_days(self, days: i64) -> Option<Self> {
        let moved = self.days_from_epoch()?.checked_add(days)?;
        Self::from_days_from_epoch(moved)
    }

    /// The first day of the month `count` months from this date's, or `None` when that month
    /// falls outside the years RFC 5545 section 3.3.4 can write.
    ///
    /// Day 1 exists in every month of every year, so this fails on the year bound and on
    /// nothing else. That is what lets [`CivilDate::add_months`] tell `Overflow` apart from
    /// `Clamped` without a range check of its own, and it is why this file needs no second
    /// copy of the maximum year: the answer is the one `from_ymd` already gives.
    fn first_of_shifted_month(self, count: i32) -> Option<Self> {
        let total_months = i32::from(self.year())
            .checked_mul(MONTHS_PER_YEAR)?
            .checked_add(i32::from(self.month()))?
            .checked_sub(1)?
            .checked_add(count)?;
        // A total below zero is a year before 0 and `u16::try_from` refuses it, a total past
        // `u16::MAX` is refused there too, and `from_ymd` refuses everything between that
        // and 9999. All three are the same answer to the caller.
        let year_index = total_months.div_euclid(MONTHS_PER_YEAR);
        let month_of_year = total_months.rem_euclid(MONTHS_PER_YEAR).checked_add(1)?;
        let year = u16::try_from(year_index).ok()?;
        let month = u8::try_from(month_of_year).ok()?;
        Self::from_ymd(year, month, 1)
    }
}

impl CivilDateTime {
    /// The instant this local date and time names when the clock showing it runs `offset`
    /// from UTC, or `None` when the result is not on the representable timeline.
    ///
    /// A leap second is folded onto the second before it, per the note at the top of this
    /// module: the value keeps the date it was written on rather than carrying into the next
    /// one.
    #[must_use]
    pub fn at_offset(self, offset: UtcOffset) -> Option<Instant> {
        let offset_seconds = i64::from(offset.seconds());
        let day_count = self.date().days_from_epoch()?;
        // East of UTC is ahead of it, so a wall clock reading these fields there names an
        // instant that many seconds earlier on the timeline.
        let unix_seconds = day_count
            .checked_mul(SECONDS_PER_DAY)?
            .checked_add(seconds_of_day(self.time()))?
            .checked_sub(offset_seconds)?;
        Some(Instant::from_unix_seconds(unix_seconds))
    }

    /// What a clock running `offset` from UTC shows at `instant`, or `None` when that falls
    /// outside the years RFC 5545 section 3.3.4 can write.
    ///
    /// The inverse of [`CivilDateTime::at_offset`] except at a leap second, which has no
    /// instant of its own to come back from.
    #[must_use]
    pub fn from_instant(instant: Instant, offset: UtcOffset) -> Option<Self> {
        let offset_seconds = i64::from(offset.seconds());
        let local = instant.unix_seconds().checked_add(offset_seconds)?;
        // The divisor is positive, so the remainder is in `0..86_400` and the quotient is
        // the day holding it on both sides of the epoch.
        let date = CivilDate::from_days_from_epoch(local.div_euclid(SECONDS_PER_DAY))?;
        let time = time_of_day(local.rem_euclid(SECONDS_PER_DAY))?;
        Some(Self::new(date, time))
    }

    /// This local date and time, `span` later, or `None` when the result leaves the years
    /// RFC 5545 section 3.3.4 can write.
    ///
    /// The days move the date and the seconds move the clock, which is what section 3.3.6's
    /// two designators mean. There is no zone at this layer for the two to disagree under,
    /// and no [`Duration`] can carry a year or a month, so month arithmetic cannot arrive
    /// here disguised as a span: it is [`CivilDate::add_months`] or it is nothing. A leap
    /// second in the input is folded onto `59` before anything is added to it.
    #[must_use]
    pub fn checked_add_duration(self, span: Duration) -> Option<Self> {
        let clock = seconds_of_day(self.time());
        let second_of_day = clock.checked_add(span.seconds())?;
        let start_day = self.date().days_from_epoch()?;
        // Seconds past midnight roll into the following day and seconds before it into the
        // preceding one; `div_euclid` gives that borrow the right sign in both directions,
        // where a truncating division would round a negative span towards midnight.
        let day_count = start_day
            .checked_add(span.days())?
            .checked_add(second_of_day.div_euclid(SECONDS_PER_DAY))?;
        let date = CivilDate::from_days_from_epoch(day_count)?;
        let time = time_of_day(second_of_day.rem_euclid(SECONDS_PER_DAY))?;
        Some(Self::new(date, time))
    }
}

/// The position of `day_of_month` in `month` counted from March 1st, which is `0`.
///
/// `None` never happens for a month a date can hold; the checked form is what keeps that
/// true if the set of callers ever grows.
fn day_of_year_from_march(month: i64, day_of_month: i64) -> Option<i64> {
    // March becomes 0 and February 11, which is what puts the leap day at the end of the
    // year, where no month after it has to step over one.
    let month_index = month.checked_add(9)?.rem_euclid(12);
    month_index
        .checked_mul(153)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(day_of_month)?
        .checked_sub(1)
}

/// The year within its era, `0` through `399`, that holds `day_of_era`.
///
/// `None` never happens for a `day_of_era` in `0..146_097`, which is the only range a
/// remainder against `DAYS_PER_ERA` can produce.
fn year_of_era_holding(day_of_era: i64) -> Option<i64> {
    // The three corrections remove the leap days accumulated so far before the division, so
    // that dividing by 365 still lands on the right year at the end of a leap year instead
    // of one year past it.
    let corrected = day_of_era
        .checked_sub(day_of_era.div_euclid(1460))?
        .checked_add(day_of_era.div_euclid(36_524))?
        .checked_sub(day_of_era.div_euclid(146_096))?;
    Some(corrected.div_euclid(DAYS_PER_COMMON_YEAR))
}

/// The date `day_of_year` days after March 1st of the March-based `shifted_year`, or `None`
/// when it is not one RFC 5545 section 3.3.4 can write.
fn from_march_year(shifted_year: i64, day_of_year: i64) -> Option<CivilDate> {
    // The inverse of the closed form in `day_of_year_from_march`: it names the month of a
    // March-based year outright rather than searching a table for it.
    let scaled = day_of_year.checked_mul(5)?.checked_add(2)?;
    let month_index = scaled.div_euclid(153);
    let month_start = month_index.checked_mul(153)?.checked_add(2)?;
    let day_offset = month_start.div_euclid(5);
    let day_of_month = day_of_year.checked_sub(day_offset)?.checked_add(1)?;
    // Indexes 0 through 9 are March to December of the same calendar year; 10 and 11 are the
    // January and February the March-based year borrowed from the next one.
    let (month, year) = if month_index < 10 {
        (month_index.checked_add(3)?, shifted_year)
    } else {
        (month_index.checked_sub(9)?, shifted_year.checked_add(1)?)
    };
    CivilDate::from_ymd(
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day_of_month).ok()?,
    )
}

/// Seconds from midnight, with a leap second folded onto the second before it.
///
/// Saturating rather than checked because every term is bounded on construction — the hour
/// is at most 23 and the minute at most 59 — so the sum is below 86400 and the saturation
/// these operators promise is unreachable. They are written rather than the operators
/// because that bound is the constructor's, and nothing rechecks it here.
fn seconds_of_day(time: CivilTime) -> i64 {
    let second = time.second().min(59);
    let hours = i64::from(time.hour());
    let minutes = i64::from(time.minute());
    let from_hours = hours.saturating_mul(SECONDS_PER_HOUR);
    let from_minutes = minutes.saturating_mul(SECONDS_PER_MINUTE);
    from_hours
        .saturating_add(from_minutes)
        .saturating_add(i64::from(second))
}

/// The time of day `second_of_day` seconds after midnight.
///
/// `None` outside `0..86_400`, which a remainder against a positive divisor cannot produce.
/// The range check is the shape an assertion would have taken in a file allowed to panic.
fn time_of_day(second_of_day: i64) -> Option<CivilTime> {
    let hour = second_of_day.div_euclid(SECONDS_PER_HOUR);
    let minute = second_of_day
        .div_euclid(SECONDS_PER_MINUTE)
        .rem_euclid(MINUTES_PER_HOUR);
    let second = second_of_day.rem_euclid(SECONDS_PER_MINUTE);
    CivilTime::from_hms(
        u8::try_from(hour).ok()?,
        u8::try_from(minute).ok()?,
        u8::try_from(second).ok()?,
    )
}

#[cfg(test)]
mod tests {
    use crate::internal::core::Instant;

    use crate::internal::core::gregorian::{
        CivilDate, CivilDateTime, CivilTime, Duration, MonthAddOutcome, UtcOffset, Weekday,
    };

    /// The first day this type admits. Every sweep below is anchored on the two ends rather
    /// than on round numbers, because that is where the interesting failures are.
    const FIRST_DAY: i64 = -719_528;

    /// The last day this type admits, which is 9999-12-31.
    const LAST_DAY: i64 = 2_932_896;

    fn day(year: u16, month: u8, day_of_month: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day_of_month).unwrap()
    }

    fn stamp(date: CivilDate, hour: u8, minute: u8, second: u8) -> CivilDateTime {
        let time = CivilTime::from_hms(hour, minute, second).unwrap();
        CivilDateTime::new(date, time)
    }

    /// Nothing this arithmetic runs over can be built in an impossible state, so no operation
    /// here has to decide what an impossible input means.
    #[test]
    fn the_types_this_arithmetic_runs_over_refuse_impossible_inputs() {
        assert_eq!(CivilDate::from_ymd(2027, 2, 29), None);
        assert_eq!(UtcOffset::from_seconds(86_400), None);
    }

    /// The one operation on the shared scalar that already exists answers `None` at the edge
    /// rather than wrapping, which is the shape every operation added here has to match.
    #[test]
    fn the_edge_of_the_timeline_is_none_and_not_a_wrap() {
        let latest = Instant::from_unix_seconds(i64::MAX);
        assert_eq!(latest.checked_add_seconds(1), None);
        assert!(latest.checked_add_seconds(0).is_some());
    }

    /// Day numbers known independently of this implementation, including both ends of the
    /// range and the century and quadricentennial years the Gregorian rule treats
    /// differently.
    #[test]
    fn the_day_number_matches_dates_computed_elsewhere() {
        let cases: [(u16, u8, u8, i64); 10] = [
            (1970, 1, 1, 0),
            (1969, 12, 31, -1),
            (1900, 1, 1, -25_567),
            (2000, 1, 1, 10_957),
            (2000, 3, 1, 11_017),
            (2024, 2, 29, 19_782),
            (2026, 8, 10, 20_675),
            (0, 1, 1, FIRST_DAY),
            (0, 3, 1, -719_468),
            (9999, 12, 31, LAST_DAY),
        ];
        for (year, month, day_of_month, days) in cases {
            let date = day(year, month, day_of_month);
            assert_eq!(date.days_from_epoch(), Some(days));
            assert_eq!(CivilDate::from_days_from_epoch(days), Some(date));
        }
    }

    /// The widest thing this unit is ever handed: the whole span a four-digit year admits,
    /// sampled on a prime stride so the samples fall on every phase of the week, the leap
    /// cycle and the era.
    #[test]
    fn the_representable_span_round_trips_end_to_end() {
        for days in (FIRST_DAY..=LAST_DAY).step_by(499) {
            let date = CivilDate::from_days_from_epoch(days).unwrap();
            assert_eq!(date.days_from_epoch(), Some(days));
        }
    }

    /// Contiguous runs across the two century years that disagree — 1900 is common, 2000 is
    /// a leap year — and across both ends, where an off-by-one in the era correction shows
    /// up as a gap or a repeat.
    #[test]
    fn a_contiguous_run_across_both_century_rules_has_no_gap() {
        let windows = [
            (day(1899, 12, 1), day(1901, 1, 31)),
            (day(1999, 12, 1), day(2001, 1, 31)),
            (day(0, 1, 1), day(1, 3, 1)),
            (day(9998, 12, 1), day(9999, 12, 31)),
        ];
        for (start, end) in windows {
            let first = start.days_from_epoch().unwrap();
            let last = end.days_from_epoch().unwrap();
            for days in first..=last {
                let date = CivilDate::from_days_from_epoch(days).unwrap();
                assert_eq!(date.days_from_epoch(), Some(days));
            }
        }
    }

    /// One day past either end is not a date, however the caller arrives there.
    #[test]
    fn a_day_past_either_end_of_the_span_is_not_a_date() {
        let before = FIRST_DAY.checked_sub(1).unwrap();
        let after = LAST_DAY.checked_add(1).unwrap();
        assert_eq!(CivilDate::from_days_from_epoch(before), None);
        assert_eq!(CivilDate::from_days_from_epoch(after), None);
        assert_eq!(CivilDate::from_days_from_epoch(i64::MIN), None);
        assert_eq!(CivilDate::from_days_from_epoch(i64::MAX), None);
        assert_eq!(day(0, 1, 1).checked_add_days(-1), None);
        assert_eq!(day(9999, 12, 31).checked_add_days(1), None);
        assert_eq!(day(2026, 8, 10).checked_add_days(i64::MAX), None);
        assert_eq!(day(2026, 8, 10).checked_add_days(i64::MIN), None);
    }

    /// The empty span: adding nothing moves nothing, at both ends and in all three shapes.
    #[test]
    fn adding_nothing_moves_nothing() {
        for date in [day(0, 1, 1), day(1970, 1, 1), day(9999, 12, 31)] {
            assert_eq!(date.checked_add_days(0), Some(date));
            assert_eq!(date.add_months(0), MonthAddOutcome::Exact(date));
        }
        let noon = stamp(day(2026, 8, 10), 12, 0, 0);
        assert_eq!(noon.checked_add_duration(Duration::ZERO), Some(noon));
    }

    /// Adding days walks the calendar one day at a time across the joints.
    #[test]
    fn adding_days_walks_across_month_and_year_joints() {
        let cases = [
            (day(2024, 2, 28), 1, day(2024, 2, 29)),
            (day(2024, 2, 28), 2, day(2024, 3, 1)),
            (day(1900, 2, 28), 1, day(1900, 3, 1)),
            (day(2026, 12, 31), 1, day(2027, 1, 1)),
            (day(2027, 1, 1), -1, day(2026, 12, 31)),
        ];
        for (start, days, expected) in cases {
            assert_eq!(start.checked_add_days(days), Some(expected));
        }
    }

    /// Weekdays advance one at a time, checked over a run that starts on a Monday so the
    /// expectation is the cycle itself rather than a restatement of the formula.
    #[test]
    fn weekdays_advance_in_order_from_a_known_monday() {
        let start = day(2026, 8, 10).days_from_epoch().unwrap();
        let end = day(2027, 8, 10).days_from_epoch().unwrap();
        for (step, days) in (start..=end).enumerate() {
            let date = CivilDate::from_days_from_epoch(days).unwrap();
            let expected = Weekday::ALL[step.rem_euclid(7)];
            assert_eq!(date.weekday(), Some(expected));
        }
    }

    /// Weekdays at the ends of the span and at the epoch, which no cycle test can anchor.
    #[test]
    fn the_weekday_is_known_at_both_ends_and_at_the_epoch() {
        let cases = [
            (day(1970, 1, 1), Weekday::Thursday),
            (day(1969, 12, 31), Weekday::Wednesday),
            (day(2000, 1, 1), Weekday::Saturday),
            (day(2024, 2, 29), Weekday::Thursday),
            (day(0, 1, 1), Weekday::Saturday),
            (day(9999, 12, 31), Weekday::Friday),
        ];
        for (date, expected) in cases {
            assert_eq!(date.weekday(), Some(expected));
        }
    }

    /// A month step onto a day the target month has.
    #[test]
    fn a_month_step_onto_a_day_that_exists_is_exact() {
        let cases = [
            (day(2026, 8, 10), 1, day(2026, 9, 10)),
            (day(2026, 8, 10), -1, day(2026, 7, 10)),
            (day(2026, 8, 10), 12, day(2027, 8, 10)),
            (day(2026, 8, 10), -12, day(2025, 8, 10)),
            (day(2026, 1, 31), 2, day(2026, 3, 31)),
            (day(0, 1, 29), 1, day(0, 2, 29)),
            (day(2026, 12, 31), 1, day(2027, 1, 31)),
            (day(9999, 11, 30), 1, day(9999, 12, 30)),
        ];
        for (start, count, expected) in cases {
            assert_eq!(start.add_months(count), MonthAddOutcome::Exact(expected));
        }
    }

    /// The reported non-fatal outcome, which is this unit's counterpart of a diagnostic: the
    /// answer arrives with the missing day attached, so a caller obeying RFC 5545 section
    /// 3.3.10 can drop the instance and still say why, and one that took the date instead
    /// did so in the open.
    #[test]
    fn a_month_step_onto_a_day_that_does_not_exist_reports_the_day_it_wanted() {
        let cases = [
            (day(2026, 1, 31), 1, day(2026, 2, 28), 31),
            (day(2024, 1, 31), 1, day(2024, 2, 29), 31),
            (day(2026, 3, 31), -1, day(2026, 2, 28), 31),
            (day(2026, 5, 31), 1, day(2026, 6, 30), 31),
            (day(1900, 1, 30), 1, day(1900, 2, 28), 30),
        ];
        for (start, count, date, requested_day) in cases {
            let clamped = MonthAddOutcome::Clamped {
                date,
                requested_day,
            };
            assert_eq!(start.add_months(count), clamped);
            assert_eq!(start.add_months(count).date(), Some(date));
        }
    }

    /// A year the format cannot write is the other failure, and it is not the same one.
    #[test]
    fn a_month_step_out_of_the_writable_years_overflows() {
        let cases = [
            (day(9999, 12, 1), 1),
            (day(9999, 1, 1), 12),
            (day(0, 1, 1), -1),
            (day(0, 12, 1), -12),
            (day(2026, 8, 10), i32::MAX),
            (day(2026, 8, 10), i32::MIN),
        ];
        for (start, count) in cases {
            assert_eq!(start.add_months(count), MonthAddOutcome::Overflow);
            assert_eq!(start.add_months(count).date(), None);
        }
    }

    /// Twelve single steps and one step of twelve land together, which is the property a
    /// mixed-radix month index gets wrong first. No start is past the 28th, so no step
    /// clamps and the walk stays well defined all the way round.
    #[test]
    fn twelve_single_month_steps_agree_with_one_step_of_twelve() {
        for start in [day(2026, 8, 10), day(1900, 3, 1), day(2024, 1, 28)] {
            let mut walked = start;
            for _ in 0..12 {
                let outcome = walked.add_months(1);
                assert!(matches!(outcome, MonthAddOutcome::Exact(_)));
                walked = outcome.date().unwrap();
            }
            assert_eq!(start.add_months(12), MonthAddOutcome::Exact(walked));
        }
    }

    /// The conversion against an offset, in both directions, including the sign convention
    /// that gets reversed most often.
    #[test]
    fn a_local_time_names_an_instant_once_the_offset_is_known() {
        let cases: [(CivilDateTime, i32, i64); 5] = [
            (stamp(day(1970, 1, 1), 0, 0, 0), 0, 0),
            (stamp(day(2026, 8, 10), 12, 0, 0), 0, 1_786_363_200),
            (stamp(day(2026, 8, 10), 12, 0, 0), 32_400, 1_786_330_800),
            (stamp(day(2026, 8, 10), 12, 0, 0), -18_000, 1_786_381_200),
            (stamp(day(1969, 12, 31), 23, 59, 59), 0, -1),
        ];
        for (local, seconds, unix) in cases {
            let offset = UtcOffset::from_seconds(seconds).unwrap();
            let instant = Instant::from_unix_seconds(unix);
            assert_eq!(local.at_offset(offset), Some(instant));
            let back = CivilDateTime::from_instant(instant, offset);
            assert_eq!(back, Some(local));
        }
    }

    /// The widest offsets the format admits, at the ends of the span, where the conversion
    /// either stays inside the writable years or says that it did not.
    #[test]
    fn the_widest_offsets_are_carried_or_refused_and_never_wrapped() {
        let ahead = UtcOffset::from_seconds(86_399).unwrap();
        let behind = UtcOffset::from_seconds(-86_399).unwrap();
        let last_local = stamp(day(9999, 12, 31), 23, 59, 59);
        let first_local = stamp(day(0, 1, 1), 0, 0, 0);
        assert!(last_local.at_offset(ahead).is_some());
        assert!(first_local.at_offset(behind).is_some());
        let end = last_local.at_offset(behind).unwrap();
        let past_end = end.checked_add_seconds(1).unwrap();
        assert_eq!(CivilDateTime::from_instant(past_end, behind), None);
        let start = first_local.at_offset(ahead).unwrap();
        let before_start = start.checked_add_seconds(-1).unwrap();
        assert_eq!(CivilDateTime::from_instant(before_start, ahead), None);
        let far = Instant::from_unix_seconds(i64::MAX);
        assert_eq!(CivilDateTime::from_instant(far, ahead), None);
    }

    /// A leap second has no instant of its own, so the arithmetic uses the second before it
    /// and leaves the date alone. What the producer wrote stays in the preserved text, which
    /// is where the round trip reads it from.
    #[test]
    fn a_leap_second_folds_onto_the_second_before_it_and_keeps_its_date() {
        let utc = UtcOffset::UTC;
        let leap = stamp(day(2026, 6, 30), 23, 59, 60);
        let ordinary = stamp(day(2026, 6, 30), 23, 59, 59);
        assert_eq!(leap.at_offset(utc), ordinary.at_offset(utc));
        let span = Duration::ZERO;
        assert_eq!(leap.checked_add_duration(span), Some(ordinary));
        let instant = leap.at_offset(utc).unwrap();
        let back = CivilDateTime::from_instant(instant, utc);
        assert_eq!(back, Some(ordinary));
    }

    /// A span moves the clock, and crossing midnight in either direction moves the date with
    /// it rather than rounding towards midnight.
    #[test]
    fn a_span_moves_the_clock_and_borrows_across_midnight() {
        let start = stamp(day(2026, 8, 10), 12, 0, 0);
        let cases: [(i64, i64, CivilDateTime); 5] = [
            (0, 0, stamp(day(2026, 8, 10), 12, 0, 0)),
            (1, 0, stamp(day(2026, 8, 11), 12, 0, 0)),
            (-1, 0, stamp(day(2026, 8, 9), 12, 0, 0)),
            (0, 43_200, stamp(day(2026, 8, 11), 0, 0, 0)),
            (0, -43_201, stamp(day(2026, 8, 9), 23, 59, 59)),
        ];
        for (days, seconds, expected) in cases {
            let span = Duration::new(days, seconds);
            assert_eq!(start.checked_add_duration(span), Some(expected));
        }
    }

    /// A span of whole days steps onto a leap day rather than over it, because the days are
    /// counted on the calendar and not converted to seconds first.
    #[test]
    fn a_span_of_days_lands_on_a_leap_day_rather_than_skipping_it() {
        let eve = stamp(day(2024, 2, 28), 1, 0, 0);
        let leap_day = stamp(day(2024, 2, 29), 1, 0, 0);
        let march = stamp(day(2024, 3, 1), 1, 0, 0);
        let one_day = Duration::new(1, 0);
        let two_days = Duration::new(2, 0);
        assert_eq!(eve.checked_add_duration(one_day), Some(leap_day));
        assert_eq!(eve.checked_add_duration(two_days), Some(march));
    }

    /// A span that leaves the writable years is refused rather than wrapped, whichever of
    /// the two fields carries it out.
    #[test]
    fn a_span_out_of_the_writable_years_is_refused() {
        let earliest = stamp(day(0, 1, 1), 0, 0, 0);
        let latest = stamp(day(9999, 12, 31), 23, 59, 59);
        let one_second_back = Duration::new(0, -1);
        let one_second_on = Duration::new(0, 1);
        let huge = Duration::new(i64::MAX, i64::MAX);
        let tiny = Duration::new(i64::MIN, i64::MIN);
        assert_eq!(earliest.checked_add_duration(one_second_back), None);
        assert_eq!(latest.checked_add_duration(one_second_on), None);
        assert_eq!(latest.checked_add_duration(huge), None);
        assert_eq!(earliest.checked_add_duration(tiny), None);
    }
}
