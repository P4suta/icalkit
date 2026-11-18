// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scale advertisements and refusals follow [RFC 7529 section 10](https://www.rfc-editor.org/rfc/rfc7529.html#section-10).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::caldav::{Client, Header, Revision, Server, ServerAnswer, WireRequest, WireResponse};
use icalkit_conformance::internal::core::{IgnoreDiagnostics, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DecodeContext, ElementName, XmlEvent, XmlPull, XmlReader,
};

fn calendar_bytes(rule: &str) -> Vec<u8> {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit scale conformance//EN\r\n\
         BEGIN:VEVENT\r\nUID:scale@example.test\r\nDTSTAMP:20200101T000000Z\r\n\
         DTSTART:20200101T090000Z\r\n{rule}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    .into_bytes()
}

fn response(property: &str) -> WireResponse {
    WireResponse::new(
        207,
        Vec::new(),
        format!(
            "<multistatus xmlns=\"DAV:\"><response><href>/cal/</href><propstat><prop>\
         {property}</prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"
        )
        .into_bytes(),
    )
}

#[test]
fn default_namespaces_entities_and_cdata_preserve_calendar_names() {
    let property = "<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\">\
        <supported-rscale>GREG&#79;<![CDATA[RIAN]]></supported-rscale>\
        <supported-rscale>X-FUTURE</supported-rscale></supported-rscale-set>";
    let mut operation = Client::new().supported_rscales("/cal/").unwrap();
    operation.accept(response(property)).unwrap();
    assert_eq!(
        operation.finish().unwrap().unwrap(),
        ["GREGORIAN", "X-FUTURE"]
    );
}

#[test]
fn advertisement_requires_the_probed_resource() {
    for (probe, href, accepted) in [
        ("/cal/", "/elsewhere/", false),
        ("https://example.test/cal/", "/cal/", true),
        ("/cal/", "https://example.test/cal/", true),
        (
            "https://example.test/cal/",
            "https://other.test/cal/",
            false,
        ),
        ("/cal/?view=1", "/cal/?view=2", false),
    ] {
        let body = response("<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\"/>");
        let xml = std::str::from_utf8(body.body())
            .unwrap()
            .replace(">/cal/<", &format!(">{href}<"));
        let mut operation = Client::new().supported_rscales(probe).unwrap();
        operation
            .accept(WireResponse::new(207, Vec::new(), xml.into_bytes()))
            .unwrap();
        assert_eq!(operation.finish().is_ok(), accepted, "{probe} {href}");
    }
}

#[test]
fn advertisement_validates_the_document_epilog() {
    for (suffix, accepted) in [(b" \n<!-- complete -->".as_slice(), true), (&[0xff], false)] {
        let mut xml = response("<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\"/>")
            .body()
            .to_vec();
        xml.extend_from_slice(suffix);
        let mut operation = Client::new().supported_rscales("/cal/").unwrap();
        operation
            .accept(WireResponse::new(207, Vec::new(), xml))
            .unwrap();
        assert_eq!(operation.finish().is_ok(), accepted);
    }
}

#[test]
fn empty_advertisements_accept_xml_layout_whitespace() {
    for (text, accepted) in [(" \r\n\t", true), ("GREGORIAN", false)] {
        let mut operation = Client::new().supported_rscales("/cal/").unwrap();
        operation.accept(response(&format!(
            "<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\">{text}</supported-rscale-set>"
        ))).unwrap();
        if accepted {
            assert_eq!(operation.finish().unwrap(), Some(Vec::new()));
        } else {
            assert!(operation.finish().is_err());
        }
    }
}

#[test]
fn folded_and_case_insensitive_rule_controls_still_need_negotiation() {
    for rule in [
        "RRULE:FREQ=DAILY;rscale=gregory;COUNT=2",
        "RRULE:FREQ=DAILY;RSCALE=GREGO\r\n RIAN;COUNT=2",
    ] {
        let source = calendar_bytes(rule);
        let calendar = Calendar::parse(&source).unwrap();
        let revision = Revision::absent("/cal/event.ics").unwrap();
        assert!(
            Client::new()
                .conditional_write(&revision, &calendar)
                .is_err()
        );
        let mut operation = Client::new()
            .conditional_write_with_rscales("/cal/", &revision, &calendar)
            .unwrap();
        operation.accept(response("<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\"><supported-rscale>GREGORIAN</supported-rscale></supported-rscale-set>")).unwrap();
        assert_eq!(operation.next_request().unwrap().body(), source);
    }
}

#[test]
fn oversized_property_markup_refuses_the_conditional_write() {
    let length = usize::try_from(Limits::DEFAULT.max_xml_text_bytes())
        .unwrap()
        .checked_add(1)
        .unwrap();
    let property = format!(
        "<supported-rscale-set xmlns=\"urn:ietf:params:xml:ns:caldav\">\
         <supported-rscale>GREGORIAN</supported-rscale>\
         <supported-rscale>{}</supported-rscale></supported-rscale-set>",
        "A".repeat(length),
    );
    let calendar =
        Calendar::parse(&calendar_bytes("RRULE:FREQ=DAILY;RSCALE=GREGORIAN;COUNT=2")).unwrap();
    let revision = Revision::absent("/cal/event.ics").unwrap();
    let mut operation = Client::new()
        .conditional_write_with_rscales("/cal/", &revision, &calendar)
        .unwrap();
    operation.accept(response(&property)).unwrap();
    assert!(operation.next_request().is_none());
    assert!(operation.finish().is_err());
}

fn raw_write(rule: &str) -> WireRequest {
    WireRequest::new(
        "PUT",
        "/cal/event.ics",
        vec![
            Header::new("Content-Type", b"text/calendar".to_vec()),
            Header::new("If-None-Match", b"*".to_vec()),
        ],
        calendar_bytes(rule),
    )
}

#[test]
fn unsupported_calendar_names_return_the_named_precondition_after_authorization() {
    let mut operation = Server::new()
        .handle(raw_write("RRULE:FREQ=YEARLY;RSCALE=ISLAMIC"))
        .unwrap();
    assert_eq!(operation.next_need().unwrap().code(), "caldav.authorize");
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    assert!(operation.next_need().is_none());
    let refusal = operation.finish().unwrap();
    assert_eq!(refusal.status(), 403);
    let mut reader = XmlReader::new(refusal.body());
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(Limits::DEFAULT, &mut meter, &mut sink);
    let mut elements = Vec::new();
    while let Some(event) = reader.next_event(&mut context).unwrap() {
        if let XmlEvent::Start { known, depth, .. } = event {
            elements.push((depth, known));
        }
    }
    assert_eq!(
        elements,
        [
            (1, Some(ElementName::Error)),
            (2, Some(ElementName::SupportedRscale))
        ]
    );
}

#[test]
fn authorization_refusal_does_not_need_a_calendar_capability_answer() {
    let mut operation = Server::new()
        .handle(raw_write("RRULE:FREQ=YEARLY;RSCALE=HEBREW"))
        .unwrap();
    operation.supply(ServerAnswer::authorized(false)).unwrap();
    let refusal = operation.finish().unwrap();
    assert_eq!(refusal.status(), 403);
    assert_eq!(refusal.body(), b"");
}

#[test]
fn unsupported_patterns_are_distinct_from_unsupported_calendar_names() {
    let Err(error) = Server::new().handle(raw_write(
        "RRULE:FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYWEEKNO=53",
    )) else {
        panic!("accepted an unsupported recurrence pattern");
    };
    assert_eq!(
        error.code().as_str(),
        "icalkit.caldav.write-request-invalid"
    );
}
