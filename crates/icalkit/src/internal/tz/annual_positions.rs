// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Keep nonempty signed positions within a yearly date or candidate set.

use crate::internal::core::CivilDate;

#[derive(Clone, Copy, Debug)]
pub(super) struct AnnualPositions([u64; 12]);

impl AnnualPositions {
    pub(super) fn values(self) -> impl Iterator<Item = i16> {
        (-366..=366).filter(move |position| self.contains(*position))
    }

    pub(super) fn indices(self, extent: u64) -> impl Iterator<Item = u64> {
        let mut forward = (1_i16..=366)
            .filter(move |position| self.contains(*position))
            .filter_map(|position| u64::try_from(position).ok())
            .filter(move |position| *position <= extent)
            .peekable();
        let mut reverse = (-366_i16..=-1)
            .filter(move |position| self.contains(*position))
            .filter_map(move |position| {
                extent
                    .checked_add(1)?
                    .checked_sub(u64::from(position.unsigned_abs()))
            })
            .filter(|position| *position != 0)
            .peekable();
        core::iter::from_fn(
            move || match (forward.peek().copied(), reverse.peek().copied()) {
                (Some(left), Some(right)) if left == right => {
                    reverse.next();
                    forward.next()
                },
                (Some(left), Some(right)) if left < right => forward.next(),
                (Some(_) | None, Some(_)) => reverse.next(),
                (Some(_), None) => forward.next(),
                (None, None) => None,
            },
        )
    }

    pub(super) fn singleton(position: i16) -> Option<Self> {
        Self([0; 12]).with_position(position)
    }

    pub(super) fn with_position(mut self, position: i16) -> Option<Self> {
        let (word, bit) = position_bit(position)?;
        *self.0.get_mut(word)? |= bit;
        Some(self)
    }

    pub(super) fn matches(self, position: u16, extent: u16) -> bool {
        if position == 0 || position > extent || extent > 371 {
            return false;
        }
        if let Ok(forward) = i16::try_from(position)
            && self.contains(forward)
        {
            return true;
        }
        extent
            .checked_add(1)
            .and_then(|end| end.checked_sub(position))
            .and_then(|reverse| i16::try_from(reverse).ok())
            .and_then(i16::checked_neg)
            .is_some_and(|reverse| self.contains(reverse))
    }

    pub(super) fn mask_in_month(self, year: u16, month: u8) -> Option<u32> {
        let mut before = 0_u16;
        for prior in 1..month {
            before = before.checked_add(u16::from(CivilDate::days_in_month(year, prior)?))?;
        }
        let extent = if CivilDate::days_in_month(year, 2)? == 29 {
            366
        } else {
            365
        };
        let length = CivilDate::days_in_month(year, month)?;
        let mut mask = 0;
        for day in 1..=length {
            if self.matches(before.checked_add(u16::from(day))?, extent) {
                mask |= 1_u32.checked_shl(u32::from(day.checked_sub(1)?))?;
            }
        }
        Some(mask)
    }

    fn contains(self, position: i16) -> bool {
        let Some((word, bit)) = position_bit(position) else {
            return false;
        };
        self.0.get(word).is_some_and(|value| *value & bit != 0)
    }
}

fn position_bit(position: i16) -> Option<(usize, u64)> {
    let number = position.unsigned_abs();
    if !(1..=366).contains(&number) {
        return None;
    }
    let index = usize::from(number.checked_sub(1)?);
    let offset = if position.is_negative() { 6 } else { 0 };
    let word = index.checked_div(64)?.checked_add(offset)?;
    let shift = u32::try_from(index.checked_rem(64)?).ok()?;
    Some((word, 1_u64.checked_shl(shift)?))
}
