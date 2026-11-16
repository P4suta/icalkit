// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Geographic values follow [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.6).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit geographic validation//EN\r\n\
         BEGIN:{kind}\r\nUID:location@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
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
    let error = Calendar::parse(source.as_bytes()).expect_err("invalid geographic value");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn geographic_properties_require_two_decimal_floats() {
    for kind in ["VEVENT", "VTODO"] {
        for value in [
            "",
            "0",
            ";0",
            "0;",
            "0;0;0",
            "0,0",
            "0\\;0",
            " 0;0",
            "0;0 ",
            ".5;0",
            "0.;0",
            "1e1;0",
            "NaN;0",
            "0;inf",
            "0;\u{0661}",
        ] {
            rejected(&calendar(kind, &format!("GEO:{value}")));
        }
    }
}

#[test]
fn geographic_properties_reject_positions_outside_the_bounds() {
    for kind in ["VEVENT", "VTODO"] {
        for value in [
            "91;0",
            "-91;0",
            "0;181",
            "0;-181",
            "90.000001;0",
            "0;-180.000001",
            "99999;0",
        ] {
            rejected(&calendar(kind, &format!("GEO:{value}")));
        }
    }
}

#[test]
fn written_decimal_bounds_are_checked_without_binary_rounding() {
    for value in [
        "90.00000000000000000001;0",
        "-90.00000000000000000001;0",
        "0;180.00000000000000000001",
        "0;-180.00000000000000000001",
    ] {
        rejected(&calendar("VEVENT", &format!("GEO:{value}")));
    }
}

#[test]
fn geographic_boundaries_and_decimal_precision_keep_their_bytes() {
    for kind in ["VEVENT", "VTODO"] {
        for value in [
            "0;0",
            "+0;-0",
            "90;-180",
            "-90;180",
            "+90.000000;+180.000000",
            "37.386013;-122.082932",
            "89.99999999999999999999;-179.99999999999999999999",
            "000090.000000;000180.000000",
            "0.00000000000000000001;-0.00000000000000000001",
        ] {
            accepted(&calendar(kind, &format!("GEO:{value}")));
        }
    }
    accepted(&calendar(
        "VEVENT",
        "gEo;VaLuE=\"FLOAT\";X-SOURCE=example:37.386013;-122.082932",
    ));
}

#[test]
fn geographic_properties_reject_incompatible_and_repeated_value_types() {
    for parameters in [
        "VALUE=TEXT",
        "VALUE=INTEGER",
        "VALUE=URI",
        "VALUE=DATE-TIME",
        "VALUE=FLOAT;VALUE=FLOAT",
        "VALUE=X-FUTURE;VALUE=FLOAT",
    ] {
        rejected(&calendar("VEVENT", &format!("GEO;{parameters}:0;0")));
    }
}

#[test]
fn unknown_geographic_value_types_remain_opaque() {
    accepted(&calendar("VEVENT", "GEO;VALUE=X-FUTURE:opaque;coordinates"));
}

#[test]
fn unrecognized_components_and_properties_keep_their_geographic_content() {
    accepted(&calendar("VEVENT", "X-GEO:99999;invalid"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque geography//EN\r\n",
        "BEGIN:X-EXTENSION\r\nGEO:invalid\r\nEND:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
