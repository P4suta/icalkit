// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Busy time responses follow [RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5).
//! Reply identity follows [RFC 5546 section 3.3.3](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.3).
//! Calendar data follows [RFC 4791 section 9.6](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6).

#![cfg(test)]

use icalkit::Error;
use icalkit::caldav::{Client, ScheduleResponse, WireResponse};
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
        "FREEBUSY:20240102T100000Z/20240102T110000Z\r\nEND:VFREEBUSY",
    )
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn outcome(recipient: &str, status: &str, data: Option<&str>) -> String {
    let data = data.map_or_else(String::new, |data| {
        format!("<C:calendar-data>{}</C:calendar-data>", xml_text(data))
    });
    format!(
        "<C:response><C:recipient><D:href>{}</D:href></C:recipient>\
         <C:request-status>{status}</C:request-status>{data}</C:response>",
        xml_text(recipient)
    )
}

fn document(children: &str) -> String {
    format!(
        "<C:schedule-response xmlns:C=\"urn:ietf:params:xml:ns:caldav\" xmlns:D=\"DAV:\">\
         {children}</C:schedule-response>"
    )
}

fn decode(status: u16, body: &str) -> Result<ScheduleResponse, Error> {
    decode_for(ASK, status, body)
}

fn decode_for(request: &str, status: u16, body: &str) -> Result<ScheduleResponse, Error> {
    let message = Message::read(request.as_bytes()).unwrap();
    let mut operation = Client::new().schedule("/outbox/alice/", &message).unwrap();
    operation.next_request().unwrap();
    operation.accept(WireResponse::new(
        status,
        Vec::new(),
        body.as_bytes().to_vec(),
    ))?;
    operation.finish()
}

#[test]
fn successful_replies_cover_the_entire_requested_interval() {
    for (start, end, covered) in [
        ("20240102T000001Z", "20240103T000000Z", false),
        ("20240102T000000Z", "20240102T235959Z", false),
        ("20240103T100000Z", "20240103T110000Z", false),
        ("20240102T000000Z", "20240103T000000Z", true),
        ("20240101T235959Z", "20240103T000001Z", true),
    ] {
        let data = ASK
            .replace("METHOD:REQUEST", "METHOD:REPLY")
            .replace("DTSTART:20240102T000000Z", &format!("DTSTART:{start}"))
            .replace("DTEND:20240103T000000Z", &format!("DTEND:{end}"));
        Message::read(data.as_bytes()).unwrap();
        let body = document(&outcome(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(&data),
        ));
        assert_eq!(decode(200, &body).is_ok(), covered, "{start} to {end}");
    }
}

#[test]
fn nonempty_responses_include_every_requested_recipient() {
    let request = ASK.replace(
        "END:VFREEBUSY",
        "ATTENDEE:mailto:carol@example.test\r\nEND:VFREEBUSY",
    );
    let bob = outcome("mailto:bob@example.test", "3.7;Invalid calendar user", None);
    let carol = outcome(
        "mailto:carol@example.test",
        "3.7;Invalid calendar user",
        None,
    );
    let error = decode_for(&request, 200, &document(&bob)).unwrap_err();
    assert_eq!(
        error.code().as_str(),
        "icalkit.caldav.schedule-response-incomplete"
    );
    assert!(decode_for(&request, 200, &document(&format!("{bob}{carol}"))).is_ok());
}

#[test]
fn successful_recipient_outcomes_require_calendar_data() {
    let body = document(&outcome("mailto:bob@example.test", "2.0;Success", None));
    assert!(decode(200, &body).is_err());
}

#[test]
fn calendar_data_must_be_a_free_busy_reply() {
    let event = reply()
        .replace("VFREEBUSY", "VEVENT")
        .replace("FREEBUSY:20240102T100000Z/20240102T110000Z\r\n", "");
    for data in [
        "not an iCalendar object".to_string(),
        ASK.to_string(),
        event,
    ] {
        let body = document(&outcome(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(&data),
        ));
        assert!(decode(200, &body).is_err(), "{data}");
    }
}

#[test]
fn replies_with_unreadable_required_values_are_refused() {
    for name in [
        "UID",
        "DTSTAMP",
        "DTSTART",
        "DTEND",
        "ORGANIZER",
        "ATTENDEE",
    ] {
        let data = reply().replace(&format!("{name}:"), &format!("{name};VALUE=X-OPAQUE:"));
        let body = document(&outcome(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(&data),
        ));
        assert!(decode(200, &body).is_err(), "{name}");
    }
}

#[test]
fn reply_identifiers_and_organizers_match_the_original_request() {
    for (before, after) in [
        ("UID:busy@example.test", "UID:unrelated@example.test"),
        (
            "ORGANIZER:mailto:alice@example.test",
            "ORGANIZER:mailto:elsewhere@example.test",
        ),
    ] {
        let data = reply().replace(before, after);
        Message::read(data.as_bytes()).unwrap();
        let body = document(&outcome(
            "mailto:bob@example.test",
            "2.0;Success",
            Some(&data),
        ));
        assert!(decode(200, &body).is_err(), "{after}");
    }
}

#[test]
fn a_reply_attendee_matches_the_enclosing_recipient() {
    let data = reply().replace(
        "ATTENDEE:mailto:bob@example.test",
        "ATTENDEE:mailto:elsewhere@example.test",
    );
    Message::read(data.as_bytes()).unwrap();
    let body = document(&outcome(
        "mailto:bob@example.test",
        "2.0;Success",
        Some(&data),
    ));
    assert!(decode(200, &body).is_err());
}

#[test]
fn structured_responses_cover_exactly_the_requested_recipients() {
    for children in [
        String::new(),
        outcome(
            "mailto:elsewhere@example.test",
            "3.7;Invalid calendar user",
            None,
        ),
        format!(
            "{}{}",
            outcome("mailto:bob@example.test", "3.7;Invalid calendar user", None),
            outcome(
                "mailto:elsewhere@example.test",
                "3.7;Invalid calendar user",
                None
            )
        ),
    ] {
        assert!(decode(200, &document(&children)).is_err(), "{children}");
    }
}

#[test]
fn http_204_is_a_success_without_recipient_data() {
    assert!(decode(204, "").is_ok());
}

#[test]
fn http_204_cannot_carry_a_response_body() {
    assert!(decode(204, &document("")).is_err());
}

#[test]
fn duplicated_calendar_data_is_refused() {
    let data = reply();
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data)).replace(
        "</C:calendar-data>",
        &format!(
            "</C:calendar-data><C:calendar-data>{}</C:calendar-data>",
            xml_text(&data)
        ),
    );
    assert!(decode(200, &document(&child)).is_err());
}

#[test]
fn scalar_response_values_cannot_hide_nested_elements_or_repeated_hrefs() {
    let data = reply();
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data));
    for forged in [
        child.replace("<C:calendar-data>", "<C:calendar-data><D:href/>"),
        child.replace(
            "mailto:bob@example.test</D:href>",
            "mailto:bo</D:href><D:href>b@example.test</D:href>",
        ),
        child.replace(
            "2.0;Success</C:request-status>",
            "2.0;<D:href/>Success</C:request-status>",
        ),
    ] {
        assert!(decode(200, &document(&forged)).is_err(), "{forged}");
    }
}

#[test]
fn failed_recipients_can_omit_calendar_data() {
    let body = document(&outcome(
        "mailto:bob@example.test",
        "3.7;Invalid calendar user",
        None,
    ));
    let response = decode(200, &body).unwrap();
    assert_eq!(response.deliveries().len(), 1);
    assert!(!response.deliveries()[0].is_success());
}

#[test]
fn calendar_data_declarations_cannot_claim_another_representation() {
    let data = reply();
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data));
    for attributes in [
        "content-type=\"application/calendar+json\"",
        "content-type=\"text/plain\"",
        "version=\"3.0\"",
        "content-type=\"text/calendar; method=REQUEST\"",
        "content-type=\"text/calendar; component=VEVENT\"",
        "content-type=\"text/calendar; charset=iso-8859-1\"",
    ] {
        let child = child.replace(
            "<C:calendar-data>",
            &format!("<C:calendar-data {attributes}>"),
        );
        assert!(decode(200, &document(&child)).is_err(), "{attributes}");
    }
}

#[test]
fn calendar_data_accepts_its_explicit_representation_and_lf_lines() {
    let data = reply().replace("\r\n", "\n");
    assert!(Message::read(data.as_bytes()).is_err());
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data)).replace(
        "<C:calendar-data>",
        "<C:calendar-data content-type=\"Text/Calendar; method=reply; component=VFREEBUSY; charset=UTF-8\" version=\"2.0\">",
    );
    let result = decode(200, &document(&child)).unwrap();
    assert_eq!(
        result.deliveries()[0].calendar_data_bytes(),
        Some(data.as_bytes())
    );
    assert_eq!(
        result.deliveries()[0].calendar_data().unwrap().to_bytes(),
        data.replace('\n', "\r\n").as_bytes()
    );
}

#[test]
fn cdata_and_character_references_preserve_calendar_octets() {
    let data = reply().replace(
        "END:VFREEBUSY",
        "COMMENT:Work & travel <private> π\r\nEND:VFREEBUSY",
    );
    let encoded = outcome("mailto:bob@example.test", "2.0;Success", Some(&data));
    for child in [
        encoded.clone(),
        encoded.replace(&xml_text(&data), &format!("<![CDATA[{data}]]>")),
    ] {
        let result = decode(200, &document(&child)).unwrap();
        assert_eq!(
            result.deliveries()[0].calendar_data().unwrap().to_bytes(),
            data.as_bytes()
        );
    }
}

#[test]
fn unknown_response_extensions_do_not_supply_known_fields() {
    let valid = outcome("mailto:bob@example.test", "3.7;Invalid calendar user", None);
    let hidden = outcome("mailto:elsewhere@example.test", "2.0;Success", None);
    let body = document(&format!(
        "<X:ignored xmlns:X=\"urn:icalkit:test\">{hidden}</X:ignored>{valid}"
    ));
    let result = decode(200, &body).unwrap();
    assert_eq!(result.deliveries().len(), 1);
    assert_eq!(
        result.deliveries()[0].recipient(),
        "mailto:bob@example.test"
    );
}

#[test]
fn an_ascii_declaration_cannot_label_non_ascii_calendar_data() {
    let data = reply().replace("END:VFREEBUSY", "COMMENT:π\r\nEND:VFREEBUSY");
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data)).replace(
        "<C:calendar-data>",
        "<C:calendar-data content-type=\"text/calendar; charset=us-ascii\">",
    );
    assert!(decode(200, &document(&child)).is_err());
}

#[test]
fn a_bare_carriage_return_is_not_a_transport_line_ending() {
    let data = reply().replace("\r\nDTSTAMP", "\rDTSTAMP");
    let child = outcome("mailto:bob@example.test", "2.0;Success", Some(&data));
    assert!(decode(200, &document(&child)).is_err());
}
