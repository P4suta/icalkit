// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ical-tz`'s transition arithmetic, attacked with real zones and real transition history.
//!
//! Every expectation below is transcribed from the published rules of the zone it names — the
//! tz database's own statement of when that zone moved its clocks — and not read off an answer
//! this workspace gave. A case that fails means the crate disagrees with the zone.
//!
//! Each case is a wall clock and a rendering of what that wall clock names, so that a
//! disagreement prints as two lines a person can read rather than as two debug dumps, and so
//! that no helper on the path to an assertion can panic on a value it could not build.
//!
//! # What each case is addressed to
//!
//! - **RFC 5545 section 3.6.5** (`VTIMEZONE`, `STANDARD`, `DAYLIGHT`) — a definition carrying
//!   an `RRULE` and `RDATE` lines at once, which section 3.8.5.2 and section 3.8.5.3 both
//!   permit and `crates/ical-tz/src/reader.rs` calls "ordinary rather than a conflict"; and two
//!   observances declared to begin on the same wall clock.
//! - **RFC 5545 section 3.3.5** (`DATE-TIME` form 3) — the hour that repeats and the hour that
//!   does not exist, at a fifteen-minute step (`Asia/Kathmandu`, 1986), a thirty-minute one
//!   (`Australia/Lord_Howe`), a forty-four-minute-and-thirty-second one (`Africa/Monrovia`,
//!   1972) and a whole day (`Pacific/Apia`, 2011).
//! - **RFC 5545 section 3.3.14** (`UTC-OFFSET`) — an offset that is not a whole hour, and one
//!   whose seconds field is not zero.
//! - **`docs/adr/0003`** — a `VTIMEZONE` written before a government changed the rules, read
//!   against a database written after it; and a table that ran out of data without saying so.
//!
//! # The arithmetic every table below was written from
//!
//! `Europe/Berlin` runs CET (`+01:00`) and CEST (`+02:00`) and moves on the last Sunday of
//! March and the last Sunday of October, both at 01:00 UTC — 02:00 CET going forward, 03:00
//! CEST coming back. July is CEST in every year the European Union has had that rule.
//!
//! `Australia/Sydney` runs AEST (`+10:00`) and AEDT (`+11:00`). Through 2007 daylight time ran
//! from the last Sunday of October to the last Sunday of March; from 2008 it runs from the
//! first Sunday of October to the first Sunday of April. It therefore spans the new year, and
//! the summer of 2007-08 began under one rule and ended under the other.
//!
//! `Asia/Kathmandu` moved from `+05:30` to `+05:45` at midnight opening 1986-01-01, so the
//! first fifteen minutes of that day never happened. `Africa/Monrovia` moved from `-00:44:30`
//! to `+00:00` at midnight opening 1972-01-07, so the first forty-four minutes and thirty
//! seconds of that day never happened. `Pacific/Apia` moved from `-10:00` to `+14:00` at
//! midnight opening 2011-12-30, so the whole of that day never happened.
//!
//! `America/New_York` moved its rules in 2007: daylight time had begun on the first Sunday of
//! April and from 2007 begins on the second Sunday of March. A file written in 2006 and a
//! database written today therefore disagree about the whole of March 2007, which is a fact
//! `docs/adr/0003` requires to be reported rather than settled.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, Diagnostic, DiagnosticCode, Document, Instant,
    Limits, Meter, UtcOffset, Weekday,
};
use ical_tz::{
    AnswerBasis, CombinedZoneSource, LocalResolution, OffsetAnswer, Reading, TransitionTable,
    VtimezoneSet, ZoneAnswer, ZoneProvenance, ZoneSource, read_calendar_zones,
};

/// A definition carrying the last Sunday rules and two years of `RDATE` lines beside them.
const BERLIN_RULE_AND_DATES: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/berlin_rule_and_restated_dates.ics");

/// The same definition with one year of `RDATE` lines instead of two.
const BERLIN_ONE_YEAR: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/berlin_rule_and_one_year_of_dates.ics");

/// The same definition with the rules alone, which is the control.
const BERLIN_RULE_ONLY: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/berlin_rule_only.ics");

/// A rule that runs on for ever beside a list of dates that stops in 2029.
const BERLIN_DATES_RUN_OUT: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/berlin_standard_dates_run_out.ics");

/// `Asia/Kathmandu`'s fifteen minutes.
const KATHMANDU: &[u8] = include_bytes!("fixtures/break_tz_transitions/kathmandu_quarter_hour.ics");

/// `Africa/Monrovia`'s offset with a seconds field.
const MONROVIA: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/monrovia_offset_with_seconds.ics");

/// `Pacific/Apia`'s missing day.
const APIA: &[u8] = include_bytes!("fixtures/break_tz_transitions/apia_skipped_a_day.ics");

/// `Australia/Sydney` across the 2008 rule change and the new year.
const SYDNEY: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/sydney_dst_across_new_year.ics");

/// `Australia/Lord_Howe`, whose step is thirty minutes.
const LORD_HOWE: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/lord_howe_half_hour_edges.ics");

/// `America/New_York` as a file written in 2006 would carry it.
const NEW_YORK_2006: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/new_york_written_in_2006.ics");

/// `Europe/Moscow` as a file written in 2010 would carry it.
const MOSCOW: &[u8] = include_bytes!("fixtures/break_tz_transitions/moscow_dst_abolished.ics");

/// Two observances declared to begin on the same wall clock, `DAYLIGHT` written first.
const OVERLAPPING: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/overlapping_observances.ics");

/// The same two observances with `STANDARD` written first and nothing else changed.
const OVERLAPPING_REORDERED: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/overlapping_observances_reordered.ics");

/// An observance whose `TZOFFSETFROM` is not the `TZOFFSETTO` of the one before it.
const MISMATCHED_OFFSET_FROM: &[u8] =
    include_bytes!("fixtures/break_tz_transitions/offset_from_mismatch.ics");

/// Two observances whose order by wall clock is not their order on the timeline.
const INVERTED_ONSETS: &[u8] = include_bytes!("fixtures/break_tz_transitions/inverted_onsets.ics");

/// A wall clock as the tables below write one: year, month, day, hour, minute, second.
type Stamp = (u16, u8, u8, u8, u8, u8);

/// One case: a wall clock, and what the zone's published rules say it names.
type Case = (Stamp, &'static str);

/// The `VCALENDAR` a fixture's document holds.
fn calendar(document: &Document) -> Option<&Component> {
    document
        .components()
        .find(|component| component.is_named(b"VCALENDAR"))
}

/// The zone definitions a fixture carries.
fn zones(octets: &[u8]) -> Option<VtimezoneSet> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(octets, Limits::DEFAULT, &mut sink).ok()?;
    Some(read_calendar_zones(
        calendar(&document)?,
        &mut meter,
        &mut sink,
    ))
}

/// The one table a fixture defines.
fn table_of(octets: &[u8], tzid: &str) -> Option<TransitionTable> {
    zones(octets)?.table(tzid).cloned()
}

/// A wall clock.
fn local(stamp: Stamp) -> Option<CivilDateTime> {
    Some(CivilDateTime::new(
        CivilDate::from_ymd(stamp.0, stamp.1, stamp.2)?,
        CivilTime::from_hms(stamp.3, stamp.4, stamp.5)?,
    ))
}

/// An instant, named by its own UTC wall clock, which is how every expectation is written.
fn utc(stamp: Stamp) -> Option<Instant> {
    local(stamp)?.at_offset(UtcOffset::UTC)
}

/// An instant as its UTC wall clock, which is the form every expectation below is written in.
fn show(instant: Instant) -> String {
    let Some(civil) = CivilDateTime::from_instant(instant, UtcOffset::UTC) else {
        return "unrepresentable".to_owned();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        civil.date().year(),
        civil.date().month(),
        civil.date().day(),
        civil.time().hour(),
        civil.time().minute(),
        civil.time().second()
    )
}

/// One reading, as an instant, an offset in seconds and the observance's own classification.
fn reading(found: Reading) -> String {
    let kind = if found.daylight {
        "daylight"
    } else {
        "standard"
    };
    format!("{} {} {kind}", show(found.instant), found.offset.seconds())
}

/// What a resolution says, in the terms a zone's published rules state it in.
fn render(resolution: LocalResolution) -> String {
    match resolution {
        LocalResolution::Unique { reading: only } => format!("unique {}", reading(only)),
        LocalResolution::Ambiguous { earlier, later } => {
            format!("twice {} then {}", reading(earlier), reading(later))
        },
        LocalResolution::Nonexistent {
            gap_end,
            offset_before,
            offset_after,
            shifted,
            ..
        } => format!(
            "never; closes {} from {} to {}; shifted {}",
            show(gap_end),
            offset_before.seconds(),
            offset_after.seconds(),
            show(shifted)
        ),
        // `LocalResolution` is `#[non_exhaustive]`, and a state this corpus does not know about
        // is a disagreement rather than a pass.
        _ => "a state this corpus does not know".to_owned(),
    }
}

/// What a zone said about one wall clock.
fn rendered(table: &TransitionTable, tzid: &str, asked: Stamp) -> String {
    let Some(wall) = local(asked) else {
        return "a wall clock the calendar cannot write".to_owned();
    };
    let Some(answer) = table.resolve(tzid, wall) else {
        return "no answer at all".to_owned();
    };
    render(answer.resolution)
}

/// Every case in `cases` that answered something other than what its zone's rules say.
///
/// Collected rather than asserted one at a time, so one run names every disagreement instead
/// of stopping at the first and hiding the shape of the defect behind it.
fn disagreements(table: &TransitionTable, tzid: &str, cases: &[Case]) -> Vec<String> {
    let mut found = Vec::new();
    for (asked, expected) in cases {
        let said = rendered(table, tzid, *asked);
        if said != *expected {
            found.push(format!(
                "{tzid} at {asked:?}\n  said {said}\n  rules {expected}"
            ));
        }
    }
    found
}

// ---------------------------------------------------------------------------------------
// The breaks.
// ---------------------------------------------------------------------------------------

/// `Europe/Berlin` in July, under three writings of one definition that differ only in how many
/// transitions the file restates as `RDATE` lines beside the rule that already states them.
const BERLIN_JULY: &[Case] = &[(
    (2026, 7, 1, 12, 0, 0),
    "unique 2026-07-01T10:00:00Z 7200 daylight",
)];

/// `Europe/Berlin` is CEST in July. This definition says so twice — once as
/// `RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU` and once as `RDATE` lines restating the same
/// transitions for 2020 and 2021 — and the crate answers CET.
///
/// RFC 5545 section 3.6.5 admits both forms in one observance and
/// `crates/ical-tz/src/reader.rs` calls the pair "ordinary rather than a conflict". What the
/// resolver does with four dated observances standing between a rule and the question is stop
/// consulting the rule: `resolve.rs`'s `RULE_WINDOW` asks only the last four observances
/// admitted at or before the query whether a rule of theirs fired later, and four `RDATE`
/// lines — two years of a zone that moves twice a year — fill that window completely.
///
/// Nothing marks the answer. `coverage_end()` is `None`, because a rule with no `UNTIL` is
/// present, so the basis is `Computed` and no diagnostic is emitted: an hour wrong, stated with
/// the confidence of a zone that knows the future.
#[test]
fn a_rule_stops_being_consulted_once_four_dated_transitions_stand_between_it_and_the_query() {
    let table = table_of(BERLIN_RULE_AND_DATES, "Europe/Berlin").expect("the fixture defines it");
    let found = disagreements(&table, "Europe/Berlin", BERLIN_JULY);
    assert!(
        found.is_empty(),
        "the last Sunday of March 2026 was the 29th and the rule saying so is in this table:\n{}",
        found.join("\n")
    );
}

/// The same file with one year of dates instead of two answers correctly, which is what makes
/// the case above a defect in the resolver rather than a disagreement about the fixture.
///
/// Four observances fit in the window; six do not. The zone, the rule, the query and every
/// offset are identical across the three files.
#[test]
fn the_same_zone_answers_differently_according_to_how_many_dates_were_restated() {
    let mut found = Vec::new();
    for (octets, restated) in [
        (BERLIN_RULE_ONLY, "no restated dates"),
        (BERLIN_ONE_YEAR, "one year of restated dates"),
        (BERLIN_RULE_AND_DATES, "two years of restated dates"),
    ] {
        let table = table_of(octets, "Europe/Berlin").expect("the fixture defines it");
        for line in disagreements(&table, "Europe/Berlin", BERLIN_JULY) {
            found.push(format!("with {restated}: {line}"));
        }
    }
    assert!(
        found.is_empty(),
        "one zone, one rule, one query, three writings of the file:\n{}",
        found.join("\n")
    );
}

/// A definition whose daylight rule runs on for ever and whose standard-time transitions are
/// three `RDATE` lines ending in 2029 has no data about when summer time ends in 2030. It says
/// it has.
///
/// `TransitionTable::coverage_end` answers `None` as soon as any one observance repeats by a
/// rule with no `UNTIL`, so every answer past 2029 carries `AnswerBasis::Computed` and no
/// `time-zone-coverage-exhausted` is ever available to emit. The answer itself — permanent
/// summer time from March 2030 onwards — is exactly the reading `docs/adr/0003` requires to be
/// marked as a continuation past the end of what the source knows rather than presented as
/// computed.
#[test]
fn a_table_that_ran_out_on_one_side_still_reports_that_it_knows_the_future() {
    let table = table_of(BERLIN_DATES_RUN_OUT, "Europe/Berlin").expect("the fixture defines it");
    let midwinter = local((2031, 1, 15, 12, 0, 0)).expect("a wall clock the calendar has");
    let answer = table
        .resolve("Europe/Berlin", midwinter)
        .expect("the table answers to its own identifier");
    assert_eq!(
        answer.resolution.unambiguous().map(show),
        utc((2031, 1, 15, 10, 0, 0)).map(show),
        "the table answers +02:00 in January, because summer time never ended in 2030"
    );
    assert_eq!(
        answer.basis,
        AnswerBasis::BeyondKnownTransitions(
            CivilDate::from_ymd(2029, 10, 28).expect("a date the calendar has")
        ),
        "January 2031 is answered past the last date half of this definition has, which is what \
         AnswerBasis exists to say"
    );
    assert_eq!(
        table.coverage_end(),
        CivilDate::from_ymd(2029, 10, 28),
        "the last transition this definition has real data for is 2029-10-28"
    );
}

/// The two wall clocks the overlapping definition is asked about.
const OVERLAP_ASKED: &[Stamp] = &[(2026, 3, 29, 2, 30, 0), (2026, 3, 29, 0, 30, 0)];

/// Two observances declared to begin on the same wall clock resolve differently according to
/// which of them the producer wrote first.
///
/// Both files declare exactly the same pair: a `DAYLIGHT` observance beginning 02:00 read
/// against `+01:00`, and a `STANDARD` one beginning 02:00 read against `+02:00`. They differ
/// only in the order the two subcomponents appear. `TransitionTable::new` sorts by wall clock
/// with `sort_unstable_by_key`, which orders equal keys arbitrarily, and `era_at` reads the era
/// before the first onset off `observances().first()`. So 02:30 that morning names no instant
/// under one writing and 2026-03-29T00:30:00Z under the other, and 00:30 names two instants an
/// hour apart. Neither reading is reported.
#[test]
fn two_observances_beginning_on_one_wall_clock_answer_by_the_order_they_were_written() {
    let tzid = "Example/Overlapping";
    let written = table_of(OVERLAPPING, tzid).expect("the fixture defines it");
    let reordered = table_of(OVERLAPPING_REORDERED, tzid).expect("the fixture defines it");
    assert_eq!(
        written.observances().len(),
        reordered.observances().len(),
        "the two files declare the same two observances"
    );

    let mut found = Vec::new();
    for asked in OVERLAP_ASKED {
        let one = rendered(&written, tzid, *asked);
        let two = rendered(&reordered, tzid, *asked);
        if one != two {
            found.push(format!(
                "{asked:?}\n  DAYLIGHT written first {one}\n  STANDARD written first {two}"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "one definition written two ways names two different instants, and says so nowhere:\n{}",
        found.join("\n")
    );
}

// ---------------------------------------------------------------------------------------
// The attacks that held.
// ---------------------------------------------------------------------------------------

/// `Asia/Kathmandu` went from `+05:30` to `+05:45` at 1986-01-01T00:00 read against `+05:30`,
/// which is 1985-12-31T18:30:00Z, so 00:00 through 00:14:59 that morning name no instant.
const KATHMANDU_CASES: &[Case] = &[
    (
        (1985, 12, 31, 23, 50, 0),
        "unique 1985-12-31T18:20:00Z 19800 standard",
    ),
    (
        (1986, 1, 1, 0, 7, 0),
        "never; closes 1985-12-31T18:30:00Z from 19800 to 20700; shifted 1985-12-31T18:37:00Z",
    ),
    (
        (1986, 1, 1, 0, 20, 0),
        "unique 1985-12-31T18:35:00Z 20700 standard",
    ),
];

/// `Africa/Monrovia` went from `-00:44:30` to `+00:00` at 1972-01-07T00:00 read against
/// `-00:44:30`, which is 1972-01-07T00:44:30Z, so the first forty-four and a half minutes of
/// that day name none either. RFC 5545 section 3.3.14 writes that offset `-004430`.
const MONROVIA_CASES: &[Case] = &[
    (
        (1972, 1, 6, 12, 0, 0),
        "unique 1972-01-06T12:44:30Z -2670 standard",
    ),
    (
        (1972, 1, 7, 0, 30, 0),
        "never; closes 1972-01-07T00:44:30Z from -2670 to 0; shifted 1972-01-07T01:14:30Z",
    ),
    (
        (1972, 1, 7, 1, 0, 0),
        "unique 1972-01-07T01:00:00Z 0 standard",
    ),
];

/// A step that is not an hour: fifteen minutes in Nepal, forty-four and a half in Liberia.
#[test]
fn a_step_that_is_not_a_whole_hour_lands_where_the_zone_put_it() {
    let mut found = Vec::new();
    let kathmandu = table_of(KATHMANDU, "Asia/Kathmandu").expect("the fixture defines it");
    found.extend(disagreements(&kathmandu, "Asia/Kathmandu", KATHMANDU_CASES));
    let monrovia = table_of(MONROVIA, "Africa/Monrovia").expect("the fixture defines it");
    found.extend(disagreements(&monrovia, "Africa/Monrovia", MONROVIA_CASES));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// `Australia/Lord_Howe` moves by thirty minutes, so its gap is half an hour wide and its fold
/// half an hour long, and both edges of each are a wall clock somebody can write down.
///
/// Daylight time begins at 02:00 on the first Sunday of October, the 4th in 2026: the clock
/// reads 01:59:59 and then 02:30:00, so 02:00 itself names no instant and 02:30 names
/// 2026-10-03T15:30:00Z. It ends at 02:00 on the first Sunday of April, the 5th in 2026, so
/// 01:30 through 01:59:59 happen twice and 02:00 happens once.
const LORD_HOWE_CASES: &[Case] = &[
    (
        (2026, 10, 4, 2, 0, 0),
        "never; closes 2026-10-03T15:30:00Z from 37800 to 39600; shifted 2026-10-03T15:30:00Z",
    ),
    (
        (2026, 10, 4, 2, 15, 0),
        "never; closes 2026-10-03T15:30:00Z from 37800 to 39600; shifted 2026-10-03T15:45:00Z",
    ),
    (
        (2026, 10, 4, 2, 30, 0),
        "unique 2026-10-03T15:30:00Z 39600 daylight",
    ),
    (
        (2026, 4, 5, 1, 30, 0),
        "twice 2026-04-04T14:30:00Z 39600 daylight then 2026-04-04T15:00:00Z 37800 standard",
    ),
    (
        (2026, 4, 5, 2, 0, 0),
        "unique 2026-04-04T15:30:00Z 37800 standard",
    ),
];

/// A thirty-minute step, at both edges of the gap and both edges of the fold.
#[test]
fn a_thirty_minute_step_is_missing_and_repeated_at_exactly_the_right_edges() {
    let table = table_of(LORD_HOWE, "Australia/Lord_Howe").expect("the fixture defines it");
    let found = disagreements(&table, "Australia/Lord_Howe", LORD_HOWE_CASES);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// `Pacific/Apia` crossed the date line at the end of 2011: the clock read 2011-12-29T23:59:59
/// at `-10:00` and then 2011-12-31T00:00:00 at `+14:00`, so the whole of 2011-12-30 names no
/// instant at all. Read with the offset in force before the jump, each hour of the missing day
/// names an instant ten hours later on the UTC clock.
const APIA_CASES: &[Case] = &[
    (
        (2011, 12, 29, 23, 0, 0),
        "unique 2011-12-30T09:00:00Z -36000 daylight",
    ),
    (
        (2011, 12, 30, 0, 0, 0),
        "never; closes 2011-12-30T10:00:00Z from -36000 to 50400; shifted 2011-12-30T10:00:00Z",
    ),
    (
        (2011, 12, 30, 12, 0, 0),
        "never; closes 2011-12-30T10:00:00Z from -36000 to 50400; shifted 2011-12-30T22:00:00Z",
    ),
    (
        (2011, 12, 30, 23, 0, 0),
        "never; closes 2011-12-30T10:00:00Z from -36000 to 50400; shifted 2011-12-31T09:00:00Z",
    ),
    (
        (2011, 12, 31, 0, 0, 0),
        "unique 2011-12-30T10:00:00Z 50400 daylight",
    ),
];

/// A whole day is the widest gap in the tz database's history, and it is the case a resolver
/// that looks one day either side of the wall clock it was asked about has to survive.
#[test]
fn the_day_samoa_did_not_have_names_no_instant_at_any_hour_of_it() {
    let table = table_of(APIA, "Pacific/Apia").expect("the fixture defines it");
    let found = disagreements(&table, "Pacific/Apia", APIA_CASES);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// `Australia/Sydney`'s daylight time spans the new year, so January is `+11:00` in every year.
///
/// The summer of 2007-08 began under the rule that ended daylight time on the last Sunday of
/// March and finished under the one that ends it on the first Sunday of April, so 2008-03-30 is
/// still `+11:00`: the rule that would have ended it that morning had already expired.
const SYDNEY_CASES: &[Case] = &[
    (
        (2026, 1, 15, 12, 0, 0),
        "unique 2026-01-15T01:00:00Z 39600 daylight",
    ),
    (
        (2026, 7, 15, 12, 0, 0),
        "unique 2026-07-15T02:00:00Z 36000 standard",
    ),
    (
        (2008, 1, 15, 12, 0, 0),
        "unique 2008-01-15T01:00:00Z 39600 daylight",
    ),
    (
        (2008, 3, 30, 12, 0, 0),
        "unique 2008-03-30T01:00:00Z 39600 daylight",
    ),
    (
        (2026, 10, 4, 2, 30, 0),
        "never; closes 2026-10-03T16:00:00Z from 36000 to 39600; shifted 2026-10-03T16:30:00Z",
    ),
    (
        (2026, 4, 5, 2, 30, 0),
        "twice 2026-04-04T15:30:00Z 39600 daylight then 2026-04-04T16:30:00Z 36000 standard",
    ),
    (
        (2007, 10, 28, 2, 30, 0),
        "never; closes 2007-10-27T16:00:00Z from 36000 to 39600; shifted 2007-10-27T16:30:00Z",
    ),
];

/// Daylight saving that begins in one year and ends in the next, across a rule change.
#[test]
fn daylight_saving_that_spans_the_new_year_is_still_in_force_in_january() {
    let table = table_of(SYDNEY, "Australia/Sydney").expect("the fixture defines it");
    let found = disagreements(&table, "Australia/Sydney", SYDNEY_CASES);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The European Union states its transitions in UTC, at 01:00Z, and every observance in a
/// `VTIMEZONE` states its own in wall-clock terms. The two have to agree in every year the rule
/// covers, or a zoned series is an hour out for half of one of them.
///
/// Walked rather than sampled: for forty-one years, the offset either side of 01:00Z on every
/// day of March and October, checking that the days the offset changes are the two last Sundays
/// of that year and no others.
#[test]
fn a_rule_stated_in_utc_moves_the_clock_on_the_last_sunday_in_every_year_it_covers() {
    let table = table_of(BERLIN_RULE_ONLY, "Europe/Berlin").expect("the fixture defines it");
    let mut moved: Vec<(u16, u8, u8)> = Vec::new();
    for year in 2000_u16..=2040 {
        for month in [3_u8, 10] {
            for day in 1_u8..=31 {
                let before = utc((year, month, day, 0, 59, 0)).expect("an instant");
                let after = utc((year, month, day, 1, 0, 0)).expect("an instant");
                let one = table.offset_at("Europe/Berlin", before).expect("an offset");
                let two = table.offset_at("Europe/Berlin", after).expect("an offset");
                if one.offset != two.offset {
                    moved.push((year, month, day));
                }
            }
        }
    }
    assert_eq!(
        moved.len(),
        82,
        "two transitions in each of forty-one years"
    );
    for (year, month, day) in moved {
        let date = CivilDate::from_ymd(year, month, day).expect("a date the calendar has");
        assert_eq!(
            date.weekday(),
            Some(Weekday::Sunday),
            "{year}-{month}-{day} is not a Sunday"
        );
        assert!(
            day.saturating_add(7) > 31,
            "{year}-{month}-{day} is not the last Sunday of its month"
        );
    }
}

/// Every quarter hour of a year, put through the zone and back: the offset in force gives a
/// wall clock, and that wall clock has to name the instant it came from.
///
/// A resolver that answers the fold or the gap by one rule and the ordinary hours by another
/// fails here at the transition and nowhere else, which is why the walk covers three zones
/// whose steps are an hour, an hour and half an hour.
#[test]
fn every_quarter_hour_of_a_year_resolves_back_to_the_instant_it_came_from() {
    for (octets, tzid) in [
        (BERLIN_RULE_ONLY, "Europe/Berlin"),
        (SYDNEY, "Australia/Sydney"),
        (LORD_HOWE, "Australia/Lord_Howe"),
    ] {
        let table = table_of(octets, tzid).expect("the fixture defines it");
        let start = utc((2026, 1, 1, 0, 0, 0)).expect("an instant");
        for step in 0_i64..(365 * 24 * 4) {
            let instant = start
                .checked_add_seconds(step.saturating_mul(900))
                .expect("an instant the timeline has");
            let answer = table.offset_at(tzid, instant).expect("an offset");
            let wall = CivilDateTime::from_instant(instant, answer.offset).expect("a wall clock");
            let named = match table.resolve(tzid, wall).expect("an answer").resolution {
                LocalResolution::Unique { reading: only } => only.instant == instant,
                LocalResolution::Ambiguous { earlier, later } => {
                    earlier.instant == instant || later.instant == instant
                },
                _ => false,
            };
            assert!(
                named,
                "{tzid}: {} did not resolve back to itself",
                show(instant)
            );
        }
    }
}

/// A zone database the caller wired in by hand, which is the whole of what `docs/adr/0003`
/// requires of a source: an identifier it answers to, a list of transition instants, and the
/// offset each one begins.
///
/// Written out rather than reached for, so that the disagreement cases below put two
/// independently written sources against each other, which is the arrangement the ADR is about.
#[derive(Debug)]
struct CallerTzdb {
    /// The identifier this source answers to, compared by exact bytes.
    tzid: &'static str,
    /// Ascending: the instant an offset takes effect, its seconds east of UTC, and whether it
    /// is the zone's daylight one.
    steps: Vec<(Instant, i32, bool)>,
    /// What ran before the first step.
    base: (i32, bool),
}

impl CallerTzdb {
    /// The offset in force at `instant`, and whether it is the daylight one.
    fn era(&self, instant: Instant) -> Option<(UtcOffset, bool)> {
        let mut found = self.base;
        for (at, seconds, daylight) in &self.steps {
            if *at <= instant {
                found = (*seconds, *daylight);
            }
        }
        Some((UtcOffset::from_seconds(found.0)?, found.1))
    }

    /// The step `wall` fell inside, when no candidate offset governed its own reading.
    fn gap(&self, wall: CivilDateTime) -> Option<LocalResolution> {
        for (at, seconds, _) in &self.steps {
            let gap_start = at.checked_add_seconds(-1)?;
            let (before, _) = self.era(gap_start)?;
            let after = UtcOffset::from_seconds(*seconds)?;
            let opened = CivilDateTime::from_instant(*at, before)?;
            let closed = CivilDateTime::from_instant(*at, after)?;
            if opened <= wall && wall < closed {
                return Some(LocalResolution::Nonexistent {
                    gap_start,
                    gap_end: *at,
                    offset_before: before,
                    offset_after: after,
                    shifted: wall.at_offset(before)?,
                });
            }
        }
        None
    }

    /// Every offset this source ever ran at, ascending and without repeats.
    fn offsets(&self) -> Vec<i32> {
        let mut all = vec![self.base.0];
        for (_, seconds, _) in &self.steps {
            all.push(*seconds);
        }
        all.sort_unstable();
        all.dedup();
        all
    }

    /// Every reading of `wall` that the offset producing it actually governs.
    fn readings(&self, wall: CivilDateTime) -> Option<Vec<Reading>> {
        let mut found = Vec::new();
        for seconds in self.offsets() {
            let candidate = UtcOffset::from_seconds(seconds)?;
            let instant = wall.at_offset(candidate)?;
            let (in_force, daylight) = self.era(instant)?;
            if in_force == candidate {
                found.push(Reading::new(instant, candidate, daylight));
            }
        }
        found.sort_unstable();
        Some(found)
    }
}

impl ZoneSource for CallerTzdb {
    fn resolve(&self, tzid: &str, wall: CivilDateTime) -> Option<ZoneAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let resolution = match self.readings(wall)?.as_slice() {
            [only] => LocalResolution::Unique { reading: *only },
            [earlier, later] => LocalResolution::Ambiguous {
                earlier: *earlier,
                later: *later,
            },
            _ => self.gap(wall)?,
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let (found, daylight) = self.era(instant)?;
        Some(OffsetAnswer::new(
            found,
            daylight,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }
}

/// A `VTIMEZONE` written in 2006 and a database written today disagree about the whole of
/// March 2007, and `docs/adr/0003` requires that to be reported rather than settled.
///
/// The file says daylight time begins on the first Sunday of April. The Energy Policy Act of
/// 2005 moved it to the second Sunday of March with effect from 2007. So 2007-03-15T12:00 is
/// 17:00Z under the file and 16:00Z under the database, and 2007-04-10T12:00 is 16:00Z under
/// both.
#[test]
fn a_file_written_before_a_rule_change_disagrees_with_a_database_written_after_it() {
    let table = table_of(NEW_YORK_2006, "America/New_York").expect("the fixture defines it");
    let modern = CallerTzdb {
        tzid: "America/New_York",
        steps: vec![
            (
                utc((2006, 10, 29, 6, 0, 0)).expect("an instant"),
                -18_000,
                false,
            ),
            (
                utc((2007, 3, 11, 7, 0, 0)).expect("an instant"),
                -14_400,
                true,
            ),
            (
                utc((2007, 11, 4, 6, 0, 0)).expect("an instant"),
                -18_000,
                false,
            ),
        ],
        base: (-14_400, true),
    };
    let combined = CombinedZoneSource::new(&table, &modern);

    let asked = utc((2007, 3, 15, 17, 0, 0)).expect("an instant");
    let in_march = combined.offset_at("America/New_York", asked);
    assert!(
        in_march.is_disagreement(),
        "March 2007 is the month the two sources cannot both be right about"
    );
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    combined.report(in_march, asked, &mut meter, &mut sink);
    assert_eq!(
        sink.first().map(|entry| entry.code()),
        Some(DiagnosticCode::TimeZoneSourceDisagreement),
        "a disagreement about a zone is a reported fact"
    );

    let in_april = utc((2007, 4, 10, 16, 0, 0)).expect("an instant");
    assert!(
        !combined
            .offset_at("America/New_York", in_april)
            .is_disagreement(),
        "April 2007 is a month both rules put daylight time in"
    );
}

/// A country that stopped moving its clocks after the file was written.
///
/// Russia abolished the twice-yearly change in 2011 and settled on `+03:00` in 2014. A file
/// written in 2010 still says `Europe/Moscow` is `+04:00` every summer, and the two readings of
/// 2026-07-01T12:00 are an hour apart. Both survive into the outcome.
#[test]
fn a_zone_that_abolished_daylight_saving_disagrees_with_the_file_that_predates_it() {
    let table = table_of(MOSCOW, "Europe/Moscow").expect("the fixture defines it");
    let permanent = CallerTzdb {
        tzid: "Europe/Moscow",
        steps: vec![(
            utc((2014, 10, 25, 22, 0, 0)).expect("an instant"),
            10_800,
            false,
        )],
        base: (14_400, false),
    };
    let combined = CombinedZoneSource::new(&table, &permanent);
    let asked = local((2026, 7, 1, 12, 0, 0)).expect("a wall clock");
    let outcome = combined.resolve("Europe/Moscow", asked);
    assert!(
        outcome.is_disagreement(),
        "a file that predates the abolition and a database that postdates it disagree"
    );
    let embedded = outcome.embedded_first().expect("both sources answered");
    assert_eq!(
        embedded.resolution.unambiguous().map(show),
        utc((2026, 7, 1, 8, 0, 0)).map(show),
        "the embedded definition still says +04:00 in July"
    );
}

/// The file declares CEST as `+02:00` from the last Sunday of March, and then that standard
/// time resumes when a clock running at `+03:00` reads 03:00 on 2026-10-25 — an offset no
/// observance in this file ever put the zone on.
///
/// RFC 5545 section 3.6.5 reads an observance's `DTSTART` against its own `TZOFFSETFROM`, so
/// the transition lands at 2026-10-25T00:00:00Z, an hour before the preceding observance
/// implies, and the hour that repeats on the wall clock is 01:00 to 02:00 rather than 02:00 to
/// 03:00. Every value below is consistent with that reading, which is the right one to give:
/// the fold's width comes from the offsets actually in force either side of the transition and
/// not from the `TZOFFSETFROM` that names neither.
const MISMATCHED_CASES: &[Case] = &[
    (
        (2026, 10, 25, 1, 30, 0),
        "twice 2026-10-24T23:30:00Z 7200 daylight then 2026-10-25T00:30:00Z 3600 standard",
    ),
    (
        (2026, 10, 25, 2, 30, 0),
        "unique 2026-10-25T01:30:00Z 3600 standard",
    ),
];

/// A `TZOFFSETFROM` naming an offset nothing else in the file ever ran at.
///
/// What no part of this workspace reports is that the file contradicted itself. The offsets are
/// both readable, so `Component::audit`'s `missing-required-property` does not fire, and
/// `docs/design/ical-tz-api.md` says this crate declines a second reading of section 3.6 —
/// which leaves the discontinuity between one observance's `TZOFFSETTO` and the next one's
/// `TZOFFSETFROM` visible to nobody. That is a gap in the diagnostic vocabulary rather than a
/// wrong answer, and it is recorded here as one.
#[test]
fn an_offset_from_that_names_no_preceding_observance_still_resolves_by_the_offsets_in_force() {
    let table = table_of(MISMATCHED_OFFSET_FROM, "Example/Mismatched").expect("the fixture has it");
    let found = disagreements(&table, "Example/Mismatched", MISMATCHED_CASES);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Two observances whose order by wall clock is not their order on the timeline, because the
/// second states a `TZOFFSETFROM` two hours east of the first's.
///
/// `DTSTART:20261025T020000` read against `+01:00` is 01:00:00Z; `DTSTART:20261025T030000` read
/// against `+03:00` is 00:00:00Z. M1 sorted the table by wall clock, so its onsets descended
/// and `resolve.rs`'s `began_by` predicate — the premise of the binary search that places a
/// query among them — was not monotone over it. This case was recorded then as a premise that
/// did not hold rather than as a defect that showed, because a table of two observances is
/// visited whole whatever the search concludes.
///
/// M2 made the order the onsets themselves, which restores the premise and changes one answer
/// here. Before the first *onset* the file states an offset and nothing else, and the first
/// onset is now the `STANDARD` one at 00:00:00Z — whose `TZOFFSETFROM` says `+03:00` was
/// running. That is what the file says about that era, and 2026-10-24T23:00:00Z is in it.
#[test]
fn a_table_whose_onsets_descend_is_still_answered_from_the_observance_in_force() {
    let table = table_of(INVERTED_ONSETS, "Example/Inverted").expect("the fixture has it");
    let expected = [
        ((2026, 10, 24, 23, 0, 0), 10_800_i32),
        ((2026, 10, 25, 0, 30, 0), 0),
        ((2026, 10, 25, 1, 30, 0), 7200),
        ((2026, 10, 25, 12, 0, 0), 7200),
    ];
    for (stamp, seconds) in expected {
        let at = utc(stamp).expect("an instant the timeline has");
        let answer = table
            .offset_at("Example/Inverted", at)
            .expect("the table answers to its own identifier");
        assert_eq!(
            answer.offset.seconds(),
            seconds,
            "at {} the latest onset this file records is the one in force",
            show(at)
        );
    }
}
