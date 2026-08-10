// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 7 — overrides that a zone transition or a rewritten rule made wrong, which closes
//! agenda items 5 and 6.
//!
//! Specification: RFC 5545 section 3.8.4.4 (`RECURRENCE-ID`) and section 3.2.13 (`RANGE`).
//!
//! The two types carry their constructors and accessors, the crate root already names both,
//! and the behavior below is owed by this unit and by nothing else. `extra_widening` is the one
//! new name, and the root's `pub use` of this module is where it is exported from — a `pub`
//! item this module is not re-exported for is unreachable, which `unreachable_pub` refuses:
//!
//! ```text
//! impl WallClockShift {
//!     pub fn measure<S: ZoneSource + ?Sized>(
//!         source: &S, tzid: &str, from: Instant, to: Instant,
//!     ) -> Option<Self>;
//! }
//! pub fn extra_widening(shifts: &[WallClockShift]) -> i64;
//! impl<'a> OrphanScan<'a> {
//!     pub fn observe(&mut self, key: Instant);
//!     pub fn finish<D: DiagnosticSink + ?Sized>(self, meter: &mut Meter, sink: &mut D) -> u32;
//! }
//! ```
//!
//! # Agenda item 5: a shift is not a scalar once a zone is involved
//!
//! `ical-recur` derives an override's shift as a scalar second count from the `RECURRENCE-ID`
//! to where the override moved, and `ical_recur::max_absolute_shift` widens the generation
//! window by the largest such count. Across a transition that is not the same move: an override
//! that shifted a meeting by one hour of *wall clock* shifted it by nothing or by two hours of
//! *elapsed* time depending on which way the zone went, and the widening inherits the
//! assumption that the two are one number.
//!
//! [`WallClockShift`] measures both and says whether they differ. `extra_widening` returns the
//! additional seconds a caller adds to `ical_recur::max_absolute_shift`'s answer so the widened
//! window still covers every shifted start. It never *narrows* the widening: a window too wide
//! costs candidates a budget already bounds, and a window too narrow loses an occurrence
//! silently, which is the asymmetry `docs/adr/0002` argues from.
//!
//! The two numbers meet at one place and it is worth naming. `ical-recur` propagates a
//! `RANGE=THISANDFUTURE` shift to later cadence keys as a count of seconds, and later keys are
//! resolved one at a time against the zone, so the *elapsed* cost of the organizer's one-hour
//! move is whatever the offset in force on each later day makes it. A widening derived from the
//! anchor's own elapsed count is therefore short by exactly the disagreement this type reports,
//! which is what `extra_widening` gives back.
//!
//! # Agenda item 6: the orphan override, which needs no zone
//!
//! An override whose `RECURRENCE-ID` names no generated cadence key is inert and was, before
//! this milestone, reported by nothing. Clients produce them routinely — an instance is edited,
//! then the rule beneath it is rewritten — so a file carries a meeting the user sees in the
//! client that wrote it and the expanded series does not have. Every other silent drop in these
//! crates has a code; this one now has `override-matches-no-instance`, and this unit is its
//! only emitter.
//!
//! [`OrphanScan`] is fed each emitted cadence key as the search runs and reports every
//! identifier it never saw when the search ends, at that identifier's own instant. No
//! materialization and no second pass, so it composes with a lazy iterator over a window rather
//! than requiring the collection `docs/adr/0002` forbids. What it cannot distinguish is an
//! override addressing an instant outside the searched window from one addressing an instant
//! the rule never generates; a caller wanting the stronger claim scans the whole series, and
//! [`OrphanScan::finish`] answering with a count rather than with nothing is what lets it say
//! so.

use alloc::vec::Vec;

use ical_core::{
    CivilDateTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity,
    report_diagnostic,
};

use crate::answer::ZoneSource;
use crate::seam::nominal;
use crate::series::ZonedSeries;

/// How far an override moved an occurrence, measured both ways.
///
/// Two numbers because across a daylight saving transition they are two facts. `elapsed` is
/// what `ical_recur::Override::shift_seconds` derives; `wall_clock` is what the organizer saw
/// happen. They are equal exactly when no transition falls between the two instants, which is
/// most of the time and is why an implementation that carries one number looks correct until it
/// is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WallClockShift {
    /// Seconds on the timeline from the cadence key to where the override moved.
    elapsed: i64,
    /// Seconds the wall clock moved between the same two points.
    wall_clock: i64,
}

impl WallClockShift {
    /// A shift of `elapsed` seconds on the timeline and `wall_clock` seconds on the clock.
    #[must_use]
    pub const fn new(elapsed: i64, wall_clock: i64) -> Self {
        Self {
            elapsed,
            wall_clock,
        }
    }

    /// Seconds on the timeline, which is what a scalar shift counts.
    #[must_use]
    pub const fn elapsed_seconds(self) -> i64 {
        self.elapsed
    }

    /// Seconds the wall clock moved, which is what the organizer meant.
    #[must_use]
    pub const fn wall_clock_seconds(self) -> i64 {
        self.wall_clock
    }

    /// Whether a transition fell between the two instants, so the two counts differ.
    #[must_use]
    pub const fn crossed_a_transition(self) -> bool {
        self.elapsed != self.wall_clock
    }

    /// Both counts for the move from `from` to `to`, read under the zone `tzid` identifies.
    ///
    /// The elapsed count is [`Instant::checked_seconds_until`] and nothing else: two instants
    /// are two points on one timeline whatever any zone thinks. The wall-clock count is taken by
    /// reading each instant through [`ZoneSource::offset_at`] into civil fields and differencing
    /// *those* — through [`nominal`], which is the same projection the seam with `ical-recur`
    /// uses. Differencing the clocks rather than subtracting the two offsets is deliberate: the
    /// arithmetic is then `docs/adr/0011`'s checked civil arithmetic, days and months and years
    /// included, and a leap second is folded onto the second before it identically at both ends.
    ///
    /// Direction is preserved. A backwards move reports two negative counts, and
    /// [`WallClockShift::crossed_a_transition`] still answers about their disagreement rather
    /// than about their sign.
    ///
    /// `None` for exactly three reasons, and they are not distinguished here because a caller
    /// can ask about each directly: the source does not recognize `tzid` at either end, the
    /// difference of the two instants does not fit an `i64`, or an instant lies outside the
    /// years RFC 5545 section 3.3.4 can write so it has no wall clock at all.
    ///
    /// What the two numbers do *not* carry is [`AnswerBasis`]. A zone asked past the end of its
    /// transitions still answers, so a shift measured out there is a real measurement of a
    /// continued observance; the fact that it was continued is on the [`OffsetAnswer`] a caller
    /// gets from [`ZoneSource::offset_at`] itself, and inventing a third field here would
    /// duplicate a fact that already has a home and a diagnostic code.
    ///
    /// [`AnswerBasis`]: crate::AnswerBasis
    /// [`OffsetAnswer`]: crate::OffsetAnswer
    /// [`ZoneSource::offset_at`]: crate::ZoneSource::offset_at
    #[must_use]
    pub fn measure<S: ZoneSource + ?Sized>(
        source: &S,
        tzid: &str,
        from: Instant,
        to: Instant,
    ) -> Option<Self> {
        let elapsed = from.checked_seconds_until(to)?;
        let started = nominal_clock(source, tzid, from)?;
        let ended = nominal_clock(source, tzid, to)?;
        let wall_clock = started.checked_seconds_until(ended)?;
        Some(Self::new(elapsed, wall_clock))
    }

    /// Both counts for a move between two cadence keys of `series`.
    ///
    /// [`WallClockShift::measure`] takes two real UTC instants, and the two instants an override
    /// actually carries — its `RECURRENCE-ID` and where it moved to — are neither: everything
    /// crossing the seam is on the series' own wall clock projected onto UTC (see
    /// [`crate::seam`]), five hours from the real instants in New York. Fed the values the seam
    /// carries, `measure` read the offsets at the wrong two points and answered that a move
    /// straddling a spring-forward crossed no transition at all — the one question the type
    /// exists to answer, about the one case it was written for.
    ///
    /// This is the conversion, in the crate that owns it: each key is read back into a wall
    /// clock, resolved against the zone under the series' own [`crate::ResolutionPolicy`], and
    /// the two real instants are what get measured. So `elapsed_seconds` is the time the move
    /// really costs and `wall_clock_seconds` is what the organizer saw, which for a move from
    /// 09:00 on the day before a spring-forward to 04:00 on the day after is 64,800 against
    /// 68,400.
    ///
    /// `None` when either key names no instant this policy takes — a wall clock in a gap under
    /// [`GapPolicy::Skip`] — or on the terms `measure` gives.
    ///
    /// [`GapPolicy::Skip`]: crate::GapPolicy::Skip
    #[must_use]
    pub fn across<S: ZoneSource + ?Sized>(
        series: &ZonedSeries<'_, S>,
        from_key: Instant,
        to_key: Instant,
    ) -> Option<Self> {
        let from = series.resolved(from_key)?;
        let to = series.resolved(to_key)?;
        Self::measure(series.source(), series.tzid(), from, to)
    }
}

/// The wall clock `instant` shows under `tzid`, as an instant on the nominal timeline.
///
/// Two civil times cannot be subtracted from one another, and this crate already owns the one
/// projection that turns a wall clock into something that can be: [`nominal`]. Going through it
/// rather than through a second convention here is what keeps the difference this returns the
/// same arithmetic the seam with `ical-recur` is stated in.
fn nominal_clock<S: ZoneSource + ?Sized>(
    source: &S,
    tzid: &str,
    instant: Instant,
) -> Option<Instant> {
    let answer = source.offset_at(tzid, instant)?;
    let local = CivilDateTime::from_instant(instant, answer.offset)?;
    nominal(local)
}

/// The seconds to add to `ical_recur::max_absolute_shift`'s answer so no shifted start escapes.
///
/// `ical-recur` widens its generation window by the largest *elapsed* shift its override slice
/// implies, and admits an occurrence by its effective start. Under a zone the shift a later
/// occurrence actually takes is the organizer's wall-clock move, whose elapsed cost differs by
/// the size of any transition between the two — so a window widened by elapsed seconds alone can
/// be short, and an occurrence it fails to generate is not reported by anything, because nothing
/// generated it. This is the difference, measured over the shifts a caller has:
///
/// ```text
/// widening a zoned series needs = max over shifts of max(|elapsed|, |wall clock|)
/// widening ical-recur already applies = max over shifts of |elapsed|
/// ```
///
/// and the answer is the first minus the second, which is never negative. That is the whole
/// guarantee this function makes: it can only *add*. A caller measuring fewer shifts than
/// `ical-recur` scanned still gets a widening at least as wide as it needs, because the term
/// being subtracted is then smaller than the one `ical-recur` used and the sum comes out wider
/// than exact. Wide costs candidates a budget already bounds; narrow loses an occurrence in
/// silence.
///
/// Zero for an empty slice and zero when every shift agrees with itself, which is every series
/// no transition falls inside — the common case, and the one worth paying nothing for.
/// Saturating at [`i64::MAX`] rather than wrapping, matching `ical_recur::max_absolute_shift`:
/// a widening that does not fit an `i64` cannot produce a window that fits one either, and
/// `ical_recur::generation_window` answers `None` for it a moment later rather than generating
/// over a window that quietly shrank.
#[must_use]
pub fn extra_widening(shifts: &[WallClockShift]) -> i64 {
    let mut by_elapsed = 0_u64;
    let mut needed = 0_u64;
    for shift in shifts {
        let elapsed = shift.elapsed.unsigned_abs();
        let clock = shift.wall_clock.unsigned_abs();
        by_elapsed = by_elapsed.max(elapsed);
        needed = needed.max(elapsed.max(clock));
    }
    // `needed` is a maximum over terms each of which is at least its own `elapsed`, so it is
    // never below `by_elapsed` and the subtraction is a difference rather than a clamp.
    i64::try_from(needed.saturating_sub(by_elapsed)).unwrap_or(i64::MAX)
}

/// Which `RECURRENCE-ID` values a search never produced a cadence key for.
///
/// Fed as the search runs rather than after it, so nothing is materialized and a lazy iterator
/// over a window stays lazy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanScan<'a> {
    /// The identifiers to account for, strictly ascending.
    recurrence_ids: &'a [Instant],
    /// Whether each has been seen, parallel to `recurrence_ids`.
    seen: Vec<bool>,
}

impl<'a> OrphanScan<'a> {
    /// A scan over `recurrence_ids`, which must be strictly ascending.
    ///
    /// Required ascending rather than sorted here, for `ical_recur::OverrideSet`'s reason: the
    /// caller already holds them in that order, and sorting them silently would hide both an
    /// allocation and the fact that they were not.
    #[must_use]
    pub fn new(recurrence_ids: &'a [Instant]) -> Self {
        let mut seen = Vec::new();
        seen.resize(recurrence_ids.len(), false);
        Self {
            recurrence_ids,
            seen,
        }
    }

    /// The identifiers being accounted for.
    #[must_use]
    pub const fn recurrence_ids(&self) -> &'a [Instant] {
        self.recurrence_ids
    }

    /// How many identifiers have not been seen yet.
    #[must_use]
    pub fn unmatched(&self) -> usize {
        self.seen.iter().filter(|marked| !**marked).count()
    }

    /// Record that the search emitted the cadence key `key`.
    ///
    /// A binary search into the ascending identifiers and a flag set beside it: the cost is
    /// logarithmic in the override count per occurrence and nothing is retained, which is what
    /// lets this be called from inside the iterator a caller is already consuming lazily.
    ///
    /// A key that matches no identifier is not an error and not a fact worth recording. It is
    /// the ordinary case — most occurrences of a series are not overridden — and an override
    /// list is a claim about some instants rather than about all of them. Observing one key
    /// twice is likewise harmless: the flag is set, not counted.
    pub fn observe(&mut self, key: Instant) {
        let Ok(index) = self.recurrence_ids.binary_search(&key) else {
            return;
        };
        if let Some(marked) = self.seen.get_mut(index) {
            *marked = true;
        }
    }

    /// Report every identifier the search never produced, and answer how many there were.
    ///
    /// One [`Diagnostic::at_instant`] per unseen identifier, at that identifier's own instant,
    /// under [`DiagnosticCode::OverrideMatchesNoInstance`] at [`Severity::Violation`] — the
    /// severity the golden list fixes, because RFC 5545 section 3.8.4.4 defines a
    /// `RECURRENCE-ID` as naming an instance of the series it sits in, and one that names none
    /// is a file saying something untrue rather than something unusual. Emission goes through
    /// `report_diagnostic`, so a sink that refuses is charged to `meter` and the refusal is
    /// counted where the sink cannot count it.
    ///
    /// The count is returned rather than left implicit because of what this scan cannot see. An
    /// identifier can go unmatched for two different reasons — the rule never generates that
    /// instant, or the search was bounded by a window that instant lies outside — and the scan
    /// has no way to tell them apart, since it is handed keys and never the question that
    /// produced them. A caller that searched the whole series may read the count as the number
    /// of genuinely inert overrides; a caller that searched one month may not, and gets a number
    /// it can compare against its own bounds instead of a claim it did not earn.
    pub fn finish<D: DiagnosticSink + ?Sized>(self, meter: &mut Meter, sink: &mut D) -> u32 {
        let mut orphans = 0_u32;
        // The slice reference is taken out first so that walking it beside the flags is not a
        // borrow of `self` competing with the move of the flags themselves.
        let ids = self.recurrence_ids;
        let unseen = ids
            .iter()
            .copied()
            .zip(self.seen)
            .filter(|(_, marked)| !*marked)
            .map(|(identifier, _)| identifier);
        for identifier in unseen {
            orphans = orphans.saturating_add(1);
            let inert = Diagnostic::at_instant(
                DiagnosticCode::OverrideMatchesNoInstance,
                Severity::Violation,
                identifier,
            );
            report_diagnostic(sink, meter, inert);
        }
        orphans
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics,
        Instant, Limits, Meter, Severity, UtcOffset,
    };

    use super::{OrphanScan, WallClockShift, extra_widening};
    use crate::answer::{
        AnswerBasis, LocalResolution, OffsetAnswer, Reading, ZoneAnswer, ZoneProvenance, ZoneSource,
    };
    use crate::seam::nominal;

    // The offsets these zones run, in seconds east of UTC.

    /// `America/New_York`'s standard offset, `-05:00`.
    const NEW_YORK_STANDARD: i32 = -18_000;
    /// `America/New_York`'s daylight offset, `-04:00`.
    const NEW_YORK_DAYLIGHT: i32 = -14_400;
    /// `Europe/Berlin`'s standard offset, `+01:00`.
    const BERLIN_STANDARD: i32 = 3600;
    /// `Europe/Berlin`'s daylight offset, `+02:00`.
    const BERLIN_DAYLIGHT: i32 = 7200;
    /// `Australia/Lord_Howe`'s standard offset, `+10:30`.
    const LORD_HOWE_STANDARD: i32 = 37_800;
    /// `Australia/Lord_Howe`'s daylight offset, `+11:00` — half an hour, not a whole one.
    const LORD_HOWE_DAYLIGHT: i32 = 39_600;

    // America/New_York's transitions under both of the rules it has had. The rule changed for
    // 2007: before it, daylight time began on the first Sunday in April and ended on the last
    // Sunday in October; after it, the second Sunday in March and the first in November. Both
    // regimes are carried and the twenty years between them are not, because a fixture that
    // invented 2015's transitions would still be a fixture — no case below asks about a year
    // this table does not state.

    /// 2006-04-02T07:00:00Z: 02:00 local, the first Sunday in April, under the old rule.
    const NEW_YORK_APRIL_RULE_2006: i64 = 1_143_961_200;
    /// 2006-10-29T06:00:00Z: 02:00 local daylight, the last Sunday in October, old rule.
    const NEW_YORK_OCTOBER_RULE_2006: i64 = 1_162_101_600;
    /// 2026-03-08T07:00:00Z: 02:00 local, the second Sunday in March, under the rule since 2007.
    const NEW_YORK_MARCH_RULE_2026: i64 = 1_772_953_200;
    /// 2026-11-01T06:00:00Z: 02:00 local daylight, the first Sunday in November.
    const NEW_YORK_NOVEMBER_RULE_2026: i64 = 1_793_512_800;

    /// 2026-03-29T01:00:00Z: the EU rule moves at 01:00 UTC, not at a local hour.
    const BERLIN_MARCH_2026: i64 = 1_774_746_000;
    /// 2026-10-25T01:00:00Z: the last Sunday in October, again at 01:00 UTC.
    const BERLIN_OCTOBER_2026: i64 = 1_792_890_000;

    /// 2026-04-04T15:00:00Z: 02:00 local on the first Sunday in April, back by thirty minutes.
    const LORD_HOWE_APRIL_2026: i64 = 1_775_314_800;
    /// 2026-10-03T15:30:00Z: 02:00 local on the first Sunday in October, forward by thirty.
    const LORD_HOWE_OCTOBER_2026: i64 = 1_791_041_400;

    /// 2029-03-25T01:00:00Z: the third of four `RDATE` transitions a hand-built zone lists.
    const CUSTOM_MARCH_2029: i64 = 1_869_094_800;
    /// 2029-10-28T01:00:00Z: the last one, and the last instant that zone has data for.
    const CUSTOM_OCTOBER_2029: i64 = 1_887_843_600;

    /// `America/New_York`, as a file that remembers both of its rules would carry it.
    const NEW_YORK_ROWS: &[(i64, i32, bool)] = &[
        (NEW_YORK_APRIL_RULE_2006, NEW_YORK_DAYLIGHT, true),
        (NEW_YORK_OCTOBER_RULE_2006, NEW_YORK_STANDARD, false),
        (NEW_YORK_MARCH_RULE_2026, NEW_YORK_DAYLIGHT, true),
        (NEW_YORK_NOVEMBER_RULE_2026, NEW_YORK_STANDARD, false),
    ];

    /// `Europe/Berlin` across 2026.
    const BERLIN_ROWS: &[(i64, i32, bool)] = &[
        (BERLIN_MARCH_2026, BERLIN_DAYLIGHT, true),
        (BERLIN_OCTOBER_2026, BERLIN_STANDARD, false),
    ];

    /// `Australia/Lord_Howe` across 2026, where the southern year starts in daylight time.
    const LORD_HOWE_ROWS: &[(i64, i32, bool)] = &[
        (LORD_HOWE_APRIL_2026, LORD_HOWE_STANDARD, false),
        (LORD_HOWE_OCTOBER_2026, LORD_HOWE_DAYLIGHT, true),
    ];

    /// A zone whose transitions are four dates and no rule, so its table simply runs out.
    const CUSTOM_ROWS: &[(i64, i32, bool)] = &[
        (BERLIN_MARCH_2026, BERLIN_DAYLIGHT, true),
        (BERLIN_OCTOBER_2026, BERLIN_STANDARD, false),
        (CUSTOM_MARCH_2029, BERLIN_DAYLIGHT, true),
        (CUSTOM_OCTOBER_2029, BERLIN_STANDARD, false),
    ];

    /// One zone as a `VTIMEZONE` states it: an identifier written exactly as a file writes it,
    /// the state before the first transition, the transitions in ascending order, and the last
    /// instant the table is backed by data rather than continued.
    #[derive(Clone, Copy, Debug)]
    struct Zone {
        /// The identifier, compared by exact bytes and never parsed.
        tzid: &'static str,
        /// The offset and daylight flag in force before the first transition.
        initial: (i32, bool),
        /// When the zone moved, what offset it ran afterwards, and whether that is daylight.
        rows: &'static [(i64, i32, bool)],
        /// The last instant with real data behind it, absent when the zone's rules run on.
        known_through: Option<i64>,
    }

    /// The zones a test source knows. `Customized Time Zone` is not an IANA name and is
    /// recognized anyway; `W. Europe Standard Time` is not here at all, which is what an
    /// unrecognized identifier looks like from the outside.
    const ZONES: &[Zone] = &[
        Zone {
            tzid: "America/New_York",
            initial: (NEW_YORK_STANDARD, false),
            rows: NEW_YORK_ROWS,
            known_through: None,
        },
        Zone {
            tzid: "Europe/Berlin",
            initial: (BERLIN_STANDARD, false),
            rows: BERLIN_ROWS,
            known_through: None,
        },
        Zone {
            tzid: "Australia/Lord_Howe",
            initial: (LORD_HOWE_DAYLIGHT, true),
            rows: LORD_HOWE_ROWS,
            known_through: None,
        },
        Zone {
            tzid: "Customized Time Zone",
            initial: (BERLIN_STANDARD, false),
            rows: CUSTOM_ROWS,
            known_through: Some(CUSTOM_OCTOBER_2029),
        },
    ];

    impl Zone {
        /// The offset and daylight flag in force at `instant`.
        fn state_at(self, instant: Instant) -> (i32, bool) {
            let mut state = self.initial;
            for (moment, offset, daylight) in self.rows.iter().copied() {
                if instant.unix_seconds() < moment {
                    break;
                }
                state = (offset, daylight);
            }
            state
        }

        /// How much of this zone stood behind an answer about `instant`.
        fn basis(self, instant: Instant) -> AnswerBasis {
            match self.known_through {
                Some(end) if instant.unix_seconds() > end => {
                    let last = Instant::from_unix_seconds(end);
                    let known = CivilDateTime::from_instant(last, UtcOffset::UTC).unwrap();
                    AnswerBasis::BeyondKnownTransitions(known.date())
                },
                _ => AnswerBasis::Computed,
            }
        }

        /// Every distinct offset this zone has ever run, which is the candidate set a local
        /// time is read against.
        fn offsets(self) -> Vec<i32> {
            let mut distinct: Vec<i32> = Vec::new();
            distinct.push(self.initial.0);
            for (_, offset, _) in self.rows.iter().copied() {
                if !distinct.contains(&offset) {
                    distinct.push(offset);
                }
            }
            distinct
        }

        /// The readings `local` survives: an offset names an instant, and the instant has to
        /// read back under the same offset for that naming to be true.
        fn readings_of(self, local: CivilDateTime) -> Vec<Reading> {
            let mut kept: Vec<Reading> = Vec::new();
            for seconds in self.offsets() {
                let Some(offset) = UtcOffset::from_seconds(seconds) else {
                    continue;
                };
                let Some(instant) = local.at_offset(offset) else {
                    continue;
                };
                let (in_force, daylight) = self.state_at(instant);
                if in_force == seconds && !kept.iter().any(|seen| seen.instant == instant) {
                    kept.push(Reading::new(instant, offset, daylight));
                }
            }
            kept.sort_unstable_by_key(|reading| reading.instant);
            kept
        }

        /// The gap `local` fell into, when no offset named it.
        ///
        /// `gap_start` is the last instant the old offset was in force and `gap_end` the
        /// transition itself, so clamping to it puts the event at the first instant it can
        /// happen, and `shifted` is the RFC 5545 section 3.3.5 reading.
        fn gap_for(self, local: CivilDateTime) -> Option<LocalResolution> {
            let wanted = nominal(local)?;
            let mut before = self.initial.0;
            for (moment, offset, _) in self.rows.iter().copied() {
                let opened = moment.checked_add(i64::from(before))?;
                let closed = moment.checked_add(i64::from(offset))?;
                if opened <= wanted.unix_seconds() && wanted.unix_seconds() < closed {
                    let transition = Instant::from_unix_seconds(moment);
                    let offset_before = UtcOffset::from_seconds(before)?;
                    return Some(LocalResolution::Nonexistent {
                        gap_start: transition.checked_add_seconds(-1)?,
                        gap_end: transition,
                        offset_before,
                        offset_after: UtcOffset::from_seconds(offset)?,
                        shifted: local.at_offset(offset_before)?,
                    });
                }
                before = offset;
            }
            None
        }
    }

    /// The zone `tzid` names, by exact bytes and never by anything parsed out of it.
    fn zone_named(tzid: &str) -> Option<Zone> {
        ZONES.iter().copied().find(|zone| zone.tzid == tzid)
    }

    /// A source over a fixed set of zones, which is what a caller wiring its own database has.
    #[derive(Clone, Copy, Debug)]
    struct Zones;

    impl ZoneSource for Zones {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            let zone = zone_named(tzid)?;
            let readings = zone.readings_of(local);
            let resolution = match readings.as_slice() {
                [] => zone.gap_for(local)?,
                [only] => LocalResolution::Unique { reading: *only },
                [earlier, later, ..] => LocalResolution::Ambiguous {
                    earlier: *earlier,
                    later: *later,
                },
            };
            let asked_about = nominal(local)?;
            Some(ZoneAnswer::new(
                resolution,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis(asked_about),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            let zone = zone_named(tzid)?;
            let (seconds, daylight) = zone.state_at(instant);
            Some(OffsetAnswer::new(
                UtcOffset::from_seconds(seconds)?,
                daylight,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis(instant),
            ))
        }
    }

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    fn local(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        CivilDateTime::new(date, CivilTime::from_hms(hour, minute, 0).unwrap())
    }

    /// How many instants a wall clock names under a zone.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Names {
        /// Exactly one, which is every day of the year but two.
        Once(i64),
        /// Two, the earlier of which is this one.
        Twice(i64),
        /// None at all.
        Never,
    }

    // The two ends of each move a shift is measured over. Every one of them is a 09:00 local
    // start on consecutive days of a real zone, which is the series shape the seam with
    // `ical-recur` exists for.

    /// 2026-03-07T14:00:00Z — 09:00 in New York, the day before the clocks move.
    const NEW_YORK_BEFORE_MARCH: i64 = 1_772_892_000;
    /// 2026-03-08T13:00:00Z — 09:00 in New York, the day they moved.
    const NEW_YORK_AFTER_MARCH: i64 = 1_772_974_800;
    /// 2026-03-08T14:00:00Z — 09:00 on that day read with the *winter* offset, which is what a
    /// client writes into a `RECURRENCE-ID` when it forgets the transition.
    const NEW_YORK_MISREAD_MARCH: i64 = 1_772_978_400;
    /// 2026-03-05T14:00:00Z — 09:00 on an ordinary March day.
    const NEW_YORK_ORDINARY_DAY: i64 = 1_772_719_200;
    /// 2026-03-06T14:00:00Z — 09:00 on the next one.
    const NEW_YORK_ORDINARY_NEXT: i64 = 1_772_805_600;
    /// 2006-04-01T14:00:00Z — 09:00 the day before the *old* rule moved the clocks.
    const NEW_YORK_BEFORE_APRIL_2006: i64 = 1_143_900_000;
    /// 2006-04-02T13:00:00Z — 09:00 the day it did.
    const NEW_YORK_AFTER_APRIL_2006: i64 = 1_143_982_800;
    /// 2026-04-01T13:00:00Z — 09:00 on the same calendar day twenty years later.
    const NEW_YORK_APRIL_FIRST_2026: i64 = 1_775_048_400;
    /// 2026-04-02T13:00:00Z — and the day after it.
    const NEW_YORK_APRIL_SECOND_2026: i64 = 1_775_134_800;

    /// 2026-10-24T07:00:00Z — 09:00 in Berlin, the day before the clocks go back.
    const BERLIN_BEFORE_OCTOBER: i64 = 1_792_825_200;
    /// 2026-10-25T08:00:00Z — 09:00 the day they did.
    const BERLIN_AFTER_OCTOBER: i64 = 1_792_915_200;

    /// 2026-10-02T22:30:00Z — 09:00 on Lord Howe, the day before its half-hour move.
    const LORD_HOWE_BEFORE_OCTOBER: i64 = 1_790_980_200;
    /// 2026-10-03T22:00:00Z — 09:00 the day it moved forward by thirty minutes.
    const LORD_HOWE_AFTER_OCTOBER: i64 = 1_791_064_800;
    /// 2026-04-03T22:00:00Z — 09:00 the day before it moved back by thirty.
    const LORD_HOWE_BEFORE_APRIL: i64 = 1_775_253_600;
    /// 2026-04-04T22:30:00Z — 09:00 the day it did.
    const LORD_HOWE_AFTER_APRIL: i64 = 1_775_341_800;

    /// 2035-06-01T12:00:00Z — six years past the end of the hand-built zone's table.
    const CUSTOM_PAST_ITS_TABLE: i64 = 2_064_312_000;
    /// 2035-06-02T12:00:00Z — the day after it, equally past the end.
    const CUSTOM_PAST_ITS_TABLE_NEXT: i64 = 2_064_398_400;

    /// One hour of elapsed time, and one of wall clock, which are the same number only usually.
    const ONE_HOUR: i64 = 3600;
    /// One day of wall clock.
    const ONE_DAY: i64 = 86_400;

    /// The two counts disagree exactly where a real zone moved, and agree everywhere else.
    ///
    /// Every row is a 09:00 local start moved to 09:00 local the next day — one day on the wall
    /// clock, by construction — so the elapsed column is the whole content of the test: it is 23
    /// hours where the clocks went forward, 25 where they went back, and 23.5 or 24.5 on Lord
    /// Howe, whose daylight saving is half an hour. The expectations are the published
    /// transition rules of those zones and not anything this code computed.
    #[test]
    fn a_shift_is_two_numbers_wherever_a_real_zone_moved() {
        // The move, the zone it happened in, its two ends, and the two counts it takes.
        let cases = [
            (
                "New York's second Sunday in March: a day of clock costs 23 hours of timeline",
                "America/New_York",
                NEW_YORK_BEFORE_MARCH,
                NEW_YORK_AFTER_MARCH,
                (82_800, ONE_DAY),
            ),
            (
                "the same series on two ordinary March days, where the counts are one number",
                "America/New_York",
                NEW_YORK_ORDINARY_DAY,
                NEW_YORK_ORDINARY_NEXT,
                (ONE_DAY, ONE_DAY),
            ),
            (
                "Berlin's last Sunday in October, which moves at 01:00 UTC and costs 25 hours",
                "Europe/Berlin",
                BERLIN_BEFORE_OCTOBER,
                BERLIN_AFTER_OCTOBER,
                (90_000, ONE_DAY),
            ),
            (
                "Lord Howe going forward by half an hour, not by a whole one",
                "Australia/Lord_Howe",
                LORD_HOWE_BEFORE_OCTOBER,
                LORD_HOWE_AFTER_OCTOBER,
                (84_600, ONE_DAY),
            ),
            (
                "Lord Howe going back by half an hour",
                "Australia/Lord_Howe",
                LORD_HOWE_BEFORE_APRIL,
                LORD_HOWE_AFTER_APRIL,
                (88_200, ONE_DAY),
            ),
            (
                "New York's first Sunday in April under the rule it used before 2007",
                "America/New_York",
                NEW_YORK_BEFORE_APRIL_2006,
                NEW_YORK_AFTER_APRIL_2006,
                (82_800, ONE_DAY),
            ),
            (
                "the same calendar days in 2026, when March has already moved the clock and \
                 April moves nothing",
                "America/New_York",
                NEW_YORK_APRIL_FIRST_2026,
                NEW_YORK_APRIL_SECOND_2026,
                (ONE_DAY, ONE_DAY),
            ),
            (
                "the March move read backwards, which is negative in both counts",
                "America/New_York",
                NEW_YORK_AFTER_MARCH,
                NEW_YORK_BEFORE_MARCH,
                (-82_800, -86_400),
            ),
        ];

        for (shape, tzid, from, to, (elapsed, clock)) in cases {
            let measured = WallClockShift::measure(&Zones, tzid, at(from), at(to));
            assert_eq!(
                measured,
                Some(WallClockShift::new(elapsed, clock)),
                "{shape}"
            );
            let shift = measured.unwrap();
            assert_eq!(shift.elapsed_seconds(), elapsed, "{shape}");
            assert_eq!(shift.wall_clock_seconds(), clock, "{shape}");
            assert_eq!(
                shift.crossed_a_transition(),
                elapsed != clock,
                "{shape}: a transition between the two ends is exactly a disagreement"
            );
        }
    }

    /// An identifier nobody knows has no shift; one past the end of a table still has one.
    ///
    /// The two answers a caller must be able to tell apart. `W. Europe Standard Time` is a real
    /// `TZID` from a real client and this source does not recognize it, so there is nothing to
    /// measure and nothing is invented. The hand-built zone *is* recognized, its transitions run
    /// out in 2029, and a question about 2035 is still answered — with a basis that says the
    /// observance was continued, which the caller reads off `offset_at` rather than off the two
    /// numbers.
    #[test]
    fn an_unknown_identifier_measures_nothing_and_an_exhausted_table_still_measures() {
        let unknown = WallClockShift::measure(
            &Zones,
            "W. Europe Standard Time",
            at(NEW_YORK_BEFORE_MARCH),
            at(NEW_YORK_AFTER_MARCH),
        );
        assert_eq!(unknown, None, "an unrecognized TZID is never a default");

        let tzid = "Customized Time Zone";
        let past = WallClockShift::measure(
            &Zones,
            tzid,
            at(CUSTOM_PAST_ITS_TABLE),
            at(CUSTOM_PAST_ITS_TABLE_NEXT),
        );
        assert_eq!(past, Some(WallClockShift::new(ONE_DAY, ONE_DAY)));

        let answer = Zones.offset_at(tzid, at(CUSTOM_PAST_ITS_TABLE)).unwrap();
        assert_eq!(
            answer.basis,
            AnswerBasis::BeyondKnownTransitions(CivilDate::from_ymd(2029, 10, 28).unwrap()),
            "the measurement holds and the fact that it continued a dead table travels beside it"
        );
        assert_eq!(
            answer.basis.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneCoverageExhausted)
        );
    }

    /// The widening only ever grows, and it grows by exactly what the elapsed count missed.
    ///
    /// The first three rows are the shifts the zones above produce: an override that moved a
    /// meeting one hour of wall clock across New York's spring forward moved it two, and
    /// `ical_recur::max_absolute_shift` would have widened by the one hour it can see.
    #[test]
    fn the_extra_widening_covers_what_an_elapsed_count_could_not_see() {
        let nothing: [WallClockShift; 0] = [];
        let agreed = [WallClockShift::new(ONE_HOUR, ONE_HOUR)];
        let sprang = [WallClockShift::new(ONE_HOUR, 7200)];
        let backwards = [WallClockShift::new(-3600, -7200)];
        let fell_back = [WallClockShift::new(90_000, ONE_DAY)];
        let dominated = [
            WallClockShift::new(ONE_DAY, ONE_DAY),
            WallClockShift::new(ONE_HOUR, 7200),
        ];
        let unrepresentable = [WallClockShift::new(0, i64::MIN)];
        let elapsed_is_huge = [WallClockShift::new(i64::MIN, 0)];

        // The slice of shifts, and the seconds a caller adds to `max_absolute_shift`'s answer.
        let cases = [
            ("a series with no shift at all", &nothing[..], 0),
            ("a move no transition fell inside", &agreed[..], 0),
            (
                "an hour of clock across a spring forward, which is two hours of clock",
                &sprang[..],
                ONE_HOUR,
            ),
            ("the same move read backwards", &backwards[..], ONE_HOUR),
            (
                "an hour of clock across a fall back, where elapsed already covers it",
                &fell_back[..],
                0,
            ),
            (
                "a larger override elsewhere that already widened past both counts",
                &dominated[..],
                0,
            ),
            (
                "a wall clock count that does not fit an i64 saturates rather than wrapping",
                &unrepresentable[..],
                i64::MAX,
            ),
            (
                "an elapsed count larger than the clock, which needs nothing added",
                &elapsed_is_huge[..],
                0,
            ),
        ];

        for (shape, shifts, expected) in cases {
            let extra = extra_widening(shifts);
            assert_eq!(extra, expected, "{shape}");
            assert!(extra >= 0, "{shape}: a widening is never narrowed");
        }
    }

    /// An override addressing an instant the series never generates is reported at that instant.
    ///
    /// The real shape of agenda item 6, taken from the zone above: a client wrote a
    /// `RECURRENCE-ID` for 09:00 on the morning New York's clocks moved and computed it with the
    /// winter offset, so it names 14:00Z and the series generates 13:00Z. Nothing else in these
    /// crates would have said a word about it.
    #[test]
    fn an_override_that_matches_no_generated_key_is_reported_at_its_own_instant() {
        let identifiers = [at(NEW_YORK_BEFORE_MARCH), at(NEW_YORK_MISREAD_MARCH)];
        let mut scan = OrphanScan::new(&identifiers);
        assert_eq!(scan.recurrence_ids(), &identifiers[..]);
        assert_eq!(
            scan.unmatched(),
            2,
            "nothing is seen before the search runs"
        );

        scan.observe(at(NEW_YORK_BEFORE_MARCH));
        scan.observe(at(NEW_YORK_BEFORE_MARCH));
        assert_eq!(scan.unmatched(), 1, "observing one key twice is one key");
        scan.observe(at(NEW_YORK_AFTER_MARCH));
        assert_eq!(
            scan.unmatched(),
            1,
            "the key the series really generates matches no identifier, which is ordinary"
        );

        let mut ledger = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        assert_eq!(scan.finish(&mut ledger, &mut sink), 1);
        assert_eq!(sink.len(), 1);
        let inert = sink.first().unwrap();
        assert_eq!(inert.code(), DiagnosticCode::OverrideMatchesNoInstance);
        assert_eq!(inert.severity(), Severity::Violation);
        assert_eq!(inert.instant(), Some(at(NEW_YORK_MISREAD_MARCH)));
        assert_eq!(ledger.diagnostics_dropped(), 0);
    }

    /// A scan with nothing to say says nothing, and a refusing sink still yields the count.
    ///
    /// The second half is why `finish` answers with a number: a caller holding
    /// `IgnoreDiagnostics` has lost *which* overrides were inert and must not also lose *that*
    /// they were.
    #[test]
    fn the_count_survives_a_sink_that_keeps_nothing() {
        let mut ledger = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();

        let none: [Instant; 0] = [];
        let mut empty = OrphanScan::new(&none);
        empty.observe(at(NEW_YORK_BEFORE_MARCH));
        assert_eq!(empty.finish(&mut ledger, &mut sink), 0);
        assert!(sink.is_empty());

        let identifiers = [at(NEW_YORK_BEFORE_MARCH), at(NEW_YORK_MISREAD_MARCH)];
        let refused = OrphanScan::new(&identifiers);
        assert_eq!(refused.finish(&mut ledger, &mut IgnoreDiagnostics), 2);
        assert_eq!(
            ledger.diagnostics_dropped(),
            2,
            "a sink that keeps nothing cannot remember how much it did not keep"
        );
    }

    /// The zones these shifts are measured against are held to their real rules first.
    ///
    /// A fixture that agreed with the code under test and with nothing else would make every
    /// row above a tautology. So the two awkward hours are asserted directly: New York and
    /// Berlin repeat an hour and skip an hour, Lord Howe repeats and skips half of one, and the
    /// hand-built zone answers a question its table does not cover.
    #[test]
    fn the_zones_behind_those_shifts_fold_and_skip_where_their_rules_say() {
        // What the local time is, the zone it is read in, and what it names there.
        let cases = [
            (
                "01:30 on New York's first Sunday in November, which happens twice",
                "America/New_York",
                local(2026, 11, 1, 1, 30),
                Names::Twice(1_793_511_000),
            ),
            (
                "02:30 on its second Sunday in March, which happens never",
                "America/New_York",
                local(2026, 3, 8, 2, 30),
                Names::Never,
            ),
            (
                "02:30 on Berlin's last Sunday in October, which happens twice",
                "Europe/Berlin",
                local(2026, 10, 25, 2, 30),
                Names::Twice(1_792_888_200),
            ),
            (
                "02:30 on its last Sunday in March, which happens never",
                "Europe/Berlin",
                local(2026, 3, 29, 2, 30),
                Names::Never,
            ),
            (
                "01:45 on Lord Howe's first Sunday in April, inside a half-hour fold",
                "Australia/Lord_Howe",
                local(2026, 4, 5, 1, 45),
                Names::Twice(1_775_313_900),
            ),
            (
                "02:15 on its first Sunday in October, inside a half-hour gap",
                "Australia/Lord_Howe",
                local(2026, 10, 4, 2, 15),
                Names::Never,
            ),
            (
                "an ordinary morning, which names one instant like every other day",
                "America/New_York",
                local(2026, 3, 5, 9, 0),
                Names::Once(NEW_YORK_ORDINARY_DAY),
            ),
        ];

        for (shape, tzid, when, expected) in cases {
            let answer = Zones.resolve(tzid, when).unwrap();
            match expected {
                Names::Once(seconds) => {
                    assert_eq!(
                        answer.resolution.unambiguous(),
                        Some(at(seconds)),
                        "{shape}"
                    );
                    assert_eq!(answer.resolution.diagnostic_code(), None, "{shape}");
                },
                Names::Twice(seconds) => {
                    assert!(answer.resolution.is_ambiguous(), "{shape}");
                    assert_eq!(answer.resolution.earliest(), Some(at(seconds)), "{shape}");
                    assert_eq!(answer.resolution.unambiguous(), None, "{shape}");
                },
                Names::Never => {
                    assert!(answer.resolution.is_nonexistent(), "{shape}");
                    assert_eq!(answer.resolution.earliest(), None, "{shape}");
                },
            }
        }

        assert_eq!(
            Zones.resolve("W. Europe Standard Time", local(2026, 3, 5, 9, 0)),
            None,
            "a TZID that is not an IANA name and that nobody wired in is unrecognized"
        );
    }
}
