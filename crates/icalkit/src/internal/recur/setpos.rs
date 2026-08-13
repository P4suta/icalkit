// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 4 — `BYSETPOS` and the period boundary.
//!
//! # What this unit owns
//!
//! Selecting from a finished period's candidate set by position, counted from both ends, and
//! owning the fact that a period must be *complete* before any of it can be emitted.
//!
//! `BYSETPOS` is not a `BYxxx` part. It applies after every other part has produced the
//! candidate set for one period, and a negative position — `BYSETPOS=-1`, "the last one" —
//! forces the whole period's set to exist before anything at all can be handed out. That is
//! exactly where a budget has to be charged, because that set is unbounded for a hostile rule:
//! a `FREQ=YEARLY` rule listing sixty seconds, sixty minutes and twenty-four hours fills a
//! year before position `-1` can be known.
//!
//! # What this unit must not do
//!
//! - It must not emit before the period closes when any position is negative. Streaming the
//!   first candidate out and revising later is how a `-1` becomes a `+1`. The shape here makes
//!   that unrepresentable rather than merely discouraged: [`select`] takes a finished
//!   [`CandidateSet`] by reference and returns the whole selection at once, so there is no
//!   partial state for a later candidate to revise.
//! - It must not treat a position past the end as an error. Section 3.3.10 says a `BYSETPOS`
//!   naming a position the set does not have selects nothing; the period is simply empty.
//! - It must not renumber. Position 1 is the first candidate in *chronological* order, which
//!   is the order unit 3 hands the set over in, and not the order the file wrote the parts.
//!   The output is chronological too, so `BYSETPOS=-1,1` emits the first candidate before the
//!   last one.
//! - It must not charge. It reports the set size it forced into existence; unit 7 charges it.
//!
//! # Signatures it provides
//!
//! ```text
//! pub struct SelectedCandidates { /* private */ }
//! impl SelectedCandidates {
//!     pub fn as_slice(&self) -> &[CivilDateTime];
//!     pub fn forced_full_period(&self) -> usize;
//! }
//! pub fn select(set: &CandidateSet, positions: &ByList<i16>) -> SelectedCandidates;
//! ```
//!
//! `forced_full_period` is the honest half, and it is a count rather than a flag because a
//! flag is not actionable. A caller tuning `Limits::candidates_per_period` needs the number of
//! candidates that had to exist before the selection could answer, and unit 7 charges that
//! same number before selection so that one `next()` cannot do unbounded uncharged work. A
//! negative position makes it the whole period unconditionally; a positive-only `BYSETPOS`
//! needs only as far as its largest position reaches, which is the one case where the two
//! answers differ and the case a flag would round up.
//!
//! # How it is tested on its own
//!
//! `BYSETPOS=1`, `-1`, `2,-2`, a position past the end, a set of one under `-1`, and duplicate
//! positions naming one candidate twice — which must yield it once, because the recurrence set
//! is a set. `BYSETPOS=0` is unreachable here: the decoder refuses it and the builder refuses
//! a `BYSETPOS` with nothing to select from, so both are tested where they are decided. It is
//! nonetheless total rather than panicking, and selects nothing, because a value the type
//! system cannot forbid must have a stated answer.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cmp::Ordering;

use ical_core::CivilDateTime;

use crate::internal::recur::byparts::CandidateSet;
use crate::internal::recur::rule::ByList;

/// What one period's `BYSETPOS` selected, and how much of the period it had to see first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedCandidates {
    /// The selected instants, ascending and each appearing at most once.
    chosen: Vec<CivilDateTime>,
    /// How many candidates had to exist before the selection could answer.
    forced: usize,
}

impl SelectedCandidates {
    /// The selected instants, in chronological order and each appearing once.
    ///
    /// Chronological rather than in the order `BYSETPOS` listed its positions: RFC 5545
    /// section 3.3.10 defines a recurrence *set*, and the order a file wrote `-1,1` in is not
    /// a claim about which instant comes first.
    #[must_use]
    pub fn as_slice(&self) -> &[CivilDateTime] {
        &self.chosen
    }

    /// How many of the period's candidates had to exist before this selection could answer.
    ///
    /// The whole set whenever any position is negative, because "the last one" is not known
    /// until the period closes. Otherwise the largest position named, capped at the set — a
    /// positive-only `BYSETPOS` never needs the tail. Unit 7 charges this; this unit does not.
    #[must_use]
    pub const fn forced_full_period(&self) -> usize {
        self.forced
    }
}

/// Apply one period's `BYSETPOS` to that period's finished candidate set.
///
/// An empty `positions` means the part is absent — [`ByList`] gives absent and empty one
/// state — and the whole set passes through unchanged.
#[must_use]
pub fn select(set: &CandidateSet, positions: &ByList<i16>) -> SelectedCandidates {
    select_within(set.as_slice(), positions)
}

/// [`select`] against a plain slice, so the selection is testable without a [`CandidateSet`].
///
/// `CandidateSet` guards its own invariant — ascending and deduplicated — by having no public
/// constructor, which is right for it and leaves this unit no way to build a fixture. The
/// split keeps the delegation in `select` down to one line rather than leaving the arithmetic
/// below untested.
fn select_within(candidates: &[CivilDateTime], positions: &ByList<i16>) -> SelectedCandidates {
    let extent = candidates.len();
    if positions.is_empty() {
        let whole = candidates.to_vec();
        return SelectedCandidates {
            chosen: whole,
            forced: extent,
        };
    }

    // A set, not a list: two positions may name one candidate (`1` and `-4` over four
    // candidates do), and RFC 5545 section 3.3.10 defines a recurrence set. `BTreeSet` also
    // hands the indices back ascending, which is the chronological order the input is in.
    let mut picked = BTreeSet::new();
    let mut forced = 0_usize;
    for nth in positions.as_slice().iter().copied() {
        forced = forced.max(reach(nth, extent));
        if let Some(index) = resolve(nth, extent) {
            picked.insert(index);
        }
    }

    let chosen = picked
        .into_iter()
        .filter_map(|index| candidates.get(index).copied())
        .collect();
    SelectedCandidates { chosen, forced }
}

/// The zero-based index `nth` names in a set of `extent` candidates, if the set has it.
///
/// `None` for a position past either end, which section 3.3.10 makes an empty period rather
/// than an error, and `None` for zero, which the decoder refuses before a rule is ever built.
fn resolve(nth: i16, extent: usize) -> Option<usize> {
    match nth.cmp(&0) {
        // `1` is the first candidate, so the index is one less; a set shorter than `nth` has
        // nothing at that position.
        Ordering::Greater => {
            let ordinal = usize::try_from(nth).ok()?;
            let index = ordinal.checked_sub(1)?;
            (index < extent).then_some(index)
        },
        // `-1` is the last candidate, so the index is that far back from the end; `checked_sub`
        // is what answers `None` for a set shorter than the position counts back.
        Ordering::Less => extent.checked_sub(usize::from(nth.unsigned_abs())),
        Ordering::Equal => None,
    }
}

/// How many candidates must exist before `nth` can be resolved against a set of `extent`.
///
/// This is what [`SelectedCandidates::forced_full_period`] reports, maximized over the
/// positions. A negative position reaches the end of the period whatever the period holds,
/// which is the whole point of charging here.
fn reach(nth: i16, extent: usize) -> usize {
    match nth.cmp(&0) {
        Ordering::Greater => usize::try_from(nth).unwrap_or(extent).min(extent),
        Ordering::Less => extent,
        Ordering::Equal => 0,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{CivilDate, CivilDateTime, CivilTime};

    use super::{select, select_within};
    use crate::internal::recur::byparts::CandidateSet;
    use crate::internal::recur::rule::ByList;

    /// A year, month and day, at the 9:00 AM the RFC 5545 section 3.8.5.3 examples all use.
    fn at(year: u16, month: u8, day: u8) -> CivilDateTime {
        CivilDateTime::new(
            CivilDate::from_ymd(year, month, day).unwrap(),
            CivilTime::from_hms(9, 0, 0).unwrap(),
        )
    }

    /// Every weekday of October 1997: the candidate set `FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR`
    /// produces for that month. October 1, 1997 was a Wednesday, so the month holds 23.
    const OCTOBER_1997_WORKDAYS: &[(u16, u8, u8)] = &[
        (1997, 10, 1),
        (1997, 10, 2),
        (1997, 10, 3),
        (1997, 10, 6),
        (1997, 10, 7),
        (1997, 10, 8),
        (1997, 10, 9),
        (1997, 10, 10),
        (1997, 10, 13),
        (1997, 10, 14),
        (1997, 10, 15),
        (1997, 10, 16),
        (1997, 10, 17),
        (1997, 10, 20),
        (1997, 10, 21),
        (1997, 10, 22),
        (1997, 10, 23),
        (1997, 10, 24),
        (1997, 10, 27),
        (1997, 10, 28),
        (1997, 10, 29),
        (1997, 10, 30),
        (1997, 10, 31),
    ];

    /// Every weekday of February 2000, the leap February. February 1, 2000 was a Tuesday and
    /// February 29 was a Tuesday too, so the month's last work day is the leap day itself.
    const FEBRUARY_2000_WORKDAYS: &[(u16, u8, u8)] = &[
        (2000, 2, 1),
        (2000, 2, 2),
        (2000, 2, 3),
        (2000, 2, 4),
        (2000, 2, 7),
        (2000, 2, 8),
        (2000, 2, 9),
        (2000, 2, 10),
        (2000, 2, 11),
        (2000, 2, 14),
        (2000, 2, 15),
        (2000, 2, 16),
        (2000, 2, 17),
        (2000, 2, 18),
        (2000, 2, 21),
        (2000, 2, 22),
        (2000, 2, 23),
        (2000, 2, 24),
        (2000, 2, 25),
        (2000, 2, 28),
        (2000, 2, 29),
    ];

    /// The Mondays of October 1997, which is `FREQ=MONTHLY;BYDAY=MO`'s set for that month.
    const OCTOBER_1997_MONDAYS: &[(u16, u8, u8)] = &[
        (1997, 10, 6),
        (1997, 10, 13),
        (1997, 10, 20),
        (1997, 10, 27),
    ];

    /// The Mondays of December 1997 — five of them, unlike the four either neighbor has.
    const DECEMBER_1997_MONDAYS: &[(u16, u8, u8)] = &[
        (1997, 12, 1),
        (1997, 12, 8),
        (1997, 12, 15),
        (1997, 12, 22),
        (1997, 12, 29),
    ];

    /// The Mondays of January 1998, the period across the year boundary from December's.
    const JANUARY_1998_MONDAYS: &[(u16, u8, u8)] =
        &[(1998, 1, 5), (1998, 1, 12), (1998, 1, 19), (1998, 1, 26)];

    /// The Mondays of February 1998.
    const FEBRUARY_1998_MONDAYS: &[(u16, u8, u8)] =
        &[(1998, 2, 2), (1998, 2, 9), (1998, 2, 16), (1998, 2, 23)];

    /// One selection: a period's finished candidate set, the `BYSETPOS` applied to it, what
    /// comes out, and how much of the period had to exist first.
    struct Case {
        /// What the row is for, quoted in the assertion so a failure names itself.
        name: &'static str,
        /// The period's candidate set, chronological, as unit 3 hands it over.
        candidates: &'static [(u16, u8, u8)],
        /// The `BYSETPOS` values, in the order the file wrote them.
        positions: &'static [i16],
        /// The instants selected, chronological.
        expected: &'static [(u16, u8, u8)],
        /// The expected `forced_full_period`.
        forced: usize,
    }

    /// The expectations come from RFC 5545 section 3.8.5.3's own worked examples wherever it
    /// carries one. "The last work day of the month" is
    /// `FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1` and the RFC's output includes October
    /// 31 and December 31, 1997; "monthly on the second-to-last Monday of the month" is
    /// `FREQ=MONTHLY;BYDAY=MO;BYSETPOS=-2` and the RFC's output is September 22, October 20,
    /// November 17, December 22, January 19 and February 16. The rows below reuse those
    /// answers per period. The leap-February row is the same rule against a month the RFC did
    /// not print, so its expectation is a calendar fact rather than a quoted one.
    const CASES: &[Case] = &[
        Case {
            name: "the last work day of October 1997 is the 31st (RFC 5545 section 3.8.5.3)",
            candidates: OCTOBER_1997_WORKDAYS,
            positions: &[-1],
            expected: &[(1997, 10, 31)],
            forced: 23,
        },
        Case {
            name: "the last work day of the leap February 2000 is the leap day",
            candidates: FEBRUARY_2000_WORKDAYS,
            positions: &[-1],
            expected: &[(2000, 2, 29)],
            forced: 21,
        },
        Case {
            name: "the second-to-last Monday of October 1997 is the 20th (section 3.8.5.3)",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[-2],
            expected: &[(1997, 10, 20)],
            forced: 4,
        },
        Case {
            name: "the second-to-last Monday of the five-Monday December 1997 is the 22nd",
            candidates: DECEMBER_1997_MONDAYS,
            positions: &[-2],
            expected: &[(1997, 12, 22)],
            forced: 5,
        },
        Case {
            name: "across the year boundary January 1998 counts its own Mondays, giving the 19th",
            candidates: JANUARY_1998_MONDAYS,
            positions: &[-2],
            expected: &[(1998, 1, 19)],
            forced: 4,
        },
        Case {
            name: "the second-to-last Monday of February 1998 is the 16th (section 3.8.5.3)",
            candidates: FEBRUARY_1998_MONDAYS,
            positions: &[-2],
            expected: &[(1998, 2, 16)],
            forced: 4,
        },
        Case {
            name: "BYSETPOS=1 takes the first candidate and needs no more of the period",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[1],
            expected: &[(1997, 10, 6)],
            forced: 1,
        },
        Case {
            name: "BYSETPOS=2,-2 counts from both ends and one negative forces the whole period",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[2, -2],
            expected: &[(1997, 10, 13), (1997, 10, 20)],
            forced: 4,
        },
        Case {
            name: "BYSETPOS=-1,1 comes out chronological, not in the order the file wrote it",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[-1, 1],
            expected: &[(1997, 10, 6), (1997, 10, 27)],
            forced: 4,
        },
        Case {
            name: "a set of one under BYSETPOS=-1 is that one: FREQ=MONTHLY;BYMONTHDAY=-1",
            candidates: &[(1997, 12, 31)],
            positions: &[-1],
            expected: &[(1997, 12, 31)],
            forced: 1,
        },
        Case {
            name: "a 31st under FREQ=MONTHLY;BYSETPOS=1 in a month that has one",
            candidates: &[(1998, 1, 31)],
            positions: &[1],
            expected: &[(1998, 1, 31)],
            forced: 1,
        },
        Case {
            name: "a position past the end selects nothing and the period is simply empty",
            candidates: FEBRUARY_1998_MONDAYS,
            positions: &[5],
            expected: &[],
            forced: 4,
        },
        Case {
            name: "a negative position past the start selects nothing rather than clamping",
            candidates: FEBRUARY_1998_MONDAYS,
            positions: &[-5],
            expected: &[],
            forced: 4,
        },
        Case {
            name: "an empty period selects nothing under -1 and forces nothing into existence",
            candidates: &[],
            positions: &[-1],
            expected: &[],
            forced: 0,
        },
        Case {
            name: "1 and -4 over four candidates name one instant, and it is yielded once",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[1, -4],
            expected: &[(1997, 10, 6)],
            forced: 4,
        },
        Case {
            name: "a position repeated verbatim is still one instant",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[2, 2],
            expected: &[(1997, 10, 13)],
            forced: 2,
        },
        Case {
            name: "an absent BYSETPOS passes the whole period through unchanged",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[],
            expected: OCTOBER_1997_MONDAYS,
            forced: 4,
        },
        Case {
            name: "BYSETPOS=0 is refused by the decoder and selects nothing if it ever arrives",
            candidates: OCTOBER_1997_MONDAYS,
            positions: &[0],
            expected: &[],
            forced: 0,
        },
    ];

    /// Turn a table column of `(year, month, day)` into the instants the selection speaks in.
    fn instants(days: &[(u16, u8, u8)]) -> Vec<CivilDateTime> {
        days.iter()
            .map(|(year, month, day)| at(*year, *month, *day))
            .collect()
    }

    #[test]
    fn by_set_pos_selects_what_the_rfc_says_it_selects() {
        for case in CASES {
            let candidates = instants(case.candidates);
            let positions = ByList::from_slice(case.positions);
            let picked = select_within(&candidates, &positions);
            assert_eq!(
                picked.as_slice(),
                instants(case.expected).as_slice(),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn the_period_size_forced_into_existence_is_reported_for_unit_seven_to_charge() {
        for case in CASES {
            let candidates = instants(case.candidates);
            let positions = ByList::from_slice(case.positions);
            let picked = select_within(&candidates, &positions);
            assert_eq!(picked.forced_full_period(), case.forced, "{}", case.name);
        }
    }

    #[test]
    fn a_negative_position_forces_the_whole_period_whatever_else_is_named() {
        // The hostile shape, in miniature: whichever positive position accompanies it, the
        // `-1` cannot be answered until the last candidate exists, so the charge is the set.
        let candidates = instants(OCTOBER_1997_WORKDAYS);
        for accompanying in [1_i16, 2, 23, 400] {
            let positions = ByList::from_slice(&[accompanying, -1]);
            let picked = select_within(&candidates, &positions);
            assert_eq!(picked.forced_full_period(), OCTOBER_1997_WORKDAYS.len());
        }
    }

    #[test]
    fn a_positive_only_by_set_pos_forces_only_as_far_as_it_reaches() {
        // The half a boolean would round up: `BYSETPOS=2` over a 23-candidate period needs two
        // candidates, and a caller tuning `Limits::candidates_per_period` on the flag would be
        // told to budget for 23.
        let candidates = instants(OCTOBER_1997_WORKDAYS);
        let positions = ByList::from_slice(&[2_i16]);
        let picked = select_within(&candidates, &positions);
        assert_eq!(picked.forced_full_period(), 2);
        assert_eq!(picked.as_slice(), instants(&[(1997, 10, 2)]).as_slice());
    }

    /// The entry point the engine calls answers what the slice behind it answers.
    ///
    /// `select` is one line of delegation and `select_within` carries every position rule, so
    /// this asserts the delegation and nothing else — over the whole table, so a row added later
    /// is covered by it too. It exists because a one-line function nobody calls in a test is
    /// where a `&` or a `.as_slice()` in the wrong place lives forever.
    #[test]
    fn the_candidate_set_door_answers_what_the_slice_behind_it_answers() {
        for case in CASES {
            let candidates = instants(case.candidates);
            let positions = ByList::from_slice(case.positions);
            let set = CandidateSet::from_ascending(&candidates);
            let through_door = select(&set, &positions);
            let direct = select_within(&candidates, &positions);
            assert_eq!(through_door.as_slice(), direct.as_slice(), "{}", case.name);
            assert_eq!(
                through_door.forced_full_period(),
                direct.forced_full_period(),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn selecting_is_a_whole_period_at_a_time_and_never_a_prefix() {
        // The revision hazard, stated as a property: the selection under `-1` over the first
        // `n` candidates of a period is not the selection over the period, for every proper
        // prefix. A streaming implementation that emitted early would have to take one of
        // these back.
        let whole = instants(DECEMBER_1997_MONDAYS);
        let positions = ByList::from_slice(&[-1_i16]);
        let closed = select_within(&whole, &positions);
        assert_eq!(closed.as_slice(), instants(&[(1997, 12, 29)]).as_slice());
        for cut in 1..whole.len() {
            let prefix = select_within(&whole[..cut], &positions);
            assert_ne!(prefix.as_slice(), closed.as_slice());
        }
    }

    #[test]
    fn a_selection_is_ascending_and_free_of_repeats() {
        // Every row at once, so no future row can quietly introduce a duplicate or an
        // out-of-order instant that its own expectation column happens to mirror.
        for case in CASES {
            let candidates = instants(case.candidates);
            let positions = ByList::from_slice(case.positions);
            let picked = select_within(&candidates, &positions);
            assert!(
                picked.as_slice().windows(2).all(|pair| pair[0] < pair[1]),
                "{}",
                case.name
            );
        }
    }
}
