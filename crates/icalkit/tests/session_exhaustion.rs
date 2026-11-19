// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A shared session does not restart admission after resource exhaustion.

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

fn parser_refusal_stops_later_admissions(bytes: &[u8]) {
    let engine = Engine::builder().build();
    let import = Import::read(CALENDAR).unwrap();
    let mut session = engine.session();
    assert!(session.parse(CALENDAR).is_ok());
    assert_eq!(
        session.parse(bytes).unwrap_err().code().as_str(),
        "icalkit.parse.resource-limit"
    );
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

#[test]
fn a_header_refusal_stops_the_shared_session() {
    let mut bytes = vec![b'X'; 4097];
    bytes.extend_from_slice(b":v\r\n");
    parser_refusal_stops_later_admissions(&bytes);
}

#[test]
fn a_parameter_refusal_stops_the_shared_session() {
    let mut bytes = b"X".to_vec();
    bytes.extend_from_slice(&b";P=v".repeat(65));
    bytes.extend_from_slice(b":v\r\n");
    parser_refusal_stops_later_admissions(&bytes);
}

#[test]
fn a_fold_refusal_stops_the_shared_session() {
    let mut bytes = b"X:v".to_vec();
    bytes.extend_from_slice(&b"\r\n x".repeat(16_385));
    bytes.extend_from_slice(b"\r\n");
    parser_refusal_stops_later_admissions(&bytes);
}

#[test]
fn a_value_refusal_stops_the_shared_session() {
    let mut bytes = b"X:".to_vec();
    bytes.resize(2 + 1_048_577, b'x');
    bytes.extend_from_slice(b"\r\n");
    parser_refusal_stops_later_admissions(&bytes);
}

#[test]
fn a_calendar_violation_does_not_exhaust_the_session() {
    let engine = Engine::builder().build();
    let mut session = engine.session();
    assert_eq!(
        session.parse(b"X:v\r\n").unwrap_err().code().as_str(),
        "icalkit.validation.failed"
    );
    assert!(session.parse(CALENDAR).is_ok());
    assert!(session.import(CALENDAR).is_ok());
    assert!(session.query(QUERY).is_ok());
}

#[test]
fn a_query_property_count_refusal_stops_the_shared_session() {
    let mut bytes =
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:"><D:prop>"#
            .to_vec();
    bytes.extend_from_slice(&b"<D:getetag/>".repeat(257));
    bytes.extend_from_slice(
        br#"</D:prop><C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#,
    );
    let engine = Engine::builder().build();
    let mut session = engine.session();
    assert_eq!(
        session.query(&bytes).unwrap_err().code().as_str(),
        "icalkit.caldav.query-invalid"
    );
    assert!(session.parse(CALENDAR).is_err());
    assert!(session.import(CALENDAR).is_err());
    assert!(session.validate(&Import::read(CALENDAR).unwrap()).is_err());
    assert!(session.query(QUERY).is_err());
    let mut fresh = engine.session();
    assert!(fresh.parse(CALENDAR).is_ok());
    assert!(fresh.import(CALENDAR).is_ok());
    assert!(fresh.query(QUERY).is_ok());
}

#[test]
fn malformed_query_syntax_does_not_exhaust_the_session() {
    let engine = Engine::builder().build();
    let mut session = engine.session();
    assert!(session.query(b"<").is_err());
    assert!(session.parse(CALENDAR).is_ok());
    assert!(session.import(CALENDAR).is_ok());
    assert!(session.query(QUERY).is_ok());
}

#[test]
fn rejected_query_roots_release_their_live_scope() {
    let engine = Engine::builder().build();
    let mut session = engine.session();
    for _ in 0..100 {
        assert!(
            session
                .query(br#"<X:unknown xmlns:X="urn:example:foreign"/>"#)
                .is_err()
        );
    }
    assert!(session.parse(CALENDAR).is_ok());
    assert!(session.query(QUERY).is_ok());
}

#[test]
fn malformed_queries_spend_the_shared_input_budget() {
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_max_input_bytes(8))
        .build();
    let mut session = engine.session();
    for _ in 0..8 {
        assert_eq!(
            session.query(b"<").unwrap_err().code().as_str(),
            "icalkit.caldav.query-invalid"
        );
    }
    assert!(session.import(b"").is_ok());
    assert!(session.import(b"x").is_err());
    assert!(session.import(b"").is_err());
    assert!(engine.session().import(b"x").is_ok());
}

#[test]
fn rejected_query_tails_spend_the_shared_input_budget() {
    let mut body = b"<x>".to_vec();
    body.extend_from_slice(&[b'a'; 512]);
    body.extend_from_slice(b"</x>");
    let budget = u64::try_from(body.len()).unwrap() - 1;
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_max_input_bytes(budget))
        .build();
    let mut session = engine.session();
    assert!(session.query(&body).is_err());
    assert!(session.import(b"").is_err());
    assert!(engine.session().import(b"").is_ok());
}
