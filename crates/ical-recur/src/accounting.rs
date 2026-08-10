// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 7 — where the meter is charged and how the window is widened before it filters.
//!
//! # What this unit owns
//!
//! Two things the engine consumes and neither owns: the charging discipline, and the window
//! arithmetic that decides what a search *generates* before anything decides what it emits.
//!
//! ## Charging
//!
//! - **Per candidate generated, never per occurrence emitted.**
//!   `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` walks years between matches; a budget charged per
//!   emitted occurrence never fires on it and the search spins without producing anything.
//!   [`Meter::open_period`] and [`Meter::try_charge_candidate`] are the pair, and the second
//!   charges two ledgers at once: `Limits::candidates_per_period` for the period, and the
//!   meter's own budget — never reset — for the search and every search sharing the meter.
//! - **Inside one `next()`.** A negative `BYSETPOS` materializes a whole period before
//!   selecting from it, so `next()` must not be able to do unbounded uncharged work. The size
//!   unit 4 reports through `forced_full_period` is charged through [`Charges::candidates`]
//!   before selection, not after.
//! - **A filtered candidate still counts.** `FREQ=MONTHLY;BYMONTHDAY=31` generates a February
//!   candidate and drops it; `docs/adr/0011` says the work was done either way.
//! - **Per emitted occurrence**, against `Limits::occurrences_per_search`, which bounds what a
//!   collecting caller retains rather than what the search does.
//!
//! ## The window
//!
//! Generation admits by cadence key. A `RANGE=THISANDFUTURE` shift moves starts, in either
//! direction. So generation runs over the caller's window *widened by the largest absolute
//! shift the override slice implies, counted on the caller's own timeline* — one scan of that
//! slice before generation, which is
//! [`max_absolute_shift`] and the only place [`Override::shift_seconds`] is called — and
//! emission is ordered by effective start and filtered back to the window the caller asked
//! for, which is [`admit`]. With no time-shifting override present the widening is zero and
//! the window is unchanged.
//!
//! The two halves are one mechanism and neither works alone. Widening without filtering
//! answers a question nobody asked; filtering without widening loses an occurrence whose
//! cadence key sat outside the window and whose shifted start sits inside it, which is
//! `docs/adr/0002`'s "a window is not a simple filter on generated instants". An occurrence
//! that the widening did generate and the filter then dropped is reported once, as
//! [`DiagnosticCode::OverrideLeftWindow`] at its cadence key: a [`Severity::Note`], because
//! the specification permits it and the answer is not wrong, only smaller than the caller
//! expected.
//!
//! Two frozen doc comments describe the pre-amendment reading and disagree with this: the
//! [`crate::search`] module's "a search bounded by a window can yield an occurrence whose
//! start is outside that window", and [`DiagnosticCode::OverrideLeftWindow`]'s "the caller
//! widens the window and filters on the start when it wants the other reading". Under
//! `docs/adr/0002` the *iterator* widens and filters and the caller does neither, so those two
//! paragraphs are stale prose about shipped behavior rather than a second design. This unit
//! implements the ADR; the divergence is reported for the integrator and not routed around.
//!
//! Skew is attacker-controlled: a file may declare a shift of years and force cadence
//! generation far outside a one-month view. The candidate budget bounds that into a reported
//! outcome rather than a hang, which is `docs/adr/0002` working as designed — and a hostile
//! shift and a legitimate one are textually identical, so some honest files will be reported
//! unresolvable. That is a known cost, not a defect to engineer around here.
//!
//! # Calibrating the default candidate budget
//!
//! `docs/adr/0010` assigns the number to whoever ships the first recurrence milestone, which is
//! this one. It held 65,536 for no better reason than roundness. Two arguments land on the same
//! order of magnitude, and the measured one binds.
//!
//! The structural argument first, because it is the one that says the old number was *wrong*
//! rather than merely unmeasured: 65,536 is exactly `Limits::DEFAULT`'s
//! `candidates_per_period`. Two bounds with one value are one bound. A search that fills a
//! single maximal period has spent its entire budget, so the per-period ceiling can never
//! refuse a runaway period before the shared ledger refuses the whole search, and the second
//! dimension `docs/adr/0010` argued for buys nothing. The search budget has to be a multiple
//! of the period ceiling or the two collapse.
//!
//! The workloads, in candidates *generated*, which is what is charged:
//!
//! | workload                                          | candidates | admitted |
//! |---------------------------------------------------|------------|----------|
//! | one month of `FREQ=DAILY`                         |         31 | yes      |
//! | a decade of `FREQ=DAILY`                          |      3,653 | yes      |
//! | a year of `FREQ=HOURLY`                           |      8,760 | yes      |
//! | a decade of a half-hourly working week            |     46,980 | yes      |
//! | a month of `FREQ=MINUTELY`                        |     44,640 | yes      |
//! | a day of `FREQ=SECONDLY`                          |     86,400 | yes      |
//! | a year of `FREQ=MINUTELY`                         |    525,600 | no       |
//! | a week of `FREQ=SECONDLY`                         |    604,800 | no       |
//!
//! The half-hourly working week is
//! `FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9,10,11,12,13,14,15,16,17;BYMINUTE=0,30`: five
//! days times nine hours times two minutes is ninety candidates a week, and a decade is 522
//! weeks. It is the most expensive rule in this table that a human being actually writes.
//!
//! **262,144** — four times the period ceiling — stands above every workload a caller that
//! said nothing plausibly means and below the two that it does not: a caller who wants a year
//! of minute-by-minute expansion is stating a policy, not accepting a default. The margin is
//! three times the largest admitted workload in one direction and twice the smallest refused
//! one in the other, which is as much precision as a table with no corpus behind it earns.
//!
//! [`crate::DEFAULT_CANDIDATE_BUDGET`] carries that number, and the table above is asserted in
//! this module's tests so it is defended by something other than prose.
//!
//! # Signatures it provides
//!
//! ```text
//! pub fn max_absolute_shift(overrides: OverrideSet<'_>) -> i64;
//! pub fn generation_window(asked: Window, overrides: OverrideSet<'_>) -> Option<Window>;
//! pub fn admit<S: DiagnosticSink + ?Sized>(asked: Window, occurrence: Occurrence<'_>,
//!                                          meter: &mut Meter, sink: &mut S) -> bool;
//! pub struct Charges { /* private */ }
//! impl Charges {
//!     pub const fn new(origin: Instant, meter: &Meter) -> Self;
//!     pub const fn mark_reached(&mut self, at: Instant);
//!     pub fn open_period(&mut self, meter: &mut Meter);
//!     pub fn candidate(&mut self, meter: &mut Meter) -> Result<(), BudgetExhausted>;
//!     pub fn candidates(&mut self, count: u32, meter: &mut Meter) -> Result<(), BudgetExhausted>;
//!     pub fn occurrence(&mut self, meter: &mut Meter) -> Result<(), BudgetExhausted>;
//!     pub fn spent(&self, meter: &Meter) -> u64;
//!     pub const fn periods(&self) -> u64;
//!     pub const fn reached(&self) -> Instant;
//!     pub const fn exhausted(&self) -> Option<BudgetExhausted>;
//! }
//! ```
//!
//! [`Charges::new`] and [`Charges::spent`] take the ledger because the count they report is a
//! difference between two of its readings rather than a tally kept beside it. Unit 3 charges
//! `Meter::try_charge_candidate` directly as it fills a period — one charge site, which is
//! `docs/adr/0002` amendment 7 — so a second counter here would be blind to every candidate a
//! period paid for before being refused, and a search refused mid-period would report having
//! generated nothing after generating a period's worth.
//!
//! Four items beyond the contract's four, and each is here because the contract's own text
//! needs it. [`admit`] is the filter half of "generate widened, then filter back", and merge
//! (unit 5) explicitly leaves the window filtering and
//! [`DiagnosticCode::OverrideLeftWindow`] here. [`Charges::new`] and
//! [`Charges::mark_reached`] exist because [`BudgetExhausted`] reports the instant a search
//! reached and no charge carries one. [`Charges::candidates`] is the whole-period charge a
//! negative `BYSETPOS` needs. [`Charges::exhausted`] and [`Charges::periods`] are reads, not
//! new mechanism.
//!
//! # What this unit must not do
//!
//! - It must not mint a [`Meter`]. One arrives as `&mut Meter` so that a fan-out over five
//!   thousand searches is bounded in aggregate; minting one here would defeat
//!   `docs/adr/0010` from inside the library.
//! - It must not exempt a rule from the budget for looking obviously cheap.
//! - It must not silently clamp a widened window that leaves the timeline. [`Window::widened`]
//!   already answers `None`, and the search reports rather than narrowing the question.
//!
//! # How it is tested on its own
//!
//! A rare-match rule that emits nothing and still exhausts, which is the case a per-emission
//! budget cannot see. A negative `BYSETPOS` period charged before selection. A fan-out of many
//! searches over one meter, which must exhaust where the same searches over fresh meters do
//! not. A widening of zero when no override moves anything, and a widening equal to the
//! largest shift when one does, across a leap day, a month end and a year boundary.

use ical_core::{
    Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity, report_diagnostic,
};

use crate::input::{Override, OverrideSet};
use crate::search::{BudgetExhausted, Occurrence, Window};

/// The largest absolute time shift `overrides` implies, in seconds.
///
/// One scan of the slice, before generation, which is the whole cost the widening adds. Every
/// entry counts and not only the anchors: an override with no `RANGE` moves one start out of
/// the window it was generated in exactly as thoroughly as an anchor moves many.
///
/// Zero when nothing moves, which is the common case and the one worth being exact about — an
/// override that changes only `LOCATION` has no shift at all (`docs/adr/0002`'s DP-10), so a
/// series full of relocations widens nothing and pays nothing.
///
/// Saturating at [`i64::MAX`] rather than wrapping. A shift whose magnitude does not fit an
/// `i64` cannot produce a window that fits one either, and [`generation_window`] answers
/// `None` for it a moment later rather than generating over a window that quietly shrank.
///
/// The seconds this counts are seconds **of the timeline the caller's own instants are on**,
/// and that sentence is a correction of the one this paragraph used to make. An override
/// carries two instants and this function differences them; which timeline they name is the
/// caller's decision and not this crate's. For a floating or UTC series it is UTC, and the
/// count is elapsed seconds. For a zoned series `ical_tz::seam` puts every instant crossing
/// the seam — the `RECURRENCE-ID` and where the override moved to included — on the series'
/// own wall clock projected onto UTC, so the difference is a *wall-clock* count and the
/// widening it gives already covers the wall-clock move that is propagated to every later key.
///
/// There is therefore no timeline on which this number and `ical_tz::extra_widening` are two
/// halves of one quantity: on the nominal timeline the shortfall that function reports is
/// always zero, and on the real timeline this function is not measuring the move that gets
/// propagated. `docs/adr/0002` amendment 8 records the correction, and `ical_tz::WallClockShift`
/// is where the two readings of one move are held apart.
#[must_use]
pub fn max_absolute_shift(overrides: OverrideSet<'_>) -> i64 {
    let widest = overrides
        .entries()
        .iter()
        .copied()
        .filter_map(Override::shift_seconds)
        .map(i64::unsigned_abs)
        .max()
        .unwrap_or(0);
    i64::try_from(widest).unwrap_or(i64::MAX)
}

/// The window a search generates cadence keys over, given the window the caller asked about.
///
/// The caller's window widened by [`max_absolute_shift`] in both directions, because a shift
/// moves a start in either one: an occurrence whose key precedes the window can be pushed into
/// it, and one whose key follows the window can be pulled back into it. Generating only over
/// the asked window would lose both.
///
/// `None` when the widening leaves the representable timeline. That is the honest answer:
/// clamping would generate over a window that answers a different question than the one asked,
/// and the caller would have no way to know which question it got.
///
/// The identity case is exact rather than incidental — with no time-shifting override the
/// window returned *is* the window asked about, so a caller can compare the two and learn
/// whether any widening happened at all.
#[must_use]
pub fn generation_window(asked: Window, overrides: OverrideSet<'_>) -> Option<Window> {
    let widest = max_absolute_shift(overrides);
    if widest == 0 {
        return Some(asked);
    }
    asked.widened(widest, widest)
}

/// Whether `asked` keeps `occurrence`, reporting one that its cadence key had promised.
///
/// The filter half of the widening. An occurrence is kept when its *effective start* falls in
/// the window the caller asked about, which is the question `docs/adr/0002` says a window
/// asks; generation asked the other question, of the cadence key, over the widened window
/// [`generation_window`] returned.
///
/// Three shapes arrive here and they are not symmetric. An occurrence the caller asked for is
/// kept silently. An occurrence generated only because of the widening — its key outside the
/// asked window — is dropped silently, because the caller never asked about its key and
/// nothing it expected has gone missing. An occurrence whose key *is* inside the asked window
/// and whose start is not is dropped and reported with [`DiagnosticCode::OverrideLeftWindow`]
/// at that key, because that is the one a caller can see the absence of: it counted the
/// cadence and got one fewer.
///
/// Ordering is not this function's business. Emission is ordered by effective start, which
/// needs the buffer the engine owns; this decides membership only.
#[must_use]
pub fn admit<S>(asked: Window, occurrence: Occurrence<'_>, meter: &mut Meter, sink: &mut S) -> bool
where
    S: DiagnosticSink + ?Sized,
{
    if occurrence.starts_within(asked) {
        return true;
    }
    if asked.contains(occurrence.key()) {
        let left = Diagnostic::at_instant(
            DiagnosticCode::OverrideLeftWindow,
            Severity::Note,
            occurrence.key(),
        );
        report_diagnostic(sink, meter, left);
    }
    false
}

/// One search's charging discipline, and the terminal state it latches.
///
/// Neither `Copy` nor `Clone`, for the reason [`Meter`] is neither: a ledger that copies on use
/// is a ledger that stops binding at a call site nobody can see it happen at. It holds no meter
/// of its own and mints none — the caller's arrives as `&mut Meter` at every charge, which is
/// what keeps a fan-out over five thousand searches bounded in aggregate rather than five
/// thousand times individually.
///
/// It latches its own terminal state as well as reading the meter's. That is not redundant:
/// this one carries the instant the search had reached and the count it had spent, which the
/// meter has no field for. The meter latches too — every bound it keeps sets its own exhaustion
/// flag, `Limits::candidates_per_period` included — so the two agree about *whether* a search
/// stopped short and only this one can say where.
///
/// What it cannot report is *which* dimension ran out, because [`BudgetExhausted`] has no field
/// for one. A caller that needs to tell a period ceiling from a shared budget from an
/// occurrence bound reads its own `Limits` against `Meter::candidates_in_period` and
/// `Meter::occurrences`.
#[derive(Debug)]
pub struct Charges {
    /// The furthest cadence key the search has reached, for the terminal report.
    reached: Instant,
    /// What `meter.candidates_charged()` read when this search began.
    opened_at: u64,
    /// Periods this search has opened, which is the number that tunes a period ceiling.
    periods: u64,
    /// The terminal state, once there is one. Latched.
    terminal: Option<BudgetExhausted>,
}

impl Charges {
    /// A fresh set of charges for a search positioned at `origin`, over `meter`.
    ///
    /// `origin` is where the search reports having reached before it has reached anything —
    /// its `DTSTART`, or the resume point a `SearchCursor` carried. A terminal state produced
    /// before the first [`Charges::mark_reached`] names that instant, which is truthful: the
    /// search got no further.
    ///
    /// The meter is read rather than charged. A search's own cost is the difference between
    /// the ledger's candidate count now and when it stops, and taking it that way is what makes
    /// the number true whichever code path did the charging: unit 3 charges the meter directly
    /// as it fills a period, so a count kept here in parallel would miss exactly the candidates
    /// a refused period had already paid for.
    #[must_use]
    pub const fn new(origin: Instant, meter: &Meter) -> Self {
        Self {
            reached: origin,
            opened_at: meter.candidates_charged(),
            periods: 0,
            terminal: None,
        }
    }

    /// Record that the search has reached `at`.
    ///
    /// Called as each period opens or each candidate is generated, whichever the engine can do
    /// without a second computation. It is what makes "cut short at the limit" a different
    /// answer from "the rule ended at `UNTIL`" for a caller deciding whether to retry with a
    /// larger budget.
    pub const fn mark_reached(&mut self, at: Instant) {
        self.reached = at;
    }

    /// Begin a period, clearing the per-period candidate count on `meter`.
    ///
    /// Infallible on purpose. Opening a period costs nothing; it is the candidates inside it
    /// that cost, and a period boundary that could fail would give the engine a second place to
    /// terminate for one reason.
    pub fn open_period(&mut self, meter: &mut Meter) {
        self.periods = self.periods.saturating_add(1);
        meter.open_period();
    }

    /// Charge one candidate, generated rather than emitted.
    ///
    /// Generated: a candidate dropped for naming February 30th is charged like any other,
    /// because the arithmetic that produced it and discovered it was nonexistent ran either
    /// way (`docs/adr/0011`).
    pub fn candidate(&mut self, meter: &mut Meter) -> Result<(), BudgetExhausted> {
        if let Some(terminal) = self.terminal {
            return Err(terminal);
        }
        if meter.try_charge_candidate().is_err() {
            return Err(self.latch(meter));
        }
        Ok(())
    }

    /// Charge a whole period's candidate set, before anything selects from it.
    ///
    /// The `BYSETPOS` shape. A negative position forces the entire period to exist before
    /// position `-1` can be known, so the size unit 4 reports through `forced_full_period` is
    /// charged here and not after the selection has thrown most of it away. Charging the
    /// selection instead would let `FREQ=YEARLY;BYHOUR=...;BYMINUTE=...;BYSECOND=...` do a
    /// year of work inside one `next()` and charge one candidate for it.
    ///
    /// One charge at a time rather than one arithmetic, so that the period ceiling refuses at
    /// exactly the candidate that crosses it and the count in the terminal report is the number
    /// of candidates that were actually paid for.
    pub fn candidates(&mut self, count: u32, meter: &mut Meter) -> Result<(), BudgetExhausted> {
        for _ in 0..count {
            self.candidate(meter)?;
        }
        Ok(())
    }

    /// Charge one emitted occurrence against `Limits::occurrences_per_search`.
    ///
    /// A different cost from a candidate and never a substitute for one: this bounds what a
    /// collecting caller retains, and a rule that matches rarely spends nothing here while
    /// spending everything above.
    pub fn occurrence(&mut self, meter: &mut Meter) -> Result<(), BudgetExhausted> {
        if let Some(terminal) = self.terminal {
            return Err(terminal);
        }
        if meter.try_charge_occurrence().is_err() {
            return Err(self.latch(meter));
        }
        Ok(())
    }

    /// Candidates this search has paid for, read off the ledger they were paid to.
    #[must_use]
    pub fn spent(&self, meter: &Meter) -> u64 {
        meter.candidates_charged().saturating_sub(self.opened_at)
    }

    /// Periods this search has opened.
    #[must_use]
    pub const fn periods(&self) -> u64 {
        self.periods
    }

    /// The furthest cadence key the search has reached.
    #[must_use]
    pub const fn reached(&self) -> Instant {
        self.reached
    }

    /// The terminal state, once the search has one.
    ///
    /// Latched, so a caller that ignored one refusal cannot charge its way back into a clean
    /// answer, and so the answer survives a period ceiling that never touched the meter's own
    /// exhaustion flag.
    #[must_use]
    pub const fn exhausted(&self) -> Option<BudgetExhausted> {
        self.terminal
    }

    /// Record and return the terminal state for a refused charge.
    fn latch(&mut self, meter: &Meter) -> BudgetExhausted {
        let terminal = BudgetExhausted::new(self.reached, self.spent(meter));
        self.terminal = Some(terminal);
        terminal
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{Diagnostic, DiagnosticCode, Instant, Limits, Meter, Severity};

    use super::{Charges, admit, generation_window, max_absolute_shift};
    use crate::input::{Override, OverrideRange, OverrideSet, PropertyDiff};
    use crate::search::{Occurrence, OverrideProvenance, Window};

    /// 2023-01-31T00:00:00Z, a month end in a month the next one is shorter than.
    const JAN_31_2023: i64 = 1_675_123_200;
    /// 2023-02-28T00:00:00Z, the end of the shorter month.
    const FEB_28_2023: i64 = 1_677_542_400;
    /// 2023-12-31T00:00:00Z, the last day of a year.
    const DEC_31_2023: i64 = 1_703_980_800;
    /// 2024-01-01T00:00:00Z, the first day of the next one.
    const JAN_1_2024: i64 = 1_704_067_200;
    /// 2024-02-01T00:00:00Z, the start of a leap February.
    const FEB_1_2024: i64 = 1_706_745_600;
    /// 2024-02-29T00:00:00Z, the leap day itself.
    const FEB_29_2024: i64 = 1_709_164_800;
    /// 2024-03-01T00:00:00Z, the first instant past that February.
    const MAR_1_2024: i64 = 1_709_251_200;

    /// One day, which is what most `RANGE=THISANDFUTURE` edits move an instance by.
    const ONE_DAY: i64 = 86_400;
    /// The 29 days from the start of a leap February to the start of March.
    const LEAP_FEBRUARY: i64 = 2_505_600;
    /// The 28 days from January 31st to February 28th.
    const SHORT_MONTH: i64 = 2_419_200;

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// One override addressing `key`, moved to `to` when it moved at all.
    ///
    /// The diff is empty throughout: this unit reads a shift and nothing else, and an override
    /// with an empty diff and no move is exactly the relocation-only shape whose widening must
    /// be zero.
    fn moved(key: i64, to: Option<i64>) -> Override<'static> {
        Override::new(
            at(key),
            OverrideRange::ThisAndFuture,
            to.map(at),
            PropertyDiff::empty(),
        )
    }

    /// The window a month view of that leap February asks about.
    fn leap_february() -> Window {
        Window::new(at(FEB_1_2024), at(MAR_1_2024)).unwrap()
    }

    /// A rule that emits nothing at all still spends the budget, and only a per-candidate
    /// charge sees it.
    ///
    /// `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` produces a candidate in every period and an
    /// instance in none. Walked with a budget charged per emitted occurrence it would run to
    /// the end of the representable calendar having charged exactly zero; charged per candidate
    /// it stops at the budget and says where it got to.
    #[test]
    fn a_rule_that_emits_nothing_exhausts_a_budget_no_emission_would_ever_spend() {
        const BUDGET: u64 = 64;
        let mut ledger = Meter::with_budget(Limits::DEFAULT, BUDGET);
        let mut charges = Charges::new(at(FEB_29_2024), &ledger);
        let mut walked = 0_u64;
        let mut terminal = None;

        for period in 0..4096_i64 {
            charges.open_period(&mut ledger);
            charges.mark_reached(at(
                FEB_29_2024.saturating_add(period.saturating_mul(ONE_DAY))
            ));
            if let Err(stopped) = charges.candidate(&mut ledger) {
                terminal = Some(stopped);
                break;
            }
            walked = walked.saturating_add(1);
        }

        let stopped = terminal.unwrap();
        assert_eq!(walked, BUDGET, "the walk stopped where the budget did");
        assert_eq!(stopped.candidates_spent(), BUDGET);
        assert_eq!(charges.spent(&ledger), BUDGET);
        assert_eq!(
            ledger.occurrences(),
            0,
            "nothing was ever emitted, so a per-emission bound never fired"
        );
        assert!(
            charges.periods() > BUDGET,
            "one period per candidate and a match in none of them: no single period was \
             expensive, so only the ledger that never resets could refuse this"
        );
        assert_eq!(
            charges.exhausted(),
            Some(stopped),
            "the terminal state latches and outlives the step that produced it"
        );
    }

    /// A negative `BYSETPOS` is charged for the set it forces into existence, before selection.
    ///
    /// `FREQ=YEARLY;BYHOUR=...;BYMINUTE=...;BYSECOND=...;BYSETPOS=-1` fills a year before
    /// position `-1` can be known. Charging the one selected instance instead would price a
    /// year of work at one candidate, which the second half of this test shows costing nothing
    /// at all.
    ///
    /// The refusal is also asserted to reach the caller's own ledger. A period ceiling refuses
    /// before the octet budget is touched, and for a while that meant a search terminated by it
    /// left `Meter::is_exhausted` reading clean — the second of `docs/adr/0002`'s three reports
    /// contradicting the first. It latches now, and the assertion is here rather than only in
    /// `ical-conform` because this file is where the earlier claim was written down.
    #[test]
    fn a_negative_by_set_pos_period_is_charged_before_selection_and_not_after() {
        const CEILING: u32 = 1024;
        let limits = Limits::DEFAULT.with_candidates_per_period(CEILING);
        let mut ledger = Meter::with_budget(limits, 1_000_000);
        let mut charges = Charges::new(at(JAN_1_2024), &ledger);

        charges.open_period(&mut ledger);
        let stopped = charges.candidates(4096, &mut ledger).unwrap_err();

        assert_eq!(stopped.candidates_spent(), u64::from(CEILING));
        assert_eq!(ledger.candidates_in_period(), CEILING);
        assert_eq!(
            ledger.occurrences(),
            0,
            "the period was refused while filling, so nothing reached selection"
        );
        assert!(
            ledger.is_exhausted(),
            "a period ceiling ends the search, and the report that outlives every combinator \
             has to say so"
        );

        // What a charge levied after selection would have cost for the same period.
        let mut after = Meter::with_budget(limits, 1_000_000);
        let mut per_emission = Charges::new(at(JAN_1_2024), &after);
        per_emission.open_period(&mut after);
        assert_eq!(per_emission.occurrence(&mut after), Ok(()));
        assert_eq!(
            per_emission.spent(&after),
            0,
            "one emitted instance, and a year of candidates charged to nobody"
        );
    }

    /// One meter across a fan-out binds where a meter per search does not.
    ///
    /// `docs/adr/0010`'s whole argument in one test: eight searches each individually inside
    /// their bound, and the total is whatever the attacker chose eight to be — unless the
    /// ledger is shared. The second half mints a meter per search deliberately, which is the
    /// amplification the ADR says the library can make visible and cannot make impossible.
    #[test]
    fn a_fan_out_over_one_ledger_exhausts_where_a_ledger_per_search_never_would() {
        const SEARCHES: u32 = 8;
        const PER_SEARCH: u32 = 100;
        const BUDGET: u64 = 512;

        let mut ledger = Meter::with_budget(Limits::DEFAULT, BUDGET);
        let mut completed = 0_u32;
        for _ in 0..SEARCHES {
            let mut charges = Charges::new(at(JAN_1_2024), &ledger);
            charges.open_period(&mut ledger);
            if charges.candidates(PER_SEARCH, &mut ledger).is_ok() {
                completed = completed.saturating_add(1);
            }
        }
        assert_eq!(
            completed, 5,
            "512 candidates buys five whole searches of 100"
        );
        assert!(ledger.is_exhausted());

        let mut finished = 0_u32;
        for _ in 0..SEARCHES {
            let mut fresh = Meter::with_budget(Limits::DEFAULT, BUDGET);
            let mut charges = Charges::new(at(JAN_1_2024), &fresh);
            charges.open_period(&mut fresh);
            if charges.candidates(PER_SEARCH, &mut fresh).is_ok() {
                finished = finished.saturating_add(1);
            }
        }
        assert_eq!(
            finished, SEARCHES,
            "eight individually bounded searches, unbounded in aggregate"
        );
    }

    /// The widening is the largest absolute shift, and it is zero when nothing moves.
    ///
    /// The dates are chosen where date arithmetic goes wrong: a leap day, a month end followed
    /// by a shorter month, and a year boundary. The widening itself is second arithmetic, but
    /// the window it produces is what the period walk is then driven over, so an off-by-a-day
    /// here is an off-by-a-period there.
    #[test]
    fn the_widening_is_the_largest_absolute_shift_and_zero_when_nothing_moves() {
        let mut ledger = Meter::new(Limits::DEFAULT);
        let asked = leap_february();

        let none: [Override<'static>; 0] = [];
        let relocation = [moved(FEB_29_2024, None)];
        let forward = [moved(FEB_29_2024, Some(MAR_1_2024))];
        let backward = [moved(MAR_1_2024, Some(FEB_1_2024))];
        let year_boundary = [moved(DEC_31_2023, Some(JAN_1_2024))];
        let month_end = [moved(JAN_31_2023, Some(FEB_28_2023))];
        let largest_first = [
            moved(FEB_1_2024, Some(MAR_1_2024)),
            moved(FEB_29_2024, Some(FEB_29_2024.saturating_add(3600))),
        ];

        // What the override slice is, and the widening it implies in seconds.
        let cases = [
            ("a series with no overrides at all", &none[..], 0),
            (
                "a THISANDFUTURE override that moved nothing, only a property",
                &relocation[..],
                0,
            ),
            (
                "a leap-day instance moved a day forward",
                &forward[..],
                ONE_DAY,
            ),
            (
                "a March instance moved back across the leap day",
                &backward[..],
                LEAP_FEBRUARY,
            ),
            (
                "an instance moved across a year boundary",
                &year_boundary[..],
                ONE_DAY,
            ),
            (
                "a month end moved to the end of a shorter month",
                &month_end[..],
                SHORT_MONTH,
            ),
            (
                "two overrides, the larger of them first",
                &largest_first[..],
                LEAP_FEBRUARY,
            ),
        ];

        for (shape, entries, expected) in cases {
            let set = OverrideSet::new(entries, &mut ledger).unwrap();
            assert_eq!(max_absolute_shift(set), expected, "{shape}");

            let generated = generation_window(asked, set).unwrap();
            assert_eq!(
                generated.start(),
                at(FEB_1_2024.saturating_sub(expected)),
                "{shape}"
            );
            assert_eq!(
                generated.end(),
                at(MAR_1_2024.saturating_add(expected)),
                "{shape}"
            );
            assert_eq!(
                generated == asked,
                expected == 0,
                "{shape}: the window is unchanged exactly when nothing moved"
            );
        }
    }

    /// A widening that leaves the timeline is refused, never clamped.
    #[test]
    fn a_widening_that_leaves_the_timeline_is_refused_rather_than_clamped() {
        let mut ledger = Meter::new(Limits::DEFAULT);
        let entries = [moved(0, Some(i64::MAX))];
        let set = OverrideSet::new(&entries, &mut ledger).unwrap();
        assert_eq!(max_absolute_shift(set), i64::MAX);

        let edge = Window::new(at(i64::MIN), at(0)).unwrap();
        assert_eq!(
            generation_window(edge, set),
            None,
            "a window that cannot be widened answers None rather than a narrower question"
        );
    }

    /// The filter half: kept by start, and reported only where the caller could miss it.
    #[test]
    fn a_start_that_left_the_window_is_dropped_and_reported_at_its_cadence_key() {
        let asked = leap_february();
        let mut ledger = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let anchor = OverrideProvenance::ThisAndFuture {
            anchor: at(FEB_29_2024),
        };

        // What the occurrence is, its cadence key, its effective start, and whether it stays.
        let cases = [
            (
                "a plain instance inside the window",
                FEB_29_2024,
                FEB_29_2024,
                true,
            ),
            (
                "a key inside the window whose start was pushed past the end",
                FEB_29_2024,
                MAR_1_2024,
                false,
            ),
            (
                "a key the widening reached whose start was pulled inside",
                MAR_1_2024,
                FEB_29_2024,
                true,
            ),
            (
                "a key the widening reached whose start stayed outside",
                MAR_1_2024,
                MAR_1_2024,
                false,
            ),
        ];

        for (shape, key, start, kept) in cases {
            let occurrence =
                Occurrence::new(at(key), at(start), Some(anchor), None, OverrideSet::empty());
            assert_eq!(
                admit(asked, occurrence, &mut ledger, &mut sink),
                kept,
                "{shape}"
            );
        }

        assert_eq!(
            sink.len(),
            1,
            "only the occurrence whose key the caller had counted is reported"
        );
        let left = sink.first().unwrap();
        assert_eq!(left.code(), DiagnosticCode::OverrideLeftWindow);
        assert_eq!(left.severity(), Severity::Note);
        assert_eq!(left.instant(), Some(at(FEB_29_2024)));
    }

    /// The calibration this milestone owes, held against the workloads it was derived from.
    ///
    /// The workloads are read against the shipped constant rather than a copy of it, so a
    /// later edit to the number has to face this table. The two assertions at the end are the
    /// structural argument: a search budget equal to the period ceiling is one bound wearing
    /// two names.
    #[test]
    fn the_calibrated_search_budget_covers_the_workloads_it_was_measured_against() {
        const CALIBRATED: u64 = crate::DEFAULT_CANDIDATE_BUDGET;

        // What a caller is expanding, the candidates it generates, and whether a caller that
        // stated no policy of its own gets it.
        let workloads = [
            ("one month of FREQ=DAILY", 31_u64, true),
            ("a decade of FREQ=DAILY", 3_653, true),
            ("a year of FREQ=HOURLY", 8_760, true),
            ("a decade of a half-hourly working week", 46_980, true),
            ("a month of FREQ=MINUTELY", 44_640, true),
            ("a day of FREQ=SECONDLY", 86_400, true),
            ("a year of FREQ=MINUTELY", 525_600, false),
            ("a week of FREQ=SECONDLY", 604_800, false),
        ];

        for (shape, candidates, admitted) in workloads {
            assert_eq!(candidates <= CALIBRATED, admitted, "{shape}");
        }

        let ceiling = u64::from(Limits::DEFAULT.candidates_per_period());
        assert!(
            CALIBRATED > ceiling,
            "a search that cannot outlive one maximal period makes the period ceiling dead"
        );
        assert_eq!(
            CALIBRATED,
            ceiling.saturating_mul(4),
            "four maximal periods, which is where the workload table and the ratio agree"
        );
    }
}
