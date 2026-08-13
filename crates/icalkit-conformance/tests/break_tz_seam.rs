// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The seam between `ical-recur` and `ical-tz`, attacked from the side the two crates share.
//!
//! M1's own words were that "the two ends have to agree or every zoned series is an hour out
//! for half the year". Everything here drives one zoned series through both crates the way a
//! caller has to — `ical-tz` projects, `ical-recur` walks, `ical-tz` resolves each key — and
//! asks whether the answer is the one the zone's published rules give.
//!
//! Every expectation below is transcribed from the transition rules `America/New_York` and
//! `Australia/Lord_Howe` actually run, and not read off an answer this workspace gave:
//!
//! - `America/New_York` is `-05:00` standard and `-04:00` daylight, moving at 02:00 local on
//!   the second Sunday of March and the first Sunday of November. In 2026 those are March 8th
//!   and November 1st, so 09:00 in New York is 14:00Z until March 8th and 13:00Z from it.
//! - `Australia/Lord_Howe` is `+10:30` standard and `+11:00` daylight — half an hour, not a
//!   whole one — moving at 02:00 local on the first Sunday of October and of April. In 2026
//!   that is October 4th, so 09:00 there is 22:30Z the previous day until October 4th and
//!   22:00Z the previous day from it. Its local date is a day ahead of UTC all year, which is
//!   what makes a `Z`-terminated `UNTIL` land on the other side of midnight from `DTSTART`.
//!
//! The zone source below is written by hand, because ADR-0003 says a zone answer comes from a
//! caller-supplied source and this is what one costs. It is held against each fixture's own
//! `VTIMEZONE` first, so a case that fails fails against two independent statements of the same
//! rules rather than against one fixture's arithmetic.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, DateTimeValue, DecodeValue, Diagnostic,
    DiagnosticCode, Document, Instant, Limits, Meter, PropertyId, UtcOffset, ValueType,
};
use ical_recur::{
    Freq, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput, RecurrenceRule,
    RecurrenceRuleBuilder, SearchStep, ValueKind, Window, generation_window, max_absolute_shift,
    parse_recur,
};
use ical_tz::{
    AnswerBasis, ExclusionReading, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer,
    OrphanScan, Reading, ResolutionPolicy, ResolvedExclusions, WallClockShift, ZoneAnswer,
    ZoneProvenance, ZoneSource, ZonedSeries, extra_widening, nominal, wall_clock,
};

/// The property identities this file reads, as statics for the reason `break_zones` gives:
/// `Component::properties_named` ties the walk's lifetime to the borrow of the identity.
mod ids {
    use ical_core::PropertyId;

    /// `UID`, which is how one calendar's several series are told apart.
    pub(crate) static UID: PropertyId = PropertyId::UID;
    /// `DTSTART`, the wall clock a zoned series is anchored at.
    pub(crate) static DTSTART: PropertyId = PropertyId::DTSTART;
    /// `RRULE`, the one rule a series is expanded from.
    pub(crate) static RRULE: PropertyId = PropertyId::RRULE;
}

/// One calendar carrying `America/New_York` and seven series written against it.
const NEW_YORK_SERIES: &[u8] = include_bytes!("fixtures/break_tz_seam/new_york_series.ics");

/// `Australia/Lord_Howe` and a series whose `UNTIL` is a UTC instant on the previous UTC day.
const LORD_HOWE_SERIES: &[u8] =
    include_bytes!("fixtures/break_tz_seam/lord_howe_until_across_midnight.ics");

/// A zoned series whose `TZID` no `VTIMEZONE` in the file defines and no source recognizes.
const SERIES_WITHOUT_A_VTIMEZONE: &[u8] =
    include_bytes!("fixtures/break_tz_seam/series_without_a_vtimezone.ics");

/// The identifier every New York case names.
const NEW_YORK: &str = "America/New_York";

/// The identifier every Lord Howe case names.
const LORD_HOWE: &str = "Australia/Lord_Howe";

/// A wall clock as the tables here write one: year, month, day, hour, minute.
type Stamp = (u16, u8, u8, u8, u8);

/// One transition, transcribed from the rule the zone publishes.
#[derive(Clone, Copy, Debug)]
struct Move {
    /// The UTC wall clock at which the offset changed.
    at: Stamp,
    /// Seconds east of UTC before it.
    before: i32,
    /// Seconds east of UTC from it.
    after: i32,
    /// Whether the observance beginning here is the zone's daylight one.
    daylight: bool,
}

/// One zone as a caller's own database holds it.
#[derive(Clone, Debug)]
struct HandZone {
    /// The identifier, compared by exact bytes and never parsed.
    tzid: &'static str,
    /// Seconds east of UTC before the first transition.
    base: i32,
    /// Whether that observance is the zone's daylight one.
    base_daylight: bool,
    /// The transitions, ascending.
    moves: Vec<Move>,
}

/// The whole of a caller-supplied source: a handful of zones and no fallback anywhere.
///
/// Writing one is the cost ADR-0003 imposes on every caller, so it is written here rather than
/// borrowed from a fixture. What it takes is two methods, a candidate-offset loop for the hard
/// direction, and the observation that a wall clock is a real reading exactly when the instant
/// it names under an offset is an instant that offset governs.
#[derive(Clone, Debug)]
struct HandSource {
    /// The zones this database was wired with.
    zones: Vec<HandZone>,
}

impl HandSource {
    /// The zone `tzid` names, by exact bytes, with no alias table and no guessing.
    fn zone(&self, tzid: &str) -> Option<&HandZone> {
        self.zones.iter().find(|zone| zone.tzid == tzid)
    }
}

impl HandZone {
    /// The offset and daylight flag in force at `instant`.
    fn state_at(&self, instant: Instant) -> (i32, bool) {
        let mut state = (self.base, self.base_daylight);
        for shift in &self.moves {
            if utc(shift.at).is_some_and(|moment| moment <= instant) {
                state = (shift.after, shift.daylight);
            }
        }
        state
    }

    /// Every offset this zone has ever run, which is the candidate set a wall clock is read
    /// against.
    fn offsets(&self) -> Vec<i32> {
        let mut seen = vec![self.base];
        for shift in &self.moves {
            if !seen.contains(&shift.after) {
                seen.push(shift.after);
            }
        }
        seen
    }

    /// Every reading `local` has, ascending: none in a gap, two in a fold, one otherwise.
    fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
        let mut found: Vec<Reading> = Vec::new();
        for seconds in self.offsets() {
            let Some(offset) = UtcOffset::from_seconds(seconds) else {
                continue;
            };
            let Some(instant) = local.at_offset(offset) else {
                continue;
            };
            let (in_force, daylight) = self.state_at(instant);
            if in_force == seconds && !found.iter().any(|kept| kept.instant == instant) {
                found.push(Reading::new(instant, offset, daylight));
            }
        }
        found.sort_unstable();
        found
    }

    /// The gap `local` fell into, on the readings `ical-tz` states as invariants.
    fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
        for shift in &self.moves {
            let moment = utc(shift.at)?;
            let offset_before = UtcOffset::from_seconds(shift.before)?;
            let offset_after = UtcOffset::from_seconds(shift.after)?;
            let opened = CivilDateTime::from_instant(moment, offset_before)?;
            let closed = CivilDateTime::from_instant(moment, offset_after)?;
            if opened <= local && local < closed {
                return Some(LocalResolution::Nonexistent {
                    gap_start: moment.checked_add_seconds(-1)?,
                    gap_end: moment,
                    offset_before,
                    offset_after,
                    shifted: local.at_offset(offset_before)?,
                });
            }
        }
        None
    }
}

impl ZoneSource for HandSource {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        let zone = self.zone(tzid)?;
        let found = zone.readings(local);
        let resolution = match found.as_slice() {
            [reading] => LocalResolution::Unique { reading: *reading },
            [earlier, later, ..] => LocalResolution::Ambiguous {
                earlier: *earlier,
                later: *later,
            },
            [] => zone.gap(local)?,
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        let zone = self.zone(tzid)?;
        let (seconds, daylight) = zone.state_at(instant);
        Some(OffsetAnswer::new(
            UtcOffset::from_seconds(seconds)?,
            daylight,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }
}

/// The two zones this file asks about, as their governments decreed them.
fn hand_source() -> HandSource {
    HandSource {
        zones: vec![
            HandZone {
                tzid: NEW_YORK,
                base: -18_000,
                base_daylight: false,
                moves: vec![
                    Move {
                        at: (2025, 3, 9, 7, 0),
                        before: -18_000,
                        after: -14_400,
                        daylight: true,
                    },
                    Move {
                        at: (2025, 11, 2, 6, 0),
                        before: -14_400,
                        after: -18_000,
                        daylight: false,
                    },
                    Move {
                        at: (2026, 3, 8, 7, 0),
                        before: -18_000,
                        after: -14_400,
                        daylight: true,
                    },
                    Move {
                        at: (2026, 11, 1, 6, 0),
                        before: -14_400,
                        after: -18_000,
                        daylight: false,
                    },
                    Move {
                        at: (2027, 3, 14, 7, 0),
                        before: -18_000,
                        after: -14_400,
                        daylight: true,
                    },
                ],
            },
            HandZone {
                tzid: LORD_HOWE,
                base: 39_600,
                base_daylight: true,
                moves: vec![
                    Move {
                        at: (2026, 4, 4, 15, 0),
                        before: 39_600,
                        after: 37_800,
                        daylight: false,
                    },
                    Move {
                        at: (2026, 10, 3, 15, 30),
                        before: 37_800,
                        after: 39_600,
                        daylight: true,
                    },
                ],
            },
        ],
    }
}

/// The civil date and time a table entry spells.
fn local(stamp: Stamp) -> Option<CivilDateTime> {
    let date = CivilDate::from_ymd(stamp.0, stamp.1, stamp.2)?;
    Some(CivilDateTime::new(
        date,
        CivilTime::from_hms(stamp.3, stamp.4, 0)?,
    ))
}

/// The real UTC instant a table entry spells, for a column written with a trailing `Z`.
fn utc(stamp: Stamp) -> Option<Instant> {
    local(stamp)?.at_offset(UtcOffset::UTC)
}

/// The nominal cadence key a wall clock is, which is the timeline `ical-recur` walks.
fn cadence(stamp: Stamp) -> Option<Instant> {
    nominal(local(stamp)?)
}

/// One instant rendered as the UTC wall clock a person reading a failure needs.
fn render(instant: Instant) -> String {
    match CivilDateTime::from_instant(instant, UtcOffset::UTC) {
        None => format!("<{}>", instant.unix_seconds()),
        Some(civil) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}Z",
            civil.date().year(),
            civil.date().month(),
            civil.date().day(),
            civil.time().hour(),
            civil.time().minute()
        ),
    }
}

/// One instant rendered on a zone's own clock, which is what a series is written in.
fn on_the_clock(source: &HandSource, tzid: &str, instant: Instant) -> String {
    match source
        .offset_at(tzid, instant)
        .and_then(|answer| CivilDateTime::from_instant(instant, answer.offset))
    {
        None => format!("<{}>", instant.unix_seconds()),
        Some(civil) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            civil.date().year(),
            civil.date().month(),
            civil.date().day(),
            civil.time().hour(),
            civil.time().minute()
        ),
    }
}

/// The `VCALENDAR` a fixture's document holds.
fn calendar(document: &Document) -> Option<&Component> {
    document
        .components()
        .find(|component| component.is_named(b"VCALENDAR"))
}

/// The `VEVENT` whose `UID` is `uid`.
fn event<'a>(document: &'a Document, uid: &str) -> Option<&'a Component> {
    calendar(document)?.components().find(|component| {
        component.is_named(b"VEVENT")
            && component
                .properties_named(&ids::UID)
                .any(|property| property.value_text().as_bytes() == uid.as_bytes())
    })
}

/// The typed value of the property `id` names on `component`.
fn value<'a>(component: &'a Component, id: &'a PropertyId) -> Option<DateTimeValue<'a>> {
    DateTimeValue::decode_property(component.properties_named(id).next()?).ok()
}

/// The one `RRULE` a component carries, read through the grammar rather than rebuilt.
fn rule_of(
    component: &Component,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<RecurrenceRule> {
    let property = component.properties_named(&ids::RRULE).next()?;
    parse_recur(property.value_text().as_bytes(), meter, sink).ok()
}

/// A fixture parsed, with nothing decided about it yet.
///
/// Answers rather than asserts, per the convention the other files in this corpus use: a helper
/// below a `#[test]` is production code as far as the lint profile is concerned, so what a case
/// is entitled to assume is stated at the case.
fn parsed(octets: &[u8]) -> Option<Document> {
    let mut sink: Vec<Diagnostic> = Vec::new();
    Document::parse(octets, Limits::DEFAULT, &mut sink).ok()
}

/// A meter and a sink, which every call across the seam needs and no case wants to spell twice.
fn ledger() -> (Meter, Vec<Diagnostic>) {
    (Meter::new(Limits::DEFAULT), Vec::new())
}

/// The cadence keys and effective starts one series emits over `window`.
///
/// Collected before anything is resolved, because the search borrows the meter for its whole
/// life — which is the shape of the seam and not an inconvenience: `ical-recur` is finished with
/// an occurrence before `ical-tz` is asked about it.
fn walk(
    input: RecurrenceInput<'_>,
    window: Window,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Vec<(Instant, Instant)> {
    let mut emitted = Vec::new();
    for step in input.search(window, meter, sink) {
        if let SearchStep::Occurrence(occurrence) = step {
            emitted.push((occurrence.key(), occurrence.start()));
        }
    }
    emitted
}

/// One series assembled from a fixture's `DTSTART` and `RRULE`, with nothing else on it.
fn plain_input<'a>(
    anchor: Instant,
    rule: &'a RecurrenceRule,
    meter: &mut Meter,
) -> Option<RecurrenceInput<'a>> {
    RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(rule),
        &[],
        &[],
        OverrideSet::empty(),
        meter,
    )
    .ok()
}

/// Every occurrence of one fixture series, as the instant it happens at and the clock it shows.
struct Run {
    /// The nominal cadence keys `ical-recur` emitted.
    keys: Vec<Instant>,
    /// What each key resolved to, with a skipped gap kept as `None`.
    starts: Vec<Option<Instant>>,
}

impl Run {
    /// The instants that really happened, in order.
    fn happened(&self) -> Vec<Instant> {
        self.starts.iter().copied().flatten().collect()
    }

    /// The wall clock each of those shows in `tzid`.
    fn clocks(&self, source: &HandSource, tzid: &str) -> Vec<String> {
        self.happened()
            .into_iter()
            .map(|moment| on_the_clock(source, tzid, moment))
            .collect()
    }
}

/// Expand one `UID` of the New York fixture over `window` and resolve every key.
fn run_new_york(
    uid: &str,
    window: (Stamp, Stamp),
    policy: ResolutionPolicy,
) -> Option<(Run, Vec<Diagnostic>)> {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES)?;
    let component = event(&document, uid)?;
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, policy);
    let anchor = series.anchor(value(component, &ids::DTSTART)?)?;
    let rule = rule_of(component, &mut meter, &mut sink)?;
    let input = plain_input(anchor, &rule, &mut meter)?;
    let span = Window::new(cadence(window.0)?, cadence(window.1)?)?;
    let emitted = walk(input, span, &mut meter, &mut sink);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let starts = emitted
        .iter()
        .map(|(key, _)| series.actual(*key, &mut meter, &mut reported))
        .collect();
    let keys = emitted.iter().map(|(key, _)| *key).collect();
    Some((Run { keys, starts }, reported))
}

/// RFC 5545 sections 3.6.5 and 3.8.5.3. The hand-written source and the file's own `VTIMEZONE`
/// answer the same questions the same way, so a failure below is a failure of the seam and not
/// of one transcription.
#[test]
fn the_caller_supplied_source_and_the_files_own_vtimezone_agree_about_both_transitions() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let (mut meter, mut sink) = ledger();
    let zones = ical_tz::read_calendar_zones(
        calendar(&document).expect("a VCALENDAR"),
        &mut meter,
        &mut sink,
    );
    let table = zones.table(NEW_YORK).expect("the file defines New York");
    for asked in [
        (2026, 3, 7, 9, 0),
        (2026, 3, 9, 9, 0),
        (2026, 10, 31, 9, 0),
        (2026, 11, 2, 9, 0),
        (2026, 3, 8, 2, 30),
        (2026, 11, 1, 1, 30),
    ] {
        let clock = local(asked).expect("a real wall clock");
        let mine = source
            .resolve(NEW_YORK, clock)
            .expect("the source knows it");
        let theirs = table.resolve(NEW_YORK, clock).expect("the file knows it");
        assert_eq!(
            mine.resolution, theirs.resolution,
            "{asked:?}: the two statements of one zone's rules disagree"
        );
    }
}

/// RFC 5545 section 3.8.5.3. A daily 09:00 series in New York, across both 2026 transitions.
///
/// Every occurrence must show 09:00 on the zone's own clock, and the UTC instants must therefore
/// step by 23 hours across the spring forward and 25 across the fall back.
#[test]
fn a_daily_nine_oclock_series_shows_nine_on_both_sides_of_both_new_york_transitions() {
    for (window, expected) in [
        (
            ((2026, 3, 6, 0, 0), (2026, 3, 11, 0, 0)),
            [
                (2026, 3, 6, 14, 0),
                (2026, 3, 7, 14, 0),
                (2026, 3, 8, 13, 0),
                (2026, 3, 9, 13, 0),
                (2026, 3, 10, 13, 0),
            ],
        ),
        (
            ((2026, 10, 30, 0, 0), (2026, 11, 4, 0, 0)),
            [
                (2026, 10, 30, 13, 0),
                (2026, 10, 31, 13, 0),
                (2026, 11, 1, 14, 0),
                (2026, 11, 2, 14, 0),
                (2026, 11, 3, 14, 0),
            ],
        ),
    ] {
        let (run, _reported) =
            run_new_york("daily-nine@example.test", window, ResolutionPolicy::DEFAULT)
                .expect("the fixture carries this series");
        let wanted: Vec<String> = expected
            .into_iter()
            .map(|stamp| render(utc(stamp).expect("a real instant")))
            .collect();
        let got: Vec<String> = run.happened().into_iter().map(render).collect();
        assert_eq!(got, wanted, "the instants New York's rules give");
        let source = hand_source();
        for clock in run.clocks(&source, NEW_YORK) {
            assert!(clock.ends_with("T09:00"), "{clock} is not nine o'clock");
        }
    }
}

/// RFC 5545 section 3.8.5.3. An interval that steps over the transition without landing near it.
///
/// `FREQ=WEEKLY;INTERVAL=2` from Sunday March 1st recurs on the 15th and the 29th; the clocks
/// moved on the 8th, which no occurrence is within a week of. The wall clock must not notice.
#[test]
fn a_fortnightly_series_that_steps_over_the_transition_keeps_its_wall_clock() {
    let (run, _reported) = run_new_york(
        "fortnightly-nine@example.test",
        ((2026, 3, 1, 0, 0), (2026, 4, 1, 0, 0)),
        ResolutionPolicy::DEFAULT,
    )
    .expect("the fixture carries this series");
    let wanted: Vec<String> = [
        (2026, 3, 1, 14, 0),
        (2026, 3, 15, 13, 0),
        (2026, 3, 29, 13, 0),
    ]
    .into_iter()
    .map(|stamp| render(utc(stamp).expect("a real instant")))
    .collect();
    let got: Vec<String> = run.happened().into_iter().map(render).collect();
    assert_eq!(got, wanted, "a fortnightly cadence over a transition");
}

/// RFC 5545 section 3.8.5.3, with the month arithmetic ADR-0011 fixed underneath it.
#[test]
fn a_monthly_series_that_steps_over_the_transition_keeps_its_wall_clock() {
    let (run, _reported) = run_new_york(
        "monthly-fifteenth@example.test",
        ((2026, 1, 1, 0, 0), (2026, 5, 1, 0, 0)),
        ResolutionPolicy::DEFAULT,
    )
    .expect("the fixture carries this series");
    let wanted: Vec<String> = [
        (2026, 1, 15, 14, 0),
        (2026, 2, 15, 14, 0),
        (2026, 3, 15, 13, 0),
        (2026, 4, 15, 13, 0),
    ]
    .into_iter()
    .map(|stamp| render(utc(stamp).expect("a real instant")))
    .collect();
    let got: Vec<String> = run.happened().into_iter().map(render).collect();
    assert_eq!(got, wanted, "a monthly cadence over a transition");
}

/// RFC 5545 section 3.3.10 against ADR-0011. `COUNT` is counted on the wrong side of the seam.
///
/// ADR-0011 states two gates — a date that exists and a local time that exists — and says "an
/// instance is admitted only when both gates pass" and that `COUNT` "counts emitted instances
/// only". `ical-recur` applies `COUNT` and has no zone; `ical-tz` applies the second gate one
/// occurrence at a time afterwards. So a `COUNT=5` series whose third cadence key falls in the
/// hour New York never showed delivered four occurrences, and no API composed the two gates in
/// the order the ADR states.
///
/// The composition is now expressible and this case drives it: `ZonedSeries::admits` is the
/// second gate as a predicate and `RecurrenceInput::admitting` is where it goes, asked after
/// the window and before the count. Five occurrences reach the caller, the last on the 11th of
/// March, and the hour the zone sprang over is still a fact anybody can ask about.
///
/// Two gates cannot be composed by the crate that holds one of them, which is why this is a
/// caller's call and not a default: the same series without a gate keeps M1's answer, which is
/// also section 3.8.5.3's reading for a `DTSTART` that lands in a gap — the instance is
/// ignored and the count is spent. The case below it holds that reading.
#[test]
fn a_count_bounded_series_that_crosses_a_gap_delivers_every_occurrence_it_promises() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "counted-over-the-gap@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let anchor = series
        .anchor(value(component, &ids::DTSTART).expect("a zoned DTSTART"))
        .expect("the anchor projects");
    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");
    let admitted = |key: Instant| series.admits(key);
    let input = plain_input(anchor, &rule, &mut meter)
        .expect("the input assembles")
        .admitting(&admitted);
    let span = Window::new(
        cadence((2026, 3, 1, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 20, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let emitted = walk(input, span, &mut meter, &mut sink);

    let mut reported: Vec<Diagnostic> = Vec::new();
    let happened: Vec<String> = emitted
        .iter()
        .filter_map(|(key, _)| series.actual(*key, &mut meter, &mut reported))
        .map(render)
        .collect();
    let wanted: Vec<String> = [
        (2026, 3, 6, 7, 30),
        (2026, 3, 7, 7, 30),
        (2026, 3, 9, 6, 30),
        (2026, 3, 10, 6, 30),
        (2026, 3, 11, 6, 30),
    ]
    .into_iter()
    .map(|stamp| render(utc(stamp).expect("a real instant")))
    .collect();
    assert_eq!(
        happened, wanted,
        "COUNT counts emitted instances, and a skipped gap is not one"
    );

    let sprang_over = cadence((2026, 3, 8, 2, 30)).expect("the key the gate refused");
    assert_eq!(
        series.actual(sprang_over, &mut meter, &mut reported),
        None,
        "02:30 on the second Sunday of March is an hour New York never showed"
    );
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::NonexistentLocalTime),
        "and the hour it never showed is a reported fact rather than a silent one"
    );
}

/// RFC 5545 section 3.3.10. A series whose `DTSTART` is itself at a nonexistent local time.
///
/// Section 3.8.5.3 says `DTSTART` is the first instance of the recurrence set; section 3.3.10
/// says an instance at a nonexistent local time MUST be ignored. Under the default policy the
/// two combine into a series whose first instance never happens, which is a reading the
/// specification permits — so what is asserted here is only that the rest of the series survives
/// and stays on its own wall clock.
#[test]
fn a_series_anchored_in_the_hour_that_does_not_exist_keeps_the_rest_of_its_cadence() {
    let (run, reported) = run_new_york(
        "anchored-in-the-gap@example.test",
        ((2026, 3, 1, 0, 0), (2026, 3, 20, 0, 0)),
        ResolutionPolicy::DEFAULT,
    )
    .expect("the fixture carries this series");
    assert_eq!(run.keys.len(), 3, "COUNT=3 generates three cadence keys");
    assert_eq!(run.starts[0], None, "the anchor itself is the skipped one");
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::NonexistentLocalTime),
        "the anchor's own wall clock is an hour the zone sprang over"
    );
    let wanted: Vec<String> = [(2026, 3, 9, 6, 30), (2026, 3, 10, 6, 30)]
        .into_iter()
        .map(|stamp| render(utc(stamp).expect("a real instant")))
        .collect();
    let got: Vec<String> = run.happened().into_iter().map(render).collect();
    assert_eq!(got, wanted, "02:30 EDT is 06:30Z on the days that have one");

    let shifting = ResolutionPolicy::DEFAULT.with_gaps(GapPolicy::ShiftForward);
    let (shifted, _) = run_new_york(
        "anchored-in-the-gap@example.test",
        ((2026, 3, 1, 0, 0), (2026, 3, 20, 0, 0)),
        shifting,
    )
    .expect("the fixture carries this series");
    assert_eq!(
        shifted.happened().len(),
        3,
        "section 3.3.5 reads a gap with the offset in force before it, and loses nothing"
    );
}

/// RFC 5545 section 3.3.10. A series whose `DTSTART` is at a local time that happens twice.
///
/// The fold policy says which of the two the series takes, and every later occurrence is an
/// ordinary 01:30 in a day with only one of them.
#[test]
fn a_series_anchored_in_the_hour_that_happens_twice_takes_the_side_the_policy_names() {
    for (folds, first) in [
        (FoldPolicy::Earlier, (2026, 11, 1, 5, 30)),
        (FoldPolicy::Later, (2026, 11, 1, 6, 30)),
    ] {
        let policy = ResolutionPolicy::DEFAULT.with_folds(folds);
        let (run, reported) = run_new_york(
            "anchored-in-the-fold@example.test",
            ((2026, 10, 25, 0, 0), (2026, 11, 10, 0, 0)),
            policy,
        )
        .expect("the fixture carries this series");
        let wanted: Vec<String> = [first, (2026, 11, 2, 6, 30), (2026, 11, 3, 6, 30)]
            .into_iter()
            .map(|stamp| render(utc(stamp).expect("a real instant")))
            .collect();
        let got: Vec<String> = run.happened().into_iter().map(render).collect();
        assert_eq!(got, wanted, "{folds:?}");
        assert!(
            reported
                .iter()
                .any(|entry| entry.code() == DiagnosticCode::AmbiguousLocalTime),
            "an hour that happens twice is a fact the caller has to be able to see"
        );
    }
}

/// RFC 5545 section 3.3.10. A `Z`-terminated `UNTIL` whose UTC day is not the zone's day.
///
/// `UNTIL=20261004T220000Z` is 09:00 on October 5th in Lord Howe, one calendar day later than
/// the value's own UTC date, and October 5th's occurrence is exactly the bound — so the series
/// ends on the 5th. Taking the instant as written ends it on the 4th.
#[test]
fn a_utc_until_that_falls_on_the_far_side_of_midnight_keeps_the_named_days_instance() {
    let source = hand_source();
    let document = parsed(LORD_HOWE_SERIES).expect("the fixture parses");
    let component = event(&document, "lord-howe-nine@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, LORD_HOWE, ResolutionPolicy::DEFAULT);
    let start = value(component, &ids::DTSTART).expect("a zoned DTSTART");
    let anchor = series.anchor(start).expect("the anchor projects");

    let written = local((2026, 10, 4, 22, 0)).expect("the UNTIL the file wrote");
    let bound = series
        .project_until(DateTimeValue::Utc(written), start, &mut meter, &mut sink)
        .expect("the bound projects");
    assert_eq!(
        bound,
        cadence((2026, 10, 5, 9, 0)).expect("09:00 on the 5th"),
        "22:00Z on the 4th is 09:00 on the 5th in Lord Howe"
    );

    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");
    let restated = RecurrenceRuleBuilder::new(rule.freq())
        .limit(ical_recur::RuleLimit::Until {
            at: bound,
            value_kind: ValueKind::DateTime,
            clock: ical_recur::UntilClock::Utc,
        })
        .build()
        .expect("a daily rule with a projected bound");
    let input = plain_input(anchor, &restated, &mut meter).expect("the input assembles");
    let window = Window::new(
        cadence((2026, 10, 1, 0, 0)).expect("a real wall clock"),
        cadence((2026, 10, 10, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let emitted = walk(input, window, &mut meter, &mut sink);
    let last = wall_clock(emitted.last().expect("occurrences").0).expect("a key reads back");
    assert_eq!(
        last.date(),
        CivilDate::from_ymd(2026, 10, 5).expect("a real date"),
        "the bound names the 5th's own occurrence and UNTIL is inclusive"
    );
    let starts: Vec<String> = emitted
        .iter()
        .map(|(key, _)| {
            render(
                series
                    .actual(*key, &mut meter, &mut sink)
                    .expect("every one of these exists"),
            )
        })
        .collect();
    let wanted: Vec<String> = [
        (2026, 9, 30, 22, 30),
        (2026, 10, 1, 22, 30),
        (2026, 10, 2, 22, 30),
        (2026, 10, 3, 22, 0),
        (2026, 10, 4, 22, 0),
    ]
    .into_iter()
    .map(|stamp| render(utc(stamp).expect("a real instant")))
    .collect();
    assert_eq!(
        starts, wanted,
        "Lord Howe moves by half an hour, not a whole one"
    );
}

/// RFC 5545 section 3.3.10. The `UNTIL` a parsed rule carries is on the other timeline, and
/// correcting it is now one call rather than a rebuild.
///
/// `parse_recur` reads `UNTIL=20260310T120000Z` into a real UTC instant. The cadence keys it
/// will be compared against are nominal. Searching the rule as parsed answers with a series one
/// day too long — its last occurrence happens at 13:00Z, an hour past the bound the file wrote
/// — and until `RecurrenceRule::with_limit` existed the only way to substitute the projection
/// was to rebuild the rule through `RecurrenceRuleBuilder` and copy every `BYxxx` list across by
/// hand, which is a correction no caller performs by accident.
///
/// What is asserted is the correction being reachable and right: `project_until` gives the
/// nominal bound, `with_limit` puts it on the rule the file was read into, and the series ends
/// on the 9th. That a caller must know to do it at all is the seam's remaining cost, stated in
/// `ical_tz::seam` and in `ical_recur::RecurrenceRule::with_limit`.
#[test]
fn a_parsed_rules_utc_until_is_correctable_without_rebuilding_the_rule() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "until-in-utc@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let start = value(component, &ids::DTSTART).expect("a zoned DTSTART");
    let anchor = series.anchor(start).expect("the anchor projects");
    let as_parsed = rule_of(component, &mut meter, &mut sink).expect("one RRULE");

    let written = local((2026, 3, 10, 12, 0)).expect("the UNTIL the file wrote");
    let bound = series
        .project_until(DateTimeValue::Utc(written), start, &mut meter, &mut sink)
        .expect("the bound projects");
    assert_eq!(
        bound,
        cadence((2026, 3, 10, 8, 0)).expect("08:00 on the 10th"),
        "12:00Z is 08:00 in New York that day, which is before the morning's instance"
    );

    let window = Window::new(
        cadence((2026, 3, 1, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 20, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let bounded = as_parsed.with_limit(ical_recur::RuleLimit::Until {
        at: bound,
        value_kind: ValueKind::DateTime,
        clock: ical_recur::UntilClock::Utc,
    });
    let input = plain_input(anchor, &bounded, &mut meter).expect("the input assembles");
    let emitted = walk(input, window, &mut meter, &mut sink);
    let last_key = emitted.last().expect("occurrences").0;
    let last_instant = series
        .actual(last_key, &mut meter, &mut sink)
        .expect("the last occurrence exists");
    let written_instant = utc((2026, 3, 10, 12, 0)).expect("the instant the file's UNTIL names");
    assert!(
        last_instant <= written_instant,
        "the file said the series stops at {} and its last occurrence happens at {}, on {}",
        render(written_instant),
        render(last_instant),
        wall_clock(last_key).expect("a key reads back").date().day()
    );
    assert_eq!(
        wall_clock(last_key).expect("a key reads back").date(),
        CivilDate::from_ymd(2026, 3, 9).expect("a real date"),
        "the projected bound falls before the 10th's own instance"
    );
}

/// RFC 5545 sections 3.8.4.4 and 3.2.13. A `RANGE=THISANDFUTURE` override across a transition.
///
/// The organizer moved the 09:00 standup to 11:00 from March 5th, four days before New York
/// sprang forward. Every later occurrence must show 11:00 on the organizer's clock.
#[test]
fn a_this_and_future_shift_keeps_its_wall_clock_across_the_transition_it_reaches_over() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "daily-nine@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let start = value(component, &ids::DTSTART).expect("a zoned DTSTART");
    let anchor = series.anchor(start).expect("the anchor projects");
    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");

    let addressed = cadence((2026, 3, 5, 9, 0)).expect("the key the override addresses");
    let landed = cadence((2026, 3, 5, 11, 0)).expect("where the organizer moved it");
    let entries = [Override::new(
        addressed,
        OverrideRange::ThisAndFuture,
        Some(landed),
        PropertyDiff::empty(),
    )];
    let overrides = OverrideSet::new(&entries, &mut meter).expect("one ascending override");
    let input = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        overrides,
        &mut meter,
    )
    .expect("the input assembles");
    let asked = Window::new(
        cadence((2026, 3, 4, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 11, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let generated = generation_window(asked, overrides).expect("a representable window");
    let emitted = walk(input, generated, &mut meter, &mut sink);

    let mut clocks = Vec::new();
    for (_key, moved) in &emitted {
        if !asked.contains(*moved) {
            continue;
        }
        let happened = series
            .actual(*moved, &mut meter, &mut sink)
            .expect("every one of these exists");
        clocks.push(on_the_clock(&source, NEW_YORK, happened));
    }
    let wanted = vec![
        "2026-03-04T09:00".to_owned(),
        "2026-03-05T11:00".to_owned(),
        "2026-03-06T11:00".to_owned(),
        "2026-03-07T11:00".to_owned(),
        "2026-03-08T11:00".to_owned(),
        "2026-03-09T11:00".to_owned(),
        "2026-03-10T11:00".to_owned(),
    ];
    assert_eq!(clocks, wanted, "the organizer's move is a wall-clock move");
}

/// RFC 5545 section 3.2.13 against `max_absolute_shift`'s own documentation, which M2 corrected.
///
/// The documentation used to call the number a count of *elapsed* seconds — "the whole of the
/// move for a floating or UTC series and only part of it for a zoned one" — with
/// `ical_tz::extra_widening` there to add back what an elapsed count could not see. That is not
/// a reading of anything. Every instant crossing the seam is nominal, so the two instants an
/// override names are wall clocks and their difference is the wall-clock count: on that
/// timeline the shortfall `extra_widening` reports is always zero, and on the real one
/// `max_absolute_shift` is not measuring the move that gets propagated. The two were never two
/// halves of one number.
///
/// What holds, and is asserted here: the number is a difference of the caller's own instants,
/// the widening it gives a zoned series is exact for the wall-clock move that series
/// propagates, and `extra_widening` over shifts measured on the same timeline is zero. See
/// `docs/adr/0002` amendment 8.
#[test]
fn max_absolute_shift_counts_the_timeline_the_callers_own_instants_are_on() {
    let source = hand_source();
    let addressed = cadence((2026, 3, 7, 9, 0)).expect("the key the override addresses");
    let landed = cadence((2026, 3, 9, 9, 0)).expect("two days later on the organizer's clock");
    let entries = [Override::new(
        addressed,
        OverrideRange::ThisAndFuture,
        Some(landed),
        PropertyDiff::empty(),
    )];
    let (mut meter, _sink) = ledger();
    let overrides = OverrideSet::new(&entries, &mut meter).expect("one ascending override");

    assert_eq!(
        max_absolute_shift(overrides),
        172_800,
        "two days of the organizer's clock, which is what the two nominal instants differ by"
    );
    let elapsed = utc((2026, 3, 7, 14, 0))
        .expect("09:00 EST")
        .checked_seconds_until(utc((2026, 3, 9, 13, 0)).expect("09:00 EDT"))
        .expect("a representable difference");
    assert_eq!(
        elapsed, 169_200,
        "the same move costs two days less an hour of real time, which is the other reading"
    );

    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let measured = WallClockShift::across(&series, addressed, landed).expect("both keys resolve");
    assert_eq!(measured.elapsed_seconds(), elapsed);
    assert_eq!(measured.wall_clock_seconds(), 172_800);
    let nominal_shift = WallClockShift::new(172_800, 172_800);
    assert_eq!(
        extra_widening(&[nominal_shift]),
        0,
        "a widening taken on the nominal timeline is already exact, and this adds nothing to it"
    );
}

/// RFC 5545 section 3.2.13. A shift measured on the instants the seam actually carries.
///
/// `WallClockShift::measure` reads each end through `ZoneSource::offset_at`, so its two
/// arguments are real UTC instants. The two instants an override names — its `RECURRENCE-ID`
/// and where it moved to — are nominal, five hours from the real ones in New York, and fed to
/// `measure` they put the transition on the wrong side of the move: it answered that a move
/// straddling the spring forward crossed no transition at all, which is the one question the
/// type exists to answer about the one case it was written for.
///
/// `WallClockShift::across` is the conversion, in the crate that owns it. Handed the two keys
/// the seam carries, it resolves each against the zone under the series' own policy and
/// measures the real instants — so it agrees, value for value, with `measure` handed those
/// instants directly.
#[test]
fn a_shift_measured_across_two_cadence_keys_finds_the_transition_between_them() {
    let source = hand_source();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let addressed = cadence((2026, 3, 7, 9, 0)).expect("09:00 the day before the clocks moved");
    let landed = cadence((2026, 3, 8, 4, 0)).expect("04:00 the morning after they did");

    let truth = WallClockShift::measure(
        &source,
        NEW_YORK,
        utc((2026, 3, 7, 14, 0)).expect("09:00 EST"),
        utc((2026, 3, 8, 8, 0)).expect("04:00 EDT"),
    )
    .expect("both ends are in the zone");
    assert_eq!(
        (truth.elapsed_seconds(), truth.wall_clock_seconds()),
        (64_800, 68_400),
        "nineteen hours of clock cost eighteen of timeline across the spring forward"
    );
    assert!(truth.crossed_a_transition());

    let measured =
        WallClockShift::across(&series, addressed, landed).expect("both keys resolve in the zone");
    assert_eq!(
        measured, truth,
        "the two keys the seam carries name the two instants measured above"
    );
    assert!(
        measured.crossed_a_transition(),
        "a transition falls inside this move: elapsed {}, wall clock {}",
        measured.elapsed_seconds(),
        measured.wall_clock_seconds()
    );
    assert_eq!(
        extra_widening(&[measured]),
        3_600,
        "the elapsed count is an hour short of the wall-clock move, and that is what a caller \
         measuring on the real timeline adds to a widening derived from it"
    );
}

/// RFC 5545 section 3.8.4.4. Two overrides at the two halves of the hour that happens twice
/// collapse onto one cadence key, and `RecurrenceInput` then refuses the whole series.
///
/// `RECURRENCE-ID:20261101T053000Z` and `RECURRENCE-ID:20261101T063000Z` name two real instants
/// an hour apart, both of which read 01:30 in New York. The projection onto the nominal timeline
/// is not injective across a fold, so both become the same key — and `OverrideSet::new` refuses
/// a duplicated identifier, which loses not the second override but every occurrence of the
/// event.
#[test]
fn two_overrides_inside_one_fold_collapse_onto_one_key_and_lose_the_whole_series() {
    let source = hand_source();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let earlier = series
        .anchor(DateTimeValue::Utc(
            local((2026, 11, 1, 5, 30)).expect("01:30 EDT"),
        ))
        .expect("the identifier projects");
    let later = series
        .anchor(DateTimeValue::Utc(
            local((2026, 11, 1, 6, 30)).expect("01:30 EST"),
        ))
        .expect("the identifier projects");
    assert_eq!(
        earlier, later,
        "the projection collapses the repeated hour, which is what the rest of this asks about"
    );

    let entries = [
        Override::new(
            earlier,
            OverrideRange::ThisOnly,
            None,
            PropertyDiff::empty(),
        ),
        Override::new(later, OverrideRange::ThisOnly, None, PropertyDiff::empty()),
    ];
    let (mut meter, _sink) = ledger();
    let assembled = OverrideSet::new(&entries, &mut meter).map(|_| ());
    assert_eq!(
        assembled,
        Ok(()),
        "two overrides naming two real instants cost the caller the whole expansion"
    );
}

/// RFC 5545 section 3.8.5.1. An `EXDATE` written as a `DATE` against a date-time `DTSTART`.
///
/// The literal reading removes nothing and says so; the whole-day reading removes the day the
/// producer named, and that day is 23 hours long because New York sprang forward inside it.
#[test]
fn a_date_exdate_against_a_date_time_series_removes_the_day_under_the_stated_reading() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "daily-nine@example.test").expect("the series");
    let excluded = [DateTimeValue::Date(
        CivilDate::from_ymd(2026, 3, 8).expect("the day the clocks moved"),
    )];
    for (reading, survivors) in [
        (ExclusionReading::Instantaneous, 5),
        (ExclusionReading::WholeDay, 4),
    ] {
        let (mut meter, mut sink) = ledger();
        let policy = ResolutionPolicy::DEFAULT.with_exclusions(reading);
        let series = ZonedSeries::new(&source, NEW_YORK, policy);
        let anchor = series
            .anchor(value(component, &ids::DTSTART).expect("a zoned DTSTART"))
            .expect("the anchor projects");
        let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");
        let mut reported: Vec<Diagnostic> = Vec::new();
        let resolved = ResolvedExclusions::read(
            &series,
            ValueType::DateTime,
            &excluded,
            &mut meter,
            &mut reported,
        );
        assert!(
            reported
                .iter()
                .any(|entry| entry.code() == DiagnosticCode::ExdateValueTypeMismatch),
            "{reading:?}: a silent no-op is the one outcome that is indefensible"
        );
        let instants = resolved.instants().to_vec();
        let input = RecurrenceInput::new(
            anchor,
            ValueKind::DateTime,
            Some(&rule),
            &[],
            &instants,
            OverrideSet::empty(),
            &mut meter,
        )
        .expect("the input assembles");
        let window = Window::new(
            cadence((2026, 3, 6, 0, 0)).expect("a real wall clock"),
            cadence((2026, 3, 11, 0, 0)).expect("a real wall clock"),
        )
        .expect("a window is not empty");
        let kept = walk(input, window, &mut meter, &mut sink)
            .into_iter()
            .filter(|(key, _)| !resolved.excludes(*key))
            .count();
        assert_eq!(kept, survivors, "{reading:?}");
    }
}

/// RFC 5545 section 3.8.5.1 and section 3.2.19. An exception written in UTC against a series
/// whose zone nothing defines.
///
/// The file declares no `VTIMEZONE` at all, which is ordinary: `TZID:Customized Time Zone` is
/// what Outlook writes and a `VTIMEZONE` for it is what half the exporters forget. The series
/// still expands, because a zoned `DTSTART` is a wall clock and needs no zone to be projected.
/// The `Z`-terminated `EXDATE` does need one — and `admit`'s own comment used to justify
/// dropping it with "a series whose `DTSTART` carries the same `Z` has no anchor either", which
/// is exactly the case that does not arise here.
///
/// **The divergence this case now records.** Nothing in this workspace can place that
/// exception: it names a real instant, the cadence keys it would have to match are on the
/// series' own wall clock, and only the zone converts between the two. The three answers
/// available were to invent an offset (UTC, which `docs/adr/0003` refuses outright), to read
/// the value's fields as though they were a wall clock (what a client that ignores `TZID`
/// does, and it removes an occurrence the producer did not name), or to keep the exception as
/// what it is and say so. The third is what ships: `ResolvedExclusions::unplaced` holds the
/// real instant, `exdate-zone-unknown` travels with it, and the occurrence stays in the series
/// until a caller that has the zone applies it. The meeting is still there — and now the file
/// says out loud that an exception could not be applied, which is the half that was missing.
#[test]
fn a_utc_exdate_on_a_series_whose_zone_nothing_defines_is_kept_and_reported() {
    let source = hand_source();
    let document = parsed(SERIES_WITHOUT_A_VTIMEZONE).expect("the fixture parses");
    let component = event(&document, "no-vtimezone-anywhere@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, "Customized Time Zone", ResolutionPolicy::DEFAULT);
    let anchor = series
        .anchor(value(component, &ids::DTSTART).expect("a zoned DTSTART"))
        .expect("a wall clock needs no zone, so the series does expand");
    assert_eq!(
        anchor,
        cadence((2026, 3, 1, 9, 0)).expect("a real wall clock")
    );

    let excluded = [DateTimeValue::Utc(
        local((2026, 3, 8, 14, 0)).expect("the exception the producer wrote"),
    )];
    let mut reported: Vec<Diagnostic> = Vec::new();
    let resolved = ResolvedExclusions::read(
        &series,
        ValueType::DateTime,
        &excluded,
        &mut meter,
        &mut reported,
    );
    assert!(
        !resolved.is_empty() && !reported.is_empty(),
        "an exception that removes nothing and reports nothing is the silent no-op this crate \
         says is the one indefensible outcome"
    );
    assert_eq!(
        reported
            .iter()
            .map(|entry| entry.code())
            .collect::<Vec<_>>(),
        vec![DiagnosticCode::ExdateZoneUnknown],
        "the code says what happened: no source recognized the zone this value needed"
    );
    assert_eq!(
        resolved.unplaced(),
        [utc((2026, 3, 8, 14, 0)).expect("the instant the producer wrote")],
        "the exception is kept as the real instant it names, for a caller that has the zone"
    );
    assert!(
        resolved.instants().is_empty(),
        "and it is not guessed onto the series' own timeline, which would remove an occurrence \
         the producer never named"
    );

    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");
    let instants = resolved.instants().to_vec();
    let input = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &instants,
        OverrideSet::empty(),
        &mut meter,
    )
    .expect("the input assembles");
    let window = Window::new(
        cadence((2026, 3, 6, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 11, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let kept = walk(input, window, &mut meter, &mut sink).len();
    assert_eq!(
        kept, 5,
        "the occurrence stays, because nothing here can place the exception on the series' own \
         clock — and the drop is now reported rather than silent"
    );
}

/// RFC 5545 section 3.8.4.4. An override whose `RECURRENCE-ID` names no generated instant.
///
/// The client wrote the identifier in UTC and computed it with the winter offset, so it names
/// 14:00Z on a morning the series really generates at 13:00Z. Nothing in `ical-recur` says a
/// word — the merge simply never matches it — and `OrphanScan` is what makes the silence a
/// report.
#[test]
fn an_override_addressing_an_instant_the_series_never_generates_is_reported_as_inert() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "daily-nine@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let start = value(component, &ids::DTSTART).expect("a zoned DTSTART");
    let anchor = series.anchor(start).expect("the anchor projects");
    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");

    let misread = series
        .anchor(DateTimeValue::Utc(
            local((2026, 3, 8, 14, 0)).expect("09:00 read with the winter offset"),
        ))
        .expect("the identifier projects");
    let real = cadence((2026, 3, 8, 9, 0)).expect("the key the series really generates");
    assert_ne!(misread, real, "the client's identifier is an hour out");

    let identifiers = [misread];
    let mut scan = OrphanScan::new(&identifiers);
    let input = plain_input(anchor, &rule, &mut meter).expect("the input assembles");
    let window = Window::new(
        cadence((2026, 3, 1, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 20, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    for (key, _) in walk(input, window, &mut meter, &mut sink) {
        scan.observe(key);
    }
    let mut reported: Vec<Diagnostic> = Vec::new();
    assert_eq!(scan.finish(&mut meter, &mut reported), 1);
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::OverrideMatchesNoInstance),
        "every other silent drop in this workspace has a code"
    );
}

/// RFC 5545 section 3.8.4.4. Which instant the seam's per-occurrence resolver is to be handed.
///
/// `ZonedSeries::actual` was documented as "the instant the occurrence at cadence key `key`
/// actually happens at", and a caller reading that sentence resolves `Occurrence::key` — which
/// is what `break_zones.rs`'s own helper did. For an occurrence a `RANGE=THISANDFUTURE`
/// override moved, the key is where the base rule put it and the effective start is where the
/// organizer put it, so resolving the key renders a meeting two hours before the one that
/// exists.
///
/// Nothing arithmetic is wrong here and nothing can be: this crate is `ical-recur`'s sibling
/// and cannot take an `Occurrence`, so the argument is a wall clock and the discipline is which
/// wall clock to pass. `actual`'s own documentation now says so, `break_zones.rs` resolves
/// effective starts, and this case holds both readings side by side so the difference between
/// them is a committed fact rather than a sentence.
#[test]
fn an_occurrence_is_resolved_by_its_effective_start_and_not_by_its_cadence_key() {
    let source = hand_source();
    let document = parsed(NEW_YORK_SERIES).expect("the fixture parses");
    let component = event(&document, "daily-nine@example.test").expect("the series");
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let start = value(component, &ids::DTSTART).expect("a zoned DTSTART");
    let anchor = series.anchor(start).expect("the anchor projects");
    let rule = rule_of(component, &mut meter, &mut sink).expect("one RRULE");

    let addressed = cadence((2026, 3, 5, 9, 0)).expect("the key the override addresses");
    let landed = cadence((2026, 3, 5, 11, 0)).expect("where the organizer moved it");
    let entries = [Override::new(
        addressed,
        OverrideRange::ThisAndFuture,
        Some(landed),
        PropertyDiff::empty(),
    )];
    let overrides = OverrideSet::new(&entries, &mut meter).expect("one ascending override");
    let input = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        overrides,
        &mut meter,
    )
    .expect("the input assembles");
    let window = Window::new(
        cadence((2026, 3, 6, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 10, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let emitted = walk(input, window, &mut meter, &mut sink);
    let shown: Vec<String> = emitted
        .iter()
        .map(|(_key, start)| {
            let happened = series
                .actual(*start, &mut meter, &mut sink)
                .expect("every one of these exists");
            on_the_clock(&source, NEW_YORK, happened)
        })
        .collect();
    let wanted = vec![
        "2026-03-06T11:00".to_owned(),
        "2026-03-07T11:00".to_owned(),
        "2026-03-08T11:00".to_owned(),
        "2026-03-09T11:00".to_owned(),
    ];
    assert_eq!(
        shown, wanted,
        "the effective start is where the organizer moved the meeting to"
    );

    let by_key: Vec<String> = emitted
        .iter()
        .map(|(key, _start)| {
            let happened = series
                .actual(*key, &mut meter, &mut sink)
                .expect("every one of these exists");
            on_the_clock(&source, NEW_YORK, happened)
        })
        .collect();
    assert!(
        by_key.iter().all(|clock| clock.ends_with("T09:00")),
        "and the cadence key is where the base rule put it, which is the other question: {by_key:?}"
    );
}

/// RFC 5545 section 3.3.10. An hourly series across the hour that happens twice, on both of the
/// two readings that ship.
///
/// November 1st 2026 is twenty-five hours long in New York. An hourly series anchored on the
/// series' own wall clock — the seam's contract for a civil cadence — has twenty-four
/// occurrences that day, because the wall clock reads twenty-four hours and the fold policy
/// takes one side of the repeated hour. Anchored on the real timeline it has twenty-five, an
/// hour apart throughout. Google's engine gives 25 and libical's local-time expansion gives 24;
/// both ship, and neither is a defect anyone can point at.
///
/// So the seam states the choice rather than making it: `ZonedSeries::anchor` is the civil
/// reading and `ZonedSeries::real_anchor` is the absolute one, `SECONDLY`, `MINUTELY` and
/// `HOURLY` are the frequencies that mean the second, and a series anchored on the real
/// timeline is walked there and needs no per-occurrence resolution because its keys are already
/// the instants. Both readings are asserted here so that a change to either is a diff.
#[test]
fn an_hourly_series_across_the_fold_fills_the_day_on_the_timeline_it_was_anchored_on() {
    let source = hand_source();
    let (mut meter, mut sink) = ledger();
    let series = ZonedSeries::new(&source, NEW_YORK, ResolutionPolicy::DEFAULT);
    let midnight = local((2026, 11, 1, 0, 0)).expect("midnight the day the clocks went back");
    let rule = RecurrenceRuleBuilder::new(Freq::Hourly)
        .build()
        .expect("an hourly rule");

    let real = series
        .real_anchor(DateTimeValue::Local(midnight))
        .expect("midnight in New York names one instant");
    assert_eq!(
        real,
        utc((2026, 11, 1, 4, 0)).expect("a real instant"),
        "midnight EDT is 04:00Z"
    );
    let absolute = plain_input(real, &rule, &mut meter).expect("the input assembles");
    let day = Window::new(
        real,
        utc((2026, 11, 2, 5, 0)).expect("midnight EST the next day"),
    )
    .expect("a window is not empty");
    let happened: Vec<Instant> = walk(absolute, day, &mut meter, &mut sink)
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        happened.len(),
        25,
        "the day really is twenty-five hours long and an absolute cadence fills it"
    );
    for pair in happened.windows(2) {
        assert_eq!(
            pair[0].checked_seconds_until(pair[1]),
            Some(3600),
            "an hourly series skipped an hour at {}",
            render(pair[0])
        );
    }

    let nominal_anchor = cadence((2026, 11, 1, 0, 0)).expect("the same midnight as a key");
    let civil = plain_input(nominal_anchor, &rule, &mut meter).expect("the input assembles");
    let window = Window::new(
        nominal_anchor,
        cadence((2026, 11, 2, 0, 0)).expect("a real wall clock"),
    )
    .expect("a window is not empty");
    let on_the_wall: Vec<Instant> = walk(civil, window, &mut meter, &mut sink)
        .into_iter()
        .filter_map(|(key, _)| series.actual(key, &mut meter, &mut sink))
        .collect();
    assert_eq!(
        on_the_wall.len(),
        24,
        "the civil reading fills the clock instead, which is twenty-four hours long"
    );
}
