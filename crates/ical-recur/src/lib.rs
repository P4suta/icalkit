// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence: turning `RRULE`, `RDATE`, and `EXDATE` into the occurrences a caller asked
//! for.
//!
//! Specification: RFC 5545 section 3.8.5 and the recurrence rule value type in section
//! 3.3.10 <https://www.rfc-editor.org/rfc/rfc5545#section-3.3.10>.
//!
//! An `RRULE` describes a series, and that series is usually infinite: a rule with neither
//! `COUNT` nor `UNTIL` never ends, and `FREQ=SECONDLY` is legal. There is therefore no
//! function here that expands a rule into a collection. Expansion is a lazy iterator over a
//! window the caller states, and nothing outside that window is computed, so an unbounded
//! rule is not a problem — the iterator is finite because the window is (see
//! `docs/adr/0002`).
//!
//! The search is bounded a second time, independently of the window. Some rules match
//! rarely: `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` sends a naive generator walking years
//! between hits. Each search spends a budget of candidate instants, and exhausting it is a
//! reported outcome — this rule found no match within the search limit — rather than a hang
//! or a silently empty result. The budget has a finite default, so a caller processing a
//! hostile file is protected before it knows the failure mode exists.
//!
//! Exceptions and overrides are applied inside the iterator, never by the caller filtering
//! afterwards. `EXDATE` removes instances, `RDATE` adds them, and a `RECURRENCE-ID`
//! component replaces one — possibly moving it out of the window it was generated in, or
//! into one it would never have been generated in. Deduplicating an `RDATE` that coincides
//! with a rule instance, matching an `EXDATE` whose value type differs from `DTSTART`, and
//! placing a moved override are exactly the points where implementations quietly disagree,
//! and a caller asked to reconcile them will get them wrong.
//!
//! Comparing an `UNTIL` in UTC against a `DTSTART` that is floating or zoned needs a time
//! zone, and the caller supplies the answer rather than this crate obtaining it: expansion
//! takes instants that have already been resolved, which is why this crate does not depend on
//! `ical-tz` even though it cannot work without one. Nothing here bundles a time zone
//! database or reads a clock, which is also why "is this occurrence in the past" is a
//! question the caller answers by passing in the instant it means.
//!
//! The budget is not this crate's own. It is a field of the workspace's shared `Limits`,
//! charged against a `Meter` the caller owns, so a fan-out over five thousand rules is
//! bounded in aggregate and not only per rule (see `docs/adr/0010`). A candidate is charged
//! when it is generated, including one filtered for naming a date that does not exist, since
//! the work was done either way (see `docs/adr/0011`).
//!
//! The terminal state of a search is an enum this crate owns and never
//! `Result<Occurrence, BudgetExhausted>`. std's `impl IntoIterator for Result` makes
//! `search.flatten()` compile against that item type and silently discard every terminal
//! marker, and `.filter_map(Result::ok)` and `.take_while(Result::is_ok)` are
//! indistinguishable — three one-line idioms that convert budget exhaustion back into the
//! truncated-but-plausible answer the budget exists to prevent. [`SearchStep`] rejects all
//! three at compile time, and its own documentation says what that does and does not buy.
//!
//! # Status
//!
//! The engine expands. `RRULE` reads in two ways — [`parse_recur`], which drops the part it
//! cannot use and reports it, and the strict `DecodeValue` the value layer offers — and
//! [`RecurrenceInput::search`] walks periods, applies every `BYxxx` part, selects with
//! `BYSETPOS`, merges `RDATE`, `EXDATE` and `RECURRENCE-ID` overrides in one pass that
//! materializes nothing, and stops at the window, at the rule's own end, or at the budget.
//! Every one of the forty-two worked examples in RFC 5545 section 3.8.5.3 is a test in
//! `ical-conform`, with the expected column transcribed from the RFC rather than read off this
//! implementation.
//!
//! Three things are known and named rather than hidden. Emission is ordered by cadence key and
//! not by effective start, because reordering needs a buffer no `Limits` field bounds
//! (`docs/adr/0002`, amendment 3). A window admits by cadence key **or** by effective start,
//! which is two questions where a caller expects one. And the period walk, the candidate set
//! and the selection over it are public today only because the modules holding them are
//! private and `unreachable_pub` is denied — `Period` here is not `ical_core::Period`, and that
//! surface is expected to narrow.
//!
//! Not here: a time zone. `UNTIL` is compared on the timeline the caller resolved, which is
//! what makes M2 a separate crate rather than a dependency.
//!
//! # The seam with `ical-tz`
//!
//! Which timeline that is was left half-specified by this crate and is settled by its sibling.
//! For a series whose `DTSTART` is floating or UTC it is the UTC timeline and nothing more
//! needs saying. For a **zoned** series it is the series' own wall clock projected onto UTC —
//! call an instant on it *nominal* — and every instant crossing the seam in either direction is
//! nominal: `DTSTART`, `UNTIL`, each `RDATE`, each `EXDATE` and each `RECURRENCE-ID` going in,
//! and every cadence key coming out.
//!
//! That is what makes a zoned series wall-clock-stable. This crate's period walk preserves the
//! civil fields it was handed, so a daily rule anchored at a nominal 09:00 emits a nominal 09:00
//! every day of the year; `ical-tz` reads each key back into a wall clock and resolves *that*
//! against the zone, which applies the offset in force on that particular day. A series anchored
//! at a real UTC instant instead would drift by an hour the moment the zone moved, and the
//! offsets cannot be applied here because this crate has no zone and is a sibling of the crate
//! that does.
//!
//! Two obligations fall on the caller and neither is optional. A `Z`-terminated `UNTIL` on a
//! zoned series has to be projected too — instant, then the zone's offset at it, then the wall
//! clock, then back onto the nominal timeline — because it is a real UTC instant and the keys it
//! is compared against are not. And a floating `UNTIL`, which is already nominal and needs no
//! conversion at all, is a violation of RFC 5545 section 3.3.10 that carries a diagnostic.
//! [`UntilClock`] names both cases from this side; `ical_tz::seam` states the contract from the
//! other.

#![no_std]

extern crate alloc;

mod accounting;
mod byparts;
mod engine;
mod grammar;
mod input;
mod merge;
mod period;
mod rule;
mod search;
mod setpos;
mod table;

pub use crate::accounting::{Charges, admit, generation_window, max_absolute_shift};
pub use crate::byparts::{CandidateSet, expand_period};
pub use crate::engine::{RecurrenceSearch, RuleCursorState, SearchCursor};
pub use crate::grammar::{RulePartText, parse_recur, parts};
pub use crate::input::{
    AppliedDiffs, InputError, InputList, Override, OverrideRange, OverrideSet, PropertyChange,
    PropertyDiff, RecurrenceInput,
};
pub use crate::merge::{Merge, keep_first_rule};
pub use crate::period::{Period, PeriodWalk};
pub use crate::rule::{
    ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleError, RuleLimit, RulePart,
    UntilClock, ValueKind, WeekdayNum,
};
pub use crate::search::{
    BudgetExhausted, Occurrence, OverrideProvenance, SearchOutcome, SearchStep, Window,
};
pub use crate::setpos::{SelectedCandidates, select};
pub use crate::table::{Cell, PartEffect, PartsPresent, WeekdayScope, cell, effect};

/// The candidate budget a caller gets by saying nothing, in candidates generated.
///
/// A `Meter` budget rather than a `Limits` field, because `Limits::candidates_per_period`
/// bounds one period and this bounds a whole search — and every other search sharing the
/// meter, which is the aggregate bound `docs/adr/0010` exists for.
///
/// Calibrated by this milestone, which `docs/adr/0010` names as the one that owes it. The
/// number has to be a multiple of `Limits::DEFAULT.candidates_per_period()` or the two are one
/// bound wearing two names: at 65,536 — the value this constant held while nothing expanded —
/// a search that filled a single maximal period had already spent everything, so the per-period
/// ceiling could never refuse a runaway period before the shared ledger refused the whole
/// search. Four times the ceiling clears every workload a caller that stated no policy
/// plausibly means, and refuses a year of `FREQ=MINUTELY` and a week of `FREQ=SECONDLY`, which
/// are policies rather than defaults. The workload table the number was read off is in
/// `accounting`'s documentation and is asserted in its tests.
pub const DEFAULT_CANDIDATE_BUDGET: u64 = 262_144;
