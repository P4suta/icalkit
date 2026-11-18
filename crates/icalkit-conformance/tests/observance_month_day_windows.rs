// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Month-day intersections follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Date-set enumeration checks compiled patterns separately from observance anchors.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{Observance, YearlyRule, read_calendar_zones};

const POSITIVE_FINAL_WEEK: &str = "25,26,27,28,29,30,31";
const NEGATIVE_FINAL_WEEK: &str = "-7,-6,-5,-4,-3,-2,-1";

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn read(parts: &str, start: &str) -> (Observance, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month day windows//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/MonthDays\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{parts}\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    let [observance] = zones.table("Test/MonthDays").unwrap().observances() else {
        panic!("expected one observance for {parts}");
    };
    (*observance, diagnostics)
}

fn admitted(parts: &str) -> YearlyRule {
    let (observance, diagnostics) = read(parts, "20240225T020000");
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    observance.rule().unwrap()
}

#[test]
fn a_positive_month_end_window_can_have_no_february_onset() {
    let rule = admitted(&format!(
        "BYMONTH=2;BYDAY=SU;BYMONTHDAY={POSITIVE_FINAL_WEEK}"
    ));
    assert_eq!(rule.occurrence_in(2024), Some(date(2024, 2, 25)));
    assert_eq!(rule.occurrence_in(2027), Some(date(2027, 2, 28)));
    assert_eq!(rule.occurrence_in(2026), None);
}

#[test]
fn positive_windows_match_independent_date_set_membership() {
    for month in 1..=12 {
        for weekday in Weekday::ALL {
            let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
            for first in 1_u8..=25 {
                let days: Vec<_> = (first..=first.checked_add(6).unwrap()).collect();
                let written = days.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
                let parts = format!("BYMONTH={month};BYDAY={name};BYMONTHDAY={written}");
                let rule = admitted(&parts);
                for year in [0, 1, 1900, 2000, 2023, 2024, 2026, 2100, 9999] {
                    let expected = (1..=31)
                        .filter_map(|day| CivilDate::from_ymd(year, month, day))
                        .find(|value| {
                            days.contains(&value.day()) && value.weekday() == Some(weekday)
                        });
                    assert_eq!(rule.occurrence_in(year), expected, "{parts}, year={year}");
                }
            }
        }
    }
}

#[test]
fn reordered_month_days_preserve_the_same_intersection() {
    for days in [
        POSITIVE_FINAL_WEEK,
        "31,25,30,26,29,27,28",
        "+31,+30,+29,+28,+27,+26,+25",
    ] {
        let rule = admitted(&format!("BYMONTH=2;BYDAY=SU;BYMONTHDAY={days}"));
        assert_eq!(rule.occurrence_in(2024), Some(date(2024, 2, 25)), "{days}");
        assert_eq!(rule.occurrence_in(2026), None, "{days}");
    }
}

#[test]
fn negative_final_week_windows_still_name_the_last_weekday() {
    for month in 1..=12 {
        for weekday in Weekday::ALL {
            let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
            let parts = format!("BYMONTH={month};BYDAY={name};BYMONTHDAY={NEGATIVE_FINAL_WEEK}");
            let rule = admitted(&parts);
            for year in [0, 1900, 2000, 2024, 2026, 9999] {
                let expected = (1..=31)
                    .filter_map(|day| CivilDate::from_ymd(year, month, day))
                    .rfind(|value| value.weekday() == Some(weekday));
                assert_eq!(rule.occurrence_in(year), expected, "{parts}, year={year}");
            }
        }
    }
}

#[test]
fn unsupported_windows_keep_the_anchor_and_diagnostic() {
    for days in [
        "25,26,27,28,29,32",
        "24,25,26,27,28,29,30,31,32",
        "24,26,27,28,29,30,0",
        "25,26,27,28,29,30,",
        "-8,-7,-6,-5,-4,-3,-32",
        "-1,25,26,27,28,29,001",
    ] {
        let parts = format!("BYMONTH=2;BYDAY=SU;BYMONTHDAY={days}");
        let (observance, diagnostics) = read(&parts, "20240225T020000");
        assert!(observance.rule().is_none(), "{parts}");
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported],
            "{parts}"
        );
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 2, 25), CivilTime::from_hms(2, 0, 0).unwrap())
        );
    }
}

#[test]
fn windows_preserve_clocks_anchors_offsets_and_end_bounds() {
    let parts =
        format!("BYMONTH=2;BYDAY=SU;BYMONTHDAY={POSITIVE_FINAL_WEEK};UNTIL=20270228T020000Z");
    let (observance, diagnostics) = read(&parts, "20240225T020000");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let rule = observance.rule().unwrap();
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    assert_eq!(rule.at(), clock);
    assert_eq!(rule.through(), Some(date(2027, 2, 28)));
    assert_eq!(observance.offset_from(), UtcOffset::UTC);
    assert_eq!(
        observance.offset_to(),
        UtcOffset::from_seconds(3600).unwrap()
    );
    assert_eq!(observance.transition_in(2023), None);
    assert_eq!(observance.transition_in(2024), Some(observance.start()));
    assert_eq!(
        observance.transition_in(2027),
        Some(CivilDateTime::new(date(2027, 2, 28), clock))
    );
    assert_eq!(observance.transition_in(2028), None);
    assert_eq!(observance.transition_in(2026), None);
}
