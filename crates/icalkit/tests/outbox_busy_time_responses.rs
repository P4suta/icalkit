// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Public busy time response data and request correlation workflows.

#![cfg(test)]

use icalkit::caldav::{
    Client, ScheduleDelivery, ScheduleResponse, Server, ServerAnswer, WireResponse,
};
use icalkit::scheduling::Message;

const ASK: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit outbox tests//EN\r\n",
    "METHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\nUID:busy@example.test\r\n",
    "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n",
    "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
    "END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
);

fn reply() -> String {
    ASK.replace("METHOD:REQUEST", "METHOD:REPLY").replace(
        "END:VFREEBUSY",
        "FREEBUSY:20240102T100000Z/20240102T110000Z\r\nCOMMENT:Work & travel <private>\r\nX-TRACE:one\r\n two\r\nEND:VFREEBUSY",
    )
}

fn response(recipient: &str, data: &str) -> WireResponse {
    let data = data
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    WireResponse::new(
        200,
        Vec::new(),
        format!(
            "<C:schedule-response xmlns:C=\"urn:ietf:params:xml:ns:caldav\" xmlns:D=\"DAV:\">\
         <C:response><C:recipient><D:href>{recipient}</D:href></C:recipient>\
         <C:request-status>2.0;Success</C:request-status>\
         <C:calendar-data>{data}</C:calendar-data></C:response></C:schedule-response>"
        )
        .into_bytes(),
    )
}

#[test]
fn the_client_keeps_the_validated_reply_and_exact_calendar_octets() {
    let data = reply();
    let message = Message::read(ASK.as_bytes()).unwrap();
    let mut operation = Client::new().schedule("/outbox/alice/", &message).unwrap();
    operation.next_request().unwrap();
    operation
        .accept(response("mailto:bob@example.test", &data))
        .unwrap();
    let result = operation.finish().unwrap();
    assert!(!result.is_no_content());
    let delivery = &result.deliveries()[0];
    assert_eq!(delivery.calendar_data().unwrap().method(), "REPLY");
    assert_eq!(
        delivery.calendar_data().unwrap().to_bytes(),
        data.as_bytes()
    );
}

#[test]
fn a_successful_reply_can_describe_no_busy_periods() {
    let data = ASK.replace("METHOD:REQUEST", "METHOD:REPLY");
    let delivery = ScheduleDelivery::new(
        "mailto:bob@example.test",
        "2.0;Success",
        Some(Message::read(data.as_bytes()).unwrap()),
    )
    .unwrap();
    assert!(delivery.is_success());
    assert_eq!(
        delivery.calendar_data().unwrap().to_bytes(),
        data.as_bytes()
    );
}

#[test]
fn data_is_required_for_success_and_must_describe_its_recipient() {
    assert!(ScheduleDelivery::new("mailto:bob@example.test", "2.0;Success", None).is_err());
    let wrong = reply().replace("mailto:bob@example.test", "mailto:elsewhere@example.test");
    assert!(
        ScheduleDelivery::new(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(Message::read(wrong.as_bytes()).unwrap())
        )
        .is_err()
    );
    assert!(
        ScheduleDelivery::new(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(Message::read(ASK.as_bytes()).unwrap())
        )
        .is_err()
    );
}

#[test]
fn client_and_server_preserve_reply_data_through_xml() {
    let message = Message::read(ASK.as_bytes()).unwrap();
    let data = Message::read(reply().as_bytes()).unwrap();
    let delivery =
        ScheduleDelivery::new("mailto:bob@example.test", "2.0;Success", Some(data.clone()))
            .unwrap();
    let result = ScheduleResponse::new(vec![delivery]).unwrap();
    let mut client = Client::new().schedule("/outbox/alice/", &message).unwrap();
    let mut server = Server::new()
        .handle(client.next_request().unwrap().clone())
        .unwrap();
    server.supply(ServerAnswer::authorized(true)).unwrap();
    server
        .supply(ServerAnswer::schedule_response(result))
        .unwrap();
    client.accept(server.finish().unwrap()).unwrap();
    let result = client.finish().unwrap();
    assert_eq!(result.deliveries()[0].calendar_data(), Some(&data));
}

#[test]
fn a_no_content_success_is_explicit_on_both_sides() {
    let message = Message::read(ASK.as_bytes()).unwrap();
    let mut client = Client::new().schedule("/outbox/alice/", &message).unwrap();
    let mut server = Server::new()
        .handle(client.next_request().unwrap().clone())
        .unwrap();
    server.supply(ServerAnswer::authorized(true)).unwrap();
    server
        .supply(ServerAnswer::schedule_response(
            ScheduleResponse::no_content(),
        ))
        .unwrap();
    let wire = server.finish().unwrap();
    assert_eq!(wire.status(), 204);
    assert_eq!(wire.body(), []);
    client.accept(wire).unwrap();
    let result = client.finish().unwrap();
    assert!(result.is_no_content());
    assert_eq!(result.deliveries(), []);
}

#[test]
fn the_server_refuses_reply_data_for_another_request() {
    let message = Message::read(ASK.as_bytes()).unwrap();
    for (before, after) in [
        ("UID:busy@example.test", "UID:elsewhere@example.test"),
        (
            "ORGANIZER:mailto:alice@example.test",
            "ORGANIZER:mailto:elsewhere@example.test",
        ),
    ] {
        let data = Message::read(reply().replace(before, after).as_bytes()).unwrap();
        let delivery =
            ScheduleDelivery::new("mailto:bob@example.test", "2.0;Success", Some(data)).unwrap();
        let client = Client::new().schedule("/outbox/alice/", &message).unwrap();
        let mut server = Server::new()
            .handle(client.next_request().unwrap().clone())
            .unwrap();
        server.supply(ServerAnswer::authorized(true)).unwrap();
        assert!(
            server
                .supply(ServerAnswer::schedule_response(
                    ScheduleResponse::new(vec![delivery]).unwrap()
                ))
                .is_err(),
            "{after}"
        );
    }
}

#[test]
fn response_correlation_decodes_identifiers_once() {
    let asked = ASK.replace("UID:busy@example.test", "UID:busy\\nvalue");
    let data = reply().replace("UID:busy@example.test", "UID:busy\\Nvalue");
    let message = Message::read(asked.as_bytes()).unwrap();
    let mut client = Client::new().schedule("/outbox/alice/", &message).unwrap();
    client.next_request().unwrap();
    client
        .accept(response("mailto:bob@example.test", &data))
        .unwrap();
    assert_eq!(
        client.finish().unwrap().deliveries()[0]
            .calendar_data()
            .unwrap()
            .to_bytes(),
        data.as_bytes()
    );
}

#[test]
fn the_server_can_forward_calendar_data_with_its_original_lf_lines() {
    let message = Message::read(ASK.as_bytes()).unwrap();
    let data = reply().replace("\r\n", "\n");
    let mut first = Client::new().schedule("/outbox/alice/", &message).unwrap();
    first.next_request().unwrap();
    first
        .accept(response("mailto:bob@example.test", &data))
        .unwrap();
    let received = first.finish().unwrap();
    let mut client = Client::new().schedule("/outbox/alice/", &message).unwrap();
    let mut server = Server::new()
        .handle(client.next_request().unwrap().clone())
        .unwrap();
    server.supply(ServerAnswer::authorized(true)).unwrap();
    server
        .supply(ServerAnswer::schedule_response(received))
        .unwrap();
    client.accept(server.finish().unwrap()).unwrap();
    let result = client.finish().unwrap();
    assert_eq!(
        result.deliveries()[0].calendar_data_bytes(),
        Some(data.as_bytes())
    );
}
