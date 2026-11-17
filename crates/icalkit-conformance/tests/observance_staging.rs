// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Staging keeps the same earliest observances as complete table admission.

#![cfg(test)]

use icalkit_conformance::internal::core::{Diagnostic, DiagnosticCode, Document, Limits, Meter};
use icalkit_conformance::internal::tz::{
    ObservanceReader, TransitionTable, VtimezoneSet, read_calendar_zones,
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
        let tzid = zone
            .read_vtimezone(&mut meter, &mut diagnostics, &mut out)
            .unwrap();
        assert!(
            out.len() <= usize::try_from(limit - spent).unwrap() + 1,
            "{} staged observances",
            out.len()
        );
        assert_eq!(meter.vtimezone_observances(), spent);
        assert_eq!(diagnostics.as_slice(), []);
        let table = TransitionTable::new(tzid, out, &mut meter, &mut diagnostics);
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
