// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `VTIMEZONE` read as an attacker writes one: at the bounds, and past them.
//!
//! Every case here is a legal RFC 5545 file. Nothing below is malformed octets — the grammar
//! layer already has those (`break_grammar.rs`, `break_hostile.rs`) — because a zone definition
//! does not need to be malformed to be hostile. It needs only to be shaped unlike the four zones
//! `break_zones.rs` transcribes.
//!
//! # What each case is addressed to
//!
//! - **RFC 5545 section 3.6.5** (`VTIMEZONE`) — an observance carrying both an `RRULE` and
//!   `RDATE`s, which section 3.6.5 permits and `docs/design/ical-tz-api.md` calls "ordinary
//!   rather than a conflict"; a rule whose day form names a date in few years; two onsets inside
//!   one day; an observance whose required properties are present and unreadable.
//! - **RFC 5545 section 3.3.5** (`DATE-TIME` form 3) — the wall clocks such a file makes
//!   ambiguous, nonexistent, or neither.
//! - **RFC 5545 section 3.3.4** (`DATE`) — both ends of the four-digit years the format writes,
//!   asked of a rule with no `UNTIL`.
//! - **`docs/adr/0003`** — the invariant that `ZoneSource::resolve` answers `None` for one
//!   reason only: this source does not recognize this identifier.
//! - **`docs/adr/0010`** — a million transitions, and a ten-thousand-year sweep, each of which
//!   must terminate with a reported outcome.
//!
//! # The arithmetic every expectation below was written from
//!
//! `America/New_York` has run daylight time from the second Sunday of March to the first Sunday
//! of November since the 2005 Energy Policy Act took effect in 2007, EST being `-05:00` and EDT
//! `-04:00`. A file stating those two rules with no `UNTIL` says the zone is on EDT throughout
//! July of every year after 2007, whatever else the file also lists.
//!
//! February holds five Sundays only when it has 29 days and opens on one, which between 1990 and
//! 2060 happens in 2004 and in 2032 and in no other year. `BYDAY=5SU` under `BYMONTH=2` is
//! therefore a rule that fires twice in seventy years, and the twenty-eight years between those
//! firings are years in which the observance it repeats is nonetheless the one in force.
//!
//! A zone that moves its clock at 02:00 and again at 20:00 of one day has three offsets in that
//! day and two gaps in it, and every wall clock strictly between 03:00 and 20:00 belongs to the
//! middle one and exists exactly once.

use std::fmt::Write as _;

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, Diagnostic, DiagnosticCode, Document,
    IgnoreDiagnostics, Instant, Limits, Meter, UtcOffset,
};
use ical_tz::{
    AnswerBasis, LocalResolution, Reading, TransitionTable, ZoneAnswer, ZoneSource,
    read_calendar_zones,
};

/// The zones one calendar declares, and everything `ical-tz` said while reading it.
///
/// Answers rather than asserts, per the convention `break_zones.rs` uses: a helper below a
/// `#[test]` is production code as far as the lint profile is concerned, so what a case is
/// entitled to assume is stated at the case and not here.
fn zones_of(octets: &[u8], limits: Limits) -> Option<(Vec<Diagnostic>, Vec<TransitionTable>)> {
    let document = Document::parse(octets, limits, &mut IgnoreDiagnostics).ok()?;
    let calendar = document.components().next()?;
    let mut meter = Meter::new(limits);
    let mut reported = Vec::new();
    let set = read_calendar_zones(calendar, &mut meter, &mut reported);
    Some((reported, set.tables().to_vec()))
}

/// The one zone a fixture declares under `tzid`, and the codes reading it produced.
fn zone(octets: &[u8], tzid: &str) -> Option<(Vec<DiagnosticCode>, TransitionTable)> {
    let (reported, tables) = zones_of(octets, Limits::DEFAULT)?;
    let table = tables
        .into_iter()
        .find(|candidate| candidate.tzid().as_str() == tzid)?;
    Some((codes(&reported), table))
}

fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
    reported.iter().copied().map(Diagnostic::code).collect()
}

/// Section 3.6's own reading of the same file, over every component and not only the outermost.
fn audit_deeply(component: &Component, meter: &mut Meter, sink: &mut Vec<Diagnostic>) {
    component.audit(meter, sink);
    for inner in component.components() {
        audit_deeply(inner, meter, sink);
    }
}

/// A wall clock, absent when those fields name no date or no time.
fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<CivilDateTime> {
    Some(CivilDateTime::new(
        CivilDate::from_ymd(year, month, day)?,
        CivilTime::from_hms(hour, minute, 0)?,
    ))
}

/// An instant named by the UTC wall clock the zone's published rules put it at.
fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<Instant> {
    stamp(year, month, day, hour, minute)?.at_offset(UtcOffset::UTC)
}

/// An offset, absent when it is a day or more from UTC.
fn offset(seconds: i32) -> Option<UtcOffset> {
    UtcOffset::from_seconds(seconds)
}

/// The one reading a wall clock names, absent when it named two or none.
fn only_reading(answer: ZoneAnswer) -> Option<Reading> {
    match answer.resolution {
        LocalResolution::Unique { reading } => Some(reading),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// RFC 5545 section 3.6.5: the rules a table is asked to keep
// ---------------------------------------------------------------------------------------

/// A rule with no `UNTIL` governs every year after it, including the years an `RDATE` list
/// beside it happens to cover.
///
/// The fixture is `America/New_York` exactly as clients export it — the second Sunday of March
/// and the first Sunday of November, no `UNTIL` — with four explicit `RDATE` transitions per
/// observance restating 2008 through 2011. `docs/design/ical-tz-api.md` calls a definition
/// carrying both forms "ordinary rather than a conflict", and nothing in the file contradicts
/// anything else in it: every listed date is a date the rule itself names.
///
/// Adding those eight dates moves the two rule-bearing observances more than four positions back
/// in the sorted table, and the resolver stops consulting the rules at all. July 2026 comes back
/// as Eastern Standard Time — an hour off, `daylight` false where the zone runs daylight time,
/// and `AnswerBasis::Computed` rather than any admission that the rules were not read.
#[test]
fn rfc5545_3_6_5_a_rule_still_governs_the_years_past_an_rdate_list_beside_it() {
    let octets = include_bytes!("fixtures/break_tz_hostile/rdates_beside_a_rule.ics");
    let (reported, table) = zone(octets, "America/New_York").expect("the fixture declares it");
    assert!(reported.is_empty(), "the file is legal and reads clean");
    assert_eq!(table.observances().len(), 10);
    assert_eq!(
        table.coverage_end(),
        None,
        "two rules with no UNTIL: this zone knows the future and says so"
    );

    let asked = stamp(2026, 7, 1, 12, 0).expect("noon on the first of July 2026");
    let answer = table
        .resolve("America/New_York", asked)
        .expect("the identifier is the table's own");
    let reading = only_reading(answer).expect("an ordinary July day names one instant");
    assert_eq!(
        Some(reading.instant),
        utc(2026, 7, 1, 16, 0),
        "noon that day in New York is 16:00 UTC, because EDT is -04:00"
    );
    assert_eq!(Some(reading.offset), offset(-14_400));
    assert!(
        reading.daylight,
        "the United States runs daylight time in July"
    );
    assert_eq!(answer.basis, AnswerBasis::Computed);
}

/// A rule that names a date in few years is in force in all the years between those dates.
///
/// `FREQ=YEARLY;BYMONTH=2;BYDAY=5SU` is legal `RECUR` and `NthWeek::Fifth` is documented as
/// exactly what a producer writing it asked for. February holds five Sundays in 2004 and then
/// not again until 2032, so the observance that rule repeats begins on 2004-02-29 and is still
/// the one in force in 2010 — nothing in the file ends it, and the only other observance the
/// file declares began in 2000.
///
/// The resolver asks each rule about three years back from the query and no further, so by 2010
/// the 2004 onset is out of reach and the answer falls back to the 2000 observance: the offset
/// the file says was superseded six years earlier, reported as `Computed`.
#[test]
fn rfc5545_3_6_5_a_rule_that_fires_rarely_is_in_force_between_its_firings() {
    let octets = include_bytes!("fixtures/break_tz_hostile/a_rule_that_fires_rarely.ics");
    let (reported, table) = zone(octets, "Rare/Rule").expect("the fixture declares it");
    assert!(
        reported.is_empty(),
        "5SU is a day form this crate evaluates"
    );

    let same_year = stamp(2004, 3, 1, 12, 0).expect("the March after the rule fired");
    let onset = table
        .resolve("Rare/Rule", same_year)
        .expect("the identifier is the table's own");
    assert_eq!(
        only_reading(onset).map(|reading| reading.offset),
        offset(3600),
        "the rule names 2004-02-29, so March 2004 is already past that onset"
    );

    let years_later = stamp(2010, 7, 1, 12, 0).expect("six years later");
    let answer = table
        .resolve("Rare/Rule", years_later)
        .expect("the identifier is the table's own");
    assert_eq!(
        only_reading(answer).map(|reading| reading.offset),
        offset(3600),
        "nothing in the file ends the observance the 2004 onset began"
    );
}

// ---------------------------------------------------------------------------------------
// RFC 5545 section 3.3.5: the wall clocks a hostile table makes awkward
// ---------------------------------------------------------------------------------------

/// A wall clock between two transitions of one day exists, once.
///
/// The fixture moves the clock forward at 02:00 and again at 20:00 on 2026-03-29, chaining
/// `+01:00` to `+02:00` to `+03:00`. Both onsets are inside one day, which is the only thing
/// about this file that is unusual, and every wall clock from 03:00 to 20:00 that day is
/// governed by `+02:00` and by nothing else.
///
/// The resolver samples the zone one day either side of the query and takes the offsets it finds
/// there as the only candidates, so for a midday query it holds `+01:00` and `+03:00` and never
/// `+02:00` — neither candidate governs its own reading, and an ordinary Sunday lunchtime is
/// reported as a local time that does not exist. Under `GapPolicy::Skip` every occurrence in
/// seventeen hours of that day is dropped.
#[test]
fn rfc5545_3_3_5_a_wall_clock_between_two_transitions_of_one_day_exists() {
    let octets = include_bytes!("fixtures/break_tz_hostile/two_transitions_in_one_day.ics");
    let (reported, table) = zone(octets, "Twice/InADay").expect("the fixture declares it");
    assert!(reported.is_empty(), "the file is legal and reads clean");

    let noon = stamp(2026, 3, 29, 12, 0).expect("an ordinary Sunday lunchtime");
    let answer = table
        .resolve("Twice/InADay", noon)
        .expect("the identifier is the table's own");
    let reading = only_reading(answer).expect("noon between two transitions names one instant");
    assert_eq!(
        Some(reading.instant),
        utc(2026, 3, 29, 10, 0),
        "noon at +02:00 is 10:00 UTC, which lies between the 01:00 and 18:00 onsets"
    );
    assert_eq!(Some(reading.offset), offset(7200));
}

/// A gap is bounded by the transition that opened it and by the offsets on either side of it.
///
/// `LocalResolution::Nonexistent` documents `gap_end` as "the first instant the new offset
/// governs" and `offset_before`/`offset_after` as the offsets across the transition, and
/// `GapPolicy::ClampToTransition` is defined as landing an occurrence "as soon as it can
/// happen". For 02:30 on the fixture's morning the transition that opened the gap is the 02:00
/// one, which lands at 01:00 UTC and takes the clock to `+02:00`.
///
/// The answer instead describes the *other* transition of that day: `gap_end` is the 20:00 one,
/// seventeen hours later, and `offset_after` is `+03:00`. A caller clamping a 02:30 meeting out
/// of the gap moves it to 21:00 rather than to 03:00.
#[test]
fn rfc5545_3_3_5_a_gap_is_bounded_by_the_transition_that_opened_it() {
    let octets = include_bytes!("fixtures/break_tz_hostile/two_transitions_in_one_day.ics");
    let (_, table) = zone(octets, "Twice/InADay").expect("the fixture declares it");
    let asked = stamp(2026, 3, 29, 2, 30).expect("a wall clock the first transition sprang over");
    let answer = table
        .resolve("Twice/InADay", asked)
        .expect("the identifier is the table's own");
    let LocalResolution::Nonexistent {
        gap_end,
        offset_after,
        ..
    } = answer.resolution
    else {
        panic!(
            "02:30 that morning names no instant: {:?}",
            answer.resolution
        )
    };
    assert_eq!(
        Some(gap_end),
        utc(2026, 3, 29, 1, 0),
        "the 02:00 transition lands at 01:00 UTC, where the clock first reads 03:00"
    );
    assert_eq!(Some(offset_after), offset(7200));
}

/// The second gap of that day is read against the offset that was actually running before it.
///
/// At 20:30 the clock in force since 03:00 is `+02:00`, so RFC 5545 section 3.3.5's reading of
/// that wall time is 18:30 UTC and `offset_before` is `+02:00`. The answer reports `+01:00`,
/// the offset the zone stopped using seventeen hours earlier, and shifts the value an hour too
/// far.
#[test]
fn rfc5545_3_3_5_the_offset_before_a_gap_is_the_one_that_was_running() {
    let octets = include_bytes!("fixtures/break_tz_hostile/two_transitions_in_one_day.ics");
    let (_, table) = zone(octets, "Twice/InADay").expect("the fixture declares it");
    let asked = stamp(2026, 3, 29, 20, 30).expect("a wall clock the second transition sprang over");
    let answer = table
        .resolve("Twice/InADay", asked)
        .expect("the identifier is the table's own");
    let LocalResolution::Nonexistent {
        offset_before,
        shifted,
        ..
    } = answer.resolution
    else {
        panic!(
            "20:30 that evening names no instant: {:?}",
            answer.resolution
        )
    };
    assert_eq!(Some(offset_before), offset(7200));
    assert_eq!(Some(shifted), utc(2026, 3, 29, 18, 30));
}

// ---------------------------------------------------------------------------------------
// docs/adr/0003: what `None` is allowed to mean
// ---------------------------------------------------------------------------------------

/// An observance this reader cannot use is reported by somebody.
///
/// Two shapes, both legal to write and both dropped: an observance whose `DTSTART` is a `DATE`
/// rather than a `DATE-TIME`, and one whose `TZOFFSETFROM` and `TZOFFSETTO` are present and
/// outside the range section 3.3.14 admits. `reader.rs` states that section 3.6's reading of "a
/// missing or unreadable required property" is `Component::audit`'s and deliberately not its
/// own — but `audit` counts a property that is present, so for both files it reports nothing,
/// and the reader reports nothing either.
///
/// What survives is a `VTIMEZONE` that is in the set, carries no observance, and answers `None`
/// to every question. `ZoneSource::resolve` documents `None` as meaning exactly one thing —
/// this source does not recognize this identifier — so the definition the file wrote is
/// indistinguishable from one it never wrote, with no code emitted on either side to tell them
/// apart. `vtimezone-without-observance` exists for nearly this claim and is not reached,
/// because the subcomponent was there and only its content was unusable.
#[test]
fn adr0003_an_observance_this_reader_cannot_use_is_reported_by_somebody() {
    let cases: [(&str, &[u8], &str); 2] = [
        (
            "a DTSTART written as a DATE",
            include_bytes!("fixtures/break_tz_hostile/observance_dtstart_is_a_date.ics"),
            "Europe/Berlin",
        ),
        (
            "an offset of +99:99",
            include_bytes!("fixtures/break_tz_hostile/absurd_offsets.ics"),
            "Absurd/Refused",
        ),
    ];
    for (name, octets, tzid) in cases {
        let (reported, table) = zone(octets, tzid).expect("the fixture declares it");
        assert!(table.is_empty(), "{name}: the observance was dropped");
        let document = Document::parse(octets, Limits::DEFAULT, &mut IgnoreDiagnostics)
            .expect("the fixture parses");
        let calendar = document.components().next().expect("one calendar");
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut audited = Vec::new();
        audit_deeply(calendar, &mut meter, &mut audited);
        let said: Vec<DiagnosticCode> = reported.into_iter().chain(codes(&audited)).collect();
        assert!(
            !said.is_empty(),
            "{name}: a definition was thrown away and nothing anywhere said so"
        );
    }
}

/// The contrast that makes the case above a hole rather than a policy.
///
/// When a required property of an observance is *absent* rather than present and unusable,
/// section 3.6's own audit does find it and reports `missing-required-property`, which is
/// exactly the delegation `reader.rs` documents. The reader still says nothing and the zone is
/// still empty, and that is defensible here because somebody said so. Nothing distinguishes
/// this file from the two above except where the unreadability sits.
#[test]
fn rfc5545_3_6_an_observance_missing_an_offset_outright_is_reported_by_the_audit() {
    let octets = include_bytes!("fixtures/break_tz_hostile/observance_without_offsetto.ics");
    let (reported, table) = zone(octets, "Missing/OffsetTo").expect("the fixture declares it");
    assert!(reported.is_empty(), "the reader defers this reading");
    assert!(table.is_empty());

    let document =
        Document::parse(octets, Limits::DEFAULT, &mut IgnoreDiagnostics).expect("it parses");
    let calendar = document.components().next().expect("one calendar");
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut audited = Vec::new();
    audit_deeply(calendar, &mut meter, &mut audited);
    assert_eq!(
        codes(&audited),
        vec![DiagnosticCode::MissingRequiredProperty],
        "an absent required property is the audit's finding, and it makes it"
    );
}

// ---------------------------------------------------------------------------------------
// docs/adr/0010 and RFC 5545 section 3.3.4: the bounds, which hold
// ---------------------------------------------------------------------------------------

/// A million `RDATE` transitions is a file somebody can write, and reading one terminates,
/// stays inside the caller's bound, and says it was cut.
#[test]
fn adr0010_a_million_rdate_transitions_is_bounded_and_reported() {
    let text = flood_of_rdates();
    assert!(text.len() > 15_000_000, "the input really is that large");
    let (reported, tables) =
        zones_of(text.as_bytes(), Limits::DEFAULT).expect("a calendar this large still parses");
    let table = tables.first().expect("one zone");
    assert_eq!(
        u32::try_from(table.observances().len()).ok(),
        Some(Limits::DEFAULT.max_vtimezone_observances()),
        "the caller's bound is what the table holds, not what the file offered"
    );
    assert!(table.is_truncated());
    assert_eq!(
        codes(&reported),
        vec![DiagnosticCode::VtimezoneObservancesTruncated],
        "exhaustion is reported as itself"
    );
    let asked = stamp(2026, 7, 1, 12, 0).expect("an ordinary day");
    assert!(
        table.resolve("Flood/Zone", asked).is_some(),
        "and the table it produced still answers"
    );
}

/// Sixty `RDATE` lines of a megabyte each, which is about a million transitions inside the
/// default octet budget.
fn flood_of_rdates() -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example Corp//EN\r\n");
    text.push_str("BEGIN:VTIMEZONE\r\nTZID:Flood/Zone\r\nBEGIN:STANDARD\r\n");
    text.push_str("TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nDTSTART:19700101T000000\r\n");
    for line in 0..60_u32 {
        text.push_str("RDATE:");
        for entry in 0..16_000_u32 {
            if entry != 0 {
                text.push(',');
            }
            // Spread over a thousand years and twelve months, so the sort the table does is a
            // real sort and the truncation lands somewhere the file did not choose.
            let year = entry.rem_euclid(1000).saturating_add(2000);
            let month = line.rem_euclid(12).saturating_add(1);
            let day = entry.rem_euclid(28).saturating_add(1);
            let _ = write!(text, "{year:04}{month:02}{day:02}T030000");
        }
        text.push_str("\r\n");
    }
    text.push_str("END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n");
    text
}

/// A rule with no `UNTIL`, asked about every year RFC 5545 section 3.3.4 can write.
///
/// Ten thousand years at two questions a year, against a zone whose rules run forever, is the
/// sweep `docs/adr/0010` says a lookup must not be able to turn into unbounded work. Each answer
/// is checked against the rule's own arithmetic rather than against the previous answer: the
/// fixture starts daylight time on the first Sunday of January and ends it on the first Sunday
/// of July, so a query in April is inside it and one in October is not.
#[test]
fn adr0010_a_ten_thousand_year_sweep_terminates_and_agrees_with_the_rule() {
    let octets = include_bytes!("fixtures/break_tz_hostile/endless_rule_year_zero_to_9999.ics");
    let (reported, table) = zone(octets, "Antarctic/Endless").expect("the fixture declares it");
    assert!(reported.is_empty());
    assert_eq!(
        table.coverage_end(),
        None,
        "a rule with no UNTIL knows every year"
    );

    let mut asked = 0_u32;
    for year in 0..=9999_u16 {
        for (month, daylight) in [(4_u8, true), (10_u8, false)] {
            let local = stamp(year, month, 15, 12, 0).expect("the middle of a month");
            let answer = table
                .resolve("Antarctic/Endless", local)
                .expect("the identifier is the table's own");
            let reading = only_reading(answer).expect("an ordinary day names one instant");
            assert_eq!(reading.daylight, daylight, "{year}-{month}");
            assert_eq!(
                Some(reading.offset),
                offset(if daylight { 3600 } else { 0 })
            );
            assert_eq!(answer.basis, AnswerBasis::Computed);
            asked = asked.saturating_add(1);
        }
    }
    assert_eq!(asked, 20_000);
}

/// The two ends of the calendar, where the arithmetic that projects a wall clock either stays
/// inside the writable years or refuses.
///
/// `arithmetic_side_effects` is denied in this workspace, so the interesting failure here would
/// be a panic rather than a wrong answer. There is none: the first Sunday of January 9999 falls
/// on the 3rd, and the resolver agrees.
#[test]
fn rfc5545_3_3_4_the_ends_of_the_writable_years_answer_rather_than_panic() {
    let octets = include_bytes!("fixtures/break_tz_hostile/endless_rule_year_zero_to_9999.ics");
    let (_, table) = zone(octets, "Antarctic/Endless").expect("the fixture declares it");
    let latest = stamp(9999, 12, 31, 23, 59).expect("the last minute the format writes");
    let last = table
        .resolve("Antarctic/Endless", latest)
        .expect("the identifier is the table's own");
    assert_eq!(only_reading(last).map(|reading| reading.offset), offset(0));

    let sprung_over = stamp(9999, 1, 3, 0, 30).expect("the first Sunday of January 9999");
    let missing = table
        .resolve("Antarctic/Endless", sprung_over)
        .expect("the identifier is the table's own");
    assert!(
        missing.resolution.is_nonexistent(),
        "the first Sunday of January 9999 is the 3rd, and 00:30 that day is sprung over"
    );
    let next_day = stamp(9999, 1, 4, 0, 30).expect("the day after");
    let after = table
        .resolve("Antarctic/Endless", next_day)
        .expect("the identifier is the table's own");
    assert_eq!(
        only_reading(after).map(|reading| reading.offset),
        offset(3600)
    );

    for seconds in [i64::MAX, i64::MIN] {
        let instant = Instant::from_unix_seconds(seconds);
        assert!(
            table.offset_at("Antarctic/Endless", instant).is_some(),
            "an instant off the calendar is answered, not panicked over"
        );
    }
}

/// An offset that is legal and absurd is carried rather than truncated, and the wall clocks it
/// makes ambiguous are found.
///
/// `+23:59` to `-23:59` is the widest transition section 3.3.14 can write: the clock goes back
/// by nearly forty-eight hours, so two whole days of wall clock repeat. Both readings are found,
/// in order, with the offsets that produced them.
#[test]
fn rfc5545_3_3_14_the_widest_offsets_the_format_writes_are_carried() {
    let octets = include_bytes!("fixtures/break_tz_hostile/absurd_offsets.ics");
    let (_, table) = zone(octets, "Absurd/Legal").expect("the fixture declares it");
    let asked = stamp(2026, 5, 31, 12, 0).expect("a wall clock inside the repeated stretch");
    let answer = table
        .resolve("Absurd/Legal", asked)
        .expect("the identifier is the table's own");
    let LocalResolution::Ambiguous { earlier, later } = answer.resolution else {
        panic!("a clock that went back two days repeats a lot of wall time")
    };
    assert!(earlier.instant < later.instant);
    assert_eq!(Some(earlier.offset), offset(86_340));
    assert_eq!(Some(later.offset), offset(-86_340));
}

/// A `VTIMEZONE` nested inside a `VEVENT` defines no zone.
///
/// RFC 5545 section 3.6 makes `VTIMEZONE` a calendar component, so a definition buried inside an
/// event is not one, and the `TZID` the same event names is then a `TZID` nothing defines. That
/// is what is reported, which is the honest reading: the alternative is honoring a zone
/// definition from a place the specification does not put one.
#[test]
fn rfc5545_3_6_a_vtimezone_nested_in_an_event_defines_no_zone() {
    let octets = include_bytes!("fixtures/break_tz_hostile/vtimezone_nested_in_a_vevent.ics");
    let (reported, tables) =
        zones_of(octets, Limits::DEFAULT).expect("the fixture parses as a calendar");
    assert!(tables.is_empty());
    assert_eq!(
        codes(&reported),
        vec![DiagnosticCode::MissingTimeZoneDefinition]
    );
}

/// The walk that looks for identifiers nothing defines keeps a worklist rather than recursing,
/// so nesting as deep as the parse admits costs heap rather than stack.
#[test]
fn adr0010_a_calendar_nested_as_deeply_as_the_parse_admits_is_walked_on_the_heap() {
    let depth = u32::from(Limits::DEFAULT.max_component_depth()).saturating_sub(2);
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example Corp//EN\r\n");
    for level in 0..depth {
        text.push_str("BEGIN:X-NEST\r\n");
        let _ = write!(text, "X-A;TZID=Deep/Zone{level}:v\r\n");
    }
    for _ in 0..depth {
        text.push_str("END:X-NEST\r\n");
    }
    text.push_str("END:VCALENDAR\r\n");
    let (reported, tables) =
        zones_of(text.as_bytes(), Limits::DEFAULT).expect("a nested calendar still parses");
    assert!(tables.is_empty());
    assert_eq!(
        u32::try_from(reported.len()).ok(),
        Some(depth),
        "one report per identifier nothing defines, and none lost to the nesting"
    );
}
