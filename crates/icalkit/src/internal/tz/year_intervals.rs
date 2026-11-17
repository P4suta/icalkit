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

    pub(super) fn periods_through(self, first: u16, last: u16) -> Option<u32> {
        u32::from(last.checked_sub(first)?).checked_div(self.0.get())
    }

    pub(super) fn year_at(self, first: u16, period: u32) -> Option<u16> {
        let distance = period.checked_mul(self.0.get())?;
        u16::try_from(u32::from(first).checked_add(distance)?).ok()
    }

    pub(super) fn contains(self, first: u16, year: u16) -> bool {
        year.checked_sub(first)
            .and_then(|distance| u32::from(distance).checked_rem(self.0.get()))
            .is_some_and(|remainder| remainder == 0)
    }

    pub(super) fn prior_years(self, first: u16, limit: u16) -> impl Iterator<Item = u16> {
        let current = self
            .periods_through(first, limit)
            .and_then(|period| self.year_at(first, period));
        core::iter::successors(current, move |year| {
            u32::from(*year)
                .checked_sub(self.0.get())
                .and_then(|year| u16::try_from(year).ok())
                .filter(|year| *year >= first)
        })
        .take(MAX_PROBES)
    }
}
