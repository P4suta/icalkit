// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Observance ordinals follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Month and year scope follow [verified erratum 1913](https://errata.rfc-editor.org/eid1913).
//! Pattern dates and start anchors have separate assertions.

#![cfg(test)]

use std::collections::BTreeMap;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{Observance, YearlyRule, read_calendar_zones};

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn read(parts: &str, start: &str) -> (Observance, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ordinal patterns//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Ordinal\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{parts}\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let calendar = document.components().next().unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(calendar, &mut meter, &mut diagnostics);
    let observance = *zones
        .table("Test/Ordinal")
        .unwrap()
        .observances()
        .first()
        .unwrap();
    (observance, diagnostics)
}

fn admitted(parts: &str, start: &str) -> YearlyRule {
    let (observance, diagnostics) = read(parts, start);
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    observance.rule().unwrap()
}

#[test]
fn a_years_fifth_sunday_can_cross_into_february() {
    let rule = admitted("BYDAY=5SU", "19950129T020000");
    for (year, month, day) in [
        (1995, 1, 29),
        (1996, 2, 4),
        (1997, 2, 2),
        (1998, 2, 1),
        (1999, 1, 31),
    ] {
        assert_eq!(rule.occurrence_in(year), Some(date(year, month, day)));
    }
}

#[test]
fn the_rfcs_twentieth_monday_counts_the_whole_year() {
    let rule = admitted("BYDAY=20MO", "19970519T020000");
    for (year, day) in [(1997, 19), (1998, 18), (1999, 17)] {
        assert_eq!(rule.occurrence_in(year), Some(date(year, 5, day)));
    }
}

#[test]
fn negative_month_ordinals_count_from_the_months_end() {
    for (ordinal, day) in [(-1, 31), (-2, 24), (-3, 17), (-4, 10), (-5, 3)] {
        let parts = format!("BYMONTH=3;BYDAY={ordinal}SU");
        let rule = admitted(&parts, &format!("202403{day:02}T020000"));
        assert_eq!(rule.occurrence_in(2024), Some(date(2024, 3, day)));
    }
    assert_eq!(
        admitted("BYMONTH=2;BYDAY=-5SU", "20150201T020000").occurrence_in(2024),
        None
    );
}

fn enumerate(year: u16, month: Option<u8>, weekday: Weekday) -> Vec<CivilDate> {
    let mut dates = Vec::new();
    for month in month.unwrap_or(1)..=month.unwrap_or(12) {
        for day in 1..=CivilDate::days_in_month(year, month).unwrap() {
            let value = date(year, month, day);
            if value.weekday() == Some(weekday) {
                dates.push(value);
            }
        }
    }
    dates
}

#[test]
fn signed_ordinals_match_independent_calendar_enumeration() {
    let mut calendar: BTreeMap<(u16, Option<u8>, u8), Vec<CivilDate>> = BTreeMap::new();
    for month in std::iter::once(None).chain((1..=12).map(Some)) {
        for weekday in Weekday::ALL {
            let name = std::str::from_utf8(weekday.as_bytes()).unwrap();
            for magnitude in 1_i8..=53 {
                for ordinal in [magnitude, magnitude.checked_neg().unwrap()] {
                    let prefix =
                        month.map_or_else(String::new, |value| format!("BYMONTH={value};"));
                    let parts = format!("{prefix}BYDAY={ordinal}{name}");
                    let rule = admitted(&parts, "20000101T020000");
                    for year in [0, 1, 1900, 2000, 2023, 2024, 2100, 9999] {
                        let dates = calendar
                            .entry((year, month, weekday.index()))
                            .or_insert_with(|| enumerate(year, month, weekday));
                        let position = if ordinal.is_positive() {
                            usize::from(ordinal.unsigned_abs()).checked_sub(1)
                        } else {
                            dates.len().checked_sub(usize::from(ordinal.unsigned_abs()))
                        };
                        let expected = position.and_then(|index| dates.get(index)).copied();
                        assert_eq!(rule.occurrence_in(year), expected, "{parts}, year={year}");
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_ordinals_keep_the_anchor_and_diagnostic() {
    for parts in [
        "BYDAY=0SU",
        "BYDAY=-0SU",
        "BYDAY=54SU",
        "BYDAY=-54SU",
        "BYMONTH=3;BYDAY=54SU",
        "BYMONTH=3;BYDAY=-54SU",
        "BYMONTH=3;BYDAY=-2SU,-54SU",
        "BYMONTH=3;BYDAY=-2SU;COUNT=0",
    ] {
        let (observance, diagnostics) = read(parts, "20200322T020000");
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
            CivilDateTime::new(date(2020, 3, 22), CivilTime::from_hms(2, 0, 0).unwrap())
        );
    }
}

#[test]
fn yearly_ordinals_preserve_clocks_anchors_and_until() {
    let (observance, diagnostics) = read("BYDAY=20MO;UNTIL=19980518T020000Z", "19970519T020000");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let rule = observance.rule().unwrap();
    assert_eq!(rule.at(), CivilTime::from_hms(2, 0, 0).unwrap());
    assert_eq!(rule.through(), Some(date(1998, 5, 18)));
    assert_eq!(rule.occurrence_in(1999), None);
    assert_eq!(observance.transition_in(1996), None);
    assert_eq!(
        observance.transition_in(1997),
        Some(CivilDateTime::new(date(1997, 5, 19), rule.at()))
    );
    assert_eq!(
        observance.transition_in(1998),
        Some(CivilDateTime::new(date(1998, 5, 18), rule.at()))
    );
    assert_eq!(observance.offset_from(), UtcOffset::UTC);
    assert_eq!(
        observance.offset_to(),
        UtcOffset::from_seconds(3600).unwrap()
    );
}
