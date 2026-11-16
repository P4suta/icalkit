// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Request status values follow [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.8.3).
//! CalDAV uses the same value under [RFC 6638](https://www.rfc-editor.org/rfc/rfc6638.html#section-10.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::caldav::ScheduleDelivery;
use icalkit::interop::Import;

const KINDS: &[&str] = &["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"];

const INVALID_CODES: &[&str] = &[
    "",
    "2",
    "2.",
    ".0",
    "2..0",
    "2.0.0.0",
    "-2.0",
    "+2.0",
    "2e0.0",
    "2.0 ",
    " 2.0",
    "2.\u{0660}",
    "2,0",
    "2\\.0",
];

const INVALID_FIELDS: &[&str] = &[
    "2.0",
    "2.0;one;two;three",
    "2.0;bad,comma",
    "2.0;bad\\q",
    "2.0;bad\\",
    "2.0;good;bad,comma",
    "2.0;good;bad\\q",
    "2.0;good;bad\\",
];

const VALID_VALUES: &[&str] = &[
    "2.0;Success",
    "3.1.1;Invalid value;DTSTART:bad",
    "2.0;",
    "2.0;;",
    "2.0;;exception",
    "2.0;a\\;b\\,c\\\\d\\ne\\Nf;other",
    "2.0;a\\\\;extra",
    "2.0;a\\\\\\;b",
    "2.0;with\ttab",
    "2.0;quote\" and colon:",
    "2.0;Unicode \u{00e9} \u{65e5};more",
    "0002.000;Success",
    "184467440737095516160.184467440737095516160;wide",
    "1.2.184467440737095516160;wide",
];

fn calendar(kind: &str, properties: &str) -> String {
    let end = if kind == "VFREEBUSY" {
        "DTEND:20240101T100000Z\r\n"
    } else {
        ""
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit request status//EN\r\n\
         BEGIN:{kind}\r\nUID:status@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{end}{properties}\r\n\
         END:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.validate().unwrap().to_bytes(), source.as_bytes());
}

fn rejected(source: &[u8]) {
    let Err(error) = Calendar::parse(source) else {
        panic!(
            "accepted malformed request status: {}",
            String::from_utf8_lossy(source)
        );
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error()
                && issue.code().as_str() == "icalkit.validation.invalid-text"),
        "missing invalid-text diagnostic: {error:?}"
    );
    let imported = Import::read(source).unwrap();
    assert_eq!(imported.as_bytes(), source);
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source);
}

#[test]
fn calendar_status_codes_require_two_or_three_numeric_parts() {
    for kind in KINDS {
        for code in INVALID_CODES {
            rejected(calendar(kind, &format!("REQUEST-STATUS:{code};Invalid")).as_bytes());
        }
    }
}

#[test]
fn calendar_status_fields_require_valid_structure_and_text() {
    for kind in KINDS {
        for value in INVALID_FIELDS {
            rejected(calendar(kind, &format!("REQUEST-STATUS:{value}")).as_bytes());
        }
    }
}

#[test]
fn valid_calendar_status_values_preserve_numeric_and_text_spelling() {
    for kind in KINDS {
        for value in VALID_VALUES {
            accepted(&calendar(kind, &format!("REQUEST-STATUS:{value}")));
        }
    }
}

#[test]
fn calendar_status_type_and_parameter_cardinality_are_checked() {
    for properties in [
        "REQUEST-STATUS;VALUE=DATE:20240101",
        "REQUEST-STATUS;VALUE=INTEGER:2",
        "REQUEST-STATUS;VALUE=TEXT;VALUE=TEXT:2.0;Success",
        "REQUEST-STATUS;LANGUAGE=en;language=fr:2.0;Success",
    ] {
        rejected(calendar("VEVENT", properties).as_bytes());
    }
}

#[test]
fn calendar_status_parameters_folds_and_multiple_properties_keep_their_bytes() {
    accepted(&calendar(
        "VEVENT",
        concat!(
            "request-status;VaLuE=\"TEXT\";LANGUAGE=en;X-SOURCE=kept:2.0;Success\r\n",
            "REQUEST-STATUS:3.1;Invalid\\, long\r\n er;RRULE:FREQ=bad\\;BYDAY=MO",
        ),
    ));
}

#[test]
fn calendar_status_text_requires_utf8() {
    for properties in ["REQUEST-STATUS:2.0;X", "REQUEST-STATUS:2.0;valid;X"] {
        let mut source = calendar("VEVENT", properties).into_bytes();
        let position = source.iter().rposition(|octet| *octet == b'X').unwrap();
        *source.get_mut(position).unwrap() = 0xff;
        rejected(&source);
    }
}

#[test]
fn unknown_status_value_types_and_component_semantics_stay_opaque() {
    accepted(&calendar(
        "VEVENT",
        "REQUEST-STATUS;VALUE=X-FUTURE:opaque;data,\\q",
    ));
    accepted(&calendar(
        "VEVENT",
        "BEGIN:X-EXTENSION\r\nREQUEST-STATUS:opaque;data,\\q\r\nEND:X-EXTENSION",
    ));
    accepted(&calendar("VEVENT", "X-REQUEST-STATUS:opaque;data,\\q"));
}

#[test]
fn delivery_status_values_use_the_same_structured_grammar() {
    for value in VALID_VALUES {
        let delivery = ScheduleDelivery::new("mailto:user@example.test", *value).unwrap();
        assert_eq!(delivery.request_status(), *value);
    }
}

#[test]
fn delivery_status_values_reject_malformed_codes_and_text() {
    let malformed_codes: Vec<_> = INVALID_CODES
        .iter()
        .map(|code| format!("{code};Invalid"))
        .collect();
    for value in malformed_codes
        .iter()
        .map(String::as_str)
        .chain(INVALID_FIELDS.iter().copied())
    {
        let Err(error) = ScheduleDelivery::new("mailto:user@example.test", value) else {
            panic!("accepted malformed delivery status: {value:?}");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.caldav.schedule-delivery-invalid"
        );
    }
}

#[test]
fn delivery_success_uses_the_numeric_status_class() {
    for (value, success) in [
        ("2.0;Success", true),
        ("02.0;Success", true),
        ("0002.0.1;Success", true),
        ("20.0;Other class", false),
        ("3.7;Invalid user", false),
        ("000.2;Other class", false),
    ] {
        let delivery = ScheduleDelivery::new("mailto:user@example.test", value).unwrap();
        assert_eq!(delivery.is_success(), success, "{value}");
    }
}
