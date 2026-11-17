// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedded transitions follow [RFC 5545 section 3.6.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.5).
//! Ordinal scope follows [verified erratum 1913](https://errata.rfc-editor.org/eid1913).
//! Queries select their zone through [RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8).

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

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn zone(
    daylight_start: &str,
    daylight_rule: &str,
    standard_start: &str,
    standard_rule: &str,
) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ordinal workflow//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Ordinal\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{daylight_start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{daylight_rule}\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\n\
         DTSTART:{standard_start}\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\n\
         RRULE:FREQ=YEARLY;{standard_rule}\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn calendar(start: &str) -> Calendar {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ordinal workflow//EN\r\n\
         BEGIN:VEVENT\r\nUID:ordinal@example.test\r\nDTSTAMP:19950101T000000Z\r\n\
         DTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let calendar = Calendar::parse(text.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), text.as_bytes());
    calendar
}

fn year_zone() -> String {
    zone(
        "19950129T020000",
        "BYDAY=5SU",
        "19951029T030000",
        "BYMONTH=10;BYDAY=-1SU",
    )
}

fn query(timezone: &str, start: &str, end: &str) -> Query {
    let text = format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{timezone}]]></C:timezone></C:calendar-query>"#
    );
    Query::parse(text.as_bytes()).unwrap()
}

fn starts(calendar: &Calendar, timezone: &str, start: &str, end: &str) -> Vec<Timestamp> {
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    let before = calendar.to_bytes();
    let projection = query(timezone, start, end)
        .project(&mut session, calendar)
        .unwrap();
    assert_eq!(calendar.to_bytes(), before);
    Calendar::parse(projection.as_bytes())
        .unwrap()
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

#[test]
fn yearly_ordinals_place_february_transitions_in_projections() {
    assert_eq!(
        starts(
            &calendar("19960203T090000"),
            &year_zone(),
            "19960203T000000Z",
            "19960206T000000Z"
        ),
        [
            at("1996-02-03T09:00:00Z"),
            at("1996-02-04T08:00:00Z"),
            at("1996-02-05T08:00:00Z")
        ]
    );
}

#[test]
fn negative_month_ordinals_apply_the_query_offset_change() {
    let timezone = zone(
        "20200322T020000",
        "BYMONTH=3;BYDAY=-2SU",
        "20201018T030000",
        "BYMONTH=10;BYDAY=-2SU",
    );
    assert_eq!(
        starts(
            &calendar("20240323T090000"),
            &timezone,
            "20240323T000000Z",
            "20240326T000000Z"
        ),
        [
            at("2024-03-23T09:00:00Z"),
            at("2024-03-24T08:00:00Z"),
            at("2024-03-25T08:00:00Z")
        ]
    );
}

#[test]
fn calendar_queries_use_the_yearly_ordinal_transition() {
    let calendar = calendar("19960203T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("19960204T075500Z", "19960204T080500Z", Match::Matched),
        ("19960204T085500Z", "19960204T090500Z", Match::Unmatched),
    ] {
        let query = query(&year_zone(), start, end);
        assert_eq!(query.matches(&mut session, &calendar).unwrap(), expected);
    }
}
