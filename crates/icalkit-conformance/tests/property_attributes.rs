// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Retained property fragments keep complete namespace and attribute pairs.

#![cfg(test)]

use icalkit_conformance::internal::core::{IgnoreDiagnostics, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DecodeContext, MultiStatus, MultiStatusReader, WriteXml, XmlEvent, XmlPull, XmlReader,
};

const VALUE_NAMESPACE: &[u8] = b"urn:example:values";
const META_NAMESPACE: &[u8] = b"urn:example:metadata";
const OTHER_NAMESPACE: &[u8] = b"urn:example:other";
const XML_NAMESPACE: &[u8] = b"http://www.w3.org/XML/1998/namespace";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservedAttribute {
    namespace: Vec<u8>,
    local_name: Vec<u8>,
    value: Vec<u8>,
}

fn attribute(namespace: &[u8], local_name: &[u8], value: &[u8]) -> ObservedAttribute {
    ObservedAttribute {
        namespace: namespace.to_vec(),
        local_name: local_name.to_vec(),
        value: value.to_vec(),
    }
}

fn read(body: &[u8]) -> MultiStatus {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut diagnostics = IgnoreDiagnostics;
    let mut events = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut events);
    MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut diagnostics),
    )
    .expect("the complete response must be valid XML")
}

fn child_attributes(body: &[u8]) -> Vec<ObservedAttribute> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut diagnostics = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
    let mut reader = XmlReader::new(body);
    while let Some(event) = reader
        .next_event(&mut context)
        .expect("the emitted response must be valid XML")
    {
        if let XmlEvent::Start { name, .. } = event
            && name.namespace.uri() == VALUE_NAMESPACE
            && name.local_name == b"child"
        {
            let mut found = Vec::new();
            for index in 0..reader.attribute_count() {
                let (name, value) = reader.attribute_at(index).unwrap();
                found.push(ObservedAttribute {
                    namespace: name
                        .namespace
                        .uri()
                        .octets()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap(),
                    local_name: name.local_name.to_vec(),
                    value: value.to_vec(),
                });
            }
            found.sort();
            return found;
        }
    }
    panic!("the response lost the child element");
}

fn assert_round_trip(child: &[u8], mut expected: Vec<ObservedAttribute>) {
    let mut body = br#"<d:multistatus xmlns:d="DAV:" xmlns:v="urn:example:values" xmlns:m="urn:example:metadata" xmlns:n="urn:example:other"><d:response><d:href>/one</d:href><d:propstat><d:prop><v:custom>"#.to_vec();
    body.extend_from_slice(child);
    body.extend_from_slice(b"</v:custom></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>");
    let first = read(&body);
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut encoded = Vec::new();
    first.write_xml(&mut encoded, limits, &mut meter).unwrap();
    expected.sort();
    assert_eq!(child_attributes(&encoded), expected);
    assert_eq!(read(&encoded), first);
}

#[test]
fn an_attribute_in_another_namespace_round_trips() {
    assert_round_trip(
        br#"<v:child m:flag="yes"/>"#,
        vec![attribute(META_NAMESPACE, b"flag", b"yes")],
    );
}

#[test]
fn opaque_values_keep_ancestor_bindings_and_nested_rebindings() {
    let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:v="urn:example:values" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:p="urn:outer" xmlns="urn:default" xmlns:X="urn:opaque"><d:response><d:href>/one</d:href><d:propstat><d:prop><v:custom><v:child xsi:type="p:Label"><inner xmlns:p="urn:inner" xsi:type="p:Label"><p:deep/></inner><v:after xsi:type="Label"/></v:child></v:custom></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let captured = read(body);
    let limits = Limits::DEFAULT.with_max_prefix_bindings(14);
    let mut encoded = Vec::new();
    captured
        .write_xml(&mut encoded, limits, &mut Meter::new(limits))
        .unwrap();
    let mut reader = XmlReader::new(&encoded);
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
    let mut seen = Vec::new();
    while let Some(event) = reader.next_event(&mut context).unwrap() {
        if let XmlEvent::Start { name, .. } = event {
            let expected = match name.local_name {
                b"child" | b"after" => b"urn:outer".as_slice(),
                b"inner" | b"deep" => b"urn:inner",
                _ => continue,
            };
            if name.local_name != b"deep" {
                let schema = icalkit_conformance::internal::dav::Namespace::from_uri(
                    b"http://www.w3.org/2001/XMLSchema-instance",
                );
                let kind = icalkit_conformance::internal::dav::QName::new(schema, b"type");
                let value = if name.local_name == b"after" {
                    b"Label".as_slice()
                } else {
                    b"p:Label"
                };
                assert_eq!(reader.attribute(kind), Some(value));
            }
            assert_eq!(reader.resolve_prefix(b"p").unwrap().uri(), expected);
            assert_eq!(
                reader.resolve_prefix(b"").unwrap().uri(),
                b"urn:default".as_slice()
            );
            assert_eq!(
                reader.resolve_prefix(b"X").unwrap().uri(),
                b"urn:opaque".as_slice()
            );
            seen.push(name.local_name.to_vec());
        }
    }
    assert_eq!(seen, [b"child".as_slice(), b"inner", b"deep", b"after"]);
    let xml = std::str::from_utf8(&encoded).unwrap();
    assert!(xml.contains("<v:child"));
    assert!(xml.contains("xsi:type="));
    assert!(xml.contains("<p:deep"));
    assert_eq!(xml.matches("xmlns:p=").count(), 2);
    assert_eq!(read(&encoded), captured);
}

#[test]
fn an_attribute_in_the_element_namespace_round_trips() {
    assert_round_trip(
        br#"<v:child v:flag="yes"/>"#,
        vec![attribute(VALUE_NAMESPACE, b"flag", b"yes")],
    );
}

#[test]
fn namespace_limits_cover_the_envelope_and_the_captured_fragment_together() {
    use std::fmt::Write;

    use icalkit_conformance::internal::core::LimitExceeded;
    use icalkit_conformance::internal::dav::DavError;

    let limits = Limits::DEFAULT;
    let cap = limits.max_prefix_bindings();
    let admitted = cap.checked_sub(8).unwrap();
    for declarations in [admitted, cap.checked_sub(7).unwrap(), cap] {
        let mut body = String::from(r#"<d:multistatus xmlns:d="DAV:" xmlns:v="urn:values""#);
        for index in 2..declarations {
            write!(body, r#" xmlns:p{index}="urn:{index}""#).unwrap();
        }
        body.push_str("><d:response><d:href>/one</d:href><d:propstat><d:prop><v:custom><v:child/></v:custom></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>");
        let captured = read(body.as_bytes());
        let mut encoded = Vec::new();
        let mut meter = Meter::new(limits);
        let written = captured.write_xml(&mut encoded, limits, &mut meter);
        if declarations == admitted {
            written.unwrap();
            assert_eq!(read(&encoded), captured);
        } else {
            assert_eq!(written, Err(DavError::Limit(LimitExceeded::PrefixBindings)));
            assert!(meter.is_exhausted());
        }
    }
}

#[test]
fn plain_and_two_namespace_attributes_round_trip() {
    assert_round_trip(
        br#"<v:child plain="zero" m:flag="one" n:flag="two"/>"#,
        vec![
            attribute(b"", b"plain", b"zero"),
            attribute(META_NAMESPACE, b"flag", b"one"),
            attribute(OTHER_NAMESPACE, b"flag", b"two"),
        ],
    );
}

#[test]
fn attribute_references_keep_their_normalized_values() {
    assert_round_trip(
        br#"<v:child m:note="a&#9;b&#10;c&#13;d&amp;&quot;&apos;&lt;&gt;"/>"#,
        vec![attribute(META_NAMESPACE, b"note", b"a\tb\nc\rd&\"'<>")],
    );
}

#[test]
fn xml_namespace_attributes_need_no_redeclaration() {
    assert_round_trip(
        br#"<v:child xml:lang="ja" xml:base="/root"/>"#,
        vec![
            attribute(XML_NAMESPACE, b"lang", b"ja"),
            attribute(XML_NAMESPACE, b"base", b"/root"),
        ],
    );
}

#[test]
fn unprefixed_attributes_keep_the_empty_namespace() {
    assert_round_trip(
        br#"<child xmlns="urn:example:values" flag="yes"/>"#,
        vec![attribute(b"", b"flag", b"yes")],
    );
}
