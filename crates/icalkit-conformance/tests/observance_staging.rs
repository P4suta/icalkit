// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Staging keeps the same earliest observances as complete table admission.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, Limits, Meter,
    UtcOffset,
};
use icalkit_conformance::internal::tz::{
    AnswerBasis, LocalResolution, ObservanceReader, TransitionTable, VtimezoneSet, ZoneSource,
    read_calendar_zones,
};

const STANDARD: &str = "BEGIN:STANDARD\r\nDTSTART:20240501T020000\r\n\
TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYMONTH=3,5;BYMONTHDAY=1,10\r\n\
RDATE:20241201T020000,20240101T020000,20240301T020000\r\nEND:STANDARD\r\n";

const DAYLIGHT: &str = "BEGIN:DAYLIGHT\r\nDTSTART:20240101T010000\r\n\
TZOFFSETFROM:-0200\r\nTZOFFSETTO:+0300\r\nEND:DAYLIGHT\r\n";

const TIED: &str = "BEGIN:STANDARD\r\nDTSTART:20240101T030000\r\n\
TZOFFSETFROM:+0300\r\nTZOFFSETTO:+0400\r\nEND:STANDARD\r\n";

fn document(observances: &[&str]) -> Document {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit staging tests//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Staging\r\n{}END:VTIMEZONE\r\nEND:VCALENDAR\r\n",
        observances.concat()
    );
    Document::parse(text.as_bytes(), Limits::GENEROUS, &mut Vec::new()).unwrap()
}

fn read(document: &Document, limit: u32, spent: u32) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(limit));
    meter.try_charge_vtimezone_observances(spent).unwrap();
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    (zones, meter, diagnostics)
}

#[test]
fn dropped_recurring_patterns_bound_later_coverage() {
    let standard = "BEGIN:STANDARD\r\nDTSTART:20240101T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nRRULE:FREQ=YEARLY;BYMONTH=1,7;BYMONTHDAY=1\r\nEND:STANDARD\r\n";
    let daylight = "BEGIN:DAYLIGHT\r\nDTSTART:20240401T020000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;BYMONTH=4,10;BYMONTHDAY=1\r\nEND:DAYLIGHT\r\n";
    let source = document(&[standard, daylight]);
    let november = CivilDateTime::new(
        CivilDate::from_ymd(2024, 11, 1).unwrap(),
        CivilTime::from_hms(12, 0, 0).unwrap(),
    );
    let (complete, _, _) = read(&source, 4, 0);
    let (bounded, _, _) = read(&source, 3, 0);
    assert!(bounded.tables()[0].is_truncated());
    assert_eq!(
        complete.tables()[0]
            .resolve("Test/Staging", november)
            .unwrap()
            .basis,
        AnswerBasis::Computed
    );
    assert!(matches!(
        bounded.tables()[0]
            .resolve("Test/Staging", november)
            .unwrap()
            .basis,
        AnswerBasis::BeyondKnownTransitions(_)
    ));
}

#[test]
fn upper_civil_boundary_retains_generated_local_gaps() {
    let daylight = "BEGIN:DAYLIGHT\r\nDTSTART:99950106T020000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;BYWEEKNO=1;BYDAY=FR;WKST=WE\r\nEND:DAYLIGHT\r\n";
    let standard = "BEGIN:STANDARD\r\nDTSTART:99991201T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nEND:STANDARD\r\n";
    let (zones, _, diagnostics) = read(&document(&[daylight, standard]), 16, 0);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let date = CivilDate::from_ymd(9999, 12, 31).unwrap();
    let local = CivilDateTime::new(date, CivilTime::from_hms(2, 30, 0).unwrap());
    let answer = zones.tables()[0].resolve("Test/Staging", local).unwrap();
    assert!(
        matches!(answer.resolution, LocalResolution::Nonexistent { .. }),
        "{answer:?}"
    );
}

#[test]
fn lower_civil_boundary_uses_the_local_year_for_offset_lookup() {
    let daylight = "BEGIN:DAYLIGHT\r\nDTSTART:00000101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nRRULE:FREQ=YEARLY;BYWEEKNO=-1;BYDAY=SA;BYMINUTE=0,45\r\nEND:DAYLIGHT\r\n";
    let standard = "BEGIN:STANDARD\r\nDTSTART:00000101T013000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n";
    let (zones, _, diagnostics) = read(&document(&[daylight, standard]), 16, 0);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let date = CivilDate::from_ymd(0, 1, 1).unwrap();
    let offset = UtcOffset::from_seconds(3600).unwrap();
    for (minute, expected) in [(40, 3600), (50, 7200)] {
        let local = CivilDateTime::new(date, CivilTime::from_hms(0, minute, 0).unwrap());
        let answer = zones.tables()[0]
            .offset_at("Test/Staging", local.at_offset(offset).unwrap())
            .unwrap();
        assert_eq!(answer.offset.seconds(), expected);
    }
}

#[test]
fn bounded_admission_matches_complete_sorting_for_every_source_order() {
    let source = document(&[STANDARD, DAYLIGHT, TIED]);
    let (complete, _, diagnostics) = read(&source, 100, 0);
    assert_eq!(diagnostics.as_slice(), []);
    let full = complete.tables()[0].observances();
    for order in [
        [STANDARD, DAYLIGHT, TIED],
        [STANDARD, TIED, DAYLIGHT],
        [DAYLIGHT, STANDARD, TIED],
        [DAYLIGHT, TIED, STANDARD],
        [TIED, STANDARD, DAYLIGHT],
        [TIED, DAYLIGHT, STANDARD],
    ] {
        let source = document(&order);
        for limit in 0..=10 {
            for spent in 0..=limit {
                let (zones, meter, diagnostics) = read(&source, limit, spent);
                let table = &zones.tables()[0];
                let kept = usize::try_from(limit - spent).unwrap().min(full.len());
                if kept == 1 {
                    let earliest = &table.observances()[0];
                    assert_eq!(
                        earliest.start().date(),
                        CivilDate::from_ymd(2024, 1, 1).unwrap()
                    );
                    assert_eq!(
                        earliest.start().time(),
                        CivilTime::from_hms(2, 0, 0).unwrap()
                    );
                    assert_eq!(earliest.offset_from().seconds(), 7200);
                }
                assert_eq!(table.observances(), &full[..kept]);
                assert_eq!(table.is_truncated(), kept < full.len());
                assert_eq!(
                    meter.vtimezone_observances(),
                    spent + u32::try_from(kept).unwrap()
                );
                assert_eq!(
                    diagnostics.len(),
                    usize::from(kept < full.len()),
                    "{diagnostics:?}"
                );
                if kept < full.len() {
                    assert_eq!(
                        diagnostics[0].code(),
                        DiagnosticCode::VtimezoneObservancesTruncated
                    );
                }
            }
        }
    }
}

#[test]
fn raw_reader_keeps_only_the_remaining_budget_and_one_overflow_witness() {
    let source = document(&[STANDARD, DAYLIGHT, TIED]);
    let root = source.components().next().unwrap();
    let zone = root.components().next().unwrap();
    for (limit, spent) in [(0, 0), (1, 0), (2, 1), (5, 3)] {
        let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(limit));
        meter.try_charge_vtimezone_observances(spent).unwrap();
        let mut diagnostics = Vec::new();
        let mut out = Vec::new();
        let staged = zone
            .read_vtimezone(&mut meter, &mut diagnostics, &mut out)
            .unwrap();
        assert!(staged.exceeds_limit);
        assert!(
            out.len() <= usize::try_from(limit - spent).unwrap() + 1,
            "{} staged observances",
            out.len()
        );
        assert_eq!(meter.vtimezone_observances(), spent);
        assert_eq!(diagnostics.as_slice(), []);
        let table = TransitionTable::new(staged.tzid, out, &mut meter, &mut diagnostics);
        assert_eq!(
            table.observances().len(),
            usize::try_from(limit - spent).unwrap()
        );
        assert!(table.is_truncated());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code(),
            DiagnosticCode::VtimezoneObservancesTruncated
        );
    }
}

#[test]
fn unreadable_values_remain_distinct_from_valid_candidates_at_capacity() {
    let invalid = "BEGIN:STANDARD\r\nDTSTART:unreadable\r\n\
                   TZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n";
    let source = document(&[STANDARD, DAYLIGHT, invalid, TIED]);
    for limit in [0, 1, 2] {
        let (_, _, diagnostics) = read(&source, limit, 0);
        for code in [
            DiagnosticCode::VtimezoneObservanceUnreadable,
            DiagnosticCode::VtimezoneObservancesTruncated,
        ] {
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.code() == code)
                    .count(),
                1,
                "{diagnostics:?}"
            );
        }
        assert_eq!(diagnostics.len(), 2);
    }
}
