// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Intersections follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Queries select floating-time zones through [RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};

const POSITIVE_FINAL_WEEK: &str = "25,26,27,28,29,30,31";

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

fn calendar(start: &str) -> Calendar {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month day windows//EN\r\n\
         BEGIN:VEVENT\r\nUID:windows@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let calendar = Calendar::parse(text.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), text.as_bytes());
    calendar
}

fn query(days: &str, start: &str, end: &str) -> Query {
    let timezone = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month day windows//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/MonthDays\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:20240225T020000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;BYMONTH=2;BYDAY=SU;BYMONTHDAY={days}\r\nEND:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\nDTSTART:20241027T030000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\n\
         RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
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

fn starts(start: &str, days: &str, from: &str, until: &str) -> Vec<Timestamp> {
    let calendar = calendar(start);
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let projection = query(days, from, until)
        .project(&mut engine.session(), &calendar)
        .unwrap();
    assert_eq!(calendar.to_bytes(), original);
    Calendar::parse(projection.as_bytes())
        .unwrap()
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

#[test]
fn positive_windows_skip_years_with_no_matching_day() {
    assert_eq!(
        starts(
            "20260221T090000",
            POSITIVE_FINAL_WEEK,
            "20260221T000000Z",
            "20260224T000000Z"
        ),
        [
            at("2026-02-21T09:00:00Z"),
            at("2026-02-22T09:00:00Z"),
            at("2026-02-23T09:00:00Z")
        ]
    );
}

#[test]
fn positive_windows_keep_valid_february_transitions() {
    for (start, from, until, expected) in [
        (
            "20240224T090000",
            "20240224T000000Z",
            "20240227T000000Z",
            [
                "2024-02-24T09:00:00Z",
                "2024-02-25T08:00:00Z",
                "2024-02-26T08:00:00Z",
            ],
        ),
        (
            "20270227T090000",
            "20270227T000000Z",
            "20270302T000000Z",
            [
                "2027-02-27T09:00:00Z",
                "2027-02-28T08:00:00Z",
                "2027-03-01T08:00:00Z",
            ],
        ),
    ] {
        assert_eq!(
            starts(start, POSITIVE_FINAL_WEEK, from, until),
            expected.map(at)
        );
    }
}

#[test]
fn queries_do_not_use_an_out_of_set_transition() {
    let calendar = calendar("20260221T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (from, until, expected) in [
        ("20260222T085500Z", "20260222T090500Z", Match::Matched),
        ("20260222T075500Z", "20260222T080500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(POSITIVE_FINAL_WEEK, from, until)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn negative_final_week_transitions_still_apply() {
    assert_eq!(
        starts(
            "20260221T090000",
            "-7,-6,-5,-4,-3,-2,-1",
            "20260221T000000Z",
            "20260224T000000Z"
        ),
        [
            at("2026-02-21T09:00:00Z"),
            at("2026-02-22T08:00:00Z"),
            at("2026-02-23T08:00:00Z")
        ]
    );
}
