// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Classify Gregorian year layouts and project adjacent week-numbering years.

use crate::internal::core::{CivilDate, Weekday};
use crate::internal::tz::rules::days_forward;

pub(super) const YEAR_LAYOUTS: [u16; 14] = [
    2001, 2002, 2003, 2009, 2010, 2005, 2006, 2024, 2008, 2020, 2004, 2016, 2000, 2012,
];

pub(super) const NORMAL_WEEK_LAYOUTS: usize = 28;
pub(super) const WEEK_LAYOUTS: usize = 32;
pub(super) const BOUNDARY_YEARS: [i32; 4] = [-1, 0, 9999, 10000];
const CYCLE_DAYS: i64 = 146_097;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum YearOffset {
    Previous,
    Same,
    Next,
}

impl YearOffset {
    pub(super) const ALL: [Self; 3] = [Self::Previous, Self::Same, Self::Next];

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Previous => 0,
            Self::Same => 1,
            Self::Next => 2,
        }
    }

    const fn number(self) -> i32 {
        match self {
            Self::Previous => -1,
            Self::Same => 0,
            Self::Next => 1,
        }
    }

    pub(super) fn between(period: i32, calendar: u16) -> Option<Self> {
        match i32::from(calendar).checked_sub(period)? {
            -1 => Some(Self::Previous),
            0 => Some(Self::Same),
            1 => Some(Self::Next),
            _ => None,
        }
    }

    pub(super) fn period_year(self, calendar: u16) -> Option<i32> {
        i32::from(calendar).checked_sub(self.number())
    }

    pub(super) fn calendar_year(self, period: i32) -> Option<u16> {
        let year = u16::try_from(period.checked_add(self.number())?).ok()?;
        CivilDate::from_ymd(year, 1, 1).map(CivilDate::year)
    }
}

pub(super) fn calendar_layout(year: u16) -> Option<usize> {
    let weekday = usize::from(CivilDate::from_ymd(year, 1, 1)?.weekday()?.index());
    let leap = CivilDate::days_in_month(year, 2)? == 29;
    weekday.checked_add(if leap { 7 } else { 0 })
}

pub(super) fn week_layout(year: i32) -> Option<usize> {
    if let Some(index) = BOUNDARY_YEARS.iter().position(|boundary| *boundary == year) {
        return NORMAL_WEEK_LAYOUTS.checked_add(index);
    }
    if !(1..=9998).contains(&year) {
        return None;
    }
    let representative = representative(year)?;
    let weekday = usize::from(
        CivilDate::from_ymd(representative, 1, 1)?
            .weekday()?
            .index(),
    );
    let context = if leap(representative)? {
        1
    } else if leap(representative.checked_sub(1)?)? {
        2
    } else if leap(representative.checked_add(1)?)? {
        3
    } else {
        0
    };
    weekday.checked_add(7_usize.checked_mul(context)?)
}

fn leap(year: u16) -> Option<bool> {
    Some(CivilDate::days_in_month(year, 2)? == 29)
}

fn representative(year: i32) -> Option<u16> {
    u16::try_from(2000_i32.checked_add(year.rem_euclid(400))?).ok()
}

fn year_start(year: i32) -> Option<i64> {
    let representative = representative(year)?;
    let origin = CivilDate::from_ymd(representative, 1, 1)?.days_from_epoch()?;
    let cycles = i64::from(year)
        .checked_sub(i64::from(representative))?
        .checked_div(400)?;
    origin.checked_add(cycles.checked_mul(CYCLE_DAYS)?)
}

fn week_one_start(year: i32, start: Weekday) -> Option<i64> {
    let fourth = CivilDate::from_ymd(representative(year)?, 1, 4)?;
    let shift = days_forward(start, fourth.weekday()?);
    year_start(year)?
        .checked_add(3)?
        .checked_sub(i64::from(shift))
}

pub(super) fn week_span(year: i32, start: Weekday) -> Option<(i64, u16)> {
    let first = week_one_start(year, start)?;
    let next = week_one_start(year.checked_add(1)?, start)?;
    let days = u16::try_from(next.checked_sub(first)?).ok()?;
    [364, 371].contains(&days).then_some((first, days))
}

pub(super) fn week_year(date: CivilDate, start: Weekday) -> Option<i32> {
    let year = i32::from(date.year());
    let day = date.days_from_epoch()?;
    if day < week_one_start(year, start)? {
        year.checked_sub(1)
    } else if day >= week_one_start(year.checked_add(1)?, start)? {
        year.checked_add(1)
    } else {
        Some(year)
    }
}
