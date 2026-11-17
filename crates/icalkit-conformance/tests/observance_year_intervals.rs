// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Year spacing follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Forward year enumeration checks transition admission and lookup independently.

#![cfg(test)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Instant, Limits, Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, ZoneSource, read_calendar_zones};

mod support;

use support::observance_dates::Pattern;

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn clock(date: CivilDate, hour: u8, minute: u8) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::from_hms(hour, minute, 0).unwrap())
}

fn stamp(value: CivilDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        value.date().year(),
        value.date().month(),
        value.date().day(),
        value.time().hour(),
        value.time().minute(),
        value.time().second()
    )
}

fn component(
    kind: &str,
    start: &str,
    offsets: (&str, &str),
    rule: Option<&str>,
    extra: &str,
) -> String {
    let (from, to) = offsets;
    let rule = rule.map_or_else(String::new, |text| format!("RRULE:{text}\r\n"));
    format!(
        "BEGIN:{kind}\r\nDTSTART:{start}\r\nTZOFFSETFROM:{from}\r\nTZOFFSETTO:{to}\r\n{rule}{extra}END:{kind}\r\n"
    )
}

fn read_components(components: &str, limits: Limits) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit year intervals//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/YearIntervals\r\n{components}END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    assert_eq!(document.to_bytes(), text.as_bytes());
    (zones, meter, diagnostics)
}

fn rule_text(interval: &str, parts: &str) -> String {
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(";{parts}")
    };
    format!("FREQ=YEARLY;INTERVAL={interval}{suffix}")
}

fn read_at(
    interval: &str,
    parts: &str,
    start: &str,
    offsets: (&str, &str),
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    read_components(
        &component(
            "DAYLIGHT",
            start,
            offsets,
            Some(&rule_text(interval, parts)),
            extra,
        ),
        limits,
    )
}

fn admitted(interval: u32, pattern: Pattern, anchor: CivilDate) -> VtimezoneSet {
    let (zones, _, diagnostics) = read_at(
        &interval.to_string(),
        pattern.parts(),
        &stamp(clock(anchor, 2, 0)),
        ("+0000", "+0100"),
        "",
        Limits::DEFAULT,
    );
    assert!(
        diagnostics.is_empty(),
        "{interval}: {pattern:?}: {diagnostics:?}"
    );
    zones
}

fn onsets(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    zones
        .table("Test/YearIntervals")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|observance| observance.transition_in(year).map(CivilDateTime::date))
        .collect()
}

fn offset_at(zones: &VtimezoneSet, instant: Instant) -> i32 {
    zones
        .table("Test/YearIntervals")
        .unwrap()
        .offset_at("Test/YearIntervals", instant)
        .unwrap()
        .offset
        .seconds()
}

fn forward_years(first: u16, interval: u32, last: u16) -> BTreeSet<u16> {
    assert!(interval > 0);
    let mut years = BTreeSet::new();
    let mut current = u32::from(first);
    while current <= u32::from(last) {
        years.insert(u16::try_from(current).unwrap());
        let Some(next) = current.checked_add(interval) else {
            break;
        };
        current = next;
    }
    years
}

#[test]
fn every_supported_pattern_keeps_its_anchor_year_spacing() {
    for pattern in Pattern::ALL {
        let anchor = *pattern.dates(2000).first().unwrap();
        let dates: Vec<_> = (2000..2400)
            .chain([0, 1999, 9999])
            .map(|year| (year, pattern.dates(year)))
            .collect();
        for interval in [
            1_u32,
            2,
            3,
            4,
            6,
            25,
            99,
            100,
            128,
            399,
            400,
            401,
            9999,
            u32::MAX,
        ] {
            let zones = admitted(interval, pattern, anchor);
            let years = forward_years(anchor.year(), interval, 9999);
            for (year, dates) in &dates {
                let expected = if years.contains(year) {
                    dates.clone()
                } else {
                    BTreeSet::new()
                };
                assert_eq!(
                    onsets(&zones, *year),
                    expected,
                    "{pattern:?}, interval={interval}, year={year}"
                );
            }
        }
    }
}

#[test]
fn zero_padding_preserves_positive_interval_phases() {
    for interval in [2_u32, 4, 100, u32::MAX] {
        let years = forward_years(2024, interval, 2032);
        for padding in 0..=16 {
            let value = format!("{}{interval}", "0".repeat(padding));
            let (zones, _, diagnostics) = read_at(
                &value,
                Pattern::MarchSunday.parts(),
                "20240310T020000",
                ("+0000", "+0100"),
                "",
                Limits::DEFAULT,
            );
            assert!(diagnostics.is_empty(), "{value}: {diagnostics:?}");
            for year in 2024..=2032 {
                let expected = if years.contains(&year) {
                    Pattern::MarchSunday.dates(year)
                } else {
                    BTreeSet::new()
                };
                assert_eq!(onsets(&zones, year), expected, "{value}, {year}");
            }
        }
    }
}

fn rare_february_dates() -> Vec<CivilDate> {
    (1600..=9998)
        .filter_map(|year| CivilDate::from_ymd(year, 2, 29))
        .filter(|value| value.weekday() == Some(Weekday::Sunday))
        .collect()
}

fn sparse_definition(
    anchor: CivilDate,
    interval: u32,
    dates: &[CivilDate],
    include_last: bool,
) -> String {
    let mut components = component(
        "DAYLIGHT",
        &stamp(clock(anchor, 2, 0)),
        ("+0000", "+0100"),
        Some(&rule_text(&interval.to_string(), "BYMONTH=2;BYDAY=5SU")),
        "",
    );
    let count = dates.len().checked_sub(usize::from(!include_last)).unwrap();
    let resets: Vec<_> = dates
        .iter()
        .take(count)
        .map(|value| stamp(clock(value.checked_add_days(1).unwrap(), 3, 0)))
        .collect();
    if let Some((first, others)) = resets.split_first() {
        let mut extra = String::new();
        for value in others {
            write!(&mut extra, "RDATE:{value}\r\n").unwrap();
        }
        components.push_str(&component(
            "STANDARD",
            first,
            ("+0100", "+0000"),
            None,
            &extra,
        ));
    }
    components
}

#[test]
fn every_interval_residue_finds_the_latest_sparse_onset() {
    let dates = rare_february_dates();
    let anchors: Vec<_> = dates
        .iter()
        .copied()
        .take_while(|value| value.year() < 2000)
        .collect();
    assert!(anchors.len() > 1);
    let query = clock(date(9998, 12, 31), 12, 0)
        .at_offset(UtcOffset::UTC)
        .unwrap();
    for anchor in anchors {
        for interval in (1_u32..=400).chain([401, 799, 800, 801, 1000, 9999, u32::MAX]) {
            let years = forward_years(anchor.year(), interval, 9998);
            let selected: Vec<_> = dates
                .iter()
                .filter(|value| years.contains(&value.year()))
                .copied()
                .collect();
            for (include_last, expected) in [(false, 3600), (true, 0)] {
                let components = sparse_definition(anchor, interval, &selected, include_last);
                let (zones, _, diagnostics) = read_components(&components, Limits::DEFAULT);
                assert!(
                    diagnostics.is_empty(),
                    "{anchor:?}, {interval}: {diagnostics:?}"
                );
                let found = offset_at(&zones, query);
                assert_eq!(
                    found, expected,
                    "anchor={anchor:?}, interval={interval}, include_last={include_last}"
                );
            }
        }
    }
}

#[test]
fn sparse_leap_centuries_do_not_invent_intervening_transitions() {
    for (reset, reset_rule, expected) in [
        (
            "22000101T030000",
            "FREQ=YEARLY;INTERVAL=400;BYMONTH=1;BYMONTHDAY=1",
            3600,
        ),
        (
            "20000301T030000",
            "FREQ=YEARLY;INTERVAL=400;BYMONTH=3;BYMONTHDAY=1",
            0,
        ),
    ] {
        let mut components = component(
            "DAYLIGHT",
            "20000229T020000",
            ("+0000", "+0100"),
            Some("FREQ=YEARLY;INTERVAL=100;BYMONTH=2;BYMONTHDAY=29"),
            "",
        );
        components.push_str(&component(
            "STANDARD",
            reset,
            ("+0100", "+0000"),
            Some(reset_rule),
            "",
        ));
        let (zones, _, diagnostics) = read_components(&components, Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let query = clock(date(2499, 6, 1), 12, 0)
            .at_offset(UtcOffset::UTC)
            .unwrap();
        assert_eq!(offset_at(&zones, query), expected);
    }
}

#[test]
fn interval_lookup_aligns_at_both_utc_year_boundaries() {
    let definitions = [
        (
            component(
                "DAYLIGHT",
                "20000101T000000",
                ("+0200", "+0300"),
                Some("FREQ=YEARLY;INTERVAL=100;BYMONTH=1;BYMONTHDAY=1"),
                "",
            ),
            component(
                "STANDARD",
                "20000102T000000",
                ("+0300", "+0200"),
                Some("FREQ=YEARLY;INTERVAL=100;BYMONTH=1;BYMONTHDAY=2"),
                "",
            ),
            clock(date(2399, 12, 31), 21, 30),
            clock(date(2399, 12, 31), 22, 30),
            (7200, 10_800),
        ),
        (
            component(
                "DAYLIGHT",
                "20001231T230000",
                ("-0200", "-0100"),
                Some("FREQ=YEARLY;INTERVAL=100;BYMONTH=12;BYMONTHDAY=31"),
                "",
            ),
            component(
                "STANDARD",
                "20010101T030000",
                ("-0100", "-0200"),
                Some("FREQ=YEARLY;INTERVAL=100;BYMONTH=1;BYMONTHDAY=1"),
                "",
            ),
            clock(date(2401, 1, 1), 0, 30),
            clock(date(2401, 1, 1), 1, 30),
            (-7200, -3600),
        ),
    ];
    for (mut daylight, standard, before, after, (expected_before, expected_after)) in definitions {
        daylight.push_str(&standard);
        let (zones, _, diagnostics) = read_components(&daylight, Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        for (at, expected) in [(before, expected_before), (after, expected_after)] {
            let instant = at.at_offset(UtcOffset::UTC).unwrap();
            assert_eq!(offset_at(&zones, instant), expected, "{at:?}");
        }
    }
}

#[test]
fn leap_anniversaries_skip_absent_dates_without_restarting_spacing() {
    for (interval, through, expected) in [
        (3_u32, 2036, vec![2024, 2036]),
        (100, 2224, vec![2024, 2124, 2224]),
    ] {
        let zones = admitted(interval, Pattern::LeapAnniversary, date(2024, 2, 29));
        let years: Vec<_> = (2024..=through)
            .filter(|year| !onsets(&zones, *year).is_empty())
            .collect();
        assert_eq!(years, expected);
    }
    let zones = admitted(100, Pattern::LeapAnniversary, date(2000, 2, 29));
    let years: Vec<_> = (2000..=2800)
        .filter(|year| !onsets(&zones, *year).is_empty())
        .collect();
    assert_eq!(years, [2000, 2400, 2800]);
}

#[test]
fn positions_still_select_within_one_calendar_year() {
    let zones = admitted(2, Pattern::SelectedSundays, date(2024, 3, 3));
    assert_eq!(onsets(&zones, 2025), BTreeSet::new());
    assert_eq!(
        onsets(&zones, 2026),
        [date(2026, 3, 1), date(2026, 9, 27)].into_iter().collect()
    );
    assert_eq!(zones.table("Test/YearIntervals").unwrap().rules().len(), 2);
}

#[test]
fn intervals_keep_clocks_bounds_offsets_and_independent_explicit_dates() {
    let (zones, meter, diagnostics) = read_at(
        "3",
        "BYMONTH=3;BYDAY=2SU;UNTIL=20300310T070000Z",
        "20240310T020000",
        ("-0500", "-0400"),
        "RDATE:20250820T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/YearIntervals").unwrap();
    assert_eq!(table.rules().len(), 1);
    assert_eq!(meter.vtimezone_observances(), 2);
    let observance = table.rules()[0];
    assert_eq!(observance.start(), clock(date(2024, 3, 10), 2, 0));
    assert_eq!(observance.offset_from().seconds(), -18_000);
    assert_eq!(observance.offset_to().seconds(), -14_400);
    assert_eq!(
        observance.rule().unwrap().at(),
        CivilTime::from_hms(2, 0, 0).unwrap()
    );
    assert_eq!(
        observance.rule().unwrap().through(),
        Some(date(2030, 3, 10))
    );
    assert_eq!(observance.transition_in(2023), None);
    assert_eq!(observance.transition_in(2025), None);
    assert_eq!(
        observance.transition_in(2027),
        Some(clock(date(2027, 3, 14), 2, 0))
    );
    assert_eq!(
        observance.transition_in(2030),
        Some(clock(date(2030, 3, 10), 2, 0))
    );
    assert_eq!(observance.transition_in(2031), None);
    let explicit: Vec<_> = table
        .observances()
        .iter()
        .filter(|value| value.rule().is_none())
        .map(|value| value.start())
        .collect();
    assert_eq!(explicit, [clock(date(2025, 8, 20), 2, 0)]);
    assert_eq!(table.coverage_end(), Some(date(2030, 3, 10)));
}

#[test]
fn intervals_keep_every_pattern_and_explicit_date_metered() {
    for cap in 0_u32..=14 {
        let (zones, meter, diagnostics) = read_at(
            "7",
            "BYMONTHDAY=1",
            "20240101T020000",
            ("+0000", "+0100"),
            "RDATE:20250215T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/YearIntervals").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(13));
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(13)).unwrap()
        );
        assert_eq!(table.is_truncated(), cap < 13);
        let expected = if cap < 13 {
            vec![DiagnosticCode::VtimezoneObservancesTruncated]
        } else {
            Vec::new()
        };
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn malformed_intervals_keep_one_anchor_and_one_note() {
    for interval in [
        "",
        "0",
        "00",
        "+1",
        "-1",
        "1.0",
        " 1",
        "1 ",
        "1x",
        "4294967296",
        "18446744073709551616",
        "１",
    ] {
        let (zones, meter, diagnostics) = read_at(
            interval,
            "BYMONTH=3;BYDAY=2SU",
            "20240310T020000",
            ("+0000", "+0100"),
            "RDATE:20250820T020000\r\n",
            Limits::DEFAULT,
        );
        let table = zones.table("Test/YearIntervals").unwrap();
        assert_eq!(table.rules(), [], "{interval}");
        assert_eq!(table.observances().len(), 2);
        assert_eq!(meter.vtimezone_observances(), 2);
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported]
        );
    }
}

#[test]
fn huge_intervals_keep_the_anchor_without_wrapping_years() {
    let zones = admitted(u32::MAX, Pattern::MonthStarts, date(2000, 1, 1));
    assert_eq!(onsets(&zones, 2000), Pattern::MonthStarts.dates(2000));
    for year in [0, 1999, 2001, 4000, 9999] {
        assert_eq!(onsets(&zones, year), BTreeSet::new());
    }
    let at: Instant = clock(date(9999, 12, 31), 12, 0)
        .at_offset(UtcOffset::UTC)
        .unwrap();
    assert_eq!(offset_at(&zones, at), 3600);
}
