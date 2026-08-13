// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private bounded recurrence kernel.
//!
//! The unpublished conformance helper also compiles these files to exercise the low-level
//! adversarial corpus. This module is the single source of truth.

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

pub use accounting::{Charges, admit, generation_window, max_absolute_shift};
pub use byparts::{CandidateSet, expand_period};
pub use engine::{RecurrenceSearch, RuleCursorState, SearchCursor};
pub use grammar::{RulePartText, parse_recur, parts};
pub use input::{
    AppliedDiffs, InputError, InputList, Override, OverrideRange, OverrideSet, PropertyChange,
    PropertyDiff, RecurrenceInput,
};
pub use merge::{Merge, keep_first_rule};
pub use period::{Period, PeriodWalk};
pub use rule::{
    ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleError, RuleLimit, RulePart,
    UntilClock, ValueKind, WeekdayNum,
};
pub use search::{
    BudgetExhausted, Occurrence, OverrideProvenance, SearchOutcome, SearchStep, Window,
};
pub use setpos::{SelectedCandidates, select};
pub use table::{Cell, PartEffect, PartsPresent, WeekdayScope, cell, effect};

/// Default aggregate candidate budget for one engine session.
pub const DEFAULT_CANDIDATE_BUDGET: u64 = 262_144;
