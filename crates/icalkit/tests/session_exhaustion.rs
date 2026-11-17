// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A shared session does not restart admission after recurrence exhaustion.

#![cfg(test)]

use icalkit::interop::Import;
use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Engine, ResourcePolicy, Session};

const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit session exhaustion//EN\r\nBEGIN:VEVENT\r\nUID:exhaustion@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:20240101T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const QUERY: &[u8] = br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"/></C:comp-filter></C:filter></C:calendar-query>"#;

fn exhaust(session: &mut Session<'_>) {
    let rule = Rule::parse("FREQ=DAILY;COUNT=2").unwrap();
    let start = "2024-01-01T00:00:00Z".parse::<Timestamp>().unwrap();
    let end = "2024-01-03T00:00:00Z".parse::<Timestamp>().unwrap();
    let mut occurrences = rule
        .occurrences(session, start, Window::new(start, end).unwrap())
        .unwrap();
    assert!(occurrences.try_next().unwrap().is_some());
    assert_eq!(
        occurrences.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
}

#[test]
fn parsing_import_and_query_admission_stay_failed_until_a_fresh_session() {
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_occurrences_per_search(1))
        .build();
    let import = Import::read(CALENDAR).unwrap();
    let mut session = engine.session();
    exhaust(&mut session);
    assert!(session.parse(CALENDAR).is_err());
    assert!(session.import(CALENDAR).is_err());
    assert!(session.validate(&import).is_err());
    assert!(session.query(QUERY).is_err());
    let mut fresh = engine.session();
    assert!(fresh.parse(CALENDAR).is_ok());
    assert!(fresh.import(CALENDAR).is_ok());
    assert!(fresh.validate(&import).is_ok());
    assert!(fresh.query(QUERY).is_ok());
}
