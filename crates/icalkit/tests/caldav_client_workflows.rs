// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stateful CalDAV client workflows through the public sans-I/O API.

use icalkit::Calendar;
use icalkit::caldav::{Client, Header, Revision, SyncToken, WireResponse};
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit client tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
DTSTART:20260814T090000Z\r\n\
SUMMARY:Planning\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|header| header.name().eq_ignore_ascii_case(name))
        .map(Header::value)
}

#[test]
fn discovery_walks_from_principal_to_calendar_home_and_outbox() {
    let client = Client::new();
    let mut operation = client.discover("/").unwrap();

    let first = operation.next_request().unwrap();
    assert_eq!(first.method(), "PROPFIND");
    assert_eq!(first.uri(), "/");
    assert_eq!(header(first.headers(), "Depth"), Some(b"0".as_slice()));
    assert!(
        first
            .body()
            .windows(b"current-user-principal".len())
            .any(|part| part == b"current-user-principal")
    );

    operation
        .accept(WireResponse::new(
            207,
            Vec::new(),
            br#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
 <D:response><D:href>/</D:href><D:propstat><D:prop>
  <D:current-user-principal><D:href>/principals/alice/</D:href></D:current-user-principal>
 </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#
                .to_vec(),
        ))
        .unwrap();

    let second = operation.next_request().unwrap();
    assert_eq!(second.method(), "PROPFIND");
    assert_eq!(second.uri(), "/principals/alice/");
    assert!(
        second
            .body()
            .windows(b"calendar-home-set".len())
            .any(|part| part == b"calendar-home-set")
    );
    assert!(
        second
            .body()
            .windows(b"schedule-outbox-URL".len())
            .any(|part| part == b"schedule-outbox-URL")
    );

    operation
        .accept(WireResponse::new(
            207,
            Vec::new(),
            br#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
 <D:response><D:href>/principals/alice/</D:href><D:propstat><D:prop>
  <C:calendar-home-set><D:href>/calendars/alice/</D:href></C:calendar-home-set>
  <C:schedule-outbox-URL><D:href>/outbox/alice/</D:href></C:schedule-outbox-URL>
 </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#
                .to_vec(),
        ))
        .unwrap();

    let discovered = operation.finish().unwrap();
    assert_eq!(discovered.principal_uri(), "/principals/alice/");
    assert_eq!(discovered.calendar_home_uri(), "/calendars/alice/");
    assert_eq!(discovered.scheduling_outbox_uri(), Some("/outbox/alice/"));
}

#[test]
fn incremental_sync_round_trips_an_opaque_token_and_classifies_removals() {
    let token = SyncToken::new("data:,sync-1").unwrap();
    let client = Client::new();
    let mut operation = client.sync("/calendars/alice/work/", Some(&token)).unwrap();

    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "REPORT");
    assert_eq!(header(request.headers(), "Depth"), Some(b"1".as_slice()));
    assert!(
        request
            .body()
            .windows(token.as_bytes().len())
            .any(|part| part == token.as_bytes())
    );

    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
 <D:response><D:href>/calendars/alice/work/one.ics</D:href><D:propstat><D:prop>
  <D:getetag>&quot;v2&quot;</D:getetag><C:calendar-data>{}</C:calendar-data>
 </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
 <D:response><D:href>/calendars/alice/work/gone.ics</D:href>
  <D:status>HTTP/1.1 404 Not Found</D:status></D:response>
 <D:sync-token>data:,sync-2</D:sync-token>
</D:multistatus>"#,
        core::str::from_utf8(CALENDAR).unwrap()
    );
    operation
        .accept(WireResponse::new(207, Vec::new(), body.into_bytes()))
        .unwrap();

    let result = operation.finish().unwrap();
    assert_eq!(result.token().unwrap().as_bytes(), b"data:,sync-2");
    assert_eq!(result.changes().len(), 2);
    assert_eq!(result.changes()[0].href(), "/calendars/alice/work/one.ics");
    assert!(!result.changes()[0].is_removed());
    assert_eq!(result.changes()[0].etag(), Some("\"v2\""));
    assert_eq!(result.changes()[0].calendar().unwrap().to_bytes(), CALENDAR);
    assert!(result.changes()[1].is_removed());
    assert!(result.changes()[1].calendar().is_none());
}

#[test]
fn conditional_writes_are_bound_to_a_strong_revision() {
    let calendar = Calendar::parse(CALENDAR).unwrap();
    let revision = Revision::stored("/calendars/alice/work/one.ics", "\"v2\"").unwrap();
    let client = Client::new();
    let mut operation = client.conditional_write(&revision, &calendar).unwrap();

    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.uri(), revision.uri());
    assert_eq!(
        header(request.headers(), "If-Match"),
        Some(b"\"v2\"".as_slice())
    );
    assert_eq!(request.body(), CALENDAR);

    operation
        .accept(WireResponse::new(
            204,
            vec![Header::new("ETag", b"\"v3\"".to_vec())],
            Vec::new(),
        ))
        .unwrap();
    assert_eq!(operation.finish().unwrap().etag(), Some("\"v3\""));

    assert_eq!(
        Revision::stored("/calendars/alice/work/one.ics", "W/\"v2\"")
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.caldav.weak-etag"
    );
}

#[test]
fn mkcalendar_escapes_properties_and_accepts_only_a_creation_response() {
    let client = Client::new();
    let mut operation = client
        .mkcalendar(
            "/calendars/alice/travel/",
            "Work & Travel",
            Some("Plans <private>"),
        )
        .unwrap();

    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "MKCALENDAR");
    assert_eq!(request.uri(), "/calendars/alice/travel/");
    assert!(
        request
            .body()
            .windows(b"Work &amp; Travel".len())
            .any(|part| part == b"Work &amp; Travel")
    );
    assert!(
        request
            .body()
            .windows(b"Plans &lt;private&gt;".len())
            .any(|part| part == b"Plans &lt;private&gt;")
    );

    operation
        .accept(WireResponse::new(201, Vec::new(), Vec::new()))
        .unwrap();
    operation.finish().unwrap();
}

#[test]
fn outbox_post_returns_one_typed_delivery_per_recipient() {
    let payload = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit client tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
DTSTART:20260814T090000Z\r\n\
SUMMARY:Planning\r\n\
ORGANIZER:mailto:alice@example.test\r\n\
ATTENDEE:mailto:bob@example.test\r\n\
SEQUENCE:1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::request(payload, Timestamp::constant(0, 0)).unwrap();
    let client = Client::new();
    let mut operation = client
        .schedule(
            "/outbox/alice/",
            "mailto:alice@example.test",
            &["mailto:bob@example.test"],
            &message,
        )
        .unwrap();

    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(
        header(request.headers(), "Originator"),
        Some(b"mailto:alice@example.test".as_slice())
    );
    assert_eq!(
        header(request.headers(), "Recipient"),
        Some(b"mailto:bob@example.test".as_slice())
    );
    assert_eq!(
        header(request.headers(), "Content-Type"),
        Some(message.imip_content_type().as_bytes())
    );
    assert_eq!(request.body(), message.to_bytes());

    operation
        .accept(WireResponse::new(
            200,
            Vec::new(),
            br#"<?xml version="1.0" encoding="utf-8"?>
<C:schedule-response xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
 <C:response>
  <C:recipient><D:href>mailto:bob@example.test</D:href></C:recipient>
  <C:request-status>2.0;Success</C:request-status>
 </C:response>
</C:schedule-response>"#
                .to_vec(),
        ))
        .unwrap();

    let response = operation.finish().unwrap();
    assert_eq!(response.deliveries().len(), 1);
    assert_eq!(
        response.deliveries()[0].recipient(),
        "mailto:bob@example.test"
    );
    assert_eq!(response.deliveries()[0].request_status(), "2.0;Success");
    assert!(response.deliveries()[0].is_success());
}
