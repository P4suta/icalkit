// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 6 — the expansion-free prefilter. `docs/adr/0012`, clause 1.
//!
//! # Why this file exists before the measurement that decides it
//!
//! ADR 0012 defers one question: whether the plain filter walk is the deliverable, or whether
//! this crate must run per-resource bounds in front of it. It also fixes what happens if the
//! measurement has not run by the time the shape is needed — "the filter walk written so that
//! the prefilter is an internal step it calls, defaulting to *cannot exclude*, rather than a
//! rewrite". That is this module. The failing branch of the measurement is then an
//! implementation of one function, and the passing branch is this file staying as it is.
//!
//! # What this unit owns
//!
//! One question, answered without expanding anything: **can this resource be excluded from this
//! `time-range` outright?** Three answers, and the safe one is the default.
//!
//! - Read the resource's own bounds: `DTSTART`, `DTEND` or `DURATION`, and for each `RRULE` the
//!   upper bound its `UNTIL` or `COUNT` implies. `COUNT` bounds a series only together with its
//!   `FREQ` and `INTERVAL`, and the bound has to be an over-estimate: an under-estimate excludes
//!   a resource that matches, which is a wrong answer, where an over-estimate only costs an
//!   expansion that was going to happen anyway.
//! - `RDATE` may put an instance past every rule's `UNTIL`, and a `RECURRENCE-ID` override may
//!   move one anywhere at all. A resource carrying either cannot be excluded on rule bounds
//!   alone, and this unit must say "cannot exclude" for it rather than reasoning about where the
//!   override went.
//! - A bound that needs a zone and has none is "cannot exclude", not undecidable. The prefilter
//!   never produces a [`crate::internal::query::Match`]: it produces "excluded" or "cannot exclude", and every
//!   uncertainty resolves to the second, so the three-valued answer is always the walk's.
//!
//! # The invariant a reviewer checks this unit against
//!
//! Exclusion must be sound with respect to the walk: for every resource and every filter, if
//! this unit says "excluded" then the walk would have answered `Match::Unmatched`. It may be
//! arbitrarily incomplete — saying "cannot exclude" for everything is correct and is the
//! default — and it may never be unsound. That asymmetry is the whole reason the safe answer is
//! the one you get by doing nothing.
//!
//! # How the invariant is actually held here
//!
//! Four decisions carry it, and each one is a place where a cleverer answer would be wrong.
//!
//! *The quantifier is wider than the walk's.* The question is asked of the whole resource: every
//! component anywhere in the document that carries the filter's name has to be excludable before
//! the answer is "excluded". The walk asks about one scope, which is a subset of that, so an
//! exclusion established over the superset holds over the scope. A resource carrying no
//! component of that name at all is excluded, because RFC 4791 section 9.7.1's third bullet
//! makes a `comp-filter` carrying a `time-range` match only through a targeted component, and
//! there is none.
//!
//! *Only the two component types whose rows are a single inequality pair are read.* `VEVENT` and
//! `VJOURNAL` occupy one period, and RFC 4791 section 9.9 states it as `start < end-expression`
//! beside `end > DTSTART`. `VTODO`'s eight rows include one that reads `TRUE` — a to-do with no
//! `DTSTART`, `DUE`, `DURATION`, `COMPLETED` or `CREATED` overlaps every range — and three more
//! whose conditions are disjunctions, `VFREEBUSY` without `DTSTART` is the union of its
//! `FREEBUSY` periods, and a `VALARM`'s trigger is relative to a parent it does not name. Each of
//! those is a place where a bound taken from `DTSTART` alone would exclude something the walk
//! matches, so none of them is read at all. The table itself belongs to `overlap`; this unit
//! deliberately holds no second copy of it.
//!
//! *Nothing is placed on the timeline that a zone did not place.* A floating or zoned value goes
//! through [`Zones`] exactly as the walk's would, and a value it cannot place — no
//! `CALDAV:timezone` on the query, a `TZID` no source knows, a wall clock inside a gap — leaves
//! the whole resource unexcludable. That is not politeness: the walk answers
//! [`crate::internal::query::Undecided::ZoneUnstated`] for such a value, and `Undecided` is not `Unmatched`, so
//! excluding it would break the invariant in the one direction that loses a resource silently.
//! Where a wall clock names two instants the earlier one is taken, which is a lower bound under
//! either [`crate::internal::query::Zones::policy`] and therefore makes the "starts after the range" test hold
//! whatever the caller's policy is.
//!
//! *The far end is computed on one timeline and widened by two days.* The near end is an
//! instant — `DTSTART` placed by the zone, which is the earliest instance there is once `RDATE`
//! is excluded — and it needs no widening at all. The far end is not: a series' last instance is
//! read under whatever offset its own wall clock falls under, which is not the offset its first
//! one fell under, and an `UNTIL` may be written as an instant while the cadence it bounds is a
//! wall clock. So everything about the far end is computed on the *nominal* timeline
//! `ical-recur` walks a series on — every value read at UTC, no zone consulted — and one day is
//! added for the conversion back to an instant, plus one more for an `UNTIL` that arrived as an
//! instant and has to be read as a cadence bound. `UtcOffset` cannot hold a day (RFC 5545
//! section 3.3.14 writes at most `+hhmmss`), so each of those conversions moves a value by
//! strictly less than a day and two days covers both at once. Where every value involved is a
//! UTC `DATE-TIME` the two timelines are the same one, nothing is widened, and the boundary
//! cases below reproduce section 9.9's own inequalities exactly rather than approximately.
//!
//! # Measuring it
//!
//! The committed conformance case ADR 0012 specifies states its `Limits` policy *and* its octet
//! budget, gives each of the 5,000 resources **its own** meter, and records per resource whether
//! that meter is exhausted, the refusal variant, octets spent and candidates charged. A rate
//! over a shared meter is a boolean wearing a number, which is why the earlier form of that
//! measurement was withdrawn.

use core::num::NonZeroU32;

use alloc::vec::Vec;

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Component, ComponentKind, DateTimeValue, Document,
    Duration, Instant, PropertyId, UtcOffset,
};
use crate::internal::dav::{CompFilter, TimeRange};
use crate::internal::recur::{Freq, RecurrenceRule, RuleLimit, RulePart, UntilClock};
use crate::internal::tz::nominal;

use crate::internal::query::{Budget, Zones};

/// What the expansion-free prefilter is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const PREFILTER_SECTIONS: &[&str] = &[
    "docs/adr/0012, clause 1: the measurement that decides this unit",
    "docs/adr/0012: the default is an internal step that answers cannot-exclude",
    "RFC 4791 section 9.7.1, CALDAV:comp-filter: a time-range matches through a component",
    "RFC 4791 section 9.9, CALDAV:time-range: an inclusive start and a non-inclusive end",
    "RFC 4791 section 9.9, VEVENT: the DTEND, DURATION and bare rows of the overlap table",
    "RFC 4791 section 9.9, VJOURNAL: the DATE-TIME and DATE rows of the overlap table",
    "RFC 5545 section 3.3.10, RECUR: UNTIL and COUNT as the only bounds a rule states",
    "RFC 5545 section 3.3.14, UTC-OFFSET: the widest offset a value can state",
    "RFC 5545 section 3.8.5.2, RDATE: an instance no rule bound covers",
    "RFC 5545 section 3.8.4.4, RECURRENCE-ID: an override no rule bound covers",
];

/// Seconds in a day of wall clock, which is what a `DATE` value occupies.
const SECONDS_PER_DAY: i64 = 86_400;

/// Seconds in a week, which `FREQ=WEEKLY` advances by exactly.
const SECONDS_PER_WEEK: i64 = 604_800;

/// Seconds in the longest month, which `FREQ=MONTHLY` advances by at most.
const SECONDS_PER_LONG_MONTH: i64 = 2_678_400;

/// Seconds in the longest year, which `FREQ=YEARLY` advances by at most.
const SECONDS_PER_LONG_YEAR: i64 = 31_622_400;

/// Seconds in an hour, which `FREQ=HOURLY` advances by exactly.
const SECONDS_PER_HOUR: i64 = 3_600;

/// Seconds in a minute, which `FREQ=MINUTELY` advances by exactly.
const SECONDS_PER_MINUTE: i64 = 60;

/// What a bound computed on the nominal timeline is widened by before it is compared.
///
/// Two days, and each of them pays for one conversion between a wall clock and an instant. RFC
/// 5545 section 3.3.14 writes a `UTC-OFFSET` as at most `+hhmmss` and `crate::internal::core::UtcOffset`
/// refuses a whole day, so no zone any source can state moves a value by a day or more: one day
/// covers reading the last instance's wall clock as an instant, and one more covers an `UNTIL`
/// that arrived as an instant and bounds a cadence counted in wall clocks. An over-estimate here
/// costs an expansion that was going to happen; an under-estimate loses a resource.
const ZONE_SLACK_SECONDS: i64 = 172_800;

/// The `RRULE` identity this unit looks up. A `static` for the reason `ical-core`'s own
/// repeatable accessors are: [`Component::properties_named`] ties the identity's lifetime to
/// the iterator, and a reference to a `const` is a temporary that cannot be `'static`.
static RRULE: PropertyId = PropertyId::RRULE;

/// The `RDATE` identity this unit looks up. See [`RRULE`] for why it is a `static`.
static RDATE: PropertyId = PropertyId::RDATE;

/// The `RECURRENCE-ID` identity this unit looks up. See [`RRULE`] for why it is a `static`.
static RECURRENCE_ID: PropertyId = PropertyId::RECURRENCE_ID;

/// What the prefilter established, which is never that a resource *does* match.
///
/// Two values and not three. The prefilter is a way of not doing work, so the only answer worth
/// having is the one that skips the walk; everything else — a value it could not read, a zone it
/// could not get, a component type section 9.9 states no single-period rule for — is the same
/// answer, and that answer is what a caller gets by doing nothing at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Exclusion {
    /// No component of the filter's name can overlap the range, so the walk would answer
    /// `Match::Unmatched` and does not have to be run.
    Excluded,
    /// Nothing was established. The walk decides, as it would have anyway.
    #[default]
    CannotExclude,
}

/// Whether `filter`'s `time-range` can be refused for `calendar` without expanding anything.
///
/// The one entry point of this unit, and the seam `walk` calls before it expands a series. It
/// takes the whole resource rather than one component because a `RECURRENCE-ID` override lives
/// in a *sibling* component of the master it moves, so the question cannot be answered from the
/// targeted component alone.
///
/// It takes a [`CompFilter`] rather than a bare [`TimeRange`] so that the restriction it is sound
/// under is structural: the bounds here are a component's occupied period, which is the question
/// RFC 4791 section 9.9 answers for a `comp-filter`. The property-level test in the same section
/// — `(start <= date-time) AND (end > date-time)` over `DTSTAMP` and friends — is a different
/// question with a different answer, and a `prop-filter` cannot be passed here at all.
///
/// A filter carrying no `time-range`, or one `ical-dav` reports as contradictory, is
/// [`Exclusion::CannotExclude`]: there is nothing to exclude on, and a contradictory filter is
/// [`crate::internal::query::QueryError::Contradictory`] in the walk rather than an answer here.
///
/// `budget` is read and charged. An already exhausted ledger excludes nothing, because a bound
/// read under one is a bound that may have been cut short, and the octets of each `RRULE` this
/// re-reads are charged to the caller like every other octet this workspace reads twice.
#[must_use]
pub(crate) fn excludes(
    calendar: &Document,
    filter: &CompFilter,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Exclusion {
    if filter.is_contradictory() || budget.is_exhausted() {
        return Exclusion::CannotExclude;
    }
    let Some(range) = filter.time_range else {
        return Exclusion::CannotExclude;
    };
    let Some(components) = every_component(calendar) else {
        return Exclusion::CannotExclude;
    };
    // One override anywhere in the resource forbids every rule bound in it, because an override
    // with `RANGE=THISANDFUTURE` moves instances the master's own properties say nothing about.
    if components
        .iter()
        .copied()
        .any(|held| carries(held, &RECURRENCE_ID))
    {
        return Exclusion::CannotExclude;
    }
    let named = components
        .iter()
        .copied()
        .filter(|held| held.is_named(filter.name()));
    for candidate in named {
        let Some(occupied) = occupancy(candidate, zones, budget) else {
            return Exclusion::CannotExclude;
        };
        if !outside(occupied, range) {
            return Exclusion::CannotExclude;
        }
    }
    Exclusion::Excluded
}

/// Every component of `calendar` at every depth, or `None` when they do not fit in memory.
///
/// An explicit worklist rather than recursion. A calendar's depth is bounded by
/// `Limits::max_component_depth` and a filter's by `Limits::max_xml_depth`, but those are two
/// numbers and this walk is over one of them, so the bound that would protect the stack here is
/// not the bound that was charged when either was read.
fn every_component(calendar: &Document) -> Option<Vec<&Component>> {
    let mut found: Vec<&Component> = Vec::new();
    let mut pending: Vec<&Component> = Vec::new();
    for component in calendar.components() {
        admit(&mut pending, component)?;
    }
    while let Some(component) = pending.pop() {
        for child in component.components() {
            admit(&mut pending, child)?;
        }
        admit(&mut found, component)?;
    }
    Some(found)
}

/// Add one component to a list, refusing rather than aborting when the allocator says no.
fn admit<'a>(into: &mut Vec<&'a Component>, component: &'a Component) -> Option<()> {
    into.try_reserve(1).ok()?;
    into.push(component);
    Some(())
}

/// Whether `component` carries any property named `id`.
fn carries(component: &Component, id: &PropertyId) -> bool {
    component.properties_named(id).next().is_some()
}

/// The stretch of the timeline every instance of one component's recurrence set falls inside.
///
/// Both bounds are one-sided claims and neither is an instance. `earliest_start` is a lower
/// bound on the start of the earliest instance and `latest_end` an upper bound on the end of the
/// latest, so a range entirely outside the two is a range no instance reaches.
#[derive(Clone, Copy, Debug)]
struct Occupancy {
    /// A lower bound on the start of the earliest instance.
    earliest_start: Instant,
    /// An upper bound on the end of the latest instance, `None` for a series with no end.
    latest_end: Option<Instant>,
    /// Whether an instance occupies an instant rather than a period.
    ///
    /// RFC 4791 section 9.9 writes the bare `DATE-TIME` rows as `(start <= DTSTART)` and every
    /// row with a length as `(start < end-expression)`, so a range beginning exactly where a
    /// punctual component sits overlaps it and a range beginning exactly where a component with
    /// a length ends does not. One bit rather than two comparisons written twice.
    punctual: bool,
}

/// How long one instance occupies on the nominal timeline, and how the far end was read.
#[derive(Clone, Copy, Debug)]
struct Extent {
    /// The length of one instance in seconds, never negative.
    seconds: i64,
    /// Whether the value the length ends at names an instant outright.
    ///
    /// A length taken from a `DURATION`, or from a row that states no end at all, is octets and
    /// nothing else and is exact by construction. A `DTEND` is a value with a zone of its own,
    /// and one that is not UTC makes the far end a wall clock like every other.
    exact: bool,
}

/// An upper bound on where a component's recurrence set reaches, and how it was read.
#[derive(Clone, Copy, Debug)]
struct Horizon {
    /// An upper bound on the start of the latest instance, `None` when there is none.
    last: Option<Instant>,
    /// Whether every bound it was read from names an instant outright.
    anchored: bool,
}

impl Horizon {
    /// A series this unit can put no upper bound on at all.
    const UNBOUNDED: Self = Self {
        last: None,
        anchored: false,
    };
}

/// The stretch of timeline `component`'s whole recurrence set falls inside, if one can be read.
///
/// `None` is "this unit cannot say", which every caller turns into
/// [`Exclusion::CannotExclude`]: an `RDATE` that may sit past every rule bound, a component type
/// section 9.9 gives more than one period, a `DTSTART` that is absent, unreadable or unplaceable,
/// and arithmetic that leaves the representable timeline all arrive here as the same answer.
fn occupancy(
    component: &Component,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Option<Occupancy> {
    if carries(component, &RDATE) {
        return None;
    }
    let kind = component.kind()?;
    let opening = component.dtstart().value()?;
    // The near end is where the zone puts the first instance, and a zone that cannot place it
    // leaves the whole resource unexcludable — the walk answers `Undecided` there, and
    // `Undecided` is not `Unmatched`.
    let first = placed(opening, zones)?;
    // The far end is computed on the timeline `ical-recur` walks, which is every value read at
    // UTC, and is turned back into a claim about instants by the slack below.
    let anchor = nominal_of(opening)?;
    let length = extent(component, kind, opening)?;
    let horizon = series_horizon(component, anchor, opening.date(), budget);
    let slack = if is_utc(opening) && length.exact && horizon.anchored {
        0
    } else {
        ZONE_SLACK_SECONDS
    };
    let latest_end = horizon
        .last
        .and_then(|last| last.checked_add_seconds(length.seconds))
        .and_then(|edge| edge.checked_add_seconds(slack));
    Some(Occupancy {
        earliest_start: first,
        latest_end,
        punctual: length.seconds == 0,
    })
}

/// Whether no instance inside `occupied` can overlap `range`.
///
/// The two halves of RFC 4791 section 9.9's overlap conditions, each negated and each weakened
/// to whichever of `<` and `<=` is safe for every row this unit reads. A bound the resource does
/// not state — an open end of the range, a series with no last instance — decides nothing, which
/// is the section's own "assume -infinity and +infinity" read from the other side.
fn outside(occupied: Occupancy, range: TimeRange) -> bool {
    let after = range
        .end()
        .is_some_and(|edge| occupied.earliest_start.unix_seconds() >= edge.unix_seconds());
    let before = range
        .start()
        .zip(occupied.latest_end)
        .is_some_and(|(edge, last)| {
            if occupied.punctual {
                last.unix_seconds() < edge.unix_seconds()
            } else {
                last.unix_seconds() <= edge.unix_seconds()
            }
        });
    after || before
}

/// Where a date-time value sits on the timeline, or `None` when nothing places it there.
///
/// A UTC value places itself. Everything else is a wall clock, and a wall clock is placed by
/// [`Zones`] or not at all (`docs/adr/0003`). Where the zone names two instants the earlier is
/// taken, which is a lower bound under either [`crate::internal::query::Zones::policy`] — so the one test this
/// answer decides, whether the series starts after the range ends, does not depend on the
/// caller's reading of a fold.
fn placed(value: DateTimeValue<'_>, zones: Zones<'_>) -> Option<Instant> {
    match value {
        DateTimeValue::Utc(stamp) => stamp.at_offset(UtcOffset::UTC),
        DateTimeValue::Date(date) => {
            resolved(CivilDateTime::new(date, CivilTime::MIDNIGHT), None, zones)
        },
        DateTimeValue::Local(stamp) => resolved(stamp, None, zones),
        DateTimeValue::Zoned { stamp, tzid } => resolved(stamp, Some(tzid), zones),
    }
}

/// The instant a value's own wall clock names read at UTC.
///
/// The projection `crate::internal::tz::seam` puts every instant crossing into `ical-recur` through, and the
/// timeline a series' cadence is counted on. No zone is consulted and none can be: this is where
/// the far end of the occupied period is computed, and it is turned back into a claim about
/// instants by [`ZONE_SLACK_SECONDS`] rather than by a resolution.
fn nominal_of(value: DateTimeValue<'_>) -> Option<Instant> {
    let stamp = match value {
        DateTimeValue::Date(date) => CivilDateTime::new(date, CivilTime::MIDNIGHT),
        DateTimeValue::Local(stamp)
        | DateTimeValue::Utc(stamp)
        | DateTimeValue::Zoned { stamp, .. } => stamp,
    };
    nominal(stamp)
}

/// The earliest instant a wall clock names under the zone it is read in.
fn resolved(local: CivilDateTime, tzid: Option<&[u8]>, zones: Zones<'_>) -> Option<Instant> {
    let named = tzid.map(core::str::from_utf8).transpose().ok()?;
    zones.resolve(named, local).ok()?.resolution.earliest()
}

/// Whether a value names an instant with no zone involved.
const fn is_utc(value: DateTimeValue<'_>) -> bool {
    matches!(value, DateTimeValue::Utc(_))
}

/// How long one instance of `component` occupies, for the two component types read here.
///
/// `None` for every other kind, which is this unit's deliberate incompleteness rather than a gap
/// in section 9.9: `VTODO`, `VFREEBUSY` and `VALARM` each have rows a single period cannot stand
/// for, and reading them here would be a second copy of `overlap`'s table with weaker rules.
fn extent(
    component: &Component,
    kind: ComponentKind,
    opening: DateTimeValue<'_>,
) -> Option<Extent> {
    match kind {
        ComponentKind::Event => event_extent(component, opening),
        // "The effective 'duration' of a VJOURNAL component is 1 day (+P1D) when the DTSTART is
        // a DATE value, and 0 seconds when the DTSTART is a DATE-TIME value." A `DTEND` or a
        // `DURATION` on a journal entry is not part of either row and is not read.
        ComponentKind::Journal => Some(Extent {
            seconds: bare_seconds(opening),
            exact: true,
        }),
        _ => None,
    }
}

/// How long one instance of an event occupies, by section 9.9's `VEVENT` rows.
fn event_extent(component: &Component, opening: DateTimeValue<'_>) -> Option<Extent> {
    let ending = component.dtend();
    if ending.is_present() {
        let closing = ending.value()?;
        let seconds = nominal_of(opening)?
            .checked_seconds_until(nominal_of(closing)?)?
            .max(0);
        return Some(Extent {
            seconds,
            exact: is_utc(closing),
        });
    }
    let stated = component.duration();
    if stated.is_present() {
        // "DURATION property value is greater than 0 seconds?" — the row for a duration that is
        // not is the same row a bare `DATE-TIME` `DTSTART` takes, which a length of zero is.
        let seconds = duration_seconds(stated.value()?)?.max(0);
        return Some(Extent {
            seconds,
            exact: true,
        });
    }
    Some(Extent {
        seconds: bare_seconds(opening),
        exact: true,
    })
}

/// The period a component with no stated end occupies: "1 day (+P1D) when the DTSTART is a DATE
/// value, and 0 seconds when the DTSTART is a DATE-TIME value".
const fn bare_seconds(opening: DateTimeValue<'_>) -> i64 {
    match opening {
        DateTimeValue::Date(_) => SECONDS_PER_DAY,
        DateTimeValue::Local(_) | DateTimeValue::Utc(_) | DateTimeValue::Zoned { .. } => 0,
    }
}

/// A `DURATION` in seconds, or `None` when it does not fit the timeline.
fn duration_seconds(stated: Duration) -> Option<i64> {
    stated
        .days()
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(stated.seconds())
}

/// An upper bound on the start of the latest instance every `RRULE` on `component` generates.
///
/// [`Horizon::UNBOUNDED`] for anything unreadable as well as for a rule that states no end, so a
/// rule this unit declines to bound costs an expansion and never an exclusion. `first` is where
/// the series starts and `day` is `DTSTART`'s own civil date, which is the date the cadence
/// steps from — not the date its instant reads as at UTC, which is a different day for half the
/// zones on earth and would make a monthly bound wrong by a month.
fn series_horizon(
    component: &Component,
    first: Instant,
    day: CivilDate,
    budget: &mut Budget<'_>,
) -> Horizon {
    let mut reach = Horizon {
        last: Some(first),
        anchored: true,
    };
    for property in component.properties_named(&RRULE) {
        let octets = u64::try_from(property.value_text().as_bytes().len()).unwrap_or(u64::MAX);
        if budget.meter.try_charge_bytes(octets).is_err() {
            return Horizon::UNBOUNDED;
        }
        let Some(rule) = property.value::<RecurrenceRule>().value() else {
            return Horizon::UNBOUNDED;
        };
        let Some(bound) = rule_reach(&rule, first, day) else {
            return Horizon::UNBOUNDED;
        };
        reach.last = reach.last.map(|held| held.max(bound));
        reach.anchored = reach.anchored && until_is_anchored(&rule);
    }
    reach
}

/// An upper bound on the start of the latest instance one rule generates.
fn rule_reach(rule: &RecurrenceRule, first: Instant, day: CivilDate) -> Option<Instant> {
    match rule.limit() {
        RuleLimit::Infinite => None,
        // An `UNTIL` before `DTSTART` describes a series with nothing after its first instance,
        // and `DTSTART` is an instance whatever the rule says, so the later of the two bounds.
        RuleLimit::Until { at, .. } => Some(at.max(first)),
        RuleLimit::Count(count) => count_reach(rule, first, day, count),
    }
}

/// Whether a rule's end is stated as an instant rather than as a wall clock read for want of one.
const fn until_is_anchored(rule: &RecurrenceRule) -> bool {
    match rule.limit() {
        RuleLimit::Until { clock, .. } => matches!(clock, UntilClock::Utc),
        RuleLimit::Infinite | RuleLimit::Count(_) => true,
    }
}

/// An upper bound on where a `COUNT` reaches, or `None` when this unit will not guess one.
///
/// A `COUNT` is a number of occurrences and not a span, so turning it into one needs every period
/// of the cadence to yield an occurrence. A `BYxxx` part breaks that — `FREQ=YEARLY;BYDAY=MO;
/// BYMONTHDAY=1` yields nothing at all in most years — and so does a cadence that steps onto a
/// date some periods do not have, which is `FREQ=MONTHLY` from a 29th, 30th or 31st and
/// `FREQ=YEARLY` from a 29th of February. In every one of those the count is spread over more
/// periods than there are occurrences, and a bound computed as if it were not is an
/// **under**-estimate, which excludes a resource that matches.
fn count_reach(
    rule: &RecurrenceRule,
    first: Instant,
    day: CivilDate,
    count: NonZeroU32,
) -> Option<Instant> {
    if RulePart::ALL.iter().any(|part| rule.has_part(*part)) {
        return None;
    }
    let step = cadence_seconds(rule.freq(), day)?;
    let periods = i64::from(count.get().checked_sub(1)?);
    let reach = periods
        .checked_mul(i64::from(rule.interval().get()))?
        .checked_mul(step)?;
    first.checked_add_seconds(reach)
}

/// The most seconds one period of `freq` advances a series that starts on `day`.
///
/// `None` where a period can pass without producing an occurrence, for the reason
/// [`count_reach`] gives. The two calendar-shaped frequencies are the ones that can, and only
/// from a day of the month that some months do not have.
fn cadence_seconds(freq: Freq, day: CivilDate) -> Option<i64> {
    let seconds = match freq {
        Freq::Secondly => 1,
        Freq::Minutely => SECONDS_PER_MINUTE,
        Freq::Hourly => SECONDS_PER_HOUR,
        Freq::Daily => SECONDS_PER_DAY,
        Freq::Weekly => SECONDS_PER_WEEK,
        Freq::Monthly if day.day() <= 28 => SECONDS_PER_LONG_MONTH,
        Freq::Yearly if day.month() != 2 || day.day() != 29 => SECONDS_PER_LONG_YEAR,
        Freq::Monthly | Freq::Yearly => return None,
    };
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, Document, IgnoreDiagnostics, Instant, Limits, Meter,
        UtcOffset,
    };
    use crate::internal::dav::{CompFilter, TimeRange};
    use crate::internal::tz::FixedOffsetSource;

    use super::{Exclusion, excludes};
    use crate::internal::query::{Budget, Zones};

    /// The one identifier any source in these tests answers to.
    const PARIS: &str = "Europe/Paris";

    /// One case: a resource, the component name a `comp-filter` states, and what RFC 4791
    /// section 9.9 requires of it, read as an exclusion.
    struct Case {
        /// The passage the expectation is taken from, quoted where it is short enough to quote.
        about: &'static str,
        /// The component name the filter states.
        name: &'static [u8],
        /// The lines between `BEGIN:VCALENDAR` and `END:VCALENDAR`.
        body: &'static [&'static str],
        /// What this unit must answer.
        verdict: Exclusion,
    }

    /// A UTC instant, spelled the way a calendar spells one.
    fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, minute, second).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// The window every case in the table is asked about: 2026-03-01T00:00:00Z inclusive to
    /// 2026-04-01T00:00:00Z non-inclusive, which is section 9.9's reading of the two attributes.
    fn window() -> TimeRange {
        TimeRange::new(Some(at(2026, 3, 1, 0, 0, 0)), Some(at(2026, 4, 1, 0, 0, 0))).unwrap()
    }

    /// A calendar object resource carrying `body`.
    fn resource(body: &[&str]) -> Document {
        let mut text = String::new();
        text.push_str("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//prefilter//EN\r\n");
        for line in body {
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("END:VCALENDAR\r\n");
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// A source that answers one identifier with one offset, so that a case naming that zone is
    /// placed on the timeline and a case naming another one is not.
    fn paris() -> FixedOffsetSource {
        FixedOffsetSource::new(PARIS, UtcOffset::from_seconds(3_600).unwrap(), false)
    }

    /// What this unit answers for one resource, one component name, one window and one seam.
    fn verdict(body: &[&str], name: &[u8], range: TimeRange, zones: Zones<'_>) -> Exclusion {
        let calendar = resource(body);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut filter = CompFilter::new(name, limits, &mut meter).unwrap();
        filter.time_range = Some(range);
        let mut budget = Budget::new(limits, &mut meter);
        excludes(&calendar, &filter, zones, &mut budget)
    }

    /// Every row's expectation is section 9.9's own condition for the component state the row
    /// describes, negated over every instance the resource can generate: a row reads
    /// [`Exclusion::Excluded`] only where that condition is false for all of them, and
    /// [`Exclusion::CannotExclude`] both where an instance can satisfy it and where this unit
    /// declines to bound the series at all.
    const CASES: &[Case] = &[
        Case {
            about: "an event a decade before the range: (start < DTEND) is false",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:past@example.com",
                "DTSTART:20160315T090000Z",
                "DTEND:20160315T100000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "an event inside the range satisfies both halves of the VEVENT DTEND row",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:inside@example.com",
                "DTSTART:20260315T090000Z",
                "DTEND:20260315T100000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "DTSTART at the non-inclusive end: (end > DTSTART) is false",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:at-end@example.com",
                "DTSTART:20260401T000000Z",
                "DTEND:20260401T010000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "DTSTART one second inside the end: (end > DTSTART) holds",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:last-second@example.com",
                "DTSTART:20260331T235959Z",
                "DTEND:20260401T010000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "DTEND at the inclusive start: (start < DTEND) is false",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:abuts@example.com",
                "DTSTART:20260228T230000Z",
                "DTEND:20260301T000000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "DTEND one second past the start: (start < DTEND) holds",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:overlaps-by-a-second@example.com",
                "DTSTART:20260228T230000Z",
                "DTEND:20260301T000001Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a bare DATE-TIME DTSTART at the start: (start <= DTSTART) holds",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:punctual-at-start@example.com",
                "DTSTART:20260301T000000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a bare DATE-TIME DTSTART a second early: (start <= DTSTART) is false",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:punctual-before-start@example.com",
                "DTSTART:20260228T235959Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "a DURATION of no seconds takes the same row as a bare DATE-TIME DTSTART",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:zero-length@example.com",
                "DTSTART:20260301T000000Z",
                "DURATION:PT0S",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "DTSTART+DURATION at the inclusive start: (start < DTSTART+DURATION) is false",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:duration-abuts@example.com",
                "DTSTART:20260228T230000Z",
                "DURATION:PT1H",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "a COUNT the cadence reaches every period of: the fourth instance is in \
                    January",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:counted@example.com",
                "DTSTART:20260101T090000Z",
                "DTEND:20260101T100000Z",
                "RRULE:FREQ=WEEKLY;COUNT=4",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "a rule with neither COUNT nor UNTIL reaches every range after DTSTART",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:endless@example.com",
                "DTSTART:20260101T090000Z",
                "DTEND:20260101T100000Z",
                "RRULE:FREQ=WEEKLY",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "UNTIL before the range bounds every instance before it",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:until-before@example.com",
                "DTSTART:20260101T090000Z",
                "DTEND:20260101T100000Z",
                "RRULE:FREQ=WEEKLY;UNTIL=20260201T090000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "UNTIL inside the range leaves an instance the range can hold",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:until-inside@example.com",
                "DTSTART:20260101T090000Z",
                "DTEND:20260101T100000Z",
                "RRULE:FREQ=WEEKLY;UNTIL=20260315T090000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a COUNT beside a BYxxx part states no span: BYDAY can leave a period empty",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:counted-byday@example.com",
                "DTSTART:20260101T090000Z",
                "DTEND:20260101T100000Z",
                "RRULE:FREQ=WEEKLY;COUNT=4;BYDAY=MO",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a monthly COUNT from a 31st skips the months with no 31st: the eighth \
                    instance is 2026-03-31, inside the range, and a bound of seven months \
                    would have excluded it",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:monthly-from-the-31st@example.com",
                "DTSTART:20250331T090000Z",
                "DTEND:20250331T100000Z",
                "RRULE:FREQ=MONTHLY;COUNT=8",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "an RDATE may sit past every rule bound the component states",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:with-rdate@example.com",
                "DTSTART:20160315T090000Z",
                "DTEND:20160315T100000Z",
                "RRULE:FREQ=WEEKLY;COUNT=4",
                "RDATE:20260315T090000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a RECURRENCE-ID override may move an instance anywhere at all",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:overridden@example.com",
                "DTSTART:20160315T090000Z",
                "DTEND:20160315T100000Z",
                "RRULE:FREQ=WEEKLY;COUNT=4",
                "END:VEVENT",
                "BEGIN:VEVENT",
                "UID:overridden@example.com",
                "RECURRENCE-ID:20160322T090000Z",
                "DTSTART:20160322T090000Z",
                "DTEND:20160322T100000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a floating DTSTART and no CALDAV:timezone: nothing places it on a timeline",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:floating@example.com",
                "DTSTART:20160315T090000",
                "DTEND:20160315T100000",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a DATE DTSTART is a wall clock too, and the query stated no zone for it",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:all-day@example.com",
                "DTSTART;VALUE=DATE:20160315",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a TZID no supplied source recognizes places nothing either",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:unknown-zone@example.com",
                "DTSTART;TZID=Mars/Olympus:20160315T090000",
                "DTEND;TZID=Mars/Olympus:20160315T100000",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a TZID the source does recognize is placed, and a decade is past every slack",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:known-zone@example.com",
                "DTSTART;TZID=Europe/Paris:20160315T090000",
                "DTEND;TZID=Europe/Paris:20160315T100000",
                "END:VEVENT",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "a VJOURNAL with a DATE-TIME DTSTART: (start <= DTSTART) is false",
            name: b"VJOURNAL",
            body: &[
                "BEGIN:VJOURNAL",
                "UID:journal@example.com",
                "DTSTART:20160315T090000Z",
                "END:VJOURNAL",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "a VTODO's rows include one that reads TRUE, so none of them is read here",
            name: b"VTODO",
            body: &[
                "BEGIN:VTODO",
                "UID:todo@example.com",
                "DTSTART:20160315T090000Z",
                "DUE:20160315T100000Z",
                "END:VTODO",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "a VALARM's trigger is relative to a parent, so its own properties bound \
                    nothing",
            name: b"VALARM",
            body: &[
                "BEGIN:VEVENT",
                "UID:with-alarm@example.com",
                "DTSTART:20160315T090000Z",
                "DTEND:20160315T100000Z",
                "BEGIN:VALARM",
                "ACTION:DISPLAY",
                "DESCRIPTION:reminder",
                "TRIGGER:-PT15M",
                "END:VALARM",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
        Case {
            about: "no component of the filter's name: section 9.7.1 matches a time-range \
                    through a targeted component and there is none",
            name: b"VEVENT",
            body: &[
                "BEGIN:VTODO",
                "UID:only-a-todo@example.com",
                "DTSTART:20260315T090000Z",
                "END:VTODO",
            ],
            verdict: Exclusion::Excluded,
        },
        Case {
            about: "one component of the name inside the range is enough to keep the resource",
            name: b"VEVENT",
            body: &[
                "BEGIN:VEVENT",
                "UID:outside@example.com",
                "DTSTART:20160315T090000Z",
                "DTEND:20160315T100000Z",
                "END:VEVENT",
                "BEGIN:VEVENT",
                "UID:also-inside@example.com",
                "DTSTART:20260315T090000Z",
                "DTEND:20260315T100000Z",
                "END:VEVENT",
            ],
            verdict: Exclusion::CannotExclude,
        },
    ];

    /// An event a decade before the window, which several tests ask about under different seams.
    const PAST: &[&str] = &[
        "BEGIN:VEVENT",
        "UID:past@example.com",
        "DTSTART:20160315T090000Z",
        "DTEND:20160315T100000Z",
        "END:VEVENT",
    ];

    /// The same event with its zone left unstated, which is a wall clock and not an instant.
    const FLOATING: &[&str] = &[
        "BEGIN:VEVENT",
        "UID:floating@example.com",
        "DTSTART:20160315T090000",
        "DTEND:20160315T100000",
        "END:VEVENT",
    ];

    /// An event a year after the window.
    const FUTURE: &[&str] = &[
        "BEGIN:VEVENT",
        "UID:future@example.com",
        "DTSTART:20270315T090000Z",
        "DTEND:20270315T100000Z",
        "END:VEVENT",
    ];

    /// The instants the table is written against, pinned against an independently known epoch
    /// second so that its expectations are read from RFC 4791 rather than from this helper.
    ///
    /// 2026-01-01T00:00:00Z is 1,767,225,600 seconds after the epoch. January has 31 days and
    /// February 28 in 2026, which is not a leap year, so 2026-03-01T00:00:00Z is 59 days later
    /// and 2026-04-01T00:00:00Z is 31 days after that.
    #[test]
    fn the_window_the_table_is_written_against_is_the_one_it_says_it_is() {
        assert_eq!(at(2026, 1, 1, 0, 0, 0).unix_seconds(), 1_767_225_600);
        assert_eq!(at(2026, 3, 1, 0, 0, 0).unix_seconds(), 1_772_323_200);
        assert_eq!(at(2026, 4, 1, 0, 0, 0).unix_seconds(), 1_775_001_600);
    }

    #[test]
    fn every_row_answers_what_section_9_9_requires_of_it() {
        let source = paris();
        let zones = Zones::new(&source);
        for case in CASES {
            assert_eq!(
                verdict(case.body, case.name, window(), zones),
                case.verdict,
                "{}",
                case.about
            );
        }
    }

    /// The undecidable one, and why it is not a no-match. A floating value compared against a
    /// `time-range` is read in the zone `CALDAV:timezone` states (RFC 4791 section 9.9), and a
    /// query that stated none leaves the walk answering `Undecided::ZoneUnstated`. `Undecided`
    /// is not `Unmatched`, so excluding the resource would report an absence nothing
    /// established — the invariant this unit is held to, in the one direction that loses a
    /// resource silently. State the zone and the same octets are placed and excluded.
    #[test]
    fn a_floating_value_is_undecidable_without_a_zone_and_placed_with_one() {
        let source = paris();
        let unstated = Zones::new(&source);
        assert_eq!(
            verdict(FLOATING, b"VEVENT", window(), unstated),
            Exclusion::CannotExclude,
            "no zone was stated, so nothing put this value on a timeline"
        );
        let stated = Zones::new(&source).with_query_zone(PARIS);
        assert_eq!(
            verdict(FLOATING, b"VEVENT", window(), stated),
            Exclusion::Excluded,
            "the query's own zone places it, a decade outside the range"
        );
    }

    /// "If either the 'start' or 'end' attribute is not specified in the CALDAV:time-range XML
    /// element, assume '-infinity' and '+infinity' as their value, respectively." An absent
    /// bound decides nothing, and the bound that is there decides alone.
    #[test]
    fn an_open_bound_is_infinity_and_decides_nothing() {
        let source = paris();
        let zones = Zones::new(&source);
        let from_march = TimeRange::starting_at(at(2026, 3, 1, 0, 0, 0));
        let until_april = TimeRange::ending_before(at(2026, 4, 1, 0, 0, 0));

        assert_eq!(
            verdict(PAST, b"VEVENT", from_march, zones),
            Exclusion::Excluded
        );
        assert_eq!(
            verdict(PAST, b"VEVENT", until_april, zones),
            Exclusion::CannotExclude,
            "everything before April is in a range that starts at -infinity"
        );
        assert_eq!(
            verdict(FUTURE, b"VEVENT", until_april, zones),
            Exclusion::Excluded
        );
        assert_eq!(
            verdict(FUTURE, b"VEVENT", from_march, zones),
            Exclusion::CannotExclude,
            "everything after March is in a range that ends at +infinity"
        );
    }

    #[test]
    fn a_filter_with_nothing_to_exclude_on_excludes_nothing() {
        let source = paris();
        let zones = Zones::new(&source);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);

        let bare = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
        assert!(bare.time_range.is_none());

        let mut absent = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
        absent.is_not_defined = true;
        absent.time_range = Some(window());
        assert!(absent.is_contradictory());

        let calendar = resource(PAST);
        for filter in [&bare, &absent] {
            let mut budget = Budget::new(limits, &mut meter);
            assert_eq!(
                excludes(&calendar, filter, zones, &mut budget),
                Exclusion::CannotExclude
            );
        }
    }

    /// Exhaustion latches, so a bound read under a spent ledger is a bound that may have been
    /// cut short. `docs/adr/0010` makes that a reported outcome rather than an answer, and here
    /// the report is the absence of an exclusion.
    #[test]
    fn an_exhausted_ledger_excludes_nothing() {
        let source = paris();
        let zones = Zones::new(&source);
        let limits = Limits::DEFAULT;
        let mut scratch = Meter::new(limits);
        let mut filter = CompFilter::new(b"VEVENT", limits, &mut scratch).unwrap();
        filter.time_range = Some(window());

        let mut spent = Meter::with_budget(limits, 0);
        assert!(
            !spent.charge(1),
            "this test needs the ledger to be exhausted"
        );
        let mut budget = Budget::new(limits, &mut spent);
        let calendar = resource(PAST);
        assert_eq!(
            excludes(&calendar, &filter, zones, &mut budget),
            Exclusion::CannotExclude
        );
    }
}
