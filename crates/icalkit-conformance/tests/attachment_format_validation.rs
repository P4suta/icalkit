// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Formats follow [RFC 5545 section 3.2.8](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.8).
//! Names follow its reference to [RFC 4288 section 4.2](https://www.rfc-editor.org/rfc/rfc4288.html#section-4.2).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn fold(line: &str) -> String {
    let mut output = String::new();
    let mut width = 0_usize;
    for character in line.chars() {
        let prospective = width.checked_add(character.len_utf8()).unwrap();
        if prospective > 60 {
            output.push_str("\r\n ");
            width = 1_usize.checked_add(character.len_utf8()).unwrap();
        } else {
            width = prospective;
        }
        output.push(character);
    }
    output
}

fn calendar(kind: &str, property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit formats//EN\r\n\
         BEGIN:{kind}\r\nUID:format@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn attachment(media_type: &str, binary: bool) -> String {
    let (parameters, value) = if binary {
        (";VALUE=BINARY;ENCODING=BASE64", "QQ==")
    } else {
        ("", "urn:attachment")
    };
    fold(&format!("ATTACH;FMTTYPE={media_type}{parameters}:{value}"))
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
        panic!("accepted invalid attachment format {source:?}");
    };
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| { issue.code().as_str() == "icalkit.validation.invalid-uri-or-binary" })
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn formats_require_one_unquoted_type_and_subtype() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for media_type in [
            "",
            "text",
            "/plain",
            "text/",
            "text/plain/extra",
            "text//plain",
            "text/plain,text/html",
            "\"text/plain\"",
            "\"text/plain;charset=utf-8\"",
            "text/pla in",
            "t\u{00e9}xt/plain",
            "text/pl\u{00e4}in",
            "text/pl%61in",
        ] {
            for binary in [false, true] {
                rejected(&calendar(kind, &attachment(media_type, binary)));
            }
        }
    }
}

#[test]
fn format_names_reject_characters_outside_the_referenced_grammar() {
    for character in [
        '*', '%', '\'', '`', '~', '|', '{', '}', '(', ')', '=', '?', '@', '[', ']', '\\',
    ] {
        for media_type in [
            format!("a{character}b/plain"),
            format!("text/a{character}b"),
        ] {
            for binary in [false, true] {
                rejected(&calendar("VEVENT", &attachment(&media_type, binary)));
            }
        }
    }
}

#[test]
fn format_names_enforce_the_protocol_length_bound() {
    for length in [1, 64, 65, 127, 128] {
        let name = "a".repeat(length);
        for media_type in [format!("{name}/plain"), format!("text/{name}")] {
            for binary in [false, true] {
                let source = calendar("VEVENT", &attachment(&media_type, binary));
                if length <= 127 {
                    accepted(&source);
                } else {
                    rejected(&source);
                }
            }
        }
    }
}

#[test]
fn names_keep_case_punctuation_and_non_standard_formats() {
    for media_type in [
        "text/plain",
        "Application/Vnd.Example+Json",
        "x-type/x-private",
        "0/1",
        "A!#$&.+-^_0/B!#$&.+-^_1",
        "!type/+subtype",
        "^n/^^",
    ] {
        for binary in [false, true] {
            accepted(&calendar("VEVENT", &attachment(media_type, binary)));
        }
    }
    accepted(&calendar(
        "VEVENT",
        "ATTACH;VALUE=URI;fmTtYpE=Text/Plain;\r\n X-SOURCE=one;X-SOURCE=two:urn:attachment",
    ));
}

#[test]
fn format_checks_use_unfolded_parameter_text() {
    accepted(&calendar(
        "VEVENT",
        "ATTACH;FMT\r\n TYPE=application/\r\n octet-stream:urn:attachment",
    ));
    rejected(&calendar(
        "VEVENT",
        "ATTACH;FMT\r\n TYPE=application/\r\n bad*type:urn:attachment",
    ));
}

#[test]
fn format_checks_reach_alarm_attachments() {
    for media_type in ["audio/basic", "audio"] {
        let alarm = format!(
            "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT1H\r\n{}\r\nEND:VALARM",
            attachment(media_type, false)
        );
        let source = calendar("VEVENT", &alarm);
        if media_type == "audio/basic" {
            accepted(&source);
        } else {
            rejected(&source);
        }
    }
}

#[test]
fn opaque_values_properties_and_components_keep_their_formats() {
    accepted(&calendar(
        "VEVENT",
        "ATTACH;VALUE=X-FUTURE;FMTTYPE=invalid:opaque",
    ));
    accepted(&calendar(
        "VEVENT",
        "X-ATTACH;FMTTYPE=invalid;FMTTYPE=other:opaque",
    ));
    accepted(&calendar(
        "VEVENT",
        "ATTACH;X-FMTTYPE=invalid:urn:attachment",
    ));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque formats//EN\r\n",
        "BEGIN:X-EXTENSION\r\nATTACH;FMTTYPE=invalid;FMTTYPE=other:opaque\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
