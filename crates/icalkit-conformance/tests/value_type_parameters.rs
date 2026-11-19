// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Value type declarations follow [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;
use icalkit_conformance::internal::core::{
    DateTimeValue, Document, Limits, Meter, Severity, ValueType,
};

fn document(property: &str) -> Document {
    let source = format!("BEGIN:VEVENT\r\n{property}\r\nEND:VEVENT\r\n");
    let document = Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    assert_eq!(document.to_bytes(), source.as_bytes());
    document
}

fn calendar(property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit value types//EN\r\n\
         BEGIN:VEVENT\r\nUID:types@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn rejected(source: &str) {
    let error = Calendar::parse(source.as_bytes()).expect_err("malformed value type declaration");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error()
                && issue.code().as_str() == "malformed-value-type-parameter")
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn malformed_value_type_tokens_have_a_distinct_diagnostic() {
    for parameters in [
        "VALUE",
        "VALUE=",
        "VALUE=\"\"",
        "VALUE=DATE,TEXT",
        "VALUE=\"DATE,TEXT\"",
        "VALUE=\"TEXT TYPE\"",
        "VALUE=TYPE/NAME",
        "VALUE=TYPE_NAME",
        "VALUE=TYPE.NAME",
        "VALUE=TYPE+NAME",
        "VALUE=TYPE^nNAME",
        "VALUE=TYPE\\NAME",
        "VALUE=\u{65e5}",
    ] {
        let document = document(&format!("DTSTART;{parameters}:20240101T090000Z"));
        let component = document.components().next().unwrap();
        let property = component.properties().next().unwrap();
        let code = property
            .declared_value_type()
            .unwrap()
            .expect_err("malformed declaration");
        assert_eq!(
            code.as_str(),
            "malformed-value-type-parameter",
            "{parameters}"
        );
        let view = property.value::<DateTimeValue<'_>>();
        assert!(!view.is_valid());
        assert_eq!(view.diagnostic().unwrap().severity(), Severity::Violation);
    }
}

#[test]
fn repeated_value_type_declarations_cant_grant_a_typed_value() {
    for parameters in [
        "VALUE=DATE-TIME;VALUE=DATE-TIME",
        "VALUE=DATE-TIME;VaLuE=DATE",
        "VALUE=X-FUTURE;VALUE=DATE-TIME",
        "VALUE=X-FUTURE;VALUE=X-FUTURE",
    ] {
        let document = document(&format!("DTSTART;{parameters}:20240101T090000Z"));
        let component = document.components().next().unwrap();
        let property = component.properties().next().unwrap();
        let view = property.value::<DateTimeValue<'_>>();
        assert!(!view.is_valid(), "{parameters}");
        let diagnostic = view.diagnostic().unwrap();
        assert_eq!(diagnostic.code().as_str(), "malformed-value-type-parameter");
        assert_eq!(diagnostic.severity(), Severity::Violation);
    }
}

#[test]
fn schema_audits_report_malformed_declarations_as_violations() {
    for parameters in ["VALUE=", "VALUE=DATE,TEXT", "VALUE=X-FUTURE;VALUE=TEXT"] {
        let source = calendar(&format!("SUMMARY;{parameters}:opaque"));
        let document =
            Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
        let component = document
            .components()
            .next()
            .unwrap()
            .components()
            .next()
            .unwrap();
        let mut diagnostics = Vec::new();
        component.audit(&mut Meter::new(Limits::DEFAULT), &mut diagnostics);
        let outcomes: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code().as_str(), diagnostic.severity()))
            .collect();
        assert_eq!(
            outcomes,
            [("malformed-value-type-parameter", Severity::Violation)]
        );
        assert_eq!(document.to_bytes(), source.as_bytes());
    }
}

#[test]
fn known_properties_reject_malformed_type_declarations_during_promotion() {
    for name in [
        "SUMMARY", "PRIORITY", "GEO", "ATTENDEE", "URL", "RRULE", "RDATE",
    ] {
        for parameters in [
            "VALUE=",
            "VALUE=\"\"",
            "VALUE=TYPE/NAME",
            "VALUE=DATE,TEXT",
            "VALUE=TEXT;VALUE=TEXT",
        ] {
            rejected(&calendar(&format!("{name};{parameters}:opaque")));
        }
    }
}

#[test]
fn calendar_metadata_uses_the_same_declared_type_boundary() {
    for parameters in ["VALUE=", "VALUE=TEXT;VALUE=TEXT", "VALUE=TYPE/NAME"] {
        rejected(
            &calendar("SUMMARY:Metadata")
                .replace("VERSION:2.0", &format!("VERSION;{parameters}:2.0")),
        );
    }
}

#[test]
fn strict_promotion_refuses_quoted_value_type_tokens() {
    for (quoted, valid) in [
        ("SUMMARY;VALUE=\"TEXT\":Value", "SUMMARY;VALUE=TEXT:Value"),
        (
            "SUMMARY;VALUE=\"X-FUTURE\":opaque",
            "SUMMARY;VALUE=X-FUTURE:opaque",
        ),
    ] {
        assert!(Calendar::parse(calendar(valid).as_bytes()).is_ok());
        rejected(&calendar(quoted));
    }
    let source = calendar("SUMMARY:Value").replace(
        "DTSTART:20240101T090000Z",
        "DTSTART;VALUE=\"DATE\":20240101",
    );
    assert!(Calendar::parse(source.replace("\"DATE\"", "DATE").as_bytes()).is_ok());
    rejected(&source);
}

#[test]
fn valid_unknown_tokens_remain_opaque_and_produce_notes() {
    for value_type in ["X-FUTURE", "FUTURE-TYPE-42", "future-type"] {
        let document = document(&format!("DTSTART;VALUE={value_type}:opaque"));
        let component = document.components().next().unwrap();
        let property = component.properties().next().unwrap();
        let code = property.declared_value_type().unwrap().unwrap_err();
        assert_eq!(code.as_str(), "unknown-value-type");
        let view = property.value::<DateTimeValue<'_>>();
        assert!(!view.is_valid());
        assert_eq!(view.diagnostic().unwrap().severity(), Severity::Note);
        for name in [
            "SUMMARY", "PRIORITY", "GEO", "ATTENDEE", "URL", "RRULE", "RDATE",
        ] {
            let source = calendar(&format!("{name};VALUE={value_type}:opaque"));
            let parsed = Calendar::parse(source.as_bytes()).unwrap();
            assert_eq!(parsed.to_bytes(), source.as_bytes());
            assert!(parsed.issues().iter().any(|issue| !issue.is_error() && issue.code().as_str() == "unknown-value-type"));
        }
    }
}

#[test]
fn recognized_and_default_types_keep_their_views_and_bytes() {
    for (property, expected) in [
        ("DTSTART:20240101T090000Z", None),
        (
            "dTsTaRt;VaLuE=\"date-time\":20240101T090000Z",
            Some(Ok(ValueType::DateTime)),
        ),
    ] {
        let document = document(property);
        let component = document.components().next().unwrap();
        let property = component.properties().next().unwrap();
        assert!(property.value::<DateTimeValue<'_>>().is_valid());
        assert_eq!(property.declared_value_type(), expected);
    }
}

#[test]
fn unrecognized_properties_and_components_stay_uninterpreted() {
    for property in [
        "X-PRODUCER;VALUE=:opaque",
        "X-PRODUCER;VALUE=TEXT,TEXT:opaque",
    ] {
        let source = calendar(property);
        assert_eq!(
            Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
            source.as_bytes()
        );
    }
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque types//EN\r\n",
        "BEGIN:X-EXTENSION\r\nDTSTART;VALUE=:opaque\r\nEND:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    );
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}
