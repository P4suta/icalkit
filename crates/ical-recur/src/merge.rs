// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 5 — merging `RDATE`, `EXDATE` and the `RECURRENCE-ID` overrides into the rule stream.
//!
//! # What this unit owns
//!
//! One ordered merge of four sources — rule candidates, `RDATE` additions, `EXDATE`
//! exclusions, and the override table — producing [`crate::search::Occurrence`] values with
//! their provenance tagged and their diffs composed. It is a linear merge over sorted inputs
//! and it materializes nothing.
//!
//! Linear is a claim about this file and not only about its inputs. All four sources are walked
//! by cursors that only move forward, the override table included:
//! [`crate::input::OverrideSet::anchors_before`] restarts at the beginning of that table for
//! every key it is asked about, which is the right shape for a caller recomposing one occurrence
//! on demand and the wrong shape for a merge that will ask about every key in order. So the
//! merge keeps its own position in the override slice and folds each anchor as it passes it.
//! [`crate::search::Occurrence::applied_anchors`] still recomposes from the start, so a caller
//! sees the same set the merge folded, arrived at the other way.
//!
//! # How a caller drives it
//!
//! The merge does not own the rule. It is offered the rule's *next* cadence key without that key
//! being consumed, so that an `RDATE` falling before it can be emitted first, and it says
//! whether it took the offer:
//!
//! ```text
//! loop {
//!     let offered = rule.peek();                       // None once the rule has ended
//!     if merge.is_drained(offered) { break; }
//!     let taken = merge.takes_rule_key(offered);
//!     let produced = merge.step(offered, meter, sink);
//!     if taken { rule.advance(); }
//!     if let Some(occurrence) = produced { /* hand it on */ }
//! }
//! ```
//!
//! [`Merge::step`] answers `None` for a candidate that was excluded as well as for no candidate
//! at all, which is why [`Merge::is_drained`] is asked first rather than inferred from the
//! answer: an `EXDATE` that removes the last instance of a series and the end of the series are
//! otherwise the same silence.
//!
//! The offered key is whatever the engine generates, `DTSTART` included. Nothing here
//! synthesizes the first instance: RFC 5545 section 3.8.5.3 makes `DTSTART` the first occurrence
//! of the rule, and a merge that added it as well would double it for every engine that already
//! does. Candidates must ascend, which they do because both streams do; the cursors would miss
//! an exclusion if they did not.
//!
//! # The precedence, stated rather than emergent
//!
//! 1. **`EXDATE` wins over an override.** An instant in both an `EXDATE` list and the override
//!    table is dropped, and [`ical_core::DiagnosticCode::ExdateShadowsOverride`] says so. There
//!    is no RFC behind this; an intentional deletion beating a modification is this project's
//!    choice.
//! 2. **The exclusion is scoped to the instant, never to the override object.** A redundant
//!    `EXDATE` landing on a `RANGE=THISANDFUTURE` anchor's own key removes that one occurrence
//!    and leaves the anchor's diff in force for every later candidate. The other reading turns
//!    one duplicated line — which real exporters have shipped — into the silent reversion of an
//!    unbounded tail of the series. Structurally, the exclusion is a test applied to a candidate
//!    and the override table is never edited, so there is nothing for a later key to have lost.
//! 3. **Provenance is `ExactMatch > ThisAndFuture > AddedByRdate`**, and the tag names the
//!    nearest anchor while application composes *every* anchor at or before the key. An
//!    `RDATE`-added instant that falls after an anchor is tagged `ThisAndFuture`, because an
//!    anchor reaches every later instance of the series and an `RDATE` instant is one of those;
//!    that reading is the only reason the last two arms of the precedence can ever compete.
//! 4. **`RANGE=THISANDFUTURE` is a property diff, not a time delta.** An anchor that changes
//!    only `LOCATION` changes `LOCATION` on every later instance, each of which keeps its own
//!    time. Implementing this as a scalar shift is the bug five of seven bake-off proposals
//!    shared, and [`crate::input::Override::shift_seconds`] derives the shift from the move
//!    precisely so that nothing can store one instead.
//! 5. **An `RDATE` coinciding with a rule instance yields one occurrence, not two.** The
//!    recurrence set is a set. The survivor is not tagged `AddedByRdate`, because that tag means
//!    "no rule generated it" and a rule generated this one.
//! 6. **Two occurrences may share an effective start, and both are emitted.** This is the case
//!    `docs/adr/0002` files without an answer: an `RDATE` names an instant that is also where a
//!    diff moved a rule instance to. The answer taken here is that they do not merge, for three
//!    reasons. Identity in this crate is the cadence key — it is what a `RECURRENCE-ID`
//!    addresses, what an `EXDATE` removes and what `COUNT` counts — and these two candidates have
//!    different keys, so fusing them would leave one addressable and the other silently gone from
//!    a file that names it. Second, a caller can still fuse them and cannot unfuse them. Third, a
//!    dedup keyed on effective start is not a linear merge at all: starts are not sorted, since a
//!    shift may reorder them, so recognizing the collision would need a buffer holding every
//!    start still in flight — the materialization this unit exists not to do. The visible cost is
//!    a renderer showing two entries at one time, which the file's author can see and correct;
//!    the cost of the other answer is invisible.
//!
//! # The shift, composed rather than accumulated
//!
//! An anchor that moved its own instance states a shift, derived by
//! [`crate::input::Override::shift_seconds`] from its `RECURRENCE-ID` to where it moved. That
//! shift reaches later cadence keys the same way the anchor's `LOCATION` does: it is one stated
//! field of one diff. So shifts compose by the rule the rest of the diff composes by — a later
//! anchor's stated shift overwrites an earlier one's, and an anchor that moved nothing states no
//! shift and therefore overwrites none. An anchor of one hour followed by one of half an hour
//! leaves the tail half an hour from its cadence, never an hour and a half: both shifts are
//! measured from a cadence key, so adding them would count the first one twice. An exact-match
//! override that names where it moved to is applied last and absolutely, since it is talking
//! about one instance rather than about a cadence — and one that moved nothing states nothing
//! about the time, so an anchor's shift survives it.
//!
//! **A shift does not reach an instant an `RDATE` named.** A cadence key is a position the
//! anchor is entitled to move; an `RDATE` value is a literal instant the file states, and there
//! is no cadence in it to shift. Moving it would render a meeting at an hour no line of the file
//! contains. The anchor's *property* diff still reaches it — a room that changed for every later
//! meeting changed for the extra one too — which is the same asymmetry precedence 4 draws, seen
//! from the other side.
//!
//! # A start that cannot exist
//!
//! `key + shift` is checked, because it is arithmetic on numbers an attacker supplied: an anchor
//! may state a shift of half the timeline. When the sum leaves the representable timeline the
//! occurrence is filtered rather than moved to a nearby one, which is `docs/adr/0011`'s rule, and
//! it is reported on [`ical_core::DiagnosticCode::OverrideShiftNotRepresentable`]. That is a
//! different fact from [`ical_core::DiagnosticCode::NonexistentRecurrenceInstance`], which names
//! a date RFC 5545 section 3.3.10 defines away in a legal file; here the file asked for an
//! instant no calendar can hold, so the two travel apart.
//!
//! # What this unit must not do
//!
//! - It must not sort. The lists arrive sorted or they are refused, which is what keeps the
//!   advertised linear cost true and keeps an `O(n log n)` and an allocation out of a `no_std`
//!   path.
//! - It must not filter by window. A window admits by cadence key and an override may move a
//!   start out of it; the filtering, the widening and
//!   [`ical_core::DiagnosticCode::OverrideLeftWindow`] are unit 7's.
//! - It must not interpret `EXRULE`. RFC 5545 obsoleted it; `ical-core` preserves it.
//! - It must not merge a second `RRULE`. The extra is dropped with
//!   [`ical_core::DiagnosticCode::ExtraRecurrenceRuleIgnored`], because `COUNT` is ambiguous
//!   across a union and the cursor carries one counter. [`keep_first_rule`] is where that
//!   happens, and it is here rather than beside [`crate::input::RecurrenceInput`] because that
//!   type holds one rule by construction — by the time an input exists the second rule is
//!   already gone, and a drop nobody reported is the silence this crate is against.
//! - It must not charge a candidate or an occurrence. Unit 7 owns those two charge sites. The
//!   meter travels here because [`ical_core::report_diagnostic`] charges a refused diagnostic to
//!   it, and because the three lists were charged to it before this type ever saw them.
//!
//! # How it is tested on its own
//!
//! The cases `docs/adr/0002` files by name: an instant in both `EXDATE` and the override table;
//! two chained anchors whose second diff is minimal; a `THISANDFUTURE` override changing only
//! `LOCATION`; and the one filed without an answer, whose chosen answer is stated above and
//! asserted below. Around them, the boundaries every date computation in this workspace has to
//! cross — the leap day, the month end, the year boundary — because a merge that compared
//! anything other than the instant scalar would survive a table of round numbers and fail on
//! 2024-02-29.

use ical_core::{
    Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity, report_diagnostic,
};

use crate::input::{Override, RecurrenceInput};
use crate::rule::RecurrenceRule;
use crate::search::{Occurrence, OverrideProvenance};

/// One candidate instant and where it came from, before anything decided whether it survives.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    /// The cadence key.
    key: Instant,
    /// Whether an `RDATE` alone put it there.
    added_by_rdate: bool,
}

/// The linear merge of a rule's cadence keys with the three lists a caller supplied.
///
/// `Clone` for a caller that wants to look ahead, and deliberately not `Copy`: every field below
/// is a position in a walk, and a cursor that copies itself on use is a cursor a caller advances
/// twice by accident.
#[derive(Clone, Debug)]
pub struct Merge<'a> {
    /// What the caller offered.
    input: RecurrenceInput<'a>,
    /// How many `RDATE` instants have been merged into the stream.
    rdates_merged: usize,
    /// How many `EXDATE` instants the candidate stream has already walked past.
    exdates_passed: usize,
    /// How many override entries the candidate stream has already walked past.
    overrides_passed: usize,
    /// The `RECURRENCE-ID` of the nearest anchor at or before the newest candidate.
    nearest_anchor: Option<Instant>,
    /// The shift the newest anchor that stated one implies, in seconds from a cadence key.
    stated_shift: Option<i64>,
}

impl<'a> Merge<'a> {
    /// A merge over everything `input` says about which occurrences exist.
    ///
    /// Infallible and unmetered: [`crate::input::RecurrenceInput::new`] already refused an
    /// unsorted list and charged each one to its own dimension, and checking either again here
    /// would be a second opinion that can eventually disagree with the first.
    #[must_use]
    pub const fn new(input: RecurrenceInput<'a>) -> Self {
        Self {
            input,
            rdates_merged: 0,
            exdates_passed: 0,
            overrides_passed: 0,
            nearest_anchor: None,
            stated_shift: None,
        }
    }

    /// Whether the next [`Merge::step`] will consume `next_rule_key` rather than an `RDATE`.
    ///
    /// Asked before the step, because the step moves the `RDATE` cursor this answer depends on.
    /// A key equal to the next `RDATE` is consumed by both, which is precedence 5.
    #[must_use]
    pub fn takes_rule_key(&self, next_rule_key: Option<Instant>) -> bool {
        match (next_rule_key, self.pending_addition()) {
            (Some(generated), Some(offered)) => generated <= offered,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Whether nothing is left to merge, so that a `None` step means the end of the series.
    #[must_use]
    pub fn is_drained(&self, next_rule_key: Option<Instant>) -> bool {
        next_rule_key.is_none() && self.pending_addition().is_none()
    }

    /// Merge one candidate, answering the occurrence it produced if it produced one.
    ///
    /// `next_rule_key` is the rule's next cadence key, unconsumed, and `None` once the rule has
    /// ended. `None` comes back for a candidate an `EXDATE` removed, for one whose start is not
    /// representable, and for no candidate at all; [`Merge::is_drained`] distinguishes the last
    /// of those from the first two.
    pub fn step<S: DiagnosticSink + ?Sized>(
        &mut self,
        next_rule_key: Option<Instant>,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Option<Occurrence<'a>> {
        let candidate = self.take_candidate(next_rule_key)?;
        self.pass_overrides(candidate.key);
        if self.is_excluded(candidate.key) {
            self.report_shadowed_override(candidate.key, meter, sink);
            return None;
        }
        let overrides = self.input.overrides();
        let exact = overrides.exact_match(candidate.key);
        let Some(start) = self.effective_start(candidate, exact) else {
            report_diagnostic(
                sink,
                meter,
                Diagnostic::at_instant(
                    DiagnosticCode::OverrideShiftNotRepresentable,
                    Severity::Violation,
                    candidate.key,
                ),
            );
            return None;
        };
        let provenance = self.provenance_of(candidate, exact);
        Some(Occurrence::new(
            candidate.key,
            start,
            provenance,
            exact,
            overrides,
        ))
    }

    /// The next `RDATE` instant not yet merged.
    fn pending_addition(&self) -> Option<Instant> {
        self.input.rdates().get(self.rdates_merged).copied()
    }

    /// The next candidate in cadence order, consuming the `RDATE` when it supplied one.
    ///
    /// The rule's key is never consumed here — the caller owns the rule — which is what
    /// [`Merge::takes_rule_key`] reports and why re-offering the same key is harmless.
    fn take_candidate(&mut self, next_rule_key: Option<Instant>) -> Option<Candidate> {
        let added = self.pending_addition();
        let key = match (next_rule_key, added) {
            (Some(generated), Some(offered)) => generated.min(offered),
            (Some(generated), None) => generated,
            (None, Some(offered)) => offered,
            (None, None) => return None,
        };
        if added == Some(key) {
            // A coincidence consumes both sources and yields one occurrence: precedence 5.
            self.rdates_merged = self.rdates_merged.saturating_add(1);
        }
        Some(Candidate {
            key,
            added_by_rdate: next_rule_key != Some(key),
        })
    }

    /// Fold every override entry at or before `key` that the walk has not passed yet.
    ///
    /// Candidates ascend, so this cursor only moves forward and the merge stays linear in the
    /// override table overall rather than linear in it per candidate.
    fn pass_overrides(&mut self, key: Instant) {
        let entries = self.input.overrides().entries();
        while let Some(entry) = entries.get(self.overrides_passed) {
            if entry.recurrence_id() > key {
                break;
            }
            if entry.is_anchor() {
                self.nearest_anchor = Some(entry.recurrence_id());
                // Omission is no opinion: an anchor that moved nothing leaves an earlier
                // anchor's shift in force exactly as it leaves that anchor's `LOCATION`.
                if let Some(shift) = entry.shift_seconds() {
                    self.stated_shift = Some(shift);
                }
            }
            self.overrides_passed = self.overrides_passed.saturating_add(1);
        }
    }

    /// Whether an `EXDATE` names `key`, advancing the exclusion cursor up to it.
    ///
    /// The comparison is against the cadence key rather than the effective start, because an
    /// `EXDATE` and a `RECURRENCE-ID` address an instance the same way — which is also what makes
    /// precedence 1 a comparison of two values rather than of two mechanisms.
    fn is_excluded(&mut self, key: Instant) -> bool {
        let exclusions = self.input.exdates();
        while let Some(excluded) = exclusions.get(self.exdates_passed) {
            if *excluded >= key {
                break;
            }
            self.exdates_passed = self.exdates_passed.saturating_add(1);
        }
        exclusions.get(self.exdates_passed) == Some(&key)
    }

    /// Report that an exclusion removed an instant an override also named.
    ///
    /// A note rather than a violation, and reported only when an override actually named the
    /// instant: an `EXDATE` on a plain rule instance is the ordinary case and has nothing to say.
    /// The override table is untouched, which is precedence 2 holding structurally.
    fn report_shadowed_override<S: DiagnosticSink + ?Sized>(
        &self,
        key: Instant,
        meter: &mut Meter,
        sink: &mut S,
    ) {
        if self.input.overrides().exact_match(key).is_none() {
            return;
        }
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(DiagnosticCode::ExdateShadowsOverride, Severity::Note, key),
        );
    }

    /// Where this candidate actually starts, or `None` when that is off the timeline.
    ///
    /// Three readings in order, and the order is the whole content of the function. An exact
    /// match that names where it moved to wins outright, because it is talking about this one
    /// instance. An instant an `RDATE` named is where the file says it is, because there is no
    /// cadence in a literal value for an anchor to shift. Everything else is a cadence key an
    /// anchor is entitled to move.
    fn effective_start(
        &self,
        candidate: Candidate,
        exact: Option<&Override<'a>>,
    ) -> Option<Instant> {
        if let Some(moved) = exact.and_then(|entry| entry.moved_to()) {
            return Some(moved);
        }
        if candidate.added_by_rdate {
            return Some(candidate.key);
        }
        match self.stated_shift {
            Some(shift) => candidate.key.checked_add_seconds(shift),
            None => Some(candidate.key),
        }
    }

    /// Which mechanism produced this candidate, under precedence 3.
    fn provenance_of(
        &self,
        candidate: Candidate,
        exact: Option<&Override<'a>>,
    ) -> Option<OverrideProvenance> {
        if exact.is_some() {
            return Some(OverrideProvenance::ExactMatch);
        }
        if let Some(anchor) = self.nearest_anchor {
            return Some(OverrideProvenance::ThisAndFuture { anchor });
        }
        if candidate.added_by_rdate {
            return Some(OverrideProvenance::AddedByRdate);
        }
        None
    }
}

/// The one `RRULE` a series is expanded from, reporting every extra rather than dropping it.
///
/// RFC 5545 section 3.8.5.3 says `SHOULD NOT` rather than `MUST NOT`, RFC 2445 permitted it, and
/// files with two exist. They are not unioned: `COUNT` counts the instances of *a* rule, so a
/// union has no defined count, and one cursor cannot carry two counters. The first is kept
/// because it is the only choice a reader can make without ranking two rules it has no basis to
/// rank.
///
/// `at` is the instant the complaint is filed against — a series' `DTSTART` is the useful
/// answer — because a second rule is a fact about a component, and a diagnostic raised during
/// expansion has no offset in any file to point at.
#[must_use]
pub fn keep_first_rule<'a, S: DiagnosticSink + ?Sized>(
    rules: &[&'a RecurrenceRule],
    at: Instant,
    meter: &mut Meter,
    sink: &mut S,
) -> Option<&'a RecurrenceRule> {
    let (first, extras) = rules.split_first()?;
    for _ in extras {
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(
                DiagnosticCode::ExtraRecurrenceRuleIgnored,
                Severity::Violation,
                at,
            ),
        );
    }
    Some(*first)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics,
        Instant, Limits, Meter, Property, PropertyId, UtcOffset,
    };

    use super::{Merge, keep_first_rule};
    use crate::input::{
        Override, OverrideRange, OverrideSet, PropertyChange, PropertyDiff, RecurrenceInput,
    };
    use crate::rule::{Freq, RecurrenceRule, RecurrenceRuleBuilder, ValueKind};
    use crate::search::{Occurrence, OverrideProvenance};

    /// The instant a UTC civil date and time name.
    ///
    /// Built through `ical-core`'s checked civil arithmetic rather than from a number somebody
    /// computed once, so that a test naming the leap day is naming the leap day. UTC because
    /// which clock an instant was resolved in is the caller's answer and not this crate's.
    fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, minute, 0).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// The instant `seconds` from the epoch, for the one case that is about the scalar's edge.
    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// An override naming one instant, optionally moved, carrying `diff`.
    fn entry(
        recurrence_id: Instant,
        range: OverrideRange,
        moved_to: Option<Instant>,
        diff: PropertyDiff<'_>,
    ) -> Override<'_> {
        Override::new(recurrence_id, range, moved_to, diff)
    }

    /// Everything one merge produced, driven the way the module documentation says to drive it.
    ///
    /// The loop is the contract: peek, ask, step, advance. A test that advanced the rule on its
    /// own terms would be exercising a protocol no engine uses.
    fn drive<'a>(
        input: RecurrenceInput<'a>,
        rule_keys: &[Instant],
        meter: &mut Meter,
        sink: &mut Vec<Diagnostic>,
    ) -> Vec<Occurrence<'a>> {
        let mut merge = Merge::new(input);
        let mut produced = Vec::new();
        let mut position = 0_usize;
        loop {
            let offered = rule_keys.get(position).copied();
            if merge.is_drained(offered) {
                break;
            }
            let taken = merge.takes_rule_key(offered);
            if let Some(occurrence) = merge.step(offered, meter, sink) {
                produced.push(occurrence);
            }
            if taken {
                position = position.saturating_add(1);
            }
        }
        produced
    }

    /// The cadence keys of everything a merge produced.
    fn keys_of(produced: &[Occurrence<'_>]) -> Vec<Instant> {
        produced.iter().map(|occurrence| occurrence.key()).collect()
    }

    /// The effective starts of everything a merge produced.
    fn starts_of(produced: &[Occurrence<'_>]) -> Vec<Instant> {
        produced
            .iter()
            .map(|occurrence| occurrence.start())
            .collect()
    }

    /// The provenance tags of everything a merge produced.
    fn tags_of(produced: &[Occurrence<'_>]) -> Vec<Option<OverrideProvenance>> {
        produced
            .iter()
            .map(|occurrence| occurrence.provenance())
            .collect()
    }

    /// The codes a sink kept, in the order it kept them.
    fn codes_of(sink: &[Diagnostic]) -> Vec<DiagnosticCode> {
        sink.iter().map(|found| found.code()).collect()
    }

    /// An input over the three lists, with no rule: the merge never reads one.
    fn input_over<'a>(
        dtstart: Instant,
        additions: &'a [Instant],
        exclusions: &'a [Instant],
        overrides: OverrideSet<'a>,
        meter: &mut Meter,
    ) -> RecurrenceInput<'a> {
        RecurrenceInput::new(
            dtstart,
            ValueKind::DateTime,
            None,
            additions,
            exclusions,
            overrides,
            meter,
        )
        .unwrap()
    }

    /// The four sources merge in cadence order across a leap day, a month end and a year end.
    ///
    /// The table is (what the case is, the rule's keys, the `RDATE`s, the `EXDATE`s, what comes
    /// out). Every instant is a date this workspace's civil arithmetic produced, because a merge
    /// that compared anything other than the instant scalar would survive a table of round
    /// numbers and fail on 2024-02-29.
    #[test]
    fn the_lists_merge_in_cadence_order_across_every_boundary_a_date_can_cross() {
        let leap_eve = utc(2024, 2, 28, 9, 0);
        let leap_day = utc(2024, 2, 29, 9, 0);
        let march_end = utc(2024, 3, 31, 9, 0);
        let year_end = utc(2024, 12, 31, 9, 0);
        let new_year = utc(2025, 1, 1, 9, 0);

        let cases = [
            (
                "the leap day is removed and the days either side of it are not",
                Vec::from([leap_eve, leap_day, march_end]),
                Vec::new(),
                Vec::from([leap_day]),
                Vec::from([leap_eve, march_end]),
            ),
            (
                "an addition at a month end sorts between two rule instances",
                Vec::from([leap_day, year_end]),
                Vec::from([march_end]),
                Vec::new(),
                Vec::from([leap_day, march_end, year_end]),
            ),
            (
                "an addition on the far side of a year boundary comes last",
                Vec::from([year_end]),
                Vec::from([new_year]),
                Vec::new(),
                Vec::from([year_end, new_year]),
            ),
            (
                "an addition coinciding with a rule instance yields one occurrence",
                Vec::from([leap_day, march_end]),
                Vec::from([march_end]),
                Vec::new(),
                Vec::from([leap_day, march_end]),
            ),
            (
                "an exclusion naming no candidate removes nothing",
                Vec::from([leap_eve, leap_day]),
                Vec::new(),
                Vec::from([march_end]),
                Vec::from([leap_eve, leap_day]),
            ),
            (
                "an exclusion removes an added instant as readily as a generated one",
                Vec::from([leap_eve]),
                Vec::from([new_year]),
                Vec::from([new_year]),
                Vec::from([leap_eve]),
            ),
        ];

        for (shape, rule_keys, additions, exclusions, expected) in cases {
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut sink: Vec<Diagnostic> = Vec::new();
            let input = input_over(
                leap_eve,
                &additions,
                &exclusions,
                OverrideSet::empty(),
                &mut meter,
            );
            let produced = drive(input, &rule_keys, &mut meter, &mut sink);
            assert_eq!(keys_of(&produced), expected, "{shape}");
            assert_eq!(starts_of(&produced), expected, "{shape}: nothing moved");
            assert!(sink.is_empty(), "{shape}: nothing to report");
        }
    }

    /// An instant in both lists is dropped, and the drop is not silent.
    ///
    /// The case `docs/adr/0002` files by name. `EXDATE` wins over the override, and the code is
    /// emitted only because an override was there: the same exclusion on a plain instance says
    /// nothing.
    #[test]
    fn an_exdate_beats_an_override_on_the_same_instant_and_says_so() {
        let first = utc(2026, 3, 2, 9, 0);
        let second = utc(2026, 3, 3, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            second,
            OverrideRange::ThisOnly,
            Some(utc(2026, 3, 3, 14, 0)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let exclusions = [second];
        let input = input_over(first, &[], &exclusions, overrides, &mut meter);

        let produced = drive(input, &[first, second], &mut meter, &mut sink);
        assert_eq!(keys_of(&produced), Vec::from([first]));
        assert_eq!(
            codes_of(&sink),
            Vec::from([DiagnosticCode::ExdateShadowsOverride])
        );
        assert_eq!(
            sink.first().unwrap().instant(),
            Some(second),
            "the complaint names the instant both lists named"
        );
    }

    /// A duplicated exclusion line removes one occurrence and reverts nothing.
    ///
    /// Precedence 2. The anchor's `LOCATION` is still in force on every later candidate, which is
    /// the difference between reading the exclusion as scoped to an instant and reading it as
    /// scoped to the override object — the second turns one duplicated line into the silent loss
    /// of an unbounded tail.
    #[test]
    fn a_redundant_exdate_on_an_anchor_leaves_it_in_force_for_every_later_key() {
        let anchored = utc(2026, 3, 2, 9, 0);
        let later = utc(2026, 3, 9, 9, 0);
        let latest = utc(2026, 3, 16, 9, 0);
        let room = Property::create(b"LOCATION", Vec::new(), b"Room 12").unwrap();
        let changes = [PropertyChange::Set(&room)];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            anchored,
            OverrideRange::ThisAndFuture,
            None,
            PropertyDiff::new(&changes),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let exclusions = [anchored];
        let input = input_over(anchored, &[], &exclusions, overrides, &mut meter);

        let produced = drive(input, &[anchored, later, latest], &mut meter, &mut sink);
        assert_eq!(keys_of(&produced), Vec::from([later, latest]));
        assert_eq!(
            codes_of(&sink),
            Vec::from([DiagnosticCode::ExdateShadowsOverride])
        );
        for occurrence in &produced {
            assert_eq!(
                occurrence.provenance(),
                Some(OverrideProvenance::ThisAndFuture { anchor: anchored }),
                "the excluded instant took nothing with it"
            );
            assert!(
                occurrence
                    .effective_change(&PropertyId::LOCATION)
                    .is_some_and(|change| change.name() == b"LOCATION"),
                "the anchor's diff still reaches every later key"
            );
            assert_eq!(occurrence.start(), occurrence.key(), "and moved nothing");
        }
    }

    /// Two anchors compose, and the later one is under no obligation to restate the earlier.
    ///
    /// The other case `docs/adr/0002` files by name. A March anchor changing `LOCATION` and a
    /// June anchor changing `SUMMARY` leave both in force in July; the tag names the nearest
    /// anchor while application composes both, which is why one is not the other.
    #[test]
    fn two_chained_anchors_compose_and_the_later_need_not_restate_the_earlier() {
        let march = utc(2026, 3, 2, 9, 0);
        let june = utc(2026, 6, 1, 9, 0);
        let july = utc(2026, 7, 6, 9, 0);
        let room = Property::create(b"LOCATION", Vec::new(), b"Room 12").unwrap();
        let title = Property::create(b"SUMMARY", Vec::new(), b"Retro").unwrap();
        let relocation = [PropertyChange::Set(&room)];
        let renaming = [PropertyChange::Set(&title)];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [
            entry(
                march,
                OverrideRange::ThisAndFuture,
                None,
                PropertyDiff::new(&relocation),
            ),
            entry(
                june,
                OverrideRange::ThisAndFuture,
                None,
                PropertyDiff::new(&renaming),
            ),
        ];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = input_over(march, &[], &[], overrides, &mut meter);

        let produced = drive(input, &[march, june, july], &mut meter, &mut sink);
        let last = produced.last().unwrap();
        assert_eq!(last.key(), july);
        assert_eq!(
            last.provenance(),
            Some(OverrideProvenance::ThisAndFuture { anchor: june }),
            "the tag names the nearest anchor and not the set"
        );
        assert!(
            last.effective_change(&PropertyId::LOCATION).is_some(),
            "a June edit that said nothing about LOCATION did not revert March's"
        );
        assert!(last.effective_change(&PropertyId::SUMMARY).is_some());
        assert_eq!(
            last.start(),
            last.key(),
            "and neither anchor moved anything, so July keeps its own time"
        );
        assert!(sink.is_empty());
    }

    /// A stated shift overwrites an earlier stated shift and is never added to it.
    ///
    /// Both shifts are measured from a cadence key, so accumulating them would count the first
    /// twice. The `ThisOnly` move in the middle proves the other half: an override that reaches
    /// one instance states nothing about the cadence, so the tail keeps the anchor's half hour.
    #[test]
    fn the_nearest_stated_move_wins_and_shifts_never_accumulate() {
        let first = utc(2026, 3, 2, 9, 0);
        let second = utc(2026, 3, 3, 9, 0);
        let third = utc(2026, 3, 4, 9, 0);
        let fourth = utc(2026, 3, 5, 9, 0);
        let fifth = utc(2026, 3, 6, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [
            entry(
                first,
                OverrideRange::ThisAndFuture,
                Some(utc(2026, 3, 2, 10, 0)),
                PropertyDiff::empty(),
            ),
            entry(
                third,
                OverrideRange::ThisAndFuture,
                Some(utc(2026, 3, 4, 9, 30)),
                PropertyDiff::empty(),
            ),
            entry(
                fourth,
                OverrideRange::ThisOnly,
                Some(utc(2026, 3, 5, 11, 0)),
                PropertyDiff::empty(),
            ),
        ];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = input_over(first, &[], &[], overrides, &mut meter);

        let keys = [first, second, third, fourth, fifth];
        let produced = drive(input, &keys, &mut meter, &mut sink);
        assert_eq!(
            starts_of(&produced),
            Vec::from([
                utc(2026, 3, 2, 10, 0),
                utc(2026, 3, 3, 10, 0),
                utc(2026, 3, 4, 9, 30),
                utc(2026, 3, 5, 11, 0),
                utc(2026, 3, 6, 9, 30),
            ]),
            "an hour, then half an hour — never an hour and a half"
        );
        assert_eq!(
            keys_of(&produced),
            Vec::from(keys),
            "a move changes where an occurrence happens, never what addresses it"
        );
    }

    /// An anchor that changes only a property moves nothing, on every later instance.
    ///
    /// The guard `docs/adr/0002` asks for against sliding back to a scalar delta: the diff is
    /// present and the shift is absent, which a time-delta implementation cannot express.
    #[test]
    fn a_this_and_future_anchor_that_changes_only_location_moves_no_start() {
        let anchored = utc(2026, 3, 2, 9, 0);
        let later = utc(2026, 3, 9, 9, 0);
        let room = Property::create(b"LOCATION", Vec::new(), b"Room 12").unwrap();
        let changes = [PropertyChange::Set(&room)];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            anchored,
            OverrideRange::ThisAndFuture,
            None,
            PropertyDiff::new(&changes),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = input_over(anchored, &[], &[], overrides, &mut meter);

        let produced = drive(input, &[anchored, later], &mut meter, &mut sink);
        assert_eq!(starts_of(&produced), Vec::from([anchored, later]));
        for occurrence in &produced {
            assert!(!occurrence.is_moved());
            assert_eq!(occurrence.shift_seconds(), None);
            assert!(occurrence.effective_change(&PropertyId::LOCATION).is_some());
        }
    }

    /// An anchor's shift does not reach an instant an `RDATE` stated, and its diff does.
    ///
    /// There is no cadence in a literal value for a shift to move, and rendering the extra
    /// meeting an hour from the line that names it is data loss on a value the file spelled out.
    #[test]
    fn a_shift_leaves_an_added_instant_where_the_file_put_it_and_the_diff_still_reaches_it() {
        let anchored = utc(2026, 3, 2, 9, 0);
        let later = utc(2026, 3, 9, 9, 0);
        let added = utc(2026, 3, 11, 15, 0);
        let room = Property::create(b"LOCATION", Vec::new(), b"Room 12").unwrap();
        let changes = [PropertyChange::Set(&room)];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            anchored,
            OverrideRange::ThisAndFuture,
            Some(utc(2026, 3, 2, 10, 0)),
            PropertyDiff::new(&changes),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let additions = [added];
        let input = input_over(anchored, &additions, &[], overrides, &mut meter);

        let produced = drive(input, &[anchored, later], &mut meter, &mut sink);
        assert_eq!(
            starts_of(&produced),
            Vec::from([utc(2026, 3, 2, 10, 0), utc(2026, 3, 9, 10, 0), added]),
            "the cadence moved an hour and the stated instant did not move at all"
        );
        let extra = produced.last().unwrap();
        assert!(
            extra.effective_change(&PropertyId::LOCATION).is_some(),
            "the room changed for the extra meeting too"
        );
    }

    /// The case the ADR filed without an answer, and the answer this unit records.
    ///
    /// A rule instance an anchor moved lands on the same effective start as an instant an
    /// `RDATE` named. Both are emitted. They are two addressable instances with two cadence
    /// keys: an `EXDATE` naming the addition's key removes that one and leaves the moved one, a
    /// distinction fusing them would destroy in favor of one of the two, silently.
    #[test]
    fn an_addition_colliding_with_a_moved_instance_yields_two_occurrences() {
        let anchored = utc(2026, 3, 2, 9, 0);
        let later = utc(2026, 3, 3, 9, 0);
        let collision = utc(2026, 3, 3, 10, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            anchored,
            OverrideRange::ThisAndFuture,
            Some(utc(2026, 3, 2, 10, 0)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let additions = [collision];
        let input = input_over(anchored, &additions, &[], overrides, &mut meter);

        let produced = drive(input, &[anchored, later], &mut meter, &mut sink);
        assert_eq!(
            keys_of(&produced),
            Vec::from([anchored, later, collision]),
            "three keys, emitted in cadence order"
        );
        let starts = starts_of(&produced);
        assert_eq!(starts.get(1), starts.get(2), "two of them start together");
        assert_eq!(
            produced.last().unwrap().provenance(),
            Some(OverrideProvenance::ThisAndFuture { anchor: anchored }),
            "an anchor reaches an added instant too, which is why the tag can compete"
        );
    }

    /// One tag where three facts sometimes apply, ranked the way `docs/adr/0002` states.
    #[test]
    fn provenance_ranks_the_exact_match_above_the_anchor_above_the_addition() {
        let anchored = utc(2026, 3, 2, 9, 0);
        let plain = utc(2026, 3, 3, 9, 0);
        let named = utc(2026, 3, 4, 9, 0);
        let added = utc(2026, 3, 5, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [
            entry(
                anchored,
                OverrideRange::ThisAndFuture,
                None,
                PropertyDiff::empty(),
            ),
            entry(named, OverrideRange::ThisOnly, None, PropertyDiff::empty()),
        ];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let additions = [added];
        let input = input_over(anchored, &additions, &[], overrides, &mut meter);

        let produced = drive(input, &[anchored, plain, named], &mut meter, &mut sink);
        assert_eq!(
            tags_of(&produced),
            Vec::from([
                Some(OverrideProvenance::ExactMatch),
                Some(OverrideProvenance::ThisAndFuture { anchor: anchored }),
                Some(OverrideProvenance::ExactMatch),
                Some(OverrideProvenance::ThisAndFuture { anchor: anchored }),
            ]),
            "an anchor at the key itself is an exact match first, and reaches the addition last"
        );
    }

    /// With nothing overriding anything, an addition is tagged and a coincidence is not.
    #[test]
    fn an_addition_alone_is_tagged_and_a_coincidence_is_not() {
        let first = utc(2026, 3, 2, 9, 0);
        let between = utc(2026, 3, 2, 14, 0);
        let second = utc(2026, 3, 3, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let additions = [between, second];
        let input = input_over(first, &additions, &[], OverrideSet::empty(), &mut meter);

        let produced = drive(input, &[first, second], &mut meter, &mut sink);
        assert_eq!(keys_of(&produced), Vec::from([first, between, second]));
        assert_eq!(
            tags_of(&produced),
            Vec::from([None, Some(OverrideProvenance::AddedByRdate), None]),
            "the last one was generated by the rule, so no RDATE added it"
        );
    }

    /// A shift off the timeline filters the instance rather than moving it to a nearby one.
    ///
    /// `docs/adr/0011`'s rule applied to arithmetic on a number the file chose. The occurrence
    /// the anchor itself names still exists, because its start was stated rather than computed.
    #[test]
    fn a_shift_off_the_representable_timeline_is_filtered_and_reported() {
        let anchored = at(0);
        let later = at(1);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let entries = [entry(
            anchored,
            OverrideRange::ThisAndFuture,
            Some(at(i64::MAX)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let input = input_over(anchored, &[], &[], overrides, &mut meter);

        let produced = drive(input, &[anchored, later], &mut meter, &mut sink);
        assert_eq!(keys_of(&produced), Vec::from([anchored]));
        assert_eq!(starts_of(&produced), Vec::from([at(i64::MAX)]));
        assert_eq!(
            codes_of(&sink),
            Vec::from([DiagnosticCode::OverrideShiftNotRepresentable])
        );
    }

    /// A report nobody kept still leaves a mark, which is what the meter is here for.
    ///
    /// The meter travels into this unit for exactly one reason — `report_diagnostic` charges a
    /// refusal — and a caller that loses which violations occurred must not also lose that they
    /// occurred.
    #[test]
    fn a_refused_diagnostic_is_counted_against_the_meter_the_merge_was_given() {
        let excluded = utc(2026, 3, 2, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [entry(
            excluded,
            OverrideRange::ThisOnly,
            None,
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let exclusions = [excluded];
        let input = input_over(excluded, &[], &exclusions, overrides, &mut meter);

        let mut merge = Merge::new(input);
        let mut sink = IgnoreDiagnostics;
        assert!(merge.step(Some(excluded), &mut meter, &mut sink).is_none());
        assert_eq!(meter.diagnostics_dropped(), 1);
    }

    /// The protocol answers both questions the return value cannot.
    #[test]
    fn the_caller_is_told_what_was_consumed_and_when_there_is_nothing_left() {
        let generated = utc(2026, 3, 3, 9, 0);
        let added = utc(2026, 3, 2, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let additions = [added];
        let input = input_over(added, &additions, &[], OverrideSet::empty(), &mut meter);

        let mut merge = Merge::new(input);
        let mut sink: Vec<Diagnostic> = Vec::new();
        assert!(!merge.is_drained(Some(generated)));
        assert!(
            !merge.takes_rule_key(Some(generated)),
            "the addition comes first, so the rule's key is not consumed yet"
        );
        assert!(merge.step(Some(generated), &mut meter, &mut sink).is_some());
        assert!(
            merge.takes_rule_key(Some(generated)),
            "and it is consumed once the addition is spent"
        );
        assert!(merge.step(Some(generated), &mut meter, &mut sink).is_some());
        assert!(merge.is_drained(None));
        assert!(merge.step(None, &mut meter, &mut sink).is_none());
    }

    /// The second rule is dropped, loudly, and the first is the one that survives.
    #[test]
    fn a_second_recurrence_rule_is_dropped_and_reported_rather_than_unioned() {
        let daily = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        let weekly = RecurrenceRuleBuilder::new(Freq::Weekly).build().unwrap();
        let dtstart = utc(2026, 3, 2, 9, 0);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();

        let kept = keep_first_rule(&[&daily, &weekly], dtstart, &mut meter, &mut sink);
        assert_eq!(kept.map(RecurrenceRule::freq), Some(Freq::Daily));
        assert_eq!(
            codes_of(&sink),
            Vec::from([DiagnosticCode::ExtraRecurrenceRuleIgnored])
        );

        sink.clear();
        let alone = keep_first_rule(&[&weekly], dtstart, &mut meter, &mut sink);
        assert_eq!(alone.map(RecurrenceRule::freq), Some(Freq::Weekly));
        assert!(sink.is_empty(), "one rule is not an extra rule");
        assert!(keep_first_rule(&[], dtstart, &mut meter, &mut sink).is_none());
    }
}
