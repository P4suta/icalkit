// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Month-day sets follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Independent date enumeration checks compiled patterns separately from anchors.

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

const MONTHS: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn signed_days() -> Vec<i8> {
    (-31_i8..=31).filter(|day| *day != 0).collect()
}

fn written_days(days: &[i8]) -> String {
    days.iter().map(i8::to_string).collect::<Vec<_>>().join(",")
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month-day sets//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/DaySets\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{parts}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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

fn admitted(parts: &str) -> VtimezoneSet {
    let (zones, _, diagnostics) = read(parts, "20000101T020000", "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    zones
}

fn compiled_dates(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    let dates: Vec<_> = zones
        .table("Test/DaySets")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|observance| observance.rule().unwrap().occurrence_in(year))
        .collect();
    let unique: BTreeSet<_> = dates.iter().copied().collect();
    assert_eq!(dates.len(), unique.len(), "duplicate dates in {year}");
    unique
}

fn expected_dates(
    year: u16,
    months: &[u8],
    days: &[i8],
    weekday: Option<Weekday>,
) -> BTreeSet<CivilDate> {
    let mut expected = BTreeSet::new();
    for &month in months {
        let dates: Vec<_> = (1..=31)
            .filter_map(|day| CivilDate::from_ymd(year, month, day))
            .collect();
        for (index, &value) in dates.iter().enumerate() {
            let positive = i8::try_from(value.day()).unwrap();
            let negative = i8::try_from(
                i16::try_from(index)
                    .unwrap()
                    .checked_sub(i16::try_from(dates.len()).unwrap())
                    .unwrap(),
            )
            .unwrap();
            if (days.contains(&positive) || days.contains(&negative))
                && weekday.is_none_or(|wanted| value.weekday() == Some(wanted))
            {
                expected.insert(value);
            }
        }
    }
    expected
}

#[test]
fn every_signed_day_pair_matches_independent_calendar_membership() {
    let domain = signed_days();
    for (index, &first) in domain.iter().enumerate() {
        for &second in domain.iter().skip(index) {
            let days = [first, second];
            let parts = format!("BYMONTHDAY={}", written_days(&days));
            let zones = admitted(&parts);
            for year in [0, 1900, 2000, 9999] {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, &MONTHS, &days, None),
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn the_full_signed_domain_stores_one_pattern_per_possible_calendar_day() {
    let days = signed_days();
    let parts = format!("BYMONTHDAY={}", written_days(&days));
    let (zones, meter, diagnostics) = read(&parts, "20000101T020000", "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(zones.table("Test/DaySets").unwrap().rules().len(), 366);
    assert_eq!(meter.vtimezone_observances(), 366);
    for year in [0, 1900, 2000, 2100, 9999] {
        assert_eq!(
            compiled_dates(&zones, year),
            expected_dates(year, &MONTHS, &days, None)
        );
    }
}

#[test]
fn duplicate_spellings_and_order_keep_the_same_patterns() {
    let repeated = (0..80)
        .map(|_| "-01,+01,01,15")
        .collect::<Vec<_>>()
        .join(",");
    for written in ["1,15,-1", "-1,15,1", repeated.as_str()] {
        let zones = admitted(&format!("BYMONTHDAY={written}"));
        assert_eq!(zones.table("Test/DaySets").unwrap().rules().len(), 36);
        for year in [1900, 2000, 9999] {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, &MONTHS, &[1, 15, -1], None),
                "{written}, year={year}"
            );
        }
    }
}

#[test]
fn coincident_positive_and_negative_positions_yield_each_date_once() {
    let zones = admitted("BYMONTH=2;BYMONTHDAY=1,-28");
    assert_eq!(zones.table("Test/DaySets").unwrap().rules().len(), 2);
    for year in [0, 1900, 2000, 2024, 9999] {
        assert_eq!(
            compiled_dates(&zones, year),
            expected_dates(year, &[2], &[1, -28], None)
        );
    }
}

#[test]
fn selected_months_intersect_signed_day_sets() {
    let days = [-31, -2, 1, 31];
    let zones = admitted("BYMONTH=2,4,12,2;BYMONTHDAY=-31,-2,1,31");
    for year in [0, 1900, 2000, 2024, 9999] {
        assert_eq!(
            compiled_dates(&zones, year),
            expected_dates(year, &[2, 4, 12], &days, None)
        );
    }
}

#[test]
fn weekday_intersections_match_the_written_date_sets() {
    let selections = [
        vec![13],
        vec![29],
        vec![31],
        vec![-31],
        vec![1, 3, 8, 15, 31],
        (-14_i8..=-8).collect(),
        vec![-31, -8, -1, 1, 8, 31],
        (25..=30).collect(),
        (24..=31).collect(),
        vec![24, 26, 27, 28, 29, 30, 31],
        vec![25, 26, 27, 28, 29, 30, 30],
        (-8..=-2).collect(),
        vec![-1, 25, 26, 27, 28, 29, 30],
        signed_days(),
    ];
    for days in selections {
        for weekday in Weekday::ALL {
            let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
            for months in [MONTHS.as_slice(), &[2, 4, 9]] {
                let prefix = if months == MONTHS {
                    String::new()
                } else {
                    String::from("BYMONTH=2,4,9;")
                };
                let parts = format!("{prefix}BYMONTHDAY={};BYDAY={name}", written_days(&days));
                let zones = admitted(&parts);
                for year in [0, 1900, 2000, 2024, 9999] {
                    assert_eq!(
                        compiled_dates(&zones, year),
                        expected_dates(year, months, &days, Some(weekday)),
                        "{parts}, year={year}"
                    );
                }
            }
        }
    }
}

#[test]
fn weekday_selection_matches_a_gregorian_cycle() {
    let days = signed_days();
    for weekday in Weekday::ALL {
        let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
        let parts = format!("BYMONTHDAY={};BYDAY={name}", written_days(&days));
        let zones = admitted(&parts);
        for year in 2000..2400 {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, &MONTHS, &days, Some(weekday)),
                "{parts}, year={year}"
            );
        }
    }
}

#[test]
fn selected_patterns_keep_bounds_anchors_offsets_and_explicit_dates() {
    let (zones, meter, diagnostics) = read(
        "BYMONTH=2,4;BYMONTHDAY=1,-1;UNTIL=20250430T020000Z",
        "20240201T020000",
        "RDATE:20240820T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/DaySets").unwrap();
    assert_eq!(table.rules().len(), 4);
    assert_eq!(meter.vtimezone_observances(), 5);
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    for observance in table.rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 2, 1), clock)
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(3600).unwrap()
        );
        assert_eq!(observance.rule().unwrap().at(), clock);
        assert_eq!(
            observance.rule().unwrap().through(),
            Some(date(2025, 4, 30))
        );
        assert_eq!(observance.transition_in(2023), None);
        assert_eq!(observance.transition_in(2026), None);
    }
    let explicit: Vec<_> = table
        .observances()
        .iter()
        .filter(|observance| observance.rule().is_none())
        .map(|observance| observance.start())
        .collect();
    assert_eq!(explicit, [CivilDateTime::new(date(2024, 8, 20), clock)]);
    for year in [2024, 2025] {
        assert_eq!(
            compiled_dates(&zones, year),
            expected_dates(year, &[2, 4], &[1, -1], None)
        );
    }
    assert!(compiled_dates(&zones, 2026).is_empty());
    assert_eq!(table.coverage_end(), Some(date(2025, 4, 30)));
}

#[test]
fn day_sets_charge_each_stored_pattern_to_the_table_limit() {
    for cap in [0_u32, 1, 3, 4, 5, 6] {
        let (zones, meter, diagnostics) = read(
            "BYMONTH=2,4;BYMONTHDAY=1,-1",
            "20240201T020000",
            "RDATE:20240820T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/DaySets").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(5), "cap={cap}");
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(5)).unwrap()
        );
        assert_eq!(table.is_truncated(), cap < 5, "cap={cap}");
        let codes: Vec<_> = diagnostics.iter().copied().map(Diagnostic::code).collect();
        if cap < 5 {
            assert_eq!(codes, [DiagnosticCode::VtimezoneObservancesTruncated]);
            assert_eq!(diagnostics[0].severity(), Severity::LimitReached);
        } else {
            assert!(codes.is_empty(), "cap={cap}: {codes:?}");
        }
    }
}

#[test]
fn malformed_or_repeated_month_day_parts_keep_one_anchor() {
    for parts in [
        "BYMONTHDAY=",
        "BYMONTHDAY=0",
        "BYMONTHDAY=+0",
        "BYMONTHDAY=-0",
        "BYMONTHDAY=32",
        "BYMONTHDAY=-32",
        "BYMONTHDAY=001",
        "BYMONTHDAY=-003",
        "BYMONTHDAY=1,",
        "BYMONTHDAY=,1",
        "BYMONTHDAY=1,,2",
        "BYMONTHDAY=1,x",
        "BYMONTHDAY=1,0",
        "BYMONTHDAY=1;BYMONTHDAY=15",
        "BYMONTHDAY=1;BYMONTHDAY=1",
        "BYMONTHDAY=-1,1;BYMONTHDAY=1",
        "BYMONTHDAY=1;BYMONTHDAY=2,3,4,5,6,7;BYDAY=MO",
    ] {
        let (zones, meter, diagnostics) = read(parts, "20240101T020000", "", Limits::DEFAULT);
        let table = zones.table("Test/DaySets").unwrap();
        assert!(table.rules().is_empty(), "{parts}");
        assert_eq!(table.observances().len(), 1, "{parts}");
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
fn sets_without_any_possible_date_keep_the_original_anchor() {
    let (zones, meter, diagnostics) = read(
        "BYMONTH=2;BYMONTHDAY=30,31",
        "20240101T020000",
        "",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/DaySets").unwrap();
    assert_eq!(table.rules(), []);
    assert_eq!(table.observances().len(), 1);
    assert_eq!(meter.vtimezone_observances(), 1);
    assert_eq!(
        table.observances()[0].start(),
        CivilDateTime::new(date(2024, 1, 1), CivilTime::from_hms(2, 0, 0).unwrap())
    );
}

#[test]
fn repeated_window_values_keep_the_compact_weekday_pattern() {
    for (written, days) in [
        ("31,25,26,27,28,29,30,25,31", (25..=31).collect::<Vec<_>>()),
        ("-1,-2,-3,-4,-5,-6,-7,-1", (-7..=-1).collect()),
    ] {
        let zones = admitted(&format!("BYMONTH=3;BYDAY=SU;BYMONTHDAY={written}"));
        assert_eq!(zones.table("Test/DaySets").unwrap().rules().len(), 1);
        for year in [0, 1900, 2000, 2024, 9999] {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, &[3], &days, Some(Weekday::Sunday))
            );
        }
    }
}
