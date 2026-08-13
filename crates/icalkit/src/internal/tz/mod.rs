// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private RFC 5545 time-zone kernel.
//!
//! Files in this module are also compiled by the temporary `ical-tz` conformance harness.
//! The unified crate always includes VTIMEZONE support; the former package's feature remains
//! only long enough to exercise its legacy compatibility surface.

mod answer;
mod combine;
mod exclusions;
mod ident;
mod model;
mod overrides;
mod reader;
mod resolve;
mod rules;
pub(crate) mod seam;
mod series;

pub(crate) use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Duration, Instant, Limits, Meter, MonthAddOutcome,
    UtcOffset, Weekday,
};

pub(crate) use answer::{
    AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, PolicyOutcome, Reading,
    ZoneAnswer, ZoneProvenance, ZoneSource,
};
pub(crate) use combine::{CombinedZoneSource, FixedOffsetSource};
pub(crate) use exclusions::ResolvedExclusions;
pub(crate) use ident::{Tzid, TzidForm};
pub(crate) use model::{
    NthWeek, Observance, ObservanceReader, RuleDay, TransitionTable, VtimezoneSet, YearlyRule,
    ZoneAdmission, ZoneSetError,
};
pub(crate) use overrides::{OrphanScan, WallClockShift, extra_widening};
pub(crate) use reader::read_calendar_zones;
pub(crate) use seam::{
    ExclusionReading, LocalInterval, ResolutionPolicy, UntilReading, nominal, wall_clock,
};
pub(crate) use series::ZonedSeries;
