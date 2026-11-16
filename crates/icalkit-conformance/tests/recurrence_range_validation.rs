// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ranges follow [RFC 5545 section 3.2.13](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.13).
//! Cardinality follows [section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ranges//EN\r\n\
         BEGIN:{kind}\r\nUID:range@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.validate().unwrap().to_bytes(), source.as_bytes());
}

fn rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid recurrence range {source:?}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error.issues().iter().any(|issue| {
            issue.code().as_str() == "icalkit.validation.invalid-recurrence-range"
        })
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn recurrence_ranges_require_the_defined_unquoted_value() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for parameter in [
            "RANGE=THISANDPRIOR",
            "RANGE=THISONLY",
            "RANGE=UNKNOWN",
            "RANGE=X-FUTURE",
            "RANGE=",
            "RANGE",
            "RANGE=\"THISANDFUTURE\"",
            "RANGE=THISANDFUTURE,THISANDFUTURE",
            "RANGE=\"THISANDFUTURE,THISANDFUTURE\"",
            "RANGE=THISANDFUTURE ",
            "RANGE=THISAND^nFUTURE",
            "RANGE=\u{ff34}HISANDFUTURE",
        ] {
            rejected(&calendar(
                kind,
                &format!("RECURRENCE-ID;{parameter}:20240102T090000Z"),
            ));
        }
    }
}

#[test]
fn recurrence_ranges_occur_at_most_once() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for parameters in [
            "RANGE=THISANDFUTURE;RANGE=THISANDFUTURE",
            "RANGE=UNKNOWN;raNGe=THISANDFUTURE",
            "RANGE=THISANDFUTURE;range=UNKNOWN",
        ] {
            rejected(&calendar(
                kind,
                &format!("RECURRENCE-ID;{parameters}:\r\n 20240102T090000Z"),
            ));
        }
    }
}

#[test]
fn absent_ranges_and_defined_spellings_keep_their_bytes() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for property in [
            "RECURRENCE-ID:20240102T090000Z",
            "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z",
            "RECURRENCE-ID;raNgE=thisandfuture:20240102T090000Z",
            "RECURRENCE-ID;VALUE=DATE-TIME;RANGE=ThisAndFuture:\r\n 20240102T090000Z",
            "RECURRENCE-ID;RANGE=THISANDFUTURE;\r\n X-SOURCE=one;X-SOURCE=two:20240102T090000Z",
        ] {
            accepted(&calendar(kind, property));
        }
    }
}

#[test]
fn date_only_recurrence_identifiers_use_the_same_range_rules() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        let date = |parameters: &str| {
            calendar(
                kind,
                &format!("RECURRENCE-ID;VALUE=DATE;{parameters}:20240102"),
            )
            .replace("DTSTART:20240102T090000Z", "DTSTART;VALUE=DATE:20240102")
        };
        accepted(&date("RANGE=THISANDFUTURE"));
        rejected(&date("RANGE=THISANDPRIOR"));
        rejected(&date("RANGE=THISANDFUTURE;range=THISANDFUTURE"));
    }
}

#[test]
fn recurrence_range_checks_use_unfolded_names_and_values() {
    accepted(&calendar(
        "VEVENT",
        "RECURRENCE-ID;RA\r\n NGE=THISAND\r\n FUTURE:20240102T090000Z",
    ));
    rejected(&calendar(
        "VEVENT",
        "RECURRENCE-ID;RA\r\n NGE=THISAND\r\n PRIOR:20240102T090000Z",
    ));
}

#[test]
fn opaque_recurrence_metadata_stays_uninterpreted() {
    accepted(&calendar(
        "VEVENT",
        "RECURRENCE-ID;VALUE=X-FUTURE;RANGE=UNKNOWN;RANGE=OTHER:opaque",
    ));
    accepted(&calendar(
        "VEVENT",
        "X-RECURRENCE-ID;RANGE=UNKNOWN;RANGE=OTHER:opaque",
    ));
    accepted(&calendar(
        "VEVENT",
        "RECURRENCE-ID;X-RANGE=UNKNOWN:20240102T090000Z",
    ));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque ranges//EN\r\n",
        "BEGIN:X-EXTENSION\r\nRECURRENCE-ID;RANGE=UNKNOWN;RANGE=OTHER:opaque\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
