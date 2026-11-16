// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Relationships follow [RFC 9253 section 9.1](https://www.rfc-editor.org/rfc/rfc9253.html#section-9.1).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;
use icalkit_conformance::internal::core::{DecodeValue as _, Duration, ValueType};

const URI_RELATIONSHIPS: [&str; 9] = [
    "FINISHTOSTART",
    "FINISHTOFINISH",
    "STARTTOFINISH",
    "STARTTOSTART",
    "FIRST",
    "NEXT",
    "DEPENDS-ON",
    "REFID",
    "CONCEPT",
];

fn fold(text: &str) -> String {
    let mut output = String::new();
    let mut width = 0_usize;
    for character in text.chars() {
        if matches!(character, '\r' | '\n') {
            output.push(character);
            width = 0;
            continue;
        }
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

fn event(properties: &str) -> String {
    let properties = if properties.is_empty() {
        String::new()
    } else {
        format!("{properties}\r\n")
    };
    format!(
        "BEGIN:VEVENT\r\nUID:relation@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\n{properties}END:VEVENT\r\n"
    )
}

fn wrap(component: &str) -> String {
    fold(&format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit relations//EN\r\n\
         {component}END:VCALENDAR\r\n"
    ))
}

fn calendar(property: &str) -> String {
    wrap(&event(property))
}

fn accepted(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.validate().unwrap().to_bytes(), source.as_bytes());
}

fn rejected(source: &str, code: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid relationship {source:?}");
    };
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == code),
        "{source:?}: {error:?}"
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn registered_uid_value_type_is_recognized_without_an_opaque_note() {
    for spelling in [b"UID".as_slice(), b"uid", b"Uid"] {
        let kind = ValueType::from_name(spelling).expect("registered UID type");
        assert_eq!(kind.as_bytes(), b"UID");
    }
    let source = calendar("RELATED-TO;VALUE=Uid:other@example.test");
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert!(
        calendar
            .issues()
            .iter()
            .all(|issue| issue.code().as_str() != "unknown-value-type")
    );
}

#[test]
fn updated_relationship_types_accept_uri_references() {
    for relation in URI_RELATIONSHIPS {
        for uri in [
            "urn:calendar:other",
            "https://example.test/cal/other.ics",
            "HTTPS://example.test/a;b,c?q=%E6%97%A5&x=1#item",
        ] {
            accepted(&calendar(&format!(
                "RELATED-TO;VALUE=URI;RELTYPE={relation}:{uri}"
            )));
        }
    }
    accepted(&calendar(
        "related-to;value=uri;reltype=FinishToStart:urn:other",
    ));
}

#[test]
fn identifiers_keep_legacy_text_and_explicit_uid_forms() {
    for relation in [
        "",
        ";RELTYPE=PARENT",
        ";RELTYPE=CHILD",
        ";RELTYPE=SIBLING",
        ";RELTYPE=SNOOZE",
        ";RELTYPE=X-PRIVATE",
        ";RELTYPE=FUTURE-RELATION",
        ";RELTYPE=FinishToStart",
    ] {
        for value_type in ["", ";VALUE=TEXT", ";VALUE=UID", ";VALUE=uid"] {
            accepted(&calendar(&format!(
                "RELATED-TO{value_type}{relation}:other\\,part\\;key\\\\end@日.test"
            )));
        }
    }
}

#[test]
fn repeated_relationships_are_permitted_in_every_known_component() {
    let properties = concat!(
        "RELATED-TO:parent@example.test\r\n",
        "RELATED-TO;VALUE=URI;RELTYPE=NEXT:urn:next",
    );
    accepted(&wrap(&format!("{properties}\r\n{}", event(""))));
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        accepted(&wrap(&event(properties).replace("VEVENT", kind)));
    }
    let observance = concat!(
        "BEGIN:STANDARD\r\nDTSTART:20240101T000000\r\n",
        "TZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n",
    );
    accepted(&wrap(&format!(
        "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\n{properties}\r\n\
         {observance}END:STANDARD\r\nEND:VTIMEZONE\r\n"
    )));
    for kind in ["STANDARD", "DAYLIGHT"] {
        accepted(&wrap(
            &format!(
                "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\n\
             {observance}{properties}\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n"
            )
            .replace("STANDARD", kind),
        ));
    }
    accepted(&calendar(&format!(
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT1H\r\n\
         DESCRIPTION:Reminder\r\n{properties}\r\nEND:VALARM"
    )));
}

#[test]
fn malformed_uri_references_fail_strict_promotion() {
    for uri in [
        "",
        "relative/path",
        "https://[invalid]/",
        "urn:bad%xx",
        "urn:bad value",
        "urn:日",
    ] {
        rejected(
            &calendar(&format!("RELATED-TO;VALUE=URI;RELTYPE=NEXT:{uri}")),
            "icalkit.validation.invalid-relationship",
        );
    }
}

#[test]
fn hierarchical_unknown_and_snooze_relationships_reference_identifiers() {
    for relation in [
        "",
        ";RELTYPE=PARENT",
        ";RELTYPE=Child",
        ";RELTYPE=sibling",
        ";RELTYPE=SNOOZE",
        ";RELTYPE=X-PRIVATE",
        ";RELTYPE=FUTURE-RELATION",
    ] {
        rejected(
            &calendar(&format!("RELATED-TO;VALUE=URI{relation}:urn:other")),
            "icalkit.validation.invalid-relationship",
        );
    }
}

#[test]
fn relationship_type_is_one_unquoted_ascii_token() {
    for parameters in [
        ";RELTYPE=",
        ";RELTYPE=NEXT;RELTYPE=FIRST",
        ";RELTYPE=NEXT,FIRST",
        ";RELTYPE=\"NEXT\"",
        ";RELTYPE=NEXT ONE",
        ";RELTYPE=NEXT_ONE",
        ";RELTYPE=NÉXT",
        ";RELTYPE=NEXT^nONE",
    ] {
        rejected(
            &calendar(&format!("RELATED-TO{parameters}:other")),
            "icalkit.validation.invalid-relationship",
        );
    }
    for relation in ["0", "x-Private", "Future-Relation", "nExT"] {
        accepted(&calendar(&format!("RELATED-TO;RELTYPE={relation}:other")));
    }
}

#[test]
fn gaps_accept_signed_duration_forms_and_case_insensitive_parameters() {
    for gap in [
        "P0D",
        "PT0S",
        "+P1W",
        "-P2D",
        "P1DT2H3M4S",
        "PT1M",
        "PT1H0M2S",
        "p1d",
        "+p1w",
        "-p1dt2h3m4s",
        "Pt1H2m3s",
    ] {
        accepted(&calendar(&format!(
            "RELATED-TO;VALUE=URI;RELTYPE=FINISHTOSTART;GAP={gap}:urn:other"
        )));
        accepted(&calendar(&format!("RELATED-TO;GAP={gap}:other")));
    }
    assert!(Duration::decode_value(b"P1DT2H").is_ok());
    assert!(Duration::decode_value(b"p1dt2h").is_err());
}

#[test]
fn gaps_reject_malformed_duplicate_quoted_and_overflowing_values() {
    for parameters in [
        ";GAP=",
        ";GAP=P",
        ";GAP=PT",
        ";GAP=1D",
        ";GAP=P1M",
        ";GAP=P1Y",
        ";GAP=P1W1D",
        ";GAP=PT1S1H",
        ";GAP=P1DT",
        ";GAP=P-1D",
        ";GAP=--P1D",
        ";GAP=P1.5D",
        ";GAP=\"P1D\"",
        ";GAP=P1D,PT1H",
        ";GAP=P1D;GAP=P2D",
        ";GAP=P9223372036854775808D",
        ";GAP=PT18446744073709551615H",
    ] {
        rejected(
            &calendar(&format!("RELATED-TO;RELTYPE=NEXT{parameters}:other")),
            "icalkit.validation.invalid-relationship",
        );
    }
}

#[test]
fn gaps_cannot_omit_minutes_between_hours_and_seconds() {
    for gap in ["PT1H2S", "P1DT1H2S", "-pt1h2s"] {
        rejected(
            &calendar(&format!("RELATED-TO;RELTYPE=NEXT;GAP={gap}:other")),
            "icalkit.validation.invalid-relationship",
        );
    }
}

#[test]
fn duration_and_period_properties_share_the_gap_term_grammar() {
    for duration in ["PT1H2S", "P1DT1H2S"] {
        assert!(Duration::decode_value(duration.as_bytes()).is_err());
        assert!(Calendar::parse(calendar(&format!("DURATION:{duration}")).as_bytes()).is_err());
        let busy = calendar(&format!("FREEBUSY:20240102T090000Z/{duration}"))
            .replace("VEVENT", "VFREEBUSY");
        assert!(Calendar::parse(busy.as_bytes()).is_err());
    }
    accepted(&calendar("DURATION:P1DT1H0M2S"));
    accepted(&calendar("FREEBUSY:20240102T090000Z/PT1H0M2S").replace("VEVENT", "VFREEBUSY"));
}

#[test]
fn parameter_and_value_checks_use_unfolded_bytes() {
    accepted(&calendar(concat!(
        "RELATED-TO;VALUE\r\n =URI;REL\r\n TYPE=FINISHTOSTART;GA\r\n P=-p1d:\r\n ",
        "https://example.test/ca\r\n l/other.ics",
    )));
    rejected(
        &calendar("RELATED-TO;REL\r\n TYPE=NEXT_\r\n ONE:other"),
        "icalkit.validation.invalid-relationship",
    );
    rejected(
        &calendar("RELATED-TO;GA\r\n P=PT1S\r\n 1H:other"),
        "icalkit.validation.invalid-relationship",
    );
}

#[test]
fn native_uid_references_cannot_hide_malformed_text() {
    for value_type in ["", ";VALUE=TEXT", ";VALUE=UID"] {
        for value in ["a,b", "a;b", "a\\q", "a\\"] {
            rejected(
                &calendar(&format!("RELATED-TO{value_type}:{value}")),
                "icalkit.validation.invalid-text",
            );
        }
    }
}

#[test]
fn known_incompatible_value_types_are_refused() {
    for value_type in [
        "DATE",
        "DATE-TIME",
        "INTEGER",
        "BINARY",
        "DURATION",
        "CAL-ADDRESS",
    ] {
        assert!(
            Calendar::parse(calendar(&format!("RELATED-TO;VALUE={value_type}:other")).as_bytes())
                .is_err()
        );
    }
    let source = calendar("RELATED-TO;VALUE=UID:other").replace(
        "UID:relation@example.test",
        "UID;VALUE=UID:relation@example.test",
    );
    rejected(&source, "icalkit.validation.invalid-text");
}

#[test]
fn opaque_relationship_values_properties_and_components_keep_their_bytes() {
    for property in [
        "RELATED-TO;VALUE=X-FUTURE;RELTYPE=\"UNKNOWN\";GAP=bad:a,b\\q",
        "X-RELATED-TO;VALUE=UID;RELTYPE=\"UNKNOWN\";GAP=bad:a,b\\q",
        "RELATED-TO;X-RELTYPE=\"UNKNOWN\";X-GAP=bad:other",
    ] {
        accepted(&calendar(property));
    }
    accepted(&wrap(concat!(
        "BEGIN:X-EXTENSION\r\nRELATED-TO;VALUE=URI;RELTYPE=\"UNKNOWN\";GAP=bad:relative\r\n",
        "END:X-EXTENSION\r\n",
    )));
    for declaration in [";VALUE=UID;VALUE=UID", ";VALUE=X-FUTURE;VALUE=UID"] {
        assert!(
            Calendar::parse(calendar(&format!("RELATED-TO{declaration}:other")).as_bytes())
                .is_err()
        );
    }
}
