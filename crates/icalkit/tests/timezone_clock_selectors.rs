// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Public queries use complete annual observance clock selections.

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn calendar(year: u16) -> Calendar {
    Calendar::parse(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit clocks//EN\r\nBEGIN:VEVENT\r\nUID:clocks@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:{year}0501T090000\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").as_bytes()).unwrap()
}

fn query(parts: &str, from: &str, until: &str) -> Query {
    let zone = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit clocks//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/Clocks\r\nBEGIN:DAYLIGHT\r\nDTSTART:20240501T010000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;BYMONTH=5;BYMONTHDAY=1,2;BYHOUR=1,3;{parts}\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\nDTSTART:20240501T030000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nRRULE:FREQ=YEARLY;BYMONTH=5;BYMONTHDAY=1,2;BYHOUR=3,4;BYSETPOS=1;COUNT=3\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    Query::parse(format!(r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{from}" end="{until}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{from}" end="{until}"/></C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{zone}]]></C:timezone></C:calendar-query>"#).as_bytes()).unwrap()
}

#[test]
fn matching_and_projection_observe_the_selected_clock_after_a_reset() {
    let calendar = calendar(2025);
    let original = calendar.to_bytes();
    let query = query(
        "BYSETPOS=1,-1;COUNT=4",
        "20250502T080000Z",
        "20250502T080001Z",
    );
    let engine = Engine::builder().build();
    assert_eq!(
        query.matches(&mut engine.session(), &calendar).unwrap(),
        Match::Matched
    );
    let projected = query.project(&mut engine.session(), &calendar).unwrap();
    let expanded = Calendar::parse(projected.as_bytes()).unwrap();
    let starts: Vec<_> = expanded
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect();
    assert_eq!(
        starts,
        vec!["2025-05-02T08:00:00Z".parse::<Timestamp>().unwrap()]
    );
    assert_eq!(calendar.to_bytes(), original);
}

#[test]
fn a_shared_count_stops_all_clock_patterns_together() {
    let second = query(
        "BYSETPOS=1,-1;COUNT=2",
        "20240502T080000Z",
        "20240502T080001Z",
    );
    let projected = second
        .project(&mut Engine::default().session(), &calendar(2024))
        .unwrap();
    let expanded = Calendar::parse(projected.as_bytes()).unwrap();
    let starts: Vec<_> = expanded
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect();
    assert_eq!(
        starts,
        ["2024-05-02T08:00:00Z".parse::<Timestamp>().unwrap()]
    );
    let calendar = calendar(2025);
    let query = query(
        "BYSETPOS=1,-1;COUNT=2",
        "20250502T090000Z",
        "20250502T090001Z",
    );
    let engine = Engine::builder().build();
    assert_eq!(
        query.matches(&mut engine.session(), &calendar).unwrap(),
        Match::Matched
    );
    let projected = query.project(&mut engine.session(), &calendar).unwrap();
    let expanded = Calendar::parse(projected.as_bytes()).unwrap();
    assert_eq!(
        expanded
            .events()
            .next()
            .unwrap()
            .dtstart()
            .unwrap()
            .as_utc()
            .unwrap(),
        "2025-05-02T09:00:00Z".parse::<Timestamp>().unwrap()
    );
}
