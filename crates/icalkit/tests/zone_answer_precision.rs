// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Zone answers respect [RFC 5545 section 3.3.12](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.12) precision.

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::recurrence::{Occurrence, Window};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, Error};

struct AnswerDatabase(Timestamp);

impl ZoneDatabase for AnswerDatabase {
    fn resolve_local(&self, tzid: &str, _local: DateTime) -> Option<ZoneResolution> {
        (tzid == "Test/Precision").then(|| ZoneResolution::exact(self.0, "precision-fixture", true))
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "Test/Precision")
            .then(|| OffsetResolution::new(0, "precision-fixture", true))
            .flatten()
    }
}

fn calendar() -> Calendar {
    Calendar::parse(
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit zone precision//EN\r\n\
          BEGIN:VEVENT\r\nUID:precision@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
          DTSTART;TZID=Test/Precision:20240102T090000\r\nDURATION:PT1H\r\n\
          RRULE:FREQ=DAILY;COUNT=1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .unwrap()
}

fn query() -> Query {
    Query::parse(br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="20240102T090000Z" end="20240102T100000Z"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240102T090000Z" end="20240102T100000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#).unwrap()
}

fn engine(value: &str) -> Engine {
    Engine::builder()
        .zone_database(AnswerDatabase(value.parse().unwrap()))
        .build()
}

fn occurrence(engine: &Engine) -> Result<Option<Occurrence>, Error> {
    let calendar = calendar();
    let mut session = engine.session();
    let window = Window::new(
        "2024-01-02T09:00:00Z".parse().unwrap(),
        "2024-01-02T10:00:00Z".parse().unwrap(),
    )
    .unwrap();
    calendar
        .occurrences(&mut session, "precision@example.test", window)
        .and_then(|mut occurrences| occurrences.try_next())
}

#[test]
fn fractional_zone_answers_do_not_place_stored_occurrences() {
    for value in [
        "2024-01-02T09:00:00.000000001Z",
        "2024-01-02T09:00:00.999999999Z",
    ] {
        let result = occurrence(&engine(value));
        assert_eq!(
            result.unwrap_err().code().as_str(),
            "icalkit.recurrence.zone-answer-unreadable",
            "{value}"
        );
    }
    let result = occurrence(&Engine::builder().build());
    assert_eq!(
        result.unwrap_err().code().as_str(),
        "icalkit.recurrence.zone-unresolved"
    );
}

#[test]
fn fractional_zone_answers_leave_queries_undecided() {
    let engine = engine("2024-01-02T09:00:00.5Z");
    assert_eq!(
        query().matches(&mut engine.session(), &calendar()).unwrap(),
        Match::Undecided
    );
}

#[test]
fn fractional_zone_answers_cannot_be_projected_as_whole_seconds() {
    let engine = engine("2024-01-02T09:00:00.5Z");
    assert!(query().project(&mut engine.session(), &calendar()).is_err());
}

#[test]
fn whole_second_zone_answers_work_in_all_workflows() {
    let expected: Timestamp = "2024-01-02T09:00:00Z".parse().unwrap();
    let engine = engine("2024-01-02T09:00:00Z");
    assert_eq!(occurrence(&engine).unwrap().unwrap().start(), expected);
    let calendar = calendar();
    let original = calendar.to_bytes();
    let query = query();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    let projection = query.project(&mut session, &calendar).unwrap();
    let projected = Calendar::parse(projection.as_bytes()).unwrap();
    assert_eq!(
        projected
            .events()
            .next()
            .unwrap()
            .dtstart()
            .unwrap()
            .as_utc(),
        Some(expected)
    );
    assert_eq!(calendar.to_bytes(), original);
}
