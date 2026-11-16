// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Outbox POST follows [RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5).
//! Message fields follow [RFC 5546 section 3.3.2](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.2).
//! Addresses use [RFC 5545 section 3.3.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.3).

#![cfg(test)]

use icalkit::caldav::{Header, ScheduleDelivery, Server, ServerAnswer, WireRequest};
use icalkit::scheduling::Message;

const BUSY: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit outbox tests//EN\r\n",
    "METHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\nUID:busy@example.test\r\n",
    "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n",
    "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
    "ATTENDEE:mailto:carol@example.test\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n",
);

fn request(body: &str, method: &str, routing: &[(&str, &str)]) -> WireRequest {
    let mut headers = vec![Header::new(
        "Content-Type",
        format!("text/calendar; charset=utf-8; method={method}").into_bytes(),
    )];
    headers.extend(
        routing
            .iter()
            .map(|(name, value)| Header::new(*name, value.as_bytes().to_vec())),
    );
    WireRequest::new("POST", "/outbox/alice/", headers, body.as_bytes().to_vec())
}

fn legacy_routing() -> [(&'static str, &'static str); 2] {
    [
        ("Originator", "mailto:alice@example.test"),
        (
            "Recipient",
            "mailto:bob@example.test, mailto:carol@example.test",
        ),
    ]
}

#[test]
fn a_valid_busy_time_post_does_not_need_routing_headers() {
    Message::read(BUSY.as_bytes()).unwrap();
    let mut operation = Server::new().handle(request(BUSY, "REQUEST", &[])).unwrap();
    let authorization = operation.next_need().unwrap();
    assert_eq!(authorization.code(), "caldav.authorize");
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    let delivery = operation.next_need().unwrap();
    assert_eq!(delivery.originator(), Some("mailto:alice@example.test"));
    assert_eq!(
        delivery.recipients(),
        &[
            "mailto:bob@example.test".to_string(),
            "mailto:carol@example.test".to_string()
        ]
    );
    assert_eq!(delivery.message().unwrap().to_bytes(), BUSY.as_bytes());
}

#[test]
fn authorization_has_the_validated_message_and_its_parties() {
    let operation = Server::new()
        .handle(request(BUSY, "REQUEST", &legacy_routing()))
        .unwrap();
    let need = operation.next_need().unwrap();
    assert_eq!(need.code(), "caldav.authorize");
    assert_eq!(
        need.message()
            .expect("authorization needs the payload")
            .to_bytes(),
        BUSY.as_bytes()
    );
    assert_eq!(need.originator(), Some("mailto:alice@example.test"));
    assert_eq!(
        need.recipients(),
        &[
            "mailto:bob@example.test".to_string(),
            "mailto:carol@example.test".to_string()
        ]
    );
}

#[test]
fn forged_routing_headers_cannot_replace_message_parties() {
    let routing = [
        ("Originator", "mailto:forged@example.test"),
        ("Recipient", "mailto:elsewhere@example.test"),
    ];
    let mut operation = Server::new()
        .handle(request(BUSY, "REQUEST", &routing))
        .unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    let delivery = operation.next_need().unwrap();
    assert_eq!(delivery.originator(), Some("mailto:alice@example.test"));
    assert_eq!(
        delivery.recipients(),
        &[
            "mailto:bob@example.test".to_string(),
            "mailto:carol@example.test".to_string()
        ]
    );
}

#[test]
fn a_comma_inside_a_calendar_address_is_one_recipient() {
    const ADDRESS: &str = "https://calendar.example.test/principals/a,b";
    let body = BUSY
        .replace("mailto:bob@example.test", ADDRESS)
        .replace("ATTENDEE:mailto:carol@example.test\r\n", "");
    Message::read(body.as_bytes()).unwrap();
    let routing = [
        ("Originator", "mailto:alice@example.test"),
        ("Recipient", ADDRESS),
    ];
    let mut operation = Server::new()
        .handle(request(&body, "REQUEST", &routing))
        .unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    assert_eq!(
        operation.next_need().unwrap().recipients(),
        &[ADDRESS.to_string()]
    );
}

#[test]
fn repeated_attendee_addresses_have_one_delivery_target() {
    let body = BUSY.replace(
        "END:VFREEBUSY",
        "ATTENDEE:mailto:bob@example.test\r\nEND:VFREEBUSY",
    );
    Message::read(body.as_bytes()).unwrap();
    let mut operation = Server::new()
        .handle(request(&body, "REQUEST", &legacy_routing()))
        .unwrap();
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    assert_eq!(
        operation.next_need().unwrap().recipients(),
        &[
            "mailto:bob@example.test".to_string(),
            "mailto:carol@example.test".to_string()
        ]
    );
}

#[test]
fn valid_itip_of_another_kind_or_method_is_not_an_outbox_busy_time_request() {
    let event = BUSY
        .replace("VFREEBUSY", "VEVENT")
        .replace("END:VEVENT", "SUMMARY:Planning\r\nEND:VEVENT");
    let publish = BUSY
        .replace("METHOD:REQUEST", "METHOD:PUBLISH")
        .replace("ATTENDEE:mailto:bob@example.test\r\n", "")
        .replace("ATTENDEE:mailto:carol@example.test\r\n", "");
    for body in [event, publish] {
        let message = Message::read(body.as_bytes()).unwrap();
        let Err(error) = Server::new().handle(request(&body, message.method(), &legacy_routing()))
        else {
            panic!("accepted an outbox POST with a different iTIP kind or method");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.caldav.schedule-request-invalid"
        );
    }
}

#[test]
fn unknown_required_values_cannot_drive_a_busy_time_request() {
    for name in [
        "UID",
        "DTSTAMP",
        "DTSTART",
        "DTEND",
        "ORGANIZER",
        "ATTENDEE",
    ] {
        let body = BUSY.replace(&format!("{name}:"), &format!("{name};VALUE=X-OPAQUE:"));
        let Err(error) = Server::new().handle(request(&body, "REQUEST", &legacy_routing())) else {
            panic!("accepted an unreadable {name} in a busy time request");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.caldav.schedule-request-invalid"
        );
    }
}

#[test]
fn scheduling_delivery_addresses_require_absolute_uri_syntax() {
    for address in [
        "person@example.test",
        "/principals/person",
        "mailto:bad%2",
        "mailto:bad%GG@example.test",
        "ht^tp://example.test/person",
        "https://example.test:bad/person",
        "https://[broken]/person",
        "https://example.test/\u{00e9}",
    ] {
        let Err(error) = ScheduleDelivery::new(address, "3.7;Invalid calendar user", None) else {
            panic!("accepted a malformed calendar address {address}");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.caldav.schedule-delivery-invalid"
        );
    }
}

#[test]
fn scheduling_delivery_addresses_keep_valid_uri_spellings() {
    for address in [
        "MAILTO:Person%2Btag@Example.test",
        "urn:example:person",
        "https://calendar.example.test/principals/a,b?view=x#fragment",
        "x-cua:opaque%2Cvalue",
        "https://[2001:db8::1]/principals/person",
    ] {
        let result = ScheduleDelivery::new(address, "3.7;Invalid calendar user", None).unwrap();
        assert_eq!(result.recipient(), address);
    }
}
