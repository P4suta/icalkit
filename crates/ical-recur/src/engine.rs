// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 6 — the search itself: the iterator, the cursor, and the two entry points.
//!
//! # What this unit owns
//!
//! [`RecurrenceSearch`], the `Iterator` whose `Item` is [`SearchStep`], and the two inherent
//! methods [`RecurrenceInput::search`] and [`RecurrenceInput::resume`]. It drives unit 2's
//! period walk, unit 3's expansion, unit 4's selection and unit 5's merge, charging through
//! unit 7 at every point unit 7 names. It owns [`SearchCursor`] and the opaque
//! [`RuleCursorState`] inside it.
//!
//! # The drive protocol, stated because two units share it
//!
//! The rule half and the merge half meet here, and the handshake between them is not visible
//! from either side alone. This unit offers the merge the *head* of the rule stream rather
//! than handing it over: `Merge::step` is called with the next cadence key the rule generated
//! and answers with the next occurrence in cadence order, which may be an earlier `RDATE`
//! instead. Which of the two the step consumed is [`Merge::takes_rule_key`]'s answer, asked
//! *before* the step because the step moves the `RDATE` cursor that answer reads, and the offer
//! is retired on that answer alone. Whether anything is left to merge is
//! [`Merge::is_drained`]'s answer, also asked before the step, because `Merge::step` returns
//! `None` both for a candidate an `EXDATE` removed and for no candidate at all — an exclusion
//! landing on the last instance of a series and the end of the series are otherwise the same
//! silence. Inferring either fact from the step's own answer loses occurrences in both
//! directions: an offered rule key that an earlier `RDATE` preempted is generated once and
//! retired anyway, and an `RDATE` tail whose head was excluded is read as the end of
//! everything. Every turn either emits an occurrence, consumes an addition or advances the rule
//! stream, which is what keeps `next` from spinning.
//!
//! # A window is not a simple filter on generated instants
//!
//! Generation runs over the caller's window widened by the largest absolute shift the override
//! set implies — unit 7's `generation_window` — and emission asks two questions rather than
//! one. An occurrence is emitted when the window the caller asked about contains its cadence
//! key **or** contains its effective start. The first half is what `crate::search` fixes: a
//! window admits by cadence key, so a `RANGE=THISANDFUTURE` shift can carry a start out of the
//! window that generated it, and [`DiagnosticCode::OverrideLeftWindow`] says so rather than the
//! search hiding it. The second half is why the widening exists at all: an override that moved
//! an occurrence *into* the caller's window from a key outside it has to appear, and a filter
//! stated only on keys would lose it. With no time-shifting override present the two questions
//! have one answer and the widening is zero.
//!
//! The stop rule follows from the same fact. The merge is ordered by cadence key and the
//! generation window is already widened by the largest shift there is, so the first key at or
//! past that window's end proves nothing after it can reach the caller's window.
//!
//! # What this unit does not decide
//!
//! - Precedence between an `EXDATE`, an override and an `RDATE` is unit 5's, and nothing here
//!   re-derives it. This unit filters by window and charges; it does not adjudicate.
//! - Where a charge happens is unit 7's. The two sites this unit calls are opening a period and
//!   emitting an occurrence; candidates are charged inside unit 3's expansion, which runs to
//!   completion before unit 4 selects from it, so "charged before selection" holds by
//!   construction rather than by a second charge here. The candidate *count* this unit keeps is
//!   bookkeeping for the terminal step's report — counting is not charging.
//! - Whether a `DTSTART` that does not match its own rule is an instance is unit 3's answer.
//!   RFC 5545 section 3.8.5.3 calls the recurrence set of an unsynchronized `DTSTART`
//!   undefined, and a second opinion here would eventually disagree with the first.
//!
//! # `UNTIL` is compared on the timeline the caller resolved
//!
//! RFC 5545 section 3.3.10 requires `UNTIL` to be a `DATE` when `DTSTART` is a `DATE` and a UTC
//! `DATE-TIME` when `DTSTART` is a zoned `DATE-TIME`, and real files violate that constantly.
//! The comparison still has to happen in a named clock, and the name is the caller's: both
//! instants arrive already resolved, so the comparison is on the UTC timeline and
//! [`crate::rule::UntilClock`] records which reading produced each. A disagreement about `DATE`
//! versus `DATE-TIME` is reported once, when the search is built, as
//! [`DiagnosticCode::RecurrenceUntilValueTypeMismatch`] — the earliest point at which both
//! halves of the comparison are in one hand.
//!
//! # Departures from the sketch this unit was contracted against
//!
//! - `RecurrenceSearch` carries a second, defaulted type parameter for the sink.
//!   `RecurrenceSearch<'a>` still names the erased form, and a caller that wants no vtable —
//!   the fixed-capacity sink on a target with no heap that `docs/design/ical-recur-api.md`
//!   argues for — gets the monomorphized one for free, because the parameter is inferred from
//!   the argument. A struct with no parameter at all could not hold a `&mut S` at all.
//! - `Charges::new` is called and is not in unit 7's published signature list; a ledger with no
//!   constructor cannot be started.

use core::fmt::{self, Formatter};
use core::iter::FusedIterator;

use ical_core::{
    CivilDateTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity, UtcOffset,
    report_diagnostic,
};

use crate::accounting::{Charges, generation_window};
use crate::byparts::expand_period;
use crate::input::RecurrenceInput;
use crate::merge::Merge;
use crate::period::{Period, PeriodWalk};
use crate::rule::{RecurrenceRule, RuleLimit};
use crate::search::{BudgetExhausted, Occurrence, SearchOutcome, SearchStep, Window};
use crate::setpos::{SelectedCandidates, select};

/// How far into an expansion a search had got, as a value the caller can hold and hand back.
///
/// Opaque, with no accessor and no constructor of its own: this is a position in an algorithm
/// and not a fact about the calendar. A caller obtains one from [`RecurrenceSearch::cursor`]
/// and returns it to [`RecurrenceInput::resume`], and that is the whole contract. It is
/// deliberately not serializable — freezing its encoding would freeze the expansion algorithm,
/// and this crate's purity gate, which admits no `serde`, is a help rather than an obstacle
/// here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleCursorState {
    /// The period the walk was inside, counted from the one holding `DTSTART`.
    period_index: u64,
}

/// Where a search stopped, so that another can carry on from it.
///
/// The counter is the load-bearing field. A `COUNT`-bounded rule describes a recurrence set
/// relative to `DTSTART`, so a resumed search that started counting again from the resume point
/// would produce a different set than the file describes — the same rule, expanded twice,
/// yielding more occurrences than it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchCursor {
    /// Cadence keys at or before this one belong to the search that produced this cursor.
    resume_after: Instant,
    /// Instances the rule has produced, which is what `COUNT` counts.
    occurrences_emitted: u32,
    /// Where the expansion algorithm had got to.
    rule_cursor: RuleCursorState,
}

impl SearchCursor {
    /// A cursor resuming after `resume_after`, having already produced `occurrences_emitted`.
    #[must_use]
    pub const fn new(
        resume_after: Instant,
        occurrences_emitted: u32,
        rule_cursor: RuleCursorState,
    ) -> Self {
        Self {
            resume_after,
            occurrences_emitted,
            rule_cursor,
        }
    }

    /// The last cadence key the search that produced this cursor reached.
    #[must_use]
    pub const fn resume_after(self) -> Instant {
        self.resume_after
    }

    /// Instances the rule has produced, which is what `COUNT` counts.
    ///
    /// Instances rather than steps a caller saw: an instance a window declined to show still
    /// counts, because RFC 5545 section 3.3.10 counts the recurrence set and not the view.
    #[must_use]
    pub const fn occurrences_emitted(self) -> u32 {
        self.occurrences_emitted
    }

    /// Where the expansion algorithm had got to.
    #[must_use]
    pub const fn rule_cursor(self) -> RuleCursorState {
        self.rule_cursor
    }
}

/// The rule and the walk over it, absent for an input that is only `RDATE`s.
struct Cadence<'a> {
    /// The rule being expanded.
    rule: &'a RecurrenceRule,
    /// `DTSTART` in civil fields, which is what the walk and the expansion take.
    dtstart: CivilDateTime,
    /// The period walk this search is stepping.
    walk: PeriodWalk,
}

impl<'a> Cadence<'a> {
    /// The walk `input` needs, or `None` when there is no rule to walk.
    fn open(input: RecurrenceInput<'a>, cursor: Option<SearchCursor>) -> Option<Self> {
        let rule = input.rule()?;
        // The caller resolved `DTSTART` onto the timeline, and the expansion works in civil
        // fields; the conversion is at UTC because the zone was already applied by whoever
        // resolved it (`docs/adr/0003`).
        let dtstart = CivilDateTime::from_instant(input.dtstart(), UtcOffset::UTC)?;
        let walk = match cursor {
            Some(held) => PeriodWalk::resume_at(dtstart, rule, held.rule_cursor().period_index),
            None => PeriodWalk::new(dtstart, rule),
        };
        Some(Self {
            rule,
            dtstart,
            walk,
        })
    }
}

/// What the rule half of a search answered when it was asked for its next cadence key.
///
/// Four answers rather than an `Option`, because three of them are different reasons to stop
/// and only one of the three means the answer is incomplete.
#[derive(Clone, Copy, Debug)]
enum RuleKey {
    /// The next cadence key the rule generates.
    Next(Instant),
    /// The rule ended, at its `COUNT` or at its `UNTIL`.
    RuleEnded,
    /// The walk reached the end of the window generation runs over.
    WindowEnded,
    /// The walk reached the end of the calendar RFC 5545 section 3.3.4 can write.
    ///
    /// Not the same answer as [`RuleKey::RuleEnded`], and reported separately because the rule
    /// did not end: a series with no `COUNT` and no `UNTIL` runs out of years to be written in
    /// long before it runs out of instances, and telling a caller that its rule finished would
    /// be telling it something false about the rule.
    CalendarEnded,
    /// The candidate budget ran out.
    Exhausted,
}

/// What the rule stream borrows from the search for the length of one step.
///
/// A bundle rather than five arguments, because the stream needs all five together and the
/// workspace bounds a function at seven.
struct StepEnv<'e, S: DiagnosticSink + ?Sized> {
    /// The window generation runs over.
    generation: Window,
    /// Where this search charges.
    charges: &'e mut Charges,
    /// The caller's ledger.
    meter: &'e mut Meter,
    /// Where diagnostics go.
    sink: &'e mut S,
}

/// The rule half of a search: periods in, cadence keys out.
struct RuleStream<'a> {
    /// The rule and its walk, absent for an input that is only `RDATE`s.
    cadence: Option<Cadence<'a>>,
    /// The candidates unit 4 selected from the period currently open.
    selected: Option<SelectedCandidates>,
    /// How far through those candidates this stream has read.
    position: usize,
    /// Instances the rule has produced, which is what `COUNT` counts.
    produced: u32,
    /// `DTSTART`: the first instant the recurrence set can hold.
    begins_at: Instant,
    /// Cadence keys at or before this one belong to an earlier search.
    resume_after: Option<Instant>,
    /// Whether the rule's own end has been reached.
    ended: bool,
    /// The whole input, for the caller's own gate on a key.
    input: RecurrenceInput<'a>,
}

impl<'a> RuleStream<'a> {
    /// The stream `input` describes, resumed from `cursor` when there is one.
    fn new(input: RecurrenceInput<'a>, cursor: Option<SearchCursor>) -> Self {
        Self {
            cadence: Cadence::open(input, cursor),
            selected: None,
            position: 0,
            produced: cursor.map_or(0, SearchCursor::occurrences_emitted),
            begins_at: input.dtstart(),
            resume_after: cursor.map(SearchCursor::resume_after),
            ended: false,
            input,
        }
    }

    /// The next cadence key, or why there is not one.
    fn next_key<S: DiagnosticSink + ?Sized>(&mut self, env: &mut StepEnv<'_, S>) -> RuleKey {
        loop {
            if self.ended {
                return RuleKey::RuleEnded;
            }
            let answer = match self.take_candidate() {
                Some(key) => self.weigh(key, env.generation),
                None => self.open_next_period(env),
            };
            if let Some(stop) = answer {
                return stop;
            }
        }
    }

    /// The next candidate of the period already open, if that period has one left.
    ///
    /// A candidate that leaves the representable timeline is skipped rather than clamped, for
    /// the reason `docs/adr/0011` gives about instances that do not exist: a nearby answer is
    /// not the answer. RFC 5545 section 3.3.4 bounds a year at four digits, so no expansion of
    /// a legal rule reaches this; it is handled because the conversion can say so and an
    /// `unwrap` here would be a claim about unit 3 that this unit is not entitled to make.
    fn take_candidate(&mut self) -> Option<Instant> {
        loop {
            let at = *self.selected.as_ref()?.as_slice().get(self.position)?;
            self.position = self.position.saturating_add(1);
            if let Some(key) = at.at_offset(UtcOffset::UTC) {
                return Some(key);
            }
        }
    }

    /// Whether `key` is this search's next key, and `None` when it is one to skip.
    ///
    /// The rule's own end is asked about before the window's, so that a series whose last
    /// instance coincides with the window's edge reports that it ended rather than that the
    /// view did. Both answers are complete; only one of them tells the caller not to ask again.
    fn weigh(&mut self, key: Instant, generation: Window) -> Option<RuleKey> {
        if key < self.begins_at {
            // The period holding `DTSTART` is expanded whole, so it offers candidates before
            // `DTSTART` as readily as after it: `FREQ=MONTHLY;BYMONTHDAY=1,-1` from the 30th of
            // September names the 1st of that same September. RFC 5545 section 3.8.5.3 begins
            // every recurrence set at `DTSTART`, so those are not instances and they are
            // skipped without counting — a `COUNT` that spent one on them would end the series
            // an instance early, which is exactly what the section's own worked example shows.
            // Skipped here rather than in the expansion, because `BYSETPOS` selects from the
            // period as the rule describes it and only then does the set begin.
            return None;
        }
        if self.resume_after.is_some_and(|after| key <= after) {
            // Already produced by the search that handed over the cursor, and already counted
            // in the `produced` this stream started from, so it is skipped without counting.
            return None;
        }
        if self.has_ended_at(key) {
            self.ended = true;
            return Some(RuleKey::RuleEnded);
        }
        if key >= generation.end() {
            return Some(RuleKey::WindowEnded);
        }
        if !self.input.admits(key) {
            // The caller's own second gate, which `docs/adr/0011` puts beside this crate's
            // date gate and which only a caller holding a zone can answer. Skipped without
            // counting, exactly as a candidate before `DTSTART` is: a `COUNT` spent on an
            // instance nobody receives ends the series an instance early. Asked after the
            // window's edge so that a gate refusing everything still terminates here.
            return None;
        }
        self.produced = self.produced.saturating_add(1);
        Some(RuleKey::Next(key))
    }

    /// Whether the rule's own end falls at or before `key`.
    fn has_ended_at(&self, key: Instant) -> bool {
        match self.cadence.as_ref().map(|cadence| cadence.rule.limit()) {
            Some(RuleLimit::Count(count)) => self.produced >= count.get(),
            Some(RuleLimit::Until { at, .. }) => key > at,
            Some(RuleLimit::Infinite) | None => false,
        }
    }

    /// Open the next period, or say why there is not one. `None` means one was opened.
    fn open_next_period<S: DiagnosticSink + ?Sized>(
        &mut self,
        env: &mut StepEnv<'_, S>,
    ) -> Option<RuleKey> {
        let (period, anchor) = match self.advance_walk() {
            Ok(opened) => opened,
            Err(stop) => {
                self.ended = true;
                return Some(stop);
            },
        };
        if anchor >= env.generation.end() {
            // Every candidate of a period falls inside it, so a period anchored past the end
            // of generation has nothing left to contribute and neither has any period after.
            return Some(RuleKey::WindowEnded);
        }
        self.expand(period, env)
    }

    /// The next period of the walk with the instant it is anchored at, or why there is not one.
    ///
    /// Two ways for a walk to have nothing left, and they are different answers to the caller.
    /// A stream with no rule at all is an `RDATE`-only series, whose rule ended before it
    /// began. A walk that ran dry did so at the last anchor RFC 5545 section 3.3.4 can write,
    /// which says nothing about whether the rule was finished.
    fn advance_walk(&mut self) -> Result<(Period, Instant), RuleKey> {
        let Some(cadence) = self.cadence.as_mut() else {
            return Err(RuleKey::RuleEnded);
        };
        let Some(period) = cadence.walk.next() else {
            return Err(RuleKey::CalendarEnded);
        };
        period
            .anchor()
            .at_offset(UtcOffset::UTC)
            .map(|anchor| (period, anchor))
            .ok_or(RuleKey::CalendarEnded)
    }

    /// Turn `period` into the candidates selected from it, or say the budget ran out.
    fn expand<S: DiagnosticSink + ?Sized>(
        &mut self,
        period: Period,
        env: &mut StepEnv<'_, S>,
    ) -> Option<RuleKey> {
        let Some(cadence) = self.cadence.as_ref() else {
            // Unreachable: a period exists only where a rule was walked to produce it.
            // Answering "the rule ended" rather than looping keeps an impossible state finite.
            self.ended = true;
            return Some(RuleKey::RuleEnded);
        };
        let rule = cadence.rule;
        let dtstart = cadence.dtstart;
        env.charges.open_period(&mut *env.meter);
        let Ok(set) = expand_period(period, rule, dtstart, &mut *env.meter, &mut *env.sink) else {
            return Some(RuleKey::Exhausted);
        };
        self.selected = Some(select(&set, rule.by_set_pos()));
        self.position = 0;
        None
    }

    /// Where the walk had got to, one period behind what it has already yielded.
    ///
    /// One behind deliberately. A cursor naming the period *after* the last one it yielded
    /// would lose every candidate the search had not reached inside that period, so a resumed
    /// search re-expands it and skips what it already produced by cadence key. That costs one
    /// period of charged work and buys a resume that cannot silently drop an occurrence.
    fn cursor_state(&self) -> RuleCursorState {
        let period_index = self
            .cadence
            .as_ref()
            .map_or(0, |cadence| cadence.walk.index().saturating_sub(1));
        RuleCursorState { period_index }
    }
}

/// What one turn of the search produced.
#[derive(Debug)]
enum Progress<'a> {
    /// An occurrence to hand the caller.
    Emit(Occurrence<'a>),
    /// Nothing to show, and the search has not finished. Turn again.
    Retry,
    /// The search is over and the outcome is recorded.
    Stop,
    /// The budget ran out.
    Exhausted(BudgetExhausted),
}

/// A lazy search over one series, bounded by a window and by a budget.
///
/// The `Item` is [`SearchStep`] and never `Result<Occurrence, BudgetExhausted>`;
/// [`SearchStep`]'s own documentation gives the mechanical reason. The terminal state is reported
/// three times over, in decreasing order of survivability: as the last [`SearchStep`], as
/// [`RecurrenceSearch::outcome`], and as the caller's own `Meter`, whose exhaustion flag
/// latches and which outlives every combinator applied to this iterator.
///
/// The second type parameter is the caller's sink and is defaulted, so `RecurrenceSearch<'a>`
/// names the erased form while a caller passing a concrete sink gets it monomorphized with no
/// vtable.
pub struct RecurrenceSearch<'a, S: DiagnosticSink + ?Sized = dyn DiagnosticSink + 'a> {
    /// The window the caller asked about.
    asked: Window,
    /// That window widened by the largest shift the override set implies.
    generation: Window,
    /// The caller's ledger.
    meter: &'a mut Meter,
    /// Where diagnostics go.
    sink: &'a mut S,
    /// Where this search charges.
    charges: Charges,
    /// The rule half.
    stream: RuleStream<'a>,
    /// The merge half.
    merge: Merge<'a>,
    /// The cadence key the rule stream produced and the merge has not consumed.
    offered: Option<Instant>,
    /// Whether the rule stream has finished.
    stream_done: bool,
    /// Cadence keys at or before this one belong to an earlier search.
    resume_after: Option<Instant>,
    /// The last cadence key this search reached.
    reached: Instant,
    /// Why the search is not producing more, or that it still is.
    outcome: SearchOutcome,
    /// Whether `next` has already answered for the last time.
    finished: bool,
}

impl<'a, S: DiagnosticSink + ?Sized> RecurrenceSearch<'a, S> {
    /// Assemble a search, reporting what only this point can see about it.
    fn start(
        input: RecurrenceInput<'a>,
        cursor: Option<SearchCursor>,
        asked: Window,
        meter: &'a mut Meter,
        sink: &'a mut S,
    ) -> Self {
        if !input.until_value_type_agrees() {
            // The first point holding both halves of the comparison, and therefore the only
            // one that can report the disagreement RFC 5545 section 3.3.10 forbids.
            report_diagnostic(
                &mut *sink,
                &mut *meter,
                Diagnostic::at_instant(
                    DiagnosticCode::RecurrenceUntilValueTypeMismatch,
                    Severity::Violation,
                    input.dtstart(),
                ),
            );
        }
        let generation = Self::widen(asked, input, &mut *meter, &mut *sink);
        let origin = cursor.map_or(input.dtstart(), SearchCursor::resume_after);
        let charges = Charges::new(origin, &*meter);
        Self {
            asked,
            generation,
            meter,
            sink,
            charges,
            stream: RuleStream::new(input, cursor),
            merge: Merge::new(input),
            offered: None,
            stream_done: false,
            resume_after: cursor.map(SearchCursor::resume_after),
            reached: origin,
            outcome: SearchOutcome::Pending,
            finished: false,
        }
    }

    /// The window generation runs over.
    ///
    /// When the widening leaves the representable timeline the window the caller asked about is
    /// used unchanged and the consequence is reported rather than swallowed: a shift that large
    /// guarantees some occurrence's effective start lies outside anything this search can
    /// generate, which is exactly what [`DiagnosticCode::OverrideLeftWindow`] names. Narrowing
    /// the question silently is the one thing unit 7 forbids here.
    fn widen(asked: Window, input: RecurrenceInput<'a>, meter: &mut Meter, sink: &mut S) -> Window {
        if let Some(widened) = generation_window(asked, input.overrides()) {
            return widened;
        }
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(
                DiagnosticCode::OverrideLeftWindow,
                Severity::Note,
                asked.start(),
            ),
        );
        asked
    }

    /// Why the search is not producing more occurrences, or that it still is.
    #[must_use]
    pub const fn outcome(&self) -> SearchOutcome {
        self.outcome
    }

    /// Where this search has got to, so another can carry on from it.
    #[must_use]
    pub fn cursor(&self) -> SearchCursor {
        SearchCursor::new(
            self.reached,
            self.stream.produced,
            self.stream.cursor_state(),
        )
    }

    /// One turn: an occurrence, a reason to turn again, or a reason to stop.
    ///
    /// The three questions are asked in the order unit 5 documents, and the order is the whole
    /// of the protocol. Drained first, because the answer to it is the only end of the merge.
    /// Then which source the step will consume, because the step moves the cursor that decides
    /// it. Then the step.
    fn advance(&mut self) -> Progress<'a> {
        if let Some(stop) = self.fill_offer() {
            return stop;
        }
        let offered = self.offered;
        if self.merge.is_drained(offered) {
            return Progress::Stop;
        }
        let consumes_offer = self.merge.takes_rule_key(offered);
        let produced = self.merge.step(offered, &mut *self.meter, &mut *self.sink);
        if consumes_offer {
            self.offered = None;
        }
        match produced {
            // Nothing to show for this candidate: an `EXDATE` removed it, or its start was not
            // representable. The merge is not over — that is `Merge::is_drained`'s answer at
            // the top of the next turn, and never this silence.
            None => Progress::Retry,
            Some(occurrence) => self.deliver(occurrence),
        }
    }

    /// Make sure a cadence key is on offer, or record why there will not be another.
    fn fill_offer(&mut self) -> Option<Progress<'a>> {
        if self.offered.is_some() || self.stream_done {
            return None;
        }
        let answer = {
            let mut env = StepEnv {
                generation: self.generation,
                charges: &mut self.charges,
                meter: &mut *self.meter,
                sink: &mut *self.sink,
            };
            self.stream.next_key(&mut env)
        };
        match answer {
            RuleKey::Next(key) => {
                self.reached = key;
                self.charges.mark_reached(key);
                self.offered = Some(key);
            },
            RuleKey::RuleEnded => {
                self.stream_done = true;
                self.record(SearchOutcome::RuleEnded);
            },
            RuleKey::WindowEnded => {
                self.stream_done = true;
                self.record(SearchOutcome::WindowEnded);
            },
            RuleKey::CalendarEnded => {
                self.stream_done = true;
                self.record(SearchOutcome::CalendarEnded);
                self.report_calendar_end();
            },
            RuleKey::Exhausted => return Some(Progress::Exhausted(self.exhausted())),
        }
        None
    }

    /// Say that the calendar ran out under a rule that had not.
    ///
    /// Once per search, because the stream latches when it answers this and is never asked
    /// again. The instant is the last cadence key the search reached, which is the last
    /// instance the recurrence set has — a caller reading it learns where the series stops
    /// rather than that some year it never named does not exist.
    fn report_calendar_end(&mut self) {
        report_diagnostic(
            &mut *self.sink,
            &mut *self.meter,
            Diagnostic::at_instant(
                DiagnosticCode::RecurrenceCalendarEnded,
                Severity::Note,
                self.reached,
            ),
        );
    }

    /// Filter one merged occurrence against the caller's window, and charge what survives.
    fn deliver(&mut self, occurrence: Occurrence<'a>) -> Progress<'a> {
        if occurrence.key() >= self.generation.end() {
            self.record(SearchOutcome::WindowEnded);
            return Progress::Stop;
        }
        if !self.admits(occurrence) {
            return Progress::Retry;
        }
        if self.charges.occurrence(&mut *self.meter).is_err() {
            return Progress::Exhausted(self.exhausted());
        }
        if occurrence.key() > self.reached {
            // An `RDATE` past the last key the rule produced is still something a resumed
            // search must not produce again, and the merge has no cursor of its own. This is
            // safe to advance over: every rule key below it was already generated, because the
            // stream only stops at a key past the end of generation and this one is not.
            self.reached = occurrence.key();
        }
        Progress::Emit(occurrence)
    }

    /// Whether this occurrence belongs to the window the caller asked about.
    ///
    /// Two questions, because an override moves a start away from the key that generated it in
    /// either direction. The key is asked first, since that is what the window bounds; the
    /// start is asked second, because an occurrence moved *into* the window has to appear.
    fn admits(&mut self, occurrence: Occurrence<'a>) -> bool {
        if self
            .resume_after
            .is_some_and(|after| occurrence.key() <= after)
        {
            // An `RDATE` the earlier search already emitted. The merge restarts from the head
            // of the list on resume and has no cursor of its own, so the skip is stated here.
            return false;
        }
        let by_key = self.asked.contains(occurrence.key());
        let by_start = occurrence.starts_within(self.asked);
        if by_key && !by_start {
            report_diagnostic(
                &mut *self.sink,
                &mut *self.meter,
                Diagnostic::at_instant(
                    DiagnosticCode::OverrideLeftWindow,
                    Severity::Note,
                    occurrence.key(),
                ),
            );
        }
        by_key || by_start
    }

    /// The terminal state, recorded in the outcome and reported through the sink on its way out.
    ///
    /// The count is unit 7's `Charges::spent`, which is what this search's candidates actually
    /// cost, and never the size of the sets that came back. A period refused while filling
    /// returns no set at all and has still charged everything it generated, and a rule that
    /// produces an instance in no period — `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` — produces no
    /// sets and spends the whole budget. Counting output would report zero for both, which is
    /// the opposite of what a caller deciding whether to retry with a larger budget needs to
    /// hear. The instant is this search's own `reached`, because the ledger counts and only
    /// this unit knows which cadence key the count stopped at.
    fn exhausted(&mut self) -> BudgetExhausted {
        let terminal = BudgetExhausted::new(self.reached, self.charges.spent(&*self.meter));
        self.outcome = SearchOutcome::BudgetExhausted(terminal);
        report_diagnostic(
            &mut *self.sink,
            &mut *self.meter,
            Diagnostic::at_instant(
                DiagnosticCode::RecurrenceBudgetExhausted,
                Severity::LimitReached,
                self.reached,
            ),
        );
        terminal
    }

    /// Record why the search stopped, keeping the first answer.
    fn record(&mut self, outcome: SearchOutcome) {
        if matches!(self.outcome, SearchOutcome::Pending) {
            self.outcome = outcome;
        }
    }
}

impl<S: DiagnosticSink + ?Sized> fmt::Debug for RecurrenceSearch<'_, S> {
    /// The sink and the ledger are the caller's and are not this type's to print.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecurrenceSearch")
            .field("window", &self.asked)
            .field("generation", &self.generation)
            .field("outcome", &self.outcome)
            .field("candidates", &self.charges.spent(&*self.meter))
            .finish_non_exhaustive()
    }
}

impl<'a, S: DiagnosticSink + ?Sized> Iterator for RecurrenceSearch<'a, S> {
    type Item = SearchStep<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.finished {
                return None;
            }
            match self.advance() {
                Progress::Emit(occurrence) => return Some(SearchStep::Occurrence(occurrence)),
                Progress::Retry => {},
                Progress::Stop => {
                    self.finished = true;
                    return None;
                },
                Progress::Exhausted(terminal) => {
                    self.finished = true;
                    return Some(SearchStep::BudgetExhausted(terminal));
                },
            }
        }
    }
}

/// Nothing follows a terminal step, and asking again is defined rather than merely harmless.
impl<S: DiagnosticSink + ?Sized> FusedIterator for RecurrenceSearch<'_, S> {}

impl RecurrenceInput<'_> {
    /// Search this series over `window`, charging `meter` and reporting to `sink`.
    ///
    /// The meter is borrowed rather than owned for the whole life of the search, which is what
    /// makes a fan-out over five thousand series bounded in aggregate and not only per series
    /// (`docs/adr/0010`). It is also the second of the three reports of the terminal state: a
    /// caller that discarded every [`SearchStep`] can still find `Meter::is_exhausted`.
    pub fn search<'s, S>(
        &'s self,
        window: Window,
        meter: &'s mut Meter,
        sink: &'s mut S,
    ) -> RecurrenceSearch<'s, S>
    where
        S: DiagnosticSink + ?Sized,
    {
        RecurrenceSearch::start(*self, None, window, meter, sink)
    }

    /// Carry on a search from `cursor`, over `window`.
    ///
    /// The window is stated again rather than remembered, because a caller resuming into the
    /// next month is asking about a different window and a cursor that carried one would make
    /// that the exception. What the cursor carries instead is the count, so a `COUNT`-bounded
    /// rule resumed here yields the recurrence set the file describes.
    pub fn resume<'s, S>(
        &'s self,
        cursor: SearchCursor,
        window: Window,
        meter: &'s mut Meter,
        sink: &'s mut S,
    ) -> RecurrenceSearch<'s, S>
    where
        S: DiagnosticSink + ?Sized,
    {
        RecurrenceSearch::start(*self, Some(cursor), window, meter, sink)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::NonZeroU32;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Instant, Limits, Meter,
        UtcOffset,
    };

    use super::{RecurrenceSearch, SearchCursor};
    use crate::input::{Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput};
    use crate::rule::{ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleLimit, ValueKind};
    use crate::search::{BudgetExhausted, SearchOutcome, SearchStep, Window};

    /// One instant, written the way RFC 5545 section 3.8.5.3 writes its worked examples.
    ///
    /// The examples are stated in a named zone; the caller resolves a zone before this crate
    /// sees anything, so the wall clock is kept and the resolution is UTC. What the tests below
    /// assert is the shape of the recurrence set, which is what the examples are about.
    fn at(year: u16, month: u8, day: u8, hour: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, 0, 0).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// What one search produced, flattened to values that outlive the borrow of the meter.
    #[derive(Debug)]
    struct Run {
        /// The effective start of every occurrence emitted.
        starts: Vec<Instant>,
        /// The terminal step, when there was one.
        terminal: Option<BudgetExhausted>,
        /// What the search said about itself when it was done.
        outcome: SearchOutcome,
        /// Steps the iterator produced, terminal step included.
        steps: usize,
    }

    /// Drive `search` to its end.
    fn drain<S>(mut search: RecurrenceSearch<'_, S>) -> Run
    where
        S: ical_core::DiagnosticSink + ?Sized,
    {
        let mut starts = Vec::new();
        let mut terminal = None;
        let mut produced = 0_usize;
        for step in &mut search {
            produced = produced.saturating_add(1);
            match step {
                SearchStep::Occurrence(occurrence) => starts.push(occurrence.start()),
                SearchStep::BudgetExhausted(exhausted) => terminal = Some(exhausted),
            }
        }
        Run {
            starts,
            terminal,
            outcome: search.outcome(),
            steps: produced,
        }
    }

    /// A rule at `freq` with `count` occurrences and nothing else stated.
    fn counted(freq: Freq, count: u32) -> RecurrenceRule {
        RecurrenceRuleBuilder::new(freq)
            .limit(RuleLimit::Count(NonZeroU32::new(count).unwrap()))
            .build()
            .unwrap()
    }

    /// An input with a rule, no additions, no exclusions and no overrides.
    fn plain<'a>(
        dtstart: Instant,
        rule: &'a RecurrenceRule,
        meter: &mut Meter,
    ) -> RecurrenceInput<'a> {
        RecurrenceInput::new(
            dtstart,
            ValueKind::DateTime,
            Some(rule),
            &[],
            &[],
            OverrideSet::empty(),
            meter,
        )
        .unwrap()
    }

    /// RFC 5545 section 3.8.5.3, "Daily for 10 occurrences".
    ///
    /// `DTSTART;TZID=America/New_York:19970902T090000` with `RRULE:FREQ=DAILY;COUNT=10` gives
    /// September 2 through 11 of 1997. The expectation is the RFC's own list and not this
    /// implementation's output.
    #[test]
    fn a_daily_rule_produces_the_ten_days_the_rfc_lists() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 10);
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 10, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        let expected: Vec<Instant> = (2..=11).map(|day| at(1997, 9, day, 9)).collect();
        assert_eq!(run.starts, expected);
        assert_eq!(run.outcome, SearchOutcome::RuleEnded);
        assert_eq!(run.terminal, None);
    }

    /// RFC 5545 section 3.8.5.3, "Monthly on the first and last day of the month for 10
    /// occurrences", which crosses two month lengths and a year boundary.
    ///
    /// `DTSTART:19980930T090000` with `RRULE:FREQ=MONTHLY;BYMONTHDAY=1,-1` gives September 30,
    /// October 1, October 31, November 1, November 30, December 1, December 31, January 1,
    /// January 31 and February 1. It is also the row the milestone brief singles out:
    /// `BYMONTHDAY` *expands* under `FREQ=MONTHLY`, so a month yields two candidates from a
    /// `DTSTART` that names neither of them.
    #[test]
    fn by_month_day_expands_under_monthly_across_the_year_boundary() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .limit(RuleLimit::Count(NonZeroU32::new(10).unwrap()))
            .by_month_day(ByList::from_slice(&[1_i8, -1_i8]))
            .build()
            .unwrap();
        let input = plain(at(1998, 9, 30, 9), &rule, &mut meter);
        let window = Window::new(at(1998, 9, 1, 0), at(1999, 3, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        let expected = alloc::vec![
            at(1998, 9, 30, 9),
            at(1998, 10, 1, 9),
            at(1998, 10, 31, 9),
            at(1998, 11, 1, 9),
            at(1998, 11, 30, 9),
            at(1998, 12, 1, 9),
            at(1998, 12, 31, 9),
            at(1999, 1, 1, 9),
            at(1999, 1, 31, 9),
            at(1999, 2, 1, 9),
        ];
        assert_eq!(run.starts, expected);
    }

    /// RFC 5545 section 3.3.10: an instance naming a date that does not exist is ignored
    /// rather than moved to a nearby one.
    ///
    /// Not a worked example — the section states the rule instead — so the expectation is read
    /// off the calendar: a yearly rule anchored on a leap day recurs only on leap days.
    #[test]
    fn a_leap_day_rule_skips_the_years_that_have_no_leap_day() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Yearly, 2);
        let input = plain(at(2020, 2, 29, 9), &rule, &mut meter);
        let window = Window::new(at(2020, 1, 1, 0), at(2026, 1, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert_eq!(
            run.starts,
            alloc::vec![at(2020, 2, 29, 9), at(2024, 2, 29, 9)]
        );
        assert!(
            sink.iter().any(|diagnostic| {
                diagnostic.code() == DiagnosticCode::NonexistentRecurrenceInstance
            }),
            "a February 29 that does not exist is reported, not silently dropped"
        );
    }

    /// The window ends where the rule does not, and the rule is free to continue past it.
    #[test]
    fn a_window_that_ends_first_says_so_and_the_rule_does_not_end() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 10);
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 9, 5, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert_eq!(run.starts.len(), 3, "September 2, 3 and 4");
        assert_eq!(run.outcome, SearchOutcome::WindowEnded);
        assert!(run.outcome.is_complete());
    }

    /// A window whose upper edge falls between a cadence key and its shifted effective start.
    ///
    /// The key is inside, so the occurrence is emitted; the start is outside, so
    /// `starts_within` says no and the search reports `override-left-window`. Both halves are
    /// asserted, because an implementation that admitted by start alone would drop this
    /// occurrence and an implementation that never reported would hide the gap.
    #[test]
    fn an_override_that_shifts_a_start_past_the_window_edge_is_still_emitted() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let moved_key = at(1997, 9, 3, 9);
        let entries = [Override::new(
            moved_key,
            OverrideRange::ThisOnly,
            Some(at(1997, 9, 6, 9)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let rule = counted(Freq::Daily, 10);
        let input = RecurrenceInput::new(
            at(1997, 9, 2, 9),
            ValueKind::DateTime,
            Some(&rule),
            &[],
            &[],
            overrides,
            &mut meter,
        )
        .unwrap();
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 9, 5, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert!(
            run.starts.contains(&at(1997, 9, 6, 9)),
            "the key was inside the window even though the start was not"
        );
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::OverrideLeftWindow),
            "a start that left the window is reported rather than hidden"
        );
    }

    /// A resumed `COUNT`-bounded expansion, against a from-scratch one truncated at the same
    /// `COUNT`.
    ///
    /// The whole point of the counter inside the cursor: resuming must reproduce the recurrence
    /// set the file describes, not start a fresh count from the resume point. A resume that
    /// counted afresh would yield fourteen occurrences from a rule that has ten.
    #[test]
    fn a_resumed_count_bounded_search_matches_the_one_it_resumed() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 10);
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 10, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let whole = drain(input.search(window, &mut meter, &mut sink));

        let (prefix, cursor) = {
            let mut search = input.search(window, &mut meter, &mut sink);
            let mut taken = Vec::new();
            for _ in 0..4 {
                if let Some(SearchStep::Occurrence(occurrence)) = search.next() {
                    taken.push(occurrence.start());
                }
            }
            (taken, search.cursor())
        };
        assert_eq!(prefix.len(), 4);
        assert_eq!(cursor.occurrences_emitted(), 4);

        let rest = drain(input.resume(cursor, window, &mut meter, &mut sink));
        let mut rejoined = prefix;
        rejoined.extend_from_slice(&rest.starts);
        assert_eq!(rejoined, whole.starts);
        assert_eq!(rest.outcome, SearchOutcome::RuleEnded);
    }

    /// `next()` past a terminal step, and the three reports of one fact agreeing.
    ///
    /// The budget is spent on the shared ledger rather than on a per-period ceiling, so the
    /// meter's own latching flag is the second report and can be read once the search that
    /// borrowed it is gone. The third is `outcome`, which must carry the same value the
    /// terminal step did.
    #[test]
    fn a_search_that_exhausts_its_budget_reports_it_three_times_and_then_stops() {
        // A shared budget far under the year this window asks about: unit 3 charges one per
        // candidate generated, so a rule with no end runs out inside the window. The number is
        // a budget and not a count of occurrences, which is the distinction a rule that matches
        // rarely depends on.
        let mut meter = Meter::with_budget(Limits::DEFAULT, 32);
        let rule = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1998, 9, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let (flags, reported) = {
            let mut search = input.search(window, &mut meter, &mut sink);
            let mut flags = Vec::new();
            for step in search.by_ref() {
                flags.push(step.is_terminal());
            }
            assert_eq!(search.next().map(SearchStep::is_terminal), None);
            assert_eq!(
                search.next().map(SearchStep::is_terminal),
                None,
                "a fused iterator answers None every time after the terminal step"
            );
            (flags, search.outcome())
        };

        assert_eq!(flags.last(), Some(&true), "the terminal step comes last");
        let terminal = match reported {
            SearchOutcome::BudgetExhausted(exhausted) => exhausted,
            other => panic!("the outcome must be the terminal state, not {other:?}"),
        };
        assert!(terminal.candidates_spent() > 0);
        assert!(!reported.is_complete());
        assert!(
            meter.is_exhausted(),
            "the meter latches, which is the report that outlives every combinator"
        );
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::RecurrenceBudgetExhausted),
            "cut short at the limit and ended at UNTIL are different answers"
        );
    }

    /// `count()` on an exhausted search, asserted as the known-inflated number it is.
    ///
    /// `Iterator::count` counts *steps*. No item type this crate can write changes that, so the
    /// number is pinned here rather than explained away: it is the occurrences plus one.
    #[test]
    fn count_on_an_exhausted_search_is_the_occurrences_plus_its_terminal_step() {
        let mut meter = Meter::with_budget(Limits::DEFAULT, 32);
        let rule = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1998, 9, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let emitted = drain(input.search(window, &mut meter, &mut sink))
            .starts
            .len();

        let mut second = Meter::with_budget(Limits::DEFAULT, 32);
        let again = plain(at(1997, 9, 2, 9), &rule, &mut second);
        let counted_steps = again.search(window, &mut second, &mut sink).count();
        assert_eq!(
            counted_steps,
            emitted.saturating_add(1),
            "count() counts steps, and an exhausted search has one that is not an occurrence"
        );
    }

    /// The rule outlives the calendar, and the two ends are told apart.
    ///
    /// `FREQ=DAILY` with no `COUNT` and no `UNTIL`, asked about a window that outlasts every
    /// calendar. The last four days of 9999 are instances — RFC 5545 section 3.3.4 writes
    /// 9999-12-31 and every instant of it is representable — and what stops the search is the
    /// timeline rather than the rule, which is a fact the rule's own `COUNT` and `UNTIL` cannot
    /// carry. The answer is complete and it is not `RuleEnded`.
    #[test]
    fn a_rule_that_outlives_the_calendar_says_so_rather_than_claiming_the_rule_ended() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        let input = plain(at(9999, 12, 28, 9), &rule, &mut meter);
        // A window no calendar can reach the end of, so nothing but the timeline can stop this.
        let window =
            Window::new(at(9999, 12, 28, 0), Instant::from_unix_seconds(i64::MAX)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        let expected: Vec<Instant> = (28..=31).map(|day| at(9999, 12, day, 9)).collect();
        assert_eq!(run.starts, expected, "December 31st is an instance");
        assert_eq!(run.outcome, SearchOutcome::CalendarEnded);
        assert!(
            run.outcome.is_complete(),
            "there is no more calendar to search, so the answer is whole"
        );
        assert_eq!(run.terminal, None, "nothing here ran out of budget");
        assert_eq!(
            sink.iter()
                .filter(|note| note.code() == DiagnosticCode::RecurrenceCalendarEnded)
                .count(),
            1,
            "said once, at the last instance the recurrence set has"
        );
    }

    /// An `EXDATE` that removes an `RDATE` removes that one occurrence and nothing after it.
    ///
    /// The merge answers `None` both for a candidate an exclusion removed and for no candidate
    /// at all, so the driver asks `Merge::is_drained` first and `Merge::takes_rule_key` second
    /// rather than inferring either from that silence. Inferring the first ends the series at
    /// the exclusion; inferring the second retires a rule key the merge never consumed, which
    /// deletes the instance after it — including `DTSTART`, which section 3.8.5.3 makes the
    /// first member of every recurrence set.
    #[test]
    fn an_exclusion_on_an_addition_leaves_every_occurrence_around_it() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 3);
        // Both additions sit before a rule instance and one of them is excluded, so an engine
        // that retired the offered rule key anyway loses September 2nd and 3rd.
        let additions = [at(1997, 9, 2, 7), at(1997, 9, 3, 7)];
        let exclusions = [at(1997, 9, 2, 7)];
        let input = RecurrenceInput::new(
            at(1997, 9, 2, 9),
            ValueKind::DateTime,
            Some(&rule),
            &additions,
            &exclusions,
            OverrideSet::empty(),
            &mut meter,
        )
        .unwrap();
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert_eq!(
            run.starts,
            alloc::vec![
                at(1997, 9, 2, 9),
                at(1997, 9, 3, 7),
                at(1997, 9, 3, 9),
                at(1997, 9, 4, 9),
            ],
            "the excluded addition is gone and the three rule instances are all here"
        );
        assert!(run.outcome.is_complete());
    }

    /// An exclusion on the head of an `RDATE` tail is not the end of the tail.
    #[test]
    fn an_exclusion_on_the_first_addition_after_the_rule_leaves_the_rest_of_the_list() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 1);
        let additions = [at(1997, 9, 5, 9), at(1997, 9, 6, 9)];
        let exclusions = [at(1997, 9, 5, 9)];
        let input = RecurrenceInput::new(
            at(1997, 9, 2, 9),
            ValueKind::DateTime,
            Some(&rule),
            &additions,
            &exclusions,
            OverrideSet::empty(),
            &mut meter,
        )
        .unwrap();
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert_eq!(
            run.starts,
            alloc::vec![at(1997, 9, 2, 9), at(1997, 9, 6, 9)],
            "one exclusion removes one addition and says nothing about the one after it"
        );
    }

    /// A series with no rule at all is its `RDATE` list, and the search still bounds it.
    #[test]
    fn an_input_with_no_rule_yields_its_additions_and_ends_with_the_rule() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let additions = [at(1997, 9, 2, 9), at(1997, 9, 20, 9)];
        let input = RecurrenceInput::new(
            at(1997, 9, 2, 9),
            ValueKind::DateTime,
            None,
            &additions,
            &[],
            OverrideSet::empty(),
            &mut meter,
        )
        .unwrap();
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let run = drain(input.search(window, &mut meter, &mut sink));
        assert_eq!(run.starts, alloc::vec![at(1997, 9, 2, 9)]);
        assert_eq!(
            run.steps, 1,
            "the addition outside the window is not a step"
        );
        assert!(run.outcome.is_complete());
    }

    /// A cursor is a position in an algorithm, and rebuilding one from its parts is the only
    /// thing a caller can do with the opaque half.
    #[test]
    fn a_cursor_round_trips_through_its_own_constructor() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 10);
        let input = plain(at(1997, 9, 2, 9), &rule, &mut meter);
        let window = Window::new(at(1997, 9, 1, 0), at(1997, 10, 1, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();

        let cursor = {
            let mut search = input.search(window, &mut meter, &mut sink);
            assert!(search.next().is_some(), "the first occurrence");
            search.cursor()
        };
        let rebuilt = SearchCursor::new(
            cursor.resume_after(),
            cursor.occurrences_emitted(),
            cursor.rule_cursor(),
        );
        assert_eq!(rebuilt, cursor);
    }
}
