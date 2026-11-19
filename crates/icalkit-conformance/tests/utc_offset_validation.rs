// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Offset syntax follows [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.14).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::caldav::Query;
use icalkit::interop::Import;
use icalkit_conformance::internal::core::{DecodeValue as _, UtcOffset};

fn calendar(kind: &str, property: &str) -> String {
    let other = if property.starts_with("TZOFFSETFROM") {
        "TZOFFSETTO:+0100"
    } else {
        "TZOFFSETFROM:+0100"
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit offset validation//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Offset\r\nBEGIN:{kind}\r\n\
         DTSTART:20200101T000000\r\n{other}\r\n{property}\r\n\
         END:{kind}\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn query(source: &str) -> Result<Query, icalkit::Error> {
    Query::parse(
        format!(
            r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240101T000000Z" end="20240102T000000Z"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{source}]]></C:timezone>
</C:calendar-query>"#
        )
        .as_bytes(),
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
    let error = Calendar::parse(source.as_bytes()).expect_err("invalid UTC offset");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert!(import.validate().is_err());
    assert_eq!(import.as_bytes(), source.as_bytes());
}

#[test]
fn offset_decoding_rejects_a_seconds_field_of_sixty() {
    for value in [
        "+000060", "-000060", "+005960", "-005960", "+235960", "-235960",
    ] {
        assert!(
            UtcOffset::decode_value(value.as_bytes()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn offset_decoding_accepts_valid_second_boundaries() {
    for (value, seconds) in [
        ("+000000", 0),
        ("+000001", 1),
        ("-000001", -1),
        ("+005959", 3_599),
        ("-005959", -3_599),
        ("+235959", 86_399),
        ("-235959", -86_399),
    ] {
        assert_eq!(
            UtcOffset::decode_value(value.as_bytes()).unwrap().seconds(),
            seconds
        );
    }
}

#[test]
fn observance_offsets_require_valid_signed_hour_minute_and_second_fields() {
    for kind in ["STANDARD", "DAYLIGHT"] {
        for name in ["TZOFFSETFROM", "TZOFFSETTO"] {
            for value in [
                "",
                "0100",
                "Z",
                "+010",
                "+01:00",
                "+2400",
                "+0060",
                "+000060",
                "-000060",
                "-0000",
                "-000000",
                "+0100 ",
                "+0100,+0200",
                "+000061",
            ] {
                rejected(&calendar(kind, &format!("{name}:{value}")));
            }
        }
    }
}

#[test]
fn valid_offset_alternatives_keep_their_bytes() {
    for kind in ["STANDARD", "DAYLIGHT"] {
        for name in ["TZOFFSETFROM", "TZOFFSETTO"] {
            for value in ["+0000", "+000000", "+0130", "-013000", "+235959", "-235959"] {
                accepted(&calendar(kind, &format!("{name}:{value}")));
            }
            accepted(&calendar(
                kind,
                &format!("{name};VaLuE=UTC-OFFSET;X-SOURCE=example:+013001"),
            ));
        }
    }
    accepted(&calendar("STANDARD", "tzoffsetto:+0100"));
}

#[test]
fn observance_offsets_reject_incompatible_and_repeated_value_types() {
    for name in ["TZOFFSETFROM", "TZOFFSETTO"] {
        for parameters in [
            "VALUE=INTEGER",
            "VALUE=TEXT",
            "VALUE=DATE-TIME",
            "VALUE=UTC-OFFSET;VALUE=UTC-OFFSET",
            "VALUE=X-FUTURE;VALUE=UTC-OFFSET",
        ] {
            rejected(&calendar("STANDARD", &format!("{name};{parameters}:+0100")));
        }
    }
}

#[test]
fn opaque_offset_types_survive_promotion_without_defining_a_query_zone() {
    for name in ["TZOFFSETFROM", "TZOFFSETTO"] {
        let source = calendar("STANDARD", &format!("{name};VALUE=X-FUTURE:opaque"));
        accepted(&source);
        assert!(query(&source).is_err());
    }
}

#[test]
fn inline_query_zones_use_the_strict_offset_domain() {
    for name in ["TZOFFSETFROM", "TZOFFSETTO"] {
        for value in ["+000060", "-000060", "+005960", "-005960"] {
            assert!(query(&calendar("STANDARD", &format!("{name}:{value}"))).is_err());
        }
        for parameters in ["VALUE=TEXT", "VALUE=UTC-OFFSET;VALUE=UTC-OFFSET"] {
            assert!(query(&calendar("STANDARD", &format!("{name};{parameters}:+0100"))).is_err());
        }
        for value in ["+000000", "+005959", "-005959"] {
            assert!(query(&calendar("STANDARD", &format!("{name}:{value}"))).is_ok());
        }
    }
}

#[test]
fn unrecognized_components_and_properties_keep_their_offset_content() {
    let source = calendar("STANDARD", "TZOFFSETTO:+0100").replace(
        "DTSTART:20200101T000000\r\n",
        "DTSTART:20200101T000000\r\nX-TZOFFSETTO:invalid\r\n",
    );
    accepted(&source);
    accepted(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque offsets//EN\r\n",
        "BEGIN:X-EXTENSION\r\nTZOFFSETFROM:invalid\r\nTZOFFSETTO:+000060\r\n",
        "END:X-EXTENSION\r\nEND:VCALENDAR\r\n",
    ));
}
