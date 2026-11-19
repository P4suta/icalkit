// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Retained markup shares the element budget used by generated XML.

#![cfg(test)]

use icalkit_conformance::internal::core::{LimitExceeded, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DavError, ElementName, PropValue, SyntaxError, WriteXml, XmlWriter,
};

fn markup(octets: &[u8]) -> PropValue {
    PropValue::Markup(octets.to_vec().into_boxed_slice())
}

fn assert_element_refusal(octets: &[u8], meter: &mut Meter) {
    let mut out = Vec::new();
    assert_eq!(
        markup(octets).write_xml(&mut out, meter.limits(), meter),
        Err(DavError::Limit(LimitExceeded::Elements))
    );
    assert_eq!(out.as_slice(), []);
    assert!(meter.is_exhausted());
    assert_eq!(meter.spent(), 0);
    assert_eq!(meter.try_charge_bytes(0), Err(LimitExceeded::Exhausted));
}

#[test]
fn an_empty_tag_refuses_the_zero_element_ceiling() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_xml_elements(0));
    assert_element_refusal(b"<x/>", &mut meter);
}

#[test]
fn an_open_tag_refuses_the_zero_element_ceiling() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_xml_elements(0));
    assert_element_refusal(b"<x></x>", &mut meter);
}

#[test]
fn a_flat_fragment_refuses_before_emitting_an_admitted_prefix() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_xml_elements(1));
    assert_element_refusal(b"<x/><y/>", &mut meter);
}

#[test]
fn a_nested_fragment_refuses_its_second_element() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_xml_elements(1));
    assert_element_refusal(b"<x><y/></x>", &mut meter);
}

#[test]
fn one_element_uses_one_slot_in_both_representations() {
    for octets in [
        b"<x/>".as_slice(),
        b"<x></x>",
        br#"<x title="&lt;y/&gt; >"/>"#,
    ] {
        let limits = Limits::DEFAULT.with_max_xml_elements(1);
        let mut meter = Meter::new(limits);
        let mut out = Vec::new();
        markup(octets)
            .write_xml(&mut out, limits, &mut meter)
            .unwrap();
        assert_eq!(out, octets);
        assert!(!meter.is_exhausted());
        assert_eq!(meter.try_charge_bytes(0), Ok(()));
        assert_eq!(meter.try_charge_element(), Err(LimitExceeded::Elements));
        let mut retry = Vec::new();
        assert_eq!(
            markup(b"").write_xml(&mut retry, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Exhausted))
        );
        assert_eq!(retry.as_slice(), []);
    }
}

#[test]
fn nested_closing_tags_use_no_additional_slots() {
    let limits = Limits::DEFAULT.with_max_xml_elements(2);
    let mut meter = Meter::new(limits);
    let mut out = Vec::new();
    let octets = b"<x><y/></x>";
    markup(octets)
        .write_xml(&mut out, limits, &mut meter)
        .unwrap();
    assert_eq!(out, octets);
    assert!(!meter.is_exhausted());
    assert_eq!(meter.try_charge_element(), Err(LimitExceeded::Elements));
}

#[test]
fn generated_and_retained_elements_share_the_running_count() {
    let limits = Limits::DEFAULT.with_max_xml_elements(2);
    let mut meter = Meter::new(limits);
    let mut generated = Vec::new();
    XmlWriter::fragment(&mut generated)
        .empty(ElementName::Getetag, &mut meter)
        .unwrap();
    let spent = meter.spent();
    let mut out = Vec::new();
    assert_eq!(
        markup(b"<x/><y/>").write_xml(&mut out, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Elements))
    );
    assert_eq!(out.as_slice(), []);
    assert_eq!(meter.spent(), spent);
    assert!(meter.is_exhausted());
}

#[test]
fn escaped_text_needs_no_element_slots() {
    let limits = Limits::DEFAULT.with_max_xml_elements(0);
    let mut meter = Meter::new(limits);
    let mut out = Vec::new();
    let octets = b"text &lt;x/&gt;";
    markup(octets)
        .write_xml(&mut out, limits, &mut meter)
        .unwrap();
    assert_eq!(out, octets);
    assert!(!meter.is_exhausted());
}

#[test]
fn a_stray_closing_tag_keeps_its_syntax_error() {
    let limits = Limits::DEFAULT.with_max_xml_elements(0);
    let mut meter = Meter::new(limits);
    let mut out = Vec::new();
    assert_eq!(
        markup(b"</x>").write_xml(&mut out, limits, &mut meter),
        Err(DavError::Syntax(SyntaxError::Malformed))
    );
    assert_eq!(out.as_slice(), []);
    assert!(!meter.is_exhausted());
    assert_eq!(meter.try_charge_bytes(0), Ok(()));
}

#[test]
fn a_depth_refusal_keeps_its_dimension() {
    let limits = Limits::DEFAULT
        .with_max_xml_depth(5)
        .with_max_xml_elements(0);
    let mut meter = Meter::new(limits);
    let mut out = Vec::new();
    assert_eq!(
        markup(b"<x/>").write_xml(&mut out, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Depth))
    );
    assert_eq!(out.as_slice(), []);
    assert!(meter.is_exhausted());
}
