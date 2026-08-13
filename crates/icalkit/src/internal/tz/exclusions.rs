// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 6 — an `EXDATE` whose value type does not match its `DTSTART`, which closes agenda
//! item 3.
//!
//! Specification: RFC 5545 section 3.8.5.1, which requires an `EXDATE` to agree with `DTSTART`
//! about `DATE` against `DATE-TIME`, and says nothing about what to do when it does not.
//!
//! The type is declared here with its accessors and its membership test, so the crate root
//! needs no edit when the behavior lands. Owed by this unit and by nothing else:
//!
//! ```text
//! impl ResolvedExclusions {
//!     pub fn read<S: ZoneSource + ?Sized, D: DiagnosticSink + ?Sized>(
//!         series: &ZonedSeries<'_, S>, dtstart_kind: ValueType,
//!         excluded: &[DateTimeValue<'_>], meter: &mut Meter, sink: &mut D,
//!     ) -> Self;
//! }
//! ```
//!
//! The milestone brief spelled that parameter `ValueKind`, which is `ical-recur`'s two-variant
//! narrowing of the same question. This crate is `ical-recur`'s sibling and does not depend on
//! it, so the type is spelled with the name that exists at the common root:
//! [`crate::internal::core::ValueType`], which is what [`DateTimeValue::value_type`] already answers with.
//! Everything that is not [`ValueType::Date`] is read as a date-time, because that is RFC 5545
//! section 3.8.2.4's default value type for `DTSTART` and because the twelve types that can be
//! neither are not a third answer this function could have.
//!
//! The failure being closed is a silent one. A `DATE` exclusion resolved to midnight names an
//! instant a 09:00 series does not have, so it removes nothing at all: the exception the
//! producer wrote disappears, the meeting the user cancelled reappears, and no diagnostic
//! anywhere says a word. `exdate-value-type-mismatch` travels whichever reading the caller
//! states, because a silent no-op is the one outcome that is indefensible. This unit is that
//! code's only emitter, and it emits one other: `exdate-zone-unknown`, for the second silent
//! no-op M2 found here — a `Z`-terminated exclusion on a series whose `TZID` no source
//! recognizes, which needs the zone to be placed where a zoned `DTSTART` beside it does not.
//! Nothing else. A fold, a gap or an exhausted table met while sizing a day is a fact about the
//! *shape* of that day rather than about an occurrence, and unit 5 reports those at the
//! occurrences they actually concern.
//!
//! [`ExclusionReading::WholeDay`] is the reading several clients implement: the date names the
//! day, and every occurrence starting inside that day in the series' own zone is excluded. It
//! is why this closure needs a zone at all — a day is a wall-clock span of 23, 24 or 25 hours
//! depending on whether the zone moved inside it, so the span is computed through the source
//! rather than assumed to be 86,400 seconds.
//!
//! # How a day is sized, which is where the assumption would have hidden
//!
//! A span here is nominal, because what it is tested against is a cadence key and every cadence
//! key is nominal (see [`crate::internal::tz::seam`]). Its two ends are the instants at which the zone's own
//! clock first reads midnight on the named day and on the day after, each asked of the source
//! and each read back onto the nominal timeline. On an ordinary day that is 86,400 nominal
//! seconds and the source has changed nothing. On a day whose *midnight* is in a gap — Cuba and
//! Chile both spring forward at midnight, so `2026-03-08` in `America/Havana` begins at 01:00 —
//! the day begins when the gap closes, and the span is 23 nominal hours; the day before it ends
//! at the same instant and is 25. Consecutive days therefore tile without overlapping, because
//! the boundary between two of them is one computation shared by both.
//!
//! A boundary is not an occurrence, so [`ResolutionPolicy`] is not consulted for it: a day
//! begins at the first instant its clock can read midnight, which is the earlier reading of a
//! fold and the far side of a gap, and there is no defensible second answer for a caller to
//! state. Where the source does not recognize the identifier at all the plain civil day is
//! used, because dropping the exclusion would be the silent no-op this unit exists to refuse.
//!
//! # The asymmetry, stated rather than hidden
//!
//! [`ResolvedExclusions::instants`] is what goes into `RecurrenceInput::new`: sorted, strictly
//! ascending, deduplicated, so ordinary exclusions are still applied inside `ical-recur`'s
//! iterator where `docs/adr/0002` puts them. [`ResolvedExclusions::spans`] is what a whole-day
//! reading produces and `ical-recur` cannot take, because its exclusion list is a slice of
//! instants and this crate does not depend on it; those are applied by the caller through
//! [`ResolvedExclusions::excludes`] on each emitted cadence key.
//!
//! That is a real cost of the sibling relation and it is documented rather than engineered
//! around. It is also bounded: one span per `DATE`-valued `EXDATE`, already charged against
//! `Limits::exdate_entries` by whoever read the property — `RecurrenceInput::new` charges the
//! list it is handed, and charging it a second time here would bill one calendar twice. Two
//! source lookups per span and one per foreign identifier is the whole cost, over a slice whose
//! length that bound already fixed, so the meter is here for the diagnostics a sink refuses.
//!
//! [`ExclusionReading::WholeDay`]: crate::internal::tz::ExclusionReading::WholeDay
//! [`ResolutionPolicy`]: crate::internal::tz::ResolutionPolicy

use alloc::vec::Vec;

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, DiagnosticSink,
    Instant, Meter, Severity, UtcOffset, ValueType, report_diagnostic,
};

use crate::internal::tz::answer::{LocalResolution, ZoneSource};
use crate::internal::tz::seam::{ExclusionReading, LocalInterval, nominal};
use crate::internal::tz::series::ZonedSeries;

/// What one series' `EXDATE` properties exclude, in the two shapes they can take.
///
/// Built by [`ResolvedExclusions::read`]; the accessors and the membership test are separate
/// from it because two units read them and only one writes them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedExclusions {
    /// The instants excluded outright, strictly ascending.
    instants: Vec<Instant>,
    /// The nominal spans excluded whole, ascending by start and non-overlapping.
    spans: Vec<LocalInterval>,
    /// The real instants no zone could place on this series' timeline, ascending.
    unplaced: Vec<Instant>,
}

impl ResolvedExclusions {
    /// Exclusions that exclude nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            instants: Vec::new(),
            spans: Vec::new(),
            unplaced: Vec::new(),
        }
    }

    /// What `excluded` removes from `series`, with every value-type mismatch reported.
    ///
    /// `dtstart_kind` is the value type `DTSTART` was written under, and the only thing the
    /// specification requires each entry of `excluded` to agree with. Where one does not, the
    /// entry is still read — under [`ExclusionReading`] when it is a `DATE` against a date-time
    /// series, and literally in the other direction — and `exdate-value-type-mismatch` travels
    /// beside it.
    ///
    /// The result's instants are sorted, strictly ascending and deduplicated, because
    /// `ical_recur::RecurrenceInput::new` refuses a list that is not and a caller sorting them
    /// again downstream would be paying twice for a promise made here.
    ///
    /// [`ExclusionReading`]: crate::internal::tz::ExclusionReading
    #[must_use]
    pub fn read<S, D>(
        series: &ZonedSeries<'_, S>,
        dtstart_kind: ValueType,
        excluded: &[DateTimeValue<'_>],
        meter: &mut Meter,
        sink: &mut D,
    ) -> Self
    where
        S: ZoneSource + ?Sized,
        D: DiagnosticSink + ?Sized,
    {
        let mut resolved = Self::empty();
        let dtstart_dated = matches!(dtstart_kind, ValueType::Date);
        for value in excluded {
            resolved.admit(series, dtstart_dated, *value, meter, sink);
        }
        // Sorted here rather than as each value arrives, because an `EXDATE` list is written in
        // whatever order a client emitted it and a stable insertion would be a linear scan per
        // entry over a list a bound already sized.
        resolved.instants.sort_unstable();
        resolved.instants.dedup();
        resolved.spans.sort_unstable();
        resolved.spans.dedup();
        resolved.unplaced.sort_unstable();
        resolved.unplaced.dedup();
        resolved
    }

    /// The instants excluded outright, strictly ascending.
    ///
    /// What `ical_recur::RecurrenceInput::new` takes, which is why the ordering is a promise
    /// rather than a convenience: that constructor refuses a list that is not.
    #[must_use]
    pub fn instants(&self) -> &[Instant] {
        &self.instants
    }

    /// The spans excluded whole, ascending by start.
    #[must_use]
    pub fn spans(&self) -> &[LocalInterval] {
        &self.spans
    }

    /// Whether `key` falls in one of the spans.
    ///
    /// Only the spans. The instants are applied inside `ical-recur`'s own merge, and applying
    /// them a second time here would be a filter the caller could accidentally rely on instead.
    #[must_use]
    pub fn excludes(&self, key: Instant) -> bool {
        self.spans.iter().any(|span| span.contains(key))
    }

    /// The real instants no zone could place, ascending.
    ///
    /// A `Z`-terminated `EXDATE` on a series whose `TZID` no source recognizes. It names a real
    /// instant and the cadence keys it would have to match are on the series' own wall clock,
    /// so nothing here can turn one into the other — but the exception is what the producer
    /// wrote, and a caller that later acquires the zone can still apply it.
    /// `exdate-zone-unknown` travels beside every entry.
    #[must_use]
    pub fn unplaced(&self) -> &[Instant] {
        &self.unplaced
    }

    /// Whether nothing is excluded at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instants.is_empty() && self.spans.is_empty() && self.unplaced.is_empty()
    }

    /// Take one `EXDATE` value, reporting it when its value type is not `DTSTART`'s.
    ///
    /// Every path adds something. A value that agrees becomes an instant, and a value that does
    /// not becomes either an instant or a span, so no branch here can be the no-op the whole
    /// unit is arranged against.
    fn admit<S, D>(
        &mut self,
        series: &ZonedSeries<'_, S>,
        dtstart_dated: bool,
        value: DateTimeValue<'_>,
        meter: &mut Meter,
        sink: &mut D,
    ) where
        S: ZoneSource + ?Sized,
        D: DiagnosticSink + ?Sized,
    {
        // Two conditions answer `None` here: a wall clock outside the years RFC 5545 section
        // 3.3.4 can write, which nothing read out of a file is, and a `Z`-terminated value on a
        // series whose identifier the source does not recognize, where there is no offset to
        // project it through.
        //
        // The second used to be justified here by "a series whose `DTSTART` carries the same
        // `Z` has no anchor either, so there is no expansion for the exception to have gone
        // missing from" — which is exactly the case that does not arise. A zoned `DTSTART` is a
        // wall clock and consults no zone, so `DTSTART;TZID=Customized Time Zone:20260301T090000`
        // anchors and expands with nothing defining that identifier anywhere, and the
        // `Z`-terminated `EXDATE` beside it was the one value that needed the zone. The
        // exception vanished and the meeting the user cancelled came back.
        //
        // Nothing here can place it — the instant it names is real and the keys it would be
        // compared against are on the series' own wall clock, and only the zone converts
        // between the two. So it is kept as what it is, reachable through
        // `ResolvedExclusions::unplaced`, and reported.
        let Some(point) = project(series, value) else {
            self.keep_unplaced(value, meter, sink);
            return;
        };
        if matches!(value, DateTimeValue::Date(_)) == dtstart_dated {
            self.instants.push(point);
            return;
        }
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(
                DiagnosticCode::ExdateValueTypeMismatch,
                Severity::Violation,
                point,
            ),
        );
        match series.policy().exclusions() {
            ExclusionReading::Instantaneous => self.instants.push(point),
            // A day that cannot be sized — the last day this calendar can write, or a day the
            // zone never had — falls back to the instant, because a span nobody could compute
            // must not become a value nobody excluded.
            ExclusionReading::WholeDay => match whole_day(series, value.date()) {
                Some(span) => self.spans.push(span),
                None => self.instants.push(point),
            },
        }
    }

    /// Keep an exclusion no zone could place, and say so.
    ///
    /// Only a `Z`-terminated value reaches this with anything to keep: it names a real instant
    /// whatever the zone does, and that instant is what a caller holding its own zone data can
    /// still act on. A wall clock the calendar cannot express names nothing at all, and there
    /// is no value to carry and no instant for a diagnostic to point at.
    fn keep_unplaced<D>(&mut self, value: DateTimeValue<'_>, meter: &mut Meter, sink: &mut D)
    where
        D: DiagnosticSink + ?Sized,
    {
        let DateTimeValue::Utc(stamp) = value else {
            return;
        };
        let Some(named) = stamp.at_offset(UtcOffset::UTC) else {
            return;
        };
        self.unplaced.push(named);
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(
                DiagnosticCode::ExdateZoneUnknown,
                Severity::Violation,
                named,
            ),
        );
    }
}

/// The nominal instant one `EXDATE` value names for `series`.
///
/// The four shapes are four different questions. A `DATE` and a floating date-time are wall
/// clocks and are already nominal; a `Z` value names a real instant and has to be converted
/// through the zone's offset at it, which is the step [`crate::internal::tz::seam`] calls the one most easily
/// forgotten; and a zoned value is nominal exactly when its identifier is the series' own.
fn project<S>(series: &ZonedSeries<'_, S>, value: DateTimeValue<'_>) -> Option<Instant>
where
    S: ZoneSource + ?Sized,
{
    match value {
        DateTimeValue::Date(date) => nominal(CivilDateTime::new(date, CivilTime::MIDNIGHT)),
        DateTimeValue::Local(stamp) => nominal(stamp),
        // Not `nominal`, though the arithmetic is the same: this is a real UTC instant being
        // named, and writing it as the projection would record the wrong reason.
        DateTimeValue::Utc(stamp) => series.to_nominal(stamp.at_offset(UtcOffset::UTC)?),
        DateTimeValue::Zoned { stamp, tzid } => project_zoned(series, stamp, tzid),
    }
}

/// The nominal instant a zoned `EXDATE` names, which usually needs no zone at all.
///
/// An `EXDATE` carrying the series' own identifier is already on the series' wall clock, so the
/// projection is [`nominal`] and no lookup happens. A different identifier is a different clock:
/// RFC 5545 does not forbid one, clients that copy an exception between calendars produce them,
/// and reading its fields as though they were the series' own would remove an occurrence an hour
/// away from the one the producer named. So it is resolved in its own zone and projected from
/// the instant that names — and where either zone is one the source has never heard of, the wall
/// clock is read as written rather than dropped, which is the reading a client that ignores
/// `TZID` performs and is at least a reading. A `Z` value has no such fallback in [`project`],
/// because reading UTC fields as a wall clock is not something any client does on purpose.
fn project_zoned<S>(
    series: &ZonedSeries<'_, S>,
    stamp: CivilDateTime,
    tzid: &[u8],
) -> Option<Instant>
where
    S: ZoneSource + ?Sized,
{
    let foreign = core::str::from_utf8(tzid)
        .ok()
        .filter(|name| *name != series.tzid());
    let Some(name) = foreign else {
        return nominal(stamp);
    };
    let policy = series.policy();
    series
        .source()
        .resolve(name, stamp)
        .and_then(|answer| answer.resolution.pick(policy.gaps(), policy.folds()))
        .and_then(|real| series.to_nominal(real))
        .or_else(|| nominal(stamp))
}

/// The nominal span the named day covers in the series' zone.
///
/// `None` in two cases, and a span of no width in neither. The day after the last day RFC 5545
/// section 3.3.4 can write has no midnight to end at. And a day the zone never had — Samoa
/// skipped 2011-12-30 outright when it crossed the date line — opens and closes at the same
/// instant, which [`LocalInterval::new`] refuses for the reason it refuses any empty span.
fn whole_day<S>(series: &ZonedSeries<'_, S>, date: CivilDate) -> Option<LocalInterval>
where
    S: ZoneSource + ?Sized,
{
    let start = day_boundary(series, date)?;
    let end = day_boundary(series, date.checked_add_days(1)?)?;
    LocalInterval::new(start, end)
}

/// The nominal instant at which the zone's clock first reads midnight on `date`.
///
/// Shared by the day it opens and the day it closes, which is what makes consecutive whole-day
/// spans tile. The policy is deliberately not consulted: a fold takes its earlier reading and a
/// gap takes the instant it closed, because a day starts the first moment it can and that is one
/// answer rather than a choice. An identifier the source does not know leaves the plain civil
/// midnight, which is the only reading available when nothing knows the zone.
fn day_boundary<S>(series: &ZonedSeries<'_, S>, date: CivilDate) -> Option<Instant>
where
    S: ZoneSource + ?Sized,
{
    let midnight = CivilDateTime::new(date, CivilTime::MIDNIGHT);
    let plain = nominal(midnight);
    let Some(answer) = series.source().resolve(series.tzid(), midnight) else {
        return plain;
    };
    let real = match answer.resolution {
        LocalResolution::Unique { reading } => reading.instant,
        LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
        LocalResolution::Nonexistent { gap_end, .. } => gap_end,
        // A source that recognizes the zone and holds no transition for it says nothing about
        // where this day begins, which leaves the plain civil midnight — the same answer as a
        // source that does not know the identifier, and for the same reason.
        LocalResolution::Undetermined => return plain,
    };
    series.to_nominal(real).or(plain)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, Instant,
        Limits, Meter, UtcOffset, ValueType,
    };

    use super::ResolvedExclusions;
    use crate::internal::tz::answer::{
        AnswerBasis, FoldPolicy, LocalResolution, OffsetAnswer, Reading, ZoneAnswer,
        ZoneProvenance, ZoneSource,
    };
    use crate::internal::tz::seam::{ExclusionReading, LocalInterval, ResolutionPolicy, nominal};
    use crate::internal::tz::series::ZonedSeries;

    /// One transition of a real zone: when it happened and what the zone ran at afterwards.
    ///
    /// A `VTIMEZONE`'s `RDATE` form in miniature, which is the form that runs out.
    #[derive(Clone, Copy, Debug)]
    struct Shift {
        /// The instant the zone moved, in seconds from the Unix epoch.
        at: i64,
        /// Seconds east of UTC from then on.
        east: i32,
        /// Whether the observance from then on is the zone's daylight one.
        daylight: bool,
    }

    /// A zone stated as the transitions a real government actually decreed.
    ///
    /// Written here rather than reached for from unit 3, for the reason `answer.rs`'s own test
    /// source gives: what is under test is this unit, and a test that leaned on another unit's
    /// file would be testing that unit too. Every instant below is a date and time from the tz
    /// database's rules for the named zone, converted by hand, and no expectation in this module
    /// is taken from what this crate returns.
    #[derive(Clone, Copy, Debug)]
    struct Zone {
        /// The identifier this zone answers to, compared by exact bytes.
        tzid: &'static str,
        /// Seconds east of UTC before the first transition.
        east: i32,
        /// The transitions, strictly ascending.
        shifts: &'static [Shift],
        /// The last date the table covers, `None` when its rules run on.
        known_through: Option<(u16, u8, u8)>,
    }

    /// The offset and observance in force, with no claim about how far the table reached.
    #[derive(Clone, Copy, Debug)]
    struct State {
        /// Seconds east of UTC.
        east: i32,
        /// Whether the observance is the zone's daylight one.
        daylight: bool,
    }

    impl Zone {
        /// What the zone was running at `at`.
        fn state_at(self, at: Instant) -> State {
            let mut state = State {
                east: self.east,
                daylight: false,
            };
            for shift in self.shifts {
                if at.unix_seconds() >= shift.at {
                    state = State {
                        east: shift.east,
                        daylight: shift.daylight,
                    };
                }
            }
            state
        }

        /// Whether `at` is past everything this table actually knows, and how far it knew.
        fn basis_at(self, at: Instant) -> AnswerBasis {
            let last = self.shifts.last().map_or(i64::MIN, |shift| shift.at);
            match self.known_through {
                Some((year, month, day)) if at.unix_seconds() >= last => {
                    let end = CivilDate::from_ymd(year, month, day).unwrap();
                    AnswerBasis::BeyondKnownTransitions(end)
                },
                _ => AnswerBasis::Computed,
            }
        }

        /// Every reading of `local` that the zone agrees with, ascending.
        ///
        /// The standard test: a candidate offset produces an instant, and the instant is a real
        /// reading exactly when the zone was running that offset there. Two survive a fold, none
        /// survive a gap, one survives every other hour of the year.
        fn readings_of(self, local: CivilDateTime) -> Vec<Reading> {
            let mut found: Vec<Reading> = Vec::new();
            let candidates =
                core::iter::once(self.east).chain(self.shifts.iter().map(|shift| shift.east));
            for east in candidates {
                let offset = UtcOffset::from_seconds(east).unwrap();
                let Some(instant) = local.at_offset(offset) else {
                    continue;
                };
                let state = self.state_at(instant);
                if state.east == east && !found.iter().any(|kept| kept.instant == instant) {
                    found.push(Reading::new(instant, offset, state.daylight));
                }
            }
            found.sort_unstable();
            found
        }

        /// The gap `local` fell into, as the resolution type spells one.
        ///
        /// A wall clock is inside a spring-forward gap exactly when reading it with the offset
        /// in force beforehand lands within the transition's own width, which is what keeps a
        /// table of several years from answering with the wrong year's transition.
        fn gap_at(self, local: CivilDateTime) -> Option<LocalResolution> {
            let mut before = self.east;
            for shift in self.shifts {
                let earlier = UtcOffset::from_seconds(before).unwrap();
                let later = UtcOffset::from_seconds(shift.east).unwrap();
                let width = i64::from(shift.east.checked_sub(before).unwrap());
                let closed = Instant::from_unix_seconds(shift.at);
                let read_ahead = local.at_offset(earlier);
                let inside = width > 0
                    && read_ahead.is_some_and(|at| {
                        at >= closed && at < closed.checked_add_seconds(width).unwrap()
                    });
                if inside {
                    return Some(LocalResolution::Nonexistent {
                        gap_start: closed.checked_add_seconds(-1).unwrap(),
                        gap_end: closed,
                        offset_before: earlier,
                        offset_after: later,
                        shifted: read_ahead.unwrap(),
                    });
                }
                before = shift.east;
            }
            None
        }
    }

    /// The zones one test calendar can name.
    #[derive(Clone, Copy, Debug)]
    struct Zones(&'static [Zone]);

    impl Zones {
        /// The zone `tzid` names, by exact bytes and with no alias table.
        fn find(self, tzid: &str) -> Option<Zone> {
            self.0.iter().copied().find(|zone| zone.tzid == tzid)
        }
    }

    impl ZoneSource for Zones {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            let zone = self.find(tzid)?;
            let readings = zone.readings_of(local);
            let resolution = match (readings.first(), readings.get(1)) {
                (Some(only), None) => LocalResolution::Unique { reading: *only },
                (Some(first), Some(second)) => LocalResolution::Ambiguous {
                    earlier: *first,
                    later: *second,
                },
                _ => zone.gap_at(local)?,
            };
            let seen = match resolution {
                LocalResolution::Unique { reading } => reading.instant,
                LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
                LocalResolution::Nonexistent { gap_end, .. } => gap_end,
                // This fixture always holds transitions, so it never answers `Undetermined`;
                // the arm is what `#[non_exhaustive]` asks of a match on another unit's enum.
                LocalResolution::Undetermined => return None,
            };
            Some(ZoneAnswer::new(
                resolution,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis_at(seen),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            let zone = self.find(tzid)?;
            let state = zone.state_at(instant);
            Some(OffsetAnswer::new(
                UtcOffset::from_seconds(state.east).unwrap(),
                state.daylight,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis_at(instant),
            ))
        }
    }

    /// `Europe/Berlin`: CET is +01:00, CEST is +02:00, and the moves are at 01:00 UTC on the
    /// last Sunday of March and of October — 2026-03-29 and 2026-10-25.
    const BERLIN: &[Shift] = &[
        Shift {
            at: 1_774_746_000,
            east: 7200,
            daylight: true,
        },
        Shift {
            at: 1_792_890_000,
            east: 3600,
            daylight: false,
        },
    ];

    /// `America/New_York`: EST is -05:00, EDT is -04:00, and the moves are at 02:00 local on the
    /// second Sunday of March and the first Sunday of November — 2026-03-08 and 2026-11-01.
    const NEW_YORK: &[Shift] = &[
        Shift {
            at: 1_772_953_200,
            east: -14400,
            daylight: true,
        },
        Shift {
            at: 1_793_512_800,
            east: -18000,
            daylight: false,
        },
    ];

    /// `America/Havana`: CST is -05:00, CDT is -04:00, and Cuba moves at 00:00 standard time —
    /// so on 2026-03-08 the clock goes straight from 00:00 to 01:00 and midnight never happens,
    /// and on 2026-11-01 it goes from 01:00 back to 00:00 and midnight happens twice.
    const HAVANA: &[Shift] = &[
        Shift {
            at: 1_772_946_000,
            east: -14400,
            daylight: true,
        },
        Shift {
            at: 1_793_509_200,
            east: -18000,
            daylight: false,
        },
    ];

    /// `Pacific/Apia`: Samoa crossed the date line at 24:00 on 2011-12-29, going from -10:00
    /// straight to +14:00, so 2011-12-30 is a date that never happened there at all.
    const APIA: &[Shift] = &[Shift {
        at: 1_325_239_200,
        east: 50400,
        daylight: true,
    }];

    /// A `VTIMEZONE` written as a finite `RDATE` table for Berlin's rules, whose last entry is
    /// the move of 2029-10-28 and which therefore says nothing at all about 2035.
    const THROUGH_2029: &[Shift] = &[Shift {
        at: 1_887_843_600,
        east: 3600,
        daylight: false,
    }];

    /// The calendar's zones, including two identifiers no tz database has.
    static SOURCE: Zones = Zones(&[
        Zone {
            tzid: "Europe/Berlin",
            east: 3600,
            shifts: BERLIN,
            known_through: None,
        },
        Zone {
            tzid: "America/New_York",
            east: -18000,
            shifts: NEW_YORK,
            known_through: None,
        },
        Zone {
            tzid: "America/Havana",
            east: -18000,
            shifts: HAVANA,
            known_through: None,
        },
        // Exchange writes this for Berlin, and a source with an alias table is entitled to
        // answer it. Nothing in the crate parsed the name to get here.
        Zone {
            tzid: "W. Europe Standard Time",
            east: 3600,
            shifts: BERLIN,
            known_through: None,
        },
        Zone {
            tzid: "Pacific/Apia",
            east: -36000,
            shifts: APIA,
            known_through: None,
        },
        Zone {
            tzid: "/mozilla.org/20050126_1/Europe/Berlin",
            east: 3600,
            shifts: THROUGH_2029,
            known_through: Some((2029, 10, 28)),
        },
    ]);

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    /// The nominal instant a wall clock spells, which is how every expectation here is written.
    fn key(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        nominal(stamp(year, month, day, hour, minute)).unwrap()
    }

    fn series(tzid: &'static str, reading: ExclusionReading) -> ZonedSeries<'static, Zones> {
        ZonedSeries::new(
            &SOURCE,
            tzid,
            ResolutionPolicy::DEFAULT.with_exclusions(reading),
        )
    }

    fn meter() -> Meter {
        Meter::new(Limits::DEFAULT)
    }

    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().copied().map(Diagnostic::code).collect()
    }

    fn seconds(span: LocalInterval) -> i64 {
        span.start().checked_seconds_until(span.end()).unwrap()
    }

    /// Agenda item 3, in the shape that made it worth closing: the exception does something and
    /// says so, under either reading, rather than quietly doing nothing.
    #[test]
    fn a_date_exclusion_against_a_date_time_series_is_never_a_silent_no_op() {
        let excluded = [DateTimeValue::Date(date(2026, 3, 29))];
        for reading in [ExclusionReading::Instantaneous, ExclusionReading::WholeDay] {
            let zoned = series("Europe/Berlin", reading);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &excluded,
                &mut ledger,
                &mut reported,
            );
            assert!(!resolved.is_empty(), "{reading:?} removed nothing at all");
            assert_eq!(
                codes(&reported),
                [DiagnosticCode::ExdateValueTypeMismatch],
                "{reading:?}"
            );
            assert_eq!(reported[0].instant(), Some(key(2026, 3, 29, 0, 0)));
        }

        let instantaneous = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let literal = ResolvedExclusions::read(
            &instantaneous,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(literal.instants(), [key(2026, 3, 29, 0, 0)]);
        assert!(literal.spans().is_empty());
        // Which is exactly the failure: a 09:00 series has no key at midnight, so the literal
        // reading removes nothing and only the diagnostic survives to say the producer tried.
        assert!(!literal.excludes(key(2026, 3, 29, 9, 0)));

        let whole = series("Europe/Berlin", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let expanded = ResolvedExclusions::read(
            &whole,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert!(expanded.instants().is_empty());
        assert!(expanded.excludes(key(2026, 3, 29, 9, 0)));
        assert!(!expanded.excludes(key(2026, 3, 30, 9, 0)));
        assert!(!expanded.excludes(key(2026, 3, 28, 9, 0)));
    }

    /// The day is sized through the source, so Cuba's midnight spring-forward makes one day 23
    /// hours long and the day before it 25 — and the two still tile.
    ///
    /// Every number here is Cuba's rule, not this crate's: `Sun>=8 Mar 0:00s` moves the clock
    /// from 00:00 to 01:00, so 2026-03-08 begins at 01:00 and 2026-03-07 ends there.
    #[test]
    fn a_whole_day_span_is_the_zone_s_own_day_and_not_a_fixed_86_400_seconds() {
        let cases = [
            (
                date(2026, 3, 7),
                key(2026, 3, 7, 0, 0),
                key(2026, 3, 8, 1, 0),
                90_000,
            ),
            (
                date(2026, 3, 8),
                key(2026, 3, 8, 1, 0),
                key(2026, 3, 9, 0, 0),
                82_800,
            ),
            (
                date(2026, 11, 1),
                key(2026, 11, 1, 0, 0),
                key(2026, 11, 2, 0, 0),
                86_400,
            ),
            (
                date(2026, 6, 15),
                key(2026, 6, 15, 0, 0),
                key(2026, 6, 16, 0, 0),
                86_400,
            ),
        ];
        for (day, start, end, length) in cases {
            let zoned = series("America/Havana", ExclusionReading::WholeDay);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &[DateTimeValue::Date(day)],
                &mut ledger,
                &mut reported,
            );
            let span = resolved.spans()[0];
            assert_eq!((span.start(), span.end()), (start, end), "{day:?}");
            assert_eq!(seconds(span), length, "{day:?}");
        }

        // The gap swallows the first hour of the day, so no key inside it belongs to the day
        // before — the two spans meet at one instant and never overlap.
        let zoned = series("America/Havana", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let pair = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[
                DateTimeValue::Date(date(2026, 3, 8)),
                DateTimeValue::Date(date(2026, 3, 7)),
            ],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(pair.spans().len(), 2);
        assert_eq!(pair.spans()[0].end(), pair.spans()[1].start());
        assert!(
            pair.excludes(key(2026, 3, 8, 0, 30)),
            "a key in the gap is the earlier day's"
        );
        assert!(pair.excludes(key(2026, 3, 8, 9, 0)));
        assert!(!pair.excludes(key(2026, 3, 9, 0, 0)));
    }

    /// A date the zone never had cannot become a span of no width, because a span that excludes
    /// nothing and a span nobody asked for would then be the same value.
    ///
    /// Samoa's 2011-12-30 is the case: the day opens and closes at one instant, so the whole-day
    /// reading has nothing to expand and falls back to the instant, with the mismatch still
    /// reported. Nothing is excluded either way, and nothing needed to be.
    #[test]
    fn a_date_the_zone_skipped_entirely_falls_back_to_the_instant() {
        let zoned = series("Pacific/Apia", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2011, 12, 30))],
            &mut ledger,
            &mut reported,
        );
        assert!(
            resolved.spans().is_empty(),
            "a day of no width is not a span"
        );
        assert_eq!(resolved.instants(), [key(2011, 12, 30, 0, 0)]);
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        // The day either side of it is ordinary, and 2011-12-31 begins at the instant Samoa
        // arrived on the other side of the line.
        let mut ledger = meter();
        let mut reported = Vec::new();
        let after = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2011, 12, 31))],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(seconds(after.spans()[0]), 86_400);
        assert!(after.excludes(key(2011, 12, 31, 9, 0)));
    }

    /// A fall-back day is 25 real hours and 24 nominal ones, which is the distinction the span
    /// exists to keep straight: the wall clock reads every hour of 2026-11-01 once on paper and
    /// 00:00 through 01:00 twice in fact.
    #[test]
    fn the_real_day_and_the_nominal_span_are_different_lengths_on_purpose() {
        let midnight = SOURCE
            .resolve("America/Havana", stamp(2026, 11, 1, 0, 0))
            .unwrap();
        assert!(midnight.resolution.is_ambiguous());
        let next = SOURCE
            .resolve("America/Havana", stamp(2026, 11, 2, 0, 0))
            .unwrap();
        let opened = midnight.resolution.earliest().unwrap();
        let closed = next.resolution.earliest().unwrap();
        assert_eq!(
            opened.checked_seconds_until(closed).unwrap(),
            90_000,
            "Cuba's 2026-11-01 really is twenty-five hours long"
        );

        let sprang = SOURCE
            .resolve("America/Havana", stamp(2026, 3, 8, 0, 0))
            .unwrap();
        assert!(
            sprang.resolution.is_nonexistent(),
            "midnight on 2026-03-08 is a local time Cuba never showed"
        );
    }

    /// A value type that agrees is not diagnosed, and each of the four shapes crosses the seam
    /// the way `crate::internal::tz::seam` says it does.
    #[test]
    fn an_agreeing_exclusion_crosses_the_seam_and_is_reported_by_nothing() {
        // 09:00 Berlin on 2026-07-01 is 07:00Z, because CEST is two hours east. All four of
        // these name that occurrence, and all four must land on one nominal key.
        let cases = [
            DateTimeValue::Local(stamp(2026, 7, 1, 9, 0)),
            DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 9, 0),
                tzid: b"Europe/Berlin",
            },
            DateTimeValue::Utc(stamp(2026, 7, 1, 7, 0)),
            DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 3, 0),
                tzid: b"America/New_York",
            },
        ];
        for value in cases {
            let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &[value],
                &mut ledger,
                &mut reported,
            );
            assert_eq!(
                resolved.instants(),
                [key(2026, 7, 1, 9, 0)],
                "{value:?} did not land on the series' own 09:00"
            );
            assert!(
                reported.is_empty(),
                "{value:?} was reported and should not be"
            );
        }

        // The bug the conversion prevents, stated as the wrong answer it would give: reading
        // 07:00Z as though it were already nominal excludes 07:00 local, two hours early.
        assert_ne!(key(2026, 7, 1, 7, 0), key(2026, 7, 1, 9, 0));
    }

    /// The mismatch has two directions and the code covers both, so a `DATE-TIME` exclusion
    /// against an all-day series is reported too rather than only the direction agenda item 3
    /// named.
    #[test]
    fn a_date_time_exclusion_against_an_all_day_series_is_reported_as_well() {
        let excluded = [DateTimeValue::Zoned {
            stamp: stamp(2026, 7, 1, 9, 0),
            tzid: b"Europe/Berlin",
        }];
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let literal = ResolvedExclusions::read(
            &zoned,
            ValueType::Date,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(literal.instants(), [key(2026, 7, 1, 9, 0)]);
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        let whole = series("Europe/Berlin", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let expanded = ResolvedExclusions::read(
            &whole,
            ValueType::Date,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert!(
            expanded.excludes(key(2026, 7, 1, 0, 0)),
            "an all-day series' key is midnight, and the whole day covers it"
        );
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        // An agreeing pair on the same series is silent, which is what makes the two above a
        // report about the file rather than about the reading.
        let mut ledger = meter();
        let mut reported = Vec::new();
        let agreed = ResolvedExclusions::read(
            &zoned,
            ValueType::Date,
            &[DateTimeValue::Date(date(2026, 7, 1))],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(agreed.instants(), [key(2026, 7, 1, 0, 0)]);
        assert!(reported.is_empty());
    }

    /// `ical_recur::RecurrenceInput::new` refuses a list that is not strictly ascending, so this
    /// is a promise rather than a tidiness: an unsorted or repeated `EXDATE` list is ordinary.
    #[test]
    fn the_instants_come_out_strictly_ascending_and_deduplicated() {
        let excluded = [
            DateTimeValue::Local(stamp(2026, 7, 3, 9, 0)),
            DateTimeValue::Local(stamp(2026, 7, 1, 9, 0)),
            DateTimeValue::Utc(stamp(2026, 7, 1, 7, 0)),
            DateTimeValue::Local(stamp(2026, 7, 2, 9, 0)),
        ];
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(
            resolved.instants(),
            [
                key(2026, 7, 1, 9, 0),
                key(2026, 7, 2, 9, 0),
                key(2026, 7, 3, 9, 0),
            ],
            "the UTC entry names the same occurrence as the floating one and is folded into it"
        );
        assert!(resolved.instants().windows(2).all(|pair| pair[0] < pair[1]));

        let empty =
            ResolvedExclusions::read(&zoned, ValueType::DateTime, &[], &mut ledger, &mut reported);
        assert!(empty.is_empty());
        assert!(!empty.excludes(key(2026, 7, 1, 9, 0)));
    }

    /// A table that ran out still answers, and the exclusion still lands: `BeyondKnownTransitions`
    /// is a fact about the answer and never a reason to drop the producer's exception.
    #[test]
    fn an_exclusion_past_the_end_of_a_finite_table_is_still_applied() {
        let zoned = series(
            "/mozilla.org/20050126_1/Europe/Berlin",
            ExclusionReading::WholeDay,
        );
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2035, 3, 15))],
            &mut ledger,
            &mut reported,
        );
        let span = resolved.spans()[0];
        assert_eq!(span.start(), key(2035, 3, 15, 0, 0));
        assert_eq!(
            seconds(span),
            86_400,
            "the continued observance has no transition in it"
        );
        assert!(resolved.excludes(key(2035, 3, 15, 9, 0)));
        assert_eq!(
            codes(&reported),
            [DiagnosticCode::ExdateValueTypeMismatch],
            "the exhausted table is unit 5's report at the occurrence, not this unit's here"
        );

        let answer = SOURCE
            .resolve(
                "/mozilla.org/20050126_1/Europe/Berlin",
                stamp(2035, 3, 15, 0, 0),
            )
            .unwrap();
        assert_eq!(
            answer.basis,
            AnswerBasis::BeyondKnownTransitions(date(2029, 10, 28)),
            "the source really is out of data, which is what makes the span above a continuation"
        );
    }

    /// A `TZID` is not an IANA identifier. One the source recognizes is answered, one it does
    /// not is left alone, and neither is parsed, rewritten or guessed at.
    #[test]
    fn an_identifier_that_is_not_a_database_name_is_neither_parsed_nor_refused() {
        let excluded = [DateTimeValue::Date(date(2026, 3, 29))];
        let exchange = series("W. Europe Standard Time", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let answered = ResolvedExclusions::read(
            &exchange,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(answered.spans()[0].start(), key(2026, 3, 29, 0, 0));
        assert_eq!(seconds(answered.spans()[0]), 86_400);
        assert!(answered.excludes(key(2026, 3, 29, 9, 0)));

        let stranger = series("Customized Time Zone", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let unknown = ResolvedExclusions::read(
            &stranger,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(
            (unknown.spans()[0].start(), seconds(unknown.spans()[0])),
            (key(2026, 3, 29, 0, 0), 86_400),
            "nothing knows the zone, so the plain civil day is the only reading left"
        );
        assert!(unknown.excludes(key(2026, 3, 29, 9, 0)));
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        // An exclusion written in a zone nothing knows keeps its own wall clock rather than
        // vanishing, which is the same refusal to drop input stated at the other end.
        let berlin = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let kept = ResolvedExclusions::read(
            &berlin,
            ValueType::DateTime,
            &[DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 9, 0),
                tzid: b"Customized Time Zone",
            }],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(kept.instants(), [key(2026, 7, 1, 9, 0)]);
        assert!(reported.is_empty());
    }

    /// A fold and a gap are values on this path too: an exclusion written in another zone is
    /// collapsed under the caller's stated policy, not under one this unit picked.
    #[test]
    fn an_awkward_hour_in_a_foreign_zone_is_collapsed_under_the_stated_policy() {
        // 2026-11-01T01:30 in New York happens twice: 05:30Z under EDT and 06:30Z under EST.
        // Berlin is one hour east in November, so those are 06:30 and 07:30 on the series' clock.
        let value = [DateTimeValue::Zoned {
            stamp: stamp(2026, 11, 1, 1, 30),
            tzid: b"America/New_York",
        }];
        let cases = [
            (FoldPolicy::Earlier, key(2026, 11, 1, 6, 30)),
            (FoldPolicy::Later, key(2026, 11, 1, 7, 30)),
        ];
        for (folds, expected) in cases {
            let zoned = ZonedSeries::new(
                &SOURCE,
                "Europe/Berlin",
                ResolutionPolicy::DEFAULT.with_folds(folds),
            );
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &value,
                &mut ledger,
                &mut reported,
            );
            assert_eq!(resolved.instants(), [expected], "{folds:?}");
        }

        // 2026-03-08T02:30 in New York happens never. Under the default gap policy the value
        // names no instant there, and the wall clock as written is what is left.
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let sprang = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Zoned {
                stamp: stamp(2026, 3, 8, 2, 30),
                tzid: b"America/New_York",
            }],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(sprang.instants(), [key(2026, 3, 8, 2, 30)]);
    }
}
