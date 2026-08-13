// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 5 — driving one zoned series across the seam, which closes agenda items 1, 2 and 4.
//!
//! Read [`crate::seam`] first. This unit is the executable form of the contract stated there
//! and may not restate it, reinterpret it, or introduce a second projection beside it.
//!
//! What a series needs and nothing else. [`ZonedSeries::anchor`] projects a `DTSTART`,
//! [`ZonedSeries::to_nominal`] projects any real UTC instant, [`ZonedSeries::project_until`]
//! projects an `UNTIL` under the readings RFC 5545 leaves open, [`ZonedSeries::answer_for`]
//! hands back the whole zone answer for one cadence key, and [`ZonedSeries::actual`] is that
//! answer collapsed to the instant the occurrence happens at.
//!
//! # Agenda item 4, which is the one the milestone turns on
//!
//! `anchor` projects a `DTSTART` onto the nominal timeline: a floating one is already there, a
//! zoned one is its wall clock read through [`nominal`], and a UTC one is converted through the
//! zone's offset at it first. `actual` is the other half — read the cadence key back with
//! [`wall_clock`], resolve it against the zone, collapse it under [`ResolutionPolicy`] — and it
//! is called once per emitted occurrence. That per-occurrence call is what keeps a daily 09:00
//! series at 09:00 across a daylight saving transition instead of an hour out for half the
//! year, and it is what the two-sided test in unit 8 exists to hold.
//!
//! `to_nominal` is the conversion a `Z`-terminated value needs and the step most easily
//! forgotten: a real UTC instant is not on the nominal timeline, and handing one to
//! `ical-recur` unprojected cuts a zoned series off an hour early or late.
//!
//! # Agenda items 1 and 2, which `project_until` closes together
//!
//! A floating `UNTIL` against a zoned or UTC `DTSTART` violates RFC 5545 section 3.3.10 and
//! Google emits it. It is read in `DTSTART`'s own zone — which under the projection means its
//! wall-clock fields are already nominal and need no conversion — and reported on
//! `recurrence-until-not-utc`. M1 found this violation had no code while its value-type sibling
//! did; this milestone added one, and this unit is its only emitter.
//!
//! An `UNTIL` written as a `DATE` against a date-time `DTSTART` is read where
//! [`UntilReading`] says. That is a resolution-time policy and not a fixed reading: midnight is
//! what libical, dateutil and the Google engine do and drops the named day's own instances, end
//! of day is what the person who typed the date meant, and the RFC licenses both.
//! `recurrence-until-value-type-mismatch` still travels from whoever parsed the rule; this unit
//! does not emit it a second time.
//!
//! # Diagnostics
//!
//! `ambiguous-local-time`, `nonexistent-local-time` and `time-zone-coverage-exhausted` are
//! emitted here, at the instant concerned, read off `LocalResolution::diagnostic_code` and
//! `AnswerBasis::diagnostic_code`. This unit emits no code those two mappings do not name,
//! plus `recurrence-until-not-utc`.
//!
//! Which instant "the instant concerned" is, is the instant the zone was asked about, and that
//! is a different value in the two directions. `project_until` asks about the real UTC instant a
//! `Z`-terminated value spells and names that. `actual` asks about a wall clock, and names the
//! cadence key: the key exists in every case and identifies exactly one occurrence, where the
//! resolution names two instants at a fold and none at all in a gap under [`GapPolicy::Skip`].
//!
//! Nothing else is charged to the meter. A resolution is a lookup against a table that was
//! bounded when it was built, so `docs/adr/0010`'s argument is satisfied structurally here and
//! the meter travels for the sake of a refused diagnostic and for nothing else.
//!
//! [`nominal`]: crate::nominal
//! [`wall_clock`]: crate::wall_clock
//! [`ResolutionPolicy`]: crate::ResolutionPolicy
//! [`UntilReading`]: crate::UntilReading
//! [`GapPolicy::Skip`]: crate::GapPolicy::Skip

use core::fmt::{self, Debug, Formatter};

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, DiagnosticSink,
    Instant, Meter, Severity, UtcOffset, report_diagnostic,
};

use crate::answer::{AnswerBasis, ZoneAnswer, ZoneSource};
use crate::seam::{ResolutionPolicy, UntilReading, nominal, wall_clock};

/// The last second of a day, where [`UntilReading::EndOfDay`] reads an `UNTIL` written as a
/// `DATE`.
///
/// 23:59:59 rather than 23:59:60. RFC 5545 section 3.3.12 writes a leap second and `ical-core`
/// folds it onto the second before it, so the two name one instant and only one of them is a
/// time every day has. The `Option` is carried rather than unwrapped because nothing in this
/// crate may panic; it is `Some` for these fields and the constructor is what says so.
///
/// [`UntilReading::EndOfDay`]: crate::UntilReading::EndOfDay
const END_OF_DAY: Option<CivilTime> = CivilTime::from_hms(23, 59, 59);

/// One series, its zone, and the readings the caller stated for it.
///
/// Holds no instant of its own. Everything it answers is derived from the source and the
/// policy, so one of these is built per series and asked once per occurrence.
pub struct ZonedSeries<'a, S: ?Sized> {
    /// Where the zone answers come from.
    source: &'a S,
    /// The identifier, compared by exact bytes and never rewritten.
    tzid: &'a str,
    /// What the caller decided about the readings RFC 5545 leaves open.
    policy: ResolutionPolicy,
}

impl<'a, S: ZoneSource + ?Sized> ZonedSeries<'a, S> {
    /// A series in the zone `tzid` names, resolved through `source` under `policy`.
    #[must_use]
    pub const fn new(source: &'a S, tzid: &'a str, policy: ResolutionPolicy) -> Self {
        Self {
            source,
            tzid,
            policy,
        }
    }

    /// Where the zone answers come from.
    #[must_use]
    pub const fn source(&self) -> &'a S {
        self.source
    }

    /// The identifier, exactly as written.
    #[must_use]
    pub const fn tzid(&self) -> &'a str {
        self.tzid
    }

    /// What the caller decided about the readings RFC 5545 leaves open.
    #[must_use]
    pub const fn policy(&self) -> ResolutionPolicy {
        self.policy
    }

    /// The nominal instant `dtstart` anchors this series at.
    ///
    /// The shapes RFC 5545 gives a `DTSTART` are three different amounts of work. A `DATE` and a
    /// floating `DATE-TIME` are wall clocks, and a wall clock is already nominal. A zoned
    /// `DATE-TIME` is a wall clock too, and deliberately does not consult the zone: the
    /// projection is [`nominal`] and nothing else, which is also why a `TZID` parameter that
    /// disagrees with this series' own identifier cannot quietly move the anchor — the zone
    /// enters at [`ZonedSeries::actual`], under the identifier the caller stated. A
    /// `Z`-terminated `DATE-TIME` names a real UTC instant, which is not on the nominal
    /// timeline, and goes through [`ZonedSeries::to_nominal`].
    ///
    /// `None` when a `Z`-terminated value meets a source that does not recognize this series'
    /// identifier, or when the projection leaves the years RFC 5545 section 3.3.4 can write.
    ///
    /// Nothing is reported here, and there is nothing this point could report that is not
    /// reported better later: a `DTSTART` is itself a cadence key, so whatever is awkward about
    /// its wall clock is seen by [`ZonedSeries::actual`] on the same terms as every other
    /// occurrence, against the caller's own meter and sink.
    #[must_use]
    pub fn anchor(&self, dtstart: DateTimeValue<'_>) -> Option<Instant> {
        match dtstart {
            DateTimeValue::Date(date) => nominal(CivilDateTime::new(date, CivilTime::MIDNIGHT)),
            DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => nominal(stamp),
            DateTimeValue::Utc(stamp) => {
                let named = stamp.at_offset(UtcOffset::UTC)?;
                self.to_nominal(named)
            },
        }
    }

    /// The real instant `dtstart` names, for a series whose cadence counts elapsed time.
    ///
    /// The other anchor, and the one the seam owes a frequency that is not a civil one.
    /// [`ZonedSeries::anchor`] projects onto the wall clock, which is what `FREQ=DAILY` and
    /// everything coarser mean: "every day at 09:00" is a statement about a clock, and a day is
    /// 23 or 25 hours wherever the zone moved. `FREQ=HOURLY` and finer say the opposite — an
    /// hour is an hour — so a series anchored on the wall clock and walked hourly loses the hour
    /// a zone repeats and gains the one it skips, which is 24 occurrences on a day that is
    /// twenty-five hours long.
    ///
    /// A series anchored here is walked on the real timeline, and its keys are already the
    /// instants the occurrences happen at: [`ZonedSeries::actual`] is neither needed nor correct
    /// for one, because the keys were never nominal. This is the divergence `crate::seam`
    /// records — Google's engine gives 25 and libical's local-time expansion gives 24, both
    /// ship, and this workspace offers both readings by making the anchor the caller's stated
    /// choice rather than a consequence of a frequency nobody looked at.
    ///
    /// `None` when the wall clock names no instant this policy takes, or when the source does
    /// not recognize this series' identifier.
    #[must_use]
    pub fn real_anchor(&self, dtstart: DateTimeValue<'_>) -> Option<Instant> {
        match dtstart {
            // Already real: a `Z`-terminated value names a UTC instant outright.
            DateTimeValue::Utc(stamp) => stamp.at_offset(UtcOffset::UTC),
            DateTimeValue::Date(date) => {
                self.resolved_clock(CivilDateTime::new(date, CivilTime::MIDNIGHT))
            },
            DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => {
                self.resolved_clock(stamp)
            },
        }
    }

    /// The instant a wall clock names under this zone and policy.
    fn resolved_clock(&self, local: CivilDateTime) -> Option<Instant> {
        self.source
            .resolve(self.tzid, local)?
            .resolution
            .pick(self.policy.gaps(), self.policy.folds())
    }

    /// The nominal instant a real UTC instant projects onto.
    ///
    /// The zone's offset at `utc`, then the wall clock that offset shows there, then
    /// [`nominal`]. Every `Z`-terminated value on a zoned series goes through this and none of
    /// them may skip it, because a real UTC instant and the keys it will be compared against
    /// live on two different timelines that coincide only where the offset is zero.
    ///
    /// `None` when the source does not recognize this series' identifier, or when the wall clock
    /// falls outside the years RFC 5545 section 3.3.4 can write.
    #[must_use]
    pub fn to_nominal(&self, utc: Instant) -> Option<Instant> {
        self.projected(utc).map(|(projected, _basis)| projected)
    }

    /// The nominal instant this series' `UNTIL` bounds it at, saying what the file got wrong.
    ///
    /// A `Z`-terminated `UNTIL` names a real UTC instant and is projected exactly as a
    /// `Z`-terminated `DTSTART` is. Skipping that is what cuts a zoned series off an hour early
    /// or late for half the year, since the cadence keys the bound is compared against are
    /// nominal and it is not.
    ///
    /// A floating `UNTIL` is already nominal and needs no conversion at all. What it needs is a
    /// diagnostic: RFC 5545 section 3.3.10 requires an `UNTIL` to be written in UTC wherever
    /// `DTSTART` is UTC or zoned, and the reading that recovers the producer's intent is
    /// `DTSTART`'s own zone, which under the projection means the wall-clock fields are used as
    /// written. That is `recurrence-until-not-utc`, and this is its only emission site. Against
    /// a floating `DTSTART` the same value is what section 3.3.10 asks for and nothing is
    /// reported.
    ///
    /// An `UNTIL` written as a `DATE` is read where [`UntilReading`] says. Where `DTSTART` is a
    /// `DATE` too there is no mismatch to resolve and the day begins at midnight under either
    /// policy. `recurrence-until-value-type-mismatch` is not emitted here: it travels from
    /// whoever parsed the rule, and reporting it twice makes one defect look like two.
    ///
    /// `None` on the terms [`ZonedSeries::anchor`] gives, and the diagnostic follows the
    /// projection rather than preceding it — a value that leaves the timeline has no instant for
    /// a diagnostic to name, and RFC 5545 section 3.3.4 makes that unreachable for a value read
    /// out of a file.
    ///
    /// [`UntilReading`]: crate::UntilReading
    pub fn project_until<D: DiagnosticSink + ?Sized>(
        &self,
        until: DateTimeValue<'_>,
        dtstart: DateTimeValue<'_>,
        meter: &mut Meter,
        sink: &mut D,
    ) -> Option<Instant> {
        match until {
            DateTimeValue::Date(date) => nominal(day_reading(date, dtstart, self.policy.until())?),
            DateTimeValue::Utc(stamp) => {
                let named = stamp.at_offset(UtcOffset::UTC)?;
                let (projected, basis) = self.projected(named)?;
                if let Some(code) = basis.diagnostic_code() {
                    report_at(code, named, meter, sink);
                }
                Some(projected)
            },
            DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => {
                let projected = nominal(stamp)?;
                if requires_utc(dtstart) {
                    report_at(
                        DiagnosticCode::RecurrenceUntilNotUtc,
                        projected,
                        meter,
                        sink,
                    );
                }
                Some(projected)
            },
        }
    }

    /// What the zone says about the wall clock the cadence key `key` stands for.
    ///
    /// The whole answer, with its provenance and its basis still on it, which is what
    /// `docs/adr/0003` requires of every result and what [`ZonedSeries::actual`] necessarily
    /// discards on its way to one instant. A caller that wants to show a person which source
    /// answered, or that a fold happened rather than only which side of it was taken, reads this
    /// and matches on it.
    ///
    /// `None` means exactly what [`ZoneSource::resolve`] means by it: this source does not
    /// recognize this identifier. That is also the one condition `actual` cannot tell a caller
    /// apart from a skipped gap, and the reason both methods exist.
    ///
    /// [`ZoneSource::resolve`]: crate::ZoneSource::resolve
    #[must_use]
    pub fn answer_for(&self, key: Instant) -> Option<ZoneAnswer> {
        let clock = wall_clock(key)?;
        self.source.resolve(self.tzid, clock)
    }

    /// The instant the wall clock at cadence key `key` names, with nothing reported.
    ///
    /// [`ZonedSeries::actual`] without the sink, for the two callers that have no diagnostic to
    /// make: a predicate handed to `ical-recur` as a series' second gate, and a measurement
    /// taken between two keys. Same policy, same answer, no meter.
    #[must_use]
    pub fn resolved(&self, key: Instant) -> Option<Instant> {
        self.answer_for(key)?
            .resolution
            .pick(self.policy.gaps(), self.policy.folds())
    }

    /// Whether an occurrence at cadence key `key` is one this zone and policy admit.
    ///
    /// `docs/adr/0011`'s second gate, in the shape `ical_recur::RecurrenceInput::admitting`
    /// takes: the first gate is a date that exists and this one is a local time that exists,
    /// and composing them in that order is what makes `COUNT` count instances a caller
    /// receives. Under the default policy an hour the zone sprang over is not admitted; under
    /// [`GapPolicy::ShiftForward`] it is, at the instant section 3.3.5 reads it as.
    ///
    /// `true` where the source does not recognize this series' identifier. Nothing there has
    /// said the local time does not exist, and a gate that dropped every occurrence of a series
    /// whose zone is merely undefined would be answering a question nobody asked it.
    ///
    /// [`GapPolicy::ShiftForward`]: crate::GapPolicy::ShiftForward
    #[must_use]
    pub fn admits(&self, key: Instant) -> bool {
        let Some(answer) = self.answer_for(key) else {
            return true;
        };
        answer
            .resolution
            .pick(self.policy.gaps(), self.policy.folds())
            .is_some()
    }

    /// The instant the wall clock that cadence key `key` spells happens at.
    ///
    /// Called once per emitted occurrence, and that is the whole mechanism: the key is read back
    /// into a wall clock, the wall clock is resolved against the zone, and the offset in force on
    /// that particular day is applied to that occurrence alone. A daily 09:00 series stays at
    /// 09:00 across a transition because the offsets are never applied in bulk to an anchor.
    ///
    /// **The argument is the wall clock to resolve, and for a moved occurrence that is not its
    /// cadence key.** This sentence used to read "the instant the occurrence at cadence key
    /// `key` actually happens at", which is false for exactly the occurrences an organizer
    /// edited: a `RANGE=THISANDFUTURE` override moving a 09:00 standup to 11:00 leaves each
    /// later occurrence with a 09:00 *key* and an 11:00 *effective start*, and resolving the key
    /// renders a meeting two hours before the one that exists. What to pass is
    /// `ical_recur::Occurrence::start`, which is the key for every occurrence no override moved
    /// and the moved value for the rest. This crate cannot take an `Occurrence` and say so
    /// itself — it is `ical-recur`'s sibling and does not depend on it — so the discipline is
    /// stated here and held by `crates/icalkit-conformance/tests/break_zones.rs`, which resolves
    /// effective starts.
    ///
    /// The three states a wall clock can be in are collapsed under [`ResolutionPolicy`] and
    /// reported before they are collapsed, so a caller reading the sink knows which occurrences
    /// the answer chose for. `None` means either that the policy skipped a gap — RFC 5545
    /// section 3.3.10's MUST, and the default — or that the source does not know this identifier;
    /// [`ZonedSeries::answer_for`] is where those come apart.
    ///
    /// [`ResolutionPolicy`]: crate::ResolutionPolicy
    pub fn actual<D: DiagnosticSink + ?Sized>(
        &self,
        key: Instant,
        meter: &mut Meter,
        sink: &mut D,
    ) -> Option<Instant> {
        let answer = self.answer_for(key)?;
        // The resolution first and the basis second: an occurrence in a fold at the far end of a
        // table that ran out is both facts at once, and the one about this occurrence's own wall
        // clock is the one a reader is looking for.
        if let Some(code) = answer.resolution.diagnostic_code() {
            report_at(code, key, meter, sink);
        }
        if let Some(code) = answer.basis.diagnostic_code() {
            report_at(code, key, meter, sink);
        }
        answer
            .resolution
            .pick(self.policy.gaps(), self.policy.folds())
    }

    /// The projection of a real UTC instant, with the basis the source answered on.
    ///
    /// The arithmetic of [`ZonedSeries::to_nominal`] and the fact that method has no channel to
    /// report. Kept together so that the one caller holding a meter reports the basis of the
    /// very answer it projected, rather than asking a second time and risking a second answer.
    fn projected(&self, utc: Instant) -> Option<(Instant, AnswerBasis)> {
        let answer = self.source.offset_at(self.tzid, utc)?;
        let clock = CivilDateTime::from_instant(utc, answer.offset)?;
        nominal(clock).map(|projected| (projected, answer.basis))
    }
}

impl<S: ?Sized> Debug for ZonedSeries<'_, S> {
    /// Written by hand for the reason `CombinedZoneSource`'s is: `S` may be `dyn ZoneSource`,
    /// and a derived implementation would hold it to `Debug`.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZonedSeries")
            .field("tzid", &self.tzid)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// Whether RFC 5545 section 3.3.10 requires this `DTSTART`'s `UNTIL` to be written in UTC.
///
/// Section 3.3.10 states the rule from `DTSTART`'s side: a floating `DTSTART` takes a floating
/// `UNTIL`, and a UTC or zoned one takes a UTC `UNTIL`. A `DATE` `DTSTART` is neither, and the
/// disagreement it can have with its `UNTIL` is about the value type rather than about the
/// clock, which is a different code and somebody else's to emit.
const fn requires_utc(dtstart: DateTimeValue<'_>) -> bool {
    matches!(dtstart, DateTimeValue::Utc(_) | DateTimeValue::Zoned { .. })
}

/// The wall clock an `UNTIL` written as a `DATE` stands for, under `reading`.
///
/// The policy applies to the mismatch and only to the mismatch. Where `DTSTART` is a `DATE` the
/// two agree, the series' own keys are midnights, and midnight is both the literal reading and
/// the one that keeps the named day inside the series — so there is nothing for a caller to
/// state and stating it changes nothing.
///
/// `None` only where [`END_OF_DAY`] is, which is nowhere.
fn day_reading(
    date: CivilDate,
    dtstart: DateTimeValue<'_>,
    reading: UntilReading,
) -> Option<CivilDateTime> {
    let time = match (dtstart, reading) {
        (DateTimeValue::Date(_), _) | (_, UntilReading::Midnight) => CivilTime::MIDNIGHT,
        (_, UntilReading::EndOfDay) => END_OF_DAY?,
    };
    Some(CivilDateTime::new(date, time))
}

/// Report `code` about the occurrence at `at`, on the channel that code travels on.
fn report_at<D: DiagnosticSink + ?Sized>(
    code: DiagnosticCode,
    at: Instant,
    meter: &mut Meter,
    sink: &mut D,
) {
    report_diagnostic(
        sink,
        meter,
        Diagnostic::at_instant(code, channel_for(code), at),
    );
}

/// The channel a code emitted here travels on, as `docs/diagnostic-codes.md` fixes it.
///
/// Which code describes which state is never decided in this file: it is read off
/// `LocalResolution::diagnostic_code` and `AnswerBasis::diagnostic_code`, so two units cannot
/// disagree about it. What those mappings do not carry is the severity, because a
/// `DiagnosticCode` is one vocabulary and a channel is a claim about how much one emission
/// means — a local time that occurs twice does occur, and a zone asked past the end of its table
/// is a legal question put to a legal file, while a local time that does not exist is the file
/// asking for something that is not there.
///
/// The last arm is required rather than chosen, because `DiagnosticCode` is `#[non_exhaustive]`.
/// It answers `Violation` because that is the channel a strict caller does not filter out: a
/// code added to either mapping after this was written is a claim this unit cannot classify, and
/// under-stating one is the failure that hides.
fn channel_for(code: DiagnosticCode) -> Severity {
    match code {
        DiagnosticCode::AmbiguousLocalTime | DiagnosticCode::TimeZoneCoverageExhausted => {
            Severity::Note
        },
        _ => Severity::Violation,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, Instant,
        Limits, Meter, Severity, UtcOffset,
    };

    use super::ZonedSeries;
    use crate::answer::{
        AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, Reading, ZoneAnswer,
        ZoneProvenance, ZoneSource,
    };
    use crate::seam::{ResolutionPolicy, UntilReading, nominal};

    /// Seconds in a day, used to walk a cadence the way `ical-recur`'s period walk does.
    const ONE_DAY: i64 = 86_400;

    /// One transition of a real zone: when the clock moved, and what it moved between.
    ///
    /// Every value below is transcribed from the transition rules the zone actually ran, not
    /// read off this crate's answers.
    #[derive(Clone, Copy, Debug)]
    struct Shift {
        /// The instant the offset changed.
        at: Instant,
        /// Seconds east of UTC before it.
        before: i32,
        /// Seconds east of UTC from it.
        after: i32,
        /// Whether the observance beginning here is the zone's daylight one.
        daylight: bool,
    }

    /// A zone source built from published transitions, standing in for a read `VTIMEZONE`.
    ///
    /// Written here rather than reached for from the unit that owns the real source, for the
    /// reason `answer.rs`'s own fixture gives: what is under test is this unit, and a test
    /// leaning on another unit's file would be testing that unit too.
    #[derive(Clone, Debug)]
    struct TestZone {
        /// The identifier this source answers to, compared by exact bytes.
        tzid: &'static str,
        /// Seconds east of UTC before the first transition.
        base: i32,
        /// Whether that offset is the zone's daylight one.
        ///
        /// True where a table starts in the middle of the zone's summer, which is what a
        /// southern hemisphere zone looks like in January, and not an afterthought: the flag is
        /// the observance's own classification and cannot be inferred from the offsets.
        base_daylight: bool,
        /// The transitions, ascending.
        shifts: Vec<Shift>,
        /// The last date backed by real data, absent when the zone knows the future.
        coverage_end: Option<CivilDate>,
    }

    impl TestZone {
        /// The offset and daylight flag in force at `instant`.
        fn state_at(&self, instant: Instant) -> (i32, bool) {
            let mut state = (self.base, self.base_daylight);
            for shift in &self.shifts {
                if shift.at <= instant {
                    state = (shift.after, shift.daylight);
                }
            }
            state
        }

        /// How much of this zone's data stands behind a question about `date`.
        fn basis_at(&self, date: CivilDate) -> AnswerBasis {
            match self.coverage_end {
                Some(end) if date > end => AnswerBasis::BeyondKnownTransitions(end),
                _ => AnswerBasis::Computed,
            }
        }

        /// Every offset this zone ever runs at.
        fn offsets(&self) -> Vec<i32> {
            let mut seen = alloc::vec![self.base];
            for shift in &self.shifts {
                if !seen.contains(&shift.after) {
                    seen.push(shift.after);
                }
            }
            seen
        }

        /// The reading `local` has under `seconds`, present only where that offset governs it.
        fn reading(&self, local: CivilDateTime, seconds: i32) -> Option<Reading> {
            let offset = UtcOffset::from_seconds(seconds)?;
            let instant = local.at_offset(offset)?;
            let (in_force, daylight) = self.state_at(instant);
            (in_force == seconds).then_some(Reading::new(instant, offset, daylight))
        }

        /// Every reading `local` has, ascending — none in a gap, two in a fold.
        fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
            let mut found: Vec<Reading> = Vec::new();
            for seconds in self.offsets() {
                if let Some(reading) = self.reading(local, seconds) {
                    found.push(reading);
                }
            }
            found.sort_unstable();
            found
        }

        /// The gap `local` fell in, on the readings `answer.rs` states as invariants: the last
        /// instant before the gap opened, and the instant it closed at.
        fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
            for shift in &self.shifts {
                let offset_before = UtcOffset::from_seconds(shift.before)?;
                let offset_after = UtcOffset::from_seconds(shift.after)?;
                let opened = CivilDateTime::from_instant(shift.at, offset_before)?;
                let closed = CivilDateTime::from_instant(shift.at, offset_after)?;
                if opened <= local && local < closed {
                    return Some(LocalResolution::Nonexistent {
                        gap_start: shift.at.checked_add_seconds(-1)?,
                        gap_end: shift.at,
                        offset_before,
                        offset_after,
                        shifted: local.at_offset(offset_before)?,
                    });
                }
            }
            None
        }
    }

    impl ZoneSource for TestZone {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let found = self.readings(local);
            let resolution = match found.as_slice() {
                [reading] => LocalResolution::Unique { reading: *reading },
                [earlier, later] => LocalResolution::Ambiguous {
                    earlier: *earlier,
                    later: *later,
                },
                _ => self.gap(local)?,
            };
            Some(ZoneAnswer::new(
                resolution,
                ZoneProvenance::EmbeddedVtimezone,
                self.basis_at(local.date()),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let (seconds, daylight) = self.state_at(instant);
            let offset = UtcOffset::from_seconds(seconds)?;
            let date = CivilDateTime::from_instant(instant, offset)?.date();
            Some(OffsetAnswer::new(
                offset,
                daylight,
                ZoneProvenance::EmbeddedVtimezone,
                self.basis_at(date),
            ))
        }
    }

    /// A wall clock with no zone attached to it yet.
    fn clock(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        CivilDateTime::new(date, CivilTime::from_hms(hour, minute, 0).unwrap())
    }

    /// The instant a published UTC timestamp names.
    fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        clock(year, month, day, hour, minute)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// The nominal cadence key a wall clock is, which is what `ical-recur` walks.
    fn key(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        nominal(clock(year, month, day, hour, minute)).unwrap()
    }

    /// One transition, spelled the way the tz database states it.
    fn shift(at: Instant, before: i32, after: i32, daylight: bool) -> Shift {
        Shift {
            at,
            before,
            after,
            daylight,
        }
    }

    /// `America/New_York`, including the 2007 rule change.
    ///
    /// Before 2007 the United States sprang forward on the first Sunday in April and fell back
    /// on the last Sunday in October; from 2007 it is the second Sunday in March and the first
    /// Sunday in November. Both sets are here, so a question about 2006 and a question about
    /// 2007 are answered by different rules.
    fn new_york() -> TestZone {
        TestZone {
            tzid: "America/New_York",
            base: -18_000,
            base_daylight: false,
            shifts: alloc::vec![
                shift(utc(2006, 4, 2, 7, 0), -18_000, -14_400, true),
                shift(utc(2006, 10, 29, 6, 0), -14_400, -18_000, false),
                shift(utc(2007, 3, 11, 7, 0), -18_000, -14_400, true),
                shift(utc(2007, 11, 4, 6, 0), -14_400, -18_000, false),
                shift(utc(2026, 3, 8, 7, 0), -18_000, -14_400, true),
                shift(utc(2026, 11, 1, 6, 0), -14_400, -18_000, false),
            ],
            coverage_end: None,
        }
    }

    /// Europe/Berlin under `tzid`, whose transitions happen at 01:00 UTC rather than at a local
    /// hour, on the last Sunday of March and of October.
    ///
    /// The identifier is a parameter because a `VTIMEZONE` carrying these rules is filed in the
    /// wild under `Europe/Berlin`, `W. Europe Standard Time`,
    /// `/mozilla.org/20050126_1/Europe/Berlin` and `Customized Time Zone`, and this crate must
    /// answer to all four without parsing any of them.
    fn berlin_as(tzid: &'static str) -> TestZone {
        TestZone {
            tzid,
            base: 3_600,
            base_daylight: false,
            shifts: alloc::vec![
                shift(utc(2026, 3, 29, 1, 0), 3_600, 7_200, true),
                shift(utc(2026, 10, 25, 1, 0), 7_200, 3_600, false),
            ],
            coverage_end: None,
        }
    }

    /// Europe/Berlin under its database name.
    fn berlin() -> TestZone {
        berlin_as("Europe/Berlin")
    }

    /// The same rules written out as explicit dates through 2029 and stopping there.
    ///
    /// What an `RDATE`-driven `VTIMEZONE` is, filed under the Windows identifier Exchange
    /// writes. A question about 2035 continues the final observance and says so, which is the
    /// input this milestone turns on.
    fn windows_western_europe() -> TestZone {
        TestZone {
            tzid: "W. Europe Standard Time",
            base: 3_600,
            base_daylight: false,
            shifts: alloc::vec![
                shift(utc(2026, 3, 29, 1, 0), 3_600, 7_200, true),
                shift(utc(2026, 10, 25, 1, 0), 7_200, 3_600, false),
                shift(utc(2027, 3, 28, 1, 0), 3_600, 7_200, true),
                shift(utc(2027, 10, 31, 1, 0), 7_200, 3_600, false),
                shift(utc(2028, 3, 26, 1, 0), 3_600, 7_200, true),
                shift(utc(2028, 10, 29, 1, 0), 7_200, 3_600, false),
                shift(utc(2029, 3, 25, 1, 0), 3_600, 7_200, true),
                shift(utc(2029, 10, 28, 1, 0), 7_200, 3_600, false),
            ],
            coverage_end: CivilDate::from_ymd(2029, 10, 28),
        }
    }

    /// `Australia/Lord_Howe`, whose daylight saving moves the clock by thirty minutes.
    ///
    /// +10:30 standard against +11:00 daylight, so an implementation that assumes an hour is
    /// wrong here in both directions: the fold is thirty minutes long and so is the gap.
    fn lord_howe() -> TestZone {
        TestZone {
            tzid: "Australia/Lord_Howe",
            base: 39_600,
            base_daylight: true,
            shifts: alloc::vec![
                shift(utc(2026, 4, 4, 15, 0), 39_600, 37_800, false),
                shift(utc(2026, 10, 3, 15, 30), 37_800, 39_600, true),
            ],
            coverage_end: None,
        }
    }

    /// A meter and a sink, which every reporting call needs and no test wants to spell twice.
    fn ledger() -> (Meter, Vec<Diagnostic>) {
        (Meter::new(Limits::DEFAULT), Vec::new())
    }

    /// The codes reported, in the order they were reported.
    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().map(|entry| entry.code()).collect()
    }

    /// An hour that repeats names two instants, and the policy says which one this caller wants.
    #[test]
    fn a_local_time_the_zone_fell_back_through_names_both_instants() {
        let cases = [
            (
                new_york(),
                clock(2026, 11, 1, 1, 30),
                utc(2026, 11, 1, 5, 30),
                utc(2026, 11, 1, 6, 30),
            ),
            (
                berlin(),
                clock(2026, 10, 25, 2, 30),
                utc(2026, 10, 25, 0, 30),
                utc(2026, 10, 25, 1, 30),
            ),
            (
                lord_howe(),
                clock(2026, 4, 5, 1, 45),
                utc(2026, 4, 4, 14, 45),
                utc(2026, 4, 4, 15, 15),
            ),
        ];
        for (zone, local, first, second) in cases {
            let cadence = nominal(local).unwrap();
            let with_first = ZonedSeries::new(&zone, zone.tzid, ResolutionPolicy::DEFAULT);
            let answer = with_first.answer_for(cadence).unwrap();
            assert!(answer.resolution.is_ambiguous(), "{}", zone.tzid);
            assert_eq!(answer.resolution.earliest(), Some(first));
            assert_eq!(answer.resolution.unambiguous(), None);

            let (mut meter, mut reported) = ledger();
            assert_eq!(
                with_first.actual(cadence, &mut meter, &mut reported),
                Some(first)
            );
            assert_eq!(
                codes(&reported),
                alloc::vec![DiagnosticCode::AmbiguousLocalTime]
            );
            assert_eq!(reported[0].severity(), Severity::Note);
            assert_eq!(reported[0].instant(), Some(cadence));

            let take_later = ResolutionPolicy::DEFAULT.with_folds(FoldPolicy::Later);
            let with_second = ZonedSeries::new(&zone, zone.tzid, take_later);
            assert_eq!(
                with_second.actual(cadence, &mut meter, &mut reported),
                Some(second)
            );
        }
    }

    /// An hour that does not exist names no instant, and each of the three readings is a value.
    #[test]
    fn a_local_time_the_zone_sprang_over_names_none_and_the_policy_says_what_to_do() {
        let cases = [
            (
                new_york(),
                clock(2026, 3, 8, 2, 30),
                utc(2026, 3, 8, 7, 30),
                utc(2026, 3, 8, 7, 0),
            ),
            (
                berlin(),
                clock(2026, 3, 29, 2, 30),
                utc(2026, 3, 29, 1, 30),
                utc(2026, 3, 29, 1, 0),
            ),
            (
                lord_howe(),
                clock(2026, 10, 4, 2, 15),
                utc(2026, 10, 3, 15, 45),
                utc(2026, 10, 3, 15, 30),
            ),
        ];
        for (zone, local, shifted, transition) in cases {
            let cadence = nominal(local).unwrap();
            let skipping = ZonedSeries::new(&zone, zone.tzid, ResolutionPolicy::DEFAULT);
            assert!(
                skipping
                    .answer_for(cadence)
                    .unwrap()
                    .resolution
                    .is_nonexistent()
            );

            let (mut meter, mut reported) = ledger();
            assert_eq!(skipping.actual(cadence, &mut meter, &mut reported), None);
            assert_eq!(
                codes(&reported),
                alloc::vec![DiagnosticCode::NonexistentLocalTime]
            );
            assert_eq!(reported[0].severity(), Severity::Violation);
            assert_eq!(reported[0].instant(), Some(cadence));

            let read_before = ResolutionPolicy::DEFAULT.with_gaps(GapPolicy::ShiftForward);
            let with_shift = ZonedSeries::new(&zone, zone.tzid, read_before);
            assert_eq!(
                with_shift.actual(cadence, &mut meter, &mut reported),
                Some(shifted),
                "RFC 5545 section 3.3.5 reads a gap with the offset in force before it"
            );

            let move_to_end = ResolutionPolicy::DEFAULT.with_gaps(GapPolicy::ClampToTransition);
            let with_clamp = ZonedSeries::new(&zone, zone.tzid, move_to_end);
            assert_eq!(
                with_clamp.actual(cadence, &mut meter, &mut reported),
                Some(transition),
                "clamping happens as soon as the gap closes, which is the transition itself"
            );
        }
    }

    /// A zone whose rules changed is answered by the rules of the year asked about.
    ///
    /// March 12th 2006 is the second Sunday in March and an ordinary day, because the United
    /// States did not move its clocks then; March 11th 2007 is the second Sunday in March under
    /// the rule that replaced it, and 02:30 does not exist.
    #[test]
    fn the_rules_that_changed_after_2007_are_read_at_the_year_asked_about() {
        let zone = new_york();
        let series = ZonedSeries::new(&zone, "America/New_York", ResolutionPolicy::DEFAULT);
        let cases = [
            (
                clock(2006, 3, 12, 2, 30),
                Some(utc(2006, 3, 12, 7, 30)),
                None,
            ),
            (
                clock(2006, 4, 2, 2, 30),
                None,
                Some(DiagnosticCode::NonexistentLocalTime),
            ),
            (
                clock(2006, 10, 29, 1, 30),
                None,
                Some(DiagnosticCode::AmbiguousLocalTime),
            ),
            (
                clock(2007, 3, 11, 2, 30),
                None,
                Some(DiagnosticCode::NonexistentLocalTime),
            ),
            (
                clock(2007, 11, 4, 1, 30),
                None,
                Some(DiagnosticCode::AmbiguousLocalTime),
            ),
        ];
        for (local, unique, code) in cases {
            let answer = series.answer_for(nominal(local).unwrap()).unwrap();
            assert_eq!(answer.resolution.unambiguous(), unique, "{local:?}");
            assert_eq!(answer.resolution.diagnostic_code(), code, "{local:?}");
            assert_eq!(answer.source, ZoneProvenance::EmbeddedVtimezone);
        }
    }

    /// Agenda item 4, stated as arithmetic: the wall clock does not move and the instant does.
    ///
    /// The keys are walked the way `ical-recur`'s period walk walks them — one nominal day at a
    /// time, civil fields preserved — and each is resolved on its own. The naive reading beside
    /// it is what anchoring at a real UTC instant and never re-resolving produces, and it is an
    /// hour out from the transition onwards, which is the failure M1 named.
    #[test]
    fn a_daily_nine_o_clock_series_stays_at_nine_across_a_transition() {
        let zone = berlin();
        let series = ZonedSeries::new(&zone, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        let dtstart = DateTimeValue::Zoned {
            stamp: clock(2026, 3, 27, 9, 0),
            tzid: b"Europe/Berlin",
        };
        let anchor = series.anchor(dtstart).unwrap();
        assert_eq!(anchor, key(2026, 3, 27, 9, 0));

        let expected = [
            utc(2026, 3, 27, 8, 0),
            utc(2026, 3, 28, 8, 0),
            utc(2026, 3, 29, 7, 0),
            utc(2026, 3, 30, 7, 0),
        ];
        let (mut meter, mut reported) = ledger();
        for (step, wanted) in expected.into_iter().enumerate() {
            let elapsed = i64::try_from(step).unwrap().checked_mul(ONE_DAY).unwrap();
            let cadence = anchor.checked_add_seconds(elapsed).unwrap();
            assert_eq!(
                series.actual(cadence, &mut meter, &mut reported),
                Some(wanted)
            );
            let naive = expected[0].checked_add_seconds(elapsed).unwrap();
            let drift = wanted.checked_seconds_until(naive).unwrap();
            let expected_drift = if step < 2 { 0 } else { 3_600 };
            assert_eq!(drift, expected_drift, "step {step} of the naive reading");
        }
        assert_eq!(
            expected[0].checked_add_seconds(ONE_DAY.checked_mul(3).unwrap()),
            Some(utc(2026, 3, 30, 8, 0)),
            "the naive reading ends an hour late, which is the failure this seam exists for"
        );
        assert!(
            reported.is_empty(),
            "an ordinary 09:00 is not an awkward hour"
        );
    }

    /// A table that ends still answers, and the answer says it was continued rather than read.
    #[test]
    fn a_transition_table_that_ends_before_the_question_says_so_and_still_answers() {
        let zone = windows_western_europe();
        let series = ZonedSeries::new(&zone, "W. Europe Standard Time", ResolutionPolicy::DEFAULT);
        let known = CivilDate::from_ymd(2029, 10, 28).unwrap();

        let covered = key(2029, 6, 15, 12, 0);
        let (mut meter, mut reported) = ledger();
        assert_eq!(
            series.actual(covered, &mut meter, &mut reported),
            Some(utc(2029, 6, 15, 10, 0))
        );
        assert!(
            reported.is_empty(),
            "a year the table covers is not an extrapolation"
        );

        let past = key(2035, 6, 15, 12, 0);
        let answer = series.answer_for(past).unwrap();
        assert_eq!(answer.basis, AnswerBasis::BeyondKnownTransitions(known));
        assert_eq!(answer.basis.nearest_known(), Some(known));
        assert_eq!(
            series.actual(past, &mut meter, &mut reported),
            Some(utc(2035, 6, 15, 11, 0)),
            "the final observance is continued, which is defensible and not silent"
        );
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::TimeZoneCoverageExhausted]
        );
        assert_eq!(reported[0].severity(), Severity::Note);
        assert_eq!(reported[0].instant(), Some(past));

        let endless = berlin();
        let rule_driven = ZonedSeries::new(&endless, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        assert_eq!(
            rule_driven.answer_for(past).unwrap().basis,
            AnswerBasis::Computed
        );
    }

    /// A `TZID` is not an IANA identifier, and this crate neither parses one nor fails on one.
    #[test]
    fn an_identifier_that_is_not_a_database_name_is_answered_and_never_parsed() {
        let names = [
            "W. Europe Standard Time",
            "/mozilla.org/20050126_1/Europe/Berlin",
            "Customized Time Zone",
            "Europe/Berlin",
        ];
        let cadence = key(2026, 7, 1, 9, 0);
        for name in names {
            let zone = berlin_as(name);
            let series = ZonedSeries::new(&zone, name, ResolutionPolicy::DEFAULT);
            let (mut meter, mut reported) = ledger();
            assert_eq!(
                series.actual(cadence, &mut meter, &mut reported),
                Some(utc(2026, 7, 1, 7, 0)),
                "{name}"
            );
            assert!(reported.is_empty(), "{name}");
        }

        let zone = berlin();
        let unknown = ZonedSeries::new(&zone, "Europe/Zurich", ResolutionPolicy::DEFAULT);
        let (mut meter, mut reported) = ledger();
        assert_eq!(unknown.answer_for(cadence), None);
        assert_eq!(unknown.actual(cadence, &mut meter, &mut reported), None);
        assert!(
            reported.is_empty(),
            "an unrecognized identifier is unit 3's code and not this unit's"
        );
    }

    /// Each shape of `DTSTART` is projected by what it is, and only the `Z` one needs the zone.
    #[test]
    fn a_dtstart_is_projected_by_its_shape_and_a_z_terminated_one_through_the_zone() {
        let zone = berlin();
        let series = ZonedSeries::new(&zone, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        let cases = [
            (
                DateTimeValue::Date(CivilDate::from_ymd(2026, 3, 29).unwrap()),
                key(2026, 3, 29, 0, 0),
            ),
            (
                DateTimeValue::Local(clock(2026, 3, 29, 9, 0)),
                key(2026, 3, 29, 9, 0),
            ),
            (
                DateTimeValue::Zoned {
                    stamp: clock(2026, 3, 29, 9, 0),
                    tzid: b"Europe/Berlin",
                },
                key(2026, 3, 29, 9, 0),
            ),
            (
                DateTimeValue::Zoned {
                    stamp: clock(2026, 3, 29, 9, 0),
                    tzid: b"W. Europe Standard Time",
                },
                key(2026, 3, 29, 9, 0),
            ),
            (
                DateTimeValue::Utc(clock(2026, 3, 27, 8, 0)),
                key(2026, 3, 27, 9, 0),
            ),
            (
                DateTimeValue::Utc(clock(2026, 3, 29, 7, 0)),
                key(2026, 3, 29, 9, 0),
            ),
        ];
        for (dtstart, expected) in cases {
            assert_eq!(series.anchor(dtstart), Some(expected), "{dtstart:?}");
        }
        assert_eq!(
            series.to_nominal(utc(2026, 3, 29, 7, 0)),
            Some(key(2026, 3, 29, 9, 0))
        );

        let unknown = ZonedSeries::new(&zone, "Europe/Zurich", ResolutionPolicy::DEFAULT);
        assert_eq!(
            unknown.anchor(DateTimeValue::Utc(clock(2026, 3, 27, 8, 0))),
            None
        );
        assert_eq!(unknown.to_nominal(utc(2026, 3, 27, 8, 0)), None);
        assert_eq!(
            unknown.anchor(DateTimeValue::Local(clock(2026, 3, 29, 9, 0))),
            Some(key(2026, 3, 29, 9, 0)),
            "a wall clock is already nominal, so no zone is consulted and none is needed"
        );
    }

    /// Agenda item 1: a floating `UNTIL` is read in `DTSTART`'s own zone and reported.
    #[test]
    fn a_floating_until_against_a_zoned_dtstart_is_read_in_the_zone_and_reported() {
        let zone = berlin();
        let series = ZonedSeries::new(&zone, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        let until = DateTimeValue::Local(clock(2026, 10, 25, 9, 0));
        let bound = key(2026, 10, 25, 9, 0);
        let zoned = DateTimeValue::Zoned {
            stamp: clock(2026, 3, 27, 9, 0),
            tzid: b"Europe/Berlin",
        };
        let reported_against = [zoned, DateTimeValue::Utc(clock(2026, 3, 27, 8, 0))];
        for dtstart in reported_against {
            let (mut meter, mut reported) = ledger();
            assert_eq!(
                series.project_until(until, dtstart, &mut meter, &mut reported),
                Some(bound),
                "the wall-clock fields are already nominal, so the reading is what was written"
            );
            assert_eq!(
                codes(&reported),
                alloc::vec![DiagnosticCode::RecurrenceUntilNotUtc]
            );
            assert_eq!(reported[0].severity(), Severity::Violation);
            assert_eq!(reported[0].instant(), Some(bound));
        }

        let silent = [
            DateTimeValue::Local(clock(2026, 3, 27, 9, 0)),
            DateTimeValue::Date(CivilDate::from_ymd(2026, 3, 27).unwrap()),
        ];
        for dtstart in silent {
            let (mut meter, mut reported) = ledger();
            assert_eq!(
                series.project_until(until, dtstart, &mut meter, &mut reported),
                Some(bound)
            );
            assert!(
                reported.is_empty(),
                "section 3.3.10 asks for a floating UNTIL here, and a value-type mismatch is \
                 somebody else's code"
            );
        }
    }

    /// The obligation easiest to miss: a `Z`-terminated `UNTIL` is not a cadence key.
    #[test]
    fn a_z_terminated_until_is_projected_and_never_taken_as_written() {
        let zone = berlin();
        let series = ZonedSeries::new(&zone, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        let dtstart = DateTimeValue::Zoned {
            stamp: clock(2026, 3, 27, 9, 0),
            tzid: b"Europe/Berlin",
        };
        let until = DateTimeValue::Utc(clock(2026, 10, 25, 0, 0));
        let (mut meter, mut reported) = ledger();
        let bound = series
            .project_until(until, dtstart, &mut meter, &mut reported)
            .unwrap();
        assert_eq!(
            bound,
            key(2026, 10, 25, 2, 0),
            "00:00Z is 02:00 in Berlin that morning, which is where the bound falls"
        );
        assert_eq!(
            utc(2026, 10, 25, 0, 0).checked_seconds_until(bound),
            Some(7_200),
            "taking the instant as written would cut the series two hours short"
        );
        assert!(reported.is_empty());

        let expiring = windows_western_europe();
        let far = ZonedSeries::new(
            &expiring,
            "W. Europe Standard Time",
            ResolutionPolicy::DEFAULT,
        );
        let late = DateTimeValue::Utc(clock(2035, 6, 15, 10, 0));
        assert!(
            far.project_until(late, dtstart, &mut meter, &mut reported)
                .is_some()
        );
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::TimeZoneCoverageExhausted]
        );
        assert_eq!(
            reported[0].instant(),
            Some(utc(2035, 6, 15, 10, 0)),
            "the instant concerned is the one the zone was asked about"
        );

        let unknown = ZonedSeries::new(&zone, "Europe/Zurich", ResolutionPolicy::DEFAULT);
        assert_eq!(
            unknown.project_until(until, dtstart, &mut meter, &mut reported),
            None
        );
    }

    /// Agenda item 2: where in the named day a `DATE` `UNTIL` sits is a policy, not a reading.
    #[test]
    fn an_until_written_as_a_date_is_read_where_the_stated_policy_says() {
        let zone = berlin();
        let until = DateTimeValue::Date(CivilDate::from_ymd(2026, 10, 25).unwrap());
        let dtstart = DateTimeValue::Zoned {
            stamp: clock(2026, 3, 27, 9, 0),
            tzid: b"Europe/Berlin",
        };
        let last_instance = key(2026, 10, 25, 9, 0);

        let (mut meter, mut reported) = ledger();
        let default = ZonedSeries::new(&zone, "Europe/Berlin", ResolutionPolicy::DEFAULT);
        let midnight = default
            .project_until(until, dtstart, &mut meter, &mut reported)
            .unwrap();
        assert_eq!(midnight, key(2026, 10, 25, 0, 0));
        assert!(
            midnight < last_instance,
            "midnight is libical, dateutil and Google, and it drops the named day"
        );

        let stated = ResolutionPolicy::DEFAULT.with_until(UntilReading::EndOfDay);
        let inclusive = ZonedSeries::new(&zone, "Europe/Berlin", stated);
        let end = inclusive
            .project_until(until, dtstart, &mut meter, &mut reported)
            .unwrap();
        assert_eq!(
            end,
            nominal(clock(2026, 10, 25, 23, 59))
                .unwrap()
                .checked_add_seconds(59)
                .unwrap()
        );
        assert!(
            end > last_instance,
            "end of day is what the person who typed the date meant"
        );
        assert!(
            reported.is_empty(),
            "recurrence-until-value-type-mismatch travels from whoever parsed the rule"
        );

        let all_day = DateTimeValue::Date(CivilDate::from_ymd(2026, 3, 27).unwrap());
        for series in [&default, &inclusive] {
            assert_eq!(
                series.project_until(until, all_day, &mut meter, &mut reported),
                Some(key(2026, 10, 25, 0, 0)),
                "matched value types leave nothing for a policy to decide"
            );
        }
    }
}
