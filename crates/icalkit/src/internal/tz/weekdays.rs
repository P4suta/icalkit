// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Normalize weekday terms and resolve their union in the stated ordinal scope.

use crate::internal::core::{CivilDate, Weekday};
use crate::internal::tz::model::NthWeek;
use crate::internal::tz::rules::{days_forward, nth_weekday_between};

const WEEKDAY_BITS: u32 = 0x1020_4081;

#[derive(Clone, Copy, Debug)]
pub(super) enum WeekdayScope {
    Month,
    Year,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WeekdayTerm {
    weekday: Weekday,
    ordinal: Option<NthWeek>,
}

impl WeekdayTerm {
    pub(super) fn new(weekday: Weekday, ordinal: Option<i8>) -> Option<Self> {
        let ordinal = match ordinal {
            None => None,
            Some(number) => Some(NthWeek::from_ordinal(number)?),
        };
        Some(Self { weekday, ordinal })
    }

    pub(super) const fn weekday(self) -> Weekday {
        self.weekday
    }

    pub(super) const fn ordinal(self) -> Option<NthWeek> {
        self.ordinal
    }

    fn bit(self) -> Option<u128> {
        let index = match self.ordinal {
            None => 0,
            Some(week) => {
                let number = week.number();
                if number.is_positive() {
                    u32::try_from(number).ok()?
                } else {
                    53_u32.checked_add(u32::from(number.unsigned_abs()))?
                }
            },
        };
        1_u128.checked_shl(index)
    }

    fn at_bit(weekday: Weekday, index: u32) -> Option<Self> {
        let ordinal = match index {
            0 => None,
            1..=53 => Some(i8::try_from(index).ok()?),
            54..=106 => Some(i8::try_from(index.checked_sub(53)?).ok()?.checked_neg()?),
            _ => return None,
        };
        Self::new(weekday, ordinal)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WeekdaySet([u128; 7]);

impl WeekdaySet {
    pub(super) fn singleton(term: WeekdayTerm) -> Option<Self> {
        Self([0; 7]).with_term(term)
    }

    pub(super) fn with_term(mut self, term: WeekdayTerm) -> Option<Self> {
        let slot = self.0.get_mut(usize::from(term.weekday.index()))?;
        if term.ordinal.is_none() {
            *slot = 1;
        } else if *slot & 1 == 0 {
            *slot |= term.bit()?;
        }
        Some(self)
    }

    pub(super) fn single(self) -> Option<WeekdayTerm> {
        let mut found = None;
        for weekday in Weekday::ALL {
            let bits = *self.0.get(usize::from(weekday.index()))?;
            if bits == 0 {
                continue;
            }
            if found.is_some() || bits.count_ones() != 1 {
                return None;
            }
            found = Some(WeekdayTerm::at_bit(weekday, bits.trailing_zeros())?);
        }
        found
    }

    pub(super) fn in_month(self, year: u16, month: u8, scope: WeekdayScope) -> Option<u32> {
        let length = CivilDate::days_in_month(year, month)?;
        let month_first = CivilDate::from_ymd(year, month, 1)?;
        let month_last = CivilDate::from_ymd(year, month, length)?;
        let (first, last) = match scope {
            WeekdayScope::Month => (month_first, month_last),
            WeekdayScope::Year => (
                CivilDate::from_ymd(year, 1, 1)?,
                CivilDate::from_ymd(year, 12, 31)?,
            ),
        };
        let mut dates = 0;
        for weekday in Weekday::ALL {
            let mut terms = *self.0.get(usize::from(weekday.index()))?;
            while terms != 0 {
                let term = WeekdayTerm::at_bit(weekday, terms.trailing_zeros())?;
                if let Some(week) = term.ordinal {
                    if let Some(date) = nth_weekday_between(first, last, weekday, week)
                        && date.month() == month
                    {
                        dates |= 1_u32.checked_shl(u32::from(date.day().checked_sub(1)?))?;
                    }
                } else {
                    let shift = u32::from(days_forward(month_first.weekday()?, weekday));
                    dates |= WEEKDAY_BITS.checked_shl(shift)?;
                }
                terms &= terms.checked_sub(1)?;
            }
        }
        let valid = u32::MAX.checked_shr(32_u32.checked_sub(u32::from(length))?)?;
        Some(dates & valid)
    }
}
