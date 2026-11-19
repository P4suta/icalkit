// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Zone lookup follows the leap-second fallback in [RFC 5545 section 3.3.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.5).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::recurrence::Window;
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, Session};

struct UtcDatabase;

impl ZoneDatabase for UtcDatabase {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        (tzid == "Test/Leap").then(|| {
            ZoneResolution::exact(
                format!("{local}Z").parse().unwrap(),
                "leap-second-fixture",
                true,
            )
        })
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "Test/Leap")
            .then(|| OffsetResolution::new(0, "leap-second-fixture", true))
            .flatten()
    }
}

fn engine() -> Engine {
    Engine::builder().zone_database(UtcDatabase).build()
}

fn calendar(tzid: &str, second: u8, recurring: bool) -> Calendar {
    let rule = if recurring {
        "RRULE:FREQ=DAILY;COUNT=1\r\n"
    } else {
        ""
    };
    Calendar::parse(
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit leap seconds//EN\r\n\
             BEGIN:VEVENT\r\nUID:leap@example.test\r\nDTSTAMP:19970630T120000Z\r\n\
             DTSTART;TZID={tzid}:19970630T2359{second:02}\r\nDURATION:PT1S\r\n\
             {rule}END:VEVENT\r\nEND:VCALENDAR\r\n"
        )
        .as_bytes(),
    )
    .unwrap()
}

fn query(property: bool, second: u8) -> Query {
    let end = if second == 58 {
        "19970630T235959Z"
    } else {
        "19970701T000000Z"
    };
    let range = format!(
        r#"<C:time-range start="19970630T2359{:02}Z" end="{end}"/>"#,
        second.min(59)
    );
    let condition = if property {
        format!(r#"<C:prop-filter name="DTSTART">{range}</C:prop-filter>"#)
    } else {
        range
    };
    Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="19970630T235958Z" end="19970701T000000Z"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
{condition}
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        )
        .as_bytes(),
    )
    .unwrap()
}

fn expected(second: u8) -> Timestamp {
    format!("1997-06-30T23:59:{:02}Z", second.min(59))
        .parse()
        .unwrap()
}

fn assert_occurrence(session: &mut Session<'_>, calendar: &Calendar, expected: Timestamp) {
    let window = Window::new(
        "1997-06-30T23:59:58Z".parse().unwrap(),
        "1997-07-01T00:00:00Z".parse().unwrap(),
    )
    .unwrap();
    let mut occurrences = calendar
        .occurrences(session, "leap@example.test", window)
        .unwrap();
    assert_eq!(occurrences.try_next().unwrap().unwrap().start(), expected);
    assert!(occurrences.try_next().unwrap().is_none());
}

fn assert_projection(session: &mut Session<'_>, calendar: &Calendar, expected: Timestamp) {
    let projection = query(false, 58).project(session, calendar).unwrap();
    let projected = Calendar::parse(projection.as_bytes()).unwrap();
    let mut events = projected.events();
    assert_eq!(
        events.next().unwrap().dtstart().unwrap().as_utc(),
        Some(expected)
    );
    assert!(events.next().is_none());
}

fn assert_source_evidence(calendar: &Calendar, original: &[u8], second: u8) {
    assert_eq!(calendar.to_bytes(), original);
    let start = calendar.events().next().unwrap().dtstart().unwrap();
    assert_eq!(start.has_leap_second(), second == 60);
    assert_eq!(
        start.as_zoned().unwrap().0.second(),
        i8::try_from(second.min(59)).unwrap()
    );
}

#[test]
fn stored_occurrences_fold_leap_seconds_without_changing_the_source() {
    for recurring in [false, true] {
        let calendar = calendar("Test/Leap", 60, recurring);
        let original = calendar.to_bytes();
        assert_occurrence(&mut engine().session(), &calendar, expected(60));
        assert_source_evidence(&calendar, &original, 60);
    }
}

#[test]
fn component_and_property_time_ranges_match_zoned_leap_seconds() {
    for recurring in [false, true] {
        let calendar = calendar("Test/Leap", 60, recurring);
        let original = calendar.to_bytes();
        for property in [false, true] {
            assert_eq!(
                query(property, 60)
                    .matches(&mut engine().session(), &calendar)
                    .unwrap(),
                Match::Matched
            );
        }
        assert_source_evidence(&calendar, &original, 60);
    }
}

#[test]
fn projection_folds_zoned_leap_seconds_without_changing_the_source() {
    for recurring in [false, true] {
        let calendar = calendar("Test/Leap", 60, recurring);
        let original = calendar.to_bytes();
        assert_projection(&mut engine().session(), &calendar, expected(60));
        assert_source_evidence(&calendar, &original, 60);
    }
}

#[test]
fn ordinary_seconds_keep_their_readings_in_every_workflow() {
    for property in [false, true] {
        let later = calendar("Test/Leap", 59, false);
        assert_eq!(
            query(property, 58)
                .matches(&mut engine().session(), &later)
                .unwrap(),
            Match::Unmatched
        );
    }
    for second in [58, 59] {
        for recurring in [false, true] {
            let calendar = calendar("Test/Leap", second, recurring);
            let original = calendar.to_bytes();
            let engine = engine();
            let mut session = engine.session();
            assert_occurrence(&mut session, &calendar, expected(second));
            for property in [false, true] {
                assert_eq!(
                    query(property, second)
                        .matches(&mut session, &calendar)
                        .unwrap(),
                    Match::Matched
                );
            }
            assert_projection(&mut session, &calendar, expected(second));
            assert_source_evidence(&calendar, &original, second);
        }
    }
}

#[cfg(feature = "system-tz")]
#[test]
fn the_system_zone_port_accepts_a_zoned_leap_second() {
    let calendar = calendar("UTC", 60, false);
    let original = calendar.to_bytes();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query(true, 60).matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    assert_occurrence(&mut session, &calendar, expected(60));
    assert_projection(&mut session, &calendar, expected(60));
    assert_source_evidence(&calendar, &original, 60);
}
