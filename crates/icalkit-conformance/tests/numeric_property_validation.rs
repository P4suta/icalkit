// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integer syntax follows [RFC 5545 section 3.3.8](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.8).
//! Bounds follow [sections 3.8.1.8 and 3.8.1.9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.8) and the revision counter in [section 3.8.7.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.7.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit numeric validation//EN\r\n\
         BEGIN:{kind}\r\nUID:numeric@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &str) {
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.validate().unwrap().to_bytes(), source.as_bytes());
}

fn rejected(source: &str) {
    let error = Calendar::parse(source.as_bytes()).expect_err("invalid numeric property");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn priorities_reject_values_outside_the_specified_range() {
    for kind in ["VEVENT", "VTODO"] {
        for value in ["-1", "10", "2147483647"] {
            rejected(&calendar(kind, &format!("PRIORITY:{value}")));
        }
    }
}

#[test]
fn completion_percentages_reject_values_outside_the_specified_range() {
    for value in ["-1", "101", "2147483647"] {
        rejected(&calendar("VTODO", &format!("PERCENT-COMPLETE:{value}")));
    }
}

#[test]
fn revision_counters_reject_negative_values() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        rejected(&calendar(kind, "SEQUENCE:-1"));
    }
}

#[test]
fn numeric_properties_require_one_signed_decimal_integer() {
    for (kind, name) in [
        ("VEVENT", "PRIORITY"),
        ("VTODO", "PERCENT-COMPLETE"),
        ("VJOURNAL", "SEQUENCE"),
    ] {
        for value in [
            "",
            "abc",
            "1.0",
            "1e3",
            "1,2",
            " 1",
            "1 ",
            "2147483648",
            "-2147483649",
            "\u{0661}",
        ] {
            rejected(&calendar(kind, &format!("{name}:{value}")));
        }
    }
}

#[test]
fn numeric_boundaries_and_signed_forms_keep_their_bytes() {
    for value in ["0", "-0", "+9", "0005"] {
        accepted(&calendar("VEVENT", &format!("PRIORITY:{value}")));
    }
    for value in ["0", "-0", "+100", "0039"] {
        accepted(&calendar("VTODO", &format!("PERCENT-COMPLETE:{value}")));
    }
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for value in ["0", "-0", "+1", "2147483647"] {
            accepted(&calendar(kind, &format!("SEQUENCE:{value}")));
        }
    }
}

#[test]
fn explicit_integer_types_and_property_name_casing_remain_supported() {
    accepted(&calendar("VEVENT", "pRiOrItY;VaLuE=\"INTEGER\":+0005"));
    accepted(&calendar("VTODO", "PERCENT-COMPLETE;VALUE=INTEGER:100"));
    accepted(&calendar("VJOURNAL", "SEQUENCE;VALUE=INTEGER:2"));
}

#[test]
fn known_alternative_value_types_are_rejected() {
    for (kind, name) in [
        ("VEVENT", "PRIORITY"),
        ("VTODO", "PERCENT-COMPLETE"),
        ("VJOURNAL", "SEQUENCE"),
    ] {
        for value_type in ["FLOAT", "TEXT", "DATE-TIME"] {
            rejected(&calendar(kind, &format!("{name};VALUE={value_type}:1")));
        }
    }
}

#[test]
fn numeric_properties_reject_repeated_value_parameters() {
    for (kind, name) in [
        ("VEVENT", "PRIORITY"),
        ("VTODO", "PERCENT-COMPLETE"),
        ("VJOURNAL", "SEQUENCE"),
    ] {
        for parameters in [
            "VALUE=INTEGER;VALUE=INTEGER",
            "VALUE=X-FUTURE;VALUE=INTEGER",
        ] {
            rejected(&calendar(kind, &format!("{name};{parameters}:1")));
        }
    }
}

#[test]
fn unknown_numeric_value_types_remain_opaque() {
    for (kind, name) in [
        ("VEVENT", "PRIORITY"),
        ("VTODO", "PERCENT-COMPLETE"),
        ("VJOURNAL", "SEQUENCE"),
    ] {
        accepted(&calendar(kind, &format!("{name};VALUE=X-FUTURE:opaque")));
    }
}

#[test]
fn unrecognized_components_and_properties_keep_their_content() {
    accepted(&calendar("VEVENT", "X-PRIORITY:999"));
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque numeric//EN\r\n",
        "BEGIN:X-EXTENSION\r\nPRIORITY:999\r\nPERCENT-COMPLETE:-1\r\nSEQUENCE:opaque\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    );
    accepted(source);
}
