// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Range changes follow [RFC 5545 section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4).

#![cfg(test)]

use icalkit::caldav::Query;
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};
use icalkit_conformance::internal::core::{Component, Document, Limits};

struct UtcZone;

impl ZoneDatabase for UtcZone {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        if tzid != "Test/UTC" {
            return None;
        }
        let instant = format!("{local}Z").parse::<Timestamp>().ok()?;
        Some(ZoneResolution::exact(instant, "test-utc", true))
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "Test/UTC")
            .then(|| OffsetResolution::new(0, "test-utc", true))
            .flatten()
    }
}

fn event(properties: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:range@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {properties}END:VEVENT\r\n"
    )
}

fn calendar(events: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ranges//EN\r\n\
         {events}END:VCALENDAR\r\n"
    )
}

fn project(source: &str, start: &str, end: &str) -> Document {
    let timezone = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit UTC query zone//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Test/UTC\r\nBEGIN:STANDARD\r\n",
        "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0000\r\n",
        "END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    let query = Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
<C:timezone>{timezone}</C:timezone></C:calendar-query>"#
        )
        .as_bytes(),
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(UtcZone).build();
    let mut session = engine.session();
    let projected = query.project(&mut session, &original).unwrap();
    Document::parse(projected.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap()
}

fn text<'a>(component: &'a Component, name: &[u8]) -> Option<&'a str> {
    let property = component
        .properties()
        .find(|property| property.is_named(name))?;
    std::str::from_utf8(property.value_text().as_bytes()).ok()
}

fn assert_shifted_addition(rdate: &str) {
    for (hour, window_end_hour, end_hour) in [(7, 8, 9), (11, 12, 13)] {
        let source = calendar(&format!(
            "{}{}",
            event(&format!(
                "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\n\
                 RRULE:FREQ=DAILY;COUNT=2\r\n{rdate}\r\n"
            )),
            event(&format!(
                "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\n\
                 DTSTART:20240102T{hour:02}0000Z\r\nDURATION:PT2H\r\n"
            )),
        ));
        let document = project(
            &source,
            &format!("20240103T{hour:02}3000Z"),
            &format!("20240103T{window_end_hour:02}3000Z"),
        );
        let root = document.components().next().unwrap();
        let events: Vec<&Component> = root.components().collect();
        assert_eq!(events.len(), 1);
        let instance = events[0];
        assert_eq!(
            text(instance, b"DTSTART"),
            Some(format!("20240103T{hour:02}0000Z").as_str())
        );
        assert_eq!(
            text(instance, b"DTEND"),
            Some(format!("20240103T{end_hour:02}0000Z").as_str())
        );
        assert_eq!(text(instance, b"RECURRENCE-ID"), Some("20240103T090000Z"));
    }
}

#[test]
fn range_shifts_apply_to_date_time_additions_in_both_directions() {
    assert_shifted_addition("RDATE:20240103T090000Z");
}

#[test]
fn range_shifts_and_lengths_apply_to_period_additions_in_both_directions() {
    assert_shifted_addition("RDATE;VALUE=PERIOD:20240103T090000Z/PT3H");
}

#[test]
fn range_shifts_apply_to_date_additions() {
    let source = calendar(&format!(
        "{}{}",
        event(
            "DTSTART;VALUE=DATE:20240101\r\nDURATION:P1D\r\n\
             RRULE:FREQ=DAILY;COUNT=2\r\nRDATE;VALUE=DATE:20240104\r\n"
        ),
        event(
            "RECURRENCE-ID;VALUE=DATE;RANGE=THISANDFUTURE:20240102\r\n\
             DTSTART;VALUE=DATE:20240103\r\nDURATION:P2D\r\n"
        ),
    ));
    let document = project(&source, "20240105T000000Z", "20240106T000000Z");
    let root = document.components().next().unwrap();
    let instance = root.components().next().unwrap();
    assert_eq!(text(instance, b"DTSTART"), Some("20240105T000000Z"));
    assert_eq!(text(instance, b"DTEND"), Some("20240107T000000Z"));
    assert_eq!(text(instance, b"RECURRENCE-ID"), Some("20240104T000000Z"));
}

#[test]
fn exact_components_do_not_inherit_earlier_range_properties() {
    for exact_start in ["20240103T150000Z", "20240101T070000Z"] {
        let source = calendar(&format!(
            "{}{}{}",
            event(
                "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\n\
                 RRULE:FREQ=DAILY;COUNT=4\r\nLOCATION:Base\r\n"
            ),
            event(
                "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\n\
                 DTSTART:20240102T110000Z\r\nDURATION:PT2H\r\nLOCATION:Changed\r\n"
            ),
            event(&format!(
                "RECURRENCE-ID:20240103T090000Z\r\nDTSTART:{exact_start}\r\n\
                 DURATION:PT1H\r\nSUMMARY:Exact\r\n"
            )),
        ));
        let document = project(&source, "20240101T000000Z", "20240105T000000Z");
        let root = document.components().next().unwrap();
        let exact = root
            .components()
            .find(|component| text(component, b"RECURRENCE-ID") == Some("20240103T090000Z"))
            .unwrap();
        assert_eq!(text(exact, b"DTSTART"), Some(exact_start));
        assert_eq!(text(exact, b"LOCATION"), Some("Base"));
        assert_eq!(text(exact, b"SUMMARY"), Some("Exact"));
        let later = root
            .components()
            .find(|component| text(component, b"RECURRENCE-ID") == Some("20240104T090000Z"))
            .unwrap();
        assert_eq!(text(later, b"LOCATION"), Some("Changed"));
        assert_eq!(text(later, b"DTSTART"), Some("20240104T110000Z"));
    }
}
