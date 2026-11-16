// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Enumerated properties follow [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.11).
//! Task statuses include [RFC 10051](https://www.rfc-editor.org/rfc/rfc10051.html#section-11.2).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;
use icalkit::model::Name;

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit enumerations//EN\r\n\
         BEGIN:{kind}\r\nUID:enum@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn calendar_property(property: &str) -> String {
    calendar("VEVENT", "SUMMARY:Enumeration").replace(
        "BEGIN:VEVENT\r\n",
        &format!("{property}\r\nBEGIN:VEVENT\r\n"),
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

fn rejected(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes())
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.validation.failed"
    );
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn status_values_match_the_component_kind() {
    for (kind, invalid) in [
        ("VEVENT", ["COMPLETED", "DRAFT", "PENDING", "FAILED"]),
        ("VTODO", ["TENTATIVE", "CONFIRMED", "DRAFT", "FINAL"]),
        (
            "VJOURNAL",
            ["CONFIRMED", "NEEDS-ACTION", "PENDING", "FAILED"],
        ),
    ] {
        for value in invalid {
            rejected(&calendar(kind, &format!("STATUS:{value}")));
        }
        for value in ["", "INVALID", "X-FUTURE", "CANCELLED ", " CONFIRMED"] {
            rejected(&calendar(kind, &format!("STATUS:{value}")));
        }
    }
}

#[test]
fn registered_statuses_and_case_variants_keep_their_bytes() {
    let cases: &[(&str, &[&str])] = &[
        ("VEVENT", &["TENTATIVE", "CONFIRMED", "CANCELLED"]),
        (
            "VTODO",
            &[
                "NEEDS-ACTION",
                "COMPLETED",
                "IN-PROCESS",
                "CANCELLED",
                "PENDING",
                "FAILED",
            ],
        ),
        ("VJOURNAL", &["DRAFT", "FINAL", "CANCELLED"]),
    ];
    for (kind, values) in cases {
        for value in *values {
            accepted(&calendar(kind, &format!("STATUS:{value}")));
            accepted(&calendar(
                kind,
                &format!(
                    "sTaTuS;VaLuE=\"TEXT\";X-SOURCE=example:{}",
                    value.to_ascii_lowercase()
                ),
            ));
        }
    }
}

#[test]
fn transparency_uses_the_defined_vocabulary() {
    for value in [
        "",
        "FREE",
        "BUSY",
        "OPAQUE ",
        "OPAQUE,TRANSPARENT",
        "OPAQUE\\n",
    ] {
        rejected(&calendar("VEVENT", &format!("TRANSP:{value}")));
    }
    for value in ["OPAQUE", "TRANSPARENT", "opaque", "transparent"] {
        accepted(&calendar("VEVENT", &format!("TRANSP:{value}")));
    }
}

#[test]
fn calendar_scale_requires_the_defined_gregorian_value() {
    for value in ["", "JULIAN", "X-FUTURE", "GREGORIAN ", "GREGORIAN\\n"] {
        rejected(&calendar_property(&format!("CALSCALE:{value}")));
    }
    for value in ["GREGORIAN", "gregorian", "GrEgOrIaN"] {
        accepted(&calendar_property(&format!("CALSCALE:{value}")));
    }
}

#[test]
fn classification_and_method_values_require_tokens() {
    for value in [
        "",
        "NOT A TOKEN",
        "UNDER_SCORE",
        "DOT.NAME",
        "TYPE/NAME",
        "A\\,B",
        "A\\;B",
        "A+B",
        "\u{65e5}",
    ] {
        rejected(&calendar("VEVENT", &format!("CLASS:{value}")));
        rejected(&calendar_property(&format!("METHOD:{value}")));
    }
}

#[test]
fn extensible_vocabularies_keep_valid_tokens_and_parameters() {
    for value in [
        "PUBLIC",
        "PRIVATE",
        "CONFIDENTIAL",
        "X-FUTURE",
        "future-class",
    ] {
        accepted(&calendar(
            "VEVENT",
            &format!("cLaSs;VALUE=TEXT;X-SOURCE=example:{value}"),
        ));
    }
    for value in [
        "PUBLISH",
        "REQUEST",
        "CANCEL",
        "X-FUTURE",
        "future-method-42",
    ] {
        accepted(&calendar_property(&format!("mEtHoD:{value}")));
    }
    for value in ["X-FUTURE", "future-action-42"] {
        accepted(&calendar(
            "VEVENT",
            &format!("BEGIN:VALARM\r\nACTION:{value}\r\nTRIGGER:-PT1H\r\nEND:VALARM"),
        ));
    }
}

#[test]
fn name_and_action_token_checks_keep_their_existing_boundaries() {
    for value in ["", "UNDER_SCORE", "DOT.NAME", "A B", "\u{65e5}"] {
        assert!(Name::new(value).is_none());
        rejected(&calendar(
            "VEVENT",
            &format!("BEGIN:VALARM\r\nACTION:{value}\r\nTRIGGER:-PT1H\r\nEND:VALARM"),
        ));
    }
    for value in ["X-FUTURE", "future-name-42", "7-NAME"] {
        assert_eq!(Name::new(value).unwrap().as_str(), value);
    }
}

#[test]
fn opaque_value_types_and_unrecognized_components_stay_uninterpreted() {
    for name in ["STATUS", "CLASS", "TRANSP"] {
        accepted(&calendar(
            "VEVENT",
            &format!("{name};VALUE=X-FUTURE:opaque;data,\\q"),
        ));
    }
    for name in ["METHOD", "CALSCALE"] {
        accepted(&calendar_property(&format!(
            "{name};VALUE=X-FUTURE:opaque;data,\\q"
        )));
    }
    accepted(&calendar("VEVENT", "X-STATUS:invalid"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque enum//EN\r\n",
        "BEGIN:X-EXTENSION\r\nSTATUS:invalid\r\nTRANSP:invalid\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
