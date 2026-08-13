// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private bounded recurrence kernel.
//!
//! The temporary `ical-recur` compatibility harness compiles these same files while legacy
//! conformance consumers migrate. This module is the single source of truth.

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

pub(crate) use accounting::{Charges, admit, generation_window, max_absolute_shift};
pub(crate) use byparts::{CandidateSet, expand_period};
pub(crate) use engine::{RecurrenceSearch, RuleCursorState, SearchCursor};
pub(crate) use grammar::{RulePartText, parse_recur, parts};
pub(crate) use input::{
    AppliedDiffs, InputError, InputList, Override, OverrideRange, OverrideSet, PropertyChange,
    PropertyDiff, RecurrenceInput,
};
pub(crate) use merge::{Merge, keep_first_rule};
pub(crate) use period::{Period, PeriodWalk};
pub(crate) use rule::{
    ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleError, RuleLimit, RulePart,
    UntilClock, ValueKind, WeekdayNum,
};
pub(crate) use search::{
    BudgetExhausted, Occurrence, OverrideProvenance, SearchOutcome, SearchStep, Window,
};
pub(crate) use setpos::{SelectedCandidates, select};
pub(crate) use table::{Cell, PartEffect, PartsPresent, WeekdayScope, cell, effect};

/// Default aggregate candidate budget for one engine session.
pub(crate) const DEFAULT_CANDIDATE_BUDGET: u64 = 262_144;
