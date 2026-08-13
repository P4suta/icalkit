// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV filter evaluation: does this calendar resource match this `calendar-query`.
//!
//! Specification: RFC 4791, "Calendaring Extensions to WebDAV (CalDAV)"
//! <https://www.rfc-editor.org/rfc/rfc4791>, sections 7.5, 7.10, 9.6, 9.7 and 9.9.
//!
//! `ical-dav` represents a filter, refuses one that contradicts itself, and hands it back. It
//! does not evaluate one, and it cannot: deciding whether a recurring event has an instance
//! inside a `time-range` needs expansion from `ical-recur` and resolution from `ical-tz`, and
//! `docs/adr/0004`'s spine gives `ical-dav` neither. This crate is where the four meet. It takes
//! `ical-dav`'s filter *values*, a calendar parsed by `ical-core`, a `ZoneSource` the caller
//! supplies, and answers whether the resource matches — so `ical-dav` gains no dependency and
//! the spine is not inverted (`docs/adr/0012`).
//!
//! # The answer has three values
//!
//! A floating `DTSTART` has no place on the timeline until something says which zone to read it
//! in, and `docs/adr/0003` forbids inventing one. When the query carried no `CALDAV:timezone`
//! and no source recognizes the `TZID`, the comparison has no timeline to be made on, and an
//! evaluator that answered "no match" would report an absence it never established. So
//! [`Match`] distinguishes matched, unmatched and [`Match::Undecided`], the three compose by
//! Kleene's rules, and the third value reaches the caller. Turning it into a two-valued answer
//! is a server's policy and this crate does not have one.
//!
//! # Everything here is bounded
//!
//! Every entry point takes the caller's [`Budget`] — a `Limits` and a `&mut Meter` — like every
//! other hostile-input door in this workspace (`docs/adr/0010`). The filter came off the wire
//! and the resource came out of a store somebody else writes to. A `time-range` over a
//! multi-decade rule is the shape that costs, so expansion runs against the caller's candidate
//! budget and a search that stops at it is [`Undecided::SearchExhausted`] rather than a resource
//! reported as not matching.
//!
//! `docs/adr/0012` fixes a measurement over 5,000 resources that decides whether an
//! expansion-free prefilter is required in front of the filter walk, and states the default if
//! the measurement cannot be run before the shape is needed: the prefilter is an internal step
//! the walk calls, defaulting to "cannot exclude". That is why `prefilter` is a module of this
//! crate from the first landing rather than a rewrite waiting to happen.
//!
//! # One place the round trip is deliberately broken
//!
//! `CALDAV:comp`, `CALDAV:expand` and `CALDAV:limit-recurrence-set` (RFC 4791 sections 9.6.1,
//! 9.6.5 and 9.6.6) all answer with a calendar that is **not** what the server stored. Nothing
//! about those octets says so — they are well-formed iCalendar — so `docs/adr/0001`'s round
//! trip is broken on purpose and the break is carried as a value: [`Selection`] holds the
//! calendar and a [`Reduction`] together, and a reduction reports itself as
//! `DiagnosticCode::QueryCalendarDataReduced`. A caller that writes a reduced calendar back
//! deletes whatever was left out.
//!
//! # Status
//!
//! The vocabulary is landed and frozen; the units below it are in flight. See `ROADMAP.md`
//! (M5).

// `vocabulary` is the frozen module: every type two units both name lives there and nothing
// else does. The eight below it are one unit each, declared here so that no unit has to add a
// module line to a file another unit is also editing, and grouped by the primitive they share
// rather than by RFC section number where the two differ. Each fills its own file and appends
// exactly one `pub use` line to the block at the bottom, in the order this block already has.
mod collate;
mod expand;
mod freebusy;
mod overlap;
mod prefilter;
mod prop;
mod subset;
mod vocabulary;
mod walk;

pub use crate::internal::query::vocabulary::{
    Budget, BusyPeriod, BusyType, Collator, FreeBusyReport, Match, QueryError, Reduction,
    Selection, Undecided, Zones,
};

// Unit re-exports. One line per unit, appended by that unit and by nothing else, in the order
// the module block above already has.
pub use crate::internal::query::collate::COLLATION_SECTIONS;
pub(crate) use crate::internal::query::expand::recurrence_set_contains;
pub use crate::internal::query::expand::{
    EXPANSION_SECTIONS, Expansion, Instance, InstanceSpan, SearchBounds, Series, SeriesClock,
    ZONE_SLACK_SECONDS, expand, overlaps,
};
pub use crate::internal::query::freebusy::{
    BusyAnswer, FREE_BUSY_SECTIONS, Placement, Unplaced, free_busy,
};
pub use crate::internal::query::overlap::OVERLAP_SECTIONS;
pub use crate::internal::query::prefilter::PREFILTER_SECTIONS;
pub use crate::internal::query::prop::PROPERTY_FILTER_SECTIONS;
pub use crate::internal::query::subset::{
    SUBSELECTION_SECTIONS, expand_calendar, limit_freebusy_set, limit_recurrence_set,
    limit_recurrence_set_in_window, select, without_values,
};
pub use crate::internal::query::walk::{COMPONENT_FILTER_SECTIONS, matches};
