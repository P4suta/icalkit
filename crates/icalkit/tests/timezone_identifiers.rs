// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Query definitions use [RFC 5545 section 3.8.3.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.3.1) identifiers.

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, IcalDateTime, Timestamp};
use icalkit::{Calendar, Engine};

fn calendar(properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit zone identity//EN\r\n\
         {properties}END:VCALENDAR\r\n"
    )
}

fn event(start: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:identity@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {start}\r\nDURATION:PT1H\r\nEND:VEVENT\r\n"
    )
}

fn request(identifier: &str, extra: &str) -> String {
    let timezone = calendar(&format!(
        "BEGIN:VTIMEZONE\r\n{identifier}\r\nBEGIN:STANDARD\r\n\
         DTSTART:20000101T000000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0200\r\n\
         END:STANDARD\r\nEND:VTIMEZONE\r\n{extra}"
    ));
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="20240102T070000Z" end="20240102T073000Z"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240102T070000Z" end="20240102T073000Z"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{timezone}]]></C:timezone></C:calendar-query>"#
    )
}

fn assert_projects(identifier: &str, extra: &str) {
    let query = Query::parse(request(identifier, extra).as_bytes()).unwrap();
    let engine = Engine::default();
    for (properties, expected) in [
        (
            "DTSTART:20240102T090000",
            IcalDateTime::floating("2024-01-02T09:00:00".parse::<DateTime>().unwrap()),
        ),
        (
            "DTSTART:20240102T090000\r\nRRULE:FREQ=DAILY;COUNT=2",
            IcalDateTime::utc("2024-01-02T07:00:00Z".parse::<Timestamp>().unwrap()),
        ),
    ] {
        let source = calendar(&event(properties));
        let calendar = Calendar::parse(source.as_bytes()).unwrap();
        let mut session = engine.session();
        assert_eq!(
            query.matches(&mut session, &calendar).unwrap(),
            Match::Matched
        );
        let projection = query.project(&mut session, &calendar).unwrap();
        let projected = Calendar::parse(projection.as_bytes()).unwrap();
        let starts: Vec<_> = projected
            .events()
            .map(|event| event.dtstart().unwrap())
            .collect();
        assert_eq!(starts, [expected]);
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn query_definitions_match_escaped_names_to_quoted_references() {
    for (property, parameter) in [
        (r"TZID:Test/Comma\,Zone", "\"Test/Comma,Zone\""),
        (r"TZID:Test/Semi\;Zone", "\"Test/Semi;Zone\""),
        (r"TZID:Test/Back\\Slash", r"Test/Back\Slash"),
    ] {
        let extra = event(&format!("DTSTART;TZID={parameter}:20240102T090000"));
        let source = calendar(&format!(
            "BEGIN:VTIMEZONE\r\n{property}\r\nBEGIN:STANDARD\r\n\
             DTSTART:20000101T000000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0200\r\n\
             END:STANDARD\r\nEND:VTIMEZONE\r\n{extra}"
        ));
        assert_eq!(
            Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
            source.as_bytes()
        );
        assert_projects(property, &extra);
    }
}

#[test]
fn literal_quotes_in_query_definition_names_are_not_parameter_delimiters() {
    for property in ["TZID:\"\"", "TZID:\"Test/Zone\"", "TZID;VALUE=TEXT:\"\""] {
        assert_projects(property, "");
    }
    let extra = event("DTSTART;TZID=Test/Zone:20240102T090000");
    assert!(Query::parse(request("TZID:\"Test/Zone\"", &extra).as_bytes()).is_err());
}

#[test]
fn zoned_event_values_decode_parameter_escapes_once() {
    for (parameter, expected) in [
        ("^'Test/Zone^'", "\"Test/Zone\""),
        ("\"Test/^^n,Zone\"", "Test/^n,Zone"),
        ("Test/^x", "Test/^x"),
    ] {
        let source = calendar(&event(&format!("DTSTART;TZID={parameter}:20240102T090000")));
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        let value = parsed.events().next().unwrap().dtstart().unwrap();
        assert_eq!(value.as_zoned().unwrap().1, expected);
        assert_eq!(parsed.to_bytes(), source.as_bytes());
    }
}

#[test]
fn opaque_query_definition_names_do_not_supply_a_zone() {
    for property in [
        "TZID;VALUE=X-OPAQUE:Test/Zone",
        "TZID;VALUE=X-OPAQUE:\"\"",
        "TZID;VALUE=URI:https://example.test/zone",
    ] {
        let result = Query::parse(request(property, "").as_bytes());
        assert!(result.is_err(), "accepted opaque identity: {property}");
    }
}
