// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resolve signed month-day sets into distinct dates before selecting an annual pattern.

use core::num::{NonZeroU8, NonZeroU64};

use crate::internal::core::{CivilDate, Weekday};
use crate::internal::tz::rules::days_forward;

const WEEKDAY_BITS: u32 = 0x1020_4081;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct MonthDaySet(NonZeroU64);

impl MonthDaySet {
    pub(super) fn singleton(day: i8) -> Option<Self> {
        NonZeroU64::new(day_bit(day)?).map(Self)
    }

    pub(super) fn with_day(self, day: i8) -> Option<Self> {
        NonZeroU64::new(self.0.get() | day_bit(day)?).map(Self)
    }

    pub(super) const fn len(self) -> u32 {
        self.0.get().count_ones()
    }

    pub(super) fn values(self) -> impl Iterator<Item = i8> {
        (-31_i8..=-1)
            .chain(1..=31)
            .filter(move |day| day_bit(*day).is_some_and(|bit| self.0.get() & bit != 0))
    }

    pub(super) fn contiguous_run(self) -> Option<(i8, i8)> {
        let mut values = self.values();
        let first = values.next()?;
        let mut last = first;
        for value in values {
            if Some(value) != last.checked_add(1) {
                return None;
            }
            last = value;
        }
        Some((first, last))
    }

    pub(super) fn max_positions(self, month: u8, weekday: Option<Weekday>) -> Option<u8> {
        let mut count = 0;
        for year in [0, 1] {
            let bits = self.for_length(CivilDate::days_in_month(year, month)?)?;
            if weekday.is_some() {
                for shift in 0..7 {
                    let matching = bits & WEEKDAY_BITS.checked_shl(shift)?;
                    count = count.max(matching.count_ones());
                }
            } else {
                count = count.max(bits.count_ones());
            }
        }
        u8::try_from(count).ok()
    }

    pub(super) fn select(
        self,
        position: u8,
        weekday: Option<Weekday>,
    ) -> Option<MonthDaySelection> {
        if position > 31 {
            return None;
        }
        Some(MonthDaySelection {
            days: self,
            position: NonZeroU8::new(position)?,
            weekday,
        })
    }

    fn for_length(self, length: u8) -> Option<u32> {
        if !(1..=31).contains(&length) {
            return None;
        }
        let shift = 32_u32.checked_sub(u32::from(length))?;
        let positive = u32::try_from(self.0.get() & 0x7fff_ffff).ok()?;
        let negative = u32::try_from(self.0.get().checked_shr(31)?)
            .ok()?
            .reverse_bits()
            .checked_shr(shift)?;
        let valid = u32::MAX.checked_shr(shift)?;
        Some((positive | negative) & valid)
    }
}

/// One rank of a month-day set after an optional weekday restriction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonthDaySelection {
    days: MonthDaySet,
    position: NonZeroU8,
    weekday: Option<Weekday>,
}

impl MonthDaySelection {
    pub(super) fn in_month(self, year: u16, month: u8) -> Option<CivilDate> {
        let mut candidates = self
            .days
            .for_length(CivilDate::days_in_month(year, month)?)?;
        if let Some(weekday) = self.weekday {
            let first = CivilDate::from_ymd(year, month, 1)?.weekday()?;
            let shift = u32::from(days_forward(first, weekday));
            candidates &= WEEKDAY_BITS.checked_shl(shift)?;
        }
        for _ in 1..self.position.get() {
            candidates &= candidates.checked_sub(1)?;
        }
        if candidates == 0 {
            return None;
        }
        let day = u8::try_from(candidates.trailing_zeros().checked_add(1)?).ok()?;
        CivilDate::from_ymd(year, month, day)
    }
}

fn day_bit(day: i8) -> Option<u64> {
    let number = day.unsigned_abs();
    if !(1..=31).contains(&number) {
        return None;
    }
    let offset = if day.is_negative() { 31_u32 } else { 0 };
    let shift = u32::from(number.checked_sub(1)?).checked_add(offset)?;
    1_u64.checked_shl(shift)
}
