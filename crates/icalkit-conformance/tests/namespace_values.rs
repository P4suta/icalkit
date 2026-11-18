// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Namespace identity uses normalized attribute values under XML Namespaces 1.0 section 2.3.

#![cfg(test)]

use icalkit_conformance::internal::core::{IgnoreDiagnostics, LimitExceeded, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DavError, DecodeContext, ElementName, ExtensionName, MultiStatus, MultiStatusReader, Namespace,
    PropName, QName, RequestBody, SyntaxError, WriteXml, XmlEvent, XmlPull, XmlReader, XmlWriter,
};

fn namespace(body: &[u8]) -> Namespace<'_> {
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    match first(&mut reader, limits, &mut Meter::new(limits)).unwrap() {
        Some(XmlEvent::Start { name, .. }) => name.namespace,
        other => panic!("expected a named element: {other:?}"),
    }
}

fn first<'a>(
    reader: &mut XmlReader<'a>,
    limits: Limits,
    meter: &mut Meter,
) -> Result<Option<XmlEvent<'a>>, DavError> {
    reader.next_event(&mut DecodeContext::new(
        limits,
        meter,
        &mut IgnoreDiagnostics,
    ))
}

fn assert_known_root(body: &[u8], expected: ElementName) {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reader = XmlReader::new(body);
    let Some(XmlEvent::Start { known, .. }) = first(&mut reader, limits, &mut meter).unwrap()
    else {
        panic!("the root must open");
    };
    assert_eq!(known, Some(expected));
    assert!(matches!(
        first(&mut reader, limits, &mut meter).unwrap(),
        Some(XmlEvent::End { known: Some(closed), .. }) if closed == expected
    ));
    assert_eq!(first(&mut reader, limits, &mut meter).unwrap(), None);
}

fn request(body: &[u8]) -> Result<RequestBody, DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reader = XmlReader::new(body);
    RequestBody::read(
        &mut reader,
        &mut DecodeContext::new(limits, &mut meter, &mut IgnoreDiagnostics),
    )
}

fn response(body: &[u8]) -> MultiStatus {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reader = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut reader);
    MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut IgnoreDiagnostics),
    )
    .unwrap()
}

#[test]
fn decimal_references_identify_the_dav_vocabulary() {
    assert_known_root(
        br#"<d:propfind xmlns:d="D&#65;V:"/>"#,
        ElementName::Propfind,
    );
}

#[test]
fn hexadecimal_references_identify_the_caldav_vocabulary() {
    assert_known_root(
        br#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:cal&#x64;av"/>"#,
        ElementName::CalendarQuery,
    );
}

#[test]
fn encoded_vendor_namespaces_identify_their_vocabulary() {
    assert_known_root(
        br#"<s:getctag xmlns:s="http://calendarserver.org/ns&#47;"/>"#,
        ElementName::Getctag,
    );
}

#[test]
fn default_declarations_normalize_before_element_resolution() {
    assert_known_root(br#"<propfind xmlns="D&#65;V:"/>"#, ElementName::Propfind);
}

#[test]
fn encoded_declarations_drive_the_request_reader() {
    let encoded = br#"<d:propfind xmlns:d="D&#65;V:"><d:allprop/></d:propfind>"#;
    let literal = br#"<d:propfind xmlns:d="DAV:"><d:allprop/></d:propfind>"#;
    let expected = request(literal).unwrap();
    assert_eq!(request(encoded).unwrap(), expected);
}

#[test]
fn unknown_namespace_names_survive_response_round_trips() {
    let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:v="urn:example:a&amp;b"><d:response><d:href>/one</d:href><d:propstat><d:prop><v:custom><v:child/></v:custom></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let read = response(body);
    let property = &read.responses()[0].propstats()[0].props()[0];
    let PropName::Extension(name) = &property.name else {
        panic!("the custom property must keep its name");
    };
    assert_eq!(name.namespace(), b"urn:example:a&b");
    let limits = Limits::DEFAULT;
    let mut wire = Vec::new();
    read.write_xml(&mut wire, limits, &mut Meter::new(limits))
        .unwrap();
    assert_eq!(response(&wire), read);
    assert!(!wire.windows(9).any(|part| part == b"&amp;amp;"));
}

#[test]
fn equivalent_namespace_spellings_collide_as_expanded_attribute_names() {
    let body = br#"<root xmlns:a="urn:example:x" xmlns:b="urn:example:&#120;" a:flag="one" b:flag="two"/>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    let mut meter = Meter::new(limits);
    assert_eq!(
        first(&mut reader, limits, &mut meter),
        Err(DavError::Syntax(SyntaxError::DuplicateAttribute))
    );
    assert!(!meter.is_exhausted());
}

#[test]
fn normalized_namespace_names_resolve_attribute_queries() {
    let body = br#"<root xmlns:a="urn:example:&#120;" a:flag="yes"/>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    first(&mut reader, limits, &mut Meter::new(limits)).unwrap();
    let name = QName::new(Namespace::from_uri(b"urn:example:x"), b"flag");
    assert_eq!(reader.attribute(name), Some(b"yes".as_slice()));
    assert!(reader.resolve_prefix(b"a").unwrap().is(name.namespace));
}

#[test]
fn normalized_bindings_shadow_and_restore_by_scope() {
    let body = br#"<d:response xmlns:d="D&#65;V:"><d:calendar xmlns:d="urn:ietf:params:xml:ns:cal&#100;av"/><d:href/></d:response>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    let mut meter = Meter::new(limits);
    let mut names = Vec::new();
    while let Some(event) = first(&mut reader, limits, &mut meter).unwrap() {
        if let XmlEvent::Start { known, .. } = event {
            names.push(known);
        }
    }
    assert_eq!(
        names,
        [
            Some(ElementName::Response),
            Some(ElementName::Calendar),
            Some(ElementName::Href)
        ]
    );
}

#[test]
fn namespace_identity_preserves_case_and_percent_escapes() {
    for uri in [b"dav:".as_slice(), b"D%41V:"] {
        let mut body = b"<d:response xmlns:d=\"".to_vec();
        body.extend_from_slice(uri);
        body.extend_from_slice(b"\"/>");
        let limits = Limits::DEFAULT;
        let mut reader = XmlReader::new(&body);
        assert!(matches!(
            first(&mut reader, limits, &mut Meter::new(limits)).unwrap(),
            Some(XmlEvent::Start { known: None, .. })
        ));
    }
}

#[test]
fn unused_declarations_still_reject_undefined_entities() {
    let body = br#"<root xmlns:unused="urn:example:&missing;"/>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    assert_eq!(
        first(&mut reader, limits, &mut Meter::new(limits)),
        Err(DavError::Syntax(SyntaxError::UndefinedEntity))
    );
}

#[test]
fn namespace_normalization_keeps_character_data_limits_separate() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(0);
    let mut reader = XmlReader::new(br#"<d:response xmlns:d="D&#65;V:"/>"#);
    assert!(matches!(
        first(&mut reader, limits, &mut Meter::new(limits)).unwrap(),
        Some(XmlEvent::Start {
            known: Some(ElementName::Response),
            ..
        })
    ));
}

#[test]
fn namespace_bindings_refuse_and_latch_shared_exhaustion() {
    let limits = Limits::DEFAULT.with_max_prefix_bindings(0);
    let mut reader = XmlReader::new(br#"<d:response xmlns:d="D&#65;V:"/>"#);
    let mut meter = Meter::new(limits);
    assert_eq!(
        first(&mut reader, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::PrefixBindings))
    );
    assert!(meter.is_exhausted());
    assert_eq!(
        first(&mut reader, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
}

#[test]
fn reserved_namespace_bindings_use_normalized_names() {
    let cases: &[&[u8]] = &[
        br#"<root xmlns:xml="urn:wrong"/>"#,
        br#"<root xmlns:p="http://www.w3.org/XML/1998/namespace"/>"#,
        br#"<root xmlns:p="http://www.w3.org/XML/1998/namespac&#101;"/>"#,
        br#"<root xmlns="http://www.w3.org/XML/1998/namespace"/>"#,
        br#"<root xmlns:xmlns="DAV:"/>"#,
        br#"<root xmlns:p="http://www.w3.org/2000/xmlns/"/>"#,
        br#"<root xmlns:p="http://www.w3.org/2000/xmlns&#47;"/>"#,
        br#"<root xmlns="http://www.w3.org/2000/xmlns/"/>"#,
    ];
    for body in cases {
        let limits = Limits::DEFAULT;
        let mut reader = XmlReader::new(body);
        assert_eq!(
            first(&mut reader, limits, &mut Meter::new(limits)),
            Err(DavError::Syntax(SyntaxError::Malformed)),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn the_xml_prefix_can_declare_its_encoded_reserved_name() {
    let body = br#"<root xmlns:xml="http://www.w3.org/XML/1998/namespac&#x65;" xml:lang="ja"/>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    first(&mut reader, limits, &mut Meter::new(limits)).unwrap();
    let name = QName::new(
        Namespace::from_uri(b"http://www.w3.org/XML/1998/namespace"),
        b"lang",
    );
    assert_eq!(reader.attribute(name), Some(b"ja".as_slice()));
}

#[test]
fn other_prefixes_starting_with_xml_are_not_fatal() {
    let body = br#"<xmlcustom:root xmlns:xmlcustom="urn:example:x"/>"#;
    let limits = Limits::DEFAULT;
    let mut reader = XmlReader::new(body);
    assert!(matches!(
        first(&mut reader, limits, &mut Meter::new(limits)).unwrap(),
        Some(XmlEvent::Start { known: None, .. })
    ));
}

#[test]
fn equivalent_namespace_names_compare_equal() {
    let left = namespace(br#"<a:x xmlns:a="urn:example:a&amp;b"/>"#);
    let right = namespace(br#"<b:x xmlns:b="urn:example:a&#38;b"/>"#);
    assert_eq!(left, right);
}

#[test]
fn namespace_order_uses_decoded_names() {
    let left = namespace(br#"<a:x xmlns:a="urn:example:a&amp;b"/>"#);
    let right = namespace(br#"<b:x xmlns:b="urn:example:a&#38;c"/>"#);
    assert_eq!(left.cmp(&right), std::cmp::Ordering::Less);
}

#[test]
fn equivalent_namespace_names_have_the_same_hash() {
    use std::hash::{Hash, Hasher};

    let left = namespace(br#"<a:x xmlns:a="urn:example:a&amp;b"/>"#);
    let right = namespace(br#"<b:x xmlns:b="urn:example:a&#38;b"/>"#);
    let fingerprint = |name: Namespace<'_>| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(fingerprint(left), fingerprint(right));
}

#[test]
fn xml_namespace_property_names_round_trip_without_rebinding() {
    let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:xml="http://www.w3.org/XML/1998/namespac&#101;"><d:response><d:href>/one</d:href><d:propstat><d:prop><xml:custom>value</xml:custom></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let read = response(body);
    let limits = Limits::DEFAULT;
    let mut wire = Vec::new();
    read.write_xml(&mut wire, limits, &mut Meter::new(limits))
        .unwrap();
    assert_eq!(response(&wire), read);
}

#[test]
fn the_writer_refuses_the_declaration_namespace_before_output() {
    let limits = Limits::DEFAULT;
    let name = ExtensionName::new(
        b"http://www.w3.org/2000/xmlns/",
        b"custom",
        &mut Meter::new(limits),
    )
    .unwrap();
    let mut wire = Vec::new();
    let mut meter = Meter::new(limits);
    {
        let mut writer = XmlWriter::new(&mut wire);
        assert_eq!(
            writer.open_extension(&name, &mut meter),
            Err(DavError::Syntax(SyntaxError::Malformed))
        );
        assert_eq!(writer.depth(), 0);
    }
    assert_eq!(wire.as_slice(), b"");
    assert!(!meter.is_exhausted());
}

#[test]
fn namespace_output_refuses_forbidden_characters_before_output() {
    for uri in [b"urn:\0".as_slice(), b"urn:\xff"] {
        let limits = Limits::DEFAULT;
        let name = ExtensionName::new(uri, b"custom", &mut Meter::new(limits)).unwrap();
        let mut wire = Vec::new();
        let mut writer = XmlWriter::new(&mut wire);
        assert_eq!(
            writer.open_extension(&name, &mut Meter::new(limits)),
            Err(DavError::Syntax(SyntaxError::ForbiddenCharacter))
        );
        assert_eq!(wire.as_slice(), b"");
    }
}

#[test]
fn shared_exhaustion_precedes_namespace_output_validation() {
    let limits = Limits::DEFAULT;
    let name = ExtensionName::new(
        b"http://www.w3.org/2000/xmlns/",
        b"custom",
        &mut Meter::new(limits),
    )
    .unwrap();
    let mut meter = Meter::with_budget(limits, 0);
    assert_eq!(meter.try_charge_bytes(1), Err(LimitExceeded::Budget));
    let mut wire = Vec::new();
    let mut writer = XmlWriter::new(&mut wire);
    assert_eq!(
        writer.open_extension(&name, &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
    assert_eq!(wire.as_slice(), b"");
}
