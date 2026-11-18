// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Addresses follow [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.3).
//! Attachments follow [the URI and binary forms](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.1).
//! Encoding controls follow [section 3.2.7](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.7).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit linked values//EN\r\n\
         BEGIN:{kind}\r\nUID:linked@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn timezone(property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit linked zone//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Linked\r\n{property}\r\nBEGIN:STANDARD\r\n\
         DTSTART:20200101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n\
         END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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
        panic!("accepted invalid address or attachment: {source:?}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "icalkit.validation.invalid-uri-or-binary")
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn resource_links_require_valid_uri_values() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for name in ["URL", "ATTACH"] {
            for value in [
                "",
                "relative.ics",
                "http://host:bad/",
                "http://[::1/",
                "x:/[item]",
                "https://exa mple.test/",
            ] {
                rejected(&calendar(kind, &format!("{name}:{value}")));
            }
        }
    }
}

#[test]
fn time_zone_links_use_the_same_uri_validation() {
    for value in [
        "",
        "zone.ics",
        "https://host:bad/zone.ics",
        "https://[example.test]/",
    ] {
        rejected(&timezone(&format!("TZURL:{value}")));
    }
    accepted(&timezone(
        "tZuRl;VALUE=URI;X-SOURCE=example:https://Example.Test/zones/a%20b.ics",
    ));
}

#[test]
fn calendar_addresses_require_a_uri_scheme_and_valid_components() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        for name in ["ORGANIZER", "ATTENDEE"] {
            for value in [
                "",
                "person@example.test",
                "mailto:person name@example.test",
                "https://host:bad/people/person",
            ] {
                rejected(&calendar(kind, &format!("{name}:{value}")));
            }
            for value in [
                "MailTo:Person@Example.Test",
                "urn:uuid:12345678-1234-1234-1234-123456789abc",
                "https://example.test/people/person",
            ] {
                accepted(&calendar(kind, &format!("{name}:{value}")));
            }
        }
    }
}

#[test]
fn links_and_addresses_check_the_property_value_type() {
    for name in ["URL", "ATTACH"] {
        for value_type in ["TEXT", "CAL-ADDRESS", "INTEGER"] {
            rejected(&calendar(
                "VEVENT",
                &format!("{name};VALUE={value_type}:mailto:person@example.test"),
            ));
        }
    }
    for name in ["ORGANIZER", "ATTENDEE"] {
        for value_type in ["TEXT", "URI", "BINARY"] {
            rejected(&calendar(
                "VEVENT",
                &format!("{name};VALUE={value_type}:mailto:person@example.test"),
            ));
        }
        accepted(&calendar(
            "VEVENT",
            &format!("{name};VaLuE=CAL-ADDRESS;X-SOURCE=example:MailTo:Person@Example.Test"),
        ));
    }
    rejected(&timezone("TZURL;VALUE=TEXT:https://example.test/zone.ics"));
}

#[test]
fn known_linked_values_reject_repeated_value_parameters() {
    for (name, value_type) in [
        ("URL", "URI"),
        ("ATTACH", "URI"),
        ("ORGANIZER", "CAL-ADDRESS"),
        ("ATTENDEE", "CAL-ADDRESS"),
    ] {
        accepted(&calendar(
            "VEVENT",
            &format!("{name};VALUE={value_type}:mailto:person@example.test"),
        ));
        for parameters in [
            format!("VALUE={value_type};VALUE={value_type}"),
            format!("VALUE=X-FUTURE;VALUE={value_type}"),
        ] {
            rejected(&calendar(
                "VEVENT",
                &format!("{name};{parameters}:mailto:person@example.test"),
            ));
        }
    }
    rejected(&timezone(
        "TZURL;VALUE=URI;VALUE=URI:https://example.test/zone.ics",
    ));
}

#[test]
fn binary_attachments_require_valid_base64_payloads() {
    for value in [
        "Q", "QQ", "QQ=", "=QQ=", "QQ===", "Q!==", "QQ==QQ==", "QU JD", "QUJD;",
    ] {
        rejected(&calendar(
            "VEVENT",
            &format!("ATTACH;VALUE=BINARY;ENCODING=BASE64:{value}"),
        ));
    }
    for value in ["", "QQ==", "QR==", "QUI=", "QUJD", "////"] {
        accepted(&calendar(
            "VEVENT",
            &format!("ATTACH;VALUE=BINARY;ENCODING=BASE64:{value}"),
        ));
    }
}

#[test]
fn binary_attachments_require_one_base64_encoding_declaration() {
    for parameters in [
        "VALUE=BINARY",
        "VALUE=BINARY;ENCODING=8BIT",
        "VALUE=BINARY;ENCODING=BASE32",
        "VALUE=BINARY;ENCODING=BASE64;ENCODING=BASE64",
        "ENCODING=BASE64;ENCODING=8BIT;VALUE=BINARY",
    ] {
        rejected(&calendar("VEVENT", &format!("ATTACH;{parameters}:QQ==")));
    }
    accepted(&calendar(
        "VEVENT",
        "aTtAcH;VaLuE=BINARY;EnCoDiNg=base64:QQ==",
    ));
}

#[test]
fn uri_attachments_use_text_encoding() {
    for parameters in [
        "ENCODING=BASE64",
        "VALUE=URI;ENCODING=BASE64",
        "ENCODING=BASE32",
        "ENCODING=8BIT;ENCODING=8BIT",
    ] {
        rejected(&calendar(
            "VEVENT",
            &format!("ATTACH;{parameters}:https://example.test/file"),
        ));
    }
    for parameters in ["VALUE=URI", "ENCODING=8BIT", "VALUE=URI;ENCODING=8bit"] {
        accepted(&calendar(
            "VEVENT",
            &format!("ATTACH;{parameters}:https://Example.Test/file%20name"),
        ));
    }
    rejected(&calendar("VEVENT", "ATTACH;ENCODING=BASE64:QQ=="));
}

#[test]
fn attachment_media_types_are_optional_and_must_not_repeat() {
    for property in [
        "ATTACH;FMTTYPE=text/plain;FMTTYPE=text/plain:https://example.test/file",
        "ATTACH;VALUE=BINARY;ENCODING=BASE64;FMTTYPE=text/plain;\r\n FMTTYPE=application/octet-stream:QQ==",
    ] {
        rejected(&calendar("VEVENT", property));
    }
    for property in [
        "ATTACH;VALUE=BINARY;ENCODING=BASE64:QQ==",
        "ATTACH;VALUE=BINARY;ENCODING=BASE64;\r\n FMTTYPE=application/octet-stream;X-SOURCE=one;X-SOURCE=two:QQ==",
        "ATTACH;FMTTYPE=text/plain:https://example.test/file",
    ] {
        accepted(&calendar("VEVENT", property));
    }
}

#[test]
fn attachment_encoding_controls_use_unquoted_tokens() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for (value_type, encoding, value) in [
            ("BINARY", "\"BASE64\"", "QQ=="),
            ("BINARY", "\"base64\"", "QQ=="),
            ("BINARY", "\"\r\n BASE64\"", "QQ=="),
            ("BINARY", "BASE64,8BIT", "QQ=="),
            ("BINARY", "BASE 64", "QQ=="),
            ("BINARY", "BASE64^n", "QQ=="),
            ("URI", "\"8BIT\"", "urn:attachment"),
            ("URI", "\"8bit\"", "urn:attachment"),
            ("URI", "\"\r\n 8BIT\"", "urn:attachment"),
            ("URI", "8BIT,BASE64", "urn:attachment"),
            ("URI", "8 BIT", "urn:attachment"),
            ("URI", "8BIT^n", "urn:attachment"),
        ] {
            let property = format!("ATTACH;VALUE={value_type};ENCODING={encoding}:{value}");
            rejected(&calendar(kind, &property));
            if kind == "VJOURNAL" {
                continue;
            }
            let nested = format!(
                "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT15M\r\n\
                 {property}\r\nEND:VALARM"
            );
            rejected(&calendar(kind, &nested));
        }
    }
}

#[test]
fn encoding_tokens_keep_case_folding_and_value_declaration_compatibility() {
    for property in [
        "ATTACH;VALUE=BINARY;ENCODING=Base64:QQ==",
        "ATTACH;VALUE=BINARY;ENCODING\r\n =base64:QQ==",
        "attach;value=binary;encoding=BASE64;X-SOURCE=one;X-SOURCE=two:QQ==",
        "ATTACH;VALUE=URI;ENCODING=8bIt:urn:attachment",
        "ATTACH;ENCODING\r\n =8BIT:urn:attachment",
    ] {
        accepted(&calendar("VEVENT", property));
    }
}

#[test]
fn unknown_attachment_encodings_stay_opaque() {
    for property in [
        "ATTACH;VALUE=X-FUTURE;ENCODING=\"UNKNOWN\":opaque",
        "X-ATTACH;VALUE=BINARY;ENCODING=\"UNKNOWN\":opaque",
        "ATTACH;X-ENCODING=\"UNKNOWN\":urn:attachment",
    ] {
        accepted(&calendar("VEVENT", property));
    }
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque controls//EN\r\n",
        "BEGIN:X-EXTENSION\r\nATTACH;VALUE=BINARY;ENCODING=\"UNKNOWN\":opaque\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}

#[test]
fn folded_binary_payloads_keep_their_original_content_lines() {
    accepted(&calendar(
        "VEVENT",
        "ATTACH;VALUE=BINARY;ENCODING=BASE64:QU\r\n JD",
    ));
    rejected(&calendar(
        "VEVENT",
        "ATTACH;VALUE=BINARY;ENCODING=BASE64:QU\r\n  JD",
    ));
}

#[test]
fn attachment_validation_covers_nested_alarms() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        accepted(&calendar(kind, "ATTACH;VALUE=BINARY;ENCODING=BASE64:QQ=="));
    }
    let alarm = "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH;VALUE=BINARY;ENCODING=BASE64:QQ==\r\nEND:VALARM";
    accepted(&calendar("VEVENT", alarm));
    rejected(&calendar(
        "VEVENT",
        &alarm.replace("BASE64:QQ==", "BASE64:invalid"),
    ));
}

#[test]
fn unknown_linked_value_types_remain_opaque() {
    for name in ["URL", "ATTACH", "ORGANIZER", "ATTENDEE"] {
        accepted(&calendar(
            "VEVENT",
            &format!("{name};VALUE=X-FUTURE:opaque;data,\\q"),
        ));
    }
    accepted(&timezone("TZURL;VALUE=X-FUTURE:opaque;data,\\q"));
}

#[test]
fn unrecognized_components_and_properties_keep_their_linked_content() {
    accepted(&calendar("VEVENT", "X-ATTACH:invalid"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque links//EN\r\n",
        "BEGIN:X-EXTENSION\r\nATTENDEE:invalid\r\nATTACH;VALUE=BINARY:invalid\r\n",
        "URL:relative.ics\r\nEND:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
