// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Contextual validation follows [RFC 5545 sections 3.6.1 and 3.6.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.1) and [section 3.8.2.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, properties: &str, calendar_properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit validation//EN\r\n\
         BEGIN:{kind}\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         {properties}END:{kind}\r\n{calendar_properties}END:VCALENDAR\r\n"
    )
}

fn assert_valid(source: &str) {
    let parsed = Calendar::parse(source.as_bytes()).expect("valid RFC 5545 input");
    assert_eq!(parsed.to_bytes(), source.as_bytes());
}

fn assert_rejected(source: &str, code: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid RFC 5545 input: {source}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error() && issue.code().as_str() == code),
        "missing {code}: {error:?}"
    );
    let imported = Import::read(source.as_bytes()).expect("lossless import");
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn an_event_requires_start_when_its_calendar_has_no_method() {
    assert_rejected(&calendar("VEVENT", "", ""), "missing-required-property");
    assert_valid(&calendar("VEVENT", "DTSTART:20260814T090000Z\r\n", ""));
    assert_valid(&calendar("VEVENT", "dtstart;value=date:20260814\r\n", ""));
}

#[test]
fn a_method_after_the_event_allows_omitting_its_start() {
    for method in ["METHOD:CANCEL\r\n", "mEtHoD:CANCEL\r\n"] {
        assert_valid(&calendar(
            "VEVENT",
            "ORGANIZER:mailto:organizer@example.test\r\n",
            method,
        ));
    }
}

#[test]
fn a_method_inside_an_extension_does_not_apply_to_its_siblings() {
    assert_rejected(
        &calendar(
            "VEVENT",
            "",
            "BEGIN:X-EXTENSION\r\nMETHOD:CANCEL\r\nEND:X-EXTENSION\r\n",
        ),
        "missing-required-property",
    );
}

#[test]
fn a_todo_with_duration_requires_start() {
    assert_rejected(
        &calendar("VTODO", "DURATION:P1D\r\n", ""),
        "missing-required-property",
    );
    assert_valid(&calendar(
        "VTODO",
        "DTSTART;VALUE=DATE:20260814\r\nDURATION:P1D\r\n",
        "",
    ));
}

#[test]
fn a_todo_or_journal_without_a_rule_can_omit_start() {
    assert_valid(&calendar("VTODO", "", ""));
    assert_valid(&calendar("VTODO", "DUE;VALUE=DATE:20260814\r\n", ""));
    assert_valid(&calendar("VJOURNAL", "", ""));
}

#[test]
fn every_recurring_component_requires_start_even_when_method_is_present() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        assert_rejected(
            &calendar(kind, "RRULE:FREQ=DAILY;COUNT=2\r\n", "METHOD:PUBLISH\r\n"),
            "missing-required-property",
        );
        assert_valid(&calendar(
            kind,
            "DTSTART;VALUE=DATE:20260814\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
            "METHOD:PUBLISH\r\n",
        ));
    }
}
