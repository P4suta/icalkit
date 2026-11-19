// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resource admission refuses work beyond a fully spent integer ceiling.

#![cfg(test)]

use icalkit_conformance::internal::core::{DiagnosticCode, Document, Limits, Meter};
use icalkit_conformance::internal::tz::read_calendar_zones;

#[test]
fn a_spent_wide_observance_budget_cannot_admit_another_transition() {
    let input = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit budget overflow//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/Overflow\r\nBEGIN:STANDARD\r\nDTSTART:20240101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
    let document = Document::parse(input, Limits::DEFAULT, &mut Vec::new()).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(u32::MAX));
    meter.try_charge_vtimezone_observances(u32::MAX).unwrap();
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    let table = &zones.tables()[0];
    assert_eq!(table.observances(), []);
    assert!(table.is_truncated());
    assert_eq!(meter.vtimezone_observances(), u32::MAX);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneObservancesTruncated
    );
}
