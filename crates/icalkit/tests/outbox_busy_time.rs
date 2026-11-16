// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Busy time requests through the public sans-I/O client and server.

#![cfg(test)]

use icalkit::caldav::{Client, ScheduleDelivery, ScheduleResponse, Server, ServerAnswer};
use icalkit::scheduling::Message;

const BUSY: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit outbox tests//EN\r\n",
    "METHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\nUID:busy@example.test\r\n",
    "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n",
    "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
    "ATTENDEE:mailto:carol@example.test\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n",
);

#[test]
fn the_client_uses_the_message_as_its_only_address_source() {
    let message = Message::read(BUSY.as_bytes()).unwrap();
    let operation = Client::new().schedule("/outbox/alice/", &message).unwrap();
    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.body(), BUSY.as_bytes());
    assert_eq!(
        request
            .headers()
            .iter()
            .find(|header| header.name().eq_ignore_ascii_case("Content-Type"))
            .unwrap()
            .value(),
        message.imip_content_type().as_bytes()
    );
    assert!(request.headers().iter().all(|header| {
        !header.name().eq_ignore_ascii_case("Originator")
            && !header.name().eq_ignore_ascii_case("Recipient")
    }));
}

#[test]
fn the_client_refuses_messages_that_do_not_define_a_readable_busy_time_request() {
    let event = BUSY
        .replace("VFREEBUSY", "VEVENT")
        .replace("END:VEVENT", "SUMMARY:Planning\r\nEND:VEVENT");
    let message = Message::read(event.as_bytes()).unwrap();
    assert!(Client::new().schedule("/outbox/alice/", &message).is_err());
    for name in ["DTSTAMP", "DTSTART", "DTEND"] {
        let body = BUSY.replace(&format!("{name}:"), &format!("{name};VALUE=X-OPAQUE:"));
        let message = Message::read(body.as_bytes()).unwrap();
        assert!(
            Client::new().schedule("/outbox/alice/", &message).is_err(),
            "{name}"
        );
    }
}

#[test]
fn client_and_server_complete_a_header_independent_busy_time_request() {
    let message = Message::read(BUSY.as_bytes()).unwrap();
    let mut client = Client::new().schedule("/outbox/alice/", &message).unwrap();
    let request = client.next_request().unwrap().clone();
    let mut server = Server::new().handle(request).unwrap();
    assert_eq!(
        server.next_need().unwrap().originator(),
        Some("mailto:alice@example.test")
    );
    server.supply(ServerAnswer::authorized(true)).unwrap();
    let outcomes = ScheduleResponse::new(vec![
        ScheduleDelivery::new("mailto:bob@example.test", "3.7;Invalid calendar user").unwrap(),
        ScheduleDelivery::new("mailto:carol@example.test", "3.7;Invalid calendar user").unwrap(),
    ])
    .unwrap();
    server
        .supply(ServerAnswer::schedule_response(outcomes))
        .unwrap();
    client.accept(server.finish().unwrap()).unwrap();
    assert_eq!(client.finish().unwrap().deliveries().len(), 2);
    assert_eq!(message.to_bytes(), BUSY.as_bytes());
}
