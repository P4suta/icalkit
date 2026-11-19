// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Align bounded Gregorian transition years with a source anchor.

use core::num::NonZeroU32;

/// Gregorian date patterns repeat after 400 years.
/// Three leading years cover UTC lookahead, an offset across New Year, and a later local onset.
const MAX_PROBES: usize = 403;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct YearInterval(NonZeroU32);

impl YearInterval {
    pub(super) const YEARLY: Self = Self(NonZeroU32::MIN);

    pub(super) const fn new(interval: NonZeroU32) -> Self {
        Self(interval)
    }

    pub(super) const fn step(self) -> NonZeroU32 {
        self.0
    }

    pub(super) fn periods_through(
        self,
        first: impl Into<i32>,
        last: impl Into<i32>,
    ) -> Option<u32> {
        u32::try_from(last.into().checked_sub(first.into())?)
            .ok()?
            .checked_div(self.0.get())
    }

    pub(super) fn year_at(self, first: impl Into<i32>, period: u32) -> Option<i32> {
        let distance = period.checked_mul(self.0.get())?;
        i32::try_from(i64::from(first.into()).checked_add(i64::from(distance))?).ok()
    }

    pub(super) fn contains(self, first: impl Into<i32>, year: impl Into<i32>) -> bool {
        year.into()
            .checked_sub(first.into())
            .and_then(|distance| u32::try_from(distance).ok())
            .and_then(|distance| distance.checked_rem(self.0.get()))
            .is_some_and(|remainder| remainder == 0)
    }

    pub(super) fn prior_years(self, first: i32, limit: i32) -> impl Iterator<Item = i32> {
        let current = self
            .periods_through(first, limit)
            .and_then(|period| self.year_at(first, period));
        core::iter::successors(current, move |year| {
            i64::from(*year)
                .checked_sub(i64::from(self.0.get()))
                .and_then(|year| i32::try_from(year).ok())
                .filter(|year| *year >= first)
        })
        .take(MAX_PROBES)
    }
}
