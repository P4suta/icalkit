// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Store checked positive and negative month-day positions without duplicates.

use core::num::NonZeroU64;

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

    pub(super) fn for_length(self, length: u8) -> Option<u32> {
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

fn day_bit(day: i8) -> Option<u64> {
    let number = day.unsigned_abs();
    if !(1..=31).contains(&number) {
        return None;
    }
    let offset = if day.is_negative() { 31_u32 } else { 0 };
    let shift = u32::from(number.checked_sub(1)?).checked_add(offset)?;
    1_u64.checked_shl(shift)
}
