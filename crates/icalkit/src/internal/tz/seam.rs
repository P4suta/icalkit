// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The seam between a resolution here and an instant in `ical-recur`.
//!
//! Specification: RFC 5545 section 3.3.10 (`UNTIL`), section 3.8.5.1 (`EXDATE`), section
//! 3.8.5.3 (`RRULE`), and section 3.6.5 (`VTIMEZONE`).
//!
//! # The contract
//!
//! `ical-recur` walks periods in civil fields read at UTC and emits cadence keys as
//! [`Instant`]s. It has no zone and cannot acquire one: it is this crate's sibling, and the
//! caller resolved `DTSTART` before the search began. A daily 09:00 series is therefore only
//! wall-clock-stable if something re-resolves each occurrence, and M1 left which side does
//! that unstated. This is that side.
//!
//! **The timeline `ical-recur` works on is the series' own wall clock projected onto UTC, and
//! not the UTC timeline.** Call an instant on it *nominal*. Every instant crossing the seam
//! into `ical-recur` — `DTSTART`, `UNTIL`, each `RDATE`, each `EXDATE`, each `RECURRENCE-ID` —
//! is nominal, and every cadence key coming back out is nominal. The projection is
//! [`nominal`] one way and [`wall_clock`] the other, and both are the identity on the offset
//! `UtcOffset::UTC`.
//!
//! What that buys is exactly the property M1 named. `ical-recur`'s period walk preserves the
//! civil fields it was handed, so a daily rule anchored at a nominal 09:00 emits a nominal
//! 09:00 every day of the year. Reading each key back through [`wall_clock`] gives 09:00
//! local, and resolving *that* through [`ZoneSource::resolve`] gives the instant it names
//! under the zone on that particular day — 08:00Z in winter, 07:00Z in summer, for a zone an
//! hour east of UTC. The series is stable on the wall clock because the wall clock is what was
//! generated, and the offsets are applied one occurrence at a time because that is the only
//! place a transition can be seen.
//!
//! Three consequences follow, and they are the answers this milestone owes.
//!
//! - **Who resolves what, and when.** This crate resolves twice per series and once per
//!   occurrence. Before the search: `DTSTART` and the rule's `UNTIL` are projected into
//!   nominal instants, along with every `RDATE`, `EXDATE` and `RECURRENCE-ID`. During the
//!   search: each cadence key `ical-recur` yields is read back and resolved, which is where a
//!   fold, a gap and an exhausted transition table are seen. `ical-recur` resolves nothing and
//!   is handed no zone.
//! - **A UTC value has to be projected too.** A `DTSTART` or an `UNTIL` written with a
//!   trailing `Z` names a real UTC instant, which is *not* on the nominal timeline. It is
//!   converted: instant, then the zone's offset at it, then the wall clock, then [`nominal`].
//!   Skipping that step is what makes a `Z`-terminated `UNTIL` cut a zoned series off an hour
//!   early or late for half the year.
//! - **A floating value is already nominal.** A wall clock read at UTC is precisely the
//!   projection, so a floating `UNTIL` needs no conversion at all — it needs a diagnostic,
//!   because RFC 5545 section 3.3.10 requires `UNTIL` to be UTC and the reading that recovers
//!   the producer's intent is `DTSTART`'s own zone. That is
//!   [`DiagnosticCode::RecurrenceUntilNotUtc`], and `UntilClock::Floating` in `ical-recur` is
//!   the same fact seen from the other side.
//!
//! # The frequency this contract is not stated for
//!
//! Everything above is written about a *civil* cadence — `FREQ=DAILY` and coarser, where "every
//! day at 09:00" is a statement about a clock and the day it steps over may be 23 or 25 hours
//! long. `FREQ=SECONDLY`, `FREQ=MINUTELY` and `FREQ=HOURLY` say the opposite thing: an hour is
//! an hour, however the zone behaves inside it. The contract above applied to one of those loses
//! an hour of the series on the day a zone falls back, because the wall clock reads twenty-four
//! hours on a day twenty-five hours long, and one nominal key names the repeated hour once.
//!
//! Both readings ship, and the disagreement is real rather than a defect anyone can point at:
//! Google's engine gives 25 occurrences that day and libical's local-time expansion gives 24.
//! This workspace states both and makes the caller pick, by making the *anchor* the choice.
//! [`ZonedSeries::anchor`] projects onto the nominal timeline and is what a civil cadence takes;
//! [`ZonedSeries::real_anchor`] resolves the same `DTSTART` into the real instant it names and
//! is what an absolute one takes. A series anchored on the real timeline is walked there — its
//! keys are already the instants its occurrences happen at, and [`ZonedSeries::actual`] is
//! neither needed nor correct for one.
//!
//! `ical-recur` cannot make that choice: it has the frequency and no zone, and this crate has
//! the zone and not the frequency. So it is the caller's, stated here rather than left to
//! whichever anchor a caller reached for first.
//!
//! [`ZonedSeries::anchor`]: crate::internal::tz::ZonedSeries::anchor
//! [`ZonedSeries::real_anchor`]: crate::internal::tz::ZonedSeries::real_anchor
//! [`ZonedSeries::actual`]: crate::internal::tz::ZonedSeries::actual
//!
//! # Where `COUNT` is applied, which is the other side of the same seam
//!
//! `docs/adr/0011` states two gates on an instance — a date that exists, and a local time that
//! exists — and says an instance is admitted only when both pass, while `COUNT` counts emitted
//! instances only. `ical-recur` owns the first gate and applies `COUNT`; this crate owns the
//! second and used to be applied after the count, so a `COUNT=5` series with one instance in an
//! hour its zone never showed delivered four and no API in either crate composed the two in the
//! stated order.
//!
//! [`ZonedSeries::admits`] is the second gate as a predicate, and
//! `ical_recur::RecurrenceInput::admitting` is where it goes: asked about each key after the
//! window and before the count, so a rejected key costs the series nothing. That is the whole
//! of the composition, and it is opt-in because a caller that wants section 3.3.10's other
//! reading — the instance is dropped and the count is spent, which is what a `DTSTART` in a gap
//! does under section 3.8.5.3 — states it by not passing a gate.
//!
//! [`ZonedSeries::admits`]: crate::internal::tz::ZonedSeries::admits
//!
//! The one place M1's shipped prose and this contract disagree is `UntilClock::Utc`'s doc
//! comment, which says the instant beside it "is that UTC instant". For a floating or UTC
//! series the projection is the identity and the sentence holds verbatim. For a zoned series
//! it does not: the instant is the projection and the variant records what the *file* wrote.
//! The divergence is reported here rather than routed around, and `ical-recur`'s own
//! documentation now carries the same paragraph.
//!
//! # What this module is and is not
//!
//! Types and projections only. The two-directional conversion is arithmetic over
//! `ical-core`'s checked primitives and belongs to no unit in particular, so it is settled
//! here where both ends can be held against it. Driving a series through it — anchoring
//! `DTSTART`, projecting `UNTIL`, resolving each key, expanding a whole-day exclusion — is the
//! work of the units above, which read their policy out of [`ResolutionPolicy`].
//!
//! [`ZoneSource::resolve`]: crate::internal::tz::ZoneSource::resolve
//! [`DiagnosticCode::RecurrenceUntilNotUtc`]: crate::internal::core::DiagnosticCode::RecurrenceUntilNotUtc

use crate::internal::core::{CivilDateTime, Instant, UtcOffset};

use crate::internal::tz::answer::{FoldPolicy, GapPolicy};

/// The wall clock `local` spells, as an instant on the nominal timeline.
///
/// The projection every instant crossing into `ical-recur` goes through. It is
/// `local.at_offset(UtcOffset::UTC)` and it is written as a named function rather than left at
/// each call site, because a seam whose one rule is spelled out six times is a seam with six
/// chances to be spelled differently.
///
/// `None` when the result leaves the timeline, which for a date RFC 5545 can write is
/// unreachable and is checked anyway.
#[must_use]
pub fn nominal(local: CivilDateTime) -> Option<Instant> {
    local.at_offset(UtcOffset::UTC)
}

/// The wall clock a nominal instant stands for.
///
/// The inverse of [`nominal`], and what turns a cadence key back into something
/// [`ZoneSource::resolve`] can be asked about. `None` when the instant is outside the years
/// RFC 5545 section 3.3.4 can write.
///
/// [`ZoneSource::resolve`]: crate::internal::tz::ZoneSource::resolve
#[must_use]
pub fn wall_clock(nominal_instant: Instant) -> Option<CivilDateTime> {
    CivilDateTime::from_instant(nominal_instant, UtcOffset::UTC)
}

/// A half-open span of the nominal timeline.
///
/// What a `DATE` becomes once a zone has expanded it: a whole day is not an instant, and the
/// exclusion and comparison questions RFC 5545 leaves open about a `DATE` are all questions
/// about an interval. Half-open — `start` included, `end` excluded — matching
/// `ical_recur::Window`, so that consecutive days tile without overlapping and midnight
/// belongs to exactly one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalInterval {
    /// The first instant in the span.
    start: Instant,
    /// The first instant past it.
    end: Instant,
}

impl LocalInterval {
    /// A span from `start` up to but not including `end`, or `None` when `end` is not later.
    ///
    /// An empty span is refused for the reason `ical_recur::Window::new` refuses one: a span
    /// that excludes nothing and a span nobody asked for are otherwise the same value.
    #[must_use]
    pub fn new(start: Instant, end: Instant) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    /// The first instant in the span.
    #[must_use]
    pub const fn start(self) -> Instant {
        self.start
    }

    /// The first instant past the span.
    #[must_use]
    pub const fn end(self) -> Instant {
        self.end
    }

    /// Whether `at` falls inside.
    #[must_use]
    pub fn contains(self, at: Instant) -> bool {
        self.start <= at && at < self.end
    }
}

/// Which instant of a named day an `UNTIL` written as a `DATE` stands for.
///
/// RFC 5545 section 3.3.10 requires `UNTIL` to agree with `DTSTART` about `DATE` against
/// `DATE-TIME` and real files disagree constantly, so the question is not whether to read a
/// mismatched `UNTIL` but where in the day to read it. Both answers are defensible and the RFC
/// picks neither: midnight is what libical, dateutil and the Google engine do, and it drops
/// the named day's own instances; end of day is what the person who typed the date meant.
///
/// This is therefore a resolution-time policy rather than a fixed reading. The default is
/// [`UntilReading::Midnight`], because interoperating with the three engines a calendar is
/// most likely to be exchanged with beats being right alone, and the other reading is one
/// named value away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum UntilReading {
    /// Midnight beginning the named day, so the day's own instances fall outside the series.
    #[default]
    Midnight,
    /// The last instant of the named day, so its instances are inside the series.
    EndOfDay,
}

/// What an `EXDATE` written as a `DATE` excludes from a series whose `DTSTART` is a date-time.
///
/// The mismatch RFC 5545 section 3.8.5.1 forbids and clients ship anyway, and the more
/// damaging of the two value-type mismatches. Read as an instant, a `DATE` exclusion resolves
/// to midnight, names an instant a 09:00 series does not have, and removes nothing at all —
/// so the exception the producer wrote vanishes and no arithmetic anywhere notices.
///
/// [`ExclusionReading::WholeDay`] is what several clients implement: the date names the day,
/// and every occurrence starting inside that day in the series' own zone is excluded. It needs
/// a zone, because a day is a wall-clock span and the span is 23, 24 or 25 hours long
/// depending on whether the zone moved inside it, which is why this policy lives in this crate
/// and not in `ical-recur`.
///
/// The default is [`ExclusionReading::Instantaneous`], because it is what the value literally
/// says and the other reading removes occurrences the file never named. Either way
/// [`DiagnosticCode::ExdateValueTypeMismatch`] travels, because a silent no-op is the one
/// outcome that is indefensible.
///
/// [`DiagnosticCode::ExdateValueTypeMismatch`]: crate::internal::core::DiagnosticCode::ExdateValueTypeMismatch
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ExclusionReading {
    /// The date names midnight, and removes an occurrence only if one starts exactly there.
    #[default]
    Instantaneous,
    /// The date names the whole day in the series' zone, and removes every occurrence in it.
    WholeDay,
}

/// Every reading this crate will not decide for a caller, in one value.
///
/// Four policies rather than four arguments threaded through six signatures. `Copy` with
/// private fields and `with_*` builders, in the shape `crate::internal::core::Limits` already uses, so
/// that adding a fifth reading is not a breaking change and no caller can construct a policy
/// that leaves one unstated.
///
/// The defaults are each the conservative reading — skip a gap, take the first of a fold, read
/// a `DATE` `UNTIL` at midnight, read a `DATE` `EXDATE` as an instant — and every one of them
/// is a reading the specification permits and some client disagrees with. A caller that states
/// nothing gets RFC 5545 section 3.3.10's MUST and the majority behavior of the engines it
/// will exchange files with; a caller that wants otherwise says so in one place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolutionPolicy {
    /// What to do with a wall clock the zone sprang over.
    gaps: GapPolicy,
    /// Which instant to take from a wall clock the zone fell back through.
    folds: FoldPolicy,
    /// Where in a named day an `UNTIL` written as a `DATE` sits.
    until: UntilReading,
    /// What an `EXDATE` written as a `DATE` excludes.
    exclusions: ExclusionReading,
}

impl ResolutionPolicy {
    /// The conservative reading of every question the specification leaves open.
    pub const DEFAULT: Self = Self {
        gaps: GapPolicy::Skip,
        folds: FoldPolicy::Earlier,
        until: UntilReading::Midnight,
        exclusions: ExclusionReading::Instantaneous,
    };

    /// What to do with a wall clock the zone sprang over.
    #[must_use]
    pub const fn gaps(self) -> GapPolicy {
        self.gaps
    }

    /// Which instant to take from a wall clock the zone fell back through.
    #[must_use]
    pub const fn folds(self) -> FoldPolicy {
        self.folds
    }

    /// Where in a named day an `UNTIL` written as a `DATE` sits.
    #[must_use]
    pub const fn until(self) -> UntilReading {
        self.until
    }

    /// What an `EXDATE` written as a `DATE` excludes.
    #[must_use]
    pub const fn exclusions(self) -> ExclusionReading {
        self.exclusions
    }

    /// The same policy with a different reading of a gap.
    #[must_use]
    pub const fn with_gaps(self, gaps: GapPolicy) -> Self {
        Self { gaps, ..self }
    }

    /// The same policy with a different reading of a fold.
    #[must_use]
    pub const fn with_folds(self, folds: FoldPolicy) -> Self {
        Self { folds, ..self }
    }

    /// The same policy with a different reading of a `DATE` `UNTIL`.
    #[must_use]
    pub const fn with_until(self, until: UntilReading) -> Self {
        Self { until, ..self }
    }

    /// The same policy with a different reading of a `DATE` `EXDATE`.
    #[must_use]
    pub const fn with_exclusions(self, exclusions: ExclusionReading) -> Self {
        Self { exclusions, ..self }
    }
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{CivilDate, CivilDateTime, CivilTime, Instant, UtcOffset};

    use super::{
        ExclusionReading, LocalInterval, ResolutionPolicy, UntilReading, nominal, wall_clock,
    };
    use crate::internal::tz::answer::{FoldPolicy, GapPolicy};

    fn stamp(year: u16, month: u8, day: u8, hour: u8) -> CivilDateTime {
        let date = CivilDate::from_ymd(year, month, day).unwrap();
        CivilDateTime::new(date, CivilTime::from_hms(hour, 0, 0).unwrap())
    }

    /// The projection is a bijection on everything RFC 5545 can write, and it is the identity
    /// on UTC — which is what makes a floating series and a UTC series need no seam at all.
    #[test]
    fn the_projection_round_trips_and_is_the_identity_at_utc() {
        for local in [
            stamp(2026, 1, 1, 0),
            stamp(2026, 8, 10, 9),
            stamp(0, 1, 1, 0),
            stamp(9999, 12, 31, 23),
        ] {
            let projected = nominal(local).unwrap();
            assert_eq!(wall_clock(projected), Some(local));
            assert_eq!(local.at_offset(UtcOffset::UTC), Some(projected));
        }
    }

    /// The property the whole seam exists for, stated as arithmetic rather than as prose: a
    /// daily cadence on the nominal timeline is a daily cadence on the wall clock, whatever the
    /// zone does in the middle of it.
    ///
    /// A day is 86,400 seconds *nominally* — that is what makes `ical-recur`'s period walk
    /// preserve the civil fields — and the offsets that turn those keys into real instants are
    /// applied one key at a time by the units above.
    #[test]
    fn a_daily_cadence_on_the_nominal_timeline_is_a_daily_cadence_on_the_wall_clock() {
        let anchor = nominal(stamp(2026, 3, 27, 9)).unwrap();
        for step in 0..6_i64 {
            let seconds = step.checked_mul(86_400).unwrap();
            let key = anchor.checked_add_seconds(seconds).unwrap();
            let clock = wall_clock(key).unwrap();
            assert_eq!(
                Some(clock.time()),
                CivilTime::from_hms(9, 0, 0),
                "the wall clock does not drift across the day a zone moves"
            );
        }
        let crossed = wall_clock(anchor.checked_add_seconds(432_000).unwrap()).unwrap();
        assert_eq!(crossed.date(), CivilDate::from_ymd(2026, 4, 1).unwrap());
    }

    #[test]
    fn a_span_is_half_open_and_refuses_to_be_empty() {
        let start = Instant::from_unix_seconds(0);
        let end = Instant::from_unix_seconds(86_400);
        let day = LocalInterval::new(start, end).unwrap();
        assert!(day.contains(start));
        assert!(!day.contains(end));
        assert_eq!(day.start(), start);
        assert_eq!(LocalInterval::new(end, start), None);
        assert_eq!(LocalInterval::new(start, start), None);
    }

    /// Every default is the conservative reading, and each is one call away from the other.
    #[test]
    fn the_stated_defaults_are_the_conservative_readings() {
        let policy = ResolutionPolicy::DEFAULT;
        assert_eq!(policy, ResolutionPolicy::default());
        assert_eq!(policy.gaps(), GapPolicy::Skip);
        assert_eq!(policy.folds(), FoldPolicy::Earlier);
        assert_eq!(policy.until(), UntilReading::Midnight);
        assert_eq!(policy.exclusions(), ExclusionReading::Instantaneous);

        let stated = policy
            .with_gaps(GapPolicy::ShiftForward)
            .with_until(UntilReading::EndOfDay)
            .with_exclusions(ExclusionReading::WholeDay)
            .with_folds(FoldPolicy::Later);
        assert_eq!(stated.gaps(), GapPolicy::ShiftForward);
        assert_eq!(stated.until(), UntilReading::EndOfDay);
        assert_eq!(stated.exclusions(), ExclusionReading::WholeDay);
        assert_eq!(stated.folds(), FoldPolicy::Later);
    }
}
