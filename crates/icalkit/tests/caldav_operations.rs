// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV workflows driven entirely through the public sans-I/O boundary.

use icalkit::caldav::{
    Client, Header, Server, ServerAnswer, ServerNeed, WireRequest, WireResponse,
};

#[test]
fn client_operation_yields_one_request_and_decodes_one_response() {
    let request = WireRequest::new(
        "PROPFIND",
        "/calendars/alice/",
        vec![Header::new("Depth", b"1".to_vec())],
        b"<propfind/>".to_vec(),
    );
    let client = Client::new();
    let mut operation = client.operation(request, |response| Ok(response.status()));

    let pending = operation.next_request().unwrap();
    assert_eq!(pending.method(), "PROPFIND");
    assert_eq!(pending.uri(), "/calendars/alice/");
    assert_eq!(pending.headers()[0].name(), "Depth");

    operation
        .accept(WireResponse::new(
            207,
            Vec::new(),
            b"<multistatus/>".to_vec(),
        ))
        .unwrap();
    assert!(operation.next_request().is_none());
    assert_eq!(operation.finish().unwrap(), 207);
}

#[test]
fn client_operation_refuses_out_of_order_transitions() {
    let client = Client::new();
    let incomplete = client.operation(
        WireRequest::new("REPORT", "/calendar/", Vec::new(), Vec::new()),
        |_| Ok(()),
    );
    assert_eq!(
        incomplete.finish().unwrap_err().code().as_str(),
        "icalkit.caldav.operation-incomplete"
    );

    let mut completed = client.operation(
        WireRequest::new("REPORT", "/calendar/", Vec::new(), Vec::new()),
        |_| Ok(()),
    );
    completed
        .accept(WireResponse::new(200, Vec::new(), Vec::new()))
        .unwrap();
    assert_eq!(
        completed
            .accept(WireResponse::new(200, Vec::new(), Vec::new()))
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.caldav.unexpected-response"
    );
}

#[test]
fn server_operation_yields_an_application_need_and_builds_a_response() {
    let server = Server::new();
    let mut operation = server.operation(ServerNeed::new("calendar.load"), |answer| {
        Ok(WireResponse::new(200, Vec::new(), answer.body().to_vec()))
    });

    assert_eq!(operation.next_need().unwrap().code(), "calendar.load");
    operation
        .supply(ServerAnswer::new(b"BEGIN:VCALENDAR\r\n".to_vec()))
        .unwrap();
    assert!(operation.next_need().is_none());
    assert_eq!(operation.finish().unwrap().body(), b"BEGIN:VCALENDAR\r\n");
}
