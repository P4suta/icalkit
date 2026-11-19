// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Search a recurrence set in recurrence identity order.
//!
//! An occurrence belongs to a window if the window contains its recurrence key or its effective start.
//! An override can move a start in either direction, so generation widens both bounds by the largest absolute shift.
//! The upper bound stays exclusive.
//!
//! The search compares recurrence keys with `UNTIL` on the timeline the caller supplies.
//! RFC 5545 section 3.3.10 defines the value type and time zone for `UNTIL`.

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::fmt::{self, Formatter};
use core::iter::FusedIterator;

use crate::internal::core::{
    CivilDateTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant, LimitExceeded, Meter,
    Severity, UtcOffset, report_diagnostic,
};

use crate::internal::recur::accounting::{Charges, generation_window};
use crate::internal::recur::byparts::{ExpansionError, candidate_lower_bound, expand_period};
use crate::internal::recur::input::RecurrenceInput;
use crate::internal::recur::merge::Merge;
use crate::internal::recur::period::{Period, PeriodWalk};
use crate::internal::recur::rule::{RecurrenceRule, RuleLimit, Skip};
use crate::internal::recur::search::{
    BudgetExhausted, Occurrence, SearchOutcome, SearchStep, Window,
};
use crate::internal::recur::setpos::{SelectedCandidates, select};

/// Fixed-size checkpoint for a standalone recurrence rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuleCheckpoint {
    delivered: Option<Instant>,
    produced: u32,
    period_index: u64,
    counted_through: Option<Instant>,
}

impl From<RuleCheckpoint> for SearchCursor {
    fn from(rule: RuleCheckpoint) -> Self {
        Self {
            rule,
            additions: DeliveredAdditions::default(),
            pending: PendingRuleKeys::default(),
        }
    }
}

/// Continuation state for a recurrence rule and its additions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleCursorState {
    period_index: u64,
    counted_through: Option<Instant>,
    additions: DeliveredAdditions,
    pending: PendingRuleKeys,
}

/// Checkpoint for a recurrence set across search windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchCursor {
    rule: RuleCheckpoint,
    additions: DeliveredAdditions,
    pending: PendingRuleKeys,
}

impl SearchCursor {
    /// Restore a checkpoint from its parts.
    #[must_use]
    pub fn new(
        resume_after: Option<Instant>,
        occurrences_emitted: u32,
        rule_cursor: RuleCursorState,
    ) -> Self {
        Self {
            rule: RuleCheckpoint {
                delivered: resume_after,
                produced: occurrences_emitted,
                period_index: rule_cursor.period_index,
                counted_through: rule_cursor.counted_through,
            },
            additions: rule_cursor.additions,
            pending: rule_cursor.pending,
        }
    }

    /// The recurrence key of the last occurrence the caller received.
    #[must_use]
    pub const fn resume_after(&self) -> Option<Instant> {
        self.rule.delivered
    }

    /// The number of rule keys that count toward `COUNT`, including keys a later window can deliver.
    #[must_use]
    pub const fn occurrences_emitted(&self) -> u32 {
        self.rule.produced
    }

    /// The continuation state for the rule and its additions.
    #[must_use]
    pub fn rule_cursor(&self) -> RuleCursorState {
        RuleCursorState {
            period_index: self.rule.period_index,
            counted_through: self.rule.counted_through,
            additions: self.additions.clone(),
            pending: self.pending.clone(),
        }
    }
}

/// Rule keys that count toward `COUNT` and can enter a later window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingRuleKeys(Option<Rc<VecDeque<Instant>>>);

impl PendingRuleKeys {
    fn len(&self) -> usize {
        self.0.as_deref().map_or(0, VecDeque::len)
    }

    fn get(&self, index: usize) -> Option<Instant> {
        self.0.as_ref()?.get(index).copied()
    }

    fn push(&mut self, key: Instant, meter: &mut Meter) -> bool {
        self.edit(1, meter, |keys, _| {
            keys.push_back(key);
            true
        })
    }

    fn remove(
        &mut self,
        index: usize,
        meter: &mut Meter,
        admit: impl FnOnce(&mut Meter) -> bool,
    ) -> bool {
        if index == 0 && self.len() == 1 {
            if !admit(meter) {
                return false;
            }
            self.0 = None;
            return true;
        }
        self.edit(0, meter, |keys, meter| {
            let shifted = index.min(keys.len().saturating_sub(index).saturating_sub(1));
            if meter
                .try_charge(u64::try_from(shifted).unwrap_or(u64::MAX))
                .is_err()
                || !admit(meter)
            {
                return false;
            }
            keys.remove(index).is_some()
        })
    }

    fn edit(
        &mut self,
        extra: usize,
        meter: &mut Meter,
        apply: impl FnOnce(&mut VecDeque<Instant>, &mut Meter) -> bool,
    ) -> bool {
        let len = self.len();
        let needed = len.saturating_add(extra);
        if let Some(keys) = self.0.as_mut().and_then(Rc::get_mut)
            && keys.capacity() >= needed
        {
            return apply(keys, meter);
        }
        let capacity = if extra == 0 {
            len
        } else {
            needed.max(len.saturating_mul(2))
        };
        let bytes = capacity
            .saturating_mul(size_of::<Instant>())
            .saturating_add(size_of::<VecDeque<Instant>>())
            .saturating_add(size_of::<[usize; 2]>());
        if meter
            .try_charge_bytes(u64::try_from(bytes).unwrap_or(u64::MAX))
            .is_err()
        {
            return false;
        }
        let mut keys = VecDeque::new();
        if meter
            .admit(|| {
                keys.try_reserve_exact(capacity)
                    .map_err(|_| LimitExceeded::Budget)
            })
            .is_err()
        {
            return false;
        }
        if let Some(previous) = &self.0 {
            keys.extend(previous.iter().copied());
        }
        if !apply(&mut keys, meter) {
            return false;
        }
        self.0 = Some(Rc::new(keys));
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DeliveredAdditions {
    prefix: usize,
    /// Snapshots share delivery marks until a search changes them.
    beyond: Option<Rc<Vec<u64>>>,
}

impl DeliveredAdditions {
    fn contains(&self, index: usize) -> bool {
        index < self.prefix
            || self
                .beyond
                .as_ref()
                .and_then(|words| words.get(index / 64))
                .is_some_and(|word| word & (1_u64 << (index % 64)) != 0)
    }

    fn admit(
        &mut self,
        index: usize,
        count: usize,
        meter: &mut Meter,
        charges: &mut Charges,
    ) -> bool {
        if index == self.prefix {
            let mut next = index.saturating_add(1);
            while self.contains(next) {
                if meter.try_charge(1).is_err() {
                    return false;
                }
                next = next.saturating_add(1);
            }
            if charges.occurrence(meter).is_err() {
                return false;
            }
            self.prefix = next;
            if next == count {
                self.beyond = None;
            }
            return true;
        }
        if let Some(words) = self.beyond.as_mut().and_then(Rc::get_mut) {
            return Self::mark(words, index, meter, charges);
        }

        let word_count = count.div_ceil(64);
        let bytes = word_count
            .saturating_mul(size_of::<u64>())
            .saturating_add(size_of::<Vec<u64>>())
            .saturating_add(size_of::<[usize; 2]>());
        if meter
            .try_charge_bytes(u64::try_from(bytes).unwrap_or(u64::MAX))
            .is_err()
        {
            return false;
        }
        let mut words = Vec::new();
        if meter
            .admit(|| {
                words
                    .try_reserve_exact(word_count)
                    .map_err(|_| LimitExceeded::Budget)
            })
            .is_err()
        {
            return false;
        }
        if let Some(old) = &self.beyond {
            words.extend_from_slice(&old[..old.len().min(word_count)]);
        }
        words.resize(word_count, 0);
        if !Self::mark(&mut words, index, meter, charges) {
            return false;
        }
        self.beyond = Some(Rc::new(words));
        true
    }

    fn mark(words: &mut [u64], index: usize, meter: &mut Meter, charges: &mut Charges) -> bool {
        let Some(word) = words.get_mut(index / 64) else {
            return false;
        };
        if charges.occurrence(meter).is_err() {
            return false;
        }
        *word |= 1_u64 << (index % 64);
        true
    }
}

struct Cadence<'a> {
    rule: &'a RecurrenceRule,
    dtstart: CivilDateTime,
    walk: PeriodWalk,
    /// A resumed search can finish before it visits another period.
    period_index: u64,
}

impl<'a> Cadence<'a> {
    fn open(input: RecurrenceInput<'a>, cursor: Option<RuleCheckpoint>) -> Option<Self> {
        let rule = input.rule()?;
        // The caller resolves the start before the walk reads its civil fields.
        let dtstart = CivilDateTime::from_instant(input.dtstart(), UtcOffset::UTC)?;
        let period_index = cursor.map_or(0, |held| held.period_index);
        let walk = match cursor {
            Some(_) => PeriodWalk::resume_at(dtstart, rule, period_index),
            None => PeriodWalk::new(dtstart, rule),
        };
        Some(Self {
            rule,
            dtstart,
            walk,
            period_index,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum RuleKey {
    Next(Instant),
    RuleEnded,
    WindowEnded,
    /// RFC 5545 section 3.3.4 limits years to four digits.
    CalendarEnded,
    Exhausted,
    CalendarArithmeticFailed,
}

struct StepEnv<'e, S: DiagnosticSink + ?Sized> {
    generation: Window,
    charges: &'e mut Charges,
    meter: &'e mut Meter,
    sink: &'e mut S,
}

#[cfg(not(test))]
type OriginMap = alloc::collections::BTreeMap<Instant, u64>;
#[cfg(test)]
use tests::ObservedOriginMap as OriginMap;

#[derive(Default)]
struct PendingAdjustedKeys {
    /// Removing a marked key advances the earliest pending period.
    keys: OriginMap,
    earliest_period: Option<u64>,
}

impl PendingAdjustedKeys {
    fn extend_period(
        &mut self,
        selected: &SelectedCandidates,
        origin: u64,
        meter: &mut Meter,
    ) -> Result<(), LimitExceeded> {
        let mut keys = selected
            .as_slice()
            .iter()
            .rev()
            .filter_map(|at| at.at_offset(UtcOffset::UTC));
        let Some(greatest) = keys.next() else {
            return Ok(());
        };
        if let Some(depth) = self.keys.len().checked_ilog2() {
            meter.try_charge(u64::from(depth) + 1)?;
        }
        match self.keys.last_entry() {
            // A later period can outlive earlier periods only beyond their greatest key.
            Some(mut last) if *last.key() < greatest => *last.get_mut() = origin,
            None => self.earliest_period = Some(origin),
            Some(_) => {},
        }
        for key in core::iter::once(greatest).chain(keys) {
            self.keys.entry(key).or_default();
        }
        Ok(())
    }

    fn pop_before(&mut self, bound: Option<Instant>) -> Option<(Instant, u64)> {
        let earliest = self.earliest_period?;
        let first = self.keys.first_entry()?;
        if bound.is_some_and(|limit| *first.key() >= limit) {
            return None;
        }
        let (key, next_period) = first.remove_entry();
        self.earliest_period = (!self.keys.is_empty()).then_some(earliest.max(next_period));
        Some((key, earliest))
    }
}

/// Corrected dates can cross period boundaries under RFC 7529 section 4.1.
#[derive(Default)]
struct AdjustedStream {
    pending: PendingAdjustedKeys,
    peeked: Option<(Period, Instant, u64)>,
    stop: Option<RuleKey>,
    /// A live offer keeps the earliest replay period until the merge commits it.
    offered_period: Option<u64>,
}

struct RuleStream<'a> {
    cadence: Option<Cadence<'a>>,
    selected: Option<SelectedCandidates>,
    position: usize,
    adjusted: AdjustedStream,
    /// The live offer counts here until the checkpoint subtracts it.
    produced: u32,
    begins_at: Instant,
    /// Earlier searches counted every rule key through this boundary.
    resume_after: Option<Instant>,
    ended: bool,
    input: RecurrenceInput<'a>,
}

impl<'a> RuleStream<'a> {
    fn new(input: RecurrenceInput<'a>, cursor: Option<RuleCheckpoint>) -> Self {
        Self {
            cadence: Cadence::open(input, cursor),
            selected: None,
            position: 0,
            adjusted: AdjustedStream::default(),
            produced: cursor.map_or(0, |position| position.produced),
            begins_at: input.dtstart(),
            resume_after: cursor.and_then(|position| position.counted_through),
            ended: false,
            input,
        }
    }

    fn next_key<S: DiagnosticSink + ?Sized>(&mut self, env: &mut StepEnv<'_, S>) -> RuleKey {
        loop {
            if self.ended || self.count_is_complete() {
                self.ended = true;
                return RuleKey::RuleEnded;
            }
            let answer = if self.uses_adjustments() {
                match self.adjusted_candidate(env) {
                    Ok(key) => {
                        let answer = self.weigh(key, env.generation);
                        if answer.is_none() {
                            self.adjusted.offered_period = None;
                        }
                        answer
                    },
                    Err(stop) => Some(stop),
                }
            } else {
                match self.take_candidate() {
                    Some(key) => self.weigh(key, env.generation),
                    None => self.open_next_period(env),
                }
            };
            if let Some(stop) = answer {
                return stop;
            }
        }
    }

    fn uses_adjustments(&self) -> bool {
        self.cadence
            .as_ref()
            .is_some_and(|cadence| cadence.rule.skip() != Skip::Omit)
    }

    /// Release a corrected key once the next period can only supply later keys.
    fn adjusted_candidate<S: DiagnosticSink + ?Sized>(
        &mut self,
        env: &mut StepEnv<'_, S>,
    ) -> Result<Instant, RuleKey> {
        loop {
            self.peek_adjusted_period();
            let bound = self.adjusted.peeked.map(|(_, bound, _)| bound);
            if let Some((key, earliest)) = self.adjusted.pending.pop_before(bound) {
                self.adjusted.offered_period = Some(earliest);
                return Ok(key);
            }
            let Some((period, bound, origin)) = self.adjusted.peeked.take() else {
                return Err(self.adjusted.stop.unwrap_or(RuleKey::RuleEnded));
            };
            if bound >= env.generation.end() {
                self.adjusted.stop = Some(RuleKey::WindowEnded);
                continue;
            }
            if let Some(stop) = self.expand(period, env) {
                return Err(stop);
            }
            if let Some(selected) = self.selected.as_ref() {
                self.adjusted
                    .pending
                    .extend_period(selected, origin, env.meter)
                    .map_err(|_| RuleKey::Exhausted)?;
            }
        }
    }

    /// Read the period bound before charging for expansion.
    fn peek_adjusted_period(&mut self) {
        if self.adjusted.peeked.is_some() || self.adjusted.stop.is_some() {
            return;
        }
        match self.advance_walk() {
            Ok((period, bound)) => {
                let origin = self
                    .cadence
                    .as_ref()
                    .map_or(0, |cadence| cadence.period_index);
                self.adjusted.peeked = Some((period, bound, origin));
            },
            Err(stop) => self.adjusted.stop = Some(stop),
        }
    }

    fn take_candidate(&mut self) -> Option<Instant> {
        loop {
            let at = *self.selected.as_ref()?.as_slice().get(self.position)?;
            self.position = self.position.saturating_add(1);
            if let Some(key) = at.at_offset(UtcOffset::UTC) {
                return Some(key);
            }
        }
    }

    /// Check the rule limit before the window edge so a completed rule reports its own end.
    fn weigh(&mut self, key: Instant, generation: Window) -> Option<RuleKey> {
        if key < self.begins_at {
            // `BYSETPOS` selects from the whole period before the search discards keys earlier than `DTSTART`.
            // RFC 5545 section 3.8.5.3 defines `DTSTART` as the first instance.
            return None;
        }
        if self.resume_after.is_some_and(|after| key <= after) {
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
            // A nonexistent local time leaves `COUNT` unchanged under RFC 5545 section 3.3.10.
            return None;
        }
        self.produced = self.produced.saturating_add(1);
        Some(RuleKey::Next(key))
    }

    fn count_is_complete(&self) -> bool {
        match self.cadence.as_ref().map(|cadence| cadence.rule.limit()) {
            Some(RuleLimit::Count(count)) => self.produced >= count.get(),
            Some(RuleLimit::Until { .. } | RuleLimit::Infinite) | None => false,
        }
    }

    /// RFC 5545 section 3.3.10 includes the `UNTIL` instant.
    fn has_ended_at(&self, key: Instant) -> bool {
        match self.cadence.as_ref().map(|cadence| cadence.rule.limit()) {
            Some(RuleLimit::Count(_)) => self.count_is_complete(),
            Some(RuleLimit::Until { at, .. }) => key > at,
            Some(RuleLimit::Infinite) | None => false,
        }
    }

    fn open_next_period<S: DiagnosticSink + ?Sized>(
        &mut self,
        env: &mut StepEnv<'_, S>,
    ) -> Option<RuleKey> {
        let (period, lower_bound) = match self.advance_walk() {
            Ok(opened) => opened,
            Err(stop) => {
                self.ended = true;
                return Some(stop);
            },
        };
        if lower_bound >= env.generation.end() {
            return Some(RuleKey::WindowEnded);
        }
        self.expand(period, env)
    }

    fn advance_walk(&mut self) -> Result<(Period, Instant), RuleKey> {
        let Some(cadence) = self.cadence.as_mut() else {
            return Err(RuleKey::RuleEnded);
        };
        let period_index = cadence.walk.index();
        let Some(period) = cadence.walk.next() else {
            return Err(RuleKey::CalendarEnded);
        };
        cadence.period_index = period_index;
        candidate_lower_bound(period, cadence.rule)
            .map_err(|_| RuleKey::CalendarArithmeticFailed)?
            .at_offset(UtcOffset::UTC)
            .map(|lower_bound| (period, lower_bound))
            .ok_or(RuleKey::CalendarEnded)
    }

    fn expand<S: DiagnosticSink + ?Sized>(
        &mut self,
        period: Period,
        env: &mut StepEnv<'_, S>,
    ) -> Option<RuleKey> {
        let Some(cadence) = self.cadence.as_ref() else {
            self.ended = true;
            return Some(RuleKey::RuleEnded);
        };
        let rule = cadence.rule;
        let dtstart = cadence.dtstart;
        env.charges.open_period(&mut *env.meter);
        let set = match expand_period(period, rule, dtstart, &mut *env.meter, &mut *env.sink) {
            Ok(set) => set,
            Err(ExpansionError::Resource(_)) => return Some(RuleKey::Exhausted),
            Err(ExpansionError::Calendar(_)) => return Some(RuleKey::CalendarArithmeticFailed),
        };
        self.selected = Some(select(&set, rule.by_set_pos()));
        self.position = 0;
        None
    }

    /// Resume from the earliest period that can reproduce a pending key or offer.
    fn period_index(&self) -> u64 {
        let visited = self
            .cadence
            .as_ref()
            .map_or(0, |cadence| cadence.period_index);
        self.adjusted
            .pending
            .earliest_period
            .into_iter()
            .chain(self.adjusted.offered_period)
            .fold(visited, u64::min)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OfferSource {
    Rule,
    Replay,
    Addition,
}

#[derive(Debug)]
enum Progress<'a> {
    Emit(Occurrence<'a>),
    Retry,
    Stop,
    Exhausted(BudgetExhausted),
    CalendarArithmeticFailed,
}

/// A recurrence search with a shared budget and a half-open window.
///
/// Budget exhaustion yields one terminal [`SearchStep`] and updates the meter and the search outcome.
pub struct RecurrenceSearch<'a, S: DiagnosticSink + ?Sized = dyn DiagnosticSink + 'a> {
    asked: Window,
    generation: Window,
    meter: &'a mut Meter,
    sink: &'a mut S,
    charges: Charges,
    stream: RuleStream<'a>,
    merge: Merge<'a>,
    /// An earlier `RDATE` can leave the rule offer pending.
    offered: Option<Instant>,
    stream_done: bool,
    additions: DeliveredAdditions,
    pending: PendingRuleKeys,
    /// Replay earlier rule keys without spending `COUNT` twice.
    replay_through: Option<Instant>,
    replay_position: usize,
    reached: Instant,
    delivered: Option<Instant>,
    /// The merge commits a rule key after it excludes, defers, or delivers the occurrence.
    counted_through: Option<Instant>,
    outcome: SearchOutcome,
    finished: bool,
}

impl<'a, S: DiagnosticSink + ?Sized> RecurrenceSearch<'a, S> {
    fn start(
        input: RecurrenceInput<'a>,
        cursor: Option<SearchCursor>,
        asked: Window,
        meter: &'a mut Meter,
        sink: &'a mut S,
    ) -> Self {
        if !input.until_value_type_agrees() {
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
        let (rule, additions, pending) = cursor.map_or_else(
            || {
                (
                    None,
                    DeliveredAdditions::default(),
                    PendingRuleKeys::default(),
                )
            },
            |cursor| (Some(cursor.rule), cursor.additions, cursor.pending),
        );
        let delivered = rule.and_then(|position| position.delivered);
        let origin = delivered.unwrap_or(input.dtstart());
        let charges = Charges::new(origin, &*meter);
        Self {
            asked,
            generation,
            meter,
            sink,
            charges,
            stream: RuleStream::new(input, rule),
            merge: Merge::new(input),
            offered: None,
            stream_done: false,
            additions,
            replay_through: rule.and_then(|position| position.counted_through),
            replay_position: 0,
            pending,
            reached: origin,
            delivered,
            counted_through: rule.and_then(|position| position.counted_through),
            outcome: SearchOutcome::Pending,
            finished: false,
        }
    }

    /// If the shift exceeds the timeline, report the overflow and search the requested window.
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

    pub(crate) fn charge_work(&mut self, units: u64) -> Result<(), LimitExceeded> {
        self.meter.try_charge(units)
    }

    /// The rule can reach its end while the merge still has additions to deliver.
    #[must_use]
    pub const fn outcome(&self) -> SearchOutcome {
        self.outcome
    }

    /// Capture a checkpoint that can retry the current rule offer.
    #[must_use]
    pub fn cursor(&self) -> SearchCursor {
        SearchCursor {
            rule: self.rule_checkpoint(),
            additions: self.additions.clone(),
            pending: self.pending.clone(),
        }
    }

    pub(crate) fn rule_checkpoint(&self) -> RuleCheckpoint {
        RuleCheckpoint {
            delivered: self.delivered,
            produced: self
                .stream
                .produced
                .saturating_sub(u32::from(self.offered.is_some())),
            period_index: self.stream.period_index(),
            counted_through: self.counted_through,
        }
    }

    /// Check both sources and identify the next key before the merge advances its addition cursor.
    /// An exclusion can consume a key while producing no occurrence.
    fn advance(&mut self) -> Progress<'a> {
        let replay = self
            .pending
            .get(self.replay_position)
            .filter(|key| self.replay_through.is_some_and(|last| *key <= last));
        if replay.is_none()
            && let Some(stop) = self.fill_offer()
        {
            return stop;
        }
        let offered = replay.or(self.offered);
        if self.merge.is_drained(offered) {
            return Progress::Stop;
        }
        let source = match (self.merge.takes_rule_key(offered), replay) {
            (false, _) => OfferSource::Addition,
            (true, None) => OfferSource::Rule,
            (true, Some(_)) => OfferSource::Replay,
        };
        if source == OfferSource::Replay && self.meter.try_charge(1).is_err() {
            return Progress::Exhausted(self.exhausted());
        }
        let produced = self.merge.step(offered, &mut *self.meter, &mut *self.sink);
        let progress = produced.map_or(Progress::Retry, |occurrence| {
            self.deliver(occurrence, source)
        });
        if source == OfferSource::Rule && matches!(progress, Progress::Emit(_) | Progress::Retry) {
            self.counted_through = self.offered.take();
            self.stream.adjusted.offered_period = None;
        }
        if let Progress::Emit(occurrence) = progress {
            self.delivered = Some(occurrence.key());
        }
        progress
    }

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
            RuleKey::CalendarArithmeticFailed => {
                self.stream_done = true;
                self.record(SearchOutcome::CalendarArithmeticFailed);
                return Some(Progress::CalendarArithmeticFailed);
            },
        }
        None
    }

    /// Report the last key the search reached when the rule outlives the calendar.
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

    fn deliver(&mut self, occurrence: Occurrence<'a>, source: OfferSource) -> Progress<'a> {
        if occurrence.key() >= self.generation.end() {
            self.record(SearchOutcome::WindowEnded);
            return Progress::Stop;
        }
        let addition = self.merge.last_addition_index(occurrence.key());
        if addition.is_some_and(|index| self.additions.contains(index)) {
            return Progress::Retry;
        }
        if !self.admits(occurrence) {
            let future =
                occurrence.start() >= self.asked.end() || occurrence.key() >= self.asked.end();
            let kept = match source {
                OfferSource::Rule if future => self.pending.push(occurrence.key(), self.meter),
                OfferSource::Replay if !future => {
                    self.pending
                        .remove(self.replay_position, self.meter, |_| true)
                },
                OfferSource::Replay => {
                    self.replay_position = self.replay_position.saturating_add(1);
                    true
                },
                _ => true,
            };
            return if kept {
                Progress::Retry
            } else {
                Progress::Exhausted(self.exhausted())
            };
        }
        let mut admit = |meter: &mut Meter| match addition {
            Some(index) => self.additions.admit(
                index,
                self.stream.input.rdates().len(),
                meter,
                &mut self.charges,
            ),
            None => self.charges.occurrence(meter).is_ok(),
        };
        let admitted = if source == OfferSource::Replay {
            self.pending.remove(self.replay_position, self.meter, admit)
        } else {
            admit(self.meter)
        };
        if !admitted {
            return Progress::Exhausted(self.exhausted());
        }
        if occurrence.key() > self.reached {
            self.reached = occurrence.key();
        }
        Progress::Emit(occurrence)
    }

    fn admits(&mut self, occurrence: Occurrence<'a>) -> bool {
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

    /// Report candidate work even when the search yields no occurrences.
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

    fn record(&mut self, outcome: SearchOutcome) {
        if matches!(self.outcome, SearchOutcome::Pending) {
            self.outcome = outcome;
        }
    }
}

impl<S: DiagnosticSink + ?Sized> fmt::Debug for RecurrenceSearch<'_, S> {
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
                Progress::CalendarArithmeticFailed => {
                    self.finished = true;
                    return Some(SearchStep::CalendarArithmeticFailed);
                },
            }
        }
    }
}

impl<S: DiagnosticSink + ?Sized> FusedIterator for RecurrenceSearch<'_, S> {}

impl<'a> RecurrenceInput<'a> {
    /// Search the series and charge the caller's shared meter.
    /// The meter bounds the total work across searches.
    pub fn search<S>(
        self,
        window: Window,
        meter: &'a mut Meter,
        sink: &'a mut S,
    ) -> RecurrenceSearch<'a, S>
    where
        S: DiagnosticSink + ?Sized,
    {
        RecurrenceSearch::start(self, None, window, meter, sink)
    }

    /// Resume the same series in the same window or a later one.
    /// The cursor preserves `COUNT`, delivered additions, and rule keys that can still enter a later window.
    pub fn resume<S>(
        self,
        cursor: SearchCursor,
        window: Window,
        meter: &'a mut Meter,
        sink: &'a mut S,
    ) -> RecurrenceSearch<'a, S>
    where
        S: DiagnosticSink + ?Sized,
    {
        RecurrenceSearch::start(self, Some(cursor), window, meter, sink)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::NonZeroU32;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Instant, Limits, Meter,
        UtcOffset,
    };

    use super::{RecurrenceSearch, SearchCursor};
    use crate::internal::recur::input::{
        Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput,
    };
    use crate::internal::recur::rule::{
        ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleLimit, ValueKind,
    };
    use crate::internal::recur::search::{
        BudgetExhausted, Occurrence, OverrideProvenance, SearchOutcome, SearchStep, Window,
    };

    #[derive(Default)]
    pub(super) struct ObservedOriginMap {
        keys: alloc::collections::BTreeMap<Instant, u64>,
        reads: core::sync::atomic::AtomicUsize,
    }

    impl ObservedOriginMap {
        pub(super) fn values(&self) -> impl Iterator<Item = &u64> {
            self.keys.values().inspect(|_| {
                self.reads
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            })
        }

        fn reads(&self) -> usize {
            self.reads.load(core::sync::atomic::Ordering::Relaxed)
        }
    }

    impl core::ops::Deref for ObservedOriginMap {
        type Target = alloc::collections::BTreeMap<Instant, u64>;

        fn deref(&self) -> &Self::Target {
            self.reads
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            &self.keys
        }
    }

    impl core::ops::DerefMut for ObservedOriginMap {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.keys
        }
    }

    #[test]
    fn adjusted_checkpoints_read_no_buffered_origins() {
        let start = at(2024, 4, 1, 0);
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .rscale(crate::internal::core::CalendarScale::Gregorian)
            .skip(crate::internal::recur::rule::Skip::Forward)
            .by_month_day(ByList::from_slice(&(1..=30).collect::<Vec<_>>()))
            .by_hour(ByList::from_slice(&(0..24).collect::<Vec<_>>()))
            .by_minute(ByList::from_slice(&(0..60).collect::<Vec<_>>()))
            .build()
            .unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let input = plain(start, &rule, &mut meter);
        let window = Window::new(start, at(2024, 5, 1, 0)).unwrap();
        let mut sink = Vec::new();
        let mut search = input.search(window, &mut meter, &mut sink);
        for minute in 0..43_200 {
            let key = search.next().unwrap().occurrence().unwrap().key();
            assert_eq!(key, start.checked_add_seconds(minute * 60).unwrap());
            let buffered = &search.stream.adjusted.pending.keys;
            if minute == 0 {
                let before = buffered.reads();
                assert_eq!(buffered.values().count(), 43_199);
                assert_eq!(buffered.reads() - before, 43_199);
            }
            let before = buffered.reads();
            let cursor = search.cursor();
            assert_eq!(cursor.resume_after(), Some(key));
            assert_eq!(buffered.reads() - before, 0, "minute {minute}");
        }
        assert!(search.next().is_none());
    }

    fn selected(stamps: &[CivilDateTime]) -> super::SelectedCandidates {
        let candidates = crate::internal::recur::byparts::CandidateSet::from_ascending(stamps);
        super::select(&candidates, &ByList::from_slice(&[]))
    }

    #[test]
    fn adjusted_checkpoint_origins_match_remaining_keys_across_periods() {
        use alloc::collections::BTreeMap;

        fn pop(buffer: &mut super::PendingAdjustedKeys, origins: &mut BTreeMap<Instant, u64>) {
            let earliest = origins.values().copied().min();
            assert_eq!(buffer.earliest_period, earliest);
            let head = origins.pop_first().map(|(key, _)| key);
            if let Some(key) = head {
                assert_eq!(buffer.pop_before(Some(key)), None);
            }
            assert_eq!(buffer.pop_before(None), head.zip(earliest));
            assert_eq!(buffer.earliest_period, origins.values().copied().min());
        }

        fn check(masks: [u8; 3], early_pops: usize) {
            let mut buffer = super::PendingAdjustedKeys::default();
            let mut origins = BTreeMap::new();
            let mut meter = Meter::new(Limits::DEFAULT);
            for (origin, mask) in [3, 7, 9].into_iter().zip(masks) {
                let mut stamps = Vec::new();
                for bit in 0..3 {
                    if mask & (1 << bit) != 0 {
                        let key = at(2024, 1, bit + 1, 0);
                        origins.entry(key).or_insert(origin);
                        stamps.push(CivilDateTime::from_instant(key, UtcOffset::UTC).unwrap());
                    }
                }
                buffer
                    .extend_period(&selected(&stamps), origin, &mut meter)
                    .unwrap();
                assert_eq!(buffer.earliest_period, origins.values().copied().min());
                if origin == 7 {
                    for _ in 0..early_pops {
                        pop(&mut buffer, &mut origins);
                    }
                }
            }
            while !origins.is_empty() {
                pop(&mut buffer, &mut origins);
            }
            pop(&mut buffer, &mut origins);
        }

        for first in 0..8 {
            for second in 0..8 {
                for third in 0..8 {
                    for early_pops in 0..=3 {
                        check([first, second, third], early_pops);
                    }
                }
            }
        }
    }

    #[test]
    fn an_adjusted_period_charges_before_changing_its_checkpoint() {
        let stamp = |day| CivilDateTime::from_instant(at(2024, 1, day, 0), UtcOffset::UTC).unwrap();
        let mut pending = super::PendingAdjustedKeys::default();
        let mut setup = Meter::new(Limits::DEFAULT);
        pending
            .extend_period(&selected(&[stamp(2), stamp(4)]), 3, &mut setup)
            .unwrap();
        let addition = selected(&[stamp(1), stamp(5)]);
        let mut tight = Meter::with_budget(Limits::DEFAULT, 1);
        assert_eq!(
            pending.extend_period(&addition, 7, &mut tight),
            Err(crate::internal::core::LimitExceeded::Budget)
        );
        assert_eq!(pending.keys.len(), 2);
        assert_eq!(pending.earliest_period, Some(3));
        let mut fresh = Meter::with_budget(Limits::DEFAULT, 2);
        pending.extend_period(&addition, 7, &mut fresh).unwrap();
        assert_eq!(fresh.spent(), 2);
        for (day, earliest) in [(1, 3), (2, 3), (4, 3), (5, 7)] {
            assert_eq!(
                pending.pop_before(None),
                Some((at(2024, 1, day, 0), earliest))
            );
        }
        assert_eq!(pending.earliest_period, None);
    }

    #[test]
    fn adjusted_checkpoints_resume_after_every_budget_refusal() {
        let mut setup = Meter::new(Limits::DEFAULT);
        let rule = crate::internal::recur::parse_recur(
            b"FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTHDAY=1,31;COUNT=8",
            &mut setup,
            &mut Vec::new(),
        )
        .unwrap();
        let expected = [
            (1, 1),
            (1, 31),
            (2, 1),
            (3, 1),
            (3, 31),
            (4, 1),
            (5, 1),
            (5, 31),
        ]
        .map(|(month, day)| at(2024, month, day, 0));
        let input = plain(expected[0], &rule, &mut setup);
        let window = Window::new(expected[0], at(2024, 6, 1, 0)).unwrap();
        let mut sink = Vec::new();
        let mut full = Meter::new(Limits::DEFAULT);
        assert_eq!(
            drain(input.search(window, &mut full, &mut sink)).starts,
            expected
        );
        for budget in 0..full.spent() {
            let mut tight = Meter::with_budget(Limits::DEFAULT, budget);
            let mut search = input.search(window, &mut tight, &mut sink);
            let mut starts = Vec::new();
            loop {
                match search.next() {
                    Some(SearchStep::Occurrence(occurrence)) => starts.push(occurrence.start()),
                    Some(SearchStep::BudgetExhausted(_)) => break,
                    step => panic!("expected budget refusal at {budget}, got {step:?}"),
                }
            }
            let checkpoint = search.cursor();
            assert_eq!(checkpoint, search.cursor());
            let mut retry = Meter::new(Limits::DEFAULT);
            let remaining = drain(input.resume(checkpoint, window, &mut retry, &mut sink));
            assert_eq!(remaining.outcome, SearchOutcome::RuleEnded);
            starts.extend(remaining.starts);
            assert_eq!(starts, expected, "budget {budget}");
        }
    }

    /// Use UTC to compare the civil dates in the RFC examples.
    fn at(year: u16, month: u8, day: u8, hour: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, 0, 0).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    #[derive(Debug)]
    struct Run {
        starts: Vec<Instant>,
        terminal: Option<BudgetExhausted>,
        outcome: SearchOutcome,
        steps: usize,
    }

    fn drain<S>(mut search: RecurrenceSearch<'_, S>) -> Run
    where
        S: crate::internal::core::DiagnosticSink + ?Sized,
    {
        let mut starts = Vec::new();
        let mut terminal = None;
        let mut produced = 0_usize;
        for step in &mut search {
            produced = produced.saturating_add(1);
            match step {
                SearchStep::Occurrence(occurrence) => starts.push(occurrence.start()),
                SearchStep::BudgetExhausted(exhausted) => terminal = Some(exhausted),
                SearchStep::CalendarArithmeticFailed => panic!("calendar arithmetic failed"),
            }
        }
        Run {
            starts,
            terminal,
            outcome: search.outcome(),
            steps: produced,
        }
    }

    fn counted(freq: Freq, count: u32) -> RecurrenceRule {
        RecurrenceRuleBuilder::new(freq)
            .limit(RuleLimit::Count(NonZeroU32::new(count).unwrap()))
            .build()
            .unwrap()
    }

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

    #[test]
    fn a_completed_count_ends_before_the_next_period_bound() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 1);
        let input = plain(at(2024, 1, 1, 9), &rule, &mut meter);
        let window = Window::new(at(2024, 1, 1, 0), at(2024, 1, 2, 0)).unwrap();
        let run = drain(input.search(window, &mut meter, &mut Vec::new()));
        assert_eq!(run.starts, [at(2024, 1, 1, 9)]);
        assert_eq!(run.outcome, SearchOutcome::RuleEnded);
        assert_eq!(run.terminal, None);
        assert_eq!(meter.candidates_charged(), 3);
    }

    #[test]
    fn a_completed_count_does_not_spend_budget_on_another_period() {
        let mut meter = Meter::with_budget(Limits::DEFAULT, 3);
        let rule = counted(Freq::Daily, 1);
        let input = plain(at(2024, 1, 1, 9), &rule, &mut meter);
        let window = Window::new(at(2024, 1, 1, 0), at(2024, 1, 3, 0)).unwrap();
        let run = drain(input.search(window, &mut meter, &mut Vec::new()));
        assert_eq!(run.starts, [at(2024, 1, 1, 9)]);
        assert_eq!(run.outcome, SearchOutcome::RuleEnded);
        assert_eq!(run.terminal, None);
        assert!(!meter.is_exhausted());
    }

    #[test]
    fn short_frequency_bounds_keep_their_clock_precision() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = RecurrenceRuleBuilder::new(Freq::Hourly)
            .by_hour(ByList::from_slice(&[23_u8]))
            .build()
            .unwrap();
        let input = plain(at(2024, 1, 1, 23), &rule, &mut meter);
        let window = Window::new(at(2024, 1, 2, 0), at(2024, 1, 2, 1)).unwrap();
        let run = drain(input.search(window, &mut meter, &mut Vec::new()));
        assert_eq!(run.starts, Vec::<Instant>::new());
        assert_eq!(run.outcome, SearchOutcome::WindowEnded);
        assert_eq!(run.terminal, None);
        assert_eq!(meter.candidates_charged(), 5);
    }

    #[test]
    fn a_later_week_year_start_does_not_expand_outside_the_window() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = crate::internal::recur::parse_recur(
            b"FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU",
            &mut meter,
            &mut Vec::new(),
        )
        .unwrap();
        let input = plain(at(2019, 1, 1, 9), &rule, &mut meter);
        let window = Window::new(at(2021, 1, 1, 0), at(2021, 1, 2, 0)).unwrap();
        let run = drain(input.search(window, &mut meter, &mut Vec::new()));
        assert_eq!(run.starts, Vec::<Instant>::new());
        assert_eq!(run.outcome, SearchOutcome::WindowEnded);
        assert_eq!(run.terminal, None);
        assert_eq!(meter.candidates_charged(), 739);
    }

    /// RFC 5545 section 3.8.5.3 lists ten daily occurrences from September 2 through 11, 1997.
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

    /// RFC 5545 section 3.8.5.3 lists the first and last days of successive months starting September 30, 1998.
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

    /// RFC 5545 section 3.3.10 excludes invalid dates from the recurrence count.
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

    #[test]
    fn a_search_that_exhausts_its_budget_reports_it_three_times_and_then_stops() {
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

    #[test]
    fn a_rule_that_outlives_the_calendar_says_so_rather_than_claiming_the_rule_ended() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        let input = plain(at(9999, 12, 28, 9), &rule, &mut meter);
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

    #[test]
    fn an_exclusion_on_an_addition_leaves_every_occurrence_around_it() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = counted(Freq::Daily, 3);
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

    #[test]
    fn a_cursor_keeps_the_first_addition_beyond_the_window() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let additions = [at(1997, 9, 2, 9), at(1997, 9, 11, 9), at(1997, 9, 20, 9)];
        let entries = [Override::new(
            at(1997, 9, 4, 9),
            OverrideRange::ThisOnly,
            Some(at(1997, 9, 6, 9)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month_day(ByList::from_slice(&[2, 11]))
            .limit(RuleLimit::Count(NonZeroU32::new(2).unwrap()))
            .build()
            .unwrap();
        let first = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let later = Window::new(first.end(), at(1997, 10, 1, 0)).unwrap();
        let sparse = [additions[0], additions[2]];
        for (cadence, rdates) in [
            (None, &additions[..]),
            (Some(&rule), &additions),
            (Some(&rule), &sparse),
        ] {
            let input = RecurrenceInput::new(
                additions[0],
                ValueKind::DateTime,
                cadence,
                rdates,
                &[],
                overrides,
                &mut meter,
            )
            .unwrap();
            let mut sink: Vec<Diagnostic> = Vec::new();
            let cursor = {
                let mut search = input.search(first, &mut meter, &mut sink);
                assert_eq!(
                    search.next().unwrap().occurrence().unwrap().key(),
                    additions[0]
                );
                assert!(search.next().is_none());
                search.cursor()
            };
            if cadence.is_some() {
                assert_eq!(cursor.occurrences_emitted(), 2);
            }
            let mut rest = input.resume(cursor, later, &mut meter, &mut sink);
            let first = rest.next().unwrap().occurrence().unwrap();
            assert_eq!(first.start(), additions[1]);
            let added = OverrideProvenance::AddedByRdate;
            assert_eq!(first.provenance(), cadence.is_none().then_some(added));
            assert_eq!(drain(rest).starts, additions[2..]);
        }
    }

    #[test]
    fn widened_rule_keys_survive_holes_and_a_refused_replay() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let day = |d| at(1997, 9, d, 9);
        let moved = |key, start| {
            Override::new(
                key,
                OverrideRange::ThisOnly,
                Some(start),
                PropertyDiff::empty(),
            )
        };
        let entries = [moved(day(12), day(9)), moved(day(14), day(8))];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let rule = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_month_day(ByList::from_slice(&[2, 11, 12, 13, 14]))
            .limit(RuleLimit::Count(NonZeroU32::new(5).unwrap()))
            .build()
            .unwrap();
        let input = RecurrenceInput::new(
            day(2),
            ValueKind::DateTime,
            Some(&rule),
            &[],
            &[],
            overrides,
            &mut meter,
        )
        .unwrap();
        let first = Window::new(day(1), day(10)).unwrap();
        let later = Window::new(first.end(), day(20)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut search = input.search(first, &mut meter, &mut sink);
        let starts: Vec<_> = search
            .by_ref()
            .map(|step| step.occurrence().unwrap().start())
            .collect();
        assert_eq!(starts, [day(2), day(9), day(8)]);
        let cursor = search.cursor();
        assert_eq!(cursor.occurrences_emitted(), 5);
        drop(search);
        let cursor = SearchCursor::new(
            cursor.resume_after(),
            cursor.occurrences_emitted(),
            cursor.rule_cursor(),
        );
        let mut no_copy = Meter::with_budget(Limits::DEFAULT, 2);
        let mut refused = input.resume(cursor.clone(), later, &mut no_copy, &mut sink);
        assert!(matches!(
            refused.next(),
            Some(SearchStep::BudgetExhausted(_))
        ));
        assert_eq!(refused.cursor(), cursor);
        drop(refused);
        let mut tight = Meter::new(Limits::DEFAULT.with_occurrences_per_search(1));
        let mut resumed = input.resume(cursor.clone(), later, &mut tight, &mut sink);
        assert_eq!(
            resumed
                .next()
                .and_then(|step| step.occurrence().map(Occurrence::key)),
            Some(day(11))
        );
        let after_one = resumed.cursor();
        assert!(matches!(
            resumed.next(),
            Some(SearchStep::BudgetExhausted(_))
        ));
        assert_eq!(resumed.cursor(), after_one);
        assert_eq!(after_one.occurrences_emitted(), 5);
        drop(resumed);
        let mut fresh = Meter::new(Limits::DEFAULT);
        assert_eq!(
            drain(input.resume(after_one, later, &mut fresh, &mut sink)).starts,
            [day(13)]
        );
        assert_eq!(
            drain(input.resume(cursor, later, &mut fresh, &mut sink)).starts,
            [day(11), day(13)]
        );
    }

    #[test]
    fn each_refused_rule_checkpoint_preserves_count_and_window_holes() {
        let start = at(1997, 9, 2, 9);
        let at_second = |second| start.checked_add_seconds(second).unwrap();
        let shift = |key, to| {
            Override::new(
                at_second(key),
                OverrideRange::ThisOnly,
                Some(at_second(to)),
                PropertyDiff::empty(),
            )
        };
        let entries = [shift(3, 7), shift(5, 1), shift(6, 1)];
        let mut setup = Meter::new(Limits::DEFAULT);
        let overrides = OverrideSet::new(&entries, &mut setup).unwrap();
        let rule = counted(Freq::Secondly, 7);
        let input = RecurrenceInput::new(
            start,
            ValueKind::DateTime,
            Some(&rule),
            &[],
            &[],
            overrides,
            &mut setup,
        )
        .unwrap();
        let first = Window::new(start, at_second(3)).unwrap();
        let next = Window::new(at_second(4), at_second(5)).unwrap();
        let tail = Window::new(at_second(7), at_second(8)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut measured = Meter::new(Limits::DEFAULT);
        let full = drain(input.search(first, &mut measured, &mut sink));
        assert_eq!(full.starts, [0, 1, 2, 1, 1].map(at_second));
        for units in 0..=measured.spent() {
            let mut small = Meter::with_budget(Limits::DEFAULT, units);
            let mut search = input.search(first, &mut small, &mut sink);
            let mut prefix: Vec<_> = search
                .by_ref()
                .filter_map(|step| step.occurrence().map(Occurrence::start))
                .collect();
            let cursor = search.cursor();
            drop(search);
            let mut fresh = Meter::new(Limits::DEFAULT);
            let mut retry = input.resume(cursor, first, &mut fresh, &mut sink);
            prefix.extend(
                retry
                    .by_ref()
                    .map(|step| step.occurrence().unwrap().start()),
            );
            assert_eq!(prefix, full.starts, "budget {units}");
            let cursor = retry.cursor();
            assert_eq!(cursor.occurrences_emitted(), 7, "budget {units}");
            drop(retry);
            let mut middle = input.resume(cursor.clone(), next, &mut fresh, &mut sink);
            assert_eq!(
                middle.next().unwrap().occurrence().unwrap().key(),
                at_second(4),
                "budget {units}"
            );
            assert!(middle.next().is_none());
            let last = middle.cursor();
            drop(middle);
            assert_eq!(
                drain(input.resume(last, tail, &mut fresh, &mut sink)).starts,
                [at_second(7)],
                "budget {units}"
            );
        }
    }

    #[test]
    fn resuming_keeps_unseen_additions_before_a_shifted_addition() {
        let start = at(1997, 9, 2, 9);
        let unseen = at(1997, 9, 11, 9);
        let shifted = at(1997, 9, 12, 9);
        let moved = at(1997, 9, 9, 9);
        let additions = [start, unseen, shifted];
        let entries = [Override::new(
            shifted,
            OverrideRange::ThisOnly,
            Some(moved),
            PropertyDiff::empty(),
        )];
        let mut meter = Meter::new(Limits::DEFAULT);
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = RecurrenceInput::new(
            start,
            ValueKind::DateTime,
            None,
            &additions,
            &[],
            overrides,
            &mut meter,
        )
        .unwrap();
        let first = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let later = Window::new(first.end(), at(1997, 9, 20, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();
        let cursor = {
            let mut search = input.search(first, &mut meter, &mut sink);
            assert_eq!(search.next().unwrap().occurrence().unwrap().key(), start);
            assert_eq!(search.next().unwrap().occurrence().unwrap().start(), moved);
            assert!(search.next().is_none());
            search.cursor()
        };
        let mut fresh = Meter::new(Limits::DEFAULT);
        assert_eq!(
            drain(input.resume(cursor, later, &mut fresh, &mut sink)).starts,
            [unseen]
        );
    }

    #[test]
    fn a_refused_history_copy_replays_multiple_shifted_additions() {
        let start = at(1997, 9, 2, 9);
        let additions = [
            start,
            at(1997, 9, 11, 9),
            at(1997, 9, 12, 9),
            at(1997, 9, 13, 9),
            at(1997, 9, 14, 9),
        ];
        let first_move = at(1997, 9, 9, 9);
        let second_move = at(1997, 9, 8, 9);
        let entries = [
            Override::new(
                additions[2],
                OverrideRange::ThisOnly,
                Some(first_move),
                PropertyDiff::empty(),
            ),
            Override::new(
                additions[4],
                OverrideRange::ThisOnly,
                Some(second_move),
                PropertyDiff::empty(),
            ),
        ];
        let mut setup = Meter::new(Limits::DEFAULT);
        let overrides = OverrideSet::new(&entries, &mut setup).unwrap();
        let input = RecurrenceInput::new(
            start,
            ValueKind::DateTime,
            None,
            &additions,
            &[],
            overrides,
            &mut setup,
        )
        .unwrap();
        let first = Window::new(at(1997, 9, 1, 0), at(1997, 9, 10, 0)).unwrap();
        let later = Window::new(first.end(), at(1997, 9, 20, 0)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut one = Meter::new(Limits::DEFAULT.with_occurrences_per_search(1));
        let mut limited_count = input.search(first, &mut one, &mut sink);
        assert_eq!(
            limited_count.next().unwrap().occurrence().unwrap().start(),
            start
        );
        let before = limited_count.cursor();
        assert!(matches!(
            limited_count.next(),
            Some(SearchStep::BudgetExhausted(_))
        ));
        assert_eq!(limited_count.cursor(), before);
        drop(limited_count);
        let mut measured = Meter::new(Limits::DEFAULT);
        {
            let mut search = input.search(first, &mut measured, &mut sink);
            for expected in [start, first_move, second_move] {
                assert_eq!(
                    search.next().unwrap().occurrence().unwrap().start(),
                    expected
                );
            }
        }
        let mut limited = Meter::with_budget(Limits::DEFAULT, measured.spent());
        let mut search = input.search(first, &mut limited, &mut sink);
        for expected in [start, first_move] {
            assert_eq!(
                search.next().unwrap().occurrence().unwrap().start(),
                expected
            );
        }
        let snapshot = search.cursor();
        let repeated = search.cursor();
        assert_eq!(repeated, snapshot);
        assert!(alloc::rc::Rc::ptr_eq(
            snapshot.additions.beyond.as_ref().unwrap(),
            repeated.additions.beyond.as_ref().unwrap()
        ));
        assert!(matches!(
            search.next(),
            Some(SearchStep::BudgetExhausted(_))
        ));
        let refused = search.cursor();
        assert_eq!(refused, snapshot);
        drop(search);
        for cursor in [snapshot, refused] {
            let mut fresh = Meter::new(Limits::DEFAULT);
            let mut resumed = input.resume(cursor, first, &mut fresh, &mut sink);
            assert_eq!(
                resumed.next().unwrap().occurrence().unwrap().start(),
                second_move
            );
            let continued = resumed.cursor();
            assert!(resumed.next().is_none());
            drop(resumed);
            assert_eq!(
                drain(input.resume(continued, later, &mut fresh, &mut sink)).starts,
                [additions[1], additions[3]]
            );
        }
    }

    #[test]
    fn shifted_additions_keep_gaps_across_a_bitmap_word_boundary() {
        let start = at(1997, 9, 1, 9);
        let at_second = |offset| start.checked_add_seconds(offset).unwrap();
        let additions: Vec<_> = (0..68).map(at_second).collect();
        let entries = [65, 67].map(|index| {
            Override::new(
                additions[index],
                OverrideRange::ThisOnly,
                Some(start),
                PropertyDiff::empty(),
            )
        });
        let mut meter = Meter::new(Limits::DEFAULT);
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = RecurrenceInput::new(
            start,
            ValueKind::DateTime,
            None,
            &additions,
            &[],
            overrides,
            &mut meter,
        )
        .unwrap();
        let first = Window::new(start, at_second(64)).unwrap();
        let later = Window::new(first.end(), at_second(68)).unwrap();
        let mut sink: Vec<Diagnostic> = Vec::new();
        let mut search = input.search(first, &mut meter, &mut sink);
        let keys: Vec<_> = search
            .by_ref()
            .map(|step| step.occurrence().unwrap().key())
            .collect();
        assert_eq!(
            keys,
            additions[..64]
                .iter()
                .copied()
                .chain([additions[65], additions[67]])
                .collect::<Vec<_>>()
        );
        let cursor = search.cursor();
        drop(search);
        assert_eq!(
            drain(input.resume(cursor, later, &mut meter, &mut sink)).starts,
            [additions[64], additions[66]]
        );
    }

    #[test]
    fn a_cursor_retries_the_initial_rule_key_after_a_refused_step() {
        let start = at(1997, 9, 2, 9);
        let window = Window::new(start, at(1997, 9, 10, 0)).unwrap();
        let rule = counted(Freq::Daily, 2);
        let mut fresh = Meter::new(Limits::DEFAULT);
        let input = plain(start, &rule, &mut fresh);
        for mut tight in [
            Meter::with_budget(Limits::DEFAULT, 1),
            Meter::new(Limits::DEFAULT.with_occurrences_per_search(0)),
        ] {
            let mut sink: Vec<Diagnostic> = Vec::new();
            let cursor = {
                let mut search = input.search(window, &mut tight, &mut sink);
                assert!(matches!(
                    search.next(),
                    Some(SearchStep::BudgetExhausted(_))
                ));
                search.cursor()
            };
            assert_eq!(cursor.occurrences_emitted(), 0);
            assert_eq!(
                drain(input.resume(cursor, window, &mut fresh, &mut sink)).starts,
                [start, at(1997, 9, 3, 9)]
            );
        }
    }

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
