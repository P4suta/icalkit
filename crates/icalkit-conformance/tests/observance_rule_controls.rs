// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Observance rule controls follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

const BASE: &str = "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU";

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn second_sunday(year: u16) -> CivilDate {
    (8..=14)
        .map(|day| date(year, 3, day))
        .find(|value| value.weekday() == Some(Weekday::Sunday))
        .unwrap()
}

fn read_at(
    rule: &str,
    extra: &str,
    start: &str,
    offsets: (&str, &str),
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let (from, to) = offsets;
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule controls//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/RuleControls\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:{from}\r\nTZOFFSETTO:{to}\r\n\
         RRULE:{rule}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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

fn read(rule: &str, extra: &str) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    read_at(
        rule,
        extra,
        "20240310T020000",
        ("-0500", "-0400"),
        Limits::DEFAULT,
    )
}

fn admitted(rule: &str) -> VtimezoneSet {
    let (zones, meter, diagnostics) = read(rule, "");
    assert!(diagnostics.is_empty(), "{rule}: {diagnostics:?}");
    assert_eq!(meter.vtimezone_observances(), 1, "{rule}");
    assert_eq!(zones.table("Test/RuleControls").unwrap().rules().len(), 1);
    zones
}

fn assert_refused(rule: &str) {
    let (zones, meter, diagnostics) = read(rule, "RDATE:20240820T020000\r\n");
    let table = zones.table("Test/RuleControls").unwrap();
    assert_eq!(table.rules(), [], "{rule}");
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    assert_eq!(
        table
            .observances()
            .iter()
            .map(|observance| observance.start())
            .collect::<Vec<_>>(),
        [
            CivilDateTime::new(date(2024, 3, 10), clock),
            CivilDateTime::new(date(2024, 8, 20), clock),
        ],
        "{rule}"
    );
    for observance in table.observances() {
        assert_eq!(
            observance.offset_from(),
            UtcOffset::from_seconds(-18_000).unwrap()
        );
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(-14_400).unwrap()
        );
    }
    assert_eq!(meter.vtimezone_observances(), 2, "{rule}");
    assert_eq!(
        diagnostics
            .iter()
            .copied()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        [DiagnosticCode::VtimezoneRuleUnsupported],
        "{rule}"
    );
    assert_eq!(diagnostics[0].severity(), Severity::Note);
}

#[test]
fn repeated_frequency_parts_keep_the_anchor_and_explicit_dates() {
    for rule in [
        format!("{BASE};FREQ=YEARLY"),
        format!("freq=yearly;{BASE}"),
        format!("{BASE};freq=yearly"),
    ] {
        assert_refused(&rule);
    }
}

#[test]
fn repeated_interval_parts_keep_the_anchor_and_explicit_dates() {
    for rule in [
        format!("{BASE};INTERVAL=1;INTERVAL=1"),
        format!("INTERVAL=1;{BASE};interval=1"),
        format!("{BASE};interval=01;INTERVAL=1"),
    ] {
        assert_refused(&rule);
    }
}

#[test]
fn repeated_week_starts_keep_the_anchor_and_explicit_dates() {
    for rule in [
        format!("{BASE};WKST=MO;WKST=MO"),
        format!("wkst=TU;{BASE};WKST=SU"),
        format!("{BASE};WKST=SU;wkst=MO"),
    ] {
        assert_refused(&rule);
    }
}

#[test]
fn repeated_ends_never_replace_the_first_bound() {
    for rule in [
        format!("{BASE};UNTIL=20260308T070000Z;UNTIL=20260308T070000Z"),
        format!("UNTIL=20260308T070000Z;{BASE};until=20250309T070000Z"),
        format!("{BASE};until=20250309T070000Z;UNTIL=20260308T070000Z"),
    ] {
        assert_refused(&rule);
    }
}

#[test]
fn week_starts_require_one_weekday_token() {
    for value in ["", "MON", "1MO", "MO,TU", " mo", "MO ", "XX", "MO=TU"] {
        assert_refused(&format!("{BASE};WKST={value}"));
    }
}

#[test]
fn all_week_starts_leave_supported_annual_dates_unchanged() {
    for weekday in Weekday::ALL {
        let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
        for value in [name.to_owned(), name.to_ascii_lowercase()] {
            let zones = admitted(&format!("{BASE};WKST={value}"));
            let rule = zones.table("Test/RuleControls").unwrap().rules()[0]
                .rule()
                .unwrap();
            for year in 2024..=2030 {
                assert_eq!(
                    rule.occurrence_in(year),
                    Some(second_sunday(year)),
                    "{value}"
                );
            }
        }
    }
}

#[test]
fn zero_padded_unit_intervals_keep_yearly_spacing() {
    for padding in (0..=64).chain([1024]) {
        let value = format!("{}1", "0".repeat(padding));
        let zones = admitted(&format!("{BASE};INTERVAL={value}"));
        let rule = zones.table("Test/RuleControls").unwrap().rules()[0]
            .rule()
            .unwrap();
        for year in 2024..=2030 {
            assert_eq!(
                rule.occurrence_in(year),
                Some(second_sunday(year)),
                "{value}"
            );
        }
    }
}

#[test]
fn malformed_and_nonunit_intervals_remain_unsupported() {
    for value in [
        "",
        "0",
        "00",
        "2",
        "02",
        "+1",
        "-1",
        "1.0",
        " 1",
        "1 ",
        "1x",
        "0x1",
        "1e0",
        "4294967296",
        "18446744073709551616",
        "１",
    ] {
        assert_refused(&format!("{BASE};INTERVAL={value}"));
    }
    assert_refused(&format!("{BASE};INTERVAL={}", "0".repeat(1024)));
}

#[test]
fn observance_ends_require_utc_date_times() {
    for value in [
        "20260308",
        "20260308T020000",
        "20260308T020000-0500",
        "20260308T070000ZZ",
    ] {
        assert_refused(&format!("{BASE};UNTIL={value}"));
    }
}

#[test]
fn utc_ends_keep_inclusive_wall_dates_across_year_boundaries() {
    for (rule, start, offsets, through, last) in [
        (
            "FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1;UNTIL=20251231T220000Z",
            "20240101T000000",
            ("+0200", "+0300"),
            date(2026, 1, 1),
            2026,
        ),
        (
            "FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=31;UNTIL=20260101T010000Z",
            "20231231T230000",
            ("-0200", "-0100"),
            date(2025, 12, 31),
            2025,
        ),
    ] {
        let (zones, _, diagnostics) = read_at(rule, "", start, offsets, Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{rule}: {diagnostics:?}");
        let observance = zones.table("Test/RuleControls").unwrap().rules()[0];
        assert_eq!(observance.rule().unwrap().through(), Some(through));
        assert_eq!(observance.transition_in(last).unwrap().date(), through);
        assert_eq!(observance.transition_in(last.checked_add(1).unwrap()), None);
    }
}

#[test]
fn supported_parts_keep_any_order_case_and_repeated_list_values() {
    let parts = [
        "FREQ=YEARLY",
        "INTERVAL=1",
        "WKST=MO",
        "BYMONTH=3,03,3",
        "BYMONTHDAY=8,09,10,11,12,13,14,8",
        "BYDAY=2SU,02SU,+2SU",
        "BYYEARDAY=60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75",
        "BYSETPOS=1,01,+001",
        "UNTIL=20260308T070000Z",
    ];
    for rotation in 0..parts.len() {
        let mut reordered = parts;
        reordered.rotate_left(rotation);
        for reverse in [false, true] {
            let mut ordered = reordered;
            if reverse {
                ordered.reverse();
            }
            let upper = ordered.join(";");
            let lower = ordered
                .iter()
                .map(|part| {
                    let (name, value) = part.split_once('=').unwrap();
                    let value = if name == "FREQ" || name == "WKST" || name == "BYDAY" {
                        value.to_ascii_lowercase()
                    } else {
                        value.to_owned()
                    };
                    format!("{}={value}", name.to_ascii_lowercase())
                })
                .collect::<Vec<_>>()
                .join(";");
            for text in [upper, lower] {
                let zones = admitted(&text);
                let rule = zones.table("Test/RuleControls").unwrap().rules()[0]
                    .rule()
                    .unwrap();
                for year in 2024..=2026 {
                    assert_eq!(
                        rule.occurrence_in(year),
                        Some(second_sunday(year)),
                        "{text}"
                    );
                }
                assert_eq!(rule.occurrence_in(2027), None);
            }
        }
    }
}

#[test]
fn malformed_pairs_and_missing_frequency_refuse_the_whole_rule() {
    for rule in [
        "",
        "FREQ",
        "FREQ=",
        "FREQ=YEARLY;",
        ";FREQ=YEARLY",
        "FREQ=YEARLY;;BYMONTH=3",
        "FREQ=YEARLY;WKST",
        "FREQ=YEARLY;INTERVAL",
        "FREQ=YEARLY;=1",
        "BYMONTH=3;BYDAY=2SU",
        "FREQ=YEARLY;INTERVAL=1=1",
        "FREQ=YEARLY; WKST=MO",
        "FREQ=YEARLY;X-CONTROL=1",
    ] {
        assert_refused(rule);
    }
}

#[test]
fn refusing_controls_preserves_the_existing_transition_budget() {
    for cap in [0_u32, 1, 2, 3] {
        let (zones, meter, diagnostics) = read_at(
            &format!("{BASE};WKST=invalid"),
            "RDATE:20240820T020000\r\n",
            "20240310T020000",
            ("-0500", "-0400"),
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/RuleControls").unwrap();
        assert_eq!(table.rules(), []);
        assert_eq!(meter.vtimezone_observances(), cap.min(2));
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(2)).unwrap()
        );
        assert_eq!(table.is_truncated(), cap < 2);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|value| value.code() == DiagnosticCode::VtimezoneRuleUnsupported)
                .count(),
            1
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|value| value.code() == DiagnosticCode::VtimezoneObservancesTruncated)
                .count(),
            usize::from(cap < 2)
        );
    }
}
