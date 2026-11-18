// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Property content keeps ordered element and character children.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    Diagnostic, DiagnosticCode, LimitExceeded, Limits, Meter, Severity,
};
use icalkit_conformance::internal::dav::{
    DavError, DecodeContext, MultiStatus, MultiStatusReader, PropValue, WriteXml, XmlEvent,
    XmlPull, XmlReader,
};

const VALUE_NAMESPACE: &[u8] = b"urn:example:values";

#[derive(Debug, PartialEq, Eq)]
enum Observed {
    Start(Vec<u8>, Vec<u8>),
    Text(Vec<u8>),
    End(Vec<u8>, Vec<u8>),
}

fn start(local_name: &[u8]) -> Observed {
    Observed::Start(VALUE_NAMESPACE.to_vec(), local_name.to_vec())
}

fn text(bytes: &[u8]) -> Observed {
    Observed::Text(bytes.to_vec())
}

fn end(local_name: &[u8]) -> Observed {
    Observed::End(VALUE_NAMESPACE.to_vec(), local_name.to_vec())
}

fn body(tag: &[u8], value: &[u8]) -> Vec<u8> {
    let mut body = br#"<d:multistatus xmlns:d="DAV:" xmlns:v="urn:example:values"><d:response><d:href>/one</d:href><d:propstat><d:prop><"#.to_vec();
    body.extend_from_slice(tag);
    body.extend_from_slice(b">");
    body.extend_from_slice(value);
    body.extend_from_slice(b"</");
    body.extend_from_slice(tag);
    body.extend_from_slice(
        b"></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
    );
    body
}

fn read(
    body: &[u8],
    limits: Limits,
    meter: &mut Meter,
) -> Result<(MultiStatus, Vec<Diagnostic>), DavError> {
    let mut diagnostics = Vec::new();
    let mut events = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut events);
    let value = MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, meter, &mut diagnostics),
    )?;
    Ok((value, diagnostics))
}

fn observed_content(body: &[u8], property: &[u8]) -> Vec<Observed> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
    let mut reader = XmlReader::new(body);
    let mut inside = None;
    let mut observed = Vec::new();
    while let Some(event) = reader.next_event(&mut context).unwrap() {
        match event {
            XmlEvent::Start { name, depth, .. } if inside.is_none() => {
                if name.local_name == property {
                    inside = Some(depth);
                }
            },
            XmlEvent::Start { name, .. } => observed.push(Observed::Start(
                name.namespace.uri().to_vec(),
                name.local_name.to_vec(),
            )),
            XmlEvent::End { depth, .. } if inside == Some(depth) => return observed,
            XmlEvent::End { name, .. } if inside.is_some() => observed.push(Observed::End(
                name.namespace.uri().to_vec(),
                name.local_name.to_vec(),
            )),
            XmlEvent::Text(decoded) if inside.is_some() => {
                if let Some(Observed::Text(held)) = observed.last_mut() {
                    held.extend_from_slice(decoded.run.as_bytes());
                } else {
                    observed.push(text(decoded.run.as_bytes()));
                }
            },
            XmlEvent::End { .. } | XmlEvent::Text(_) => {},
        }
    }
    panic!("the response lost the property");
}

fn assert_property_round_trip(tag: &[u8], property: &[u8], value: &[u8], expected: &[Observed]) {
    let body = body(tag, value);
    assert_eq!(observed_content(&body, property), expected);
    let limits = Limits::DEFAULT;
    let (first, diagnostics) = read(&body, limits, &mut Meter::new(limits)).unwrap();
    let mut encoded = Vec::new();
    first
        .write_xml(&mut encoded, limits, &mut Meter::new(limits))
        .unwrap();
    assert_eq!(observed_content(&encoded, property), expected);
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.code() != DiagnosticCode::DavPropertyMarkupDropped
            && diagnostic.severity() != Severity::Violation
    }));
    assert_eq!(
        read(&encoded, limits, &mut Meter::new(limits)).unwrap().0,
        first
    );
}

fn assert_round_trip(value: &[u8], expected: &[Observed]) {
    assert_property_round_trip(b"v:custom", b"custom", value, expected);
}

#[test]
fn text_before_a_child_keeps_its_position() {
    assert_round_trip(
        b"before<v:child/>",
        &[text(b"before"), start(b"child"), end(b"child")],
    );
}

#[test]
fn text_after_a_child_keeps_its_position() {
    assert_round_trip(
        b"<v:child/>after",
        &[start(b"child"), end(b"child"), text(b"after")],
    );
}

#[test]
fn text_between_children_keeps_its_position() {
    assert_round_trip(
        b"<v:first/>between<v:second/>",
        &[
            start(b"first"),
            end(b"first"),
            text(b"between"),
            start(b"second"),
            end(b"second"),
        ],
    );
}

#[test]
fn nested_and_property_text_share_document_order() {
    assert_round_trip(
        b"before<v:child>inside<v:nested/>tail</v:child>after",
        &[
            text(b"before"),
            start(b"child"),
            text(b"inside"),
            start(b"nested"),
            end(b"nested"),
            text(b"tail"),
            end(b"child"),
            text(b"after"),
        ],
    );
}

#[test]
fn mixed_content_keeps_significant_whitespace() {
    assert_round_trip(
        b" \tbefore\n<v:child/>\n between\t<v:other/> after \n",
        &[
            text(b" \tbefore\n"),
            start(b"child"),
            end(b"child"),
            text(b"\n between\t"),
            start(b"other"),
            end(b"other"),
            text(b" after \n"),
        ],
    );
}

#[test]
fn comments_cdata_and_references_keep_character_order() {
    assert_round_trip(
        b"a<!--split--><![CDATA[<&]]>&#13;<v:child/>z&amp;",
        &[text(b"a<&\r"), start(b"child"), end(b"child"), text(b"z&")],
    );
}

#[test]
fn escaped_element_syntax_stays_character_data() {
    assert_round_trip(
        b"&lt;v:fake/&gt;<v:child/>&lt;/v:fake&gt;",
        &[
            text(b"<v:fake/>"),
            start(b"child"),
            end(b"child"),
            text(b"</v:fake>"),
        ],
    );
}

#[test]
fn mixed_content_in_a_known_plain_property_stays_unmodeled() {
    assert_property_round_trip(
        b"d:displayname",
        b"displayname",
        b"before<v:child/>after",
        &[
            text(b"before"),
            start(b"child"),
            end(b"child"),
            text(b"after"),
        ],
    );
}

#[test]
fn element_only_layout_remains_significant() {
    assert_round_trip(
        b" \n<v:child/>\t ",
        &[text(b" \n"), start(b"child"), end(b"child"), text(b"\t ")],
    );
}

#[test]
fn text_only_values_do_not_pay_the_escaped_fragment_width() {
    let value = vec![b'>'; 1024];
    let body = body(b"v:custom", &value);
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(1024);
    let (read, _) = read(&body, limits, &mut Meter::new(limits)).unwrap();
    assert_eq!(
        read.responses()[0].propstats()[0].props()[0].value,
        PropValue::Unmodeled(value.into_boxed_slice())
    );
}

#[test]
fn an_escaped_mixed_prefix_refuses_before_admitting_a_response() {
    let mut value = vec![b'>'; 24];
    value.extend_from_slice(b"<v:child/>");
    let body = body(b"v:custom", &value);
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(96);
    let mut meter = Meter::new(limits);
    assert_eq!(
        read(&body, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Text))
    );
    assert!(meter.is_exhausted());
    assert_eq!(
        read(&body, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
}
