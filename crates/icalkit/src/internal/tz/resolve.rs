// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resolve instants and local times from `VTIMEZONE` transitions under RFC 5545 sections 3.6.5 and 3.3.5.
//!
//! RFC 5545 section 3.3.14 limits offsets to less than a day in either direction.
//! Every candidate instant lies within a day on either side of the local time's reading at UTC.
//! The resolver visits the starting era and every transition inside that window.
//! It accepts an instant only if the era's offset governs that instant.
//! One reading gives a unique time, two give a fold, and a forward transition can leave a gap.
//!
//! Explicit dates use binary search.
//! Recurring observances calculate onsets near the query year without materializing intervening years.
//! Each answer carries its coverage basis.
//! Outside known coverage the table continues the nearest offset and reports the limiting date.
//! A truncated table also treats its last admitted date as uncertain.
//! A recognized empty table returns `LocalResolution::Undetermined`.
//!
//! Before the first onset, `TZOFFSETFROM` supplies the offset and no observance supplies a daylight classification.
//! `observance_at` returns `None` and `offset_at` returns the offset with the daylight flag clear.
//!
//! In a gap, `gap_end` is the transition instant and `gap_start` is one second earlier.
//! The difference between the two offsets gives the missing stretch of local time.
//! `GapPolicy::ClampToTransition` chooses `gap_end`.
//! Callers can inspect the resolution and coverage codes without supplying a diagnostic sink.

use alloc::vec::Vec;

use crate::internal::core::{CivilDate, CivilDateTime, Instant, UtcOffset};

use crate::internal::tz::answer::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, ZoneAnswer, ZoneProvenance, ZoneSource,
};
use crate::internal::tz::model::{Observance, TransitionTable};

/// The lower bound on possible readings, relative to the wall clock's reading at UTC.
/// RFC 5545 section 3.3.14 limits offsets to less than one day in either direction.
const EARLIEST_READING_SECONDS: i64 = -86_400;

/// The upper bound on possible readings, relative to the wall clock's reading at UTC.
const LATEST_READING_SECONDS: i64 = 86_400;

/// An interval with a constant offset, including the era before any observance begins.
#[derive(Clone, Copy, Debug)]
struct Era {
    /// The observance that began it, absent before the table's first onset.
    observance: Option<Observance>,
    /// The offset in force through it.
    offset: UtcOffset,
    /// Whether that offset is the zone's daylight one.
    daylight: bool,
}

impl TransitionTable {
    /// The observance in force at `instant`.
    /// Returns `None` before the first onset or when the table has no observances.
    /// The earliest `TZOFFSETFROM` can still supply an offset before the first onset.
    #[must_use]
    pub fn observance_at(&self, instant: Instant) -> Option<Observance> {
        self.era_at(instant)?.observance
    }

    /// Sample observances at the endpoints of the local time's candidate window.
    /// Use [`ZoneSource::resolve`] to classify the local time.
    #[must_use]
    pub fn observances_around(
        &self,
        local: CivilDateTime,
    ) -> (Option<Observance>, Option<Observance>) {
        let Some((opening, closing)) = self.eras_around(local) else {
            return (None, None);
        };
        (opening.observance, closing.observance)
    }

    /// Find the readings whose offsets govern their own instants, or the transition that skipped the local time.
    fn resolution_of(&self, local: CivilDateTime) -> Option<LocalResolution> {
        let (opens, closes) = window_around(local)?;
        let mut era = self.era_at(opens)?;
        let mut found = Readings::EMPTY;
        let mut sprang_over: Option<LocalResolution> = None;
        found.offer(self.reading_through(local, era));
        for onset in self.onsets_between(opens, closes) {
            let Some(next) = self.era_at(onset) else {
                continue;
            };
            found.offer(self.reading_through(local, next));
            if sprang_over.is_none() {
                sprang_over = gap(local, era, next, onset);
            }
            era = next;
        }
        found
            .resolution()
            .or(sprang_over)
            .or_else(|| read_before(local, self.era_at(opens)?))
    }

    /// Read `local` with the era's offset and accept the instant only if that offset governs it.
    fn reading_through(&self, local: CivilDateTime, era: Era) -> Option<Reading> {
        let instant = local.at_offset(era.offset)?;
        let landed = self.era_at(instant)?;
        (landed.offset == era.offset).then(|| Reading::new(instant, era.offset, landed.daylight))
    }

    /// The eras at the two ends of the local time's candidate window.
    fn eras_around(&self, local: CivilDateTime) -> Option<(Era, Era)> {
        let (opens, closes) = window_around(local)?;
        Some((self.era_at(opens)?, self.era_at(closes)?))
    }

    /// Return explicit and recurring transitions after `from` through `to` in ascending order.
    fn onsets_between(&self, from: Instant, to: Instant) -> Vec<Instant> {
        let listed = self.observances();
        let first = listed.partition_point(|candidate| began_by(*candidate, from));
        let mut found: Vec<Instant> = Vec::new();
        for observance in listed.get(first..).unwrap_or(&[]) {
            let Some(onset) = onset_of(*observance) else {
                continue;
            };
            if onset > to {
                break;
            }
            found.push(onset);
        }
        for observance in self.rules() {
            push_rule_onsets(*observance, from, to, &mut found);
        }
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The stretch of the timeline `instant` falls in, absent when this table declares nothing.
    fn era_at(&self, instant: Instant) -> Option<Era> {
        let earliest = *self.observances().first()?;
        let Some((_onset, observance)) = self.latest_onset_at_or_before(instant) else {
            // Before the first onset the file states one thing and asserts nothing else: the earliest observance's `TZOFFSETFROM` was running, and no subcomponent covers that era to classify it.
            // A `daylight` flag is an assertion, so its absence is `false`.
            return Some(Era {
                observance: None,
                offset: earliest.offset_from(),
                daylight: false,
            });
        };
        Some(Era {
            observance: Some(observance),
            offset: observance.offset_to(),
            daylight: observance.daylight(),
        })
    }

    /// Find the last onset at or before `instant`.
    /// Binary search locates explicit dates, and the recurring patterns supply candidate onsets.
    fn latest_onset_at_or_before(&self, instant: Instant) -> Option<(Instant, Observance)> {
        let listed = self.observances();
        let past = listed.partition_point(|candidate| began_by(*candidate, instant));
        let mut latest = listed
            .get(..past)
            .and_then(<[Observance]>::last)
            .and_then(|dated| onset_of(*dated).map(|onset| (onset, *dated)));
        for observance in self.rules() {
            let Some(onset) = latest_rule_onset(*observance, instant) else {
                continue;
            };
            if latest.is_none_or(|(known, _)| known <= onset) {
                latest = Some((onset, *observance));
            }
        }
        latest
    }

    /// Classify the date using the known coverage bounds.
    /// On the final date of a truncated table, omitted onsets may change the answer.
    fn basis_for(&self, date: Option<CivilDate>) -> AnswerBasis {
        if let (Some(asked), Some(first)) = (date, self.coverage_start())
            && asked < first
        {
            return AnswerBasis::BeforeKnownTransitions(first);
        }
        let Some(known) = self.coverage_end() else {
            return AnswerBasis::Computed;
        };
        if date.is_none_or(|asked| asked > known || (self.is_truncated() && asked == known)) {
            AnswerBasis::BeyondKnownTransitions(known)
        } else {
            AnswerBasis::Computed
        }
    }
}

/// Distinct candidate readings for one local time.
/// The extreme instants describe a fold even if an unusual definition supplies more than two readings.
#[derive(Clone, Copy, Debug)]
struct Readings {
    /// The earliest reading found.
    earliest: Option<Reading>,
    /// The last distinct reading in instant order.
    latest: Option<Reading>,
    /// Number of distinct candidate instants.
    count: u32,
}

impl Readings {
    /// Nothing found yet.
    const EMPTY: Self = Self {
        earliest: None,
        latest: None,
        count: 0,
    };

    /// Record a distinct candidate reading.
    fn offer(&mut self, reading: Option<Reading>) {
        let Some(found) = reading else {
            return;
        };
        let held = |kept: Option<Reading>| kept.is_some_and(|one| one.instant == found.instant);
        if held(self.earliest) || held(self.latest) {
            return;
        }
        if self
            .earliest
            .is_none_or(|kept| found.instant < kept.instant)
        {
            self.earliest = Some(found);
        }
        if self.latest.is_none_or(|kept| found.instant > kept.instant) {
            self.latest = Some(found);
        }
        self.count = self.count.saturating_add(1);
    }

    /// Return a unique reading or the two extreme readings of a fold.
    /// Zero readings leave the caller to find a gap.
    fn resolution(self) -> Option<LocalResolution> {
        match (self.count, self.earliest, self.latest) {
            (0, _, _) => None,
            (1, Some(only), _) => Some(LocalResolution::Unique { reading: only }),
            (_, Some(earlier), Some(later)) => Some(LocalResolution::Ambiguous { earlier, later }),
            // Unreachable: a non-zero count has set both fields.
            _ => None,
        }
    }
}

impl ZoneSource for TransitionTable {
    /// Resolve `local` for the table's exact identifier.
    /// Unknown identifiers return `None`.
    /// An empty table returns [`LocalResolution::Undetermined`] for its own identifier.
    /// Vendor name mapping belongs to the caller under `docs/adr/0003`.
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        if self.tzid().as_str() != tzid {
            return None;
        }
        // `None` from the resolution has exactly one cause once the identifier matches: a table with no observance to walk a window over.
        // Every wall clock RFC 5545 section 3.3.4 can write has a window, and a non-empty table has an era covering it.
        let resolution = self
            .resolution_of(local)
            .unwrap_or(LocalResolution::Undetermined);
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::EmbeddedVtimezone,
            self.basis_for(Some(local.date())),
        ))
    }

    /// Find the offset at `instant`.
    /// An unknown identifier or an empty table returns `None`.
    /// [`ZoneSource::recognizes`] distinguishes the two cases.
    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        if self.tzid().as_str() != tzid {
            return None;
        }
        let era = self.era_at(instant)?;
        let date = CivilDateTime::from_instant(instant, era.offset).map(CivilDateTime::date);
        Some(OffsetAnswer::new(
            era.offset,
            era.daylight,
            ZoneProvenance::EmbeddedVtimezone,
            self.basis_for(date),
        ))
    }

    /// Check the exact identifier even when the table holds no transitions.
    fn recognizes(&self, tzid: &str) -> bool {
        self.tzid().as_str() == tzid
    }
}

/// Bound candidate instants to one day on either side of the local time's reading at UTC.
fn window_around(local: CivilDateTime) -> Option<(Instant, Instant)> {
    let read_at_utc = local.at_offset(UtcOffset::UTC)?;
    let opens = read_at_utc.checked_add_seconds(EARLIEST_READING_SECONDS)?;
    let closes = read_at_utc.checked_add_seconds(LATEST_READING_SECONDS)?;
    Some((opens, closes))
}

/// Compute the onset from `DTSTART` and `TZOFFSETFROM` under RFC 5545 section 3.6.5.
fn onset_of(observance: Observance) -> Option<Instant> {
    observance.start().at_offset(observance.offset_from())
}

/// Test whether the observance's anchor begins at or before `instant`.
fn began_by(observance: Observance, instant: Instant) -> bool {
    onset_of(observance).is_none_or(|onset| onset <= instant)
}

/// Find the last recurring onset at or before the query instant.
/// Scan years backwards from the bounded query year.
fn latest_rule_onset(observance: Observance, instant: Instant) -> Option<Instant> {
    let from = probe_year(observance, instant)?;
    let rule = observance.rule()?;
    for year in rule.prior_calendar_years(observance.start().date(), from) {
        let asked = observance
            .transition_in(year)
            .and_then(|local| local.at_offset(observance.offset_from()));
        if let Some(onset) = asked.filter(|found| *found <= instant) {
            return Some(onset);
        }
    }
    rule.boundary_calendar_years(observance.start().date(), from)
        .filter_map(|year| observance.transition_in(year))
        .filter_map(|local| local.at_offset(observance.offset_from()))
        .filter(|found| *found <= instant)
        .max()
}

/// Append every onset `observance`'s rule has after `from` and at or before `to`.
///
/// The offset preceding the transition places both endpoints on the observance's clock.
/// A yearly rule contributes at most one onset per year, so this pushes at most a handful whatever the window holds.
fn push_rule_onsets(observance: Observance, from: Instant, to: Instant, out: &mut Vec<Instant>) {
    let first = onset_year(from, observance.offset_from()).max(observance.start().date().year());
    let last = onset_year(to, observance.offset_from());
    for year in first..=last {
        let asked = observance
            .transition_in(year)
            .and_then(|local| local.at_offset(observance.offset_from()));
        if let Some(onset) = asked.filter(|found| *found > from && *found <= to) {
            out.push(onset);
        }
    }
}

/// Clamp the search year to the civil range on the observance's clock.
fn onset_year(instant: Instant, offset: UtcOffset) -> u16 {
    CivilDateTime::from_instant(instant, offset).map_or_else(
        || if instant < Instant::EPOCH { 0 } else { 9999 },
        |clock| clock.date().year(),
    )
}

/// The first year to ask `observance`'s rule about, absent when it repeats by no rule.
fn probe_year(observance: Observance, instant: Instant) -> Option<u16> {
    let rule = observance.rule()?;
    let reachable = onset_year(instant, observance.offset_from());
    // An inclusive `UNTIL` bounds the final onset year.
    Some(
        rule.through()
            .map_or(reachable, |end| end.year().min(reachable)),
    )
}

/// Return the gap if `local` lies between the transition's old and new clock readings.
/// The old reading opens the gap and the new reading closes it.
fn gap(local: CivilDateTime, before: Era, after: Era, onset: Instant) -> Option<LocalResolution> {
    if before.offset == after.offset {
        return None;
    }
    let opened = CivilDateTime::from_instant(onset, before.offset)?;
    let closed = CivilDateTime::from_instant(onset, after.offset)?;
    if opened > local || local >= closed {
        return None;
    }
    let gap_start = onset.checked_add_seconds(-1)?;
    Some(LocalResolution::Nonexistent {
        gap_start,
        gap_end: onset,
        offset_before: before.offset,
        offset_after: after.offset,
        shifted: local.at_offset(before.offset)?,
    })
}

/// Use the preceding era's offset if the candidate search finds neither a valid reading nor a matching forward jump.
fn read_before(local: CivilDateTime, era: Era) -> Option<LocalResolution> {
    let instant = local.at_offset(era.offset)?;
    Some(LocalResolution::Unique {
        reading: Reading::new(instant, era.offset, era.daylight),
    })
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, DiagnosticCode, IgnoreDiagnostics, Instant, Limits,
        Meter, UtcOffset, Weekday,
    };

    use crate::internal::tz::answer::{
        AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, Reading, ZoneAnswer, ZoneProvenance,
        ZoneSource,
    };
    use crate::internal::tz::model::{NthWeek, Observance, RuleDay, TransitionTable, YearlyRule};

    fn day(year: u16, month: u8, day_of_month: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day_of_month).unwrap()
    }

    fn stamp(year: u16, month: u8, day_of_month: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            day(year, month, day_of_month),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    /// Build an expected instant from its UTC clock.
    fn utc(year: u16, month: u8, day_of_month: u8, hour: u8, minute: u8) -> Instant {
        let clock = stamp(year, month, day_of_month, hour, minute);
        clock.at_offset(UtcOffset::UTC).unwrap()
    }

    fn table(tzid: &str, observances: Vec<Observance>) -> TransitionTable {
        let mut meter = Meter::new(Limits::DEFAULT);
        TransitionTable::new(
            Box::from(tzid),
            observances,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
    }

    /// A rule on the given Sunday of a month, at a whole hour.
    fn sunday_rule(month: u8, week: NthWeek, hour: u8, through: Option<CivilDate>) -> YearlyRule {
        YearlyRule::new(
            month,
            RuleDay::Nth {
                weekday: Weekday::Sunday,
                week,
            },
            CivilTime::from_hms(hour, 0, 0).unwrap(),
            through,
        )
        .unwrap()
    }

    /// Berlin's 2026 transitions both occur at 01:00 UTC.
    /// The spring onset uses `02:00 CET` and the autumn onset uses `03:00 CEST`.
    fn berlin_transitions() -> Vec<Observance> {
        alloc::vec![
            Observance::new(
                stamp(2026, 3, 29, 2, 0),
                offset(3600),
                offset(7200),
                true,
                None
            ),
            Observance::new(
                stamp(2026, 10, 25, 3, 0),
                offset(7200),
                offset(3600),
                false,
                None
            ),
        ]
    }

    fn berlin() -> TransitionTable {
        table("Europe/Berlin", berlin_transitions())
    }

    /// New York changed its daylight rules in 2007.
    /// The old rules used the first Sunday in April and the last Sunday in October.
    /// The new rules use the second Sunday in March and the first Sunday in November.
    fn new_york() -> TransitionTable {
        table(
            "America/New_York",
            alloc::vec![
                Observance::new(
                    stamp(1987, 4, 5, 2, 0),
                    offset(-18_000),
                    offset(-14_400),
                    true,
                    Some(sunday_rule(4, NthWeek::First, 2, Some(day(2006, 4, 2))))
                ),
                Observance::new(
                    stamp(1967, 10, 29, 2, 0),
                    offset(-14_400),
                    offset(-18_000),
                    false,
                    Some(sunday_rule(10, NthWeek::Last, 2, Some(day(2006, 10, 29))))
                ),
                Observance::new(
                    stamp(2007, 3, 11, 2, 0),
                    offset(-18_000),
                    offset(-14_400),
                    true,
                    Some(sunday_rule(3, NthWeek::Second, 2, None))
                ),
                Observance::new(
                    stamp(2007, 11, 4, 2, 0),
                    offset(-14_400),
                    offset(-18_000),
                    false,
                    Some(sunday_rule(11, NthWeek::First, 2, None))
                ),
            ],
        )
    }

    /// Lord Howe moves between `+10:30` and `+11:00`.
    /// The half-hour transition tests gaps and folds shorter than an hour.
    fn lord_howe() -> TransitionTable {
        table(
            "Australia/Lord_Howe",
            alloc::vec![
                Observance::new(
                    stamp(2025, 10, 5, 2, 0),
                    offset(37_800),
                    offset(39_600),
                    true,
                    None
                ),
                Observance::new(
                    stamp(2026, 4, 5, 2, 0),
                    offset(39_600),
                    offset(37_800),
                    false,
                    None
                ),
                Observance::new(
                    stamp(2026, 10, 4, 2, 0),
                    offset(37_800),
                    offset(39_600),
                    true,
                    None
                ),
            ],
        )
    }

    /// What a case expects, in the terms the zone's own published rules state it in.
    #[derive(Debug)]
    enum Expected {
        /// One reading with its offset and daylight flag.
        Unique(i32, bool),
        /// Two instants: the earlier and the later of the repeated hour.
        Fold(Instant, Instant),
        /// No instant: the gap closes at the first, and section 3.3.5 reads it as the second.
        Gap(Instant, Instant),
    }

    fn check(name: &str, zone: &TransitionTable, local: CivilDateTime, expected: Expected) {
        let answer = zone.resolve(zone.tzid().as_str(), local).unwrap();
        assert_eq!(answer.source, ZoneProvenance::EmbeddedVtimezone, "{name}");
        match (answer.resolution, expected) {
            (LocalResolution::Unique { reading }, Expected::Unique(seconds, daylight)) => {
                assert_eq!(reading.offset, offset(seconds), "{name}");
                assert_eq!(reading.daylight, daylight, "{name}");
                assert_eq!(
                    Some(reading.instant),
                    local.at_offset(offset(seconds)),
                    "{name}"
                );
            },
            (LocalResolution::Ambiguous { earlier, later }, Expected::Fold(first, second)) => {
                assert_eq!(earlier.instant, first, "{name}");
                assert_eq!(later.instant, second, "{name}");
                assert!(earlier.instant < later.instant, "{name}");
                assert_ne!(earlier.offset, later.offset, "{name}");
            },
            (
                LocalResolution::Nonexistent {
                    gap_start,
                    gap_end,
                    offset_before,
                    offset_after,
                    shifted,
                },
                Expected::Gap(closes_at, shifted_to),
            ) => {
                assert_eq!(gap_end, closes_at, "{name}");
                assert_eq!(shifted, shifted_to, "{name}");
                assert!(gap_start < gap_end, "{name}");
                assert!(offset_before.seconds() < offset_after.seconds(), "{name}");
                assert_eq!(Some(shifted), local.at_offset(offset_before), "{name}");
            },
            (resolution, wanted) => panic!("{name}: {resolution:?} is not {wanted:?}"),
        }
    }

    /// Zone fixtures supply independent UTC instants for unique times, gaps, and folds.
    #[test]
    fn a_wall_clock_resolves_by_counting_the_offsets_that_govern_the_instants_it_names() {
        let cases = alloc::vec![
            (
                "Europe/Berlin, an ordinary summer day",
                berlin(),
                stamp(2026, 7, 1, 12, 0),
                Expected::Unique(7200, true),
            ),
            (
                "Europe/Berlin, an ordinary winter day",
                berlin(),
                stamp(2026, 12, 1, 12, 0),
                Expected::Unique(3600, false),
            ),
            (
                "Europe/Berlin, 02:30 on the morning the clocks went forward",
                berlin(),
                stamp(2026, 3, 29, 2, 30),
                Expected::Gap(utc(2026, 3, 29, 1, 0), utc(2026, 3, 29, 1, 30)),
            ),
            (
                "Europe/Berlin, 02:30 on the morning the clocks went back",
                berlin(),
                stamp(2026, 10, 25, 2, 30),
                Expected::Fold(utc(2026, 10, 25, 0, 30), utc(2026, 10, 25, 1, 30)),
            ),
            (
                "America/New_York, 02:30 on the second Sunday in March",
                new_york(),
                stamp(2026, 3, 8, 2, 30),
                Expected::Gap(utc(2026, 3, 8, 7, 0), utc(2026, 3, 8, 7, 30)),
            ),
            (
                "America/New_York, 01:30 on the first Sunday in November",
                new_york(),
                stamp(2026, 11, 1, 1, 30),
                Expected::Fold(utc(2026, 11, 1, 5, 30), utc(2026, 11, 1, 6, 30)),
            ),
            (
                "America/New_York in March 2000, when April still began daylight time",
                new_york(),
                stamp(2000, 3, 15, 12, 0),
                Expected::Unique(-18_000, false),
            ),
            (
                "America/New_York in April 2000, past the first Sunday of it",
                new_york(),
                stamp(2000, 4, 10, 12, 0),
                Expected::Unique(-14_400, true),
            ),
            (
                "Australia/Lord_Howe, 02:15 inside a thirty-minute gap",
                lord_howe(),
                stamp(2026, 10, 4, 2, 15),
                Expected::Gap(utc(2026, 10, 3, 15, 30), utc(2026, 10, 3, 15, 45)),
            ),
            (
                "Australia/Lord_Howe, 01:45 inside a thirty-minute fold",
                lord_howe(),
                stamp(2026, 4, 5, 1, 45),
                Expected::Fold(utc(2026, 4, 4, 14, 45), utc(2026, 4, 4, 15, 15)),
            ),
        ];
        for (name, zone, local, expected) in cases {
            check(name, &zone, local, expected);
        }
    }

    /// Daylight classification comes from the observance name.
    #[test]
    fn the_repeated_hour_names_two_instants_and_each_says_which_observance_made_it() {
        let zone = berlin();
        let local = stamp(2026, 10, 25, 2, 30);
        let answer = zone.resolve("Europe/Berlin", local).unwrap();
        let LocalResolution::Ambiguous { earlier, later } = answer.resolution else {
            panic!("a wall clock the zone fell back through names two instants");
        };
        assert_eq!(earlier.offset, offset(7200));
        assert!(earlier.daylight, "the first reading is still summer time");
        assert_eq!(later.offset, offset(3600));
        assert!(!later.daylight);
        assert_eq!(
            answer.resolution.diagnostic_code(),
            Some(DiagnosticCode::AmbiguousLocalTime)
        );
        assert_eq!(
            answer.resolution.pick(GapPolicy::Skip, FoldPolicy::Later),
            Some(utc(2026, 10, 25, 1, 30))
        );
        assert_eq!(answer.basis, AnswerBasis::Computed);
    }

    /// Clamping to Berlin's spring transition selects the first instant at `03:00 CEST`.
    #[test]
    fn the_hour_that_did_not_happen_carries_both_readings_and_picks_neither() {
        let zone = berlin();
        let local = stamp(2026, 3, 29, 2, 30);
        let answer = zone.resolve("Europe/Berlin", local).unwrap();
        let LocalResolution::Nonexistent {
            gap_start,
            gap_end,
            offset_before,
            offset_after,
            shifted,
        } = answer.resolution
        else {
            panic!("a wall clock the zone sprang over names no instant");
        };
        assert_eq!(gap_end, utc(2026, 3, 29, 1, 0));
        assert_eq!(
            gap_start.checked_add_seconds(1),
            Some(gap_end),
            "a gap has no width on the timeline, so its two instants are adjacent"
        );
        assert_eq!(offset_before, offset(3600));
        assert_eq!(offset_after, offset(7200));
        assert_eq!(
            CivilDateTime::from_instant(gap_end, offset_after),
            Some(stamp(2026, 3, 29, 3, 0)),
            "the instant the gap closed reads as the first wall clock past it"
        );
        assert_eq!(
            answer.resolution.pick(GapPolicy::Skip, FoldPolicy::Earlier),
            None,
            "RFC 5545 section 3.3.10 drops it"
        );
        assert_eq!(
            answer
                .resolution
                .pick(GapPolicy::ShiftForward, FoldPolicy::Earlier),
            Some(shifted),
            "RFC 5545 section 3.3.5 reads it with the offset in force before"
        );
        assert_eq!(
            answer
                .resolution
                .pick(GapPolicy::ClampToTransition, FoldPolicy::Earlier),
            Some(gap_end)
        );
    }

    /// An answer after the last explicit transition reports its finite coverage.
    #[test]
    fn a_table_of_dates_that_ran_out_answers_and_says_it_was_extrapolating() {
        let zone = berlin();
        let ends = day(2026, 10, 25);
        assert_eq!(zone.coverage_end(), Some(ends));

        let inside = stamp(2026, 7, 1, 12, 0);
        let covered = zone.resolve("Europe/Berlin", inside).unwrap();
        assert_eq!(covered.basis, AnswerBasis::Computed);
        assert_eq!(covered.basis.diagnostic_code(), None);

        let years_later = stamp(2029, 7, 1, 12, 0);
        let past = zone.resolve("Europe/Berlin", years_later).unwrap();
        assert_eq!(
            past.basis,
            AnswerBasis::BeyondKnownTransitions(ends),
            "the last observance is continued, and the answer names the date it stopped at"
        );
        assert_eq!(
            past.basis.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneCoverageExhausted)
        );
        assert_ne!(
            past.basis, covered.basis,
            "an exhausted table must not be indistinguishable from a computed answer"
        );
        assert_eq!(
            past.resolution,
            LocalResolution::Unique {
                reading: Reading::new(utc(2029, 7, 1, 11, 0), offset(3600), false)
            },
            "July 2029 continues October 2026's standard time, because nothing says otherwise"
        );

        let noon = utc(2029, 7, 1, 11, 0);
        let later = zone.offset_at("Europe/Berlin", noon).unwrap();
        assert_eq!(later.offset, offset(3600));
        assert_eq!(later.basis, AnswerBasis::BeyondKnownTransitions(ends));

        let endless = new_york();
        assert_eq!(
            endless.coverage_end(),
            None,
            "a rule with no UNTIL means the zone knows the future"
        );
        let far = endless.resolve("America/New_York", years_later).unwrap();
        assert_eq!(far.basis, AnswerBasis::Computed);
    }

    /// Lookup preserves literal `TZID` bytes, including vendor prefixes.
    #[test]
    fn an_identifier_is_matched_by_its_bytes_and_is_never_read_as_a_zone_name() {
        let noon = stamp(2026, 7, 1, 12, 0);
        for name in [
            "W. Europe Standard Time",
            "/mozilla.org/20050126_1/Europe/Berlin",
            "Customized Time Zone",
            "Europe/Berlin",
        ] {
            let zone = table(name, berlin_transitions());
            assert!(
                zone.resolve(name, noon).is_some(),
                "{name} answers to the identifier it was built under"
            );
            assert!(zone.offset_at(name, utc(2026, 7, 1, 10, 0)).is_some());
            assert_eq!(
                zone.resolve("Etc/UTC", noon),
                None,
                "{name} does not answer to an identifier it does not carry"
            );
        }

        let vendor = table("W. Europe Standard Time", berlin_transitions());
        assert_eq!(
            vendor.resolve("Europe/Berlin", noon),
            None,
            "mapping a vendor identifier onto an IANA one is the caller's visible step"
        );
    }

    /// An empty definition recognizes its identifier without supplying an offset.
    #[test]
    fn a_table_with_no_observance_has_nothing_to_resolve_against() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let zone = TransitionTable::new(
            Box::from("Europe/Berlin"),
            Vec::new(),
            &mut meter,
            &mut IgnoreDiagnostics,
        );
        assert!(zone.is_empty());
        assert_eq!(zone.observances(), &[]);
        assert_eq!(
            zone.coverage_end(),
            None,
            "no observance covers nothing, which is not the same as covering everything"
        );
        assert_eq!(
            zone.resolve("Europe/Berlin", stamp(2026, 7, 1, 12, 0)),
            Some(ZoneAnswer::new(
                LocalResolution::Undetermined,
                ZoneProvenance::EmbeddedVtimezone,
                AnswerBasis::Computed
            )),
            "the identifier is one this table answers to, and `None` would say it was not"
        );
        assert!(
            zone.recognizes("Europe/Berlin"),
            "a table recognizes its own identifier whether or not it holds a transition"
        );
        assert!(!zone.recognizes("America/New_York"));
        assert_eq!(zone.offset_at("Europe/Berlin", Instant::EPOCH), None);
        assert_eq!(zone.observance_at(Instant::EPOCH), None);
        assert_eq!(
            zone.observances_around(stamp(2026, 7, 1, 12, 0)),
            (None, None)
        );
    }

    /// Before the first onset the file states an offset and classifies nothing, and the two methods say exactly that much each.
    #[test]
    fn before_the_first_onset_the_offset_is_stated_and_no_observance_is_in_force() {
        let zone = berlin();
        let early = utc(2020, 1, 1, 0, 0);
        assert_eq!(
            zone.observance_at(early),
            None,
            "no observance has begun, and the one that has not begun states a different offset"
        );
        let answer = zone.offset_at("Europe/Berlin", early).unwrap();
        assert_eq!(
            answer.offset,
            offset(3600),
            "TZOFFSETFROM on the earliest observance is what the file says was running"
        );
        assert!(!answer.daylight);

        let (opening, closing) = zone.observances_around(stamp(2026, 3, 29, 2, 30));
        assert_eq!(opening, None, "the gap's own morning has no earlier onset");
        assert_eq!(
            closing.map(Observance::offset_to),
            Some(offset(7200)),
            "and the observance on the other side is the one that took the clock forward"
        );

        let (before, after) = zone.observances_around(stamp(2026, 10, 25, 2, 30));
        assert_eq!(before.map(Observance::offset_to), Some(offset(7200)));
        assert_eq!(after.map(Observance::offset_to), Some(offset(3600)));

        let (steady, unchanged) = zone.observances_around(stamp(2026, 7, 1, 12, 0));
        assert_eq!(steady, unchanged, "an ordinary day has one observance");
        assert_eq!(steady.map(Observance::daylight), Some(true));
    }

    /// Recurring observances can answer a year beyond the explicit dates.
    #[test]
    fn a_rule_answers_a_year_the_table_holds_no_observance_for() {
        let zone = new_york();
        let latest = zone
            .observances()
            .iter()
            .map(|observance| observance.start().date().year())
            .max();
        assert_eq!(latest, Some(2007), "the table itself stops in 2007");

        let in_july = utc(2026, 7, 1, 16, 0);
        let in_december = utc(2026, 12, 1, 17, 0);
        let summer = zone.offset_at("America/New_York", in_july).unwrap();
        assert_eq!(summer.offset, offset(-14_400));
        assert!(summer.daylight);
        let winter = zone.offset_at("America/New_York", in_december).unwrap();
        assert_eq!(winter.offset, offset(-18_000));
        assert!(!winter.daylight);
        assert!(!summer.agrees_with(winter));

        let observance = zone.observance_at(in_july).unwrap();
        assert_eq!(
            observance.start().date().year(),
            2007,
            "the observance in force is the 2007 one, repeating; no 2026 observance was made"
        );
    }
}
