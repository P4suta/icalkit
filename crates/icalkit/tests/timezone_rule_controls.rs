// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Query time zones enforce [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};

struct NoExternalZones;

impl ZoneDatabase for NoExternalZones {
    fn resolve_local(&self, _tzid: &str, _local: DateTime) -> Option<ZoneResolution> {
        None
    }

    fn offset_at(&self, _tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        None
    }
}

fn calendar() -> Calendar {
    Calendar::parse(
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule controls//EN\r\n\
          BEGIN:VEVENT\r\nUID:controls@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
          DTSTART:20250308T090000\r\nRRULE:FREQ=DAILY;COUNT=3\r\n\
          END:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .unwrap()
}

fn fold_line(text: &str) -> String {
    assert!(text.is_ascii());
    text.as_bytes()
        .chunks(74)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n ")
}

fn request(extra: &str, start: &str, end: &str) -> String {
    let daylight = fold_line(&format!("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU{extra}"));
    let standard = fold_line(&format!("RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU{extra}"));
    let zone = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule controls//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/RuleControls\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:20240310T020000\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n\
         {daylight}\r\nEND:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\nDTSTART:20241103T020000\r\n\
         TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
         {standard}\r\nEND:STANDARD\r\n\
         END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{zone}]]></C:timezone></C:calendar-query>"#
    )
}

fn projected_starts(extra: &str) -> Vec<Timestamp> {
    let calendar = calendar();
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let query =
        Query::parse(request(extra, "20250308T000000Z", "20250311T000000Z").as_bytes()).unwrap();
    let projected = query.project(&mut engine.session(), &calendar).unwrap();
    assert_eq!(calendar.to_bytes(), original);
    Calendar::parse(projected.as_bytes())
        .unwrap()
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

#[test]
fn padded_unit_intervals_project_the_spring_transition() {
    for padding in 0..=32 {
        let extra = format!(";INTERVAL={}1", "0".repeat(padding));
        assert_eq!(
            projected_starts(&extra),
            [
                "2025-03-08T14:00:00Z",
                "2025-03-09T13:00:00Z",
                "2025-03-10T13:00:00Z"
            ]
            .map(|value| value.parse::<Timestamp>().unwrap()),
            "{extra}"
        );
    }
}

#[test]
fn padded_unit_intervals_keep_query_windows_on_the_transition() {
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let calendar = calendar();
    for (start, end, expected) in [
        ("20250309T125500Z", "20250309T130500Z", Match::Matched),
        ("20250309T135500Z", "20250309T140500Z", Match::Unmatched),
    ] {
        let query = Query::parse(request(";INTERVAL=0001", start, end).as_bytes()).unwrap();
        assert_eq!(
            query.matches(&mut engine.session(), &calendar).unwrap(),
            expected
        );
    }
}

#[test]
fn valid_week_starts_keep_the_same_projection() {
    let expected = projected_starts("");
    for weekday in ["MO", "TU", "WE", "TH", "FR", "SA", "SU"] {
        for value in [weekday.to_owned(), weekday.to_ascii_lowercase()] {
            assert_eq!(projected_starts(&format!(";WKST={value}")), expected);
        }
    }
}

#[test]
fn malformed_controls_cannot_enter_a_query_plan() {
    for extra in [
        ";FREQ=YEARLY",
        ";INTERVAL=1;interval=1",
        ";WKST=MO;wkst=SU",
        ";WKST=invalid",
        ";WKST=",
        ";UNTIL=20260308",
        ";UNTIL=20260308T020000",
        ";UNTIL=20260308T070000Z;until=20250309T070000Z",
    ] {
        let parsed =
            Query::parse(request(extra, "20250308T000000Z", "20250311T000000Z").as_bytes());
        assert!(parsed.is_err(), "{extra}: {parsed:?}");
    }
}
