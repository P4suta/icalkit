// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Time zones: `VTIMEZONE` interpreted against a source the caller supplies.
//!
//! Specification: RFC 5545 section 3.6.5, the `VTIMEZONE` component
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.6.5>.
//!
//! A calendar carries its own time zone definitions: `STANDARD` and `DAYLIGHT`
//! subcomponents with offsets and transition rules, written down when the file was written.
//! It also carries `TZID` strings that usually, but not always, name an IANA zone — Windows
//! zone names such as `W. Europe Standard Time` and prefixed identifiers such as
//! `/mozilla.org/20050126_1/Europe/Berlin` are both common in the wild.
//!
//! The embedded rules and today's IANA database legitimately disagree. A calendar written
//! in 2018 carries 2018's rules for a zone whose government has since changed them, and
//! which answer is right depends on the question being asked: *what did the organizer mean
//! when they scheduled this* is the embedded `VTIMEZONE`, *what time will this actually
//! happen* is the current database, and *what does the server think* is whatever it was
//! configured with. This crate therefore prefers neither. Resolution goes through a policy
//! the caller states, every result names the source that produced it, and a disagreement
//! about a given instant is a fact the caller can inspect rather than something settled out
//! of sight (see `docs/adr/0003`).
//!
//! No time zone data is bundled and no clock is read. That keeps the crate small and
//! `no_std`, and it means the library never becomes wrong because tzdata moved: it has no
//! opinion about tzdata. The cost is that a caller must supply something, which for most is
//! one line wiring in the database they already depend on.
//!
//! The awkward local times are values here, not errors. When a zone falls back, an hour
//! repeats and a local time has two instants; when it springs forward, an hour does not
//! exist and a local time has none. Real calendars contain events scheduled at 02:30 on a
//! spring-forward morning, and picking one interpretation silently is how a meeting appears
//! to move by an hour for one participant and not another.
//!
//! What this crate owns is resolution, not the types it resolves into. `CivilDate`,
//! `CivilDateTime`, `UtcOffset` and `Instant` belong to `ical-core` and below, because
//! `ical-recur` is a sibling of this crate and `ical-dav` names an instant without depending
//! on it at all; they are re-exported here so a caller still names one crate for one concept
//! (see `docs/adr/0011`). Every operation on them is checked, and no `Duration` carries years
//! or months, because RFC 5545's `DURATION` grammar has no designator for either.
//!
//! # The seam with `ical-recur`
//!
//! One thing has to be read before any zoned series is expanded, because getting it wrong
//! puts every such series an hour out for half the year. `ical-recur` walks periods in civil
//! fields read at UTC and emits cadence keys as instants; it has no zone and is a sibling
//! crate that cannot acquire one. **The timeline it works on is the series' own wall clock
//! projected onto UTC, and not the UTC timeline.** Every instant crossing into it — `DTSTART`,
//! `UNTIL`, each `RDATE`, `EXDATE` and `RECURRENCE-ID` — is projected by [`nominal`], every
//! cadence key coming back is read by [`wall_clock`], and each key is resolved against the
//! zone one at a time, which is the only place a transition can be seen. A daily 09:00 series
//! is then stable on the wall clock because the wall clock is what was generated. The [`seam`]
//! module states the contract in full, including what it means for a `Z`-terminated `UNTIL`
//! and where it and `ical-recur`'s shipped prose disagree.
//!
//! [`seam`]: crate::seam
//!
//! # Feature flags
//!
//! One, `vtimezone`, on by default. It compiles the `VTIMEZONE` half: [`Observance`],
//! [`YearlyRule`], [`TransitionTable`], [`VtimezoneSet`] and the source over a table. With it
//! off the crate is the trait, the answer types, [`Tzid`], the seam and the combinator — the
//! surface a caller needs when the only zone data in play is its own. `ical-core` is a
//! dependency either way, because the shared value types come from there.
//!
//! # Status
//!
//! Landed, and the milestone it belongs to is met. A `VTIMEZONE` is read into a bounded
//! [`TransitionTable`] whose rules are evaluated in closed form — no loop over candidate dates,
//! so a lookup cannot be made to do unbounded work — and that table is a [`ZoneSource`]. Above
//! it: [`CombinedZoneSource`] over two sources that never short-circuits, [`ZonedSeries`] for
//! the seam described above, [`ResolvedExclusions`] for an `EXDATE` list, and
//! [`WallClockShift`] with [`OrphanScan`] for the two override questions `ical-recur` could not
//! answer without a zone.
//!
//! Two things are known rather than hidden. An answer past a table's last transition continues
//! its final observance and says so through [`AnswerBasis::BeyondKnownTransitions`], but this
//! crate has no opinion on what a caller should do about a continuation six years wide as
//! against one a day wide. And [`VtimezoneSet::insert`] charges a zone-count bound that no
//! diagnostic code reports, so a calendar declaring more zones than the caller's policy admits
//! silently keeps the ones that fit; [`ZoneSetError::TooMany`]'s own wording assumes otherwise
//! and one of the two is wrong. `docs/design/ical-tz-api.md` carries the surface and its "What
//! M2 shipped" section the reasoning; `ROADMAP.md` carries the milestone.

#![no_std]

extern crate alloc;

// The four foundation modules — `answer`, `ident`, `model` and `seam` — carry every type two
// units both name, and are frozen. The seven below them are one unit each, declared here so
// that no unit has to add a module line to a file another unit is also editing; each fills its
// own file and appends exactly one `pub use` line to the block at the bottom, in the order this
// block already has. `seam` is the one public module, because the contract it states is
// something a caller reads rather than only something a caller calls.
mod answer;
mod combine;
mod exclusions;
mod ident;
#[cfg(feature = "vtimezone")]
mod model;
mod overrides;
#[cfg(feature = "vtimezone")]
mod reader;
#[cfg(feature = "vtimezone")]
mod resolve;
#[cfg(feature = "vtimezone")]
mod rules;
pub mod seam;
mod series;

// The civil-time vocabulary is re-exported so that a caller names one crate for one concept.
// `docs/adr/0011` puts the types in `ical-core` and the meaning here; a caller resolving a
// `DTSTART` should not have to know which side of that seam a name came from.
pub use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Duration, Instant, Limits, Meter, MonthAddOutcome,
    UtcOffset, Weekday,
};

pub use crate::answer::{
    AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, PolicyOutcome, Reading,
    ZoneAnswer, ZoneProvenance, ZoneSource,
};
// Every type a unit contributes is named here already, with the unit adding only the behavior
// on it. `reader` is the one exception: its free function is a new name, and adding it is the
// single line any unit appends to this block.
pub use crate::combine::{CombinedZoneSource, FixedOffsetSource};
pub use crate::exclusions::ResolvedExclusions;
pub use crate::ident::{Tzid, TzidForm};
#[cfg(feature = "vtimezone")]
pub use crate::model::{
    NthWeek, Observance, ObservanceReader, RuleDay, TransitionTable, VtimezoneSet, YearlyRule,
    ZoneSetError,
};
pub use crate::overrides::{OrphanScan, WallClockShift, extra_widening};
#[cfg(feature = "vtimezone")]
pub use crate::reader::read_calendar_zones;
pub use crate::seam::{
    ExclusionReading, LocalInterval, ResolutionPolicy, UntilReading, nominal, wall_clock,
};
pub use crate::series::ZonedSeries;
