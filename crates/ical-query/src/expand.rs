// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 5 — expansion and window composition. RFC 4791 sections 9.6.5 and 9.9.
//!
//! # What this unit owns
//!
//! The bridge from a `time-range` to `ical-recur`, and it is the only file in this crate that
//! names `RecurrenceInput`, `Window` or `SearchStep`. Everything above it asks "does this
//! component occupy a period overlapping that range" and gets an answer; everything about how
//! that answer is obtained is here.
//!
//! - Compose the search window from the filter's `time-range` and the component's own bounds.
//!   Both of the range's bounds are independently optional; a search window is not, so an open
//!   bound has to be closed against something the calendar can express, and the choice has to be
//!   the same one `overlap` assumes. `ical_recur::generation_window` and `max_absolute_shift`
//!   exist because an override may move an instance into a window it would never have been
//!   generated in, and a window composed without them silently drops moved occurrences.
//! - Run the search under the caller's `Meter`. A search that stops at its budget is
//!   [`crate::Undecided::SearchExhausted`] through [`crate::Undecided::of_search`] — never an
//!   empty result, and never a resource reported as not matching.
//! - Stop at the first occurrence that overlaps. A filter asks whether *any* instance is in the
//!   window, so expanding the rest of a decade after the answer is known is work an attacker
//!   sizes.
//! - Honor `docs/adr/0002`'s ordering warning: occurrences are emitted by cadence key and not by
//!   effective start, so an override that moved an instance earlier arrives after instances that
//!   start later than it does. A walk that stopped at the first key past the window would miss
//!   it.
//!
//! # The seam with `ical-tz`, which is not optional
//!
//! `ical_tz::seam` states it in full and it is stated again here because getting it wrong puts
//! every zoned series an hour out for half the year: `ical-recur` works on the series' own wall
//! clock projected onto UTC, not on the UTC timeline. Every instant going in — `DTSTART`,
//! `UNTIL`, each `RDATE`, `EXDATE` and `RECURRENCE-ID` — goes through `ical_tz::nominal`, every
//! cadence key coming back through `ical_tz::wall_clock`, and each key is resolved against the
//! zone one at a time. Do that through [`crate::Zones`], which is the only door in this crate
//! that reaches a `ZoneSource`.
//!
//! [`Series`] therefore takes its instants **already nominal**, which is where the caller's own
//! projection ends, and every instant this unit hands back is **real UTC**, which is where the
//! comparison RFC 4791 section 9.9 states has to be made. [`SeriesClock`] is what says which of
//! RFC 5545 section 3.3.5's three forms the file wrote, because the way back is the identity for
//! one of them, the query's own `CALDAV:timezone` for another and the series' own `TZID` for the
//! third — and reading any of the three as another is the hour this seam exists to keep.
//!
//! ## Why the search window carries a day of slack
//!
//! The two timelines are not the same one, so a window stated on the real timeline cannot bound
//! nominal cadence keys exactly. It can bound them *safely*: a nominal instant and the real
//! instant it stands for differ by the zone's offset, and RFC 5545 section 3.3.14 writes an
//! offset as at most `+hhmmss`, which `ical_core::UtcOffset` refuses to hold a whole day of. So
//! every cadence key whose occurrence starts inside the caller's window lies inside that window
//! widened by [`ZONE_SLACK_SECONDS`], generation over the widened window is a superset, and the
//! exact comparison then filters it back down. A series written in UTC needs no slack at all,
//! because there the projection is the identity, and that case is the common one.
//!
//! # `CALDAV:expand`
//!
//! Section 9.6.5 asks for the instances instead of the rule: the returned components carry a
//! `RECURRENCE-ID`, carry no `RRULE`, `RDATE` or `EXDATE`, and have their `DTSTART` and `DTEND`
//! rewritten to UTC. That is a calendar the server did not store, so whatever this unit produces
//! for `subset` carries `crate::Reduction::expanded`.
//!
//! [`Instance`] is that shape as a value: three real UTC instants and nothing else, so the unit
//! that builds the calendar has nothing left to decide about the clock and no way to leave an
//! `RRULE` on a component by forgetting to remove it.
//!
//! # What this unit does not decide
//!
//! - Which period a component occupies is RFC 4791 section 9.9's table, and transcribing that
//!   table is `overlap`'s work. What arrives here is [`InstanceSpan`], the length of the period
//!   it selected.
//! - Whether a component type has an overlap rule at all. That is
//!   [`crate::Undecided::OverlapUndefined`], and it is decided before a window is composed.
//! - What a reduced calendar looks like as octets. This unit answers instants; `subset` writes
//!   components.

use alloc::vec::Vec;

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, ComponentKind, DateTimeValue, DecodeValue,
    Diagnostic, DiagnosticSink, Duration, Instant, Item, LimitExceeded, Meter, Period, Property,
    PropertyId, Severity, Subject, UtcOffset, View, report_diagnostic,
};
use ical_dav::TimeRange;
use ical_recur::{
    InputError, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput,
    RecurrenceRule, RuleLimit, UntilClock, ValueKind, Window, generation_window,
    max_absolute_shift,
};
use ical_tz::{nominal, wall_clock};

use crate::overlap::Occupancy;
use crate::vocabulary::{
    Budget, BusyPeriod, BusyType, Match, QueryError, Reduction, Undecided, Zones,
};

/// Property identities used while turning a stored component into one recurrence series.
///
/// These are statics because `Component::properties_named` ties the identity borrow to the
/// iterator. Keeping the identities here also makes the component-to-series bridge the only
/// place in this unit that knows how the stored RFC 5545 names map onto [`Series`].
mod ids {
    use ical_core::PropertyId;

    pub(super) static RRULE: PropertyId = PropertyId::RRULE;
    pub(super) static RDATE: PropertyId = PropertyId::RDATE;
    pub(super) static EXDATE: PropertyId = PropertyId::EXDATE;
    pub(super) static RECURRENCE_ID: PropertyId = PropertyId::RECURRENCE_ID;
    pub(super) static UID: PropertyId = PropertyId::UID;
    pub(super) static COMPLETED: PropertyId = PropertyId::from_static(b"COMPLETED");
    pub(super) static CREATED: PropertyId = PropertyId::from_static(b"CREATED");
}

/// What expansion and window composition is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
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

/// How far a nominal instant and the real instant it stands for can lie apart, in seconds.
///
/// RFC 5545 section 3.3.14 writes a UTC offset as `+hhmmss` at most, and
/// `ical_core::UtcOffset::from_seconds` refuses a magnitude of a whole day or more, so no zone
/// this workspace can hold moves a wall clock further than this from the instant it names. That
/// makes this an exact bound rather than a margin somebody guessed, which is what lets a window
/// composed on the real timeline bound the nominal keys of every occurrence inside it.
pub const ZONE_SLACK_SECONDS: i64 = 86_399;

/// The seconds in a nominal day, which is what RFC 4791 section 9.9's `+P1D` measures.
///
/// Nominal: the projection `ical_tz::seam` describes preserves civil fields, so one day on the
/// series' own wall clock is 86,400 seconds on that timeline whatever the zone did inside it.
/// Placing both ends of the span through the zone separately is what turns that back into the
/// 23 or 25 real hours the day actually had.
const NOMINAL_DAY_SECONDS: i64 = 86_400;

/// How long one instance of a component occupies, on the series' own clock.
///
/// The length of the period RFC 4791 section 9.9's table gives the component, handed in already
/// computed: this unit composes windows and expands series, and the table that decides which row
/// a component is on belongs to `overlap`.
///
/// Measured on the series' own wall clock rather than in elapsed seconds, because that is the
/// timeline every other instant crossing this seam is on and because it is what makes `+P1D` a
/// day. Both ends of the span are placed through the zone independently, so an instance that
/// spans a transition is as long in real seconds as the zone made it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceSpan {
    /// Seconds on the series' own wall clock, never negative.
    seconds: i64,
}

impl InstanceSpan {
    /// No length at all: the instant itself.
    ///
    /// RFC 4791 section 9.9 gives this to a `VEVENT` whose `DTSTART` is a `DATE-TIME` with
    /// neither `DTEND` nor `DURATION`, and to a `DURATION` of zero seconds. It is the one length
    /// that changes the comparison rather than only its operand, which is why
    /// [`InstanceSpan::is_instantaneous`] is asked at both sites the two rows differ.
    pub const INSTANT: Self = Self { seconds: 0 };

    /// `+P1D`, which section 9.9 gives a component whose `DTSTART` is a `DATE`.
    pub const WHOLE_DAY: Self = Self {
        seconds: NOMINAL_DAY_SECONDS,
    };

    /// A span of `seconds`, or `None` for a negative one.
    ///
    /// `None` rather than an absolute value. RFC 4791 section 9.9 requires `DTEND` to be later
    /// than `DTSTART`, so a negative span is a component the table cannot be evaluated for, and
    /// reading it as its own magnitude would answer for a period the file never described.
    #[must_use]
    pub const fn of_seconds(seconds: i64) -> Option<Self> {
        if seconds < 0 {
            None
        } else {
            Some(Self { seconds })
        }
    }

    /// The length, in seconds on the series' own clock.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    /// Whether the period is a single instant.
    #[must_use]
    pub const fn is_instantaneous(self) -> bool {
        self.seconds == 0
    }
}

/// Which clock a series' own values are written on, RFC 5545 section 3.3.5's three forms.
///
/// Closed rather than `#[non_exhaustive]`, because section 3.3.5 defines three forms and a
/// fourth would be a new value type rather than a new variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SeriesClock<'a> {
    /// Form 2, a `Z`-terminated value: the nominal timeline and the real one coincide.
    Utc,
    /// Form 1, a floating value: read in the zone the query's `CALDAV:timezone` stated.
    ///
    /// RFC 4791 section 9.9 makes that zone the one a floating comparison is made in and section
    /// 9.5 permits a query to state none, which is `docs/adr/0003`'s refusal to invent one and
    /// [`Undecided::ZoneUnstated`] here.
    Floating,
    /// Form 3, a value written with a `TZID`.
    Zoned(&'a str),
}

impl<'a> SeriesClock<'a> {
    /// The identifier this clock names, absent for the two forms that name none.
    const fn tzid(self) -> Option<&'a str> {
        match self {
            Self::Zoned(tzid) => Some(tzid),
            Self::Utc | Self::Floating => None,
        }
    }

    /// The real instant that a nominal instant of this series stands for.
    ///
    /// The outgoing half of the seam, in one place: the nominal instant is read back into the
    /// wall clock it spells, and that wall clock is resolved against the zone in force on that
    /// particular day. Resolving one key at a time is what keeps a daily 09:00 series at 09:00
    /// across a transition; applying one offset to an anchor would move half the year by an hour.
    ///
    /// A UTC series skips the door entirely, and that is not an optimization: a `Z`-terminated
    /// value names a real instant already, so asking a zone about it would be putting a question
    /// the file never posed.
    fn place(self, at: Instant, zones: Zones<'a>) -> Result<Instant, Undecided> {
        if matches!(self, Self::Utc) {
            return Ok(at);
        }
        // Unreachable for a key inside the years RFC 5545 section 3.3.4 writes, and checked
        // anyway: a generation window widened past them would otherwise be resolved into an
        // instant no calendar can name.
        let local = wall_clock(at).ok_or(Undecided::ValueUnreadable)?;
        let answer = zones.resolve(self.tzid(), local)?;
        answer
            .resolution
            .pick(zones.policy().gaps(), zones.policy().folds())
            .ok_or(Undecided::ZoneAmbiguous)
    }

    /// Whether the caller's zone and policy admit an occurrence at cadence key `key`.
    ///
    /// `docs/adr/0011`'s second gate, in the shape `ical_recur::RecurrenceInput::admitting`
    /// takes. This crate holds the zone and `ical-recur` holds `COUNT`, so an instance dropped
    /// after the count is an instance the count already spent: a `COUNT=5` series with one
    /// occurrence in an hour its zone never showed delivers four without it.
    ///
    /// A key the zone could not be asked about at all — no query zone, an identifier nothing
    /// recognizes — is admitted. Nothing there established that the local time does not exist,
    /// and a gate that dropped it would be answering a question nobody asked; the undecidability
    /// is reported when the occurrence is placed, where the caller sees it.
    fn admits(self, key: Instant, zones: Zones<'a>) -> bool {
        !matches!(self.place(key, zones), Err(Undecided::ZoneAmbiguous))
    }
}

/// One component's recurrence set, as the values RFC 5545 section 3.8.5 describes it with.
///
/// Every instant here is **nominal** — the series' own wall clock projected onto UTC, which is
/// the timeline `ical_tz::seam` puts `ical-recur` on. A caller holding a parsed component gets
/// them from `ical_tz::nominal` and from `ical_tz::ZonedSeries`; handing in real UTC instants for
/// a zoned series is the mistake the seam exists to name, and it moves the whole series by the
/// zone's offset.
#[derive(Clone, Copy, Debug)]
pub struct Series<'a> {
    /// `DTSTART`, nominal.
    pub dtstart: Instant,
    /// Whether `DTSTART` was written as a `DATE` or a `DATE-TIME`.
    pub dtstart_kind: ValueKind,
    /// Which of RFC 5545 section 3.3.5's forms this series' values are written on.
    pub clock: SeriesClock<'a>,
    /// How long one instance occupies, from RFC 4791 section 9.9's table.
    pub occupies: InstanceSpan,
    /// The `RRULE`, absent for a component that carries none.
    pub rule: Option<&'a RecurrenceRule>,
    /// The `RDATE` instants, nominal and ascending.
    pub rdates: &'a [Instant],
    /// The `EXDATE` instants, nominal and ascending.
    pub exdates: &'a [Instant],
    /// The `RECURRENCE-ID` overrides, nominal.
    pub overrides: OverrideSet<'a>,
}

/// One expanded recurrence instance, on the UTC timeline RFC 4791 section 9.6.5 requires.
///
/// Three instants and nothing else. Section 9.6.5 requires the returned components to carry a
/// `RECURRENCE-ID`, to carry no `RRULE`, `RDATE` or `EXDATE`, and to state their `DTSTART` and
/// `DTEND` as dates with UTC time; a value holding only what survives that leaves the unit
/// building the calendar nothing to decide about the clock and no recurrence property it can
/// forget to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instance {
    /// The `RECURRENCE-ID` this instance answers to: its cadence key, in UTC.
    recurrence_id: Instant,
    /// `DTSTART`, in UTC — where the instance happens, after any override moved it.
    start: Instant,
    /// `DTEND`, in UTC. Equal to `start` for a component RFC 4791 section 9.9 gives no length.
    end: Instant,
}

impl Instance {
    /// An instance addressed by `recurrence_id`, running from `start` up to `end`.
    #[must_use]
    pub const fn new(recurrence_id: Instant, start: Instant, end: Instant) -> Self {
        Self {
            recurrence_id,
            start,
            end,
        }
    }

    /// The cadence key this instance answers to, in UTC.
    ///
    /// Carried for every instance including the first. RFC 4791 section 9.6.5 requires it of
    /// every recurring component "other than the initial instance", which is a statement about
    /// what a server must write rather than about what an expansion knows.
    #[must_use]
    pub const fn recurrence_id(self) -> Instant {
        self.recurrence_id
    }

    /// `DTSTART`, in UTC.
    #[must_use]
    pub const fn start(self) -> Instant {
        self.start
    }

    /// `DTEND`, in UTC.
    #[must_use]
    pub const fn end(self) -> Instant {
        self.end
    }

    /// Whether an override moved this instance away from its cadence key.
    #[must_use]
    pub fn is_moved(self) -> bool {
        self.start != self.recurrence_id
    }
}

/// The window a search runs over, and how far an override set widened it.
///
/// Two windows rather than one, because they answer different questions and a caller that saw
/// only the second could not tell a widening from a wider query. [`SearchBounds::window`] is what
/// an occurrence is admitted against; [`SearchBounds::generation`] is what cadence keys are
/// generated over, and it reaches further precisely so that an override which moved an instance
/// into the window from a key outside it is still generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchBounds {
    /// The window over instance starts the filter asked about.
    window: Window,
    /// That window widened by the largest shift the override set implies.
    generation: Window,
    /// The widening itself, in seconds.
    widening: i64,
}

impl SearchBounds {
    /// The bounds a `time-range` and a series imply, or `None` when nothing can fall inside.
    ///
    /// `None` is a fact rather than a refusal: an open bound is closed against the years RFC 5545
    /// section 3.3.4 can write, so a range lying entirely outside them holds no instant any
    /// calendar can express and no instance can overlap it.
    pub fn compose(series: Series<'_>, range: TimeRange) -> Result<Option<Self>, QueryError> {
        let Some(window) = start_window(range, series.occupies, series.clock)? else {
            return Ok(None);
        };
        let widening = max_absolute_shift(series.overrides);
        // `None` where the widening leaves the timeline. The window asked about is then used
        // unchanged, which is what `ical-recur`'s own search does with it, and `widening` still
        // reports the shift — so a caller can see that some occurrence's start lies outside
        // anything a search could generate rather than being told the question narrowed.
        let generation = generation_window(window, series.overrides).unwrap_or(window);
        Ok(Some(Self {
            window,
            generation,
            widening,
        }))
    }

    /// The window over instance starts, as the first instant inside it and the first past it.
    ///
    /// Instants rather than an `ical_recur::Window`, so that the type stays inside this file.
    #[must_use]
    pub const fn window(self) -> (Instant, Instant) {
        (self.window.start(), self.window.end())
    }

    /// The window cadence keys are generated over.
    ///
    /// Equal to [`SearchBounds::window`] when no override moves anything, and equal to it again
    /// when the widening would leave the timeline — in which case [`SearchBounds::widening`] is
    /// the number that says so.
    #[must_use]
    pub const fn generation(self) -> (Instant, Instant) {
        (self.generation.start(), self.generation.end())
    }

    /// The largest absolute shift the override set implies, in seconds.
    #[must_use]
    pub const fn widening(self) -> i64 {
        self.widening
    }
}

/// Every instance of a series that a `CALDAV:expand` request asks to be returned.
///
/// The instances, the witness that they are not what the server stored, and the reason the set is
/// incomplete when it is. The third is not an error: an instance whose wall clock no zone could
/// place is one this expansion could not write in UTC, and a report that dropped it silently
/// would be a shorter answer that looked complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    /// The instances that overlap the range, in cadence order.
    instances: Vec<Instance>,
    /// What this expansion left out of the resource it came from.
    reduction: Reduction,
    /// Why the set is incomplete, absent when it is not.
    incomplete: Option<Undecided>,
}

impl Expansion {
    /// The instances that overlap the range, in the order the search produced them.
    ///
    /// Cadence order, which is not the order they start in: `docs/adr/0002` emits by cadence key,
    /// so an override that moved an instance earlier arrives after ones that start later than it
    /// does. A caller rendering a list sorts by [`Instance::start`] and knows why.
    #[must_use]
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// What this expansion left out, which is always at least the recurrence rule itself.
    #[must_use]
    pub const fn reduction(&self) -> Reduction {
        self.reduction
    }

    /// Why the set is incomplete, absent when every instance in the range is in it.
    #[must_use]
    pub const fn incomplete(&self) -> Option<Undecided> {
        self.incomplete
    }
}

/// Whether the recurrence set represented by one stored component overlaps `range`.
///
/// This is the storage-to-engine bridge used by the filter tree. A master component is read
/// together with its sibling `RECURRENCE-ID` components; an override component is never tested
/// as a second independent series. Non-recurring components go through RFC 4791 section 9.9's
/// complete table, while recurring event, to-do and journal components are reduced to the
/// single-instance span that the recurrence engine can move and repeat.
pub(crate) fn component_overlaps<S>(
    component: &Component,
    siblings: &[Item],
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    if carries(component, &ids::RECURRENCE_ID) {
        return Ok(Match::Unmatched);
    }
    let Some(kind) = component.kind() else {
        return Ok(Match::Undecided(Undecided::OverlapUndefined));
    };
    let related: Vec<&Component> = siblings
        .iter()
        .filter_map(Item::as_component)
        .filter(|candidate| is_override_of(candidate, component))
        .collect();
    let recurring =
        carries(component, &ids::RRULE) || carries(component, &ids::RDATE) || !related.is_empty();
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
        return Ok(crate::overlap::overlaps(kind, &held, range));
    }
    if !matches!(
        kind,
        ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal
    ) {
        return Ok(Match::Undecided(Undecided::OverlapUndefined));
    }

    recurring_component_overlaps(component, related, kind, range, zones, budget, sink)
}

/// Assemble and search one component known to carry a recurrence set.
fn recurring_component_overlaps<S>(
    component: &Component,
    related: Vec<&Component>,
    kind: ComponentKind,
    range: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
    sink: &mut S,
) -> Result<Match, QueryError>
where
    S: DiagnosticSink + ?Sized,
{
    macro_rules! decided {
        ($answer:expr) => {
            match $answer {
                Ok(value) => value,
                Err(reason) => return Ok(Match::Undecided(reason)),
            }
        };
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
    let mut rdates = instant_list(component, &ids::RDATE, clock, zones)?;
    let mut exdates = instant_list(component, &ids::EXDATE, clock, zones)?;
    rdates.sort_unstable();
    rdates.dedup();
    exdates.sort_unstable();
    exdates.dedup();

    let mut entries = Vec::new();
    entries
        .try_reserve(related.len())
        .map_err(|_| QueryError::Limit(LimitExceeded::Occurrences))?;
    for candidate in related {
        let addressed = decided!(required(
            candidate.get::<DateTimeValue<'_>>(&ids::RECURRENCE_ID)
        ));
        let recurrence_id = decided!(nominal_value(addressed, clock, zones));
        let moved_to = match decided!(optional(candidate.dtstart())) {
            Some(value) => Some(decided!(nominal_value(value, clock, zones))),
            None => None,
        };
        let range = candidate
            .properties_named(&ids::RECURRENCE_ID)
            .next()
            .is_some_and(|property| {
                property
                    .parameters_named(b"RANGE")
                    .any(|parameter| parameter.unquoted().eq_ignore_ascii_case(b"THISANDFUTURE"))
            });
        entries.push(Override::new(
            recurrence_id,
            if range {
                OverrideRange::ThisAndFuture
            } else {
                OverrideRange::ThisOnly
            },
            moved_to,
            PropertyDiff::empty(),
        ));
    }
    entries.sort_by_key(|entry| entry.recurrence_id());
    let overrides = OverrideSet::new(&entries, budget.meter).map_err(input_error)?;
    overlaps(
        Series {
            dtstart,
            dtstart_kind,
            clock,
            occupies,
            rule: rule.as_ref(),
            rdates: &rdates,
            exdates: &exdates,
            overrides,
        },
        range,
        zones,
        budget,
        sink,
    )
}

/// Map the only caller-policy failure in a prepared recurrence list onto the query channel.
fn input_error(error: InputError) -> QueryError {
    match error {
        InputError::TooMany(_, exceeded) => QueryError::Limit(exceeded),
        _ => QueryError::Unrepresentable,
    }
}

/// Whether `component` carries at least one property named by `id`.
fn carries(component: &Component, id: &'static PropertyId) -> bool {
    component.properties_named(id).next().is_some()
}

/// Whether `candidate` is an override belonging to `master`.
fn is_override_of(candidate: &Component, master: &Component) -> bool {
    if candidate.kind() != master.kind() || !carries(candidate, &ids::RECURRENCE_ID) {
        return false;
    }
    let master_uid = master
        .properties_named(&ids::UID)
        .next()
        .map(|property| property.value_text().as_bytes());
    let candidate_uid = candidate
        .properties_named(&ids::UID)
        .next()
        .map(|property| property.value_text().as_bytes());
    master_uid.is_some() && master_uid == candidate_uid
}

/// Turn a singular typed view into an optional value without flattening malformed into absent.
fn optional<T>(view: View<'_, T>) -> Result<Option<T>, Undecided> {
    match view {
        View::Absent => Ok(None),
        View::Valid { value, .. } => Ok(Some(value)),
        View::Malformed { .. } => Err(Undecided::ValueUnreadable),
    }
}

/// Turn a required typed view into its value.
fn required<T>(view: View<'_, T>) -> Result<T, Undecided> {
    optional(view)?.ok_or(Undecided::ValueUnreadable)
}

/// The clock a component's recurrence values are written on.
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
        DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => stamp,
        DateTimeValue::Utc(stamp) if matches!(clock, SeriesClock::Utc) => {
            return stamp
                .at_offset(UtcOffset::UTC)
                .ok_or(Undecided::ValueUnreadable);
        },
        DateTimeValue::Utc(stamp) => {
            let real = stamp
                .at_offset(UtcOffset::UTC)
                .ok_or(Undecided::ValueUnreadable)?;
            let answer = zones.offset_at(clock.tzid(), real)?;
            CivilDateTime::from_instant(real, answer.offset).ok_or(Undecided::ValueUnreadable)?
        },
    };
    nominal(local).ok_or(Undecided::ValueUnreadable)
}

/// Place a stored date or date-time on the UTC timeline.
fn placed_value(value: DateTimeValue<'_>, zones: Zones<'_>) -> Result<Instant, Undecided> {
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

/// Place one wall clock according to the query's explicit zone policy.
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

/// Read the one recurrence rule a component carries.
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

/// Every comma-separated date or date-time in properties named by `id`.
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
            let value = list_value(property, field).map_err(|_| QueryError::Unrepresentable)?;
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
    let value = DateTimeValue::decode_value(field).map_err(|_| Undecided::ValueUnreadable)?;
    let Some(parameter) = property.parameters_named(b"TZID").next() else {
        return Ok(value);
    };
    match value {
        DateTimeValue::Local(stamp) => Ok(DateTimeValue::Zoned {
            stamp,
            tzid: parameter.unquoted(),
        }),
        DateTimeValue::Date(_) | DateTimeValue::Utc(_) | DateTimeValue::Zoned { .. } => Ok(value),
    }
}

/// The wall-clock length of one recurring component instance.
fn instance_span(
    component: &Component,
    kind: ComponentKind,
    opening: DateTimeValue<'_>,
    clock: SeriesClock<'_>,
    zones: Zones<'_>,
) -> Result<InstanceSpan, Undecided> {
    let start = nominal_value(opening, clock, zones)?;
    let stated_end = match kind {
        ComponentKind::Event => optional(component.dtend())?,
        ComponentKind::Todo => optional(component.due())?,
        ComponentKind::Journal => None,
        _ => return Err(Undecided::OverlapUndefined),
    };
    if let Some(end) = stated_end {
        let end = nominal_value(end, clock, zones)?;
        let seconds = start
            .checked_seconds_until(end)
            .ok_or(Undecided::ValueUnreadable)?;
        return InstanceSpan::of_seconds(seconds).ok_or(Undecided::ValueUnreadable);
    }
    if !matches!(kind, ComponentKind::Journal) {
        if let Some(duration) = optional(component.duration())? {
            return duration_span(duration);
        }
    }
    Ok(if matches!(opening, DateTimeValue::Date(_)) {
        InstanceSpan::WHOLE_DAY
    } else {
        InstanceSpan::INSTANT
    })
}

/// A non-negative RFC 5545 duration as an instance span.
fn duration_span(duration: Duration) -> Result<InstanceSpan, Undecided> {
    let seconds = duration
        .days()
        .checked_mul(NOMINAL_DAY_SECONDS)
        .and_then(|days| days.checked_add(duration.seconds()))
        .ok_or(Undecided::ValueUnreadable)?;
    InstanceSpan::of_seconds(seconds).ok_or(Undecided::ValueUnreadable)
}

/// Read the values section 9.9's non-recurring overlap table names.
fn occupancy_of<'a>(
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
            let seconds = duration_span(span)?.seconds();
            let nominal_end = nominal_start
                .checked_add_seconds(seconds)
                .ok_or(Undecided::ValueUnreadable)?;
            Some(clock.place(nominal_end, zones)?)
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

/// Read a `VFREEBUSY` component's absolute UTC periods.
fn freebusy_periods(component: &Component) -> Result<Vec<BusyPeriod>, QueryError> {
    let mut periods = Vec::new();
    for property in component.freebusy() {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let written = Period::decode_value(field).map_err(|_| QueryError::Unrepresentable)?;
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

/// One FREEBUSY bound, which RFC 5545 requires to be UTC.
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

/// Whether any instance of `series` overlaps `range`, RFC 4791 section 9.9.
///
/// Section 9.9 makes the test a disjunction over the recurrence set — "if any one instance
/// matches, then the test returns true" — so the answer is [`Match::or`] folded over the
/// instances, which is also what makes stopping at the first overlap safe: Kleene's disjunction
/// is decided by a matched operand whatever the rest of them would have been.
///
/// The two answers that are not a fold are the ones a budget produces. A search that stopped at
/// its own limit did not establish that nothing matches, so it is [`Undecided::SearchExhausted`]
/// and never an empty result; and a ledger that had already latched before this call is the same
/// fact one layer earlier, so no search is started at all.
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
        decide(placed(series, key, start, zones), range, series.occupies)
    });
    if !answer.is_matched() {
        let incomplete = walk(series, bounds, zones, budget, &mut *sink, |found| {
            answer = answer.or(decide(found, range, series.occupies));
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

/// The instances of `series` that overlap `range`, RFC 4791 section 9.6.5.
///
/// Every instance rather than the first, because section 9.6.5 asks a server to return them and
/// not to decide anything with them. What comes back carries [`Reduction::expanded`]: the octets
/// a caller writes from it are well-formed iCalendar the server never stored, and writing them
/// back replaces a rule with the handful of instances one query asked about.
///
/// The range bounds the answer twice over, exactly as section 9.6.5 says it does: it is the
/// window the search runs over, and it is the overlap test each instance is then put to.
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
    if budget.is_exhausted() {
        collected.undecided = Some(Undecided::SearchExhausted);
    } else if let Some(bounds) = SearchBounds::compose(series, range)? {
        if let Some((key, start)) = lone_instance(series) {
            let found = placed(series, key, start, zones);
            if let Ok(established) = found {
                collected.offer(found, range, series.occupies);
                collected.lone = Some(established.recurrence_id());
            } else {
                collected.offer(found, range, series.occupies);
            }
        }
        let incomplete = walk(series, bounds, zones, budget, &mut *sink, |found| {
            collected.offer(found, range, series.occupies)
        })?;
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
    })
}

/// The instances an expansion has collected, and the two ways it stops early.
///
/// A named state rather than four captured bindings, because the closure that fills it is handed
/// to [`walk`], and a closure borrowing four locals mutably reads as four independent facts when
/// it is one.
#[derive(Debug)]
struct Collector {
    /// The instances kept so far.
    instances: Vec<Instance>,
    /// The `RECURRENCE-ID` of the instance established outside the search, if there was one.
    lone: Option<Instant>,
    /// Why the set is incomplete, absent when it is not.
    undecided: Option<Undecided>,
    /// The refusal that ended the collection, absent when none did.
    refused: Option<QueryError>,
}

impl Collector {
    /// An empty collection.
    const fn new() -> Self {
        Self {
            instances: Vec::new(),
            lone: None,
            undecided: None,
            refused: None,
        }
    }

    /// Offer one instance, answering whether the walk should carry on.
    ///
    /// An instance the zone could not place keeps the walk going and is recorded: the rest of the
    /// range is still expandable, and one occurrence in an hour a zone never showed does not make
    /// the others unknown.
    fn offer(
        &mut self,
        found: Result<Instance, Undecided>,
        range: TimeRange,
        span: InstanceSpan,
    ) -> bool {
        match found {
            Err(reason) => {
                self.undecided = self.undecided.or(Some(reason));
                true
            },
            Ok(instance) => self.keep(instance, range, span),
        }
    }

    /// Keep one placed instance if it overlaps, answering whether the walk should carry on.
    fn keep(&mut self, instance: Instance, range: TimeRange, span: InstanceSpan) -> bool {
        if !overlaps_range(instance, range, span) {
            return true;
        }
        if self.lone == Some(instance.recurrence_id()) {
            // The recurrence set is a set and `DTSTART` is in it once. A component with no rule
            // whose `RDATE` list repeats its own `DTSTART` offers this key twice, and the one
            // established before the search is the one carrying the override that addresses it.
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

/// Why a series could not be assembled into a search.
///
/// Two answers, because they reach the caller on different channels. A list longer than the
/// caller's own policy admits is a refusal the caller stated itself. A list that does not ascend,
/// or that names one instant twice, is a series whose instances cannot be established at all,
/// which is an undecided filter rather than an error: nothing about it says the resource does not
/// match.
#[derive(Debug)]
enum Unassembled {
    /// A caller-stated bound refused one of the series' own lists.
    Refused(QueryError),
    /// The lists are not the shape expansion requires, so no instance can be established.
    Undecidable(Undecided),
}

impl From<InputError> for Unassembled {
    fn from(error: InputError) -> Self {
        match error {
            InputError::TooMany(_, breach) => Self::Refused(QueryError::Limit(breach)),
            // `InputError` is `#[non_exhaustive]`, so this arm is required rather than chosen.
            // Undecidable is the conservative reading of a shape this unit cannot expand: it
            // neither claims the resource matches nor claims that it does not.
            _ => Self::Undecidable(Undecided::ValueUnreadable),
        }
    }
}

/// The window over instance starts that `range` asks about, or `None` when it holds none.
///
/// The whole of RFC 4791 section 9.9's start-edge distinction is the middle arm. An instance
/// occupying a positive span is inside when `start < DTSTART+DURATION`, so a start as early as
/// `range.start - span + 1` still reaches into the range; an instance occupying no time at all is
/// inside when `start <= DTSTART`, and shifting that bound by a second would drop every
/// zero-length component out of every query.
fn start_window(
    range: TimeRange,
    span: InstanceSpan,
    clock: SeriesClock<'_>,
) -> Result<Option<Window>, QueryError> {
    let first = calendar_start().ok_or(QueryError::Unrepresentable)?;
    let last = calendar_end().ok_or(QueryError::Unrepresentable)?;
    let lower = match range.start() {
        // "assume -infinity", section 9.9, closed against the years section 3.3.4 can write.
        None => first,
        Some(from) if span.is_instantaneous() => from,
        Some(from) => earlier(from, span.seconds().saturating_sub(1), first),
    };
    let upper = range.end().unwrap_or(last);
    let slack = match clock {
        // The projection is the identity, so the window needs no room for an offset.
        SeriesClock::Utc => 0,
        SeriesClock::Floating | SeriesClock::Zoned(_) => ZONE_SLACK_SECONDS,
    };
    Ok(Window::new(
        earlier(lower, slack, first),
        later(upper, slack, last),
    ))
}

/// Whether `instance` overlaps `range`, RFC 4791 section 9.9's condition for its own row.
///
/// Two comparisons, each of which an absent bound satisfies: section 9.9 reads a missing `start`
/// as minus infinity and a missing `end` as plus infinity, and a range with neither is one
/// `ical_dav::TimeRange` refuses to hold.
fn overlaps_range(instance: Instance, range: TimeRange, span: InstanceSpan) -> bool {
    let reaches_start = match range.start() {
        None => true,
        // `(start <= DTSTART)`, the rows for a component that occupies no time.
        Some(from) if span.is_instantaneous() => from <= instance.start(),
        // `(start < DTEND)` and `(start < DTSTART+DURATION)`, which are one condition once the
        // effective end has been computed.
        Some(from) => from < instance.end(),
    };
    // `(end > DTSTART)`, which every row of every table states the same way.
    reaches_start && range.end().is_none_or(|until| until > instance.start())
}

/// The answer one instance gives: a fact where the zone placed it, and not where it did not.
fn decide(found: Result<Instance, Undecided>, range: TimeRange, span: InstanceSpan) -> Match {
    match found {
        Ok(instance) => Match::of(overlaps_range(instance, range, span)),
        Err(reason) => Match::Undecided(reason),
    }
}

/// The real UTC instants an occurrence at nominal `key`, starting at nominal `start`, occupies.
///
/// Three placements at most and one at least. The cadence key and the effective start are the
/// same instant for every occurrence no override moved, and the end is the start again for a
/// component RFC 4791 section 9.9 gives no length, so the ordinary case asks the zone once.
fn placed<'a>(
    series: Series<'a>,
    key: Instant,
    start: Instant,
    zones: Zones<'a>,
) -> Result<Instance, Undecided> {
    let clock = series.clock;
    let happens_at = clock.place(start, zones)?;
    let addressed = if key == start {
        happens_at
    } else {
        clock.place(key, zones)?
    };
    let finishes_at = if series.occupies.is_instantaneous() {
        happens_at
    } else {
        // Both ends go through the zone, so the span is as long in real seconds as the zone made
        // that wall-clock day. The sum is checked because the span is a number a file supplies:
        // `DURATION:P100000000W` is a value RFC 5545 section 3.3.6 can write.
        let ends = start
            .checked_add_seconds(series.occupies.seconds())
            .ok_or(Undecided::ValueUnreadable)?;
        clock.place(ends, zones)?
    };
    Ok(Instance::new(addressed, happens_at, finishes_at))
}

/// The one instance a component with no `RRULE` has, which no search produces.
///
/// RFC 5545 section 3.8.5 puts `DTSTART` in every recurrence set, and `ical-recur` produces it
/// from the rule rather than adding it: a component with no rule yields its `RDATE` instants and
/// nothing else, and one with neither yields nothing at all. Most resources in a collection have
/// no rule, so a filter reading that silence as "no instance" would answer "no match" for nearly
/// every calendar object there is.
///
/// The exclusion list and any override addressing the key are applied here for the reason the
/// search applies them: an `EXDATE` naming `DTSTART` removes the only instance there is, and an
/// override that moved it moved the only start there is.
///
/// `None` where the component has a rule. There the recurrence set is what the rule generates,
/// including whether an unsynchronized `DTSTART` belongs to it — a question RFC 5545 section
/// 3.8.5.3 calls undefined and `ical-recur` answers once, on its own terms.
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

/// Walk the occurrences of `series` inside `bounds`, offering each to `visit`.
///
/// `visit` answers whether to carry on, which is how "stop at the first overlapping occurrence"
/// is stated once rather than at each caller. The reason the walk is incomplete comes back, and
/// it is `None` for a walk that reached the end of the search.
///
/// The window the search is given is the one the caller asked about and never the widened one:
/// `ical-recur` widens generation itself, from the same override set, and admits an occurrence
/// whose cadence key falls in the asked window **or** whose effective start does. Handing it the
/// widened window instead would admit occurrences by a key nobody asked about.
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
    let input = match assemble(series, &mut *budget.meter) {
        Ok(assembled) => assembled,
        Err(Unassembled::Refused(refusal)) => return Err(refusal),
        Err(Unassembled::Undecidable(reason)) => return Ok(Some(reason)),
    };
    let clock = series.clock;
    let admitted = move |key: Instant| clock.admits(key, zones);
    let gated = input.admitting(&admitted);
    let mut search = gated.search(bounds.window, &mut *budget.meter, sink);
    let mut incomplete = None;
    let mut stopped_by_caller = false;
    for step in &mut search {
        // `SearchStep::occurrence` answers `None` for the terminal step, and for any terminal
        // state a later `ical-recur` adds. Recording it rather than dropping it is the whole
        // difference between the discard that type documents and this: a search stopped short
        // never established that the range holds no instance.
        let Some(occurrence) = step.occurrence() else {
            incomplete = Some(Undecided::SearchExhausted);
            break;
        };
        if !visit(placed(series, occurrence.key(), occurrence.start(), zones)) {
            stopped_by_caller = true;
            break;
        }
    }
    if incomplete.is_none() && !stopped_by_caller {
        // Only for a walk the caller let run out. A search stopped early leaves its outcome
        // `SearchOutcome::Pending`, which `Undecided::of_search` reads as incomplete — true of
        // the search and false of the question, which was answered.
        incomplete = Undecided::of_search(search.outcome());
    }
    Ok(incomplete)
}

/// Assemble the search input `series` describes, charging its lists to `meter`.
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

/// The answer a call gets when the caller's ledger had already latched.
///
/// Exhaustion latches, so a search started here would walk a rule against a meter refusing every
/// charge and come back empty. That is the truncated-but-plausible answer a budget exists to
/// prevent, one layer up from where `docs/adr/0002` prevents it.
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

/// Report that a filter could not be decided, and why.
///
/// One code for all six reasons, which is what `crate::Undecided::CODE` fixes, and the reason
/// itself as the diagnostic's subject. A subject is what tells two reports of one code apart, and
/// six undecidable filters arriving as six equal values would tell a caller that something could
/// not be answered without saying what.
///
/// The instant is the series' own `DTSTART`, nominal: it is the one instant identifying the
/// component whatever went wrong, and an occurrence that could not be placed has no instant of
/// its own to name.
///
/// Nothing about the zone is reported here. `ical-tz` emits `ambiguous-local-time` and its
/// siblings from the answers themselves, and a second emission would make one awkward hour look
/// like two.
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

/// `at` moved `seconds` earlier, stopping at `floor`.
///
/// Saturating at the calendar's own edge rather than refusing: an instance long enough to reach
/// back past the year RFC 5545 section 3.3.4 begins at has a window that starts at that year, and
/// no instant before it exists to be missed.
fn earlier(at: Instant, seconds: i64, floor: Instant) -> Instant {
    seconds
        .checked_neg()
        .and_then(|back| at.checked_add_seconds(back))
        .unwrap_or(floor)
        .max(floor)
}

/// `at` moved `seconds` later, stopping at `ceiling`.
fn later(at: Instant, seconds: i64, ceiling: Instant) -> Instant {
    at.checked_add_seconds(seconds)
        .unwrap_or(ceiling)
        .min(ceiling)
}

/// The first instant RFC 5545 section 3.3.4 can write, on the nominal timeline.
///
/// Computed rather than written as a number, so that the bound and the calendar it comes from
/// cannot drift apart. `None` is unreachable — year zero, January, the first — and is checked
/// because a bound that holds today is not a bound the compiler checks.
fn calendar_start() -> Option<Instant> {
    nominal(CivilDateTime::new(
        CivilDate::from_ymd(0, 1, 1)?,
        CivilTime::MIDNIGHT,
    ))
}

/// The first instant past the last one RFC 5545 section 3.3.4 can write.
///
/// One second past 9999-12-31T23:59:59, because every window in this workspace is half-open and a
/// closed upper bound would exclude the last second a calendar can name.
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

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics,
        Instant, Limits, Meter, Severity, Subject, UtcOffset,
    };
    use ical_dav::TimeRange;
    use ical_recur::{
        Freq, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceRule,
        RecurrenceRuleBuilder, RuleLimit, ValueKind,
    };
    use ical_tz::FixedOffsetSource;

    use super::{
        EXPANSION_SECTIONS, Instance, InstanceSpan, SearchBounds, Series, SeriesClock,
        calendar_end, calendar_start, expand, overlaps, overlaps_range,
    };
    use crate::vocabulary::{Budget, Match, Undecided, Zones};

    /// The zone RFC 4791 section 7.8.2's calendar data is written in.
    const EASTERN: &str = "US/Eastern";

    /// `-05:00`, the offset `US/Eastern` runs at in January.
    const EST_SECONDS: i32 = -18_000;

    /// One hour, which is the `DURATION:PT1H` every event in the example carries.
    const ONE_HOUR: i64 = 3_600;

    // The instants of RFC 4791 section 7.8.3's worked example, transcribed by hand. The nominal
    // ones are the wall clocks the file writes -- `DTSTART;TZID=US/Eastern:20060102T120000` and
    // the daily keys after it -- projected by `ical_tz::seam`; the UTC ones are what those wall
    // clocks name under EST, and what the example's own `>> Response <<` states. Every one of
    // them is checked against `ical-core`'s calendar arithmetic by the first test below, so a
    // transcription slip fails a test rather than quietly moving the example.

    /// `20060102T120000` local, nominal: the example's `DTSTART`.
    const NOMINAL_JAN2: i64 = 1_136_203_200;
    /// `20060103T120000` local, nominal.
    const NOMINAL_JAN3: i64 = 1_136_289_600;
    /// `20060104T120000` local, nominal: the `RECURRENCE-ID` the example overrides.
    const NOMINAL_JAN4: i64 = 1_136_376_000;
    /// `20060104T140000` local, nominal: where that override moved the instance to.
    const NOMINAL_JAN4_MOVED: i64 = 1_136_383_200;
    /// `20060108T120000` local, nominal: a key five days past the window a case moves back.
    const NOMINAL_JAN8: i64 = 1_136_721_600;
    /// `20060103T000000Z`: the example's `start` attribute.
    const UTC_RANGE_START: i64 = 1_136_246_400;
    /// `20060105T000000Z`: the example's `end` attribute.
    const UTC_RANGE_END: i64 = 1_136_419_200;
    /// `20060102T170000Z`: where the instance before the range happens.
    const UTC_JAN2_1700: i64 = 1_136_221_200;
    /// `20060102T180000Z`: where it ends.
    const UTC_JAN2_1800: i64 = 1_136_224_800;
    /// `20060103T170000Z`: the `DTSTART` of the first instance the example returns.
    const UTC_JAN3_1700: i64 = 1_136_307_600;
    /// `20060103T180000Z`: its `DTEND`.
    const UTC_JAN3_1800: i64 = 1_136_311_200;
    /// `20060104T170000Z`: the `RECURRENCE-ID` the second returned instance carries.
    const UTC_JAN4_1700: i64 = 1_136_394_000;
    /// `20060104T190000Z`: the `DTSTART` it carries instead.
    const UTC_JAN4_1900: i64 = 1_136_401_200;
    /// `20060104T200000Z`: its `DTEND`.
    const UTC_JAN4_2000: i64 = 1_136_404_800;

    // The composed window for the example's range over an hour-long zoned event, by hand:
    // `20060103T000000Z` less the 3,599 seconds a positive span reaches back by, less the
    // 86,399 of zone slack; and `20060105T000000Z` plus that same slack. The generation window
    // is those two moved out by the 7,200 seconds the example's own override shifts.

    /// `1_136_246_400 - 3_599 - 86_399`.
    const WINDOW_LOWER: i64 = 1_136_156_402;
    /// `1_136_419_200 + 86_399`.
    const WINDOW_UPPER: i64 = 1_136_505_599;
    /// `WINDOW_LOWER - 7_200`.
    const GENERATION_LOWER: i64 = 1_136_149_202;
    /// `WINDOW_UPPER + 7_200`.
    const GENERATION_UPPER: i64 = 1_136_512_799;
    /// The shift `20060104T120000` to `20060104T140000` implies, in seconds.
    const TWO_HOURS: i64 = 7_200;

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// The example's zone, as the one offset January has in it.
    ///
    /// A fixed offset is not a zone and this source says so by construction. It is the right
    /// stand-in here and nowhere else: the `VTIMEZONE` the example carries puts `US/Eastern` on
    /// EST from the last Sunday in October to the first in April, so every instant under test is
    /// on `-05:00` and no transition falls inside any window here.
    fn eastern() -> FixedOffsetSource {
        FixedOffsetSource::new(
            EASTERN,
            UtcOffset::from_seconds(EST_SECONDS).unwrap(),
            false,
        )
    }

    /// The instant a UTC wall clock names, which is how every expected value is spelled.
    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        let time = CivilTime::from_hms(hour, minute, 0).unwrap();
        CivilDateTime::new(date, time)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// `FREQ=DAILY;COUNT=count`, the rule the example's `abcd2.ics` carries.
    fn daily(count: u32) -> RecurrenceRule {
        RecurrenceRuleBuilder::new(Freq::Daily)
            .limit(RuleLimit::Count(NonZeroU32::new(count).unwrap()))
            .build()
            .unwrap()
    }

    /// The example's override: the January 4th instance moved two hours later.
    fn moved_jan4() -> Override<'static> {
        Override::new(
            at(NOMINAL_JAN4),
            OverrideRange::ThisOnly,
            Some(at(NOMINAL_JAN4_MOVED)),
            PropertyDiff::empty(),
        )
    }

    /// The example's event, over whichever rule and override table a case supplies.
    fn event<'a>(rule: Option<&'a RecurrenceRule>, overrides: OverrideSet<'a>) -> Series<'a> {
        Series {
            dtstart: at(NOMINAL_JAN2),
            dtstart_kind: ValueKind::DateTime,
            clock: SeriesClock::Zoned(EASTERN),
            occupies: InstanceSpan::of_seconds(ONE_HOUR).unwrap(),
            rule,
            rdates: &[],
            exdates: &[],
            overrides,
        }
    }

    fn spanning(from: Instant, until: Instant) -> TimeRange {
        TimeRange::new(Some(from), Some(until)).unwrap()
    }

    fn instance(recurrence_id: i64, start: i64, end: i64) -> Instance {
        Instance::new(at(recurrence_id), at(start), at(end))
    }

    /// What an expansion produced, as the three UTC instants each instance carries.
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

    /// A budget over a fresh ledger, for a case that does not inspect the ledger afterwards.
    fn spend(meter: &mut Meter) -> Budget<'_> {
        Budget::new(Limits::DEFAULT, meter)
    }

    /// Every constant transcribed from RFC 4791 section 7.8.3 is the instant its text names.
    ///
    /// The transcription is what every other expectation in this module rests on, so it is
    /// checked against `ical-core`'s calendar arithmetic rather than against itself.
    #[test]
    fn the_worked_examples_instants_are_the_ones_the_rfc_writes() {
        // Nominal: the wall clock the file writes, read at UTC, which is the projection.
        assert_eq!(at(NOMINAL_JAN2), stamp(2006, 1, 2, 12, 0));
        assert_eq!(at(NOMINAL_JAN3), stamp(2006, 1, 3, 12, 0));
        assert_eq!(at(NOMINAL_JAN4), stamp(2006, 1, 4, 12, 0));
        assert_eq!(at(NOMINAL_JAN4_MOVED), stamp(2006, 1, 4, 14, 0));
        assert_eq!(at(NOMINAL_JAN8), stamp(2006, 1, 8, 12, 0));
        // Real: `20060103T000000Z` and the rest, as the example writes them.
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

    /// RFC 4791 section 9.9's VEVENT table, at every boundary the two rows distinguish.
    ///
    /// The expected column is the RFC's own condition evaluated by hand over a range of
    /// `[10_000, 20_000)`: `(start <= DTSTART AND end > DTSTART)` for a component that occupies
    /// no time, and `(start < DTSTART+DURATION AND end > DTSTART)` for one that does.
    #[test]
    fn the_overlap_condition_is_the_one_section_9_9_writes_for_each_row() {
        let window = spanning(at(10_000), at(20_000));
        let point = InstanceSpan::INSTANT;
        let hour = InstanceSpan::of_seconds(ONE_HOUR).unwrap();
        let cases: [(&str, Instance, InstanceSpan, bool); 8] = [
            // `(start <= DTSTART)`: the first instant of the range is inside it.
            (
                "no length, at the range start",
                instance(10_000, 10_000, 10_000),
                point,
                true,
            ),
            (
                "no length, a second before it",
                instance(9_999, 9_999, 9_999),
                point,
                false,
            ),
            // `(end > DTSTART)`: the range's own end is not inside it.
            (
                "no length, at the range end",
                instance(20_000, 20_000, 20_000),
                point,
                false,
            ),
            (
                "no length, a second before that",
                instance(19_999, 19_999, 19_999),
                point,
                true,
            ),
            // `(start < DTSTART+DURATION)`: an hour ending where the range opens is outside.
            (
                "an hour ending at the range start",
                instance(6_400, 6_400, 10_000),
                hour,
                false,
            ),
            (
                "an hour ending a second later",
                instance(6_401, 6_401, 10_001),
                hour,
                true,
            ),
            // `(end > DTSTART)`: an hour starting where the range closes is outside.
            (
                "an hour starting at the range end",
                instance(20_000, 20_000, 23_600),
                hour,
                false,
            ),
            (
                "an hour starting a second earlier",
                instance(19_999, 19_999, 23_599),
                hour,
                true,
            ),
        ];
        for (shape, found, span, expected) in cases {
            assert_eq!(overlaps_range(found, window, span), expected, "{shape}");
        }
    }

    /// An open bound is closed against the years RFC 5545 section 3.3.4 can write.
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

    /// The composed window is the range reaching back by the span, and by nothing else.
    ///
    /// A UTC series is the case with no slack in it, so the arithmetic RFC 4791 section 9.9
    /// states stands on its own here.
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

    /// A zoned window carries the slack; the generation window carries the override's shift.
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

    /// RFC 4791 section 7.8.3's expansion, instance for instance.
    ///
    /// The expected column is that example's `>> Response <<` for `abcd2.ics`: two components,
    /// the first at `20060103T170000` and the second at `20060104T190000` with
    /// `RECURRENCE-ID:20060104T170000`. The January 2nd instance ends before the range opens and
    /// the January 5th one starts after it closes, and neither is in the response.
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

    /// An expansion is never what the server stored, and says so.
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

    /// The two answers a filter over a whole recurrence set gives at its extremes.
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

    /// An instance an override moved into the window is found by its moved start.
    ///
    /// RFC 4791 section 9.6.6 states the same fact from the other side: an overridden component
    /// impacts a range if its *current* start and end overlap it. The unmoved January 4th
    /// instance runs 17:00Z to 18:00Z and does not reach a range opening at 18:00Z; the moved one
    /// runs 19:00Z to 20:00Z and does.
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

    /// An override reaching further than the slack is why generation is widened at all.
    ///
    /// The moved instance's cadence key is five days past the window, so only the widening
    /// `ical_recur::generation_window` applies reaches it, and an exclusion removes the ordinary
    /// instance of that day so that the answer can only have come from the moved one.
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

    /// A component with no rule still has the one instance RFC 5545 section 3.8.5 gives it.
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

    /// An `EXDATE` on the only instance a ruleless component has removes it.
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

    /// The answer this crate exists for: a floating value with no zone is not "no match".
    ///
    /// RFC 4791 section 9.9 compares a floating value in the zone the query's `CALDAV:timezone`
    /// stated and section 9.5 permits a query to state none, so there is no timeline to make the
    /// comparison on. `docs/adr/0003` forbids inventing one.
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

    /// A `TZID` no supplied source recognizes is the other half of the same refusal.
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

    /// A search that ran out of budget is undecided, and never a resource that does not match.
    #[test]
    fn a_search_stopped_at_its_budget_is_undecidable() {
        let rule = daily(10_000);
        let source = eastern();
        // Two octets of budget: the third candidate the walk generates is refused, long before
        // the walk reaches a window four years past `DTSTART`.
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

    /// A ledger that had already latched answers before starting a search it cannot finish.
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

    /// A range naming no instant a calendar can express matches nothing, and refuses nothing.
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

    /// Every reason fits the subject a diagnostic carries, so none of them arrives cut in half.
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

    /// The manifest names the passages this file transcribes, and names each of them once.
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
