// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV server workflows with ACL and storage kept outside the crate.

use icalkit::Calendar;
use icalkit::caldav::{Server, ServerAnswer, StoredResource, WireRequest};

const MATCHING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit server tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
SUMMARY:Planning\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const MISSING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit server tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:two@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
SUMMARY:Finance\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const QUERY: &[u8] = br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><D:getetag/><C:calendar-data/></D:prop>
<C:filter>
 <C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
  <C:prop-filter name="SUMMARY"><C:text-match>plan</C:text-match></C:prop-filter>
 </C:comp-filter></C:comp-filter>
</C:filter>
</C:calendar-query>"#;

fn report() -> WireRequest {
    WireRequest::new(
        "REPORT",
        "/calendars/alice/work/",
        Vec::new(),
        QUERY.to_vec(),
    )
}

fn partial_report() -> WireRequest {
    WireRequest::new(
        "REPORT",
        "/calendars/alice/work/",
        Vec::new(),
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:comp name="VCALENDAR">
  <C:prop name="VERSION"/>
  <C:comp name="VEVENT"><C:prop name="UID"/><C:prop name="SUMMARY"/></C:comp>
</C:comp></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#
            .to_vec(),
    )
}

#[test]
fn server_query_asks_for_acl_then_storage_and_returns_only_matches() {
    let server = Server::new();
    let mut operation = server.handle(report()).unwrap();

    let acl = operation.next_need().unwrap();
    assert_eq!(acl.code(), "caldav.authorize");
    assert_eq!(acl.method(), Some("REPORT"));
    assert_eq!(acl.uri(), Some("/calendars/alice/work/"));
    operation.supply(ServerAnswer::authorized(true)).unwrap();

    let storage = operation.next_need().unwrap();
    assert_eq!(storage.code(), "caldav.query.resources");
    operation
        .supply(ServerAnswer::resources(vec![
            StoredResource::new(
                "/calendars/alice/work/one.ics",
                Some("\"one-v1\""),
                Calendar::parse(MATCHING).unwrap(),
            )
            .unwrap(),
            StoredResource::new(
                "/calendars/alice/work/two.ics",
                Some("\"two-v1\""),
                Calendar::parse(MISSING).unwrap(),
            )
            .unwrap(),
        ]))
        .unwrap();

    let response = operation.finish().unwrap();
    assert_eq!(response.status(), 207);
    assert!(
        response
            .body()
            .windows(b"one.ics".len())
            .any(|part| part == b"one.ics")
    );
    assert!(
        !response
            .body()
            .windows(b"two.ics".len())
            .any(|part| part == b"two.ics")
    );
    assert!(
        response
            .body()
            .windows(b"SUMMARY:Planning".len())
            .any(|part| part == b"SUMMARY:Planning")
    );
}

#[test]
fn an_acl_refusal_finishes_without_requesting_storage() {
    let server = Server::new();
    let mut operation = server.handle(report()).unwrap();

    operation.supply(ServerAnswer::authorized(false)).unwrap();
    assert!(operation.next_need().is_none());
    assert_eq!(operation.finish().unwrap().status(), 403);
}

#[test]
fn server_returns_a_reduced_calendar_data_projection() {
    let server = Server::new();
    let mut operation = server.handle(partial_report()).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    operation
        .supply(ServerAnswer::resources(vec![
            StoredResource::new(
                "/calendars/alice/work/one.ics",
                None,
                Calendar::parse(MATCHING).unwrap(),
            )
            .unwrap(),
        ]))
        .unwrap();

    let response = operation.finish().unwrap();
    assert_eq!(response.status(), 207);
    assert!(
        response
            .body()
            .windows(b"SUMMARY:Planning".len())
            .any(|part| part == b"SUMMARY:Planning")
    );
    assert!(!response.body().windows(7).any(|part| part == b"DTSTAMP"));
}
