// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Month expansion follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Date enumeration checks compiled patterns separately from original anchors.

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(";{parts}")
    };
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month expansion//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Months\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY{suffix}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    assert_eq!(document.to_bytes(), text.as_bytes());
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    (zones, meter, diagnostics)
}

fn admitted(parts: &str, start: &str) -> VtimezoneSet {
    let (zones, _, diagnostics) = read(parts, start, "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    zones
}

fn compiled_dates(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    zones
        .table("Test/Months")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|observance| observance.rule().unwrap().occurrence_in(year))
        .collect()
}

fn expected_dates(year: u16, days: &[i8], weekday: Option<Weekday>) -> BTreeSet<CivilDate> {
    let mut found = BTreeSet::new();
    for month in 1..=12 {
        for day in 1..=31 {
            let Some(value) = CivilDate::from_ymd(year, month, day) else {
                continue;
            };
            let length = CivilDate::days_in_month(year, month).unwrap();
            let positive = i8::try_from(day).unwrap();
            let negative = i16::from(day)
                .checked_sub(i16::from(length))
                .unwrap()
                .checked_sub(1)
                .unwrap();
            if (days.contains(&positive) || days.contains(&i8::try_from(negative).unwrap()))
                && weekday.is_none_or(|expected| value.weekday() == Some(expected))
            {
                found.insert(value);
            }
        }
    }
    found
}

fn anchor(value: CivilDate) -> String {
    format!(
        "{:04}{:02}{:02}T020000",
        value.year(),
        value.month(),
        value.day()
    )
}

#[test]
fn unrestricted_month_days_include_every_month() {
    let zones = admitted("BYMONTHDAY=1", "20240101T020000");
    let expected = (1..=12).map(|month| date(2024, month, 1)).collect();
    assert_eq!(compiled_dates(&zones, 2024), expected);
}

#[test]
fn every_admitted_fixed_day_matches_independent_date_enumeration() {
    for day in (-31_i8..=31).filter(|value| *value != 0) {
        let expected = expected_dates(2000, &[day], None);
        let start = anchor(*expected.first().unwrap());
        let zones = admitted(&format!("BYMONTHDAY={day}"), &start);
        for year in [0, 1, 1900, 2000, 2023, 2024, 2100, 9999] {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, &[day], None),
                "day={day}, year={year}"
            );
        }
    }
}

#[test]
fn weekday_windows_expand_by_the_written_date_intersection() {
    for weekday in Weekday::ALL {
        let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
        let windows = (1_i8..=25)
            .map(|first| (first..=first.checked_add(6).unwrap()).collect::<Vec<_>>())
            .chain(std::iter::once((-7_i8..=-1).collect()));
        for days in windows {
            let written = days.iter().map(i8::to_string).collect::<Vec<_>>().join(",");
            let parts = format!("BYDAY={name};BYMONTHDAY={written}");
            let expected = expected_dates(2000, &days, Some(weekday));
            let zones = admitted(&parts, &anchor(*expected.first().unwrap()));
            for year in [0, 1900, 2000, 2024, 2026, 9999] {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, &days, Some(weekday)),
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn explicit_months_anniversaries_and_year_ordinals_stay_single() {
    for (parts, start, expected) in [
        (
            "BYMONTH=5;BYMONTHDAY=15",
            "20240515T020000",
            date(2024, 5, 15),
        ),
        ("", "20240615T020000", date(2024, 6, 15)),
    ] {
        let zones = admitted(parts, start);
        assert_eq!(
            zones.table("Test/Months").unwrap().rules().len(),
            1,
            "{parts}"
        );
        assert_eq!(
            compiled_dates(&zones, 2024),
            [expected].into_iter().collect(),
            "{parts}"
        );
    }
    let zones = admitted("BYDAY=20MO", "19970519T020000");
    assert_eq!(zones.table("Test/Months").unwrap().rules().len(), 1);
    assert_eq!(
        compiled_dates(&zones, 1997),
        [date(1997, 5, 19)].into_iter().collect()
    );
}

#[test]
fn expanded_rules_keep_original_anchors_clocks_offsets_and_until() {
    let zones = admitted("BYMONTHDAY=15;UNTIL=20250315T020000Z", "20240615T020000");
    let table = zones.table("Test/Months").unwrap();
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    for observance in table.rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 6, 15), clock)
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(3600).unwrap()
        );
        assert_eq!(observance.rule().unwrap().at(), clock);
        assert_eq!(
            observance.rule().unwrap().through(),
            Some(date(2025, 3, 15))
        );
        assert_eq!(observance.transition_in(2023), None);
        assert_eq!(observance.transition_in(2026), None);
    }
    let onsets: BTreeSet<_> = table
        .rules()
        .iter()
        .filter_map(|observance| observance.transition_in(2024))
        .collect();
    let expected = (6..=12)
        .map(|month| CivilDateTime::new(date(2024, month, 15), clock))
        .collect();
    assert_eq!(onsets, expected);
    assert_eq!(
        compiled_dates(&zones, 2025),
        (1..=3).map(|month| date(2025, month, 15)).collect()
    );
    assert_eq!(table.coverage_end(), Some(date(2025, 3, 15)));
}

#[test]
fn explicit_dates_are_stored_once_beside_expanded_rules() {
    let (zones, meter, diagnostics) = read(
        "BYMONTHDAY=15",
        "20240615T020000",
        "RDATE:20240820T020000,20240920T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/Months").unwrap();
    let dated: Vec<_> = table
        .observances()
        .iter()
        .filter(|observance| observance.rule().is_none())
        .map(|observance| observance.start().date())
        .collect();
    assert_eq!(dated, [date(2024, 8, 20), date(2024, 9, 20)]);
    assert_eq!(table.rules().len(), 12);
    assert_eq!(meter.vtimezone_observances(), 14);
}

#[test]
fn every_stored_pattern_is_charged_to_the_existing_observance_bound() {
    for cap in [0_u32, 1, 11, 12, 13] {
        let (zones, meter, diagnostics) = read(
            "BYMONTHDAY=1",
            "20240101T020000",
            "",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/Months").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(12), "cap={cap}");
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(12)).unwrap(),
            "cap={cap}"
        );
        assert_eq!(table.is_truncated(), cap < 12, "cap={cap}");
        if cap < 12 {
            assert_eq!(
                diagnostics
                    .iter()
                    .copied()
                    .map(Diagnostic::code)
                    .collect::<Vec<_>>(),
                [DiagnosticCode::VtimezoneObservancesTruncated],
                "cap={cap}"
            );
            assert_eq!(diagnostics[0].severity(), Severity::LimitReached);
        } else {
            assert!(diagnostics.is_empty(), "cap={cap}: {diagnostics:?}");
        }
    }
}

#[test]
fn unsupported_selectors_keep_one_anchor_and_one_note() {
    for parts in [
        "BYMONTHDAY=-0",
        "BYMONTHDAY=-32",
        "BYMONTHDAY=-003",
        "BYMONTHDAY=1,32",
        "BYMONTHDAY=1;COUNT=4",
        "BYMONTHDAY=1;INTERVAL=2",
        "BYMONTH=1,13;BYMONTHDAY=1",
        "BYDAY=54MO;BYMONTHDAY=1",
        "BYDAY=SX",
        "RSCALE=CHINESE;BYMONTHDAY=1",
    ] {
        let (zones, meter, diagnostics) = read(parts, "20240101T020000", "", Limits::DEFAULT);
        let table = zones.table("Test/Months").unwrap();
        assert_eq!(table.observances().len(), 1, "{parts}");
        assert!(table.rules().is_empty(), "{parts}");
        assert_eq!(meter.vtimezone_observances(), 1, "{parts}");
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported],
            "{parts}"
        );
    }
}

#[test]
fn negative_positions_in_explicit_months_match_independent_enumeration() {
    for month in 1..=12 {
        for day in -31_i8..=-1 {
            let initial = expected_dates(2000, &[day], None)
                .into_iter()
                .find(|value| value.month() == month)
                .unwrap_or(date(2000, 1, 1));
            let parts = format!("BYMONTH={month};BYMONTHDAY={day}");
            let zones = admitted(&parts, &anchor(initial));
            assert_eq!(
                zones.table("Test/Months").unwrap().rules().len(),
                1,
                "{parts}"
            );
            for year in [0, 1, 1900, 2000, 2024, 2100, 9999] {
                let expected = expected_dates(year, &[day], None)
                    .into_iter()
                    .filter(|value| value.month() == month)
                    .collect();
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected,
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn negative_day_positions_follow_the_rfc_example_dates() {
    let zones = admitted("BYMONTHDAY=-3", "19970928T020000");
    let onsets: BTreeSet<_> = zones
        .table("Test/Months")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|observance| observance.transition_in(1997).map(CivilDateTime::date))
        .collect();
    assert_eq!(
        onsets,
        [
            date(1997, 9, 28),
            date(1997, 10, 29),
            date(1997, 11, 28),
            date(1997, 12, 29)
        ]
        .into_iter()
        .collect()
    );
    let following = compiled_dates(&zones, 1998);
    assert!(following.contains(&date(1998, 1, 29)));
    assert!(following.contains(&date(1998, 2, 26)));
}

#[test]
fn negative_positions_keep_clocks_anchors_and_inclusive_end_bounds() {
    for (until, expected) in [
        ("20250226T020000Z", None),
        ("20250227T020000Z", Some(date(2025, 2, 27))),
    ] {
        let zones = admitted(
            &format!("BYMONTH=2;BYMONTHDAY=-2;UNTIL={until}"),
            "20240228T020000",
        );
        let observance = *zones
            .table("Test/Months")
            .unwrap()
            .observances()
            .first()
            .unwrap();
        let clock = CivilTime::from_hms(2, 0, 0).unwrap();
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 2, 28), clock)
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(3600).unwrap()
        );
        assert_eq!(observance.transition_in(2023), None);
        assert_eq!(observance.transition_in(2024), Some(observance.start()));
        assert_eq!(
            observance.transition_in(2025),
            expected.map(|value| CivilDateTime::new(value, clock))
        );
        assert_eq!(observance.transition_in(2026), None);
    }
}

#[test]
fn every_month_set_selects_exactly_its_written_months() {
    let mut month_sets = vec![Vec::<u8>::new()];
    for month in 1_u8..=12 {
        let included: Vec<_> = month_sets
            .iter()
            .cloned()
            .map(|mut months| {
                months.push(month);
                months
            })
            .collect();
        month_sets.extend(included);
    }
    assert_eq!(month_sets.len(), 4096);
    for months in month_sets.into_iter().filter(|months| !months.is_empty()) {
        let written = months
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        for day in [1_i8, -2, 31] {
            let parts = format!("BYMONTH={written};BYMONTHDAY={day}");
            let initial = expected_dates(2000, &[day], None)
                .into_iter()
                .find(|value| months.contains(&value.month()))
                .unwrap_or(date(2000, 1, 1));
            let zones = admitted(&parts, &anchor(initial));
            for year in [0, 1900, 2000, 9999] {
                let expected = expected_dates(year, &[day], None)
                    .into_iter()
                    .filter(|value| months.contains(&value.month()))
                    .collect();
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected,
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn month_set_order_and_duplicate_spellings_keep_one_pattern_per_month() {
    for months in ["1,3,12", "12,3,1", "01,03,12,01,3,12"] {
        let (zones, meter, diagnostics) = read(
            &format!("BYMONTH={months};BYMONTHDAY=-2"),
            "20240130T020000",
            "",
            Limits::DEFAULT,
        );
        assert!(diagnostics.is_empty(), "{months}: {diagnostics:?}");
        assert_eq!(
            zones.table("Test/Months").unwrap().rules().len(),
            3,
            "{months}"
        );
        assert_eq!(meter.vtimezone_observances(), 3, "{months}");
        assert_eq!(
            compiled_dates(&zones, 2024),
            [date(2024, 1, 30), date(2024, 3, 30), date(2024, 12, 30)]
                .into_iter()
                .collect(),
            "{months}"
        );
    }
}

#[test]
fn explicit_month_sets_keep_numeric_weekdays_in_month_scope() {
    for months in [vec![2, 4, 6], (1..=12).collect::<Vec<_>>()] {
        let written = months
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let expected: BTreeSet<_> =
            expected_dates(2024, &[1, 2, 3, 4, 5, 6, 7], Some(Weekday::Monday))
                .into_iter()
                .filter(|value| months.contains(&value.month()))
                .collect();
        let zones = admitted(
            &format!("BYMONTH={written};BYDAY=1MO"),
            &anchor(*expected.first().unwrap()),
        );
        assert_eq!(compiled_dates(&zones, 2024), expected, "{written}");
    }
    let yearly = admitted("BYDAY=1MO", "20240101T020000");
    assert_eq!(
        compiled_dates(&yearly, 2024),
        [date(2024, 1, 1)].into_iter().collect()
    );
}

#[test]
fn month_sets_without_day_parts_keep_the_anchor_day() {
    let zones = admitted("BYMONTH=2,4", "20000229T020000");
    assert_eq!(
        compiled_dates(&zones, 2000),
        [date(2000, 2, 29), date(2000, 4, 29)].into_iter().collect()
    );
    assert_eq!(
        compiled_dates(&zones, 2001),
        [date(2001, 4, 29)].into_iter().collect()
    );
}

#[test]
fn month_set_patterns_keep_end_bounds_explicit_dates_and_meter_charges() {
    for cap in [1_u32, 2, 3] {
        let (zones, meter, diagnostics) = read(
            "BYMONTH=1,3,1;BYMONTHDAY=1;UNTIL=20250301T020000Z",
            "20240101T020000",
            "RDATE:20240820T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/Months").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap, "cap={cap}");
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap).unwrap(),
            "cap={cap}"
        );
        assert_eq!(table.is_truncated(), cap < 3, "cap={cap}");
        if cap < 3 {
            assert_eq!(
                diagnostics
                    .iter()
                    .copied()
                    .map(Diagnostic::code)
                    .collect::<Vec<_>>(),
                [DiagnosticCode::VtimezoneObservancesTruncated],
                "cap={cap}"
            );
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            assert_eq!(table.rules().len(), 2);
            assert_eq!(
                table
                    .observances()
                    .iter()
                    .filter(|value| value.rule().is_none())
                    .count(),
                1
            );
            assert_eq!(
                compiled_dates(&zones, 2025),
                [date(2025, 1, 1), date(2025, 3, 1)].into_iter().collect()
            );
            let clock = CivilTime::from_hms(2, 0, 0).unwrap();
            for observance in table.rules() {
                assert_eq!(
                    observance.start(),
                    CivilDateTime::new(date(2024, 1, 1), clock)
                );
                assert_eq!(observance.offset_from(), UtcOffset::UTC);
                assert_eq!(
                    observance.offset_to(),
                    UtcOffset::from_seconds(3600).unwrap()
                );
                assert_eq!(observance.rule().unwrap().at(), clock);
                assert_eq!(observance.rule().unwrap().through(), Some(date(2025, 3, 1)));
                assert_eq!(observance.transition_in(2026), None);
            }
        }
    }
}

#[test]
fn invalid_month_lists_keep_one_anchor_and_one_diagnostic() {
    for months in [
        "", "0", "13", "-1", "+1", "+01", "001", "1,", ",1", "1,,2", "1,13", "1,2,x",
    ] {
        let (zones, meter, diagnostics) = read(
            &format!("BYMONTH={months};BYMONTHDAY=1"),
            "20240101T020000",
            "",
            Limits::DEFAULT,
        );
        let table = zones.table("Test/Months").unwrap();
        assert_eq!(table.observances().len(), 1, "{months}");
        assert!(table.rules().is_empty(), "{months}");
        assert_eq!(meter.vtimezone_observances(), 1, "{months}");
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported],
            "{months}"
        );
    }
}

#[test]
fn repeated_month_parts_do_not_replace_or_merge_the_first_set() {
    for parts in [
        "BYMONTH=1;BYMONTH=3;BYMONTHDAY=1",
        "BYMONTH=1;BYMONTH=1;BYMONTHDAY=1",
        "BYMONTH=1,3;BYMONTH=3;BYMONTHDAY=1",
    ] {
        let (zones, _, diagnostics) = read(parts, "20240101T020000", "", Limits::DEFAULT);
        let table = zones.table("Test/Months").unwrap();
        assert_eq!(table.observances().len(), 1, "{parts}");
        assert!(table.rules().is_empty(), "{parts}");
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported],
            "{parts}"
        );
    }
}
