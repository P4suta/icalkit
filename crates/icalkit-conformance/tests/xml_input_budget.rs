// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Supplied XML consumes the shared input budget before lexical validation.

#![cfg(test)]

use icalkit_conformance::internal::core::{LimitExceeded, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DavError, DecodeContext, TextMode, TextPolicy, XmlEvent, XmlPull, XmlReader, decode_text,
};

fn first_event<'a>(
    reader: &mut XmlReader<'a>,
    limits: Limits,
    meter: &mut Meter,
) -> Result<Option<XmlEvent<'a>>, DavError> {
    reader.next_event(&mut DecodeContext::new(limits, meter, &mut Vec::new()))
}

fn drain_text(body: &[u8], limits: Limits, meter: &mut Meter) -> Result<Vec<u8>, DavError> {
    let mut reader = XmlReader::new(body);
    let mut diagnostics = Vec::new();
    let mut context = DecodeContext::new(limits, meter, &mut diagnostics);
    let mut text = Vec::new();
    while let Some(event) = reader.next_event(&mut context)? {
        if let XmlEvent::Text(decoded) = event {
            text.extend_from_slice(decoded.run.as_bytes());
        }
    }
    assert!(reader.next_event(&mut context)?.is_none());
    Ok(text)
}

#[test]
fn malformed_lexical_input_consumes_supplied_bytes() {
    for body in [b"<".as_slice(), b"<x a=", b"\xff"] {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reader = XmlReader::new(body);
        let result = first_event(&mut reader, Limits::DEFAULT, &mut meter);
        assert!(matches!(result, Err(DavError::Syntax(_))));
        assert_eq!(meter.spent(), u64::try_from(body.len()).unwrap());
        assert!(!meter.is_exhausted());
    }
}

#[test]
fn malformed_xml_refuses_bytes_before_syntax() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 0);
    let mut reader = XmlReader::new(b"<");
    assert_eq!(
        first_event(&mut reader, Limits::DEFAULT, &mut meter),
        Err(DavError::Limit(LimitExceeded::Budget))
    );
    assert_eq!(meter.spent(), 1);
    assert!(meter.is_exhausted());
    assert_eq!(
        first_event(&mut reader, Limits::DEFAULT, &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
    assert_eq!(meter.spent(), 1);
}

#[test]
fn an_early_root_return_accounts_for_the_unread_body() {
    let mut body = b"<x>".to_vec();
    body.extend_from_slice(&[b'a'; 512]);
    body.extend_from_slice(b"</x>");
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reader = XmlReader::new(&body);
    assert!(matches!(
        first_event(&mut reader, Limits::DEFAULT, &mut meter),
        Ok(Some(XmlEvent::Start { .. }))
    ));
    assert_eq!(meter.spent(), u64::try_from(body.len()).unwrap());
    assert_eq!(reader.offset(), 3);
}

#[test]
fn complete_bodies_charge_wire_bytes_once_at_the_exact_ceiling() {
    let cases: [(&[u8], &[u8]); 4] = [
        (b"<x/>", b""),
        (
            b"\xef\xbb\xbf<?xml version=\"1.0\"?><!--before--> \n<x><!--inside--><y/></x><!--after--> \t",
            b"",
        ),
        (b"<x>a<y/>b</x>", b"ab"),
        (b"<x>a&amp;<![CDATA[b]]>&#13;</x>", b"a&b\r"),
    ];
    for (body, expected) in cases {
        let budget = u64::try_from(body.len()).unwrap();
        let mut meter = Meter::with_budget(Limits::DEFAULT, budget);
        assert_eq!(
            drain_text(body, Limits::DEFAULT, &mut meter).unwrap(),
            expected
        );
        assert_eq!(meter.spent(), budget);
        assert!(!meter.is_exhausted());
        assert_eq!(meter.try_charge_bytes(0), Ok(()));
    }
}

#[test]
fn byte_refusal_precedes_attribute_storage() {
    let body = br#"<x a="&amp;"/>"#;
    let length = u64::try_from(body.len()).unwrap();
    let mut meter = Meter::with_budget(Limits::DEFAULT, length - 1);
    let mut reader = XmlReader::new(body);
    assert_eq!(
        first_event(&mut reader, Limits::DEFAULT, &mut meter),
        Err(DavError::Limit(LimitExceeded::Budget))
    );
    assert_eq!(meter.spent(), length);
    assert!(meter.is_exhausted());
    assert_eq!(reader.offset(), 0);
}

#[test]
fn attribute_storage_remains_separate_from_wire_bytes() {
    let body = br#"<x a="&amp;"/>"#;
    let length = u64::try_from(body.len()).unwrap();
    let mut meter = Meter::with_budget(Limits::DEFAULT, length);
    let mut reader = XmlReader::new(body);
    assert_eq!(
        first_event(&mut reader, Limits::DEFAULT, &mut meter),
        Err(DavError::Limit(LimitExceeded::Budget))
    );
    assert!(meter.spent() > length);
    assert!(meter.is_exhausted());
}

#[test]
fn a_body_cap_refusal_precedes_wire_admission() {
    let limits = Limits::DEFAULT.with_max_response_bytes(0);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reader = XmlReader::new(b"<");
    assert_eq!(
        first_event(&mut reader, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Budget))
    );
    assert_eq!(meter.spent(), 0);
    assert!(meter.is_exhausted());
}

#[test]
fn admitted_xml_still_enforces_text_width() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(1);
    let body = b"<x>xx</x>";
    let mut meter = Meter::new(limits);
    let mut reader = XmlReader::new(body);
    assert!(matches!(
        first_event(&mut reader, limits, &mut meter),
        Ok(Some(XmlEvent::Start { .. }))
    ));
    assert_eq!(
        first_event(&mut reader, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Text))
    );
    assert_eq!(meter.spent(), u64::try_from(body.len()).unwrap());
    assert!(meter.is_exhausted());
}

#[test]
fn standalone_text_keeps_its_input_charge() {
    let limits = Limits::DEFAULT;
    let raw = b"a&amp;";
    let mut meter = Meter::with_budget(limits, u64::try_from(raw.len()).unwrap());
    let mode = TextMode::of(None, TextPolicy::Verbatim);
    let decoded = decode_text(raw, mode, 0, &mut meter, &mut Vec::new()).unwrap();
    assert_eq!(decoded.run.as_bytes(), b"a&");
    assert_eq!(meter.spent(), u64::try_from(raw.len()).unwrap());
    assert!(!meter.is_exhausted());
    assert_eq!(
        decode_text(b"x", mode, 0, &mut meter, &mut Vec::new()),
        Err(DavError::Limit(LimitExceeded::Budget))
    );
    assert!(meter.is_exhausted());
    assert_eq!(
        decode_text(b"", mode, 0, &mut meter, &mut Vec::new()),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
}
