// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Classifications follow [RFC 5545 section 3.2.9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.9).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit classifications//EN\r\n\
         BEGIN:VFREEBUSY\r\nUID:busy@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n{property}\r\n\
         END:VFREEBUSY\r\nEND:VCALENDAR\r\n"
    )
}

fn freebusy(parameters: &str) -> String {
    calendar(&format!("FREEBUSY{parameters}:\r\n 20240102T090000Z/PT1H"))
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
        panic!("accepted invalid free busy classification {source:?}");
    };
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| { issue.code().as_str() == "icalkit.validation.invalid-date-time-list" })
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn classifications_require_one_unquoted_ascii_token() {
    for value in [
        "",
        "\"FREE\"",
        "FREE,BUSY",
        "\"FREE,BUSY\"",
        "BUSY_TENTATIVE",
        "BUSY TENTATIVE",
        "BUSY/TENTATIVE",
        "BUSY+PRIVATE",
        "BUSY.PRIVATE",
        "BUSY^nTENTATIVE",
        "fr\u{00e9}e",
    ] {
        for declared in ["", ";VALUE=PERIOD"] {
            rejected(&freebusy(&format!("{declared};FBTYPE={value}")));
        }
    }
    rejected(&freebusy(";FBTYPE"));
}

#[test]
fn classifications_occur_at_most_once() {
    for parameters in [
        ";FBTYPE=FREE;FBTYPE=BUSY",
        ";FBTYPE=BUSY;fbtype=BUSY",
        ";FBTYPE=X-PRIVATE;FBTYPE=FREE",
    ] {
        rejected(&freebusy(parameters));
    }
}

#[test]
fn standard_classifications_and_absent_parameters_keep_their_bytes() {
    accepted(&freebusy(""));
    for value in ["FREE", "BUSY", "BUSY-TENTATIVE", "BUSY-UNAVAILABLE"] {
        accepted(&freebusy(&format!(";FBTYPE={value}")));
        accepted(&freebusy(&format!(
            ";fbTyPe={}",
            value.to_ascii_lowercase()
        )));
    }
}

#[test]
fn valid_extension_tokens_keep_their_spelling() {
    for value in ["X-PRIVATE", "FUTURE-BUSY", "future-2", "123", "-"] {
        accepted(&freebusy(&format!(
            ";FBTYPE={value};X-SOURCE=one;X-SOURCE=two"
        )));
    }
}

#[test]
fn classification_checks_use_unfolded_names_and_values() {
    accepted(&freebusy(";FB\r\n TYPE=BUSY-\r\n TENTATIVE"));
    rejected(&freebusy(";FB\r\n TYPE=BUSY_\r\n TENTATIVE"));
}

#[test]
fn unknown_semantics_keep_their_classification_metadata() {
    accepted(&calendar(
        "FREEBUSY;VALUE=X-FUTURE;FBTYPE=bad_token:opaque,,",
    ));
    accepted(&calendar(
        "X-FREEBUSY;FBTYPE=bad_token;FBTYPE=other:opaque,,",
    ));
    accepted(&freebusy(";X-FBTYPE=bad_token"));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque classifications//EN\r\n",
        "BEGIN:X-EXTENSION\r\nFREEBUSY;FBTYPE=bad_token;FBTYPE=other:opaque,,\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
