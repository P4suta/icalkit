// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Participation controls follow [RFC 5545 section 3.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2).
//! Attendee constraints follow [section 3.8.4.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.1).
//! Task participation includes [RFC 10051 section 9.1](https://www.rfc-editor.org/rfc/rfc10051.html#section-9.1).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, property: &str) -> String {
    let end = if kind == "VFREEBUSY" {
        "DTEND:20240101T100000Z\r\n"
    } else {
        ""
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit participation//EN\r\n\
         BEGIN:{kind}\r\nUID:participation@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{end}{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn attendee(parameters: &str) -> String {
    format!(
        "ATTENDEE{}:mailto:person@example.test",
        parameters.replace(';', ";\r\n ")
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
        panic!("accepted invalid participation control: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error() && issue.code().as_str() == "icalkit.validation.invalid-attendee-parameter"
    }));
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn rsvp_is_an_unquoted_boolean() {
    for value in [
        "YES",
        "0",
        "",
        "TRUE,FALSE",
        "\"TRUE\"",
        "TRUE^n",
        "true false",
    ] {
        rejected(&calendar("VEVENT", &attendee(&format!(";RSVP={value}"))));
    }
}

#[test]
fn scalar_participation_values_follow_the_token_grammar() {
    for name in ["CUTYPE", "ROLE", "PARTSTAT"] {
        for value in [
            "",
            "bad_token",
            "x,y",
            "\"ACCEPTED\"",
            "M\u{00e9}decin",
            "x y",
            "x^n",
        ] {
            rejected(&calendar("VEVENT", &attendee(&format!(";{name}={value}"))));
        }
    }
}

#[test]
fn scalar_controls_cannot_repeat_even_with_different_case() {
    for (name, value) in [
        ("RSVP", "TRUE"),
        ("CUTYPE", "INDIVIDUAL"),
        ("ROLE", "CHAIR"),
        ("PARTSTAT", "ACCEPTED"),
    ] {
        for repeated in [name.to_owned(), name.to_ascii_lowercase()] {
            rejected(&calendar(
                "VEVENT",
                &attendee(&format!(";{name}={value};{repeated}={value}")),
            ));
        }
    }
}

#[test]
fn registered_statuses_match_the_component() {
    for value in ["COMPLETED", "IN-PROCESS", "FAILED"] {
        rejected(&calendar(
            "VEVENT",
            &attendee(&format!(";PARTSTAT={value}")),
        ));
    }
    for value in [
        "TENTATIVE",
        "DELEGATED",
        "COMPLETED",
        "IN-PROCESS",
        "FAILED",
    ] {
        rejected(&calendar(
            "VJOURNAL",
            &attendee(&format!(";PARTSTAT={value}")),
        ));
    }
}

#[test]
fn published_values_and_case_variants_keep_their_bytes() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        accepted(&calendar(kind, &attendee("")));
        for value in ["TRUE", "FALSE", "tRuE", "false"] {
            accepted(&calendar(kind, &attendee(&format!(";rSvP={value}"))));
        }
        for value in ["INDIVIDUAL", "GROUP", "RESOURCE", "ROOM", "UNKNOWN"] {
            accepted(&calendar(kind, &attendee(&format!(";CUTYPE={value}"))));
        }
        for value in [
            "CHAIR",
            "REQ-PARTICIPANT",
            "OPT-PARTICIPANT",
            "NON-PARTICIPANT",
        ] {
            accepted(&calendar(kind, &attendee(&format!(";ROLE={value}"))));
        }
        for value in ["NEEDS-ACTION", "ACCEPTED", "DECLINED"] {
            accepted(&calendar(kind, &attendee(&format!(";PARTSTAT={value}"))));
        }
        accepted(&calendar(
            kind,
            "aTtEnDeE;VALUE=CAL-ADDRESS;CuTyPe=individual;RoLe=req-participant;\r\n \
             PaRtStAt=accepted;RsVp=false:mailto:Person@example.test",
        ));
    }
    for kind in ["VEVENT", "VTODO"] {
        for value in ["TENTATIVE", "DELEGATED"] {
            accepted(&calendar(kind, &attendee(&format!(";PARTSTAT={value}"))));
        }
    }
}

#[test]
fn task_participation_includes_failed() {
    for value in ["COMPLETED", "IN-PROCESS", "FAILED", "failed"] {
        accepted(&calendar("VTODO", &attendee(&format!(";PARTSTAT={value}"))));
    }
}

#[test]
fn controls_are_forbidden_on_freebusy_and_email_recipients() {
    accepted(&calendar("VFREEBUSY", &attendee("")));
    let email = |parameters: &str| {
        calendar(
            "VEVENT",
            &format!(
                "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT15M\r\n\
                 DESCRIPTION:Body\r\nSUMMARY:Subject\r\n{}\r\nEND:VALARM",
                attendee(parameters)
            ),
        )
    };
    accepted(&email(""));
    for parameters in [
        ";RSVP=FALSE",
        ";ROLE=CHAIR",
        ";CUTYPE=INDIVIDUAL",
        ";PARTSTAT=ACCEPTED",
    ] {
        rejected(&calendar("VFREEBUSY", &attendee(parameters)));
        rejected(&email(parameters));
    }
}

#[test]
fn extension_tokens_and_parameters_remain_lossless() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        accepted(&calendar(
            kind,
            &attendee(
                ";CUTYPE=X-PERSON;ROLE=FUTURE-ROLE;PARTSTAT=X-LATER;\
                 RSVP=TRUE;X-VENDOR=a_b;X-VENDOR=\"x,y\"",
            ),
        ));
    }
}

#[test]
fn opaque_values_and_components_keep_uninterpreted_controls() {
    let parameters = ";RSVP=YES;ROLE=bad_token;PARTSTAT=COMPLETED;CUTYPE=x,y";
    accepted(&calendar(
        "VEVENT",
        &attendee(&format!(";VALUE=X-FUTURE{parameters}")),
    ));
    accepted(&calendar("X-FUTURE", &attendee(parameters)));
    accepted(&calendar(
        "VEVENT",
        &attendee(parameters).replacen("ATTENDEE", "X-ATTENDEE", 1),
    ));
}
