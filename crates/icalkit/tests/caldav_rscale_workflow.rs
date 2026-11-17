// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence scale negotiation follows [RFC 7529 section 10](https://www.rfc-editor.org/rfc/rfc7529.html#section-10).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::caldav::{
    Client, Discovery, Header, Revision, Server, ServerAnswer, WireRequest, WireResponse,
};

fn calendar(property: &str, component: &str) -> Calendar {
    let source = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit scales//EN\r\n\
         BEGIN:VEVENT\r\nUID:scale@example.test\r\nDTSTAMP:20200101T000000Z\r\n\
         DTSTART:20200101T090000Z\r\n{property}END:VEVENT\r\n\
         {component}END:VCALENDAR\r\n"
    );
    Calendar::parse(source.as_bytes()).unwrap()
}

#[test]
fn interpreted_scales_require_a_server_advertisement_before_storage() {
    for name in ["GREGORIAN", "gregory"] {
        let calendar = calendar(&format!("RRULE:FREQ=DAILY;RSCALE={name};COUNT=2\r\n"), "");
        let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
        let Err(error) = Client::new().conditional_write(&revision, &calendar) else {
            panic!("offered a scale-dependent write without a server advertisement");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.caldav.rscale-advertisement-required"
        );
    }
}

#[test]
fn ordinary_rules_still_produce_the_conditional_write_directly() {
    let calendar = calendar("RRULE:FREQ=DAILY;COUNT=2\r\n", "");
    let revision = Revision::stored("/calendars/work/scale.ics", "\"held\"").unwrap();
    let operation = Client::new()
        .conditional_write(&revision, &calendar)
        .unwrap();
    let request = operation.next_request().unwrap();
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.body(), calendar.to_bytes());
    assert!(request.headers().iter().any(|header| {
        header.name().eq_ignore_ascii_case("If-Match") && header.value() == b"\"held\""
    }));
}

#[test]
fn unknown_values_and_properties_do_not_become_interpreted_scale_rules() {
    for property in [
        "RRULE;VALUE=X-FUTURE:RSCALE=CHINESE;FREQ=YEARLY\r\n",
        "X-RRULE:RSCALE=CHINESE;FREQ=YEARLY\r\n",
    ] {
        let calendar = calendar(property, "");
        let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
        let operation = Client::new()
            .conditional_write(&revision, &calendar)
            .unwrap();
        assert_eq!(operation.next_request().unwrap().method(), "PUT");
    }
}

#[test]
fn rules_inside_unknown_components_remain_opaque() {
    let calendar = calendar(
        "",
        "BEGIN:X-FUTURE\r\nRRULE:RSCALE=CHINESE;FREQ=YEARLY\r\nEND:X-FUTURE\r\n",
    );
    let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
    let operation = Client::new()
        .conditional_write(&revision, &calendar)
        .unwrap();
    assert_eq!(operation.next_request().unwrap().method(), "PUT");
}

fn reply(property: &str, status: u16) -> WireResponse {
    WireResponse::new(
        207,
        Vec::new(),
        format!(
            "<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
         <D:response><D:href>/calendars/work/</D:href><D:propstat><D:prop>{property}</D:prop>\
         <D:status>HTTP/1.1 {status} Status</D:status></D:propstat></D:response></D:multistatus>"
        )
        .into_bytes(),
    )
}

fn listed(names: &[&str]) -> String {
    let mut text = String::from("<C:supported-rscale-set>");
    for name in names {
        text.push_str("<C:supported-rscale>");
        text.push_str(name);
        text.push_str("</C:supported-rscale>");
    }
    text.push_str("</C:supported-rscale-set>");
    text
}

#[test]
fn explicit_discovery_preserves_absent_empty_and_named_advertisements() {
    for (property, expected) in [
        (String::new(), None),
        (listed(&[]), Some(Vec::new())),
        (
            listed(&["gregory", "X-FUTURE", "CHINESE"]),
            Some(vec![
                "gregory".to_owned(),
                "X-FUTURE".to_owned(),
                "CHINESE".to_owned(),
            ]),
        ),
    ] {
        let mut operation = Client::new().supported_rscales("/calendars/work/").unwrap();
        let request = operation.next_request().unwrap();
        assert_eq!(request.method(), "PROPFIND");
        assert!(String::from_utf8_lossy(request.body()).contains("supported-rscale-set"));
        assert!(
            request
                .headers()
                .iter()
                .any(|header| header.name() == "Depth" && header.value() == b"0")
        );
        operation.accept(reply(&property, 200)).unwrap();
        assert_eq!(operation.finish().unwrap(), expected);
    }
}

#[test]
fn a_probe_releases_the_write_only_after_successful_scale_support() {
    for advertised in [&["GREGORIAN"][..], &["gReGoRy"][..], &[][..]] {
        let calendar = calendar("RRULE:FREQ=DAILY;RSCALE=GREGORIAN;COUNT=2\r\n", "");
        for revision in [
            Revision::absent("/calendars/work/scale.ics").unwrap(),
            Revision::stored("/calendars/work/scale.ics", "\"held\"").unwrap(),
        ] {
            let mut operation = Client::new()
                .conditional_write_with_rscales("/calendars/work/", &revision, &calendar)
                .unwrap();
            assert_eq!(operation.next_request().unwrap().method(), "PROPFIND");
            operation.accept(reply(&listed(advertised), 200)).unwrap();
            let request = operation.next_request().unwrap();
            assert_eq!(request.method(), "PUT");
            assert_eq!(request.uri(), revision.uri());
            assert_eq!(request.body(), calendar.to_bytes());
            let (name, value) = if revision.is_absent() {
                ("If-None-Match", b"*".as_slice())
            } else {
                ("If-Match", b"\"held\"".as_slice())
            };
            assert!(
                request
                    .headers()
                    .iter()
                    .any(|header| header.name() == name && header.value() == value)
            );
            operation
                .accept(WireResponse::new(
                    201,
                    vec![Header::new("ETag", b"\"stored\"".to_vec())],
                    Vec::new(),
                ))
                .unwrap();
            assert_eq!(operation.finish().unwrap().etag(), Some("\"stored\""));
        }
    }
}

#[test]
fn refused_or_missing_advertisements_never_offer_a_write() {
    for (property, status) in [
        (String::new(), 200),
        (listed(&["CHINESE"]), 200),
        (listed(&["GREGORIAN"]), 403),
        (listed(&[]), 404),
        (listed(&[]), 206),
        (
            "<F:supported-rscale-set xmlns:F=\"urn:foreign\"/>".to_owned(),
            200,
        ),
    ] {
        let calendar = calendar("RRULE:FREQ=DAILY;RSCALE=GREGORIAN;COUNT=2\r\n", "");
        let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
        let mut operation = Client::new()
            .conditional_write_with_rscales("/calendars/work/", &revision, &calendar)
            .unwrap();
        operation.accept(reply(&property, status)).unwrap();
        assert!(operation.next_request().is_none());
        assert!(operation.finish().is_err());
    }
}

#[test]
fn malformed_lists_never_offer_a_write() {
    for property in [
        listed(&[""]),
        listed(&["bad_name"]),
        "<C:supported-rscale-set>GREGORIAN</C:supported-rscale-set>".to_owned(),
        "<C:supported-rscale-set><F:supported-rscale xmlns:F=\"urn:foreign\">GREGORIAN</F:supported-rscale></C:supported-rscale-set>".to_owned(),
        "<C:supported-rscale-set><C:supported-rscale><C:nested/></C:supported-rscale></C:supported-rscale-set>".to_owned(),
        format!("{}{}", listed(&["GREGORIAN"]), listed(&["CHINESE"])),
    ] {
        let calendar = calendar("RRULE:FREQ=DAILY;RSCALE=GREGORIAN;COUNT=2\r\n", "");
        let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
        let mut operation = Client::new().conditional_write_with_rscales(
            "/calendars/work/", &revision, &calendar,
        ).unwrap();
        operation.accept(reply(&property, 200)).unwrap();
        assert!(operation.next_request().is_none());
        assert!(operation.finish().is_err());
    }
}

fn property_response(body: &str, discovery: Discovery) -> WireResponse {
    let request = WireRequest::new(
        "PROPFIND",
        "/calendars/work/",
        Vec::new(),
        body.as_bytes().to_vec(),
    );
    let mut operation = Server::new().handle(request).unwrap();
    assert_eq!(operation.next_need().unwrap().code(), "caldav.authorize");
    operation.supply(ServerAnswer::authorized(true)).unwrap();
    assert_eq!(
        operation.next_need().unwrap().code(),
        "caldav.discovery.properties"
    );
    operation
        .supply(ServerAnswer::discovery(discovery))
        .unwrap();
    operation.finish().unwrap()
}

#[test]
fn invalid_advertisement_names_are_refused_before_serving() {
    let discovery = Discovery::new("/principals/me/", "/calendars/", None::<String>).unwrap();
    for name in ["", "bad_name", "GREGORIAN CHINESE", "foo&bar"] {
        assert!(discovery.clone().with_supported_rscales(&[name]).is_err());
    }
}

#[test]
fn failed_or_incomplete_probe_responses_never_offer_a_write() {
    let valid = reply(&listed(&[]), 200);
    for response in [
        WireResponse::new(500, Vec::new(), valid.body().to_vec()),
        WireResponse::new(200, Vec::new(), valid.body().to_vec()),
        WireResponse::new(207, Vec::new(), b"<D:multistatus".to_vec()),
        WireResponse::new(
            207,
            Vec::new(),
            String::from_utf8(valid.body().to_vec())
                .unwrap()
                .replace("</D:multistatus>", "<D:response><D:href>/other/</D:href><D:status>HTTP/1.1 404 Not Found</D:status></D:response></D:multistatus>")
                .into_bytes(),
        ),
    ] {
        let calendar = calendar("RRULE:FREQ=DAILY;RSCALE=GREGORIAN;COUNT=2\r\n", "");
        let revision = Revision::absent("/calendars/work/scale.ics").unwrap();
        let mut operation = Client::new()
            .conditional_write_with_rscales("/calendars/work/", &revision, &calendar)
            .unwrap();
        operation.accept(response).unwrap();
        assert!(operation.next_request().is_none());
        assert!(operation.finish().is_err());
    }
}

#[test]
fn server_advertisements_require_configuration_and_explicit_selection() {
    let request = "<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\"><D:prop><C:supported-rscale-set/></D:prop></D:propfind>";
    let discovery = Discovery::new("/principals/me/", "/calendars/", None::<String>).unwrap();
    let absent = property_response(request, discovery.clone());
    assert!(String::from_utf8_lossy(absent.body()).contains("404"));
    let names_request = "<D:propfind xmlns:D=\"DAV:\"><D:propname/></D:propfind>";
    let names_without_support = property_response(names_request, discovery.clone());
    assert!(
        !String::from_utf8_lossy(names_without_support.body()).contains("supported-rscale-set")
    );
    for names in [&["GREGORIAN", "CHINESE"][..], &[][..]] {
        let configured = discovery.clone().with_supported_rscales(names).unwrap();
        assert_eq!(configured.advertised_rscales().unwrap(), names);
        let explicit = property_response(request, configured.clone());
        let mut probe = Client::new().supported_rscales("/calendars/work/").unwrap();
        probe.accept(explicit).unwrap();
        assert_eq!(probe.finish().unwrap().unwrap(), names);
        let all = property_response(
            "<D:propfind xmlns:D=\"DAV:\"><D:allprop/></D:propfind>",
            configured.clone(),
        );
        assert!(!String::from_utf8_lossy(all.body()).contains("supported-rscale-set"));
        let included = property_response(
            "<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\"><D:allprop/><D:include><C:supported-rscale-set/></D:include></D:propfind>",
            configured,
        );
        assert!(String::from_utf8_lossy(included.body()).contains("supported-rscale-set"));
        let names_only = property_response(
            names_request,
            discovery.clone().with_supported_rscales(names).unwrap(),
        );
        let body = String::from_utf8_lossy(names_only.body());
        assert!(body.contains("supported-rscale-set"));
        assert!(!body.contains("<C:supported-rscale>"));
    }
}
