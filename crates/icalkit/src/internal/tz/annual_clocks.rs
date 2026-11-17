// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Rank date-time products before bounded scalar-pattern admission.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BinaryHeap};
use alloc::vec::Vec;
use core::num::NonZeroU32;

use crate::internal::core::{CivilDate, CivilDateTime, CivilTime};
use crate::internal::tz::annual_counts::{AnnualCandidates, CountEnd, counted_end};
use crate::internal::tz::annual_dates::YearDateMasks;
use crate::internal::tz::annual_positions::AnnualPositions;
use crate::internal::tz::model::YearlyRule;
use crate::internal::tz::week_years::WeekYearMasks;
use crate::internal::tz::year_intervals::YearInterval;

#[derive(Clone, Copy, Debug)]
pub(super) struct ClockSet {
    hours: u64,
    minutes: u64,
    seconds: u64,
}

impl ClockSet {
    pub(super) fn new(
        hours: Option<u64>,
        minutes: Option<u64>,
        seconds: Option<u64>,
        default: CivilTime,
    ) -> Self {
        Self {
            hours: hours.unwrap_or(1_u64 << default.hour()),
            minutes: minutes.unwrap_or(1_u64 << default.minute()),
            seconds: seconds.unwrap_or(1_u64 << default.second()),
        }
    }

    fn count(self) -> u64 {
        u64::from(self.hours.count_ones())
            .saturating_mul(u64::from(self.minutes.count_ones()))
            .saturating_mul(u64::from(self.seconds.count_ones()))
    }

    fn at(self, index: u64) -> Option<CivilTime> {
        if index >= self.count() {
            return None;
        }
        let seconds = u64::from(self.seconds.count_ones());
        let minutes = u64::from(self.minutes.count_ones());
        CivilTime::from_hms(
            select(
                self.hours,
                index.checked_div(seconds.checked_mul(minutes)?)?,
            )?,
            select(
                self.minutes,
                index.checked_div(seconds)?.checked_rem(minutes)?,
            )?,
            select(self.seconds, index.checked_rem(seconds)?)?,
        )
    }

    fn count_through(self, time: CivilTime) -> u64 {
        let (hours, hour) = before(self.hours, time.hour());
        let (minutes, minute) = before(self.minutes, time.minute());
        let (seconds, second) = before(self.seconds, time.second());
        let mut total = hours
            .saturating_mul(u64::from(self.minutes.count_ones()))
            .saturating_mul(u64::from(self.seconds.count_ones()));
        if hour {
            total =
                total.saturating_add(minutes.saturating_mul(u64::from(self.seconds.count_ones())));
            if minute {
                total = total.saturating_add(seconds.saturating_add(u64::from(second)));
            }
        }
        total
    }
}

fn before(mask: u64, field: u8) -> (u64, bool) {
    let bit = 1_u64 << field;
    (
        u64::from((mask & bit.saturating_sub(1)).count_ones()),
        mask & bit != 0,
    )
}

fn select(mut mask: u64, index: u64) -> Option<u8> {
    for _ in 0..index {
        mask &= mask.checked_sub(1)?;
    }
    (mask != 0)
        .then(|| u8::try_from(mask.trailing_zeros()).ok())
        .flatten()
}

pub(super) fn read_clock_field(text: &[u8], limit: u8) -> Option<u64> {
    let mut mask = 0_u64;
    for field in text.split(|octet| *octet == b',') {
        if field.is_empty() || field.len() > 2 {
            return None;
        }
        let value = field.iter().try_fold(0_u8, |value, octet| {
            octet.is_ascii_digit().then_some(())?;
            value.checked_mul(10)?.checked_add(octet.checked_sub(b'0')?)
        })?;
        if value > limit {
            return None;
        }
        mask |= 1_u64.checked_shl(u32::from(value))?;
    }
    Some(mask)
}

pub(super) trait ClockDates: AnnualCandidates<Point = CivilDate> + Clone {
    fn selected(self, positions: AnnualPositions) -> Option<Self>;
    fn prune(
        &mut self,
        start: CivilDate,
        end: CivilDate,
        interval: YearInterval,
        include_start: bool,
    ) -> Option<()>;
    fn rules(self, at: CivilTime, through: Option<CivilDate>) -> Vec<YearlyRule>;
}

impl ClockDates for YearDateMasks {
    fn selected(self, positions: AnnualPositions) -> Option<Self> {
        self.select(positions)
    }
    fn prune(
        &mut self,
        start: CivilDate,
        end: CivilDate,
        interval: YearInterval,
        include_start: bool,
    ) -> Option<()> {
        self.retain_between(start, end, interval, include_start)
    }
    fn rules(self, at: CivilTime, through: Option<CivilDate>) -> Vec<YearlyRule> {
        Box::new(self).into_rules(at, through).collect()
    }
}

impl ClockDates for WeekYearMasks {
    fn selected(self, positions: AnnualPositions) -> Option<Self> {
        self.select(positions)
    }
    fn prune(
        &mut self,
        start: CivilDate,
        end: CivilDate,
        interval: YearInterval,
        include_start: bool,
    ) -> Option<()> {
        self.retain_between(start, end, interval, include_start)
    }
    fn rules(self, at: CivilTime, through: Option<CivilDate>) -> Vec<YearlyRule> {
        Box::new(self).into_rules(at, through).collect()
    }
}

#[derive(Clone, Copy)]
pub(super) enum ClockLimit {
    Unlimited,
    Until(CivilDateTime),
    Count(NonZeroU32),
}

struct TimeCandidates<'a, D> {
    dates: &'a D,
    clocks: ClockSet,
    positions: Option<AnnualPositions>,
}

impl<D: AnnualCandidates<Point = CivilDate>> TimeCandidates<'_, D> {
    fn extent(&self, year: i32) -> Option<u64> {
        self.dates
            .year_count(year)?
            .checked_mul(self.clocks.count())
    }

    fn raw_at(&self, year: i32, position: u64) -> Option<CivilDateTime> {
        let index = position.checked_sub(1)?;
        Some(CivilDateTime::new(
            self.dates.point_at(
                year,
                index.checked_div(self.clocks.count())?.checked_add(1)?,
            )?,
            self.clocks.at(index.checked_rem(self.clocks.count())?)?,
        ))
    }

    fn raw_through(&self, year: i32, point: CivilDateTime) -> Option<u64> {
        let through = self.dates.count_through(year, point.date())?;
        let matches = self.dates.point_at(year, through) == Some(point.date());
        through
            .checked_sub(u64::from(matches))?
            .checked_mul(self.clocks.count())?
            .checked_add(if matches {
                self.clocks.count_through(point.time())
            } else {
                0
            })
    }

    fn contains(&self, point: CivilDateTime) -> Option<bool> {
        let year = self.first_year(point)?;
        let rank = self.count_through(year, point)?;
        Some(self.point_at(year, rank) == Some(point))
    }

    fn final_onset(&self, last: CivilDateTime) -> Option<CivilDateTime> {
        let leap = (last.time().second() == 59)
            .then(|| CivilTime::from_hms(last.time().hour(), last.time().minute(), 60))
            .flatten()
            .map(|time| CivilDateTime::new(last.date(), time));
        let mut selected = None;
        for candidate in [Some(last), leap].into_iter().flatten() {
            if self.contains(candidate)? {
                selected = Some(candidate);
            }
        }
        selected
    }
}

impl<D: AnnualCandidates<Point = CivilDate>> AnnualCandidates for TimeCandidates<'_, D> {
    type Point = CivilDateTime;
    fn first_year(&self, start: CivilDateTime) -> Option<i32> {
        self.dates.first_year(start.date())
    }
    fn last_year(&self) -> i32 {
        self.dates.last_year()
    }
    fn periodic_range(&self) -> (i32, i32) {
        self.dates.periodic_range()
    }
    fn year_count(&self, year: i32) -> Option<u64> {
        let extent = self.extent(year)?;
        match self.positions {
            None => Some(extent),
            Some(positions) => u64::try_from(positions.indices(extent).count()).ok(),
        }
    }
    fn count_through(&self, year: i32, point: CivilDateTime) -> Option<u64> {
        let through = self.raw_through(year, point)?;
        match self.positions {
            None => Some(through),
            Some(positions) => u64::try_from(
                positions
                    .indices(self.extent(year)?)
                    .take_while(|rank| *rank <= through)
                    .count(),
            )
            .ok(),
        }
    }
    fn point_at(&self, year: i32, position: u64) -> Option<CivilDateTime> {
        let rank = match self.positions {
            None => position,
            Some(positions) => positions
                .indices(self.extent(year)?)
                .nth(usize::try_from(position.checked_sub(1)?).ok()?)?,
        };
        self.raw_at(year, rank)
    }
}

pub(super) fn compile<D: ClockDates>(
    dates: &D,
    clocks: ClockSet,
    positions: Option<AnnualPositions>,
    start: CivilDateTime,
    interval: YearInterval,
    end: ClockLimit,
    capacity: usize,
) -> Option<Vec<YearlyRule>> {
    let candidates = TimeCandidates {
        dates,
        clocks,
        positions,
    };
    let end = match end {
        ClockLimit::Unlimited => None,
        ClockLimit::Count(count) => match counted_end(&candidates, start, interval, count)? {
            CountEnd::Within(last) => Some(last),
            CountEnd::BeyondCivilRange => None,
        },
        ClockLimit::Until(last) => {
            let last = candidates.final_onset(last)?;
            if last < start
                || !interval.contains(candidates.first_year(start)?, candidates.first_year(last)?)
            {
                return None;
            }
            Some(last)
        },
    };
    let mut kept = BinaryHeap::new();
    if let Some(positions) = positions {
        let mut groups = BTreeMap::<u64, AnnualPositions>::new();
        for position in positions.values() {
            let index = u64::from(position.unsigned_abs()).checked_sub(1)?;
            let date = i16::try_from(index.checked_div(clocks.count())?.checked_add(1)?).ok()?;
            let clock = index.checked_rem(clocks.count())?;
            let (clock, date) = if position < 0 {
                (
                    clocks.count().checked_sub(clock)?.checked_sub(1)?,
                    date.checked_neg()?,
                )
            } else {
                (clock, date)
            };
            let selection = match groups.get(&clock) {
                Some(selection) => selection.with_position(date)?,
                None => AnnualPositions::singleton(date)?,
            };
            groups.insert(clock, selection);
        }
        for (clock, positions) in groups {
            let context = Product {
                clocks,
                start,
                end,
                interval,
                capacity,
            };
            context.offer(
                dates.clone().selected(positions)?,
                clock,
                clock.checked_add(1)?,
                &mut kept,
            )?;
        }
    } else {
        let mut cuts = [
            0,
            clocks.count_through(start.time()),
            end.map_or(clocks.count(), |last| clocks.count_through(last.time())),
            clocks.count(),
        ];
        cuts.sort_unstable();
        let context = Product {
            clocks,
            start,
            end,
            interval,
            capacity,
        };
        for pair in cuts.windows(2) {
            let [first, after] = *pair else {
                return None;
            };
            if first < after {
                context.offer(dates.clone(), first, after, &mut kept)?;
            }
        }
    }
    Some(kept.into_sorted_vec())
}

struct Product {
    clocks: ClockSet,
    start: CivilDateTime,
    end: Option<CivilDateTime>,
    interval: YearInterval,
    capacity: usize,
}

impl Product {
    fn offer<D: ClockDates>(
        &self,
        mut dates: D,
        first: u64,
        after: u64,
        kept: &mut BinaryHeap<YearlyRule>,
    ) -> Option<()> {
        let at = self.clocks.at(first)?;
        let through = match self.end {
            Some(last) if at > last.time() => {
                let Some(date) = last.date().checked_add_days(-1) else {
                    return Some(());
                };
                Some(date)
            },
            Some(last) => Some(last.date()),
            None => None,
        };
        let last = through.unwrap_or(CivilDate::from_ymd(9999, 12, 31)?);
        if last < self.start.date() {
            return Some(());
        }
        dates.prune(
            self.start.date(),
            last,
            self.interval,
            at > self.start.time(),
        )?;
        let mut patterns = dates.rules(at, through);
        patterns.sort_unstable();
        'patterns: for pattern in patterns {
            for index in first..after {
                let rule = pattern
                    .with_clock(self.clocks.at(index)?)
                    .with_year_interval(self.interval);
                if kept.len() < self.capacity {
                    kept.push(rule);
                } else if let Some(mut latest) = kept.peek_mut() {
                    if rule >= *latest {
                        break 'patterns;
                    }
                    *latest = rule;
                }
            }
        }
        Some(())
    }
}
