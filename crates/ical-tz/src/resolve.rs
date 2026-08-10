// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 2 — the zone source over a transition table, which is where the two awkward hours are
//! found.
//!
//! Specification: RFC 5545 section 3.6.5, with section 3.3.5 for the reading of a `DATE-TIME`
//! that falls in a gap.
//!
//! Owed by this unit and by nothing else, as an impl and inherent methods on a type the crate
//! root already exports, so no re-export line is needed:
//!
//! ```text
//! impl ZoneSource for TransitionTable { .. }
//! impl TransitionTable {
//!     pub fn observance_at(&self, instant: Instant) -> Option<Observance>;
//!     pub fn observances_around(&self, local: CivilDateTime)
//!         -> (Option<Observance>, Option<Observance>);
//! }
//! ```
//!
//! The load-bearing unit. A wall clock is resolved by taking the observances on either side of
//! it, projecting the queried fields through each candidate offset, and counting how many
//! results land in the interval that offset actually governs: two is `LocalResolution::Ambiguous`,
//! none is `LocalResolution::Nonexistent`, one is `LocalResolution::Unique`. The count is the
//! definition rather than a special case bolted onto a happy path, which is what keeps a zone
//! with an unusual transition — one that moves the clock by thirty minutes, or backwards into a
//! smaller daylight offset — from needing a branch of its own.
//!
//! A table whose rules run past the query is asked for the transition in the query's own year
//! through unit 1, rather than having its future observances materialized. That is what makes a
//! lookup logarithmic in the table and constant in the rules, and it is the whole of this
//! crate's answer to `docs/adr/0010` on the resolution path.
//!
//! Every answer carries its basis: `AnswerBasis::Computed` while the query is at or before
//! `TransitionTable::coverage_end`, and `AnswerBasis::BeyondKnownTransitions` carrying that date
//! after it, which is the `RDATE` table that ran out. A `coverage_end` of `None` means the zone
//! knows the future and no answer it gives is ever an extrapolation.
//!
//! This unit emits no diagnostic. What it produces are values whose codes units 3 and 5 read
//! off `LocalResolution::diagnostic_code` and `AnswerBasis::diagnostic_code`, because
//! `ZoneSource` has no meter and no sink and must stay implementable by a caller that has never
//! heard of either.
//!
//! # What the two candidates are, and why exactly two
//!
//! RFC 5545 section 3.3.14 cannot write an offset of a whole day, so every instant a wall clock
//! could name lies strictly inside one day either side of that clock's reading at UTC. The
//! offsets in force at those two bounds are therefore the only offsets any reading of the clock
//! could have been taken at, and looking them up is what `observances_around` does. Where the
//! two agree there is one candidate and one reading; where they differ the zone moved its clock
//! through the queried wall time, and the count over the two says which way.
//!
//! # Before the first onset
//!
//! A table's earliest observance states, in `TZOFFSETFROM`, what was running before it. That is
//! the whole of what the file says about that era: no `STANDARD` or `DAYLIGHT` subcomponent
//! covers it, so nothing classifies it. `offset_at` reports that offset with `daylight` false —
//! the flag is an assertion and there is none — while `observance_at` reports `None`, because
//! there is genuinely no observance in force and answering with the one that has not begun yet
//! would misstate the offset.
//!
//! # What a gap's two instants are
//!
//! A gap has no width on the UTC timeline at all. The clock jumps at one instant; what is
//! missing is a stretch of wall clock, and how wide it was is stated by `offset_before` against
//! `offset_after`. So `gap_end` is that instant — the first the new offset governs, which is
//! what `GapPolicy::ClampToTransition` moves an event to, "as soon as it can" — and `gap_start`
//! is the last instant before the gap opened, one second earlier, which is the only pair that
//! keeps both field descriptions literally true and `gap_start < gap_end` with them.

use ical_core::{CivilDate, CivilDateTime, Instant, UtcOffset};

use crate::answer::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, ZoneAnswer, ZoneProvenance, ZoneSource,
};
use crate::model::{Observance, TransitionTable};

/// How many observances admitted at or before a query may still be repeating by a rule.
///
/// A definition states its current rules as the observances with the latest `DTSTART`s — two
/// for an ordinary zone, four for one that changed its rules and kept the superseded pair, the
/// shape Mozilla and Apple both export — so the rules that can still reach a query are the last
/// few admitted before it. Bounding the scan this way is what leaves a lookup logarithmic in
/// the table and constant in the rules, which is `docs/adr/0010`'s argument on a path that has
/// no meter to charge.
///
/// The input this gives up on is a definition whose `RDATE` onsets interleave with a rule's own
/// onsets over the same period, which is a file contradicting itself about that period. It gets
/// the explicit onset, which is the one it wrote down.
const RULE_WINDOW: usize = 4;

/// How many years back from a query a rule is asked about.
///
/// A yearly rule fires at most once a year, so the latest onset at or before a query is in the
/// year the scan starts at, the one before it, or — when an `UNTIL` fell earlier in its year
/// than the rule does — the one before that.
const RULE_PROBE_YEARS: u16 = 3;

/// The earliest instant a wall clock can name, as seconds from that clock's reading at UTC.
///
/// RFC 5545 section 3.3.14 writes an offset as at most `+hhmmss`, so a whole day is not
/// expressible and every reading lies strictly inside this bound.
const EARLIEST_READING_SECONDS: i64 = -86_400;

/// The latest instant a wall clock can name, as seconds from that clock's reading at UTC.
const LATEST_READING_SECONDS: i64 = 86_400;

/// One stretch of the timeline over which this zone's offset does not change.
///
/// The internal shape of an answer, kept private because it says one thing more than
/// [`Observance`] does — the era before the first onset, which no observance describes — and
/// one thing less, since a caller holding an era holds nothing it could put back into a file.
#[derive(Clone, Copy, Debug)]
struct Era {
    /// The observance that began it, absent before the table's first onset.
    observance: Option<Observance>,
    /// The instant it began, absent before the table's first onset.
    began: Option<Instant>,
    /// The offset in force through it.
    offset: UtcOffset,
    /// Whether that offset is the zone's daylight one.
    daylight: bool,
}

impl TransitionTable {
    /// The observance in force at `instant`, absent when this table has none in force there.
    ///
    /// Two conditions share the `None` and both are the honest answer rather than a stand-in
    /// for UTC: a table declaring no observance at all, and an instant before the earliest
    /// onset the table records. The offset the file states for that earlier era is its first
    /// observance's `TZOFFSETFROM`, which [`ZoneSource::offset_at`] does report; what the file
    /// has no observance for is the classification, and this method answers about observances.
    #[must_use]
    pub fn observance_at(&self, instant: Instant) -> Option<Observance> {
        self.era_at(instant)?.observance
    }

    /// The observances in force on either side of `local`, which is where a fold and a gap are.
    ///
    /// Read at the earliest and at the latest instant a wall clock showing `local` could name.
    /// The two are the same observance every day of the year but two; where they differ, the
    /// zone moved its clock through `local`, and which of the two offsets govern the readings
    /// of it is what says whether the wall time happened twice or not at all.
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

    /// What `local` names under this table, absent when the table has nothing to say at all.
    fn resolution_of(&self, local: CivilDateTime) -> Option<LocalResolution> {
        let (opening, closing) = self.eras_around(local)?;
        let through_opening = self.reading_through(local, opening);
        // One offset cannot govern two readings of one wall clock, so an era reaching both ends
        // of the window contributes one candidate and is counted once. The count is over
        // offsets that govern, not over eras looked up.
        let through_closing = (closing.offset != opening.offset)
            .then(|| self.reading_through(local, closing))
            .flatten();
        match (through_opening, through_closing) {
            (Some(first), Some(second)) => Some(fold(first, second)),
            (Some(only), None) | (None, Some(only)) => {
                Some(LocalResolution::Unique { reading: only })
            },
            (None, None) => gap(local, opening, closing).or_else(|| read_before(local, opening)),
        }
    }

    /// The reading of `local` through `era`'s offset, present only when the instant it produces
    /// falls in a stretch that same offset governs.
    ///
    /// This is the count. A wall clock read with an offset that is not in force at the instant
    /// the reading produces has not named that instant, and a wall clock no candidate offset
    /// governs has named none at all. `daylight` comes from the stretch the reading landed in
    /// rather than from comparing the two offsets, because a zone whose daylight offset is the
    /// smaller of the two exists and arithmetic would classify it backwards.
    fn reading_through(&self, local: CivilDateTime, era: Era) -> Option<Reading> {
        let instant = local.at_offset(era.offset)?;
        let landed = self.era_at(instant)?;
        (landed.offset == era.offset).then(|| Reading::new(instant, era.offset, landed.daylight))
    }

    /// The eras at the earliest and at the latest instant a wall clock showing `local` names.
    fn eras_around(&self, local: CivilDateTime) -> Option<(Era, Era)> {
        let read_at_utc = local.at_offset(UtcOffset::UTC)?;
        let opening = read_at_utc.checked_add_seconds(EARLIEST_READING_SECONDS)?;
        let closing = read_at_utc.checked_add_seconds(LATEST_READING_SECONDS)?;
        Some((self.era_at(opening)?, self.era_at(closing)?))
    }

    /// The stretch of the timeline `instant` falls in, absent when this table declares nothing.
    fn era_at(&self, instant: Instant) -> Option<Era> {
        let earliest = *self.observances().first()?;
        let Some((began, observance)) = self.latest_onset_at_or_before(instant) else {
            // Before the first onset the file states one thing and asserts nothing else: the
            // earliest observance's `TZOFFSETFROM` was running, and no subcomponent covers that
            // era to classify it. A `daylight` flag is an assertion, so its absence is `false`.
            return Some(Era {
                observance: None,
                began: None,
                offset: earliest.offset_from(),
                daylight: false,
            });
        };
        Some(Era {
            observance: Some(observance),
            began: Some(began),
            offset: observance.offset_to(),
            daylight: observance.daylight(),
        })
    }

    /// The latest onset this table records at or before `instant`, and what it began.
    ///
    /// Logarithmic in the table: the binary search places the explicit onsets, and only the
    /// window of observances just before that point is asked whether a rule of theirs fired
    /// later still. Nothing between two onsets is ever materialized.
    fn latest_onset_at_or_before(&self, instant: Instant) -> Option<(Instant, Observance)> {
        let listed = self.observances();
        let past = listed.partition_point(|candidate| began_by(*candidate, instant));
        let window = listed
            .get(past.saturating_sub(RULE_WINDOW)..past)
            .unwrap_or(&[]);
        let mut latest: Option<(Instant, Observance)> = None;
        for observance in window {
            let Some(onset) = latest_onset_of(*observance, instant) else {
                continue;
            };
            if latest.is_none_or(|(known, _)| known <= onset) {
                latest = Some((onset, *observance));
            }
        }
        latest
    }

    /// How much of this table stood behind an answer about `date`.
    ///
    /// `docs/adr/0003`'s third field, and the `RDATE` table that ran out. A date past
    /// `coverage_end` is answered by continuing the final observance, which is the defensible
    /// thing to do and a dishonest thing to do quietly, so the answer says so. A date the
    /// calendar cannot express is not one this table holds data for either.
    fn basis_for(&self, date: Option<CivilDate>) -> AnswerBasis {
        let Some(known) = self.coverage_end() else {
            return AnswerBasis::Computed;
        };
        if date.is_none_or(|asked| asked > known) {
            AnswerBasis::BeyondKnownTransitions(known)
        } else {
            AnswerBasis::Computed
        }
    }
}

impl ZoneSource for TransitionTable {
    /// What `local` names under this zone, or `None` when this is not this table's identifier.
    ///
    /// Identifiers are compared by exact bytes and never parsed. `W. Europe Standard Time`,
    /// `/mozilla.org/20050126_1/Europe/Berlin` and `Customized Time Zone` are all identifiers a
    /// table answers to when that is what it was built under, and none of them is one this
    /// crate may look inside: mapping a vendor name onto an IANA one is the caller's visible
    /// step, per `docs/adr/0003`.
    ///
    /// `None` also for a table declaring no observance, which RFC 5545 section 3.6.5 forbids
    /// and files carry anyway. "The zone answers nothing" is a smaller claim than "the zone is
    /// UTC", and only the first one the file supports.
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        if self.tzid().as_str() != tzid {
            return None;
        }
        let resolution = self.resolution_of(local)?;
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::EmbeddedVtimezone,
            self.basis_for(Some(local.date())),
        ))
    }

    /// What offset this zone was running at `instant`, or `None` when the identifier or the
    /// table is not one that can answer.
    ///
    /// The direction with no ambiguity in it: every instant has exactly one offset under a
    /// zone, which is precisely the asymmetry that makes [`ZoneSource::resolve`] hard.
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
}

/// The instant an observance's own `DTSTART` names.
///
/// RFC 5545 section 3.6.5 reads it against `TZOFFSETFROM`: the transition happens when the
/// clock that is still running reaches that wall time.
fn onset_of(observance: Observance) -> Option<Instant> {
    observance.start().at_offset(observance.offset_from())
}

/// Whether `observance`'s own `DTSTART` names an instant at or before `instant`.
///
/// The binary search's predicate, and the reason the search stays a search: it reads one
/// observance rather than the stretch between two. An onset that is not representable can only
/// have come from a date before the timeline, which sorts before everything and so leaves the
/// predicate monotone over a table sorted by `DTSTART`.
fn began_by(observance: Observance, instant: Instant) -> bool {
    onset_of(observance).is_none_or(|onset| onset <= instant)
}

/// The latest onset `observance` has at or before `instant`, by its `DTSTART` and by its rule.
fn latest_onset_of(observance: Observance, instant: Instant) -> Option<Instant> {
    let mut latest = onset_of(observance).filter(|onset| *onset <= instant);
    let Some(from) = probe_year(observance, instant) else {
        return latest;
    };
    for step in 0..RULE_PROBE_YEARS {
        let Some(year) = from.checked_sub(step) else {
            break;
        };
        // RFC 5545 makes an observance's `DTSTART` its first onset, so no rule is asked about
        // an earlier year: a year that answered would be an onset the definition does not have.
        if year < observance.start().date().year() {
            break;
        }
        let asked = observance
            .transition_in(year)
            .and_then(|local| local.at_offset(observance.offset_from()));
        let Some(onset) = asked.filter(|found| *found <= instant) else {
            continue;
        };
        if latest.is_none_or(|known| known < onset) {
            latest = Some(onset);
        }
    }
    latest
}

/// The first year to ask `observance`'s rule about, absent when it repeats by no rule.
fn probe_year(observance: Observance, instant: Instant) -> Option<u16> {
    let rule = observance.rule()?;
    let queried = CivilDateTime::from_instant(instant, UtcOffset::UTC)?
        .date()
        .year();
    // A year late, because an instant still in one year at UTC may already be in the next one
    // on the zone's own clock, and a transition dated there would otherwise be missed.
    let reachable = queried.saturating_add(1);
    // A rule with an `UNTIL` has no onset past it, so the scan starts at the last year it could
    // have fired in rather than at years it certainly did not.
    Some(
        rule.through()
            .map_or(reachable, |end| end.year().min(reachable)),
    )
}

/// The two readings of a wall clock the zone fell back through, in timeline order.
fn fold(first: Reading, second: Reading) -> LocalResolution {
    if first.instant <= second.instant {
        LocalResolution::Ambiguous {
            earlier: first,
            later: second,
        }
    } else {
        LocalResolution::Ambiguous {
            earlier: second,
            later: first,
        }
    }
}

/// The wall clock the zone sprang over, carrying the material for both readings of it.
///
/// `None` when the two eras agree about the offset, which is not a transition and therefore
/// not a gap; the caller falls back to the only claim left in that case.
fn gap(local: CivilDateTime, opening: Era, closing: Era) -> Option<LocalResolution> {
    if opening.offset == closing.offset {
        return None;
    }
    let gap_end = closing.began?;
    let gap_start = gap_end.checked_add_seconds(-1)?;
    Some(LocalResolution::Nonexistent {
        gap_start,
        gap_end,
        offset_before: opening.offset,
        offset_after: closing.offset,
        shifted: local.at_offset(opening.offset)?,
    })
}

/// RFC 5545 section 3.3.5's reading of `local`, as the only claim left when no candidate offset
/// governs its own reading and no transition separates the two candidates.
///
/// Unreachable for any table whose onsets are more than a day apart, which is every zone there
/// is: an offset governing both ends of the window governs everything between them, and every
/// reading of `local` lies inside it. A definition that moved its clock twice inside one day
/// and back to where it started could reach here, and reading the value with the offset in
/// force before it is the smallest claim available then.
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

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, DiagnosticCode, IgnoreDiagnostics, Instant, Limits,
        Meter, UtcOffset, Weekday,
    };

    use crate::answer::{
        AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, Reading, ZoneProvenance, ZoneSource,
    };
    use crate::model::{NthWeek, Observance, RuleDay, TransitionTable, YearlyRule};

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

    /// An instant named by its UTC wall clock, which is how every expectation below is
    /// written: from where the zone's published rules put the transition on the UTC timeline,
    /// not from what this file computes.
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

    /// Europe/Berlin's two 2026 transitions as dates, which is what a client writes when it
    /// writes `RDATE`s instead of rules.
    ///
    /// CET is `+01:00` and CEST `+02:00`, and both onsets are at 01:00 UTC — 02:00 CET on the
    /// last Sunday in March, 03:00 CEST on the last Sunday in October. The table says nothing
    /// about any year but 2026, which is the point of it.
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

    /// `America/New_York` with the rules that changed in 2007 and the pair they replaced, which
    /// is the shape Mozilla and Apple both export.
    ///
    /// Daylight saving time began on the first Sunday in April and ended on the last Sunday in
    /// October through 2006; from 2007 it begins on the second Sunday in March and ends on the
    /// first Sunday in November. Both pairs move the clock at 02:00 local, EST is `-05:00` and
    /// EDT `-04:00`.
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

    /// `Australia/Lord_Howe`, whose daylight saving time moves the clock by thirty minutes.
    ///
    /// Standard is `+10:30` and daylight `+11:00`; the clock goes forward at 02:00 on the first
    /// Sunday in October and back at 02:00 on the first Sunday in April. The gap is half an
    /// hour wide and the fold is half an hour long, and nothing in this crate branches on that.
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
        /// One instant, at this offset, daylight or not.
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

    /// The whole unit, against four real zones and their real transition rules.
    ///
    /// Every expected instant here is the one the zone's published rule puts the transition at
    /// on the UTC timeline — 01:00 UTC for Berlin, 07:00 UTC for New York's March Sunday,
    /// 15:30 UTC the day before for Lord Howe's October Sunday — so a case failing means the
    /// crate disagrees with the zone rather than with a previous version of itself.
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

    /// The daylight flag is the observance's own classification and not a comparison of the two
    /// offsets, which is the only reading that survives a zone whose daylight offset is smaller.
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

    /// The gap's closing instant is a real one: clamping into it lands on the first wall clock
    /// the transition left standing, which for Berlin is 03:00 CEST.
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

    /// The agenda item: a table of dates simply stops, and the answer past its end is not
    /// allowed to look like one a rule produced.
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

    /// A `TZID` is bytes as written. It is not parsed, not stripped, and not guessed at.
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

    /// The base case this unit must not answer UTC to.
    ///
    /// A `VTIMEZONE` with no observance violates RFC 5545 section 3.6.5, and the file is kept
    /// anyway. What the table may not do is invent an offset: "the identifier is known and the
    /// zone answers nothing" is a smaller claim than "the zone is UTC", and only the first one
    /// the file supports.
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
            None
        );
        assert_eq!(zone.offset_at("Europe/Berlin", Instant::EPOCH), None);
        assert_eq!(zone.observance_at(Instant::EPOCH), None);
        assert_eq!(
            zone.observances_around(stamp(2026, 7, 1, 12, 0)),
            (None, None)
        );
    }

    /// Before the first onset the file states an offset and classifies nothing, and the two
    /// methods say exactly that much each.
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

    /// The rule seam, from this side: a query past the last observance in the table is answered
    /// by asking the rule about the query's own year, not by materializing the years between.
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
