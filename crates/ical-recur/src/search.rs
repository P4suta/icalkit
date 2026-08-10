// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The window, the occurrence, and the terminal state a search must not be able to lose.
//!
//! # Why the item type is not a `Result`
//!
//! `docs/adr/0002` requires budget exhaustion to be a *reported outcome*, never a hang and
//! never a silent empty result. An item type of `Result<Occurrence, BudgetExhausted>` does not
//! deliver that, and the reason is mechanical rather than stylistic: std ships
//! `impl<T, E> IntoIterator for Result<T, E>`, so `search.flatten()` compiles against that
//! exact item type and discards every terminal marker; `Result::ok` and `Result::is_ok` make
//! `.filter_map(Result::ok)` and `.take_while(Result::is_ok)` do the same. Each is a
//! one-line, reviewed-without-comment idiom that turns budget exhaustion back into a
//! truncated-but-plausible answer.
//!
//! [`SearchStep`] is a crate-owned enum, so none of those three compiles: `SearchStep` is not
//! an iterator, and neither `Result::ok` nor `Result::is_ok` accepts one. The claim is
//! visibility rather than impossibility, and only that. A hand-written
//! `filter_map(|step| match step { SearchStep::Occurrence(o) => Some(o), _ => None })`
//! discards the terminal step just as thoroughly — but it is a visible line in a diff instead
//! of an idiom. [`Iterator::count`] is the honest remainder: it counts *steps*, so an
//! exhausted search returns a number inflated by one. That is a documented hazard with a test,
//! not a wrong number a type can remove.
//!
//! # Why `key` and `start` are not interchangeable
//!
//! This is the crate's sharpest invariant. [`Occurrence::key`] is the base cadence instant:
//! what a `RECURRENCE-ID` addresses, what the merge sorts on, what `COUNT` counts, and what
//! generation walks. [`Occurrence::start`] is when the occurrence actually happens. They differ
//! exactly when an override moved it, which is what a `RANGE=THISANDFUTURE` time shift means.
//!
//! A [`Window`] therefore admits on *either*: a search emits an occurrence whose key falls in
//! the window the caller asked about **or** whose effective start does. `docs/adr/0002` fixes
//! the second half — the caller's question is about starts — and the first half is what keeps a
//! cadence the caller can address from vanishing because something moved it. Both halves are
//! the library's own work: generation runs over the asked window widened by the largest
//! absolute shift the override set implies, and the filtering back down happens inside the
//! search, so [`Window::widened`] is a tool a caller may reach for and never an obligation it
//! has to discover. An occurrence admitted by its key whose start lies outside is emitted and
//! reported on `DiagnosticCode::OverrideLeftWindow`, and [`Occurrence::starts_within`] is how a
//! caller asks the second question for itself.

use core::error::Error;
use core::fmt::{self, Display, Formatter};

use ical_core::{Instant, PropertyId};

use crate::input::{AppliedDiffs, Override, OverrideSet, PropertyChange};

/// The half-open range of cadence keys a search covers.
///
/// Half-open — `start` included, `end` excluded — because adjacent windows have to tile
/// without overlapping, and a month view asking for the first of the next month is how a
/// caller writes "the whole of this month" without knowing how long it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Window {
    /// The first instant in the window.
    start: Instant,
    /// The first instant past it.
    end: Instant,
}

impl Window {
    /// A window from `start` up to but not including `end`, or `None` when `end` is not later.
    ///
    /// An empty window is refused rather than admitted, because an empty search and a search
    /// the caller asked nothing of return the same thing and only one of them is a mistake.
    #[must_use]
    pub fn new(start: Instant, end: Instant) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    /// The first instant in the window.
    #[must_use]
    pub const fn start(self) -> Instant {
        self.start
    }

    /// The first instant past the window.
    #[must_use]
    pub const fn end(self) -> Instant {
        self.end
    }

    /// Whether `at` falls inside.
    #[must_use]
    pub fn contains(self, at: Instant) -> bool {
        self.start <= at && at < self.end
    }

    /// The same window reaching `before` seconds earlier and `after` seconds later.
    ///
    /// The remedy for the key-versus-start gap: a renderer that wants every occurrence whose
    /// *start* lands in a month searches a widened window and filters on
    /// [`Occurrence::starts_within`]. How far to widen is the largest absolute shift the
    /// override set implies, which is a number this crate does not compute for the caller.
    ///
    /// `None` when either edge leaves the representable timeline, rather than a silently
    /// clamped window that would answer a question nobody asked.
    #[must_use]
    pub fn widened(self, before: i64, after: i64) -> Option<Self> {
        let start = self.start.checked_add_seconds(before.checked_neg()?)?;
        let end = self.end.checked_add_seconds(after)?;
        Self::new(start, end)
    }
}

/// Which mechanism produced an occurrence that is not a plain rule instance.
///
/// One tag where three facts sometimes apply, and the precedence is stated so implementations
/// agree rather than because it is the only defensible order:
/// `ExactMatch > ThisAndFuture > AddedByRdate`. An `RDATE`-added instant that an exact-match
/// override also modifies reports `ExactMatch`, and its `RDATE` origin is recoverable only by
/// the caller checking the `RDATE` slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OverrideProvenance {
    /// A `RECURRENCE-ID` names this exact cadence key.
    ExactMatch,
    /// A `RANGE=THISANDFUTURE` anchor at or before this key is in force.
    ThisAndFuture {
        /// The nearest such anchor's `RECURRENCE-ID`, for reporting.
        ///
        /// The nearest one only. Application composes *every* anchor at or before the key —
        /// see [`Occurrence::applied_anchors`] — and one tag cannot name a set.
        anchor: Instant,
    },
    /// An `RDATE` added this instant; no rule generated it.
    AddedByRdate,
}

/// One occurrence of a series, with everything in force on it reachable and nothing
/// materialized.
///
/// `Copy`, borrowing the override table, so [`Occurrence::applied_anchors`] and
/// [`Occurrence::effective_change`] recompose on demand. A search over a year of a daily rule
/// therefore allocates nothing per occurrence, which is what makes `docs/adr/0007`'s
/// "every allocated byte is charged" a claim with nothing to charge here.
#[derive(Clone, Copy, Debug)]
pub struct Occurrence<'a> {
    /// The cadence key: what a `RECURRENCE-ID` addresses and what `COUNT` counts.
    key: Instant,
    /// When it actually happens, after any override moved it.
    start: Instant,
    /// Which mechanism produced it, absent for a plain rule instance.
    provenance: Option<OverrideProvenance>,
    /// The override naming this exact key, if there is one.
    exact: Option<&'a Override<'a>>,
    /// The whole override table, for anchor composition.
    overrides: OverrideSet<'a>,
}

impl<'a> Occurrence<'a> {
    /// An occurrence at `key`, starting at `start`.
    #[must_use]
    pub const fn new(
        key: Instant,
        start: Instant,
        provenance: Option<OverrideProvenance>,
        exact: Option<&'a Override<'a>>,
        overrides: OverrideSet<'a>,
    ) -> Self {
        Self {
            key,
            start,
            provenance,
            exact,
            overrides,
        }
    }

    /// The cadence key.
    #[must_use]
    pub const fn key(self) -> Instant {
        self.key
    }

    /// When the occurrence actually happens.
    #[must_use]
    pub const fn start(self) -> Instant {
        self.start
    }

    /// Which mechanism produced it, absent for a plain rule instance.
    #[must_use]
    pub const fn provenance(self) -> Option<OverrideProvenance> {
        self.provenance
    }

    /// Whether an override moved this occurrence away from its cadence key.
    #[must_use]
    pub fn is_moved(self) -> bool {
        self.start != self.key
    }

    /// How far an override moved it, absent when it did not move.
    #[must_use]
    pub fn shift_seconds(self) -> Option<i64> {
        self.key
            .checked_seconds_until(self.start)
            .filter(|shift| *shift != 0)
    }

    /// Whether the occurrence's *start* falls in `window`.
    ///
    /// Not the same question as whether the search admitted it, which was asked of the key.
    #[must_use]
    pub fn starts_within(self, window: Window) -> bool {
        window.contains(self.start)
    }

    /// Every `RANGE=THISANDFUTURE` anchor in force here, oldest first.
    #[must_use]
    pub const fn applied_anchors(self) -> AppliedDiffs<'a> {
        self.overrides.anchors_before(self.key)
    }

    /// The override naming this exact cadence key, if there is one.
    #[must_use]
    pub const fn exact_override(self) -> Option<&'a Override<'a>> {
        self.exact
    }

    /// What is in force on `id` here, composing every anchor and then the exact match.
    ///
    /// Oldest anchor to newest, then the exact override last: a `LOCATION` set by a March
    /// anchor survives a June anchor that changed only `SUMMARY`, because omission means no
    /// opinion and never revert-to-base. RFC 5545 offers no syntax for reverting a property
    /// to its base value, so that loss is documented rather than eliminated.
    #[must_use]
    pub fn effective_change(self, id: &PropertyId) -> Option<PropertyChange<'a>> {
        let mut winner = None;
        for anchor in self.applied_anchors() {
            if let Some(change) = anchor.diff().get(id) {
                winner = Some(*change);
            }
        }
        if let Some(change) = self.exact.and_then(|exact| exact.diff().get(id)) {
            winner = Some(*change);
        }
        winner
    }
}

/// A search stopped because it ran out of candidate budget.
///
/// Carries how far it got and what it spent, because "cut short at the limit" and "the rule
/// ended at `UNTIL`" must be different answers to the caller — and because a caller deciding
/// whether to retry with a larger budget needs to know it was close rather than nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BudgetExhausted {
    /// The last cadence key the search reached.
    reached: Instant,
    /// Candidates generated before the budget ran out.
    candidates_spent: u64,
}

impl BudgetExhausted {
    /// The terminal state of a search that reached `reached` having spent `candidates_spent`.
    #[must_use]
    pub const fn new(reached: Instant, candidates_spent: u64) -> Self {
        Self {
            reached,
            candidates_spent,
        }
    }

    /// The last cadence key the search reached.
    #[must_use]
    pub const fn reached(self) -> Instant {
        self.reached
    }

    /// Candidates generated before the budget ran out.
    #[must_use]
    pub const fn candidates_spent(self) -> u64 {
        self.candidates_spent
    }
}

impl Display for BudgetExhausted {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the recurrence search stopped at the candidate budget after {} candidates, \
             having reached instant {}",
            self.candidates_spent,
            self.reached.unix_seconds()
        )
    }
}

impl Error for BudgetExhausted {}

/// One step of a search: an occurrence, or the terminal state.
///
/// **Not** `Result<Occurrence, BudgetExhausted>`, and the module documentation says why at
/// length. `#[must_use]` so that a step produced and dropped without being looked at is a
/// warning at the call site, and `#[non_exhaustive]` so a future terminal state — a window
/// that ended for a reason budget exhaustion does not cover — can be added without a major
/// version.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
#[must_use]
pub enum SearchStep<'a> {
    /// An occurrence the caller asked for.
    Occurrence(Occurrence<'a>),
    /// The search stopped at the candidate budget. Nothing follows it.
    BudgetExhausted(BudgetExhausted),
}

impl<'a> SearchStep<'a> {
    /// The occurrence, if this step is one.
    ///
    /// Deliberately not named `ok`: this is the discard the module documentation calls
    /// visible-but-possible, and a name borrowed from `Result` would make
    /// `.filter_map(SearchStep::occurrence)` read like the idiom it is a re-creation of.
    #[must_use]
    pub const fn occurrence(self) -> Option<Occurrence<'a>> {
        match self {
            Self::Occurrence(occurrence) => Some(occurrence),
            Self::BudgetExhausted(_) => None,
        }
    }

    /// Whether this step is the terminal one.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::BudgetExhausted(_))
    }
}

/// Why a search is not producing more occurrences, or that it still is.
///
/// The third and weakest of the three reports of one fact, available only to a caller that
/// still holds the search by name. The other two are the terminal [`SearchStep`] and the
/// caller's own `Meter`, whose exhaustion flag latches and which outlives every combinator
/// applied to the iterator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SearchOutcome {
    /// The search has not finished.
    Pending,
    /// The rule reached its `COUNT` or its `UNTIL`.
    RuleEnded,
    /// The window ended. The rule may well continue past it.
    WindowEnded,
    /// The calendar ended: the rule would continue, and RFC 5545 cannot write where.
    ///
    /// Section 3.3.4 writes a four-digit year, so the recurrence set of a rule with no `COUNT`
    /// and no `UNTIL` stops at 9999-12-31T23:59:59 whatever the window asked for. Complete, and
    /// deliberately not [`SearchOutcome::RuleEnded`]: every instance the calendar can express
    /// was produced, and the rule nonetheless reached neither its `COUNT` nor its `UNTIL`, so a
    /// caller told "the rule ended" would be told something false about the rule.
    CalendarEnded,
    /// The candidate budget ran out. This is the only outcome that means "incomplete".
    BudgetExhausted(BudgetExhausted),
}

impl SearchOutcome {
    /// Whether the answer is complete for the window that was asked about.
    ///
    /// [`SearchOutcome::CalendarEnded`] is complete: what stopped the search is the end of the
    /// timeline RFC 5545 can name, so nothing a second search could find is missing from the
    /// answer. What it is not is a fact about the rule, which is why it is its own variant.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(
            self,
            Self::RuleEnded | Self::WindowEnded | Self::CalendarEnded
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::Instant;

    use super::{
        BudgetExhausted, Occurrence, OverrideProvenance, SearchOutcome, SearchStep, Window,
    };
    use crate::input::OverrideSet;

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    #[test]
    fn a_window_is_half_open_and_refuses_to_be_empty() {
        let window = Window::new(at(100), at(200)).unwrap();
        assert!(window.contains(at(100)));
        assert!(!window.contains(at(200)));
        assert_eq!(Window::new(at(100), at(100)), None);
    }

    #[test]
    fn widening_moves_both_edges_and_refuses_to_leave_the_timeline() {
        let window = Window::new(at(100), at(200)).unwrap();
        let wide = window.widened(50, 50).unwrap();
        assert_eq!(wide.start(), at(50));
        assert_eq!(wide.end(), at(250));
        let edge = Window::new(at(i64::MIN), at(0)).unwrap();
        assert_eq!(edge.widened(1, 0), None);
    }

    /// A window admits by key and the start is free to leave it.
    #[test]
    fn an_occurrence_moved_by_an_override_keeps_its_key_and_reports_the_gap() {
        let window = Window::new(at(0), at(100)).unwrap();
        let moved = Occurrence::new(
            at(10),
            at(500),
            Some(OverrideProvenance::ThisAndFuture { anchor: at(10) }),
            None,
            OverrideSet::empty(),
        );
        assert!(window.contains(moved.key()));
        assert!(!moved.starts_within(window));
        assert!(moved.is_moved());
        assert_eq!(moved.shift_seconds(), Some(490));
    }

    /// The hazard the type cannot remove, asserted so it stays a known one.
    ///
    /// `count()` counts steps. An exhausted search returns a number one larger than the
    /// occurrences it produced, and no item type this crate can write changes that.
    #[test]
    fn count_counts_steps_and_an_exhausted_search_inflates_it_by_one() {
        let steps: Vec<SearchStep<'_>> = alloc::vec![
            SearchStep::Occurrence(Occurrence::new(
                at(1),
                at(1),
                None,
                None,
                OverrideSet::empty()
            )),
            SearchStep::BudgetExhausted(BudgetExhausted::new(at(2), 64)),
        ];
        let occurrences = steps.iter().filter(|step| !step.is_terminal()).count();
        assert_eq!(steps.len(), 2);
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn only_budget_exhaustion_means_the_answer_is_incomplete() {
        assert!(SearchOutcome::RuleEnded.is_complete());
        assert!(SearchOutcome::WindowEnded.is_complete());
        assert!(SearchOutcome::CalendarEnded.is_complete());
        assert!(!SearchOutcome::Pending.is_complete());
        assert!(!SearchOutcome::BudgetExhausted(BudgetExhausted::new(at(0), 1)).is_complete());
    }

    /// The end of the calendar and the end of the rule are different answers.
    ///
    /// Both are complete and only one of them is a claim about the rule, so a caller asking
    /// "did this rule finish?" has to be able to tell them apart.
    #[test]
    fn the_calendar_ending_is_not_the_rule_ending() {
        assert_ne!(SearchOutcome::CalendarEnded, SearchOutcome::RuleEnded);
        assert_ne!(SearchOutcome::CalendarEnded, SearchOutcome::WindowEnded);
    }
}
