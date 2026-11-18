// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV recurrence expansion and time-range matching under RFC 4791 sections 9.6.5 and 9.9.
//!
//! Recurrence keys preserve the series' local clock fields on a nominal timeline.
//! Each occurrence uses the zone offset in effect on its date to map its local time to UTC.
//! The search widens its window for moved occurrences, then compares their UTC bounds with the requested range.
//! A filter stops at the first overlap.
//! An expansion returns overlapping instances in recurrence order and records any interruption.

use alloc::vec::Vec;
use core::ops::ControlFlow;

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Component, ComponentKind, DateTimeValue, DecodeValue,
    Diagnostic, DiagnosticSink, Duration, Instant, Item, LimitExceeded, Meter, Period, Property,
    PropertyId, Severity, Subject, UtcOffset, ValueType, View, report_diagnostic,
};
use crate::internal::dav::TimeRange;
use crate::internal::recur::{
    InputError, Occurrence, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput,
    RecurrenceRule, RuleLimit, SearchOutcome, SearchStep, UntilClock, ValueKind, Window,
    generation_window, max_absolute_shift,
};
use crate::internal::tz::{nominal, wall_clock};

use crate::internal::query::overlap::Occupancy;
use crate::internal::query::series_index::{Member, SeriesIndex};
use crate::internal::query::vocabulary::{
    Budget, BusyPeriod, BusyType, Match, QueryError, Reduction, Undecided, Zones,
};

/// Property iterators borrow the identifiers during each scan.
mod ids {
    use crate::internal::core::PropertyId;

    pub(super) static RRULE: PropertyId = PropertyId::RRULE;
    pub(super) static RDATE: PropertyId = PropertyId::RDATE;
    pub(super) static EXDATE: PropertyId = PropertyId::EXDATE;
    pub(super) static RECURRENCE_ID: PropertyId = PropertyId::RECURRENCE_ID;

    pub(super) static COMPLETED: PropertyId = PropertyId::from_static(b"COMPLETED");
    pub(super) static CREATED: PropertyId = PropertyId::from_static(b"CREATED");
}

/// Specification passages for recurrence expansion and time-range matching.
pub const EXPANSION_SECTIONS: &[&str] = &[
    "RFC 4791 section 9.6.5, CALDAV:expand",
    "RFC 4791 section 9.9, the window a time-range states",
    "RFC 4791 section 9.9, the VEVENT table: start < DTSTART+DURATION against start <= DTSTART",
    "RFC 4791 section 9.9, every recurrence instance is tested and any one of them decides",
    "RFC 4791 section 7.8.3, the worked example an expansion is checked against",
    "RFC 5545 section 3.8.5, RRULE, RDATE and EXDATE",
    "RFC 5545 section 3.3.4, the four-digit year an open bound is closed against",
    "RFC 5545 section 3.3.5, the three forms a DATE-TIME is written in",
    "RFC 5545 section 3.3.14, the bound on a UTC offset that sizes the search window's slack",
];

/// Largest absolute UTC offset in seconds for a supported time zone.
/// RFC 5545 section 3.3.14 defines the offset format, and `UtcOffset` accepts offsets smaller than one day in either direction.
pub const ZONE_SLACK_SECONDS: i64 = 86_399;

/// A nominal day has 86,400 seconds even across a change of UTC offset.
const NOMINAL_DAY_SECONDS: i64 = 86_400;

/// An occurrence length with calendar days distinct from elapsed time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceSpan {
    seconds: i64,
    nominal_days: i64,
}

impl InstanceSpan {
    /// An instant occupies no time and can match the start of a query range.
    pub const INSTANT: Self = Self {
        seconds: 0,
        nominal_days: 0,
    };

    /// One calendar day for a date-only event with no stated end.
    pub const WHOLE_DAY: Self = Self {
        seconds: NOMINAL_DAY_SECONDS,
        nominal_days: 1,
    };

    /// Returns a span of elapsed seconds, or `None` for a negative length.
    #[must_use]
    pub const fn of_seconds(seconds: i64) -> Option<Self> {
        if seconds < 0 {
            None
        } else {
            Some(Self {
                seconds,
                nominal_days: 0,
            })
        }
    }

    /// Returns `None` for negative values or a nominal length that exceeds the integer range.
    #[must_use]
    pub fn of_duration(duration: Duration) -> Option<Self> {
        if duration.days() < 0 || duration.seconds() < 0 {
            return None;
        }
        let seconds = duration
            .days()
            .checked_mul(NOMINAL_DAY_SECONDS)?
            .checked_add(duration.seconds())?;
        Some(Self {
            seconds,
            nominal_days: duration.days(),
        })
    }

    /// Returns the nominal duration in seconds for bounding a search.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    /// Whether the period is a single instant.
    #[must_use]
    pub const fn is_instantaneous(self) -> bool {
        self.seconds == 0
    }

    fn finish<'a>(
        self,
        nominal_start: Instant,
        start: Instant,
        clock: SeriesClock<'a>,
        zones: Zones<'a>,
    ) -> Result<Instant, Undecided> {
        let day_seconds = self
            .nominal_days
            .checked_mul(NOMINAL_DAY_SECONDS)
            .ok_or(Undecided::ValueUnreadable)?;
        let after_days = if day_seconds == 0 {
            start
        } else {
            let local_end = nominal_start
                .checked_add_seconds(day_seconds)
                .ok_or(Undecided::ValueUnreadable)?;
            clock.place(local_end, zones)?
        };
        let elapsed = self
            .seconds
            .checked_sub(day_seconds)
            .ok_or(Undecided::ValueUnreadable)?;
        after_days
            .checked_add_seconds(elapsed)
            .ok_or(Undecided::ValueUnreadable)
    }
}

/// The three date-time forms in RFC 5545 section 3.3.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SeriesClock<'a> {
    /// A UTC date-time with a `Z` suffix.
    Utc,
    /// A floating date-time uses the query's time zone.
    /// A missing zone leaves placement undecided.
    Floating,
    /// A local date-time with a `TZID` parameter.
    Zoned(&'a str),
}

impl<'a> SeriesClock<'a> {
    const fn tzid(self) -> Option<&'a str> {
        match self {
            Self::Zoned(tzid) => Some(tzid),
            Self::Utc | Self::Floating => None,
        }
    }

    /// Use the offset at each occurrence to preserve local time across zone transitions.
    fn place(self, at: Instant, zones: Zones<'a>) -> Result<Instant, Undecided> {
        if matches!(self, Self::Utc) {
            return Ok(at);
        }
        let local = wall_clock(at).ok_or(Undecided::ValueUnreadable)?;
        let answer = zones.resolve(self.tzid(), local)?;
        answer
            .resolution
            .pick(zones.policy().gaps(), zones.policy().folds())
            .ok_or(Undecided::ZoneAmbiguous)
    }

    /// Apply the zone policy before charging a recurrence toward `COUNT`.
    /// An unknown zone defers the decision until placement.
    fn admits(self, key: Instant, zones: Zones<'a>) -> bool {
        !matches!(self.place(key, zones), Err(Undecided::ZoneAmbiguous))
    }
}

/// A recurrence set in the nominal time of its series.
/// The input instants encode the series' local clock fields as UTC.
#[derive(Clone, Copy, Debug)]
pub struct Series<'a> {
    /// The nominal start of the series.
    pub dtstart: Instant,
    /// The `DTSTART` value type.
    pub dtstart_kind: ValueKind,
    /// The clock for interpreting nominal recurrence keys.
    pub clock: SeriesClock<'a>,
    /// The base occurrence duration under RFC 4791 section 9.9.
    pub occupies: InstanceSpan,
    kind: ComponentKind,
    end_kind: EndKind,
    durations: InstanceDurations<'a>,
    /// The recurrence rule, if present.
    pub rule: Option<&'a RecurrenceRule>,
    /// Nominal inclusion dates in ascending order.
    pub rdates: &'a [Instant],
    /// Nominal exclusion dates in ascending order.
    pub exdates: &'a [Instant],
    /// Overrides by nominal recurrence identifier.
    pub overrides: OverrideSet<'a>,
}

#[derive(Clone, Copy, Debug)]
struct ContextSpan<'a> {
    span: InstanceSpan,
    clock: SeriesClock<'a>,
    end_kind: EndKind,
}

impl<'a> ContextSpan<'a> {
    fn finish(
        self,
        nominal_start: Instant,
        start: Instant,
        series_clock: SeriesClock<'a>,
        zones: Zones<'a>,
    ) -> Result<Instant, Undecided> {
        if self.span.nominal_days == 0 || self.clock == series_clock {
            self.span.finish(nominal_start, start, series_clock, zones)
        } else {
            let opening = nominal_instant(start, self.clock, zones)?;
            self.span.finish(opening, start, self.clock, zones)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PeriodSpan {
    key: Instant,
    span: InstanceSpan,
}

#[derive(Clone, Copy, Debug)]
struct ComponentSpan<'a> {
    key: Instant,
    range: OverrideRange,
    stated: Option<ContextSpan<'a>>,
    source_position: usize,
    /// Index of the last range override through this key that specifies a duration.
    latest_range: Option<usize>,
}

impl ComponentSpan<'_> {
    fn index(components: &mut Vec<Self>, meter: &mut Meter) -> Result<(), LimitExceeded> {
        let count = u64::try_from(components.len()).unwrap_or(u64::MAX);
        if count == 0 {
            return Ok(());
        }
        let sort_work = u64::from(count.ilog2()).saturating_add(1);
        meter.try_charge(count.saturating_mul(sort_work.saturating_add(2)))?;
        components.sort_unstable_by_key(|component| (component.key, component.source_position));
        components.dedup_by_key(|component| component.key);
        let mut latest = None;
        for (index, component) in components.iter_mut().enumerate() {
            if component.range == OverrideRange::ThisAndFuture && component.stated.is_some() {
                latest = Some(index);
            }
            component.latest_range = latest;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct InstanceDurations<'a> {
    periods: &'a [PeriodSpan],
    components: &'a [ComponentSpan<'a>],
}

impl<'a> InstanceDurations<'a> {
    const EMPTY: Self = Self {
        periods: &[],
        components: &[],
    };

    fn value_kind_at(self, key: Instant, default: ValueKind) -> ValueKind {
        if self
            .periods
            .binary_search_by_key(&key, |period| period.key)
            .is_ok()
        {
            ValueKind::DateTime
        } else {
            default
        }
    }

    fn at(
        self,
        key: Instant,
        base: InstanceSpan,
        end_kind: EndKind,
        clock: SeriesClock<'a>,
        mut charge_probe: impl FnMut() -> Result<(), LimitExceeded>,
    ) -> Result<ContextSpan<'a>, LimitExceeded> {
        let period = self
            .periods
            .binary_search_by_key(&key, |period| period.key)
            .ok()
            .and_then(|at| self.periods.get(at));
        let base = ContextSpan {
            span: period.map_or(base, |period| period.span),
            clock,
            end_kind: if period.is_some() && end_kind == EndKind::Implied {
                EndKind::Explicit
            } else {
                end_kind
            },
        };
        let mut lower = 0usize;
        let mut upper = self.components.len();
        while lower < upper {
            charge_probe()?;
            let middle = lower.midpoint(upper);
            let probe = &self.components[middle];
            match probe.key.cmp(&key) {
                core::cmp::Ordering::Equal => return Ok(probe.stated.unwrap_or(base)),
                core::cmp::Ordering::Less => lower = middle.saturating_add(1),
                core::cmp::Ordering::Greater => upper = middle,
            }
        }
        let inherited = lower
            .checked_sub(1)
            .and_then(|last| self.components[last].latest_range)
            .and_then(|anchor| self.components[anchor].stated);
        Ok(inherited.unwrap_or(base))
    }

    fn maximum(self, base: InstanceSpan, clock: SeriesClock<'a>) -> InstanceSpan {
        let periods = self.periods.iter().map(|period| period.span.seconds());
        let components = self
            .components
            .iter()
            .filter_map(|component| component.stated)
            .map(|context| {
                let extra = if context.span.nominal_days > 0 && context.clock != clock {
                    ZONE_SLACK_SECONDS.saturating_mul(2)
                } else {
                    0
                };
                context.span.seconds().saturating_add(extra)
            });
        InstanceSpan {
            seconds: periods.chain(components).fold(base.seconds(), i64::max),
            nominal_days: 0,
        }
    }

    fn uses_duration(self, base: EndKind) -> bool {
        base == EndKind::Duration
            || self
                .components
                .iter()
                .filter_map(|component| component.stated)
                .any(|context| context.end_kind == EndKind::Duration)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum EndKind {
    Implied,
    Explicit,
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum InstanceShape {
    Event {
        is_date: bool,
        end: EndKind,
    },
    Todo {
        end: EndKind,
    },
    Journal {
        dates: Option<(CivilDate, CivilDate)>,
    },
}

/// An occurrence with UTC bounds and the original component form for CalDAV projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instance {
    recurrence_id: Instant,
    start: Instant,
    end: Instant,
    shape: InstanceShape,
}

impl Instance {
    /// Constructs an event with a UTC recurrence identifier, start, and end.
    #[must_use]
    pub const fn new(recurrence_id: Instant, start: Instant, end: Instant) -> Self {
        Self {
            recurrence_id,
            start,
            end,
            shape: InstanceShape::Event {
                is_date: false,
                end: if start.unix_seconds() == end.unix_seconds() {
                    EndKind::Implied
                } else {
                    EndKind::Explicit
                },
            },
        }
    }

    /// Returns the original UTC start that identifies the occurrence, including the first instance.
    #[must_use]
    pub const fn recurrence_id(self) -> Instant {
        self.recurrence_id
    }

    /// Returns the effective start in UTC after any rescheduling.
    #[must_use]
    pub const fn start(self) -> Instant {
        self.start
    }

    /// Returns the end in UTC, equal to the start for an instantaneous occurrence.
    #[must_use]
    pub const fn end(self) -> Instant {
        self.end
    }

    #[must_use]
    pub(super) const fn shape(self) -> InstanceShape {
        self.shape
    }

    fn with_shape(
        mut self,
        kind: ComponentKind,
        value_kind: ValueKind,
        end: EndKind,
        key: Instant,
        start: Instant,
    ) -> Result<Self, Undecided> {
        self.shape = match kind {
            ComponentKind::Event => InstanceShape::Event {
                is_date: value_kind == ValueKind::Date,
                end,
            },
            ComponentKind::Todo => InstanceShape::Todo { end },
            ComponentKind::Journal => InstanceShape::Journal {
                dates: if value_kind == ValueKind::Date {
                    Some((
                        wall_clock(key).ok_or(Undecided::ValueUnreadable)?.date(),
                        wall_clock(start).ok_or(Undecided::ValueUnreadable)?.date(),
                    ))
                } else {
                    None
                },
            },
            ComponentKind::Calendar
            | ComponentKind::FreeBusy
            | ComponentKind::TimeZone
            | ComponentKind::Standard
            | ComponentKind::Daylight
            | ComponentKind::Alarm => return Err(Undecided::OverlapUndefined),
        };
        Ok(self)
    }

    /// Whether an override moved this instance away from its cadence key.
    #[must_use]
    pub fn is_moved(self) -> bool {
        self.start != self.recurrence_id
    }
}

/// Nominal windows for testing starts and generating keys that an override could move into range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchBounds {
    window: Window,
    generation: Window,
    widening: i64,
}

impl SearchBounds {
    /// Builds the search windows for a series and query range.
    /// Open bounds use the four-digit calendar years in RFC 5545 section 3.3.4.
    /// An empty window yields `None`.
    pub fn compose(series: Series<'_>, range: TimeRange) -> Result<Option<Self>, QueryError> {
        let mut span = series.durations.maximum(series.occupies, series.clock);
        if series.kind == ComponentKind::Todo && series.durations.uses_duration(series.end_kind) {
            span.seconds = span.seconds.saturating_add(1);
        }
        let Some(window) = start_window(range, span, series.clock)? else {
            return Ok(None);
        };
        let widening = max_absolute_shift(series.overrides);
        let generation = generation_window(window, series.overrides).unwrap_or(window);
        Ok(Some(Self {
            window,
            generation,
            widening,
        }))
    }

    /// Returns the inclusive lower and exclusive upper bounds for instance starts.
    #[must_use]
    pub const fn window(self) -> (Instant, Instant) {
        (self.window.start(), self.window.end())
    }

    /// Returns the generation window for recurrence keys.
    /// If widening exceeds the timeline, generation uses the start window and [`Self::widening`] still reports the shift.
    #[must_use]
    pub const fn generation(self) -> (Instant, Instant) {
        (self.generation.start(), self.generation.end())
    }

    /// Returns the largest absolute override shift in seconds.
    #[must_use]
    pub const fn widening(self) -> i64 {
        self.widening
    }
}

/// Overlapping instances of a series and the reason for any incomplete result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    instances: Vec<Instance>,
    reduction: Reduction,
    incomplete: Option<Undecided>,
    /// Last publishable pair of effective start and recurrence identifier after budget exhaustion.
    ordered_through: Option<(Instant, Instant)>,
}

impl Expansion {
    /// Returns overlapping instances in recurrence order.
    /// Sort by [`Instance::start`] for chronological order after rescheduling.
    #[must_use]
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Instances whose order a calendar cursor can preserve across retries.
    pub(crate) fn resumable_instances(&self) -> impl Iterator<Item = &Instance> {
        self.instances.iter().filter(|instance| {
            self.incomplete.is_none()
                || self
                    .ordered_through
                    .is_some_and(|bound| (instance.start(), instance.recurrence_id()) <= bound)
        })
    }

    /// Describes the changes from the stored recurrence set to its expanded representation.
    #[must_use]
    pub const fn reduction(&self) -> Reduction {
        self.reduction
    }

    /// Returns the reason for an incomplete expansion, or `None` after a complete search.
    #[must_use]
    pub const fn incomplete(&self) -> Option<Undecided> {
        self.incomplete
    }
}

/// A master accounts for its sibling overrides.
/// An override without a master uses its own component bounds.
pub(super) fn component_overlaps<S>(
    component: &Component,
    index: &SeriesIndex<'_>,
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    if carries(component, &ids::RECURRENCE_ID) {
        match index.master(component, budget.meter)? {
            Ok(Some(_)) => return Ok(Match::Unmatched),
            Ok(None) => {},
            Err(reason) => return Ok(Match::Undecided(reason)),
        }
    }
    let Some(kind) = component.kind() else {
        return Ok(Match::Undecided(Undecided::OverlapUndefined));
    };
    let related = match index.related(component, budget.meter)? {
        Ok(related) => related,
        Err(reason) => return Ok(Match::Undecided(reason)),
    };
    let recurring = has_recurrence(component) || !related.is_empty();
    if !recurring {
        let periods = if matches!(kind, ComponentKind::FreeBusy) {
            freebusy_periods(component)?
        } else {
            Vec::new()
        };
        let held = match occupancy_of(component, zones, &periods) {
            Ok(held) => held,
            Err(reason) => return Ok(Match::Undecided(reason)),
        };
        return Ok(crate::internal::query::overlap::overlaps(
            kind, &held, range,
        ));
    }
    if !matches!(
        kind,
        ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal
    ) {
        return Ok(Match::Undecided(Undecided::OverlapUndefined));
    }

    recurring_component_overlaps(component, related, range, zones, budget, sink)
}

/// RFC 4791 section 9.6.6 checks the current and original times of an override.
pub(crate) fn override_impacts(
    master: &Component,
    candidate: &Component,
    related: &[Member<'_>],
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    let current = match candidate.kind() {
        Some(kind) => match occupancy_of(candidate, zones, &[]) {
            Ok(held) => crate::internal::query::overlap::overlaps(kind, &held, range),
            Err(reason) => Match::Undecided(reason),
        },
        None => Match::Undecided(Undecided::OverlapUndefined),
    };
    if current.is_matched() {
        return Ok(current);
    }

    let anchor = match override_recurrence_id(master, candidate, zones) {
        Ok(key) => key,
        Err(reason) => return Ok(current.or(Match::Undecided(reason))),
    };
    let propagates = is_this_and_future(candidate);
    let mut prior_range = false;
    let mut shadowed = Vec::new();
    for member in related {
        if core::ptr::eq(member.component, candidate) {
            continue;
        }
        budget.meter.try_charge(1)?;
        let key = match override_recurrence_id(master, member.component, zones) {
            Ok(key) => key,
            Err(reason) => return Ok(current.or(Match::Undecided(reason))),
        };
        prior_range |= key < anchor && is_this_and_future(member.component);
        if propagates && key > anchor {
            retain(&mut shadowed, key, budget.meter)?;
        }
    }
    let direct = if prior_range {
        current
    } else {
        current.or(original_override_span(
            master, candidate, range, zones, budget,
        )?)
    };
    if direct != Match::Unmatched {
        return Ok(direct);
    }

    shadowed.sort_unstable();
    let impact = |expanded: Expansion| {
        if expanded.instances().iter().any(|instance| {
            let key = instance.recurrence_id();
            key == anchor || (propagates && key > anchor && shadowed.binary_search(&key).is_err())
        }) {
            Match::Matched
        } else {
            expanded
                .incomplete()
                .map_or(Match::Unmatched, Match::Undecided)
        }
    };
    let present = direct.or(impact(expand_related(
        master, related, range, zones, budget,
    )?));
    if present.is_matched() || (!propagates && !prior_range) {
        return Ok(present);
    }
    let original = expand_members(master, related, Some(candidate), range, zones, budget)?;
    Ok(present.or(impact(original)))
}

fn original_override_span(
    master: &Component,
    candidate: &Component,
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    let original = (|| {
        let kind = master.kind().ok_or(Undecided::OverlapUndefined)?;
        let opening = required(master.dtstart())?;
        let clock = clock_of(opening)?;
        let occupies = instance_span(master, kind, opening, clock, zones)?;
        let addressed = required(candidate.get::<DateTimeValue<'_>>(&ids::RECURRENCE_ID))?;
        let key = nominal_value(addressed, clock, zones)?;
        if master.properties_named(&ids::RDATE).any(|property| {
            property
                .declared_value_type()
                .is_some_and(|value_type| value_type.is_err())
        }) {
            return Err(Unassembled::Undecidable(Undecided::ValueUnreadable));
        }
        let periods = period_spans(master, clock, zones, budget.meter)?;
        let durations = InstanceDurations {
            periods: &periods,
            components: &[],
        };
        let span = durations
            .at(key, occupies, end_kind_of(master), clock, || {
                budget.meter.try_charge(1)
            })
            .map_err(QueryError::from)?;
        let start = clock.place(key, zones)?;
        let end = span.finish(key, start, clock, zones)?;
        Instance::new(start, start, end)
            .with_shape(
                kind,
                durations.value_kind_at(
                    key,
                    if matches!(opening, DateTimeValue::Date(_)) {
                        ValueKind::Date
                    } else {
                        ValueKind::DateTime
                    },
                ),
                span.end_kind,
                key,
                key,
            )
            .map_err(Unassembled::from)
    })();
    let original = match original {
        Ok(instance) => Match::of(overlaps_range(instance, range)),
        Err(Unassembled::Undecidable(reason)) => Match::Undecided(reason),
        Err(Unassembled::Refused(error)) => return Err(error),
    };
    Ok(original)
}

fn is_this_and_future(component: &Component) -> bool {
    component
        .properties_named(&ids::RECURRENCE_ID)
        .next()
        .is_some_and(|property| {
            property
                .parameters_named(b"RANGE")
                .any(|parameter| parameter.unquoted().eq_ignore_ascii_case(b"THISANDFUTURE"))
        })
}

/// Expand one stored recurring component together with its sibling overrides.
pub(crate) fn expand_component(
    component: &Component,
    siblings: &[Item],
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Expansion, QueryError> {
    let index = SeriesIndex::new(siblings, budget.meter)?;
    match index.related(component, budget.meter)? {
        Ok(related) => expand_related(component, related, range, zones, budget),
        Err(reason) => Ok(incomplete_expansion(reason)),
    }
}

pub(super) fn expand_related(
    component: &Component,
    related: &[Member<'_>],
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Expansion, QueryError> {
    expand_members(component, related, None, range, zones, budget)
}

fn expand_members(
    component: &Component,
    related: &[Member<'_>],
    omitted: Option<&Component>,
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Expansion, QueryError> {
    match with_related_series(
        component,
        related,
        omitted,
        zones,
        budget,
        |series, budget| {
            expand(
                series,
                range,
                zones,
                budget,
                &mut crate::internal::core::IgnoreDiagnostics,
            )
        },
    )? {
        Ok(expansion) => Ok(expansion),
        Err(reason) => Ok(incomplete_expansion(reason)),
    }
}

pub(super) fn unique_reply_key(
    value: DateTimeValue<'_>,
    zones: Zones<'_>,
) -> Result<Instant, QueryError> {
    if let DateTimeValue::Zoned { stamp, tzid } = value {
        let name = core::str::from_utf8(tzid).map_err(|_| QueryError::Unrepresentable)?;
        return match zones
            .resolve(Some(name), stamp)
            .map_err(|_| QueryError::Unrepresentable)?
            .resolution
        {
            crate::internal::tz::LocalResolution::Unique { reading } => Ok(reading.instant),
            _ => Err(QueryError::Unrepresentable),
        };
    }
    placed_value(value, zones).map_err(|_| QueryError::Unrepresentable)
}

pub(super) fn event_instance_at_key<S: DiagnosticSink + ?Sized>(
    master: &Component,
    related: &[Member<'_>],
    addressed: DateTimeValue<'_>,
    target: Instant,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Instance, QueryError> {
    with_related_series(master, related, None, zones, budget, |series, budget| {
        let kind = if matches!(addressed, DateTimeValue::Date(_)) {
            ValueKind::Date
        } else {
            ValueKind::DateTime
        };
        let floating = matches!(addressed, DateTimeValue::Date(_) | DateTimeValue::Local(_));
        if kind != series.dtstart_kind || floating != matches!(series.clock, SeriesClock::Floating)
        {
            return Err(QueryError::Unrepresentable);
        }
        let key = nominal_instant(target, series.clock, zones)
            .map_err(|_| QueryError::Unrepresentable)?;
        let start = occurrence_start_at(series, key, zones, budget, sink)?
            .map_err(|_| QueryError::Unrepresentable)?
            .ok_or(QueryError::Unrepresentable)?;
        let instance = placed(series, key, start, zones, || budget.meter.try_charge(1))?
            .map_err(|_| QueryError::Unrepresentable)?;
        (target == instance.recurrence_id())
            .then_some(instance)
            .ok_or(QueryError::Unrepresentable)
    })?
    .map_err(|_| QueryError::Unrepresentable)
}

fn with_related_series<T>(
    component: &Component,
    related: &[Member<'_>],
    omitted: Option<&Component>,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    project: impl FnOnce(Series<'_>, &mut Budget<'_>) -> Result<T, QueryError>,
) -> Result<Result<T, Undecided>, QueryError> {
    macro_rules! decided {
        ($answer:expr) => {
            match $answer {
                Ok(value) => value,
                Err(reason) => return Ok(Err(reason)),
            }
        };
    }

    let Some(kind) = component.kind() else {
        return Ok(Err(Undecided::OverlapUndefined));
    };
    if !matches!(
        kind,
        ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal
    ) {
        return Ok(Err(Undecided::OverlapUndefined));
    }

    let opening = decided!(required(component.dtstart()));
    let clock = decided!(clock_of(opening));
    let dtstart = decided!(nominal_value(opening, clock, zones));
    let dtstart_kind = if matches!(opening, DateTimeValue::Date(_)) {
        ValueKind::Date
    } else {
        ValueKind::DateTime
    };
    let occupies = decided!(instance_span(component, kind, opening, clock, zones));
    let mut rule = decided!(recurrence_rule(component));
    if let Some(parsed) = rule.as_mut() {
        decided!(project_rule_limit(parsed, clock, zones));
    }
    let periods = period_spans(component, clock, zones, budget.meter)?;
    let mut rdates = instant_list(component, &ids::RDATE, clock, zones)?;
    let mut exdates = instant_list(component, &ids::EXDATE, clock, zones)?;
    rdates.sort_unstable();
    rdates.dedup();
    exdates.sort_unstable();
    exdates.dedup();

    let mut entries = Vec::new();
    let mut durations = Vec::new();
    entries
        .try_reserve(related.len())
        .map_err(|_| QueryError::Limit(LimitExceeded::Occurrences))?;
    for member in related {
        let candidate = member.component;
        if omitted.is_some_and(|component| core::ptr::eq(component, candidate)) {
            continue;
        }
        let addressed = decided!(required(
            candidate.get::<DateTimeValue<'_>>(&ids::RECURRENCE_ID)
        ));
        let recurrence_id = decided!(nominal_value(addressed, clock, zones));
        retain(
            &mut durations,
            component_span(candidate, addressed, recurrence_id, member.position, zones)?,
            budget.meter,
        )?;
        let moved_to = match decided!(optional(candidate.dtstart())) {
            Some(value) => Some(decided!(nominal_value(value, clock, zones))),
            None => None,
        };
        entries.push(Override::new(
            recurrence_id,
            if is_this_and_future(candidate) {
                OverrideRange::ThisAndFuture
            } else {
                OverrideRange::ThisOnly
            },
            moved_to,
            PropertyDiff::empty(),
        ));
    }
    ComponentSpan::index(&mut durations, budget.meter)?;
    entries.sort_by_key(|entry| entry.recurrence_id());
    let overrides = OverrideSet::new(&entries, budget.meter).map_err(input_error)?;
    Ok(Ok(project(
        Series {
            dtstart,
            dtstart_kind,
            clock,
            occupies,
            kind,
            end_kind: end_kind_of(component),
            durations: InstanceDurations {
                periods: &periods,
                components: &durations,
            },
            rule: rule.as_ref(),
            rdates: &rdates,
            exdates: &exdates,
            overrides,
        },
        budget,
    )?))
}

const fn incomplete_expansion(reason: Undecided) -> Expansion {
    Expansion {
        instances: Vec::new(),
        reduction: Reduction {
            expanded: true,
            ..Reduction::FAITHFUL
        },
        incomplete: Some(reason),
        ordered_through: None,
    }
}

/// The UTC recurrence identifier an override addresses under its master's clock.
pub(crate) fn override_recurrence_id(
    master: &Component,
    candidate: &Component,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    let opening = required(master.dtstart())?;
    let clock = clock_of(opening)?;
    let addressed = required(candidate.get::<DateTimeValue<'_>>(&ids::RECURRENCE_ID))?;
    let nominal = nominal_value(addressed, clock, zones)?;
    clock.place(nominal, zones)
}

/// Check whether the master's recurrence set contains the detached component's identifier.
/// An instant inside an occurrence can still fall between recurrence keys.
pub(crate) fn recurrence_set_contains<S>(
    master: &Component,
    candidate: &Component,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    if budget.is_exhausted() {
        return Ok(Match::Undecided(Undecided::SearchExhausted));
    }
    let opening = required(master.dtstart()).map_err(|_| QueryError::Unrepresentable)?;
    let clock = clock_of(opening).map_err(|_| QueryError::Unrepresentable)?;
    let dtstart = nominal_value(opening, clock, zones).map_err(|_| QueryError::Unrepresentable)?;
    let dtstart_kind = if matches!(opening, DateTimeValue::Date(_)) {
        ValueKind::Date
    } else {
        ValueKind::DateTime
    };
    let mut rule = recurrence_rule(master).map_err(|_| QueryError::Unrepresentable)?;
    if let Some(parsed) = rule.as_mut()
        && let Err(reason) = project_rule_limit(parsed, clock, zones)
    {
        return Ok(Match::Undecided(reason));
    }
    let mut rdates = instant_list(master, &ids::RDATE, clock, zones)?;
    let mut exdates = instant_list(master, &ids::EXDATE, clock, zones)?;
    rdates.sort_unstable();
    rdates.dedup();
    exdates.sort_unstable();
    exdates.dedup();
    let addressed = required(candidate.get::<DateTimeValue<'_>>(&ids::RECURRENCE_ID))
        .map_err(|_| QueryError::Unrepresentable)?;
    let anchor = nominal_value(addressed, clock, zones).map_err(|_| QueryError::Unrepresentable)?;
    let series = Series {
        dtstart,
        dtstart_kind,
        clock,
        occupies: InstanceSpan::INSTANT,
        kind: master.kind().ok_or(QueryError::Unrepresentable)?,
        end_kind: EndKind::Implied,
        durations: InstanceDurations::EMPTY,
        rule: rule.as_ref(),
        rdates: &rdates,
        exdates: &exdates,
        overrides: OverrideSet::empty(),
    };
    Ok(
        match occurrence_start_at(series, anchor, zones, budget, sink)? {
            Ok(Some(_)) => Match::Matched,
            Ok(None) => Match::Unmatched,
            Err(reason) => Match::Undecided(reason),
        },
    )
}

fn occurrence_start_at<S: DiagnosticSink + ?Sized>(
    series: Series<'_>,
    anchor: Instant,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Result<Option<Instant>, Undecided>, QueryError> {
    if budget.is_exhausted() {
        return Ok(Err(Undecided::SearchExhausted));
    }
    if let Some((key, start)) = lone_instance(series)
        && key == anchor
    {
        return Ok(Ok(Some(start)));
    }
    let end = anchor
        .checked_add_seconds(1)
        .ok_or(QueryError::Unrepresentable)?;
    let window = Window::new(anchor, end).ok_or(QueryError::Unrepresentable)?;
    let input = match assemble(series, &mut *budget.meter) {
        Ok(input) => input,
        Err(Unassembled::Refused(error)) => return Err(error),
        Err(Unassembled::Undecidable(reason)) => return Ok(Err(reason)),
    };
    let admitted = move |key: Instant| series.clock.admits(key, zones);
    let gated = input.admitting(&admitted);
    let mut search = gated.search(window, &mut *budget.meter, sink);
    loop {
        match query_step(search.next(), search.outcome()) {
            ControlFlow::Break(reason) => {
                return Ok(reason.map_or(Ok(None), Err));
            },
            ControlFlow::Continue(occurrence) if occurrence.key() == anchor => {
                return Ok(Ok(Some(occurrence.start())));
            },
            ControlFlow::Continue(_) => {},
        }
    }
}

/// The first instance's UTC recurrence identifier.
pub(crate) fn component_start(
    component: &Component,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    let opening = required(component.dtstart())?;
    placed_value(opening, zones)
}

fn recurring_component_overlaps<S: DiagnosticSink + ?Sized>(
    component: &Component,
    related: &[Member<'_>],
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError> {
    Ok(
        with_related_series(component, related, None, zones, budget, |series, budget| {
            overlaps(series, range, zones, budget, sink)
        })?
        .unwrap_or_else(Match::Undecided),
    )
}

fn input_error(error: InputError) -> QueryError {
    error
        .limit()
        .map_or(QueryError::Unrepresentable, QueryError::Limit)
}

fn carries(component: &Component, id: &'static PropertyId) -> bool {
    component.properties_named(id).next().is_some()
}

pub(super) fn has_recurrence(component: &Component) -> bool {
    [&ids::RRULE, &ids::RDATE, &ids::EXDATE]
        .iter()
        .any(|id| carries(component, id))
}

fn optional<T>(view: View<'_, T>) -> Result<Option<T>, Undecided> {
    match view {
        View::Absent => Ok(None),
        View::Valid { value, .. } => Ok(Some(value)),
        View::Malformed { .. } => Err(Undecided::ValueUnreadable),
    }
}

fn required<T>(view: View<'_, T>) -> Result<T, Undecided> {
    optional(view)?.ok_or(Undecided::ValueUnreadable)
}

fn clock_of(value: DateTimeValue<'_>) -> Result<SeriesClock<'_>, Undecided> {
    match value {
        DateTimeValue::Utc(_) => Ok(SeriesClock::Utc),
        DateTimeValue::Date(_) | DateTimeValue::Local(_) => Ok(SeriesClock::Floating),
        DateTimeValue::Zoned { tzid, .. } => core::str::from_utf8(tzid)
            .map(SeriesClock::Zoned)
            .map_err(|_| Undecided::ValueUnreadable),
    }
}

/// Read a stored date or date-time onto the recurrence engine's nominal timeline.
fn nominal_value(
    value: DateTimeValue<'_>,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    let local = match value {
        DateTimeValue::Date(date) => CivilDateTime::new(date, CivilTime::MIDNIGHT),
        DateTimeValue::Local(stamp) => stamp,
        DateTimeValue::Zoned { stamp, tzid }
            if clock.tzid().is_some_and(|zone| zone.as_bytes() == tzid) =>
        {
            stamp
        },
        DateTimeValue::Zoned { .. } => {
            return nominal_instant(placed_value(value, zones)?, clock, zones);
        },
        DateTimeValue::Utc(stamp) => {
            let real = stamp
                .at_offset(UtcOffset::UTC)
                .ok_or(Undecided::ValueUnreadable)?;
            return nominal_instant(real, clock, zones);
        },
    };
    nominal(local).ok_or(Undecided::ValueUnreadable)
}

fn nominal_instant(
    at: Instant,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    if matches!(clock, SeriesClock::Utc) {
        return Ok(at);
    }
    let offset = zones.offset_at(clock.tzid(), at)?.offset;
    let local = CivilDateTime::from_instant(at, offset).ok_or(Undecided::ValueUnreadable)?;
    nominal(local).ok_or(Undecided::ValueUnreadable)
}

/// Place a stored date or date-time on the UTC timeline.
pub(super) fn placed_value(
    value: DateTimeValue<'_>,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    match value {
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(Undecided::ValueUnreadable),
        DateTimeValue::Date(date) => {
            place_local(CivilDateTime::new(date, CivilTime::MIDNIGHT), None, zones)
        },
        DateTimeValue::Local(stamp) => place_local(stamp, None, zones),
        DateTimeValue::Zoned { stamp, tzid } => {
            let named = core::str::from_utf8(tzid).map_err(|_| Undecided::ValueUnreadable)?;
            place_local(stamp, Some(named), zones)
        },
    }
}

fn place_local(
    local: CivilDateTime,
    tzid: Option<&str>,
    zones: Zones<'_>,
) -> Result<Instant, Undecided> {
    zones
        .resolve(tzid, local)?
        .resolution
        .pick(zones.policy().gaps(), zones.policy().folds())
        .ok_or(Undecided::ZoneAmbiguous)
}

fn recurrence_rule(component: &Component) -> Result<Option<RecurrenceRule>, Undecided> {
    optional(component.get::<RecurrenceRule>(&ids::RRULE))
}

/// Project a UTC `UNTIL` onto the nominal timeline of a non-UTC series.
fn project_rule_limit(
    rule: &mut RecurrenceRule,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<(), Undecided> {
    let RuleLimit::Until {
        at,
        value_kind,
        clock: until_clock,
    } = rule.limit()
    else {
        return Ok(());
    };
    if matches!(until_clock, UntilClock::Utc) && !matches!(clock, SeriesClock::Utc) {
        let answer = zones.offset_at(clock.tzid(), at)?;
        let local =
            CivilDateTime::from_instant(at, answer.offset).ok_or(Undecided::ValueUnreadable)?;
        let projected = nominal(local).ok_or(Undecided::ValueUnreadable)?;
        *rule = rule.with_limit(RuleLimit::Until {
            at: projected,
            value_kind,
            clock: until_clock,
        });
    }
    Ok(())
}

fn instant_list(
    component: &Component,
    id: &'static PropertyId,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<Vec<Instant>, QueryError> {
    let mut values = Vec::new();
    for property in component.properties_named(id) {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let value = if property.is_named(b"RDATE")
                && property.declared_value_type() == Some(Ok(ValueType::Period))
            {
                Period::decode_property_field(property, field)
                    .map_err(|_| QueryError::Unrepresentable)?
                    .start()
            } else {
                list_value(property, field).map_err(|_| QueryError::Unrepresentable)?
            };
            values
                .try_reserve(1)
                .map_err(|_| QueryError::Limit(LimitExceeded::Occurrences))?;
            values
                .push(nominal_value(value, clock, zones).map_err(|_| QueryError::Unrepresentable)?);
        }
    }
    Ok(values)
}

/// Decode one field of a list property, retaining the property's `TZID` parameter.
fn list_value<'a>(property: &'a Property, field: &'a [u8]) -> Result<DateTimeValue<'a>, Undecided> {
    DateTimeValue::decode_property_field(property, field).map_err(|_| Undecided::ValueUnreadable)
}

fn retain<T>(values: &mut Vec<T>, value: T, meter: &mut Meter) -> Result<(), QueryError> {
    let bytes = u64::try_from(core::mem::size_of::<T>())
        .map_err(|_| QueryError::Limit(LimitExceeded::Budget))?;
    meter.try_charge_bytes(bytes).map_err(QueryError::Limit)?;
    values
        .try_reserve(1)
        .map_err(|_| QueryError::Limit(LimitExceeded::Occurrences))?;
    values.push(value);
    Ok(())
}

fn period_spans(
    component: &Component,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<Vec<PeriodSpan>, QueryError> {
    let mut spans = Vec::new();
    for property in component
        .properties_named(&ids::RDATE)
        .filter(|property| property.declared_value_type() == Some(Ok(ValueType::Period)))
    {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let period = Period::decode_property_field(property, field)
                .map_err(|_| QueryError::Unrepresentable)?;
            let opening = period.start();
            let start = placed_value(opening, zones).map_err(|_| QueryError::Unrepresentable)?;
            let end = match period {
                Period::Explicit { end, .. } => placed_value(end, zones),
                Period::Starting { duration, .. } => {
                    let own_clock = clock_of(opening).map_err(|_| QueryError::Unrepresentable)?;
                    let nominal = nominal_value(opening, own_clock, zones)
                        .map_err(|_| QueryError::Unrepresentable)?;
                    duration_span(duration)
                        .and_then(|span| span.finish(nominal, start, own_clock, zones))
                },
            }
            .map_err(|_| QueryError::Unrepresentable)?;
            if end <= start {
                return Err(QueryError::Unrepresentable);
            }
            let length = start
                .checked_seconds_until(end)
                .ok_or(QueryError::Unrepresentable)?;
            let span = InstanceSpan::of_seconds(length).ok_or(QueryError::Unrepresentable)?;
            let key =
                nominal_value(opening, clock, zones).map_err(|_| QueryError::Unrepresentable)?;
            retain(&mut spans, PeriodSpan { key, span }, meter)?;
        }
    }
    spans.sort_by_key(|period| period.key);
    if spans
        .windows(2)
        .any(|pair| pair[0].key == pair[1].key && pair[0].span != pair[1].span)
    {
        return Err(QueryError::Unrepresentable);
    }
    spans.dedup_by_key(|period| period.key);
    Ok(spans)
}

fn component_span<'a>(
    component: &'a Component,
    addressed: DateTimeValue<'a>,
    key: Instant,
    source_position: usize,
    zones: Zones<'a>,
) -> Result<ComponentSpan<'a>, QueryError> {
    let states_length = [b"DTEND".as_slice(), b"DUE", b"DURATION"]
        .iter()
        .any(|name| {
            component
                .properties()
                .any(|property| property.is_named(name))
        });
    let stated = if states_length {
        let opening = optional(component.dtstart())
            .map_err(|_| QueryError::Unrepresentable)?
            .unwrap_or(addressed);
        let clock = clock_of(opening).map_err(|_| QueryError::Unrepresentable)?;
        let kind = component.kind().ok_or(QueryError::Unrepresentable)?;
        Some(ContextSpan {
            span: instance_span(component, kind, opening, clock, zones)
                .map_err(|_| QueryError::Unrepresentable)?,
            clock,
            end_kind: end_kind_of(component),
        })
    } else {
        None
    };
    Ok(ComponentSpan {
        key,
        source_position,
        latest_range: None,
        range: if is_this_and_future(component) {
            OverrideRange::ThisAndFuture
        } else {
            OverrideRange::ThisOnly
        },
        stated,
    })
}

fn end_kind_of(component: &Component) -> EndKind {
    if component
        .properties()
        .any(|property| property.is_named(b"DURATION"))
    {
        EndKind::Duration
    } else if component
        .properties()
        .any(|property| property.is_named(b"DTEND") || property.is_named(b"DUE"))
    {
        EndKind::Explicit
    } else {
        EndKind::Implied
    }
}

/// Date-only endpoints span calendar days.
/// Date-time endpoints span elapsed seconds after zone resolution.
fn instance_span(
    component: &Component,
    kind: ComponentKind,
    opening: DateTimeValue<'_>,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<InstanceSpan, Undecided> {
    let stated_end = match kind {
        ComponentKind::Event => optional(component.dtend())?,
        ComponentKind::Todo => optional(component.due())?,
        ComponentKind::Journal => None,
        _ => return Err(Undecided::OverlapUndefined),
    };
    if let Some(end) = stated_end {
        if matches!(
            (opening, end),
            (DateTimeValue::Date(_), DateTimeValue::Date(_))
        ) {
            let start = nominal_value(opening, clock, zones)?;
            let end = nominal_value(end, clock, zones)?;
            let seconds = start
                .checked_seconds_until(end)
                .ok_or(Undecided::ValueUnreadable)?;
            return InstanceSpan::of_duration(Duration::new(
                seconds.div_euclid(NOMINAL_DAY_SECONDS),
                0,
            ))
            .ok_or(Undecided::ValueUnreadable);
        }
        let start = placed_value(opening, zones)?;
        let end = placed_value(end, zones)?;
        let seconds = start
            .checked_seconds_until(end)
            .ok_or(Undecided::ValueUnreadable)?;
        return InstanceSpan::of_seconds(seconds).ok_or(Undecided::ValueUnreadable);
    }
    if !matches!(kind, ComponentKind::Journal)
        && let Some(duration) = optional(component.duration())?
    {
        return duration_span(duration);
    }
    Ok(if matches!(opening, DateTimeValue::Date(_)) {
        InstanceSpan::WHOLE_DAY
    } else {
        InstanceSpan::INSTANT
    })
}

fn duration_span(duration: Duration) -> Result<InstanceSpan, Undecided> {
    InstanceSpan::of_duration(duration).ok_or(Undecided::ValueUnreadable)
}

/// Read the component fields for the occupancy table in RFC 4791 section 9.9.
pub(super) fn occupancy_of<'a>(
    component: &Component,
    zones: Zones<'_>,
    periods: &'a [BusyPeriod],
) -> Result<Occupancy<'a>, Undecided> {
    let opening = optional(component.dtstart())?;
    let start = opening
        .map(|value| placed_value(value, zones))
        .transpose()?;
    let end = optional(component.dtend())?
        .map(|value| placed_value(value, zones))
        .transpose()?;
    let due = optional(component.due())?
        .map(|value| placed_value(value, zones))
        .transpose()?;
    let completed = optional(component.get::<DateTimeValue<'_>>(&ids::COMPLETED))?
        .map(|value| placed_value(value, zones))
        .transpose()?;
    let created = optional(component.get::<DateTimeValue<'_>>(&ids::CREATED))?
        .map(|value| placed_value(value, zones))
        .transpose()?;
    let duration = optional(component.duration())?;
    let duration_end = match (opening, duration) {
        (Some(value), Some(span)) => {
            let clock = clock_of(value)?;
            let nominal_start = nominal_value(value, clock, zones)?;
            Some(duration_span(span)?.finish(
                nominal_start,
                start.ok_or(Undecided::ValueUnreadable)?,
                clock,
                zones,
            )?)
        },
        (None, _) | (_, None) => None,
    };
    let one_day_end = match opening {
        Some(value) if matches!(value, DateTimeValue::Date(_)) => {
            let clock = clock_of(value)?;
            let nominal_start = nominal_value(value, clock, zones)?;
            let nominal_end = nominal_start
                .checked_add_seconds(NOMINAL_DAY_SECONDS)
                .ok_or(Undecided::ValueUnreadable)?;
            Some(clock.place(nominal_end, zones)?)
        },
        Some(_) | None => None,
    };
    Ok(Occupancy {
        start,
        start_is_date_time: opening.map(|value| !matches!(value, DateTimeValue::Date(_))),
        end,
        due,
        completed,
        created,
        has_duration: duration.is_some(),
        duration_end,
        one_day_end,
        periods,
        triggers: None,
    })
}

fn freebusy_periods(component: &Component) -> Result<Vec<BusyPeriod>, QueryError> {
    let mut periods = Vec::new();
    for property in component.freebusy() {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let written = Period::decode_property_field(property, field)
                .map_err(|_| QueryError::Unrepresentable)?;
            let start = utc_value(written.start())?;
            let end = match written {
                Period::Explicit { end, .. } => utc_value(end)?,
                Period::Starting { duration, .. } => {
                    let span = duration_span(duration).map_err(|_| QueryError::Unrepresentable)?;
                    start
                        .checked_add_seconds(span.seconds())
                        .ok_or(QueryError::Unrepresentable)?
                },
            };
            periods.push(BusyPeriod::new(start, end, BusyType::Busy));
        }
    }
    Ok(periods)
}

/// Free/busy periods require UTC endpoints under RFC 5545 section 3.8.2.6.
fn utc_value(value: DateTimeValue<'_>) -> Result<Instant, QueryError> {
    match value {
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(QueryError::Unrepresentable),
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Zoned { .. } => {
            Err(QueryError::Unrepresentable)
        },
    }
}

/// Tests whether any occurrence overlaps the range under RFC 4791 section 9.9.
/// Budget exhaustion before a match yields [`Undecided::SearchExhausted`].
pub fn overlaps<'a, S>(
    series: Series<'a>,
    range: TimeRange,
    zones: Zones<'a>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    if budget.is_exhausted() {
        return Ok(exhausted_before_starting(series, budget, sink));
    }
    let Some(bounds) = SearchBounds::compose(series, range)? else {
        return Ok(Match::Unmatched);
    };
    let mut answer = lone_instance(series).map_or(Match::Unmatched, |(key, start)| {
        let found = placed(series, key, start, zones, || budget.meter.try_charge(1))
            .unwrap_or(Err(Undecided::SearchExhausted));
        decide(found, range)
    });
    if !answer.is_matched() {
        let incomplete = walk(series, bounds, zones, budget, &mut *sink, |found| {
            answer = answer.or(decide(found, range));
            !answer.is_matched()
        })?;
        if let Some(reason) = incomplete {
            answer = answer.or(Match::Undecided(reason));
        }
    }
    if let Some(reason) = answer.undecided() {
        note_undecided(reason, series.dtstart, &mut *budget.meter, sink);
    }
    Ok(answer)
}

/// Expands overlapping occurrences under RFC 4791 section 9.6.5.
/// The result marks the projected calendar with [`Reduction::expanded`].
pub fn expand<'a, S>(
    series: Series<'a>,
    range: TimeRange,
    zones: Zones<'a>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Expansion, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    let mut collected = Collector::new();
    let mut ordered_through = None;
    if budget.is_exhausted() {
        collected.undecided = Some(Undecided::SearchExhausted);
    } else if let Some(bounds) = SearchBounds::compose(series, range)? {
        if let Some((key, start)) = lone_instance(series) {
            let found = placed(series, key, start, zones, || budget.meter.try_charge(1))
                .unwrap_or(Err(Undecided::SearchExhausted));
            if let Ok(established) = found {
                collected.offer(found, range);
                collected.lone = Some(established.recurrence_id());
            } else {
                collected.offer(found, range);
            }
        }
        let mut last_walked = None;
        let incomplete = walk(series, bounds, zones, budget, &mut *sink, |found| {
            if let Ok(instance) = found {
                last_walked = Some(instance.recurrence_id());
            }
            collected.offer(found, range)
        })?;
        if series.clock == SeriesClock::Utc
            && collected.undecided.is_none()
            && incomplete == Some(Undecided::SearchExhausted)
        {
            ordered_through = last_walked.and_then(|key| {
                let earliest = key.checked_add_seconds(bounds.widening().checked_neg()?)?;
                Some((earliest, key))
            });
        }
        collected.undecided = collected.undecided.or(incomplete);
        if let Some(refusal) = collected.refused {
            return Err(refusal);
        }
    }
    if let Some(reason) = collected.undecided {
        note_undecided(reason, series.dtstart, &mut *budget.meter, sink);
    }
    Ok(Expansion {
        instances: collected.instances,
        reduction: Reduction {
            expanded: true,
            ..Reduction::FAITHFUL
        },
        incomplete: collected.undecided,
        ordered_through,
    })
}

#[derive(Debug)]
struct Collector {
    instances: Vec<Instance>,
    /// Recurrence identifier of the initial occurrence collected outside the search.
    lone: Option<Instant>,
    undecided: Option<Undecided>,
    refused: Option<QueryError>,
}

impl Collector {
    const fn new() -> Self {
        Self {
            instances: Vec::new(),
            lone: None,
            undecided: None,
            refused: None,
        }
    }

    /// An occurrence with an unresolved time leaves the result incomplete while later occurrences remain eligible.
    fn offer(&mut self, found: Result<Instance, Undecided>, range: TimeRange) -> bool {
        match found {
            Err(reason) => {
                self.undecided = self.undecided.or(Some(reason));
                true
            },
            Ok(instance) => self.keep(instance, range),
        }
    }

    fn keep(&mut self, instance: Instance, range: TimeRange) -> bool {
        if !overlaps_range(instance, range) {
            return true;
        }
        if self.lone == Some(instance.recurrence_id()) {
            // An `RDATE` equal to `DTSTART` repeats the initial occurrence already in the result.
            return true;
        }
        if self.instances.try_reserve(1).is_err() {
            self.refused = Some(QueryError::Limit(LimitExceeded::Occurrences));
            return false;
        }
        self.instances.push(instance);
        true
    }
}

/// Policy limits produce a query error.
/// Invalid recurrence lists leave the match undecided.
#[derive(Debug)]
enum Unassembled {
    Refused(QueryError),
    Undecidable(Undecided),
}

impl From<Undecided> for Unassembled {
    fn from(reason: Undecided) -> Self {
        Self::Undecidable(reason)
    }
}

impl From<QueryError> for Unassembled {
    fn from(error: QueryError) -> Self {
        match error {
            QueryError::Unrepresentable => Self::Undecidable(Undecided::ValueUnreadable),
            error => Self::Refused(error),
        }
    }
}

impl From<InputError> for Unassembled {
    fn from(error: InputError) -> Self {
        Self::from(input_error(error))
    }
}

/// At second precision, an occurrence of positive length can overlap from `range.start - span + 1`.
/// The caller adds one second for a component whose duration includes its end.
fn start_window(
    range: TimeRange,
    span: InstanceSpan,
    clock: SeriesClock<'_>,
) -> Result<Option<Window>, QueryError> {
    let first = calendar_start().ok_or(QueryError::Unrepresentable)?;
    let last = calendar_end().ok_or(QueryError::Unrepresentable)?;
    let lower = match range.start() {
        None => first,
        Some(from) if span.is_instantaneous() => from,
        Some(from) => earlier(from, span.seconds().saturating_sub(1), first),
    };
    let upper = range.end().unwrap_or(last);
    let slack = match clock {
        SeriesClock::Utc => 0,
        SeriesClock::Floating | SeriesClock::Zoned(_) => ZONE_SLACK_SECONDS,
    };
    Ok(Window::new(
        earlier(lower, slack, first),
        later(upper, slack, last),
    ))
}

fn overlaps_range(instance: Instance, range: TimeRange) -> bool {
    let (kind, is_date, end_kind) = match instance.shape {
        InstanceShape::Event { is_date, end } => (ComponentKind::Event, is_date, end),
        InstanceShape::Todo { end } => (ComponentKind::Todo, false, end),
        InstanceShape::Journal { dates } => {
            (ComponentKind::Journal, dates.is_some(), EndKind::Implied)
        },
    };
    let occupancy = Occupancy {
        start: Some(instance.start()),
        start_is_date_time: Some(!is_date),
        end: (kind == ComponentKind::Event && end_kind == EndKind::Explicit)
            .then_some(instance.end()),
        due: (kind == ComponentKind::Todo && end_kind == EndKind::Explicit)
            .then_some(instance.end()),
        has_duration: end_kind == EndKind::Duration,
        duration_end: (end_kind == EndKind::Duration).then_some(instance.end()),
        one_day_end: is_date.then_some(instance.end()),
        ..Occupancy::default()
    };
    crate::internal::query::overlap::overlaps(kind, &occupancy, range).is_matched()
}

fn decide(found: Result<Instance, Undecided>, range: TimeRange) -> Match {
    match found {
        Ok(instance) => Match::of(overlaps_range(instance, range)),
        Err(reason) => Match::Undecided(reason),
    }
}

/// Resolve the original recurrence key and effective start independently when an override moves an occurrence.
fn placed<'a>(
    series: Series<'a>,
    key: Instant,
    start: Instant,
    zones: Zones<'a>,
    charge_probe: impl FnMut() -> Result<(), LimitExceeded>,
) -> Result<Result<Instance, Undecided>, LimitExceeded> {
    let clock = series.clock;
    let value_kind = series.durations.value_kind_at(key, series.dtstart_kind);
    let span = if series.kind == ComponentKind::Journal {
        ContextSpan {
            span: if value_kind == ValueKind::Date {
                InstanceSpan::WHOLE_DAY
            } else {
                InstanceSpan::INSTANT
            },
            clock,
            end_kind: EndKind::Implied,
        }
    } else {
        series
            .durations
            .at(key, series.occupies, series.end_kind, clock, charge_probe)?
    };
    let instance = clock.place(start, zones).and_then(|happens_at| {
        let addressed = if key == start {
            happens_at
        } else {
            clock.place(key, zones)?
        };
        let finishes_at = span.finish(start, happens_at, clock, zones)?;
        Instance::new(addressed, happens_at, finishes_at).with_shape(
            series.kind,
            value_kind,
            span.end_kind,
            key,
            start,
        )
    });
    Ok(instance)
}

/// RFC 5545 section 3.8.5.3 includes `DTSTART` in the recurrence set unless an exclusion removes it.
/// Without a rule, the search emits inclusion dates and the caller supplies the initial occurrence.
fn lone_instance(series: Series<'_>) -> Option<(Instant, Instant)> {
    if series.rule.is_some() || series.exdates.binary_search(&series.dtstart).is_ok() {
        return None;
    }
    let moved = series
        .overrides
        .exact_match(series.dtstart)
        .and_then(|entry| entry.moved_to());
    Some((series.dtstart, moved.unwrap_or(series.dtstart)))
}

/// Pass the start window to the search, which widens generation for overrides.
/// Returning `false` from the visitor stops the walk once the caller has its answer.
fn walk<'a, S, V>(
    series: Series<'a>,
    bounds: SearchBounds,
    zones: Zones<'a>,
    budget: &mut Budget<'_>,
    sink: &mut S,
    mut visit: V,
) -> Result<Option<Undecided>, QueryError>
where
    S: DiagnosticSink + ?Sized,
    V: FnMut(Result<Instance, Undecided>) -> bool,
{
    if budget.is_exhausted() {
        return Ok(Some(Undecided::SearchExhausted));
    }
    let input = match assemble(series, &mut *budget.meter) {
        Ok(assembled) => assembled,
        Err(Unassembled::Refused(refusal)) => return Err(refusal),
        Err(Unassembled::Undecidable(reason)) => return Ok(Some(reason)),
    };
    let clock = series.clock;
    let admitted = move |key: Instant| clock.admits(key, zones);
    let gated = input.admitting(&admitted);
    let mut search = gated.search(bounds.window, &mut *budget.meter, sink);
    loop {
        match query_step(search.next(), search.outcome()) {
            ControlFlow::Break(reason) => return Ok(reason),
            ControlFlow::Continue(occurrence) => {
                let Ok(instance) =
                    placed(series, occurrence.key(), occurrence.start(), zones, || {
                        search.charge_work(1)
                    })
                else {
                    return Ok(Some(Undecided::SearchExhausted));
                };
                if !visit(instance) {
                    return Ok(None);
                }
            },
        }
    }
}

fn query_step(
    step: Option<SearchStep<'_>>,
    outcome: SearchOutcome,
) -> ControlFlow<Option<Undecided>, Occurrence<'_>> {
    match step.and_then(SearchStep::occurrence) {
        Some(occurrence) => ControlFlow::Continue(occurrence),
        None => ControlFlow::Break(Undecided::of_search(outcome)),
    }
}

fn assemble<'a>(series: Series<'a>, meter: &mut Meter) -> Result<RecurrenceInput<'a>, Unassembled> {
    RecurrenceInput::new(
        series.dtstart,
        series.dtstart_kind,
        series.rule,
        series.rdates,
        series.exdates,
        series.overrides,
        meter,
    )
    .map_err(Unassembled::from)
}

fn exhausted_before_starting<S>(series: Series<'_>, budget: &mut Budget<'_>, sink: &mut S) -> Match
where
    S: DiagnosticSink + ?Sized,
{
    note_undecided(
        Undecided::SearchExhausted,
        series.dtstart,
        &mut *budget.meter,
        sink,
    );
    Match::Undecided(Undecided::SearchExhausted)
}

/// Use nominal `DTSTART` to identify the series even when the zone can't resolve an occurrence.
fn note_undecided<S>(reason: Undecided, at: Instant, meter: &mut Meter, sink: &mut S)
where
    S: DiagnosticSink + ?Sized,
{
    report_diagnostic(
        sink,
        meter,
        Diagnostic::at_instant(Undecided::CODE, Severity::Note, at)
            .about(Subject::new(reason.reason().as_bytes())),
    );
}

fn earlier(at: Instant, seconds: i64, floor: Instant) -> Instant {
    seconds
        .checked_neg()
        .and_then(|back| at.checked_add_seconds(back))
        .unwrap_or(floor)
        .max(floor)
}

fn later(at: Instant, seconds: i64, ceiling: Instant) -> Instant {
    at.checked_add_seconds(seconds)
        .unwrap_or(ceiling)
        .min(ceiling)
}

/// The lower bound for the four-digit years in RFC 5545 section 3.3.4.
fn calendar_start() -> Option<Instant> {
    nominal(CivilDateTime::new(
        CivilDate::from_ymd(0, 1, 1)?,
        CivilTime::MIDNIGHT,
    ))
}

/// The exclusive upper bound includes the last second of year 9999.
fn calendar_end() -> Option<Instant> {
    nominal(CivilDateTime::new(
        CivilDate::from_ymd(9999, 12, 31)?,
        CivilTime::from_hms(23, 59, 59)?,
    ))?
    .checked_add_seconds(1)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::NonZeroU32;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics,
        Instant, Limits, Meter, Severity, Subject, UtcOffset,
    };
    use crate::internal::dav::TimeRange;
    use crate::internal::recur::{
        Freq, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceRule,
        RecurrenceRuleBuilder, RuleLimit, ValueKind,
    };
    use crate::internal::tz::FixedOffsetSource;

    use super::{
        EXPANSION_SECTIONS, Instance, InstanceSpan, SearchBounds, Series, SeriesClock,
        calendar_end, calendar_start, expand, overlaps, overlaps_range,
    };
    use crate::internal::query::vocabulary::{Budget, Match, Undecided, Zones};

    const EASTERN: &str = "US/Eastern";

    const EST_SECONDS: i32 = -18_000;

    const ONE_HOUR: i64 = 3_600;

    // RFC 4791 section 7.8.3 supplies the query range and the January 3 and 4 results.
    // January 8 supplies a separate case for an override outside the range.

    const NOMINAL_JAN2: i64 = 1_136_203_200;
    const NOMINAL_JAN3: i64 = 1_136_289_600;
    const NOMINAL_JAN4: i64 = 1_136_376_000;
    const NOMINAL_JAN4_MOVED: i64 = 1_136_383_200;
    const NOMINAL_JAN8: i64 = 1_136_721_600;
    const UTC_RANGE_START: i64 = 1_136_246_400;
    const UTC_RANGE_END: i64 = 1_136_419_200;
    const UTC_JAN2_1700: i64 = 1_136_221_200;
    const UTC_JAN2_1800: i64 = 1_136_224_800;
    const UTC_JAN3_1700: i64 = 1_136_307_600;
    const UTC_JAN3_1800: i64 = 1_136_311_200;
    const UTC_JAN4_1700: i64 = 1_136_394_000;
    const UTC_JAN4_1900: i64 = 1_136_401_200;
    const UTC_JAN4_2000: i64 = 1_136_404_800;

    // The reference window accounts for the event length and the largest supported UTC offset.
    // The generation window covers the two-hour shift of the January 4 occurrence.

    /// `1_136_246_400 - 3_599 - 86_399`.
    const WINDOW_LOWER: i64 = 1_136_156_402;
    /// `1_136_419_200 + 86_399`.
    const WINDOW_UPPER: i64 = 1_136_505_599;
    /// `WINDOW_LOWER - 7_200`.
    const GENERATION_LOWER: i64 = 1_136_149_202;
    /// `WINDOW_UPPER + 7_200`.
    const GENERATION_UPPER: i64 = 1_136_512_799;
    const TWO_HOURS: i64 = 7_200;

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// All January dates in the RFC example use the same `-05:00` offset.
    fn eastern() -> FixedOffsetSource {
        FixedOffsetSource::new(
            EASTERN,
            UtcOffset::from_seconds(EST_SECONDS).unwrap(),
            false,
        )
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, minute, 0).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    fn daily(count: u32) -> RecurrenceRule {
        RecurrenceRuleBuilder::new(Freq::Daily)
            .limit(RuleLimit::Count(NonZeroU32::new(count).unwrap()))
            .build()
            .unwrap()
    }

    fn moved_jan4() -> Override<'static> {
        Override::new(
            at(NOMINAL_JAN4),
            OverrideRange::ThisOnly,
            Some(at(NOMINAL_JAN4_MOVED)),
            PropertyDiff::empty(),
        )
    }

    fn event<'a>(rule: Option<&'a RecurrenceRule>, overrides: OverrideSet<'a>) -> Series<'a> {
        Series {
            dtstart: at(NOMINAL_JAN2),
            dtstart_kind: ValueKind::DateTime,
            clock: SeriesClock::Zoned(EASTERN),
            occupies: InstanceSpan::of_seconds(ONE_HOUR).unwrap(),
            kind: crate::internal::core::ComponentKind::Event,
            end_kind: super::EndKind::Explicit,
            durations: super::InstanceDurations::EMPTY,
            rule,
            rdates: &[],
            exdates: &[],
            overrides,
        }
    }

    fn spanning(from: Instant, until: Instant) -> TimeRange {
        TimeRange::new(Some(from), Some(until)).unwrap()
    }

    #[test]
    fn duration_lookups_charge_logarithmic_work_and_stop_on_refusal() {
        let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let rule = daily(32);
        let start = stamp(2024, 1, 1, 0, 0);
        let range = spanning(start, stamp(2024, 2, 3, 0, 0));
        let future = stamp(2025, 1, 1, 0, 0).unix_seconds();
        let components: Vec<_> = (0..4096)
            .map(|offset| super::ComponentSpan {
                key: at(future + offset),
                source_position: usize::try_from(offset).unwrap(),
                latest_range: None,
                range: OverrideRange::ThisOnly,
                stated: None,
            })
            .collect();
        let measure = |rows: &[super::ComponentSpan<'_>], units| {
            let mut meter = Meter::with_budget(Limits::DEFAULT, units);
            let mut series = event(Some(&rule), OverrideSet::empty());
            series.dtstart = start;
            series.clock = SeriesClock::Utc;
            series.durations.components = rows;
            let result = expand(
                series,
                range,
                Zones::new(&utc),
                &mut spend(&mut meter),
                &mut Vec::new(),
            )
            .unwrap();
            (result, meter.spent())
        };
        let (ordinary, baseline) = measure(&[], 1_000_000);
        let (indexed, spent) = measure(&components, 1_000_000);
        assert_eq!(ordinary.instances().len(), 32);
        assert_eq!(indexed, ordinary);
        let comparisons = spent.checked_sub(baseline).unwrap();
        assert!(
            (32..=32 * 14).contains(&comparisons),
            "{comparisons} duration comparisons"
        );
        let mut first = event(Some(&rule), OverrideSet::empty());
        first.dtstart = start;
        first.clock = SeriesClock::Utc;
        let mut meter = Meter::new(Limits::DEFAULT);
        assert_eq!(
            overlaps(
                first,
                range,
                Zones::new(&utc),
                &mut spend(&mut meter),
                &mut Vec::new()
            )
            .unwrap(),
            Match::Matched
        );
        let first_probe = meter.spent().checked_add(1).unwrap();
        let (incomplete, refused_at) = measure(&components, first_probe);
        assert_eq!(refused_at, first_probe.checked_add(1).unwrap());
        assert_eq!(incomplete.incomplete(), Some(Undecided::SearchExhausted));
        assert_eq!(incomplete.instances(), []);
    }

    #[test]
    fn duration_index_admits_sorting_and_keeps_the_first_duplicate() {
        let row = |key, source_position| super::ComponentSpan {
            key: at(key),
            range: OverrideRange::ThisOnly,
            stated: None,
            source_position,
            latest_range: None,
        };
        let mut rows = alloc::vec![row(2, 1), row(1, 2), row(1, 0)];
        let mut refused = Meter::with_budget(Limits::DEFAULT, 0);
        assert_eq!(
            super::ComponentSpan::index(&mut rows, &mut refused),
            Err(crate::internal::core::LimitExceeded::Budget)
        );
        assert!(refused.is_exhausted());
        assert_eq!(rows[0].key, at(2));
        let mut meter = Meter::new(Limits::DEFAULT);
        super::ComponentSpan::index(&mut rows, &mut meter).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source_position, 0);
        assert_eq!(rows[1].key, at(2));
    }

    fn instance(recurrence_id: i64, start: i64, end: i64) -> Instance {
        Instance::new(at(recurrence_id), at(start), at(end))
    }

    fn triples(instances: &[Instance]) -> Vec<(i64, i64, i64)> {
        instances
            .iter()
            .map(|found| {
                (
                    found.recurrence_id().unix_seconds(),
                    found.start().unix_seconds(),
                    found.end().unix_seconds(),
                )
            })
            .collect()
    }

    fn spend(meter: &mut Meter) -> Budget<'_> {
        Budget::new(Limits::DEFAULT, meter)
    }

    #[test]
    fn the_worked_examples_instants_are_the_ones_the_rfc_writes() {
        assert_eq!(at(NOMINAL_JAN2), stamp(2006, 1, 2, 12, 0));
        assert_eq!(at(NOMINAL_JAN3), stamp(2006, 1, 3, 12, 0));
        assert_eq!(at(NOMINAL_JAN4), stamp(2006, 1, 4, 12, 0));
        assert_eq!(at(NOMINAL_JAN4_MOVED), stamp(2006, 1, 4, 14, 0));
        assert_eq!(at(NOMINAL_JAN8), stamp(2006, 1, 8, 12, 0));
        assert_eq!(at(UTC_RANGE_START), stamp(2006, 1, 3, 0, 0));
        assert_eq!(at(UTC_RANGE_END), stamp(2006, 1, 5, 0, 0));
        assert_eq!(at(UTC_JAN2_1700), stamp(2006, 1, 2, 17, 0));
        assert_eq!(at(UTC_JAN2_1800), stamp(2006, 1, 2, 18, 0));
        assert_eq!(at(UTC_JAN3_1700), stamp(2006, 1, 3, 17, 0));
        assert_eq!(at(UTC_JAN3_1800), stamp(2006, 1, 3, 18, 0));
        assert_eq!(at(UTC_JAN4_1700), stamp(2006, 1, 4, 17, 0));
        assert_eq!(at(UTC_JAN4_1900), stamp(2006, 1, 4, 19, 0));
        assert_eq!(at(UTC_JAN4_2000), stamp(2006, 1, 4, 20, 0));
    }

    /// RFC 4791 section 9.9 includes a point at the start of a range and excludes an event ending there.
    #[test]
    fn the_overlap_condition_is_the_one_section_9_9_writes_for_each_row() {
        let window = spanning(at(10_000), at(20_000));
        let cases: [(&str, Instance, bool); 8] = [
            (
                "no length, at the range start",
                instance(10_000, 10_000, 10_000),
                true,
            ),
            (
                "no length, a second before it",
                instance(9_999, 9_999, 9_999),
                false,
            ),
            (
                "no length, at the range end",
                instance(20_000, 20_000, 20_000),
                false,
            ),
            (
                "no length, a second before that",
                instance(19_999, 19_999, 19_999),
                true,
            ),
            (
                "an hour ending at the range start",
                instance(6_400, 6_400, 10_000),
                false,
            ),
            (
                "an hour ending a second later",
                instance(6_401, 6_401, 10_001),
                true,
            ),
            (
                "an hour starting at the range end",
                instance(20_000, 20_000, 23_600),
                false,
            ),
            (
                "an hour starting a second earlier",
                instance(19_999, 19_999, 23_599),
                true,
            ),
        ];
        for (shape, found, expected) in cases {
            assert_eq!(overlaps_range(found, window), expected, "{shape}");
        }
    }

    #[test]
    fn an_open_bound_is_closed_against_the_calendar_and_not_against_infinity() {
        let first = calendar_start().unwrap();
        let last = calendar_end().unwrap();
        assert_eq!(first, stamp(0, 1, 1, 0, 0));
        assert_eq!(
            last,
            stamp(9999, 12, 31, 23, 59).checked_add_seconds(60).unwrap(),
            "the upper bound is one past the last instant, because the window is half-open"
        );
        let series = event(None, OverrideSet::empty());
        let open_end = SearchBounds::compose(series, TimeRange::starting_at(at(UTC_RANGE_START)))
            .unwrap()
            .unwrap();
        assert_eq!(open_end.window().1, last);
        let open_start = SearchBounds::compose(series, TimeRange::ending_before(at(UTC_RANGE_END)))
            .unwrap()
            .unwrap();
        assert_eq!(open_start.window().0, first);
    }

    #[test]
    fn the_window_reaches_back_by_the_span_and_stops_at_the_range_end() {
        let utc = Series {
            clock: SeriesClock::Utc,
            ..event(None, OverrideSet::empty())
        };
        let window = spanning(at(10_000), at(20_000));
        let hour = SearchBounds::compose(utc, window).unwrap().unwrap();
        assert_eq!(
            hour.window(),
            (at(6_401), at(20_000)),
            "an instance starting there still ends one second inside the range"
        );
        let instantaneous = Series {
            occupies: InstanceSpan::INSTANT,
            ..utc
        };
        let point = SearchBounds::compose(instantaneous, window)
            .unwrap()
            .unwrap();
        assert_eq!(
            point.window(),
            (at(10_000), at(20_000)),
            "section 9.9 admits a zero-length component at the range start, so the window does"
        );
    }

    #[test]
    fn a_zoned_window_carries_the_slack_and_the_override_set_widens_generation() {
        let window = spanning(at(UTC_RANGE_START), at(UTC_RANGE_END));
        let plain = SearchBounds::compose(event(None, OverrideSet::empty()), window)
            .unwrap()
            .unwrap();
        assert_eq!(plain.window(), (at(WINDOW_LOWER), at(WINDOW_UPPER)));
        assert_eq!(plain.widening(), 0);
        assert_eq!(
            plain.generation(),
            plain.window(),
            "with nothing moved the two windows are the same one"
        );

        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [moved_jan4()];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let widened = SearchBounds::compose(event(None, overrides), window)
            .unwrap()
            .unwrap();
        assert_eq!(widened.widening(), TWO_HOURS);
        assert_eq!(widened.window(), (at(WINDOW_LOWER), at(WINDOW_UPPER)));
        assert_eq!(
            widened.generation(),
            (at(GENERATION_LOWER), at(GENERATION_UPPER))
        );
    }

    /// RFC 4791 section 7.8.3 returns two occurrences of `abcd2.ics` within the range.
    #[test]
    fn the_worked_example_expands_to_the_two_instances_the_rfc_returns() {
        let rule = daily(5);
        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [moved_jan4()];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let source = eastern();
        let mut budget = spend(&mut meter);
        let expansion = expand(
            event(Some(&rule), overrides),
            spanning(at(UTC_RANGE_START), at(UTC_RANGE_END)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            triples(expansion.instances()),
            alloc::vec![
                (UTC_JAN3_1700, UTC_JAN3_1700, UTC_JAN3_1800),
                (UTC_JAN4_1700, UTC_JAN4_1900, UTC_JAN4_2000),
            ]
        );
        assert_eq!(expansion.incomplete(), None);
        assert!(expansion.instances().last().unwrap().is_moved());
    }

    #[test]
    fn an_expansion_carries_the_witness_that_it_replaced_the_rule() {
        let rule = daily(5);
        let mut meter = Meter::new(Limits::DEFAULT);
        let source = eastern();
        let mut budget = spend(&mut meter);
        let expansion = expand(
            event(Some(&rule), OverrideSet::empty()),
            spanning(at(UTC_RANGE_START), at(UTC_RANGE_END)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        let reduction = expansion.reduction();
        assert!(reduction.expanded);
        assert!(!reduction.is_faithful());
        assert_eq!(
            reduction.code(),
            Some(DiagnosticCode::QueryCalendarDataReduced)
        );
        assert!(!reduction.components_dropped);
        assert!(!reduction.instances_dropped);
    }

    #[test]
    fn a_range_beside_the_series_matches_nothing_and_one_around_it_matches_everything() {
        let rule = daily(5);
        let source = eastern();
        let mut meter = Meter::new(Limits::DEFAULT);
        let cases: [(&str, TimeRange, Match); 3] = [
            (
                "a day a month before the first instance",
                spanning(stamp(2005, 12, 1, 0, 0), stamp(2005, 12, 2, 0, 0)),
                Match::Unmatched,
            ),
            (
                "a day a month after the last one",
                spanning(stamp(2006, 2, 1, 0, 0), stamp(2006, 2, 2, 0, 0)),
                Match::Unmatched,
            ),
            (
                "the whole of January",
                spanning(stamp(2006, 1, 1, 0, 0), stamp(2006, 2, 1, 0, 0)),
                Match::Matched,
            ),
        ];
        for (shape, window, expected) in cases {
            let mut budget = spend(&mut meter);
            let answer = overlaps(
                event(Some(&rule), OverrideSet::empty()),
                window,
                Zones::new(&source),
                &mut budget,
                &mut IgnoreDiagnostics,
            )
            .unwrap();
            assert_eq!(answer, expected, "{shape}");
        }
    }

    #[test]
    fn an_instance_an_override_moved_into_the_window_is_not_lost() {
        let rule = daily(5);
        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [moved_jan4()];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let source = eastern();
        let evening = spanning(stamp(2006, 1, 4, 18, 0), at(UTC_RANGE_END));

        let mut budget = spend(&mut meter);
        let moved = overlaps(
            event(Some(&rule), overrides),
            evening,
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(moved, Match::Matched);

        let mut second = Meter::new(Limits::DEFAULT);
        let mut untouched = spend(&mut second);
        let plain = overlaps(
            event(Some(&rule), OverrideSet::empty()),
            evening,
            Zones::new(&source),
            &mut untouched,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            plain,
            Match::Unmatched,
            "the same series without the override has nothing in that evening"
        );
    }

    /// The excluded ordinary occurrence makes the moved override the only possible match.
    #[test]
    fn an_override_moved_from_beyond_the_slack_is_still_generated() {
        let rule = daily(10);
        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [Override::new(
            at(NOMINAL_JAN8),
            OverrideRange::ThisOnly,
            Some(at(NOMINAL_JAN3)),
            PropertyDiff::empty(),
        )];
        let overrides = OverrideSet::new(&entries, &mut meter).unwrap();
        let exclusions = [at(NOMINAL_JAN3)];
        let series = Series {
            exdates: &exclusions,
            ..event(Some(&rule), overrides)
        };
        let third = spanning(at(UTC_RANGE_START), stamp(2006, 1, 4, 0, 0));
        let source = eastern();
        let mut budget = spend(&mut meter);
        let answer = overlaps(
            series,
            third,
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            answer,
            Match::Matched,
            "the January 8th key was moved onto the third, and the widening is what reaches it"
        );
        let bounds = SearchBounds::compose(series, third).unwrap().unwrap();
        assert!(bounds.widening() > super::ZONE_SLACK_SECONDS);
    }

    #[test]
    fn a_component_with_no_rule_is_its_own_single_instance() {
        let source = eastern();
        let window = spanning(at(UTC_JAN2_1700), at(UTC_RANGE_END));
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = spend(&mut meter);
        let answer = overlaps(
            event(None, OverrideSet::empty()),
            window,
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(answer, Match::Matched);

        let mut second = Meter::new(Limits::DEFAULT);
        let mut collecting = spend(&mut second);
        let expansion = expand(
            event(None, OverrideSet::empty()),
            window,
            Zones::new(&source),
            &mut collecting,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            triples(expansion.instances()),
            alloc::vec![(UTC_JAN2_1700, UTC_JAN2_1700, UTC_JAN2_1800)]
        );
    }

    #[test]
    fn an_exclusion_on_a_lone_dtstart_leaves_no_instance_at_all() {
        let source = eastern();
        let exclusions = [at(NOMINAL_JAN2)];
        let series = Series {
            exdates: &exclusions,
            ..event(None, OverrideSet::empty())
        };
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = spend(&mut meter);
        let answer = overlaps(
            series,
            spanning(at(UTC_JAN2_1700), at(UTC_RANGE_END)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(answer, Match::Unmatched);
    }

    #[test]
    fn a_floating_series_with_no_query_zone_is_undecidable_and_not_unmatched() {
        let rule = daily(5);
        let source = eastern();
        let series = Series {
            clock: SeriesClock::Floating,
            ..event(Some(&rule), OverrideSet::empty())
        };
        let window = spanning(at(UTC_RANGE_START), at(UTC_RANGE_END));
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut budget = spend(&mut meter);
        let answer = overlaps(
            series,
            window,
            Zones::new(&source),
            &mut budget,
            &mut reported,
        )
        .unwrap();
        assert_eq!(answer, Match::Undecided(Undecided::ZoneUnstated));

        let mut second = Meter::new(Limits::DEFAULT);
        let mut settled = spend(&mut second);
        let decided = overlaps(
            series,
            window,
            Zones::new(&source).with_query_zone(EASTERN),
            &mut settled,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            decided,
            Match::Matched,
            "the same series decides once the query says which zone to read it in"
        );

        let note = reported
            .iter()
            .find(|entry| entry.code() == DiagnosticCode::QueryFilterUndecidable)
            .copied()
            .unwrap();
        assert_eq!(note.severity(), Severity::Note);
        assert_eq!(
            note.subject().map(|named| named.as_bytes().to_vec()),
            Some(Undecided::ZoneUnstated.reason().as_bytes().to_vec()),
            "the reason travels as the subject, so six reasons are not one value"
        );
    }

    #[test]
    fn a_zone_no_source_recognizes_is_undecidable_rather_than_unmatched() {
        let rule = daily(5);
        let source = eastern();
        let series = Series {
            clock: SeriesClock::Zoned("Europe/Berlin"),
            ..event(Some(&rule), OverrideSet::empty())
        };
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = spend(&mut meter);
        let answer = overlaps(
            series,
            spanning(at(UTC_RANGE_START), at(UTC_RANGE_END)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(answer, Match::Undecided(Undecided::ZoneUnknown));
    }

    #[test]
    fn a_search_stopped_at_its_budget_is_undecidable() {
        let rule = daily(10_000);
        let source = eastern();
        let mut meter = Meter::with_budget(Limits::DEFAULT, 2);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = overlaps(
            event(Some(&rule), OverrideSet::empty()),
            spanning(stamp(2010, 1, 1, 0, 0), stamp(2010, 1, 2, 0, 0)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(answer, Match::Undecided(Undecided::SearchExhausted));
        assert!(budget.is_exhausted());
    }

    #[test]
    fn an_exhausted_ledger_is_undecidable_without_a_search() {
        let rule = daily(5);
        let source = eastern();
        let mut meter = Meter::with_budget(Limits::DEFAULT, 1);
        assert!(!meter.charge(4), "the ledger is spent on purpose");
        let already = meter.candidates_charged();
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = overlaps(
            event(Some(&rule), OverrideSet::empty()),
            spanning(at(UTC_RANGE_START), at(UTC_RANGE_END)),
            Zones::new(&source),
            &mut budget,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(answer, Match::Undecided(Undecided::SearchExhausted));
        assert_eq!(
            budget.meter.candidates_charged(),
            already,
            "no candidate was generated, because no search was started"
        );
    }

    #[test]
    fn a_range_outside_the_years_a_calendar_can_write_matches_nothing() {
        let series = Series {
            clock: SeriesClock::Utc,
            ..event(None, OverrideSet::empty())
        };
        let last = calendar_end().unwrap();
        let beyond = spanning(
            last.checked_add_seconds(10_000_000).unwrap(),
            last.checked_add_seconds(20_000_000).unwrap(),
        );
        assert_eq!(SearchBounds::compose(series, beyond).unwrap(), None);
    }

    #[test]
    fn every_undecidable_reason_fits_the_subject_it_travels_in() {
        for reason in [
            Undecided::ZoneUnstated,
            Undecided::ZoneUnknown,
            Undecided::ZoneAmbiguous,
            Undecided::SearchExhausted,
            Undecided::ValueUnreadable,
            Undecided::OverlapUndefined,
        ] {
            let carried = Subject::new(reason.reason().as_bytes());
            assert!(!carried.is_truncated(), "{reason}");
            assert_eq!(carried.as_bytes(), reason.reason().as_bytes());
        }
    }

    #[test]
    fn terminal_steps_and_drained_searches_keep_the_same_query_reason() {
        use crate::internal::recur::{BudgetExhausted, SearchOutcome, SearchStep};
        let exhausted = BudgetExhausted::new(at(0), 1);
        for (step, outcome, reason) in [
            (
                Some(SearchStep::CalendarArithmeticFailed),
                SearchOutcome::CalendarArithmeticFailed,
                Some(Undecided::ValueUnreadable),
            ),
            (
                None,
                SearchOutcome::CalendarArithmeticFailed,
                Some(Undecided::ValueUnreadable),
            ),
            (
                Some(SearchStep::BudgetExhausted(exhausted)),
                SearchOutcome::BudgetExhausted(exhausted),
                Some(Undecided::SearchExhausted),
            ),
            (
                None,
                SearchOutcome::BudgetExhausted(exhausted),
                Some(Undecided::SearchExhausted),
            ),
            (None, SearchOutcome::RuleEnded, None),
            (None, SearchOutcome::WindowEnded, None),
            (None, SearchOutcome::CalendarEnded, None),
            (
                None,
                SearchOutcome::Pending,
                Some(Undecided::SearchExhausted),
            ),
        ] {
            assert_eq!(super::query_step(step, outcome).break_value(), Some(reason));
        }
    }

    #[test]
    fn the_transcription_manifest_has_no_repeated_row() {
        let mut rows: Vec<&str> = EXPANSION_SECTIONS.to_vec();
        let stated = rows.len();
        rows.sort_unstable();
        rows.dedup();
        assert_eq!(rows.len(), stated);
        assert!(EXPANSION_SECTIONS.iter().any(|row| row.contains("9.6.5")));
        assert!(EXPANSION_SECTIONS.iter().any(|row| row.contains("9.9")));
    }
}
