// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Metadata follows [RFC 5545 sections 3.2.2 and 3.2.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2).
//! Tags follow [RFC 5646 sections 2.1 and 2.2.9](https://www.rfc-editor.org/rfc/rfc5646.html#section-2.1).

#![cfg(test)]

use std::fmt::Write as _;

use icalkit::interop::Import;
use icalkit::{Calendar, Engine, ResourcePolicy};

const TEXT_PROPERTIES: &[&str] = &[
    "CATEGORIES",
    "COMMENT",
    "CONTACT",
    "DESCRIPTION",
    "LOCATION",
    "RESOURCES",
    "SUMMARY",
    "REQUEST-STATUS",
];

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
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit metadata//EN\r\n\
         BEGIN:{kind}\r\nUID:metadata@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn text_property(name: &str, parameters: &str) -> String {
    let value = if name == "REQUEST-STATUS" {
        "2.0;Success"
    } else {
        "Original"
    };
    calendar("VEVENT", &fold(&format!("{name};{parameters}:{value}")))
}

fn party(kind: &str, name: &str, parameters: &str) -> String {
    calendar(
        kind,
        &fold(&format!("{name};{parameters}:mailto:person@example.test")),
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

fn rejected(bytes: &[u8], code: &str) {
    let Err(error) = Calendar::parse(bytes) else {
        panic!(
            "accepted invalid metadata {:?}",
            String::from_utf8_lossy(bytes)
        );
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == code)
    );
    let import = Import::read(bytes).unwrap();
    assert_eq!(import.as_bytes(), bytes);
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), bytes);
}

#[test]
fn common_names_are_single_values_with_one_parameter() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for name in ["ATTENDEE", "ORGANIZER"] {
            for parameters in [
                "CN=Alice,Bob",
                "CN=\"Alice\",\"Bob\"",
                "CN=Alice;cn=Alice",
                "CN=\"Alice\";CN=\"Bob\"",
                "CN",
            ] {
                rejected(
                    party(kind, name, parameters).as_bytes(),
                    "icalkit.validation.invalid-common-name",
                );
            }
        }
    }
}

#[test]
fn common_names_require_utf8() {
    let mut bytes = party("VEVENT", "ATTENDEE", "CN=Alice").into_bytes();
    let index = bytes.windows(5).position(|part| part == b"Alice").unwrap();
    bytes[index] = 0xff;
    rejected(&bytes, "icalkit.validation.invalid-common-name");
}

#[test]
fn common_names_keep_empty_text_unicode_punctuation_and_caret_escapes() {
    for name in ["ATTENDEE", "ORGANIZER"] {
        for value in [
            "",
            "\"\"",
            "Alice Smith",
            "\"Doe, Jane; Team: One\"",
            "\"M\u{00fc}ller \u{5c71}\u{7530}\"",
            "\"Alice^'s^^name^nsecond^qline\"",
            "\"literal\\q\"",
        ] {
            accepted(&party("VEVENT", name, &format!("cN={value}")));
        }
    }
}

#[test]
fn native_text_languages_require_one_unquoted_tag() {
    for name in TEXT_PROPERTIES {
        for value in [
            "",
            "\"en\"",
            "en,fr",
            "\"en,fr\"",
            "en_US",
            "en US",
            "-en",
            "en-",
            "en--US",
            "a",
            "i-unknown",
            "123",
            "abcdefghi",
            "en-abcdefghi",
            "en-US-Latn",
            "en-u",
            "en-u-a",
            "en-x",
            "x",
            "\u{00e9}n",
        ] {
            rejected(
                text_property(name, &format!("LANGUAGE={value}")).as_bytes(),
                "icalkit.validation.invalid-language",
            );
        }
        rejected(
            text_property(name, "LANGUAGE=en;language=fr").as_bytes(),
            "icalkit.validation.invalid-language",
        );
    }
}

#[test]
fn language_tags_keep_structure_case_and_unregistered_subtags() {
    for value in [
        "en",
        "EN-us",
        "zh-cmn-Hans-CN",
        "es-419",
        "de-CH-1901",
        "sl-rozaj-biske-1994",
        "en-u-ca-gregory",
        "en-a-foo-foo-b-foo-0-bar-x-one-one",
        "x-a-A-one-one",
        "abcd",
        "abcdefgh",
        "qaa-Latn-QM-abcde",
        "abc-def-ghi-jkl-Latn-419-1234-abcde-u-attr-x-one",
    ] {
        for name in TEXT_PROPERTIES {
            accepted(&text_property(name, &format!("lAnGuAgE={value}")));
        }
    }
}

#[test]
fn all_grandfathered_tags_keep_their_original_spelling() {
    for value in [
        "en-GB-oed",
        "i-ami",
        "i-bnn",
        "i-default",
        "i-enochian",
        "i-hak",
        "i-klingon",
        "i-lux",
        "i-mingo",
        "i-navajo",
        "i-pwn",
        "i-tao",
        "i-tay",
        "i-tsu",
        "sgn-BE-FR",
        "sgn-BE-NL",
        "sgn-CH-DE",
        "art-lojban",
        "cel-gaulish",
        "no-bok",
        "no-nyn",
        "zh-guoyu",
        "zh-hakka",
        "zh-min",
        "zh-min-nan",
        "zh-xiang",
    ] {
        accepted(&text_property("SUMMARY", &format!("LANGUAGE={value}")));
        accepted(&text_property(
            "SUMMARY",
            &format!("LANGUAGE={}", value.to_ascii_uppercase()),
        ));
    }
}

#[test]
fn variants_and_extension_singletons_cannot_repeat() {
    for value in [
        "de-1901-1901",
        "sl-rozaj-biske-ROZAJ",
        "en-abcde-ABCDE",
        "en-a-foo-A-bar",
        "en-0-foo-b-bar-0-baz",
    ] {
        rejected(
            text_property("SUMMARY", &format!("LANGUAGE={value}")).as_bytes(),
            "icalkit.validation.invalid-language",
        );
    }
}

#[test]
fn participant_languages_use_the_same_tag_and_cardinality_rules() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for name in ["ATTENDEE", "ORGANIZER"] {
            accepted(&party(kind, name, "CN=Alice;LANGUAGE=en-GB"));
            accepted(&party(kind, name, "LANGUAGE=x-person"));
            for parameters in [
                "CN=Alice;LANGUAGE=en_US",
                "LANGUAGE=en;language=fr",
                "LANGUAGE=en-a-one-A-two",
            ] {
                rejected(
                    party(kind, name, parameters).as_bytes(),
                    "icalkit.validation.invalid-language",
                );
            }
        }
    }
    accepted(&party("VFREEBUSY", "ORGANIZER", "CN=Alice;LANGUAGE=en"));
    accepted(&party("VFREEBUSY", "ATTENDEE", "LANGUAGE=en"));
    rejected(
        party("VFREEBUSY", "ATTENDEE", "CN=Alice").as_bytes(),
        "icalkit.validation.invalid-common-name",
    );
}

#[test]
fn alarm_attendees_check_languages_without_allowing_common_names() {
    for (parameter, code) in [
        ("CN=Alice", Some("icalkit.validation.invalid-common-name")),
        ("LANGUAGE=en", None),
        (
            "LANGUAGE=en_US",
            Some("icalkit.validation.invalid-language"),
        ),
    ] {
        let alarm = format!(
            "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT1H\r\nSUMMARY:Alarm\r\n\
             DESCRIPTION:Reminder\r\n{}\r\nEND:VALARM",
            fold(&format!("ATTENDEE;{parameter}:mailto:person@example.test"))
        );
        let source = calendar("VEVENT", &alarm);
        if let Some(code) = code {
            rejected(source.as_bytes(), code);
        } else {
            accepted(&source);
        }
    }
}

#[test]
fn language_checks_reach_time_zone_names_and_alarm_text() {
    for value in ["en", "en_US"] {
        let zone = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit metadata//EN\r\n\
             BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n\
             DTSTART:20240101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0000\r\n\
             TZNAME;LANGUAGE={value}:Standard\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
             END:VCALENDAR\r\n"
        );
        let alarm = calendar(
            "VEVENT",
            &format!(
                "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT1H\r\n\
                 DESCRIPTION;LANGUAGE={value}:Reminder\r\nEND:VALARM"
            ),
        );
        for source in [zone, alarm] {
            if value == "en" {
                accepted(&source);
            } else {
                rejected(source.as_bytes(), "icalkit.validation.invalid-language");
            }
        }
    }
}

#[test]
fn descriptive_parameters_use_unfolded_text() {
    accepted(&calendar(
        "VEVENT",
        "ATTENDEE;C\r\n N=\"Doe, Jane\";LANGU\r\n AGE=en-\r\n US:mailto:person@example.test",
    ));
    rejected(
        calendar("VEVENT", "SUMMARY;LANGUAGE=en-\r\n _US:Original").as_bytes(),
        "icalkit.validation.invalid-language",
    );
}

#[test]
fn unknown_semantics_keep_their_metadata() {
    for name in TEXT_PROPERTIES {
        accepted(&text_property(
            name,
            "VALUE=X-FUTURE;LANGUAGE=bad_tag;LANGUAGE=other",
        ));
    }
    accepted(&party(
        "VEVENT",
        "ATTENDEE",
        "VALUE=X-FUTURE;CN=A,B;LANGUAGE=bad_tag",
    ));
    accepted(&text_property(
        "X-UNKNOWN",
        "CN=A,B;LANGUAGE=bad_tag;LANGUAGE=other",
    ));
    accepted(&calendar("VEVENT", "SUMMARY;X-LANGUAGE=bad_tag:Original"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque metadata//EN\r\n",
        "BEGIN:X-EXTENSION\r\nSUMMARY;LANGUAGE=bad_tag:Original\r\n",
        "ATTENDEE;CN=A,B;LANGUAGE=bad_tag:opaque\r\nEND:X-EXTENSION\r\n",
        "END:VCALENDAR\r\n",
    ));
}

#[test]
fn variant_comparisons_share_the_input_resource_budget() {
    let mut variants = String::new();
    for index in 0..80 {
        write!(&mut variants, "-{index:04}").unwrap();
    }
    let private = text_property("SUMMARY", &format!("LANGUAGE=en-x{variants}"));
    let repeated_work = text_property("SUMMARY", &format!("LANGUAGE=en{variants}"));
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_max_input_bytes(8192))
        .build();
    assert!(engine.session().parse(private.as_bytes()).is_ok());
    let Err(error) = engine.session().parse(repeated_work.as_bytes()) else {
        panic!("accepted unbounded variant comparisons");
    };
    assert_eq!(error.code().as_str(), "icalkit.parse.resource-limit");
    accepted(&repeated_work);
}

#[test]
fn exhaustion_never_promotes_an_invalid_language() {
    let source = text_property("SUMMARY", "LANGUAGE=en-a-one-A-two");
    for budget in 0..=2048 {
        let engine = Engine::builder()
            .resource_policy(ResourcePolicy::secure().with_max_input_bytes(budget))
            .build();
        assert!(
            engine.session().parse(source.as_bytes()).is_err(),
            "budget {budget}"
        );
    }
}
