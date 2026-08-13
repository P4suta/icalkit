// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV server workflows with ACL and storage kept outside the crate.

use icalkit::Calendar;
use icalkit::caldav::{
    Client, Discovery, Header, Revision, ScheduleDelivery, ScheduleResponse, Server, ServerAnswer,
    StoredResource, SyncChange, SyncResult, SyncToken, WireRequest,
};
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

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

const SCHEDULING_PAYLOAD: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit server tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:scheduled@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
DTSTART:20260814T090000Z\r\n\
SUMMARY:Planning\r\n\
ORGANIZER:mailto:alice@example.test\r\n\
ATTENDEE:mailto:bob@example.test\r\n\
ATTENDEE:mailto:carol@example.test\r\n\
SEQUENCE:1\r\n\
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

fn mkcalendar(body: &[u8]) -> WireRequest {
    WireRequest::new(
        "MKCALENDAR",
        "/calendars/alice/new/",
        Vec::new(),
        body.to_vec(),
    )
}

fn schedule_operation() -> Option<(icalkit::caldav::Operation<ScheduleResponse>, WireRequest)> {
    let message = Message::request(SCHEDULING_PAYLOAD, Timestamp::constant(0, 0)).ok()?;
    let operation = Client::new()
        .schedule(
            "/outbox/alice/",
            "mailto:alice@example.test",
            &["mailto:bob@example.test", "mailto:carol@example.test"],
            &message,
        )
        .ok()?;
    let request = operation.next_request()?.clone();
    Some((operation, request))
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

#[test]
fn server_mkcalendar_asks_for_acl_then_validated_creation() {
    let body = br#"<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:set><D:prop><D:displayname>Work &amp; travel</D:displayname>
<C:calendar-description>Shared</C:calendar-description></D:prop></D:set>
</C:mkcalendar>"#;
    let mut operation = Server::new().handle(mkcalendar(body)).unwrap();

    let acl = operation.next_need().unwrap();
    assert_eq!(acl.code(), "caldav.authorize");
    assert_eq!(acl.method(), Some("MKCALENDAR"));
    assert_eq!(acl.uri(), Some("/calendars/alice/new/"));
    assert_eq!(acl.body(), &[]);
    operation.supply(ServerAnswer::authorized(true)).unwrap();

    let create = operation.next_need().unwrap();
    assert_eq!(create.code(), "caldav.mkcalendar.create");
    assert_eq!(create.method(), Some("MKCALENDAR"));
    assert_eq!(create.uri(), Some("/calendars/alice/new/"));
    assert_eq!(create.body(), body);
    operation.supply(ServerAnswer::created(true)).unwrap();

    let response = operation.finish().unwrap();
    assert_eq!(response.status(), 201);
    assert!(response.body().is_empty());
}

#[test]
fn server_mkcalendar_maps_a_storage_conflict_without_hiding_it() {
    let body = br#"<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:set><D:prop/></D:set></C:mkcalendar>"#;
    let mut operation = Server::new().handle(mkcalendar(body)).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    operation.supply(ServerAnswer::created(false)).unwrap();

    assert_eq!(operation.finish().unwrap().status(), 409);
}

#[test]
fn server_mkcalendar_accepts_an_absent_optional_request_body() {
    let mut operation = Server::new().handle(mkcalendar(b"")).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();

    assert_eq!(operation.next_need().unwrap().body(), &[]);
    operation.supply(ServerAnswer::created(true)).unwrap();
    assert_eq!(operation.finish().unwrap().status(), 201);
}

#[test]
fn server_rejects_malformed_mkcalendar_before_asking_for_acl() {
    let malformed = br#"<!DOCTYPE x><C:mkcalendar xmlns:C="urn:ietf:params:xml:ns:caldav"/>"#;
    let error = Server::new().handle(mkcalendar(malformed)).unwrap_err();

    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.mkcalendar-invalid"
    );
}

#[test]
fn server_outbox_post_round_trips_typed_delivery_results() {
    let (mut client_operation, request) = schedule_operation().unwrap();
    let mut server_operation = Server::new().handle(request).unwrap();

    let acl = server_operation.next_need().unwrap();
    assert_eq!(acl.code(), "caldav.authorize");
    assert_eq!(acl.method(), Some("POST"));
    assert_eq!(acl.uri(), Some("/outbox/alice/"));
    server_operation
        .supply(ServerAnswer::authorized(true))
        .unwrap();

    let delivery = server_operation.next_need().unwrap();
    assert_eq!(delivery.code(), "caldav.schedule.deliver");
    assert_eq!(delivery.message().unwrap().method(), "REQUEST");
    assert_eq!(delivery.originator(), Some("mailto:alice@example.test"));
    assert_eq!(
        delivery.recipients(),
        &[
            "mailto:bob@example.test".to_string(),
            "mailto:carol@example.test".to_string()
        ]
    );

    let outcomes = ScheduleResponse::new(vec![
        ScheduleDelivery::new("mailto:carol@example.test", "3.7;Invalid <calendar> & user")
            .unwrap(),
        ScheduleDelivery::new("mailto:bob@example.test", "2.0;Success").unwrap(),
    ])
    .unwrap();
    server_operation
        .supply(ServerAnswer::schedule_response(outcomes))
        .unwrap();

    let wire_response = server_operation.finish().unwrap();
    assert_eq!(wire_response.status(), 200);
    client_operation.accept(wire_response).unwrap();
    let decoded = client_operation.finish().unwrap();
    assert_eq!(decoded.deliveries().len(), 2);
    assert_eq!(
        decoded.deliveries()[0].recipient(),
        "mailto:carol@example.test"
    );
    assert_eq!(
        decoded.deliveries()[0].request_status(),
        "3.7;Invalid <calendar> & user"
    );
    assert_eq!(decoded.deliveries()[1].request_status(), "2.0;Success");
}

#[test]
fn server_rejects_an_invalid_scheduling_request_before_acl() {
    let request = WireRequest::new(
        "POST",
        "/outbox/alice/",
        vec![
            Header::new("Content-Type", b"text/calendar; method=REQUEST".to_vec()),
            Header::new("Originator", b"mailto:alice@example.test".to_vec()),
            Header::new("Recipient", b"mailto:bob@example.test".to_vec()),
        ],
        b"not an iTIP message".to_vec(),
    );
    let error = Server::new().handle(request).unwrap_err();

    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.schedule-request-invalid"
    );
}

#[test]
fn server_rejects_a_mime_method_that_disagrees_with_itip() {
    let (_, valid) = schedule_operation().unwrap();
    let request = WireRequest::new(
        "POST",
        valid.uri(),
        vec![
            Header::new("Content-Type", b"text/calendar; method=REPLY".to_vec()),
            Header::new("Originator", b"mailto:alice@example.test".to_vec()),
            Header::new(
                "Recipient",
                b"mailto:bob@example.test, mailto:carol@example.test".to_vec(),
            ),
        ],
        valid.body().to_vec(),
    );
    let error = Server::new().handle(request).unwrap_err();

    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.schedule-request-invalid"
    );
}

#[test]
fn server_refuses_incomplete_delivery_results() {
    let (_, request) = schedule_operation().unwrap();
    let mut operation = Server::new().handle(request).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    let incomplete = ScheduleResponse::new(vec![
        ScheduleDelivery::new("mailto:bob@example.test", "2.0;Success").unwrap(),
    ])
    .unwrap();

    let error = operation
        .supply(ServerAnswer::schedule_response(incomplete))
        .unwrap_err();
    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.schedule-response-incomplete"
    );
}

#[test]
fn server_conditional_put_round_trips_a_new_strong_revision() {
    let calendar = Calendar::parse(MATCHING).unwrap();
    let expected = Revision::stored("/calendars/alice/work/one.ics", "\"v1\"").unwrap();
    let mut client_operation = Client::new()
        .conditional_write(&expected, &calendar)
        .unwrap();
    let request = client_operation.next_request().unwrap().clone();
    let mut server_operation = Server::new().handle(request).unwrap();

    assert_eq!(
        server_operation.next_need().unwrap().code(),
        "caldav.authorize"
    );
    server_operation
        .supply(ServerAnswer::authorized(true))
        .unwrap();
    let write = server_operation.next_need().unwrap();
    assert_eq!(write.code(), "caldav.resource.write");
    assert_eq!(write.calendar().unwrap().to_bytes(), MATCHING);
    assert_eq!(write.revision().unwrap(), &expected);

    let stored = Revision::stored("/calendars/alice/work/one.ics", "\"v2\"").unwrap();
    server_operation
        .supply(ServerAnswer::written(Some(stored.clone())))
        .unwrap();
    let wire_response = server_operation.finish().unwrap();
    assert_eq!(wire_response.status(), 204);

    client_operation.accept(wire_response).unwrap();
    assert_eq!(client_operation.finish().unwrap(), stored);
}

#[test]
fn server_conditional_put_returns_precondition_failed_on_conflict() {
    let calendar = Calendar::parse(MATCHING).unwrap();
    let expected = Revision::stored("/calendars/alice/work/one.ics", "\"stale\"").unwrap();
    let request = Client::new()
        .conditional_write(&expected, &calendar)
        .unwrap()
        .next_request()
        .unwrap()
        .clone();
    let mut operation = Server::new().handle(request).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    operation.supply(ServerAnswer::written(None)).unwrap();

    assert_eq!(operation.finish().unwrap().status(), 412);
}

#[test]
fn server_conditional_put_creates_only_when_the_resource_is_absent() {
    let calendar = Calendar::parse(MATCHING).unwrap();
    let expected = Revision::absent("/calendars/alice/work/new.ics").unwrap();
    let request = Client::new()
        .conditional_write(&expected, &calendar)
        .unwrap()
        .next_request()
        .unwrap()
        .clone();
    let mut operation = Server::new().handle(request).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    assert!(
        operation
            .next_need()
            .unwrap()
            .revision()
            .unwrap()
            .is_absent()
    );

    let stored = Revision::stored("/calendars/alice/work/new.ics", "\"created\"").unwrap();
    operation
        .supply(ServerAnswer::written(Some(stored)))
        .unwrap();
    assert_eq!(operation.finish().unwrap().status(), 201);
}

#[test]
fn server_conditional_put_rejects_a_weak_precondition_before_acl() {
    let request = WireRequest::new(
        "PUT",
        "/calendars/alice/work/one.ics",
        vec![
            Header::new("Content-Type", b"text/calendar".to_vec()),
            Header::new("If-Match", b"W/\"v1\"".to_vec()),
        ],
        MATCHING.to_vec(),
    );
    let error = Server::new().handle(request).unwrap_err();

    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.write-request-invalid"
    );
}

#[test]
fn server_sync_report_round_trips_updates_removals_and_the_new_token() {
    let previous = SyncToken::new("data:,sync-1").unwrap();
    let mut client_operation = Client::new()
        .sync("/calendars/alice/work/", Some(&previous))
        .unwrap();
    let request = client_operation.next_request().unwrap().clone();
    let mut server_operation = Server::new().handle(request).unwrap();

    server_operation
        .supply(ServerAnswer::authorized(true))
        .unwrap();
    let changes = server_operation.next_need().unwrap();
    assert_eq!(changes.code(), "caldav.sync.changes");
    assert_eq!(changes.sync_token(), Some(&previous));
    assert_eq!(changes.sync_limit(), None);

    let next = SyncToken::new("data:,sync-2").unwrap();
    let result = SyncResult::new(
        next.clone(),
        vec![
            SyncChange::updated(
                "/calendars/alice/work/one.ics",
                Some("\"v2\""),
                Calendar::parse(MATCHING).unwrap(),
            )
            .unwrap(),
            SyncChange::removed("/calendars/alice/work/gone.ics").unwrap(),
        ],
    )
    .unwrap();
    server_operation
        .supply(ServerAnswer::sync_result(result))
        .unwrap();

    let wire_response = server_operation.finish().unwrap();
    assert_eq!(wire_response.status(), 207);
    client_operation.accept(wire_response).unwrap();
    let decoded = client_operation.finish().unwrap();
    assert_eq!(decoded.token(), Some(&next));
    assert_eq!(decoded.changes().len(), 2);
    assert_eq!(decoded.changes()[0].etag(), Some("\"v2\""));
    assert_eq!(
        decoded.changes()[0].calendar().unwrap().to_bytes(),
        MATCHING
    );
    assert!(decoded.changes()[1].is_removed());
}

#[test]
fn server_sync_exposes_initial_limit_and_rejects_an_oversized_page() {
    let request = WireRequest::new(
        "REPORT",
        "/calendars/alice/work/",
        Vec::new(),
        br#"<D:sync-collection xmlns:D="DAV:">
<D:sync-token/><D:sync-level>1</D:sync-level>
<D:limit><D:nresults>1</D:nresults></D:limit>
<D:prop><D:getetag/></D:prop>
</D:sync-collection>"#
            .to_vec(),
    );
    let mut operation = Server::new().handle(request).unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    let need = operation.next_need().unwrap();
    assert_eq!(need.sync_token(), None);
    assert_eq!(need.sync_limit(), Some(1));

    let result = SyncResult::new(
        SyncToken::new("data:,page-1").unwrap(),
        vec![
            SyncChange::removed("/calendars/alice/work/one.ics").unwrap(),
            SyncChange::removed("/calendars/alice/work/two.ics").unwrap(),
        ],
    )
    .unwrap();
    let error = operation
        .supply(ServerAnswer::sync_result(result))
        .unwrap_err();
    assert_eq!(
        error.issues()[0].code().as_str(),
        "icalkit.caldav.sync-response-limit"
    );
}

#[test]
fn server_propfind_drives_client_service_discovery_end_to_end() {
    let discovery = Discovery::new(
        "/principals/alice/",
        "/calendars/alice/",
        Some("/outbox/alice/"),
    )
    .unwrap();
    let mut client_operation = Client::new().discover("/").unwrap();

    for expected_uri in ["/", "/principals/alice/"] {
        let request = client_operation.next_request().unwrap().clone();
        assert_eq!(request.uri(), expected_uri);
        let mut server_operation = Server::new().handle(request).unwrap();
        server_operation
            .supply(ServerAnswer::authorized(true))
            .unwrap();
        let need = server_operation.next_need().unwrap();
        assert_eq!(need.code(), "caldav.discovery.properties");
        server_operation
            .supply(ServerAnswer::discovery(discovery.clone()))
            .unwrap();
        client_operation
            .accept(server_operation.finish().unwrap())
            .unwrap();
    }

    let decoded = client_operation.finish().unwrap();
    assert_eq!(decoded.principal_uri(), "/principals/alice/");
    assert_eq!(decoded.calendar_home_uri(), "/calendars/alice/");
    assert_eq!(decoded.scheduling_outbox_uri(), Some("/outbox/alice/"));
}
