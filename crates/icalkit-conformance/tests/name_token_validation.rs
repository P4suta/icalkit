// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Names follow [RFC 5545 section 3.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.1).
//! Extension components follow [section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(content: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit names//EN\r\n\
         BEGIN:VEVENT\r\nUID:name@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{content}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &str) -> Calendar {
    let parsed = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(parsed.to_bytes(), source.as_bytes());
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.validate().unwrap().to_bytes(), source.as_bytes());
    parsed
}

fn rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid name syntax: {source}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error()
                && issue.code().as_str() == "icalkit.validation.invalid-name"),
        "missing invalid-name diagnostic: {error:?}"
    );
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn property_names_require_ascii_tokens() {
    for name in [
        "UNDER_SCORE",
        "DOT.NAME",
        "NAME WITH SPACE",
        "X+NAME",
        "TYPE/NAME",
        "\u{65e5}",
    ] {
        rejected(&calendar(&format!("{name}:opaque")));
        rejected(&calendar(&format!("{name};VALUE=X-FUTURE:opaque;data,\\q")));
    }
}

#[test]
fn parameter_names_require_ascii_tokens_on_known_and_unknown_properties() {
    for name in [
        "",
        "UNDER_SCORE",
        "DOT.NAME",
        "TWO WORDS",
        "X+FLAG",
        "\u{65e5}",
    ] {
        for property in ["SUMMARY", "X-PROPERTY"] {
            rejected(&calendar(&format!("{property};{name}=value:opaque")));
        }
    }
}

#[test]
fn component_names_require_ascii_tokens() {
    for name in [
        "UNDER_SCORE",
        "DOT.NAME",
        "NAME WITH SPACE",
        "X+NAME",
        "TYPE/NAME",
        "\u{65e5}",
    ] {
        rejected(&calendar(&format!(
            "BEGIN:{name}\r\nX-PROPERTY:opaque\r\nEND:{name}"
        )));
    }
}

#[test]
fn names_are_checked_inside_unknown_component_subtrees() {
    for content in [
        "UNDER_SCORE:opaque",
        "X-PROPERTY;UNDER_SCORE=value:opaque",
        "BEGIN:UNDER_SCORE\r\nX-PROPERTY:opaque\r\nEND:UNDER_SCORE",
    ] {
        rejected(&calendar(&format!(
            "BEGIN:X-OUTER\r\nBEGIN:X-INNER\r\n{content}\r\n\
             END:X-INNER\r\nEND:X-OUTER"
        )));
    }
}

#[test]
fn valid_tokens_and_case_keep_names_and_opaque_values() {
    for name in ["x-MiXeD-42", "future-name", "7-NAME", "123", "-", "X-"] {
        let source = calendar(&format!(
            "{name};{name}=value:opaque\r\n\
             BEGIN:{name}\r\nSTATUS:unknown\r\nSUMMARY:opaque;data,\\q\r\nEND:{name}"
        ));
        let parsed = accepted(&source);
        let event = parsed.events().next().unwrap();
        assert_eq!(event.property(name).unwrap().name().as_str(), name);
        assert_eq!(event.components().next().unwrap().name().as_str(), name);
    }
}

#[test]
fn unfolded_names_are_checked_without_changing_physical_spelling() {
    let source = calendar(concat!(
        "x-Pr\r\n operTy;x-Pa\r\n ram=\"value\":opaque\r\n",
        "BEGIN:x-Co\r\n mpOnent\r\nX-PROPERTY:opaque\r\n",
        "END:x-Co\r\n mpOnent",
    ));
    let parsed = accepted(&source);
    let event = parsed.events().next().unwrap();
    assert_eq!(
        event.property("X-PROPERTY").unwrap().name().as_str(),
        "x-ProperTy"
    );
    assert_eq!(
        event.components().next().unwrap().name().as_str(),
        "x-CompOnent"
    );
    rejected(&calendar("X-UNDER\r\n _SCORE:opaque"));
    rejected(&calendar("SUMMARY;X-UNDER\r\n _SCORE=value:opaque"));
}
