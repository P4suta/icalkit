// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Compile complete numbered-week periods before projecting their calendar dates.

use alloc::boxed::Box;
use core::num::NonZeroU128;

use crate::internal::core::{CivilDate, CivilTime, Weekday};
use crate::internal::tz::annual_counts::AnnualCandidates;
use crate::internal::tz::annual_dates::{AnnualDateSelection, YearDateMasks, select_positions};
use crate::internal::tz::annual_positions::AnnualPositions;
use crate::internal::tz::model::{RuleDay, YearlyRule};
use crate::internal::tz::year_intervals::YearInterval;
use crate::internal::tz::year_layouts::{
    BOUNDARY_YEARS, NORMAL_WEEK_LAYOUTS, WEEK_LAYOUTS, YearOffset, week_layout, week_span,
    week_year,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct WeekNumbers(NonZeroU128);

impl WeekNumbers {
    pub(super) fn singleton(number: i16) -> Option<Self> {
        NonZeroU128::new(number_bit(number)?).map(Self)
    }

    pub(super) fn with_number(self, number: i16) -> Option<Self> {
        NonZeroU128::new(self.0.get() | number_bit(number)?).map(Self)
    }

    fn matches(self, number: u8, extent: u8) -> bool {
        if number == 0 || number > extent || extent > 53 {
            return false;
        }
        let forward = u32::from(number)
            .checked_sub(1)
            .and_then(|index| 1_u128.checked_shl(index));
        let reverse = 53_u32
            .checked_add(u32::from(extent))
            .and_then(|end| end.checked_sub(u32::from(number)))
            .and_then(|index| 1_u128.checked_shl(index));
        forward
            .zip(reverse)
            .is_some_and(|(forward, reverse)| self.0.get() & (forward | reverse) != 0)
    }
}

fn number_bit(number: i16) -> Option<u128> {
    let magnitude = number.unsigned_abs();
    if !(1..=53).contains(&magnitude) {
        return None;
    }
    let index = u32::from(magnitude.checked_sub(1)?).checked_add(if number.is_negative() {
        53
    } else {
        0
    })?;
    1_u128.checked_shl(index)
}

#[derive(Clone, Debug)]
pub(super) struct WeekYearMasks {
    layouts: [[u32; 36]; WEEK_LAYOUTS],
    start: Weekday,
}

impl WeekYearMasks {
    pub(super) fn compile(
        filters: &YearDateMasks,
        weeks: WeekNumbers,
        start: Weekday,
        positions: Option<AnnualPositions>,
    ) -> Option<Self> {
        let mut layouts = [[0; 36]; WEEK_LAYOUTS];
        let mut seen = [false; NORMAL_WEEK_LAYOUTS];
        for year in 2000..2400 {
            let layout = week_layout(year)?;
            if *seen.get(layout)? {
                continue;
            }
            *layouts.get_mut(layout)? = compile_period(year, filters, weeks, start, positions)?;
            *seen.get_mut(layout)? = true;
        }
        if seen.into_iter().any(|seen| !seen) {
            return None;
        }
        for year in BOUNDARY_YEARS {
            *layouts.get_mut(week_layout(year)?)? =
                compile_period(year, filters, weeks, start, positions)?;
        }
        Some(Self { layouts, start })
    }

    pub(super) fn period_year(&self, date: CivilDate) -> Option<i32> {
        week_year(date, self.start)
    }

    pub(super) fn contains(&self, date: CivilDate) -> Option<bool> {
        let year = self.period_year(date)?;
        let offset = YearOffset::between(year, date.year())?;
        let column = column(offset, date.month())?;
        let mask = self.layouts.get(week_layout(year)?)?.get(column)?;
        Some(mask & 1_u32.checked_shl(u32::from(date.day().checked_sub(1)?))? != 0)
    }

    pub(super) fn retain_after(
        &mut self,
        first: CivilDate,
        last: CivilDate,
        interval: YearInterval,
    ) -> Option<()> {
        self.retain_between(first, last, interval, false)
    }

    pub(super) fn select(mut self, positions: AnnualPositions) -> Option<Self> {
        for dates in &mut self.layouts {
            select_positions(dates, positions)?;
        }
        Some(self)
    }

    pub(super) fn retain_between(
        &mut self,
        first: CivilDate,
        last: CivilDate,
        interval: YearInterval,
        include_first: bool,
    ) -> Option<()> {
        let mut kept = Self {
            layouts: [[0; 36]; WEEK_LAYOUTS],
            start: self.start,
        };
        if last < first || (last == first && !include_first) {
            *self = kept;
            return Some(());
        }
        let origin = self.period_year(first)?;
        let end = self.period_year(last)?;
        let final_period = interval.periods_through(origin, end)?;
        for period in 0..=final_period.min(402) {
            kept.include_period(
                self,
                interval.year_at(origin, period)?,
                first,
                last,
                include_first,
            )?;
        }
        if final_period > 402 {
            kept.include_period(
                self,
                interval.year_at(origin, final_period)?,
                first,
                last,
                include_first,
            )?;
        }
        for year in BOUNDARY_YEARS {
            if year >= origin && year <= end && interval.contains(origin, year) {
                kept.include_period(self, year, first, last, include_first)?;
            }
        }
        *self = kept;
        Some(())
    }

    fn include_period(
        &mut self,
        source: &Self,
        year: i32,
        first: CivilDate,
        last: CivilDate,
        include_first: bool,
    ) -> Option<()> {
        let layout = week_layout(year)?;
        let row = source.layouts.get(layout)?;
        let kept = self.layouts.get_mut(layout)?;
        for offset in YearOffset::ALL {
            let lower = offset.period_year(first.year())?;
            let upper = offset.period_year(last.year())?;
            for month in 1_u8..=12 {
                let column = column(offset, month)?;
                let mut mask = *row.get(column)?;
                if year < lower || year > upper {
                    mask = 0;
                }
                if year == lower && month <= first.month() {
                    mask &= if month == first.month() {
                        !through_day(first.day().checked_sub(u8::from(include_first))?)?
                    } else {
                        0
                    };
                }
                if year == upper && month >= last.month() {
                    mask &= if month == last.month() {
                        through_day(last.day())?
                    } else {
                        0
                    };
                }
                *kept.get_mut(column)? |= mask;
            }
        }
        Some(())
    }

    pub(super) fn into_rules(
        self: Box<Self>,
        at: CivilTime,
        through: Option<CivilDate>,
    ) -> impl Iterator<Item = YearlyRule> {
        YearOffset::ALL
            .into_iter()
            .flat_map(|offset| (1_u8..=12).map(move |month| (offset, month)))
            .flat_map(move |(offset, month)| {
                let masks = self.month_masks(offset, month);
                let count = masks.map_or(0, |masks| {
                    masks.into_iter().map(u32::count_ones).max().unwrap_or(0)
                });
                let start = self.start;
                (1..=count).filter_map(move |position| {
                    let dates = AnnualDateSelection::for_week_dates(
                        month, offset, start, position, masks?,
                    )?;
                    YearlyRule::new(month, RuleDay::SelectedDates(dates), at, through)
                })
            })
    }

    fn month_masks(&self, offset: YearOffset, month: u8) -> Option<[u32; WEEK_LAYOUTS]> {
        let column = column(offset, month)?;
        let mut masks = [0; WEEK_LAYOUTS];
        for (index, row) in self.layouts.iter().enumerate() {
            *masks.get_mut(index)? = *row.get(column)?;
        }
        Some(masks)
    }
}

impl AnnualCandidates for WeekYearMasks {
    type Point = CivilDate;
    fn first_year(&self, start: CivilDate) -> Option<i32> {
        self.period_year(start)
    }
    fn last_year(&self) -> i32 {
        10000
    }
    fn periodic_range(&self) -> (i32, i32) {
        (1, 9998)
    }

    fn year_count(&self, year: i32) -> Option<u64> {
        self.layouts
            .get(week_layout(year)?)?
            .iter()
            .try_fold(0_u64, |total, mask| {
                total.checked_add(u64::from(mask.count_ones()))
            })
    }

    fn count_through(&self, year: i32, date: CivilDate) -> Option<u64> {
        let row = self.layouts.get(week_layout(year)?)?;
        let mut total = 0_u64;
        for offset in YearOffset::ALL {
            let boundary = offset.period_year(date.year())?;
            for month in 1_u8..=12 {
                let mut mask = *row.get(column(offset, month)?)?;
                if year > boundary || (year == boundary && month > date.month()) {
                    mask = 0;
                }
                if year == boundary && month == date.month() {
                    mask &= through_day(date.day())?;
                }
                total = total.checked_add(u64::from(mask.count_ones()))?;
            }
        }
        Some(total)
    }

    fn point_at(&self, year: i32, mut position: u64) -> Option<CivilDate> {
        if position == 0 {
            return None;
        }
        for offset in YearOffset::ALL {
            for month in 1_u8..=12 {
                let mut mask = *self
                    .layouts
                    .get(week_layout(year)?)?
                    .get(column(offset, month)?)?;
                let count = u64::from(mask.count_ones());
                if position > count {
                    position = position.checked_sub(count)?;
                    continue;
                }
                for _ in 1..position {
                    mask &= mask.checked_sub(1)?;
                }
                let day = u8::try_from(mask.trailing_zeros().checked_add(1)?).ok()?;
                return CivilDate::from_ymd(offset.calendar_year(year)?, month, day);
            }
        }
        None
    }
}

fn column(offset: YearOffset, month: u8) -> Option<usize> {
    if !(1..=12).contains(&month) {
        return None;
    }
    offset
        .index()
        .checked_mul(12)?
        .checked_add(usize::from(month.checked_sub(1)?))
}

fn through_day(day: u8) -> Option<u32> {
    1_u32.checked_shl(u32::from(day))?.checked_sub(1)
}

fn compile_period(
    year: i32,
    filters: &YearDateMasks,
    weeks: WeekNumbers,
    start: Weekday,
    positions: Option<AnnualPositions>,
) -> Option<[u32; 36]> {
    let (first, days) = week_span(year, start)?;
    let extent = u8::try_from(days.checked_div(7)?).ok()?;
    let mut dates = [0; 36];
    for index in 0..days {
        let Some(date) = CivilDate::from_days_from_epoch(first.checked_add(i64::from(index))?)
        else {
            continue;
        };
        let number = u8::try_from(index.checked_div(7)?.checked_add(1)?).ok()?;
        if !weeks.matches(number, extent) || !filters.contains(date)? {
            continue;
        }
        let column = column(YearOffset::between(year, date.year())?, date.month())?;
        *dates.get_mut(column)? |= 1_u32.checked_shl(u32::from(date.day().checked_sub(1)?))?;
    }
    if let Some(positions) = positions {
        select_positions(&mut dates, positions)?;
    }
    Some(dates)
}
