// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Address parameters follow [RFC 5545 section 3.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2).
//! Agent addresses follow [RFC 6068 section 2](https://www.rfc-editor.org/rfc/rfc6068.html#section-2).

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

fn party(name: &str, parameters: &str) -> String {
    fold(&format!("{name}{parameters}:mailto:person@example.test"))
}

fn calendar(kind: &str, property: &str) -> String {
    let end = if kind == "VFREEBUSY" {
        "DTEND:20240101T100000Z\r\n"
    } else {
        ""
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit address parameters//EN\r\n\
         BEGIN:{kind}\r\nUID:address-parameters@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{end}{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
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
        panic!("accepted invalid address parameter: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error() && issue.code().as_str() == "icalkit.validation.invalid-address-parameter"
    }));
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

fn agent(value: &str) -> String {
    calendar(
        "VEVENT",
        &party("ORGANIZER", &format!(";SENT-BY=\"{value}\"")),
    )
}

#[test]
fn parameter_addresses_require_quotes_and_valid_uris() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO", "DIR", "SENT-BY"] {
        for value in [
            "relative",
            "\"\"",
            "\"person@example.test\"",
            "\"https://host:bad/people\"",
            "\"https://[::1/people\"",
            "\"https://example.test/%GG\"",
            "\"https://example.test/a b\"",
            "\"https://example.test/\u{00e9}\"",
        ] {
            rejected(&calendar(
                "VEVENT",
                &party("ATTENDEE", &format!(";{name}={value}")),
            ));
        }
    }
}

#[test]
fn quoted_address_lists_validate_every_field_and_separator() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO"] {
        for value in [
            "\"mailto:first@example.test\",\"relative\"",
            "\"relative\",\"mailto:last@example.test\"",
            "\"mailto:first@example.test\",\"\"",
            "\"mailto:first@example.test\",",
            ",\"mailto:first@example.test\"",
            "\"mailto:first@example.test\",,\"mailto:last@example.test\"",
            "\"mailto:first@example.test\" \"mailto:last@example.test\"",
        ] {
            rejected(&calendar(
                "VEVENT",
                &party("ATTENDEE", &format!(";{name}={value}")),
            ));
        }
    }
}

#[test]
fn lists_and_uri_delimiters_preserve_their_bytes() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO"] {
        for value in [
            "\"mailto:first@example.test\"",
            "\"mailto:first@example.test\",\"urn:uuid:group-A\"",
            "\"https://example.test/people/a,b;version=one\",\"urn:group:last\"",
        ] {
            for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
                accepted(&calendar(
                    kind,
                    &party("ATTENDEE", &format!(";{name}={value}")),
                ));
            }
        }
    }
    for name in ["ATTENDEE", "ORGANIZER"] {
        accepted(&calendar(
            "VEVENT",
            &party(
                name,
                ";dIr=\"ldap://example.test/o=People,c=US???(cn=Person)\";\
                          sEnT-bY=\"MAILTO:Agent+Tag@Example.Test\"",
            ),
        ));
    }
}

#[test]
fn directory_and_agent_parameters_are_single_values() {
    for name in ["ATTENDEE", "ORGANIZER"] {
        for parameter in ["DIR", "SENT-BY"] {
            rejected(&calendar(
                "VEVENT",
                &party(
                    name,
                    &format!(
                        ";{parameter}=\"mailto:first@example.test\",\"mailto:last@example.test\""
                    ),
                ),
            ));
        }
    }
}

#[test]
fn address_parameters_cannot_repeat() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO", "DIR", "SENT-BY"] {
        let lower = name.to_ascii_lowercase();
        for repeated in [name, lower.as_str()] {
            rejected(&calendar(
                "VEVENT",
                &party(
                    "ATTENDEE",
                    &format!(
                        ";{name}=\"mailto:first@example.test\";{repeated}=\"mailto:last@example.test\""
                    ),
                ),
            ));
        }
    }
    for name in ["DIR", "SENT-BY"] {
        rejected(&calendar(
            "VEVENT",
            &party(
                "ORGANIZER",
                &format!(
                    ";{name}=\"mailto:first@example.test\";{name}=\"mailto:last@example.test\""
                ),
            ),
        ));
    }
}

#[test]
fn agent_addresses_require_the_mailto_scheme() {
    for value in [
        "https://example.test/agent",
        "urn:uuid:agent",
        "data:text/plain,agent",
    ] {
        rejected(&agent(value));
    }
}

#[test]
fn agent_mailboxes_require_current_address_syntax() {
    for value in [
        "mailto:person",
        "mailto:@example.test",
        "mailto:person@",
        "mailto:a@@example.test",
        "mailto:.person@example.test",
        "mailto:person.@example.test",
        "mailto:a..b@example.test",
        "mailto:person@.example.test",
        "mailto:person@example..test",
        "mailto:person@example.test.",
        "mailto://person@example.test",
        "mailto:a;b@example.test",
        "mailto:person(comment)@example.test",
        "mailto:a%00b@example.test",
        "mailto:person@exa%20mple.test",
        "mailto:%22unterminated@example.test",
        "mailto:%22a%22b@example.test",
        "mailto:%22a%0Ab%22@example.test",
        "mailto:%22a%20b%22@example.test",
        "mailto:person@%5Bunterminated",
        "mailto:person@%5Ba%5Cb%5D",
    ] {
        rejected(&agent(value));
    }
}

#[test]
fn agent_uri_fields_follow_the_mailto_grammar() {
    for value in [
        "mailto:person@example.test?subject",
        "mailto:person@example.test?",
        "mailto:person@example.test?subject=x&",
        "mailto:person@example.test?subject=x&&body=y",
        "mailto:person@example.test?subject=x?body=y",
        "mailto:person@example.test?subject=x=y",
        "mailto:person@example.test?subject=a/b",
        "mailto:person@example.test?body=%FF",
    ] {
        rejected(&agent(value));
    }
}

#[test]
fn agent_percent_escapes_decode_once_as_utf8() {
    for value in [
        "mailto:person@%FF.example",
        "mailto:%C0%AF@example.test",
        "mailto:person@%ED%A0%80.example",
        "mailto:%F4%90%80%80@example.test",
        "mailto:person@%E2%82.example",
        "mailto:person@example.test?body=%E2%28%A1",
    ] {
        rejected(&agent(value));
    }
    for value in [
        "mailto:person@%E7%94%BB%E5%83%8F.example",
        "mailto:%E7%94%BB%E5%83%8F@example.test",
        "mailto:person@example.test?subject=%C3%A9&body=line%0D%0Aline",
        "mailto:a%2525@example.test",
    ] {
        accepted(&agent(value));
    }
}

#[test]
fn rfc_mailto_examples_and_quoted_addresses_are_admitted() {
    for value in [
        "mailto:chris@example.com",
        "mailto:infobot@example.com?subject=current-issue",
        "mailto:joe@example.com?cc=bob@example.com&body=hello",
        "mailto:addr1@example.test,addr2@example.test",
        "mailto:?to=addr1@example.test",
        "mailto:",
        "mailto:person@example.test#opaque",
        "mailto:gorby%25kremvax@example.com",
        "mailto:unlikely%3Faddress@example.com?blat=foop",
        "mailto:Mike%26family@example.org",
        "mailto:%22not%40me%22@example.org",
        "mailto:%22oh%5C%5Cno%22@example.org",
        "mailto:%22%5C%5C%5C%22it's%5C%20ugly%5C%5C%5C%22%22@example.org",
        "mailto:%22a,b@c%22@example.test",
        "mailto:person@%5BIPv6:2001:db8::1%5D",
    ] {
        accepted(&agent(value));
    }
}

#[test]
fn verified_mailto_corrections_allow_path_subdelimiters() {
    for value in [
        "mailto:Mike&family@example.org",
        "mailto:a=b@example.test",
        "mailto:%22a;b=c&d%22@example.test",
        "mailto:Mike&family@example.org?subject=Use%20of%20%26&body=mailbox",
    ] {
        accepted(&agent(value));
    }
}

#[test]
fn group_addresses_are_forbidden_on_email_and_freebusy_attendees() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO", "DIR", "SENT-BY"] {
        let property = party(
            "ATTENDEE",
            &format!(";{name}=\"mailto:agent@example.test\""),
        );
        rejected(&calendar("VFREEBUSY", &property));
        rejected(&calendar(
            "VEVENT",
            &format!(
                "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT15M\r\n\
             DESCRIPTION:Body\r\nSUMMARY:Subject\r\n{property}\r\nEND:VALARM"
            ),
        ));
    }
    accepted(&calendar(
        "VFREEBUSY",
        &party(
            "ORGANIZER",
            ";DIR=\"urn:group:one\";SENT-BY=\"mailto:agent@example.test\"",
        ),
    ));
}

#[test]
fn organizer_parameters_follow_the_organizer_definition() {
    for name in ["MEMBER", "DELEGATED-FROM", "DELEGATED-TO"] {
        rejected(&calendar(
            "VEVENT",
            &party(
                "ORGANIZER",
                &format!(";{name}=\"mailto:person@example.test\""),
            ),
        ));
    }
}

#[test]
fn opaque_values_and_components_keep_address_parameters() {
    let parameters = ";DIR=\"relative\";SENT-BY=\"urn:agent:one\";MEMBER=\"opaque\"";
    for name in ["ORGANIZER", "ATTENDEE"] {
        accepted(&calendar(
            "VEVENT",
            &party(name, &format!(";VALUE=X-FUTURE{parameters}")),
        ));
        accepted(&calendar("X-FUTURE", &party(name, parameters)));
    }
    accepted(&calendar("VEVENT", &party("X-ATTENDEE", parameters)));
    accepted(&calendar(
        "VEVENT",
        &party("ATTENDEE", ";X-AGENT=\"relative\";X-AGENT=\"opaque\""),
    ));
}
