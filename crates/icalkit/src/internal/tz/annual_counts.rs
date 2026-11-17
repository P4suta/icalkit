// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bound a complete counted rule before splitting its dates into month patterns.

use core::num::NonZeroU32;

use crate::internal::core::CivilDate;
use crate::internal::tz::year_intervals::YearInterval;

const CYCLE_PERIODS: u32 = 400;
const MAX_RANK_PROBES: usize = 402;

pub(super) trait AnnualCandidates {
    fn first_year(&self, start: CivilDate) -> Option<i32>;
    fn last_year(&self) -> i32;
    fn periodic_range(&self) -> (i32, i32);
    fn year_count(&self, year: i32) -> Option<u32>;
    fn count_through(&self, year: i32, date: CivilDate) -> Option<u32>;
    fn date_at(&self, year: i32, position: u32) -> Option<CivilDate>;
}

#[derive(Clone, Copy, Debug)]
pub(super) enum CountEnd {
    Within(CivilDate),
    BeyondCivilRange,
}

pub(super) fn counted_end(
    dates: &impl AnnualCandidates,
    start: CivilDate,
    interval: YearInterval,
    count: NonZeroU32,
) -> Option<CountEnd> {
    let mut remaining = count.get().checked_sub(1)?;
    if remaining == 0 {
        return Some(CountEnd::Within(start));
    }
    let mut first = dates.first_year(start)?;
    let prefix = dates.count_through(first, start)?;
    let first_count = dates.year_count(first)?.checked_sub(prefix)?;
    if remaining <= first_count {
        return dates
            .date_at(first, prefix.checked_add(remaining)?)
            .map(CountEnd::Within);
    }
    remaining = remaining.checked_sub(first_count)?;
    let (periodic_first, periodic_last) = dates.periodic_range();
    for _ in 0..2 {
        if interval.periods_through(first, dates.last_year())? == 0 {
            break;
        }
        let next = interval.year_at(first, 1)?;
        if next >= periodic_first {
            break;
        }
        let count = dates.year_count(next)?;
        if remaining <= count {
            return dates.date_at(next, remaining).map(CountEnd::Within);
        }
        remaining = remaining.checked_sub(count)?;
        first = next;
    }
    let cycle_total = cycle_count(dates, first, interval)?;
    if cycle_total == 0 {
        return Some(CountEnd::Within(start));
    }
    let rank = remaining.checked_sub(1)?;
    let available = interval
        .periods_through(first, periodic_last)
        .unwrap_or(0)
        .checked_div(CYCLE_PERIODS)?;
    let cycles = rank.checked_div(cycle_total)?.min(available);
    let mut position = remaining.checked_sub(cycles.checked_mul(cycle_total)?)?;
    let mut period = u64::from(cycles)
        .checked_mul(u64::from(CYCLE_PERIODS))?
        .checked_add(1)?;
    let final_period = u64::from(interval.periods_through(first, dates.last_year())?);
    for _ in 0..MAX_RANK_PROBES {
        // Reject an out-of-range index before multiplying it by the interval.
        if period > final_period {
            return Some(CountEnd::BeyondCivilRange);
        }
        let year = interval.year_at(first, u32::try_from(period).ok()?)?;
        let count = dates.year_count(year)?;
        if position <= count {
            return dates.date_at(year, position).map(CountEnd::Within);
        }
        position = position.checked_sub(count)?;
        period = period.checked_add(1)?;
    }
    None
}

fn cycle_count(dates: &impl AnnualCandidates, first: i32, interval: YearInterval) -> Option<u32> {
    let mut residue = u32::try_from(first.rem_euclid(i32::try_from(CYCLE_PERIODS).ok()?)).ok()?;
    let step = interval.step().get().checked_rem(CYCLE_PERIODS)?;
    let mut total = 0_u32;
    for _ in 0..CYCLE_PERIODS {
        residue = residue.checked_add(step)?.checked_rem(CYCLE_PERIODS)?;
        let year = i32::try_from(2000_u32.checked_add(residue)?).ok()?;
        total = total.checked_add(dates.year_count(year)?)?;
    }
    Some(total)
}
