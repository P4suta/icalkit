// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private RFC 5545 time-zone kernel.
//!
//! The unpublished conformance helper also compiles these files to exercise the low-level adversarial corpus.
//! The unified crate always includes VTIMEZONE support.

mod annual_dates;
mod annual_positions;
mod answer;
mod combine;
mod exclusions;
mod ident;
mod model;
mod month_days;
mod overrides;
mod reader;
mod resolve;
mod rules;
pub mod seam;
mod series;
mod weekdays;
mod year_intervals;

pub use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Duration, Instant, Limits, Meter, MonthAddOutcome,
    UtcOffset, Weekday,
};

pub use answer::{
    AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, PolicyOutcome, Reading,
    ZoneAnswer, ZoneProvenance, ZoneSource,
};
pub use combine::{CombinedZoneSource, FixedOffsetSource};
pub use exclusions::ResolvedExclusions;
pub use ident::{Tzid, TzidForm};
pub use model::{
    NthWeek, Observance, ObservanceReader, RuleDay, TransitionTable, VtimezoneSet, YearlyRule,
    ZoneAdmission, ZoneSetError,
};
pub use overrides::{OrphanScan, WallClockShift, extra_widening};
pub use reader::read_calendar_zones;
pub use seam::{
    ExclusionReading, LocalInterval, ResolutionPolicy, UntilReading, nominal, wall_clock,
};
pub use series::ZonedSeries;
