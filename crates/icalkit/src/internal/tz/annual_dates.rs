// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Compile Gregorian date selectors once for each leap status and January weekday.

use alloc::boxed::Box;
use core::num::{NonZeroU8, NonZeroU16};

use crate::internal::core::{CivilDate, CivilTime};
use crate::internal::tz::annual_positions::AnnualPositions;
use crate::internal::tz::model::{RuleDay, YearlyRule};
use crate::internal::tz::month_days::MonthDaySet;
use crate::internal::tz::weekdays::{WeekdayScope, WeekdaySet};
use crate::internal::tz::year_intervals::YearInterval;

const YEAR_LAYOUTS: [u16; 14] = [
    2001, 2002, 2003, 2009, 2010, 2005, 2006, 2024, 2008, 2020, 2004, 2016, 2000, 2012,
];

#[derive(Clone, Copy, Debug)]
pub(super) enum DateSelectors {
    YearDays {
        days: AnnualPositions,
        month_days: Option<MonthDaySet>,
        weekdays: Option<WeekdaySet>,
    },
    MonthDays {
        days: MonthDaySet,
        weekdays: Option<WeekdaySet>,
    },
    Weekdays(WeekdaySet),
}

impl DateSelectors {
    fn in_month(self, year: u16, month: u8, scope: WeekdayScope) -> Option<u32> {
        match self {
            Self::YearDays {
                days,
                month_days,
                weekdays,
            } => {
                let mut dates = days.mask_in_month(year, month)?;
                if let Some(month_days) = month_days {
                    dates &= month_days.for_length(CivilDate::days_in_month(year, month)?)?;
                }
                if let Some(weekdays) = weekdays {
                    dates &= weekdays.in_month(year, month, scope)?;
                }
                Some(dates)
            },
            Self::MonthDays { days, weekdays } => {
                let dates = days.for_length(CivilDate::days_in_month(year, month)?)?;
                match weekdays {
                    None => Some(dates),
                    Some(weekdays) => Some(dates & weekdays.in_month(year, month, scope)?),
                }
            },
            Self::Weekdays(weekdays) => weekdays.in_month(year, month, scope),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MonthSet(NonZeroU16);

impl MonthSet {
    pub(super) fn singleton(month: u8) -> Option<Self> {
        NonZeroU16::new(month_bit(month)?).map(Self)
    }

    pub(super) fn with_month(self, month: u8) -> Option<Self> {
        NonZeroU16::new(self.0.get() | month_bit(month)?).map(Self)
    }

    pub(super) fn contains(self, month: u8) -> bool {
        month_bit(month).is_some_and(|bit| self.0.get() & bit != 0)
    }
}

fn month_bit(month: u8) -> Option<u16> {
    if !(1..=12).contains(&month) {
        return None;
    }
    1_u16.checked_shl(u32::from(month.checked_sub(1)?))
}

#[derive(Debug)]
pub(super) struct YearDateMasks {
    layouts: [[u32; 12]; 14],
}

impl YearDateMasks {
    pub(super) fn compile(
        months: Option<MonthSet>,
        selectors: DateSelectors,
        scope: WeekdayScope,
        positions: Option<AnnualPositions>,
    ) -> Option<Self> {
        let mut layouts = [[0; 12]; 14];
        for (index, year) in YEAR_LAYOUTS.into_iter().enumerate() {
            if year_layout(year)? != index {
                return None;
            }
            let dates = layouts.get_mut(index)?;
            for month in 1..=12 {
                if months.is_some_and(|selected| !selected.contains(month)) {
                    continue;
                }
                *dates.get_mut(usize::from(month.checked_sub(1)?))? =
                    selectors.in_month(year, month, scope)?;
            }
            if let Some(positions) = positions {
                select_positions(dates, positions)?;
            }
        }
        Some(Self { layouts })
    }

    pub(super) fn year_count(&self, year: u16) -> Option<u32> {
        self.layouts
            .get(year_layout(year)?)?
            .iter()
            .try_fold(0_u32, |total, mask| total.checked_add(mask.count_ones()))
    }

    pub(super) fn count_through(&self, date: CivilDate) -> Option<u32> {
        let dates = self.layouts.get(year_layout(date.year())?)?;
        let mut total = 0_u32;
        for month in 1..=date.month() {
            let mut mask = *dates.get(usize::from(month.checked_sub(1)?))?;
            if month == date.month() {
                mask &= through_day(date.day())?;
            }
            total = total.checked_add(mask.count_ones())?;
        }
        Some(total)
    }

    pub(super) fn date_at(&self, year: u16, mut position: u32) -> Option<CivilDate> {
        if position == 0 {
            return None;
        }
        for (index, mask) in self.layouts.get(year_layout(year)?)?.iter().enumerate() {
            let count = mask.count_ones();
            if position > count {
                position = position.checked_sub(count)?;
                continue;
            }
            let mut remaining = *mask;
            for _ in 1..position {
                remaining &= remaining.checked_sub(1)?;
            }
            let day = u8::try_from(remaining.trailing_zeros().checked_add(1)?).ok()?;
            let month = u8::try_from(index.checked_add(1)?).ok()?;
            return CivilDate::from_ymd(year, month, day);
        }
        None
    }

    pub(super) fn retain_after(
        &mut self,
        first: CivilDate,
        last: CivilDate,
        interval: YearInterval,
    ) -> Option<()> {
        if last < first {
            return None;
        }
        let final_period = interval.periods_through(first.year(), last.year())?;
        let mut kept = Self {
            layouts: [[0; 12]; 14],
        };
        // Four hundred aligned periods cover every reachable Gregorian layout.
        for period in 0..=final_period.min(400) {
            kept.include_year(self, interval.year_at(first.year(), period)?, first, last)?;
        }
        if final_period > 400 {
            kept.include_year(
                self,
                interval.year_at(first.year(), final_period)?,
                first,
                last,
            )?;
        }
        *self = kept;
        Some(())
    }

    fn include_year(
        &mut self,
        source: &Self,
        year: u16,
        first: CivilDate,
        last: CivilDate,
    ) -> Option<()> {
        let layout = year_layout(year)?;
        let dates = source.layouts.get(layout)?;
        let kept = self.layouts.get_mut(layout)?;
        for month in 1_u8..=12 {
            let column = usize::from(month.checked_sub(1)?);
            let mut mask = *dates.get(column)?;
            if year == first.year() && month <= first.month() {
                mask &= if month == first.month() {
                    !through_day(first.day())?
                } else {
                    0
                };
            }
            if year == last.year() && month >= last.month() {
                mask &= if month == last.month() {
                    through_day(last.day())?
                } else {
                    0
                };
            }
            *kept.get_mut(column)? |= mask;
        }
        Some(())
    }

    pub(super) fn into_rules(
        self: Box<Self>,
        at: CivilTime,
        through: Option<CivilDate>,
    ) -> impl Iterator<Item = YearlyRule> {
        (1..=12).flat_map(move |month| {
            let masks = self.month_masks(month);
            let count = masks.map_or(0, MonthDateMasks::max_positions);
            (1..=count).filter_map(move |position| {
                YearlyRule::new(
                    month,
                    RuleDay::SelectedDates(masks?.select(position)?),
                    at,
                    through,
                )
            })
        })
    }

    fn month_masks(&self, month: u8) -> Option<MonthDateMasks> {
        if !(1..=12).contains(&month) {
            return None;
        }
        let column = usize::from(month.checked_sub(1)?);
        let mut masks = [0; 14];
        for (index, dates) in self.layouts.iter().enumerate() {
            *masks.get_mut(index)? = *dates.get(column)?;
        }
        Some(MonthDateMasks {
            month: NonZeroU8::new(month)?,
            masks,
        })
    }
}

fn through_day(day: u8) -> Option<u32> {
    1_u32.checked_shl(u32::from(day))?.checked_sub(1)
}

fn select_positions(dates: &mut [u32; 12], positions: AnnualPositions) -> Option<()> {
    let extent = dates.iter().try_fold(0_u16, |total, mask| {
        total.checked_add(u16::try_from(mask.count_ones()).ok()?)
    })?;
    let mut position = 0_u16;
    for mask in dates {
        let mut remaining = *mask;
        let mut selected = 0;
        while remaining != 0 {
            position = position.checked_add(1)?;
            if positions.matches(position, extent) {
                selected |= 1_u32.checked_shl(remaining.trailing_zeros())?;
            }
            remaining &= remaining.checked_sub(1)?;
        }
        *mask = selected;
    }
    Some(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct MonthDateMasks {
    month: NonZeroU8,
    masks: [u32; 14],
}

impl MonthDateMasks {
    pub(super) fn max_positions(self) -> u32 {
        self.masks
            .into_iter()
            .map(u32::count_ones)
            .fold(0, u32::max)
    }

    pub(super) fn select(self, position: u32) -> Option<AnnualDateSelection> {
        if position > self.max_positions() {
            return None;
        }
        Some(AnnualDateSelection {
            dates: self,
            position: NonZeroU8::new(u8::try_from(position).ok()?)?,
        })
    }
}

/// One distinct date in a compiled month across Gregorian year layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnnualDateSelection {
    dates: MonthDateMasks,
    position: NonZeroU8,
}

impl AnnualDateSelection {
    pub(super) const fn month(self) -> u8 {
        self.dates.month.get()
    }

    pub(super) fn in_year(self, year: u16) -> Option<CivilDate> {
        let mut candidates = *self.dates.masks.get(year_layout(year)?)?;
        for _ in 1..self.position.get() {
            candidates &= candidates.checked_sub(1)?;
        }
        if candidates == 0 {
            return None;
        }
        let day = u8::try_from(candidates.trailing_zeros().checked_add(1)?).ok()?;
        CivilDate::from_ymd(year, self.month(), day)
    }
}

fn year_layout(year: u16) -> Option<usize> {
    let weekday = usize::from(CivilDate::from_ymd(year, 1, 1)?.weekday()?.index());
    let leap = CivilDate::days_in_month(year, 2)? == 29;
    weekday.checked_add(if leap { 7 } else { 0 })
}
