// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 8 — free-busy report generation. RFC 4791 sections 7.10 and 9.11.
//!
//! # What this unit owns
//!
//! Turning a `CALDAV:free-busy-query` and a collection of resources into a
//! [`crate::internal::query::FreeBusyReport`]: the busy periods, merged, inside the window the query stated.
//!
//! - Only the components that contribute do. Section 7.10 counts `VEVENT` occurrences whose
//!   `TRANSP` is `OPAQUE` (the default) and whose `STATUS` is not `CANCELLED`, and the
//!   `FREEBUSY` periods of `VFREEBUSY` components. A `VEVENT` with `TRANSP:TRANSPARENT` states
//!   that its time is free and must not appear; counting it reports somebody busy when they said
//!   they were not.
//! - `STATUS:TENTATIVE` contributes as [`crate::internal::query::BusyType::Tentative`] rather than as `BUSY`, and
//!   the two must not be merged into one period.
//! - Expansion goes through `expand` and the window through the same composition, so a report
//!   and a `time-range` filter over the same window agree about which occurrences exist.
//! - Periods are clipped to the window and merged where they overlap and share a
//!   [`crate::internal::query::BusyType`]. Merging across types would state busy time the calendar does not.
//! - Every period is charged through [`crate::internal::query::FreeBusyReport::push`], which is where
//!   `Limits::max_freebusy_periods` binds. A report stopped at that bound calls
//!   [`crate::internal::query::FreeBusyReport::note_truncated`]: a caller reading the missing periods as free
//!   would double-book somebody, and a plain list of periods cannot say what is not in it.
//!
//! # Section 7.10's mapping, transcribed
//!
//! The free or busy type of a `VEVENT` is a function of two properties and not of one, and the
//! table is the whole of the rule:
//!
//! | `TRANSP`           | `STATUS`              | `FBTYPE`               |
//! | ------------------ | --------------------- | ---------------------- |
//! | `OPAQUE` (default) | `CONFIRMED` (default) | `BUSY`                 |
//! | `OPAQUE`           | `CANCELLED`           | `FREE`                 |
//! | `OPAQUE`           | `TENTATIVE`           | `BUSY-TENTATIVE`       |
//! | `OPAQUE`           | an `x-name`           | `BUSY` or the `x-name` |
//! | `TRANSPARENT`      | any of the four       | `FREE`                 |
//!
//! A row answering `FREE` states the absence of busy time, so it contributes no period at all:
//! [`crate::internal::query::BusyType::Free`]'s own definition says a report generated from events does not write
//! one. The `x-name` row is answered `BUSY`, which is the branch section 7.10 offers and this
//! crate has no extension vocabulary to take the other one with.
//!
//! # Coalescing
//!
//! Section 7.10: *"Servers SHOULD coalesce consecutive or overlapping busy time periods of the
//! same type."* Consecutive as well as overlapping — two `BUSY` periods that meet at one instant
//! come back as one, because half-open periods that abut describe exactly the same busy time
//! whether they are written as one or as two, and a client diffing two servers' answers should
//! not see a difference that is not one. Periods of *different* types are never coalesced, which
//! is why the same hour can appear twice.
//!
//! # No occurrence is placed here, and no length is derived here
//!
//! [`Placement`] carries `expand`'s own [`crate::internal::query::expand::Instance`] values, each already on the
//! UTC timeline with a start and an end. That is deliberate and it is the point of the unit
//! boundary: RFC 4791 section 9.9's table of what period a component occupies belongs to
//! `overlap`, placing that period through a zone belongs to `expand`, and a report that derived
//! either again would be free to disagree with a `time-range` filter run over the same window —
//! which is the one thing section 7.10 and section 9.9 have to agree about. What is left here is
//! section 7.10's own subject: which components contribute, under which type, clipped and
//! coalesced how.
//!
//! One consequence is worth stating because it is the case a re-derivation gets wrong. An
//! all-day event occupies its whole day in whichever zone places it, which is between 23 and 25
//! hours and never exactly 24. Nothing in this file divides a period by a day or adds 86,400 to
//! a start, so a 23-hour instance arrives 23 hours long and leaves 23 hours long.
//!
//! # The report has nowhere to put a third value
//!
//! A `VEVENT` with a floating `DTSTART` contributes busy time at no particular instant until a
//! zone places it, and `docs/adr/0003` forbids inventing one. Unlike a filter, a report has no
//! third answer available — a period is either in the list or it is not — so a resource that
//! could not be placed must not be silently omitted, because the caller would read the gap as
//! free time. [`Placement::incomplete`] carries the reason `expand` gives for an incomplete
//! expansion straight through to [`BusyAnswer::unplaced`], where the caller can see it.

use alloc::vec::Vec;

use ical_core::{
    Component, ComponentKind, DateTimeValue, DecodeValue, DiagnosticCode, Instant, LimitExceeded,
    Meter, Period, Property, UtcOffset,
};
use ical_dav::FreeBusyQuery;

use crate::internal::query::expand::Instance;
use crate::internal::query::vocabulary::{
    Budget, BusyPeriod, BusyType, FreeBusyReport, QueryError, Undecided,
};

/// What free-busy report generation is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const FREE_BUSY_SECTIONS: &[&str] = &[
    "RFC 4791 section 7.10, the CALDAV:free-busy-query REPORT",
    "RFC 4791 section 7.10, the TRANSP and STATUS to FBTYPE mapping",
    "RFC 4791 section 7.10, coalescing consecutive or overlapping periods of one type",
    "RFC 4791 section 9.9, the half-open window a time-range states",
    "RFC 4791 section 9.11, the free-busy-query body",
    "RFC 5545 section 3.2.9, FBTYPE",
    "RFC 5545 section 3.8.1.11, STATUS",
    "RFC 5545 section 3.8.2.6, FREEBUSY: a UTC period list",
    "RFC 5545 section 3.8.2.7, TRANSP",
];

/// Seconds in one day, RFC 5545 putting no leap second on the timeline.
///
/// Read at exactly one site — a `FREEBUSY` period written as a start and a duration, whose
/// bounds section 3.8.2.6 requires in UTC, where a day is a day because UTC runs no transitions.
/// It is not, and must not become, the length of anybody's calendar day.
const SECONDS_PER_DAY: i64 = 86_400;

/// The parameter RFC 5545 section 3.2.9 states a `FREEBUSY` period's type in.
const FBTYPE: &[u8] = b"FBTYPE";

/// `TRANSP:OPAQUE`, RFC 5545 section 3.8.2.7, and the reading an absent property gets.
const OPAQUE: &[u8] = b"OPAQUE";

/// `TRANSP:TRANSPARENT`, RFC 5545 section 3.8.2.7.
const TRANSPARENT: &[u8] = b"TRANSPARENT";

/// `STATUS:CANCELLED`, RFC 5545 section 3.8.1.11.
const CANCELLED: &[u8] = b"CANCELLED";

/// `STATUS:TENTATIVE`, RFC 5545 section 3.8.1.11.
const TENTATIVE: &[u8] = b"TENTATIVE";

/// One component of a calendar collection, expanded.
///
/// The component itself and not the calendar object holding it: unwrapping a `VCALENDAR` is the
/// caller's step, because the caller is the one that knows which of its children the query
/// reached. A component whose type section 7.10 does not consider is accepted and contributes
/// nothing, so a caller may hand in everything it holds without filtering first.
///
/// The three fields are exactly what `expand` produces for one component, so a caller writes
/// `Placement { component, instances: expansion.instances(), incomplete: expansion.incomplete() }`
/// and nothing else.
#[derive(Clone, Copy, Debug)]
pub struct Placement<'a> {
    /// The component the instances are of.
    pub component: &'a Component,
    /// The instances `expand` placed inside the window, each with a start and an end in UTC.
    ///
    /// Empty means `expand` placed none there, which contributes nothing. It does *not* mean
    /// "place it from `DTSTART`": a unit that guessed there would report busy time for a series
    /// the window excludes, and it would do so by deriving a period `overlap`'s table already
    /// owns.
    ///
    /// Ignored for a `VFREEBUSY`, whose `FREEBUSY` periods are absolute instants of their own
    /// (RFC 5545 section 3.8.2.6) rather than occurrences anything generates.
    pub instances: &'a [Instance],
    /// Why that expansion is incomplete, absent when it is not.
    ///
    /// The third value the report cannot hold, carried in so it can be handed back out. A budget
    /// that stopped a search, a `TZID` no source knew, a floating value the query stated no zone
    /// for: each leaves this component contributing less busy time than it has, and each reaches
    /// the caller through [`BusyAnswer::unplaced`] rather than as an absence.
    pub incomplete: Option<Undecided>,
}

/// One resource the report could not fully account for, and why.
///
/// The third value a list of periods has nowhere to hold. A caller that drops these is reporting
/// the time as free, which is the one reading that double-books somebody.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unplaced {
    /// Which entry of the placements handed to [`free_busy`] this is about.
    ///
    /// One entry may appear twice: an expansion that stopped short and a value inside the
    /// component that did not decode are two different facts about one resource, and collapsing
    /// them would drop whichever was noticed second.
    pub placement: usize,
    /// What could not be decided about it.
    pub reason: Undecided,
}

/// A free-busy report and what it could not account for.
///
/// The two travel together for the reason [`crate::internal::query::Selection`]'s two halves do: separating them
/// is how the second gets dropped, and a bare [`FreeBusyReport`] is indistinguishable from one
/// generated over a collection every member of which was placeable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyAnswer {
    /// The busy periods, coalesced and clipped to the query's window.
    report: FreeBusyReport,
    /// The resources that could not be fully accounted for, in the order they were handed in.
    unplaced: Vec<Unplaced>,
}

impl BusyAnswer {
    /// The report, whether or not everything reached it.
    #[must_use]
    pub const fn report(&self) -> &FreeBusyReport {
        &self.report
    }

    /// The resources that could not be fully accounted for.
    #[must_use]
    pub fn unplaced(&self) -> &[Unplaced] {
        &self.unplaced
    }

    /// Whether the report accounts for every resource it was given.
    ///
    /// `false` when something could not be placed *or* when a bound stopped the report short.
    /// One question rather than two, because a caller acts on both the same way: what it holds
    /// is less busy time than the collection states.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unplaced.is_empty() && !self.report.is_truncated()
    }

    /// The code an unaccounted resource is reported under, or `None` when there was none.
    #[must_use]
    pub fn code(&self) -> Option<DiagnosticCode> {
        if self.unplaced.is_empty() {
            None
        } else {
            Some(Undecided::CODE)
        }
    }

    /// The report, given up along with the record of what is missing from it.
    ///
    /// Named rather than a `From` impl, for [`crate::internal::query::Selection::into_calendar`]'s reason:
    /// discarding the witness is something a reader of the call site sees happening.
    #[must_use]
    pub fn into_report(self) -> FreeBusyReport {
        self.report
    }
}

/// Generate the busy time a `CALDAV:free-busy-query` asks for, RFC 4791 section 7.10.
///
/// `placements` is the collection: one entry per component, each carrying the instances `expand`
/// placed for it and the reason, if there is one, that it placed fewer than the component has.
/// `budget` bounds the report, through `Limits::max_freebusy_periods` and through the ledger
/// every period is charged to.
///
/// The refusals that end the whole call are the ones no report can be produced past: a value
/// this crate would have to write back and cannot spell ([`QueryError::Unrepresentable`]), and
/// an allocation the caller's ledger refused. Everything else is per resource and is reported.
///
/// One `Meter` per report, or one across a whole collection, is the caller's choice and it is
/// the choice `docs/adr/0010` describes: a shared ledger makes the period bound bind across
/// every report an exchange produces, and truncates the later ones rather than letting each
/// spend the bound again.
pub fn free_busy(
    query: FreeBusyQuery,
    placements: &[Placement<'_>],
    budget: &mut Budget<'_>,
) -> Result<BusyAnswer, QueryError> {
    let window = (query.range.start(), query.range.end());
    let mut coalesced = Coalesced::new(budget.limits.max_freebusy_periods());
    let mut unplaced = Vec::new();
    for (index, entry) in placements.iter().enumerate() {
        if let Some(reason) = entry.incomplete {
            record(&mut unplaced, index, reason, budget.meter)?;
        }
        match contribute(*entry, window, &mut coalesced) {
            Ok(()) => {},
            Err(Refusal::Unplaceable(reason)) => {
                record(&mut unplaced, index, reason, budget.meter)?;
            },
            Err(Refusal::Refused(error)) => return Err(error),
        }
    }
    Ok(BusyAnswer {
        report: assemble(window, coalesced, budget.meter),
        unplaced,
    })
}

/// What stopped one resource from reaching the report.
///
/// Two kinds and not one, because they end different things: a resource nothing could place is a
/// row of [`BusyAnswer::unplaced`] and the rest of the collection is still reported, while a
/// value this crate cannot write back ends the call. Collapsing them would either turn one bad
/// resource into a failed report or turn an unwritable answer into a silently short one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// This resource has no place on the timeline, and the answer says so.
    Unplaceable(Undecided),
    /// The whole evaluation stops here.
    Refused(QueryError),
}

impl From<Undecided> for Refusal {
    fn from(reason: Undecided) -> Self {
        Self::Unplaceable(reason)
    }
}

impl From<QueryError> for Refusal {
    fn from(error: QueryError) -> Self {
        Self::Refused(error)
    }
}

/// Add what one component contributes to `coalesced`.
fn contribute(
    placement: Placement<'_>,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    match placement.component.kind() {
        Some(ComponentKind::Event) => collect_event(placement, window, coalesced),
        Some(ComponentKind::FreeBusy) => collect_free_busy(placement.component, window, coalesced),
        // Section 7.10 names two component types and considers no others, so a `VTODO` or a
        // `VJOURNAL` contributes nothing — and that is the specification's own answer rather than
        // a gap, which makes it the one place in this file where saying nothing is not an
        // invention.
        Some(_) | None => Ok(()),
    }
}

/// Add every instance of one `VEVENT`, RFC 4791 section 7.10.
///
/// The type is read once from the component and applied to every instance, because `TRANSP` and
/// `STATUS` are properties of the component: an override that changed either would arrive as its
/// own component with its own placement.
fn collect_event(
    placement: Placement<'_>,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    let Some(kind) = event_kind(placement.component)? else {
        return Ok(());
    };
    for instance in placement.instances {
        let period = BusyPeriod::new(instance.start(), instance.end(), kind);
        coalesced.insert(clip(period, window))?;
    }
    Ok(())
}

/// What one `VEVENT` contributes, or `None` for the rows of the table that state `FREE`.
fn event_kind(component: &Component) -> Result<Option<BusyType>, Undecided> {
    if !is_opaque(component)? {
        // Every `TRANSPARENT` row of section 7.10's table answers `FREE`, whatever the `STATUS`
        // beside it says, so the `STATUS` is not even read.
        return Ok(None);
    }
    let status = component.status();
    let Some(text) = status.value() else {
        return if status.is_present() {
            Err(Undecided::ValueUnreadable)
        } else {
            // The table's default row: no `STATUS` is `CONFIRMED`, which is `BUSY`.
            Ok(Some(BusyType::Busy))
        };
    };
    // RFC 5545 section 3.1 compares an enumerated property value case-insensitively.
    let value = text.as_bytes();
    if value.eq_ignore_ascii_case(CANCELLED) {
        Ok(None)
    } else if value.eq_ignore_ascii_case(TENTATIVE) {
        Ok(Some(BusyType::Tentative))
    } else {
        // `CONFIRMED`, and the `x-name` row, which section 7.10 answers "BUSY or x-name". This
        // crate holds no extension vocabulary, so it takes the branch it can state.
        Ok(Some(BusyType::Busy))
    }
}

/// Whether a component's time is its own, RFC 5545 section 3.8.2.7.
fn is_opaque(component: &Component) -> Result<bool, Undecided> {
    let transp = component.transp();
    let Some(text) = transp.value() else {
        return if transp.is_present() {
            Err(Undecided::ValueUnreadable)
        } else {
            // Section 3.8.2.7's default, which section 7.10's table names as such.
            Ok(true)
        };
    };
    let value = text.as_bytes();
    if value.eq_ignore_ascii_case(OPAQUE) {
        Ok(true)
    } else if value.eq_ignore_ascii_case(TRANSPARENT) {
        Ok(false)
    } else {
        // Section 3.8.2.7 defines two values and section 7.10's table has a row for each. A third
        // is a value that is present and does not decode, and reading it as `OPAQUE` would report
        // somebody busy on a guess.
        Err(Undecided::ValueUnreadable)
    }
}

/// Add every `FREEBUSY` period of one `VFREEBUSY`, RFC 5545 section 3.8.2.6.
///
/// The `DTSTART` and `DTEND` of the component itself are deliberately not read. They state the
/// window the publisher's answer covers rather than time anybody is busy, and section 7.10
/// aggregates busy time: a `VFREEBUSY` carrying a week's window and one meeting states one busy
/// hour and not a busy week.
fn collect_free_busy(
    component: &Component,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    for property in component.freebusy() {
        let kind = free_busy_kind(property)?;
        if matches!(kind, BusyType::Free) {
            // `FREE` states the absence of busy time. Aggregating it as busy would invert it, and
            // emitting it beside an overlapping `BUSY` would state two contradictory things about
            // one hour.
            continue;
        }
        // Section 3.8.2.6 writes `fbvalue = period *("," period)`, so one line carries a list.
        for text in property
            .value_text()
            .as_bytes()
            .split(|&octet| octet == b',')
        {
            let written = Period::decode_value(text).map_err(|_| Undecided::ValueUnreadable)?;
            coalesced.insert(clip(utc_period(written, kind)?, window))?;
        }
    }
    Ok(())
}

/// Which type a `FREEBUSY` line states, RFC 5545 section 3.2.9.
fn free_busy_kind(property: &Property) -> Result<BusyType, Undecided> {
    match property.parameters_named(FBTYPE).next() {
        // Section 3.2.9 makes `BUSY` the value of an absent parameter.
        None => Ok(BusyType::Busy),
        Some(parameter) => BusyType::parse(parameter.unquoted()).ok_or(Undecided::ValueUnreadable),
    }
}

/// One `FREEBUSY` period, whose bounds RFC 5545 section 3.8.2.6 requires in UTC.
fn utc_period(written: Period<'_>, kind: BusyType) -> Result<BusyPeriod, Refusal> {
    let begins = utc_instant(written.start())?;
    let ends = match written {
        Period::Explicit { end, .. } => utc_instant(end)?,
        Period::Starting { duration, .. } => {
            // UTC runs no transitions, so a nominal day and an exact one are the same 86,400
            // seconds here and RFC 5545 section 3.3.6's split has nothing to decide. This is the
            // only place in this file where a day has a length at all.
            let carried = duration
                .days()
                .checked_mul(SECONDS_PER_DAY)
                .ok_or(QueryError::Unrepresentable)?;
            let length = carried
                .checked_add(duration.seconds())
                .ok_or(QueryError::Unrepresentable)?;
            begins
                .checked_add_seconds(length)
                .ok_or(QueryError::Unrepresentable)?
        },
    };
    Ok(BusyPeriod::new(begins, ends, kind))
}

/// One bound of a `FREEBUSY` period, which section 3.8.2.6 requires be written in UTC.
fn utc_instant(bound: DateTimeValue<'_>) -> Result<Instant, Refusal> {
    match bound {
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(Refusal::Refused(QueryError::Unrepresentable)),
        // A bound that is not in UTC is a value section 3.8.2.6 does not define, and placing it
        // in the query's zone would report busy time at an instant nobody wrote.
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Zoned { .. } => {
            Err(Refusal::Unplaceable(Undecided::ValueUnreadable))
        },
    }
}

/// The part of `period` that lies inside `window`, which may be none of it.
///
/// Half-open at both ends, as RFC 4791 section 9.9 writes every window in this workspace: a
/// period ending exactly where the window opens is outside it, and so is one beginning exactly
/// where the window closes.
fn clip(period: BusyPeriod, window: (Option<Instant>, Option<Instant>)) -> BusyPeriod {
    let start = match window.0 {
        Some(bound) if bound > period.start => bound,
        Some(_) | None => period.start,
    };
    let end = match window.1 {
        Some(bound) if bound < period.end => bound,
        Some(_) | None => period.end,
    };
    BusyPeriod::new(start, end, period.kind)
}

/// Whether RFC 4791 section 7.10 says to coalesce two periods, given they share a type.
///
/// "Consecutive or overlapping", so touching at one instant counts: two half-open periods that
/// abut describe the same busy time as the single period covering both, and a report that wrote
/// them separately would differ from a conformant server's for no reason a client could act on.
fn touches(held: BusyPeriod, other: BusyPeriod) -> bool {
    held.start <= other.end && other.start <= held.end
}

/// The busy periods gathered so far, kept coalesced and ordered as they arrive.
///
/// Coalesced on the way in rather than at the end, for one reason that is not tidiness:
/// [`FreeBusyReport`] has no way to withdraw a period once pushed, so merging afterwards is not
/// possible, and a working list that grew without bound before the merge would be the very
/// unbounded retention `Limits::max_freebusy_periods` exists to refuse.
#[derive(Debug)]
struct Coalesced {
    /// The periods, ordered by start and pairwise non-touching within each type.
    periods: Vec<BusyPeriod>,
    /// The most periods this list may hold, from the caller's policy.
    cap: usize,
    /// Whether that bound turned a period away.
    truncated: bool,
}

impl Coalesced {
    /// An empty list bounded at `cap` periods.
    fn new(cap: u32) -> Self {
        Self {
            periods: Vec::new(),
            // A policy larger than this target's addressable memory binds at the allocator
            // instead, which refuses in the same direction.
            cap: usize::try_from(cap).unwrap_or(usize::MAX),
            truncated: false,
        }
    }

    /// Record `period`, merging it into every period of its own type that it touches.
    ///
    /// One pass suffices because the list is ordered by start and holds no two touching periods
    /// of one type: an entry passed over cannot be brought into contact by a later merge, since
    /// the entry that would have done so is one this list would already have merged with it.
    fn insert(&mut self, period: BusyPeriod) -> Result<(), QueryError> {
        // Section 3.8.2.6 writes a period with a positive duration, so a zero-width one states no
        // busy time for anybody to read — and a period the window clipped away is exactly that.
        if period.is_empty() {
            return Ok(());
        }
        let mut grown = period;
        let before = self.periods.len();
        self.periods.retain(|held| {
            if held.kind != grown.kind || !touches(*held, grown) {
                return true;
            }
            grown.start = grown.start.min(held.start);
            grown.end = grown.end.max(held.end);
            false
        });
        if self.periods.len() == before && before >= self.cap {
            // Nothing was absorbed, so this period would be a new entry against a full list. A
            // period that *did* merge is always admitted, because it replaces at least one entry
            // and so cannot grow the list.
            self.truncated = true;
            return Ok(());
        }
        self.periods
            .try_reserve(1)
            .map_err(|_| LimitExceeded::FreeBusyPeriods)?;
        let at = self
            .periods
            .partition_point(|held| held.start <= grown.start);
        self.periods.insert(at, grown);
        Ok(())
    }
}

/// Move the gathered periods into the report the caller receives.
///
/// The only refusal [`FreeBusyReport::push`] gives is the free-busy period bound, which is a
/// truncation of the answer rather than a failure to produce one: the periods already recorded
/// are true, the ones after it are missing, and the report says so. The caller's ledger latches
/// on the same charge, so the fact survives being handed on as a plain list of periods.
fn assemble(
    window: (Option<Instant>, Option<Instant>),
    coalesced: Coalesced,
    meter: &mut Meter,
) -> FreeBusyReport {
    let mut report = FreeBusyReport::new(window.0, window.1);
    if coalesced.truncated {
        report.note_truncated();
    }
    for period in coalesced.periods {
        if report.push(period, meter).is_err() {
            report.note_truncated();
            break;
        }
    }
    report
}

/// Retain one unaccounted resource, charging the caller's ledger for holding it.
///
/// Charged against the octet budget because no other dimension counts it: a collection of
/// resources that are each unplaceable states no busy time at all, so it crosses no free-busy
/// period bound while buying as much retention as the collection is long.
fn record(
    unplaced: &mut Vec<Unplaced>,
    placement: usize,
    reason: Undecided,
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let cost = u64::try_from(core::mem::size_of::<Unplaced>()).unwrap_or(u64::MAX);
    meter.try_charge(cost)?;
    unplaced.try_reserve(1).map_err(|_| LimitExceeded::Budget)?;
    unplaced.push(Unplaced { placement, reason });
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{Component, Document, IgnoreDiagnostics, Instant, Limits, Meter};
    use ical_dav::{FreeBusyQuery, TimeRange};

    use super::{BusyAnswer, Placement, free_busy};
    use crate::internal::query::expand::Instance;
    use crate::internal::query::vocabulary::{Budget, BusyType, Undecided};

    /// 2026-01-01T00:00:00Z, where every window below opens.
    const WINDOW_START: i64 = 1_767_225_600;

    /// 2026-01-08T00:00:00Z, one week later.
    const WINDOW_END: i64 = 1_767_830_400;

    /// 2026-01-02T09:00:00Z.
    const AT_NINE: i64 = 1_767_344_400;

    /// 2026-01-02T10:00:00Z.
    const AT_TEN: i64 = 1_767_348_000;

    /// 2026-01-02T11:00:00Z.
    const AT_ELEVEN: i64 = 1_767_351_600;

    /// 2026-01-02T12:00:00Z.
    const AT_TWELVE: i64 = 1_767_355_200;

    /// 2026-01-02T13:00:00Z.
    const AT_THIRTEEN: i64 = 1_767_358_800;

    /// 2026-01-02T14:00:00Z.
    const AT_FOURTEEN: i64 = 1_767_362_400;

    /// 2025-12-31T23:00:00Z, one hour before the window opens.
    const BEFORE_WINDOW: i64 = 1_767_222_000;

    /// 2026-01-01T01:00:00Z, one hour after it opens.
    const INSIDE_WINDOW: i64 = 1_767_229_200;

    /// 2026-01-08T01:00:00Z, one hour after it closes.
    const AFTER_WINDOW: i64 = 1_767_834_000;

    /// One row of the transcription table.
    struct Case {
        /// What the row asserts, quoted at whichever assertion fails.
        what: &'static str,
        /// The calendar, whose children are the collection, in the order they are written.
        ics: &'static str,
        /// Where `expand` placed each child's one instance, as a start and an end.
        instances: &'static [(i64, i64)],
        /// The periods RFC 4791 section 7.10 says come back, in order.
        periods: &'static [(i64, i64, BusyType)],
        /// The resources it says cannot be accounted for, in order.
        unplaced: &'static [Undecided],
    }

    /// The calendar `ics` spells.
    fn parse(ics: &str) -> Document {
        Document::parse(ics.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// Every child of every `VCALENDAR`, unfiltered.
    ///
    /// Unfiltered on purpose: a component section 7.10 does not consider is handed in, so that
    /// its being skipped is something the table proves rather than something the test arranges.
    fn children(document: &Document) -> Vec<&Component> {
        document
            .components()
            .flat_map(Component::components)
            .collect()
    }

    /// The instances `spans` names, each addressed by its own start.
    fn placed(spans: &[(i64, i64)]) -> Vec<Instance> {
        spans
            .iter()
            .map(|&(start, end)| {
                let at = Instant::from_unix_seconds(start);
                Instance::new(at, at, Instant::from_unix_seconds(end))
            })
            .collect()
    }

    /// One placement per child, each carrying one instance and no incompleteness.
    fn one_each<'a>(components: &[&'a Component], found: &'a [Instance]) -> Vec<Placement<'a>> {
        components
            .iter()
            .zip(found.iter())
            .map(|(&child, at)| Placement {
                component: child,
                instances: core::slice::from_ref(at),
                incomplete: None,
            })
            .collect()
    }

    /// A query over `start ..< end`.
    fn query(start: i64, end: i64) -> FreeBusyQuery {
        FreeBusyQuery {
            range: TimeRange::new(
                Some(Instant::from_unix_seconds(start)),
                Some(Instant::from_unix_seconds(end)),
            )
            .unwrap(),
        }
    }

    /// The report as the triples a table row states.
    fn periods_of(answer: &BusyAnswer) -> Vec<(i64, i64, BusyType)> {
        answer
            .report()
            .periods()
            .iter()
            .map(|held| {
                (
                    held.start.unix_seconds(),
                    held.end.unix_seconds(),
                    held.kind,
                )
            })
            .collect()
    }

    /// The reasons the report could not account for a resource, in order.
    fn reasons_of(answer: &BusyAnswer) -> Vec<Undecided> {
        answer.unplaced().iter().map(|held| held.reason).collect()
    }

    /// Every row is RFC 4791 section 7.10's own answer, read off its mapping table, its
    /// coalescing sentence and section 9.9's half-open window.
    ///
    /// One table rather than thirty tests because that is the review this transcription can
    /// actually get: a reviewer puts the `periods` column beside the RFC and reads down it.
    const CASES: &[Case] = &[
        Case {
            what: "an opaque confirmed event is busy for its whole period",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:OPAQUE\r\nSTATUS:CONFIRMED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a VEVENT stating neither property takes the table's two defaults",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "TRANSPARENT states the time is free, so nothing is reported",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:c\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:TRANSPARENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "TRANSPARENT wins over a STATUS that would otherwise be busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:TRANSPARENT\r\nSTATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an opaque CANCELLED event maps to FREE, so nothing is reported",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "TENTATIVE maps to BUSY-TENTATIVE",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:f\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Tentative)],
            unplaced: &[],
        },
        Case {
            what: "a STATUS nothing defines takes the x-name row, which is BUSY",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:g\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:X-CATERING\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a TRANSP section 3.8.2.7 does not define is undecidable rather than opaque",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:h\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:X-MAYBE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "a busy hour and a tentative hour over the same time stay two periods",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:i1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:i2\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_NINE, AT_TEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_NINE, AT_TEN, BusyType::Tentative),
            ],
            unplaced: &[],
        },
        Case {
            what: "two consecutive busy periods of one type coalesce into one",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:j1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:j2\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_TEN, AT_ELEVEN)],
            periods: &[(AT_NINE, AT_ELEVEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "two overlapping busy periods of one type coalesce into one",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:k1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:k2\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_ELEVEN), (AT_TEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TWELVE, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a period the second one swallows whole leaves one period",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:l1\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:l2\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_TEN, AT_ELEVEN), (AT_NINE, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TWELVE, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an event that begins before the window is clipped to it",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m\r\n\
                  DTSTART:20251231T230000Z\r\nDTEND:20260101T010000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(BEFORE_WINDOW, INSIDE_WINDOW)],
            periods: &[(WINDOW_START, INSIDE_WINDOW, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an event ending exactly at the window start is outside a half-open window",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:n\r\n\
                  DTSTART:20251231T230000Z\r\nDTEND:20260101T000000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(BEFORE_WINDOW, WINDOW_START)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an event beginning exactly at the window end is outside it",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:o\r\n\
                  DTSTART:20260108T000000Z\r\nDTEND:20260108T010000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(WINDOW_END, AFTER_WINDOW)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an instance of no length states no busy time, section 9.9's instant row",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:p\r\n\
                  DTSTART:20260102T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_NINE)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "a VFREEBUSY's FREEBUSY periods are aggregated",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:q\r\n\
                  FREEBUSY:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE of BUSY-TENTATIVE is carried through",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:r\r\n\
                  FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Tentative)],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE of FREE states free time, which is not busy time",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:s\r\n\
                  FREEBUSY;FBTYPE=FREE:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE nothing defines is undecidable rather than busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:t\r\n\
                  FREEBUSY;FBTYPE=X-VACATION:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "one FREEBUSY line carrying two periods contributes both",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:u\r\n\
                  FREEBUSY:20260102T090000Z/20260102T100000Z,\
                  20260102T110000Z/20260102T120000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_THIRTEEN, AT_FOURTEEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_ELEVEN, AT_TWELVE, BusyType::Busy),
            ],
            unplaced: &[],
        },
        Case {
            what: "a period written as a start and a duration is aggregated too",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:v\r\n\
                  FREEBUSY:20260102T090000Z/PT1H\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a FREEBUSY bound that is not in UTC is undecidable rather than placed",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:w\r\n\
                  FREEBUSY:20260102T090000/20260102T100000\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "a VTODO is not one of the two component types section 7.10 considers",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:x\r\n\
                  DTSTART:20260102T090000Z\r\nDUE:20260102T100000Z\r\n\
                  END:VTODO\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "a collection with nothing in it reports nothing and hides nothing",
            ics: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
            instances: &[],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "every component of a collection contributes when every one is busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:y1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:y2\r\n\
                  DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VFREEBUSY\r\nUID:y3\r\n\
                  FREEBUSY:20260102T130000Z/20260102T140000Z\r\nEND:VFREEBUSY\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_ELEVEN, AT_TWELVE), (AT_NINE, AT_TEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_ELEVEN, AT_TWELVE, BusyType::Busy),
                (AT_THIRTEEN, AT_FOURTEEN, BusyType::Busy),
            ],
            unplaced: &[],
        },
    ];

    #[test]
    fn section_7_10_decides_every_row_of_the_table() {
        for case in CASES {
            let document = parse(case.ics);
            let components = children(&document);
            assert_eq!(
                components.len(),
                case.instances.len(),
                "the row states one instance per child: {}",
                case.what
            );
            let found = placed(case.instances);
            let placements = one_each(&components, &found);
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            let answer =
                free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

            assert_eq!(periods_of(&answer), case.periods, "{}", case.what);
            assert_eq!(reasons_of(&answer), case.unplaced, "{}", case.what);
            assert_eq!(
                answer.is_complete(),
                case.unplaced.is_empty(),
                "a resource that could not be accounted for is an incomplete report: {}",
                case.what
            );
        }
    }

    /// A day is between 23 and 25 hours and this unit never assumes otherwise.
    ///
    /// The lengths are the zone's, computed where `docs/adr/0003` says they belong: 2026-03-08 in
    /// a zone that springs forward at 02:00 local runs 05:00Z to 04:00Z the following day, which
    /// is 23 hours, and 2026-11-01 in the same zone runs 04:00Z to 05:00Z the following day,
    /// which is 25. What is asserted here is that a report neither rounds them to 24 nor
    /// re-derives them: what `expand` placed is what comes back.
    #[test]
    fn a_day_that_is_not_twenty_four_hours_long_survives_the_report_unchanged() {
        // 2026-03-08T05:00:00Z to 2026-03-09T04:00:00Z, and 2026-11-01T04:00:00Z to
        // 2026-11-02T05:00:00Z.
        let cases = [
            (
                "the day a zone springs forward",
                1_772_946_000_i64,
                1_773_028_800_i64,
                82_800_i64,
            ),
            (
                "the day it falls back",
                1_793_505_600_i64,
                1_793_595_600_i64,
                90_000_i64,
            ),
        ];
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:all-day\r\n\
                   DTSTART;VALUE=DATE:20260308\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);

        for (what, begins, ends, seconds) in cases {
            let found = placed(&[(begins, ends)]);
            let placements = one_each(&components, &found);
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            // A year-wide window, so what shortens or lengthens the day is the zone that placed
            // it and never the query's own clipping.
            let answer =
                free_busy(query(WINDOW_START, 1_798_761_600), &placements, &mut budget).unwrap();

            assert_eq!(
                periods_of(&answer),
                [(begins, ends, BusyType::Busy)],
                "{what}"
            );
            assert_eq!(
                Instant::from_unix_seconds(begins)
                    .checked_seconds_until(Instant::from_unix_seconds(ends)),
                Some(seconds),
                "{what}"
            );
            assert_ne!(
                seconds, 86_400,
                "a day of exactly 24 hours would prove nothing: {what}"
            );
        }
    }

    /// A report the period bound stopped says so, because a missing period reads as free time.
    #[test]
    fn a_report_stopped_at_its_bound_is_truncated_rather_than_short_and_silent() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:3\r\n\
                   DTSTART:20260102T130000Z\r\nDTEND:20260102T140000Z\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[
            (AT_NINE, AT_TEN),
            (AT_ELEVEN, AT_TWELVE),
            (AT_THIRTEEN, AT_FOURTEEN),
        ]);
        let placements = one_each(&components, &found);
        let limits = Limits::DEFAULT.with_max_freebusy_periods(2);
        let mut meter = Meter::new(limits);
        let mut budget = Budget::new(limits, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(answer.report().periods().len(), 2);
        assert!(answer.report().is_truncated());
        assert!(!answer.is_complete());
        assert_eq!(
            answer.report().window(),
            (
                Some(Instant::from_unix_seconds(WINDOW_START)),
                Some(Instant::from_unix_seconds(WINDOW_END))
            )
        );
    }

    /// A ledger already spent on an earlier report truncates the next one where it is pushed.
    #[test]
    fn a_shared_ledger_truncates_the_report_and_latches() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN), (AT_ELEVEN, AT_TWELVE)]);
        let placements = one_each(&components, &found);
        let limits = Limits::DEFAULT.with_max_freebusy_periods(2);
        let mut meter = Meter::new(limits);
        // One period already charged by an earlier report over the same ledger, which is the
        // aggregate posture a shared meter exists for.
        meter.try_charge_freebusy_period().unwrap();
        let mut budget = Budget::new(limits, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(answer.report().periods().len(), 1);
        assert!(answer.report().is_truncated());
        assert!(meter.is_exhausted(), "the ledger latches on the refusal");
    }

    /// An expansion that could not place a resource is reported and not read as free time.
    ///
    /// The row this crate exists to get right: the second component is in the window, nothing
    /// could say where, and a report that simply omitted it would tell the caller that hour is
    /// free.
    #[test]
    fn an_incomplete_expansion_is_reported_rather_than_read_as_free_time() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000\r\nDTEND:20260102T120000\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN)]);
        let placements = [
            Placement {
                component: components[0],
                instances: &found,
                incomplete: None,
            },
            Placement {
                component: components[1],
                instances: &[],
                incomplete: Some(Undecided::ZoneUnstated),
            },
        ];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TEN, BusyType::Busy)]);
        assert_eq!(answer.unplaced().len(), 1);
        assert_eq!(answer.unplaced()[0].placement, 1);
        assert_eq!(answer.unplaced()[0].reason, Undecided::ZoneUnstated);
        assert_eq!(answer.code(), Some(Undecided::CODE));
        assert!(!answer.is_complete());
        assert!(
            !answer.report().is_truncated(),
            "an unaccounted resource is not a truncation: no bound turned anything away"
        );
    }

    /// A partial expansion contributes what it placed and still reports what it did not.
    #[test]
    fn a_partial_expansion_contributes_its_instances_and_reports_the_rest() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:series\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=DAILY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN)]);
        let placements = [Placement {
            component: components[0],
            instances: &found,
            incomplete: Some(Undecided::SearchExhausted),
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TEN, BusyType::Busy)]);
        assert_eq!(reasons_of(&answer), [Undecided::SearchExhausted]);
        assert!(!answer.is_complete());
    }

    /// Every instance of a series contributes its own period, and adjacent ones coalesce.
    #[test]
    fn every_instance_of_a_series_contributes_its_own_period() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:series\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=HOURLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[
            (AT_NINE, AT_TEN),
            (AT_TEN, AT_ELEVEN),
            (AT_ELEVEN, AT_TWELVE),
        ]);
        let placements = [Placement {
            component: components[0],
            instances: &found,
            incomplete: None,
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        // Three consecutive hours of one type, which section 7.10 says to coalesce.
        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TWELVE, BusyType::Busy)]);
        assert!(answer.is_complete());
    }

    /// A series `expand` placed nowhere inside the window contributes nothing, and is not a
    /// resource that could not be placed.
    #[test]
    fn a_series_with_no_instance_in_the_window_is_not_an_unaccounted_resource() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:elsewhere\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=YEARLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let placements = [Placement {
            component: components[0],
            instances: &[],
            incomplete: None,
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert!(answer.report().periods().is_empty());
        assert!(answer.is_complete());
    }

    /// An open-ended window clips at the bound it has and nowhere else.
    #[test]
    fn a_window_open_at_one_end_clips_only_at_the_other() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:open\r\n\
                   DTSTART:20251231T230000Z\r\nDTEND:20260101T010000Z\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(BEFORE_WINDOW, INSIDE_WINDOW)]);
        let placements = one_each(&components, &found);
        let open_ended = FreeBusyQuery {
            range: TimeRange::starting_at(Instant::from_unix_seconds(WINDOW_START)),
        };
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(open_ended, &placements, &mut budget).unwrap();

        assert_eq!(
            periods_of(&answer),
            [(WINDOW_START, INSIDE_WINDOW, BusyType::Busy)]
        );
        assert_eq!(
            answer.report().window(),
            (Some(Instant::from_unix_seconds(WINDOW_START)), None)
        );
    }
}
