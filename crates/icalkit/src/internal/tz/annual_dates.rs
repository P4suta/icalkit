// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Compile Gregorian date selectors once for each leap status and January weekday.

use core::num::NonZeroU8;

use crate::internal::core::CivilDate;
use crate::internal::tz::month_days::MonthDaySet;
use crate::internal::tz::weekdays::{WeekdayScope, WeekdaySet};

const YEAR_LAYOUTS: [u16; 14] = [
    2001, 2002, 2003, 2009, 2010, 2005, 2006, 2024, 2008, 2020, 2004, 2016, 2000, 2012,
];

#[derive(Clone, Copy, Debug)]
pub(super) enum DateSelectors {
    MonthDays {
        days: MonthDaySet,
        weekdays: Option<WeekdaySet>,
    },
    Weekdays(WeekdaySet),
}

impl DateSelectors {
    fn in_month(self, year: u16, month: u8, scope: WeekdayScope) -> Option<u32> {
        match self {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct MonthDateMasks {
    month: NonZeroU8,
    masks: [u32; 14],
}

impl MonthDateMasks {
    pub(super) fn compile(
        month: u8,
        selectors: DateSelectors,
        scope: WeekdayScope,
    ) -> Option<Self> {
        if !(1..=12).contains(&month) {
            return None;
        }
        let mut masks = [0; 14];
        for (index, year) in YEAR_LAYOUTS.into_iter().enumerate() {
            if year_layout(year)? != index {
                return None;
            }
            *masks.get_mut(index)? = selectors.in_month(year, month, scope)?;
        }
        Some(Self {
            month: NonZeroU8::new(month)?,
            masks,
        })
    }

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
