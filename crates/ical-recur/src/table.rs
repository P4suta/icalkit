// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5545 section 3.3.10's expand/limit table, transcribed as data.
//!
//! The table is the specification of the expansion engine, not a hint about it. For each
//! `FREQ` and each `BYxxx` rule part it says one of three things: the part *expands* the
//! candidate set for one period, the part *limits* an already-expanded set, or the pair is
//! not applicable and the part is ignored. An engine that reads `BYMONTHDAY` as limiting
//! under `FREQ=MONTHLY` — it expands — is wrong in a way that looks right on every rule
//! whose `DTSTART` already falls on the day it names, which is most of them.
//!
//! It is transcribed here rather than distributed through a `match` per frequency for two
//! reasons. A reviewer can diff sixty-three cells against the RFC and cannot diff seven
//! branches against it. And this crate's Clippy profile bounds a function at 100 lines and a
//! cognitive complexity of 15, which one hand-written match over the whole table exceeds
//! twice over — the shape the gate is asking for is the shape the specification already has.
//!
//! Two cells are not values. RFC 5545 writes "Note 1" and "Note 2" for `BYDAY` under
//! `MONTHLY` and `YEARLY`, and both notes resolve against *which other parts are present*
//! rather than against the frequency alone. That resolution is [`PartsPresent`] and
//! [`effect`], and it is the only place in this module where a cell is computed rather than
//! read.

use crate::rule::{Freq, RulePart};

/// A cell of RFC 5545 section 3.3.10's table, exactly as the RFC writes one.
///
/// Four inhabitants because the printed table has four kinds of cell. Resolving the two notes
/// here would lose the ability to check this type against the source, which is the whole
/// reason the transcription is a type at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cell {
    /// `N/A`. The part does not apply to this frequency and is ignored.
    NotApplicable,
    /// `Limit`. The part filters candidates the frequency and earlier parts produced.
    Limit,
    /// `Expand`. The part multiplies the candidate set for one period.
    Expand,
    /// `Note 1`. `BYDAY` under `FREQ=MONTHLY`.
    NoteOne,
    /// `Note 2`. `BYDAY` under `FREQ=YEARLY`.
    NoteTwo,
}

/// The scope a `BYDAY` ordinal counts within once a note has been resolved.
///
/// `BYDAY=-1MO` means the last Monday *of something*, and RFC 5545's two notes exist because
/// that something is not always the period the frequency names: under `FREQ=YEARLY` with
/// `BYWEEKNO` it is the week, with `BYMONTH` it is the month, and with neither it is the
/// year. An engine that assumed the period would place `-1MO` in December every time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeekdayScope {
    /// Within the week, as `FREQ=WEEKLY` and as Note 2's `BYWEEKNO` branch.
    Week,
    /// Within the month, as Note 1's default and as Note 2's `BYMONTH` branch.
    Month,
    /// Within the year, as Note 2's last branch.
    Year,
}

/// What one rule part does to the candidate set, after both notes are resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PartEffect {
    /// The part does not apply to this frequency and is ignored.
    NotApplicable,
    /// The part filters the candidate set.
    Limit,
    /// The part multiplies the candidate set, one candidate per listed value.
    Expand,
    /// `BYDAY` multiplies the candidate set with weekdays counted within this scope.
    ///
    /// Distinct from [`PartEffect::Expand`] because a `BYDAY` ordinal needs the scope to mean
    /// anything, and the RFC's phrase for these cells is "special expand" rather than
    /// "expand".
    ExpandWeekdays(WeekdayScope),
}

impl PartEffect {
    /// Whether this part contributes candidates rather than removing them.
    #[must_use]
    pub const fn expands(self) -> bool {
        matches!(self, Self::Expand | Self::ExpandWeekdays(_))
    }

    /// Whether this part filters an already-expanded set.
    #[must_use]
    pub const fn limits(self) -> bool {
        matches!(self, Self::Limit)
    }
}

/// Which rule parts a rule carries, as the two notes ask.
///
/// A bit per row of the table rather than a field per part. Four parts are ever asked
/// about — Note 1 reads `BYMONTHDAY`, and Note 2 reads `BYYEARDAY`, `BYMONTHDAY`, `BYWEEKNO`
/// and `BYMONTH` — but four named booleans is a shape this workspace's Clippy profile refuses
/// on sight, and it is right to: four `bool` parameters in a row is four chances to transpose
/// two of them with nothing to catch it. A part names its own bit and cannot be passed
/// positionally at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartsPresent {
    /// One bit per [`RulePart`], set when the rule carries that part.
    present: u16,
}

impl PartsPresent {
    /// A rule carrying no rule part at all.
    pub const NONE: Self = Self { present: 0 };

    /// The same set, with `part` also present.
    #[must_use]
    pub const fn with(self, part: RulePart) -> Self {
        Self {
            present: self.present | part.bit(),
        }
    }

    /// Whether `part` is present.
    #[must_use]
    pub const fn has(self, part: RulePart) -> bool {
        self.present & part.bit() != 0
    }
}

/// RFC 5545 section 3.3.10's table, one row per rule part and one column per frequency.
///
/// Rows are in [`RulePart::ALL`]'s order and columns in [`Freq::ALL`]'s order, and both of
/// those are the RFC's own printed order — top to bottom `BYMONTH` through `BYSETPOS`, left
/// to right `SECONDLY` through `YEARLY`. That is not decoration: a reviewer holding the RFC
/// beside this array compares it cell by cell in one pass, and a transposed or reordered
/// transcription is invisible in every other arrangement.
const TABLE: [[Cell; Freq::COUNT]; RulePart::COUNT] = {
    use Cell::{Expand, Limit, NotApplicable, NoteOne, NoteTwo};
    [
        // SECONDLY     MINUTELY  HOURLY   DAILY          WEEKLY         MONTHLY  YEARLY
        // BYMONTH
        [Limit, Limit, Limit, Limit, Limit, Limit, Expand],
        // BYWEEKNO
        [
            NotApplicable,
            NotApplicable,
            NotApplicable,
            NotApplicable,
            NotApplicable,
            NotApplicable,
            Expand,
        ],
        // BYYEARDAY
        [
            Limit,
            Limit,
            Limit,
            NotApplicable,
            NotApplicable,
            NotApplicable,
            Expand,
        ],
        // BYMONTHDAY
        [Limit, Limit, Limit, Limit, NotApplicable, Expand, Expand],
        // BYDAY
        [Limit, Limit, Limit, Limit, Expand, NoteOne, NoteTwo],
        // BYHOUR
        [Limit, Limit, Limit, Expand, Expand, Expand, Expand],
        // BYMINUTE
        [Limit, Limit, Expand, Expand, Expand, Expand, Expand],
        // BYSECOND
        [Limit, Expand, Expand, Expand, Expand, Expand, Expand],
        // BYSETPOS
        [Limit, Limit, Limit, Limit, Limit, Limit, Limit],
    ]
};

/// The cell RFC 5545 section 3.3.10 prints for `part` under `freq`, notes unresolved.
///
/// Total by construction: both indices come from the enums the table is sized by.
#[must_use]
pub const fn cell(freq: Freq, part: RulePart) -> Cell {
    TABLE[part.index()][freq.index()]
}

/// What `part` does to the candidate set under `freq`, with both notes resolved.
///
/// `present` is only ever read for the two `BYDAY` cells; every other cell is a value the
/// table already carries.
#[must_use]
pub const fn effect(freq: Freq, part: RulePart, present: PartsPresent) -> PartEffect {
    match cell(freq, part) {
        Cell::NotApplicable => PartEffect::NotApplicable,
        Cell::Limit => PartEffect::Limit,
        Cell::Expand => match part {
            // `BYDAY` under `FREQ=WEEKLY` is the one plain `Expand` the RFC prints for this
            // row, and its weekdays are counted within the week the frequency names.
            RulePart::Day => PartEffect::ExpandWeekdays(WeekdayScope::Week),
            _ => PartEffect::Expand,
        },
        Cell::NoteOne => note_one(present),
        Cell::NoteTwo => note_two(present),
    }
}

/// "Limit if BYMONTHDAY is present; otherwise, special expand for MONTHLY."
#[must_use]
const fn note_one(present: PartsPresent) -> PartEffect {
    if present.has(RulePart::MonthDay) {
        PartEffect::Limit
    } else {
        PartEffect::ExpandWeekdays(WeekdayScope::Month)
    }
}

/// "Limit if BYYEARDAY or BYMONTHDAY is present; otherwise, special expand for WEEKLY if
/// BYWEEKNO present; otherwise, special expand for MONTHLY if BYMONTH present; otherwise,
/// special expand for YEARLY."
///
/// The branches are in the note's own order and the order is load-bearing: `BYWEEKNO` beats
/// `BYMONTH`, so `FREQ=YEARLY;BYWEEKNO=20;BYMONTH=5;BYDAY=MO` counts Mondays within week 20
/// and not within May.
#[must_use]
const fn note_two(present: PartsPresent) -> PartEffect {
    if present.has(RulePart::YearDay) || present.has(RulePart::MonthDay) {
        PartEffect::Limit
    } else if present.has(RulePart::WeekNo) {
        PartEffect::ExpandWeekdays(WeekdayScope::Week)
    } else if present.has(RulePart::Month) {
        PartEffect::ExpandWeekdays(WeekdayScope::Month)
    } else {
        PartEffect::ExpandWeekdays(WeekdayScope::Year)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, PartEffect, PartsPresent, WeekdayScope, cell, effect};
    use crate::rule::{Freq, RulePart};

    /// The row the milestone brief singles out, held against the RFC in both directions.
    #[test]
    fn by_month_day_expands_under_monthly_and_limits_under_daily() {
        assert_eq!(cell(Freq::Monthly, RulePart::MonthDay), Cell::Expand);
        assert_eq!(cell(Freq::Daily, RulePart::MonthDay), Cell::Limit);
        assert_eq!(cell(Freq::Weekly, RulePart::MonthDay), Cell::NotApplicable);
    }

    /// `BYSETPOS` limits under every frequency, which is the row that has no exception.
    #[test]
    fn by_set_pos_limits_everywhere() {
        for freq in Freq::ALL {
            assert_eq!(
                effect(freq, RulePart::SetPos, PartsPresent::NONE),
                PartEffect::Limit,
                "{freq:?}"
            );
        }
    }

    /// Note 1, both branches.
    #[test]
    fn note_one_limits_beside_by_month_day_and_expands_within_the_month_otherwise() {
        let bare = PartsPresent::NONE;
        let with_month_day = PartsPresent::NONE.with(RulePart::MonthDay);
        assert_eq!(
            effect(Freq::Monthly, RulePart::Day, bare),
            PartEffect::ExpandWeekdays(WeekdayScope::Month)
        );
        assert_eq!(
            effect(Freq::Monthly, RulePart::Day, with_month_day),
            PartEffect::Limit
        );
    }

    /// Note 2, every branch, in the order the note states them.
    #[test]
    fn note_two_resolves_in_the_order_the_rfc_writes_it() {
        let cases = [
            (
                PartsPresent::NONE.with(RulePart::YearDay),
                PartEffect::Limit,
            ),
            (
                PartsPresent::NONE.with(RulePart::MonthDay),
                PartEffect::Limit,
            ),
            (
                PartsPresent::NONE
                    .with(RulePart::Month)
                    .with(RulePart::WeekNo),
                PartEffect::ExpandWeekdays(WeekdayScope::Week),
            ),
            (
                PartsPresent::NONE.with(RulePart::Month),
                PartEffect::ExpandWeekdays(WeekdayScope::Month),
            ),
            (
                PartsPresent::NONE,
                PartEffect::ExpandWeekdays(WeekdayScope::Year),
            ),
        ];
        for (present, expected) in cases {
            assert_eq!(effect(Freq::Yearly, RulePart::Day, present), expected);
        }
    }

    /// Every cell is reachable and the two notes appear exactly where the RFC prints them.
    #[test]
    fn the_transcription_covers_every_pair_and_notes_only_by_day() {
        for part in RulePart::ALL {
            for freq in Freq::ALL {
                let printed = cell(freq, part);
                let is_note = matches!(printed, Cell::NoteOne | Cell::NoteTwo);
                assert_eq!(
                    is_note,
                    part == RulePart::Day && matches!(freq, Freq::Monthly | Freq::Yearly),
                    "{part:?} under {freq:?}"
                );
                let resolved = effect(freq, part, PartsPresent::NONE);
                assert_eq!(
                    resolved == PartEffect::NotApplicable,
                    printed == Cell::NotApplicable,
                    "{part:?} under {freq:?}"
                );
            }
        }
    }
}
