// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Alternate representations follow [RFC 5545 section 3.2.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.1).
//! Contact URI commas follow [verified erratum 7691](https://www.rfc-editor.org/errata/eid7691/).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

const TEXT_PROPERTIES: &[&str] = &[
    "COMMENT",
    "CONTACT",
    "DESCRIPTION",
    "LOCATION",
    "RESOURCES",
    "SUMMARY",
];

fn calendar(property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit alternate text//EN\r\n\
         BEGIN:VEVENT\r\nUID:alternate@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\n{property}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.validate().unwrap().to_bytes(), source.as_bytes());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

fn rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid alternate representation {source:?}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(error.issues().iter().any(|issue| {
        issue.code().as_str() == "icalkit.validation.invalid-alternate-representation"
    }));
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn alternate_representations_require_a_quoted_uri() {
    for name in TEXT_PROPERTIES {
        for parameter in [
            "ALTREP=relative",
            "ALTREP=\"/relative\"",
            "ALTREP=\"\"",
            "ALTREP=\"mailto:bad%2\"",
            "ALTREP=\"https://[broken]/text\"",
            "ALTREP=\"https://example.test/\u{00e9}\"",
            "ALTREP=\"https://example.test/a b\"",
        ] {
            rejected(&calendar(&format!("{name};{parameter}:Original")));
        }
    }
}

#[test]
fn alternate_representations_occur_at_most_once() {
    for name in TEXT_PROPERTIES {
        for parameters in [
            "ALTREP=\"urn:one\";ALTREP=\"urn:one\"",
            "ALTREP=\"urn:one\";alTrEp=\"urn:two\"",
        ] {
            rejected(&calendar(&format!("{name};{parameters}:Original")));
        }
    }
}

#[test]
fn alternate_representations_are_single_values_rather_than_uri_lists() {
    for name in TEXT_PROPERTIES {
        rejected(&calendar(&format!(
            "{name};ALTREP=\"urn:one\",\"urn:two\":Original"
        )));
    }
}

#[test]
fn uri_schemes_case_and_unknown_parameters_keep_their_bytes() {
    for name in TEXT_PROPERTIES {
        for uri in [
            "CID:part.1@example.test",
            "https://example.test/a,b?view=one;two#part",
            "urn:example:text",
            "x-text:opaque%2Cvalue",
            "https://[2001:db8::1]/text",
            "https://example.test/%C3%A9",
        ] {
            accepted(&calendar(&format!(
                "{name};aLtReP=\"{uri}\";\r\n X-SOURCE=one;X-SOURCE=two:Original"
            )));
        }
    }
}

#[test]
fn alternate_representations_use_unfolded_parameter_text() {
    accepted(&calendar(
        "SUMMARY;ALTR\r\n EP=\"https://example.test/a%2\r\n Ftext\":Original",
    ));
    rejected(&calendar(
        "SUMMARY;ALTR\r\n EP=\"https://example.test/a%2\r\n Gtext\":Original",
    ));
}

#[test]
fn contact_uris_keep_commas_without_text_escapes() {
    accepted(&calendar(
        "CONTACT;ALTREP=\"ldap://example.test:6666/o=ABC%20Industries,\r\n c=US???(cn=Jim%20Dolittle)\":Contact",
    ));
    rejected(&calendar(
        "CONTACT;ALTREP=\"ldap://example.test:6666/o=ABC%20Industries\\,\r\n c=US???(cn=Jim%20Dolittle)\":Contact",
    ));
}

#[test]
fn opaque_values_properties_and_components_keep_their_metadata() {
    for name in TEXT_PROPERTIES {
        accepted(&calendar(&format!(
            "{name};VALUE=X-FUTURE;ALTREP=relative;ALTREP=other:opaque;data,\\q\\"
        )));
    }
    accepted(&calendar(
        "X-TEXT;ALTREP=relative;ALTREP=other:opaque;data,\\q\\",
    ));
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque text//EN\r\n",
        "BEGIN:X-EXTENSION\r\nSUMMARY;ALTREP=relative;ALTREP=other:opaque;data,\\q\\\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}

#[test]
fn alternate_representation_checks_reach_alarm_descriptions() {
    let alarm = |uri: &str| {
        calendar(&format!(
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT1H\r\n\
             DESCRIPTION;ALTREP=\"{uri}\":Reminder\r\nEND:VALARM"
        ))
    };
    accepted(&alarm("urn:example:reminder"));
    rejected(&alarm("relative"));
}
