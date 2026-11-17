// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bound a complete counted rule before splitting its dates into month patterns.

use core::num::NonZeroU32;

use crate::internal::core::CivilDate;
use crate::internal::tz::annual_dates::YearDateMasks;
use crate::internal::tz::year_intervals::YearInterval;

const CYCLE_PERIODS: u32 = 400;
const LAST_CIVIL_YEAR: u16 = 9999;

#[derive(Clone, Copy, Debug)]
pub(super) enum CountEnd {
    Within(CivilDate),
    BeyondCivilRange,
}

pub(super) fn counted_end(
    dates: &YearDateMasks,
    start: CivilDate,
    interval: YearInterval,
    count: NonZeroU32,
) -> Option<CountEnd> {
    let mut remaining = count.get().checked_sub(1)?;
    if remaining == 0 {
        return Some(CountEnd::Within(start));
    }
    let prefix = dates.count_through(start)?;
    let first_year = dates.year_count(start.year())?.checked_sub(prefix)?;
    if remaining <= first_year {
        return dates
            .date_at(start.year(), prefix.checked_add(remaining)?)
            .map(CountEnd::Within);
    }
    remaining = remaining.checked_sub(first_year)?;
    let cycle_total = cycle_count(dates, start.year(), interval)?;
    if cycle_total == 0 {
        return Some(CountEnd::Within(start));
    }
    let rank = remaining.checked_sub(1)?;
    let cycles = rank.checked_div(cycle_total)?;
    let mut position = rank.checked_rem(cycle_total)?.checked_add(1)?;
    let mut period = u64::from(cycles)
        .checked_mul(u64::from(CYCLE_PERIODS))?
        .checked_add(1)?;
    let final_period = u64::from(interval.periods_through(start.year(), LAST_CIVIL_YEAR)?);
    for _ in 0..CYCLE_PERIODS {
        // Reject an out-of-range index before multiplying it by the interval.
        if period > final_period {
            return Some(CountEnd::BeyondCivilRange);
        }
        let year = interval.year_at(start.year(), u32::try_from(period).ok()?)?;
        let count = dates.year_count(year)?;
        if position <= count {
            return dates.date_at(year, position).map(CountEnd::Within);
        }
        position = position.checked_sub(count)?;
        period = period.checked_add(1)?;
    }
    None
}

fn cycle_count(dates: &YearDateMasks, first: u16, interval: YearInterval) -> Option<u32> {
    let mut residue = u32::from(first).checked_rem(CYCLE_PERIODS)?;
    let step = interval.step().get().checked_rem(CYCLE_PERIODS)?;
    let mut total = 0_u32;
    for _ in 0..CYCLE_PERIODS {
        residue = residue.checked_add(step)?.checked_rem(CYCLE_PERIODS)?;
        let year = u16::try_from(2000_u32.checked_add(residue)?).ok()?;
        total = total.checked_add(dates.year_count(year)?)?;
    }
    Some(total)
}
