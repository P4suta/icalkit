// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Text escaping follows [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(property: &str) -> String {
    let property = if property.is_empty() {
        String::new()
    } else {
        format!("{property}\r\n")
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit text validation//EN\r\n\
         BEGIN:VEVENT\r\nUID:text@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}END:VEVENT\r\nEND:VCALENDAR\r\n"
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
    let error = Calendar::parse(source.as_bytes()).expect_err("invalid known text");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "icalkit.validation.invalid-text")
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn ordinary_text_properties_require_escaped_delimiters() {
    for name in [
        "SUMMARY",
        "DESCRIPTION",
        "LOCATION",
        "COMMENT",
        "CONTACT",
        "RELATED-TO",
    ] {
        for value in ["a,b", "a;b", "a\\"] {
            rejected(&calendar(&format!("{name}:{value}")));
        }
    }
    for value in ["a,b", "a;b", "a\\"] {
        rejected(&calendar("").replace("UID:text@example.test", &format!("UID:{value}")));
    }
}

#[test]
fn text_properties_reject_undefined_escapes() {
    for value in ["a\\q", "a\\:", "a\\t", "a\\u1234", "a\\,b\\x"] {
        rejected(&calendar(&format!("SUMMARY:{value}")));
    }
}

#[test]
fn all_defined_escapes_and_plain_unicode_keep_their_bytes() {
    for value in [
        "",
        "plain",
        "a:b\"c",
        "a\\,b\\;c\\\\d\\ne\\Nf",
        "Unicode: \u{00e9} \u{65e5}\u{672c}\u{8a9e}",
    ] {
        accepted(&calendar(&format!("SUMMARY:{value}")));
    }
    accepted(&calendar(
        "sUmMaRy;VaLuE=\"TEXT\";X-SOURCE=example:kept\\, value",
    ));
}

#[test]
fn text_lists_distinguish_separators_from_escaped_delimiters() {
    for name in ["CATEGORIES", "RESOURCES"] {
        for value in ["a,b", "a\\,b,c\\;d", ",a,,b,", "a\\\\,b"] {
            accepted(&calendar(&format!("{name}:{value}")));
        }
        for value in ["a;b,c", "a,b\\q", "a,b\\"] {
            rejected(&calendar(&format!("{name}:{value}")));
        }
    }
}

#[test]
fn known_text_checks_declared_types_and_parameter_multiplicity() {
    for name in ["SUMMARY", "DESCRIPTION", "CATEGORIES", "RESOURCES"] {
        for parameters in [
            "VALUE=INTEGER",
            "VALUE=URI",
            "VALUE=DATE-TIME",
            "VALUE=TEXT;VALUE=TEXT",
            "VALUE=X-FUTURE;VALUE=TEXT",
        ] {
            rejected(&calendar(&format!("{name};{parameters}:1")));
        }
    }
}

#[test]
fn text_escapes_are_checked_after_unfolding() {
    accepted(&calendar("SUMMARY:before\\\r\n nafter"));
    accepted(&calendar("CATEGORIES:first\\\r\n ,second,third"));
    rejected(&calendar("SUMMARY:before\\\r\n qafter"));
    rejected(&calendar("SUMMARY:before\r\n ,after"));
}

#[test]
fn calendar_and_time_zone_text_use_the_same_validation() {
    rejected(&calendar("").replace(
        "PRODID:-//icalkit text validation//EN",
        "PRODID:invalid;product",
    ));
    for property in ["METHOD:PUBLISH\\q", "CALSCALE:GREGORIAN\\q"] {
        rejected(
            &calendar("").replace("VERSION:2.0\r\n", &format!("VERSION:2.0\r\n{property}\r\n")),
        );
    }
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit zone text//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n",
        "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n",
        "TZNAME:Local time\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    accepted(source);
    rejected(&source.replace("TZID:Test/Zone", "TZID:Test\\q"));
    rejected(&source.replace("TZNAME:Local time", "TZNAME:local;time"));
    rejected(&calendar(
        "BEGIN:VALARM\r\nACTION:X-SOUND\\q\r\nTRIGGER:-PT1H\r\nEND:VALARM",
    ));
}

#[test]
fn opaque_text_types_and_extensions_remain_uninterpreted() {
    for name in ["SUMMARY", "DESCRIPTION", "CATEGORIES", "RESOURCES"] {
        accepted(&calendar(&format!(
            "{name};VALUE=X-FUTURE:opaque;data,\\q\\"
        )));
    }
    accepted(&calendar("X-PAYLOAD:opaque;data,\\q\\"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque text//EN\r\n",
        "BEGIN:X-EXTENSION\r\nSUMMARY:opaque;data,\\q\\\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
