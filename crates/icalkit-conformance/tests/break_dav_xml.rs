// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regression cases from the XML boundary audit.
//!
//! Parser and response writer checks cover hostile input, preserved content, and shared limits.

#![cfg(test)]

use std::fs;
use std::path::PathBuf;

use icalkit_conformance::internal::core::{
    Diagnostic, IgnoreDiagnostics, LimitExceeded, Limits, Meter,
};
use icalkit_conformance::internal::dav::{
    DavError, DecodeContext, MultiStatus, MultiStatusReader, PropValue, RequestBody, SyntaxError,
    WriteXml, XmlEvent, XmlPull, XmlReader,
};

/// The octets of one fixture in this attacker's directory.
///
/// Read from disk rather than written inline, because `.gitattributes` marks the directory `-text` and several of these fixtures are not text at all — two are UTF-16 and three carry octets no encoding admits.
fn fixture(name: &str) -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push("break_dav_xml");
    path.push(name);
    let found = fs::read(&path);
    assert!(
        found.is_ok(),
        "reading {}: {:?}",
        path.display(),
        found.err()
    );
    found.unwrap_or_default()
}

/// Read a whole multistatus out of octets, through the tokenizer that ships.
fn read_multistatus(body: &[u8]) -> Result<MultiStatus, DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
    let mut events = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut events);
    MultiStatus::read(&mut source, &mut context)
}

/// Read a request body out of octets, through the same tokenizer.
fn read_request(body: &[u8]) -> Result<RequestBody, DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
    let mut events = XmlReader::new(body);
    RequestBody::read(&mut events, &mut context)
}

/// Encode one value into a fresh buffer under the default bounds.
fn encode(value: &dyn WriteXml) -> Result<Vec<u8>, DavError> {
    let limits = Limits::DEFAULT;
    let mut out: Vec<u8> = Vec::new();
    let mut meter = Meter::new(limits);
    value.write_xml(&mut out, limits, &mut meter)?;
    Ok(out)
}

/// Drain a body through the tokenizer alone, answering the first refusal or `Ok`.
fn tokenize(body: &[u8], limits: Limits, meter: &mut Meter) -> Result<(), DavError> {
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, meter, &mut sink);
    let mut reader = XmlReader::new(body);
    loop {
        match reader.next_event(&mut context) {
            Ok(None) => return Ok(()),
            Ok(Some(_)) => {},
            Err(refusal) => return Err(refusal),
        }
    }
}

/// Drain a body under a ledger of its own.
fn refusal(body: &[u8]) -> Result<(), DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    tokenize(body, limits, &mut meter)
}

fn property_text(collected: &MultiStatus) -> Vec<u8> {
    let response = collected.responses().first().expect("the response exists");
    let group = response.propstats().first().expect("the group exists");
    let property = group.props().first().expect("the property exists");
    match &property.value {
        PropValue::Unmodeled(octets) => octets.to_vec(),
        PropValue::Markup(fragment) => {
            let mut body = b"<value>".to_vec();
            body.extend_from_slice(fragment);
            body.extend_from_slice(b"</value>");
            let limits = Limits::DEFAULT;
            let mut meter = Meter::new(limits);
            let mut diagnostics = IgnoreDiagnostics;
            let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
            let mut reader = XmlReader::new(&body);
            let mut text = Vec::new();
            while let Some(event) = reader
                .next_event(&mut context)
                .expect("the fragment is XML")
            {
                if let XmlEvent::Text(decoded) = event {
                    text.extend_from_slice(decoded.run.as_bytes());
                }
            }
            text
        },
        other => panic!("unexpected property value: {other:?}"),
    }
}

/// Whether `needle` appears anywhere in `haystack`.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A root element wrapping `levels` nested `DAV:response` elements.
fn nested(levels: usize) -> Vec<u8> {
    let mut body = Vec::from(&br#"<D:multistatus xmlns:D="DAV:">"#[..]);
    for _ in 0..levels {
        body.extend_from_slice(b"<D:response>");
    }
    for _ in 0..levels {
        body.extend_from_slice(b"</D:response>");
    }
    body.extend_from_slice(b"</D:multistatus>");
    body
}

/// A root element holding `bytes` octets of the shape `wrap` describes.
fn filled(bytes: usize, opener: &[u8], closer: &[u8]) -> Vec<u8> {
    let mut body = Vec::from(&br#"<D:multistatus xmlns:D="DAV:">"#[..]);
    body.extend_from_slice(opener);
    body.extend(core::iter::repeat_n(b'a', bytes));
    body.extend_from_slice(closer);
    body.extend_from_slice(b"</D:multistatus>");
    body
}

// -------------------------------------------------------------------------------------------
// The five breaks.
// -------------------------------------------------------------------------------------------

/// Text a peer escaped must not come out of the encoder as markup.
#[test]
fn escaped_text_does_not_become_markup_on_the_way_out() {
    let body = fixture("promoted-markup.xml");
    let collected = read_multistatus(&body).expect("the body reads");
    assert_eq!(
        property_text(&collected),
        b"<D:href>/calendars/ann/private/secret.ics</D:href>",
        "the decoded text is what the crate kept"
    );
    let wire = encode(&collected).expect("the multistatus re-encodes");
    assert!(
        !holds(&wire, b"<D:href>/calendars/ann/private/secret.ics</D:href>"),
        "text was promoted to markup: {}",
        String::from_utf8_lossy(&wire)
    );
}

/// A complete property survives reading, writing, and reading again.
#[test]
fn a_kept_property_reads_back_as_the_value_that_wrote_it() {
    let body = fixture("promoted-propstat.xml");
    let collected = read_multistatus(&body).expect("the body reads");
    let wire = encode(&collected).expect("the multistatus re-encodes");
    let again = read_multistatus(&wire).expect("the re-encode reads");
    assert_eq!(
        again,
        collected,
        "the octets changed across one re-encode: {}",
        String::from_utf8_lossy(&wire)
    );
}

/// A value this crate read is a value this crate can write, and can read back.
#[test]
fn a_kept_value_carrying_an_ampersand_survives_the_encoder() {
    let refused = fixture("unencodable-ampersand.xml");
    let collected = read_multistatus(&refused).expect("the body reads");
    assert_eq!(property_text(&collected), b"AT&T");
    let wire = encode(&collected).expect("the multistatus re-encodes");
    assert_eq!(
        read_multistatus(&wire).expect("the emitted response reads"),
        collected
    );

    let corrupting = fixture("bare-ampersand.xml");
    let second = read_multistatus(&corrupting).expect("the body reads");
    let wire = encode(&second).expect("this one encodes");
    assert_eq!(
        read_multistatus(&wire).expect("the emitted response reads"),
        second,
        "the encoder emitted octets its own reader refuses: {}",
        String::from_utf8_lossy(&wire)
    );
}

/// A comment costs what its octets cost.
#[test]
fn a_comment_is_charged_against_the_ledger_like_the_octets_it_is() {
    let bytes = 8 * 1024 * 1024;
    let spoken = filled(bytes, b"", b"");
    let commented = filled(bytes, b"<!--", b"-->");
    let limits = Limits::DEFAULT;

    let mut loud = Meter::new(limits);
    assert_eq!(
        tokenize(&spoken, limits, &mut loud),
        Err(DavError::Limit(LimitExceeded::Text)),
        "character data is bounded"
    );

    let mut quiet = Meter::new(limits);
    let outcome = tokenize(&commented, limits, &mut quiet);
    assert!(
        quiet.spent() >= u64::try_from(bytes).unwrap_or(u64::MAX) || outcome.is_err(),
        "{bytes} octets of comment cost {} of the ledger",
        quiet.spent()
    );
}

/// One ledger bounds aggregate comment bytes across bodies.
#[test]
fn one_ledger_across_many_bodies_bounds_what_a_peer_can_spend() {
    let commented = filled(1024 * 1024, b"<!--", b"-->");
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut bodies: u64 = 0;
    while !meter.is_exhausted() && bodies < 32 {
        let _ = tokenize(&commented, limits, &mut meter);
        bodies = bodies.saturating_add(1);
    }
    assert!(
        meter.is_exhausted(),
        "{bodies} bodies of one mebibyte each spent {} of a {} octet ledger",
        meter.spent(),
        meter.budget()
    );
}

/// An attribute value is what XML 1.0 section 3.3.3 says it is.
#[test]
fn an_attribute_value_means_what_xml_1_0_says_it_means() {
    let body = fixture("attribute-character-reference.xml");
    let read = read_request(&body).expect("the query reads");
    let RequestBody::CalendarQuery(query) = read else {
        panic!("the root is a calendar-query");
    };
    let root = query.filter.as_ref().expect("the query carries a filter");
    let outer = root.comps().first().expect("one nested filter");
    assert_eq!(
        outer.name(),
        b"VENT".as_slice(),
        "a character reference in an attribute value was not resolved"
    );
}

/// Whatever a reference may not spell, an octet may not spell either.
#[test]
fn a_character_the_char_production_excludes_is_refused_however_it_is_spelled() {
    let expected = Err(DavError::Syntax(SyntaxError::ForbiddenCharacter));
    assert_eq!(
        refusal(&fixture("nul-as-reference.xml")),
        expected,
        "the reference is refused"
    );
    let cases = [
        ("a NUL written as an octet", "nul-as-octet.xml"),
        (
            "a backspace written as an octet",
            "control-char-as-octet.xml",
        ),
        ("octets that are not UTF-8", "invalid-utf8-text.xml"),
    ];
    for (shape, name) in cases {
        assert_eq!(refusal(&fixture(name)), expected, "{shape}");
    }
}

// -------------------------------------------------------------------------------------------
// What held.
// -------------------------------------------------------------------------------------------

/// Every entity attack `SECURITY.md` names is refused at the declaration that would carry it.
///
/// There is no I/O in this crate to be redirected, and no expansion budget to be raced: the `DOCTYPE` itself is the refusal, in any casing and behind a comment.
#[test]
fn the_doctype_family_is_refused_where_it_would_be_declared() {
    let cases = [
        "xxe-external-general-entity.xml",
        "xxe-parameter-entity.xml",
        "billion-laughs.xml",
        "external-dtd-subset.xml",
        "doctype-behind-a-comment.xml",
    ];
    for name in cases {
        assert_eq!(
            refusal(&fixture(name)),
            Err(DavError::Syntax(SyntaxError::Doctype)),
            "{name}"
        );
    }
}

/// A body written in an encoding this crate does not read never becomes a document.
#[test]
fn an_encoding_this_crate_does_not_read_is_refused() {
    for name in ["utf16le-bom.xml", "utf16be-bom.xml", "utf16le-no-bom.xml"] {
        assert!(refusal(&fixture(name)).is_err(), "{name}");
    }
    // A UTF-8 byte order mark is a server's to write and is skipped rather than refused.
    assert_eq!(refusal(&fixture("utf8-bom.xml")), Ok(()));
}

/// The truncations, each under its own name.
#[test]
fn a_construct_the_body_ends_inside_is_refused_rather_than_read_to_the_end() {
    let cases = [
        "unterminated-attribute.xml",
        "unterminated-comment.xml",
        "unterminated-cdata.xml",
    ];
    for name in cases {
        assert_eq!(
            refusal(&fixture(name)),
            Err(DavError::Syntax(SyntaxError::Truncated)),
            "{name}"
        );
    }
}

/// Namespace declarations preserve default bindings and enforce reserved prefixes.
#[test]
fn a_prefix_is_the_document_s_choice_and_the_reserved_ones_are_not() {
    assert_eq!(refusal(&fixture("dav-under-the-empty-prefix.xml")), Ok(()));
    assert_eq!(
        refusal(&fixture("undeclared-prefix.xml")),
        Err(DavError::Syntax(SyntaxError::UnboundPrefix))
    );
    assert_eq!(
        refusal(&fixture("xml-prefix-rebound.xml")),
        Err(DavError::Syntax(SyntaxError::Malformed))
    );
    assert_eq!(
        refusal(&fixture("xmlns-prefix-declared.xml")),
        Err(DavError::Syntax(SyntaxError::Malformed))
    );
}

/// Depth, one name and one attribute, each bounded and none of them on the caller's stack.
///
/// Ten thousand levels is refused at `Limits::max_xml_depth` under the default policy and read through without a frame of recursion when the policy admits it, which is what DP-14 promised and what an explicit `Vec` of open elements buys.
#[test]
fn nesting_a_name_and_an_attribute_are_all_bounded() {
    let deep = nested(10_000);
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    assert_eq!(
        tokenize(&deep, limits, &mut meter),
        Err(DavError::Limit(LimitExceeded::Depth))
    );

    let raised = limits.with_max_xml_depth(u16::MAX);
    let mut roomy = Meter::new(raised);
    assert_eq!(tokenize(&deep, raised, &mut roomy), Ok(()));

    // A megabyte-long element name is a foreign element and nothing worse.
    let mut named = Vec::from(&br#"<D:multistatus xmlns:D="DAV:"><D:"#[..]);
    named.extend(core::iter::repeat_n(b'a', 1024 * 1024));
    named.extend_from_slice(b"/></D:multistatus>");
    let mut counting = Meter::new(limits);
    assert_eq!(tokenize(&named, limits, &mut counting), Ok(()));

    // A hundred-megabyte attribute value never becomes resident.
    let mut wide = Vec::from(&br#"<D:multistatus xmlns:D="DAV:" x=""#[..]);
    wide.extend(core::iter::repeat_n(b'a', 100 * 1024 * 1024));
    wide.extend_from_slice(b"\"/>");
    let mut charged = Meter::new(limits);
    assert!(tokenize(&wide, limits, &mut charged).is_err());
}
