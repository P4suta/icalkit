// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The line-ending collision, proved against bodies shaped like the ones real servers send.
//!
//! XML 1.0 section 2.11 requires a conformant processor to fold every `CRLF` and every lone
//! `CR` to `LF` before parsing. RFC 5545 section 3.1 makes that same `CRLF` the syntax of a
//! content line. `SabreDAV` and Radicale both write those octets literally inside
//! `<C:calendar-data>`; Calendar Server writes the `CR` as `&#13;`. Read conformantly, the
//! first two hand `ical-core` a calendar the server did not store, and writing it back changes
//! the resource and its `ETag` with nobody having edited anything.
//!
//! The three fixtures below are that exchange in the three spellings the deployed world uses,
//! with three different namespace prefixes — `d:`/`cal:`, `ns0:`/`ns1:`, and a default `DAV:`
//! declaration with `C:` beside it — because a reader keyed on prefix strings gets two of the
//! three wrong. The assertions are what this crate's design hands `ical-core`.

use icalkit_conformance::internal::core::{IgnoreDiagnostics, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DavError, DecodedText, LineEndings, TextMode, decode_text,
};

/// The `.ics` all three servers are carrying, byte for byte.
const PAYLOAD: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/calendar-data-payload.ics");

const SABREDAV: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/sabredav-calendar-multiget.xml");
const RADICALE: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/radicale-calendar-multiget.xml");
const CALENDAR_SERVER: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/calendarserver-calendar-multiget.xml");

/// The octets between a `calendar-data` start tag and its end tag.
///
/// The tokenizer's job in the shipped crate; done by hand here so that the collision is proved
/// against the character-data rules alone, with nothing else in the way. Fallible rather than
/// panicking, so a fixture that stops carrying the element fails an assertion in the test that
/// names it instead of inside a helper.
fn calendar_data_span<'a>(body: &'a [u8], open: &[u8], close: &[u8]) -> Option<&'a [u8]> {
    let start = find(body, open)?.checked_add(open.len())?;
    let rest = body.get(start..)?;
    let end = find(rest, close)?;
    rest.get(..end)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn verbatim(span: &[u8]) -> Result<DecodedText<'_>, DavError> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink = IgnoreDiagnostics;
    decode_text(span, TextMode::Verbatim, 0, &mut meter, &mut sink)
}

/// The payload with every carriage return removed, which is what a conformant read leaves.
fn without_carriage_returns(octets: &[u8]) -> Vec<u8> {
    octets
        .iter()
        .copied()
        .filter(|byte| *byte != b'\r')
        .collect()
}

#[test]
fn the_fixtures_still_carry_the_carriage_returns_the_case_is_about() {
    // A checkout that rewrote line endings would make every assertion below vacuous, so the
    // fixtures are asserted to be what `.gitattributes` promises before they are read.
    assert!(PAYLOAD.windows(2).any(|pair| pair == b"\r\n"));
    assert!(SABREDAV.windows(2).any(|pair| pair == b"\r\n"));
    assert!(RADICALE.windows(2).any(|pair| pair == b"\r\n"));
    assert!(!CALENDAR_SERVER.contains(&b'\r'));
}

#[test]
fn sabredav_octets_reach_ical_core_as_the_server_wrote_them() {
    let span =
        calendar_data_span(SABREDAV, b"<cal:calendar-data>", b"</cal:calendar-data>").unwrap();
    let decoded = verbatim(span).unwrap();
    assert_eq!(decoded.run.as_bytes(), PAYLOAD);
    assert_eq!(decoded.line_endings, LineEndings::Crlf);
    assert!(decoded.line_endings.is_as_sent());
    // Borrowed out of the body: no allocation, and the fold is still `CRLF SPACE`.
    assert!(!decoded.run.is_reassembled());
    assert!(decoded.run.as_bytes().windows(3).any(|at| at == b"\r\n "));
}

#[test]
fn radicale_octets_reach_ical_core_as_the_server_wrote_them() {
    let span =
        calendar_data_span(RADICALE, b"<ns1:calendar-data>", b"</ns1:calendar-data>").unwrap();
    let decoded = verbatim(span).unwrap();
    assert_eq!(decoded.run.as_bytes(), PAYLOAD);
    assert_eq!(decoded.line_endings, LineEndings::Crlf);
}

#[test]
fn a_conformant_writer_and_a_literal_one_converge_on_the_same_octets() {
    let escaped =
        calendar_data_span(CALENDAR_SERVER, b"<C:calendar-data>", b"</C:calendar-data>").unwrap();
    let decoded = verbatim(escaped).unwrap();
    // `&#13;` is markup rather than a line break, so section 2.11 never reaches it and it is
    // resolved in either mode. That is what makes the carve-out one rule and not two dialects.
    assert_eq!(decoded.run.as_bytes(), PAYLOAD);
    assert!(decoded.run.is_reassembled());
    assert_eq!(decoded.line_endings, LineEndings::Crlf);
}

#[test]
fn the_conformant_read_is_available_lossy_and_never_silent() {
    let span =
        calendar_data_span(SABREDAV, b"<cal:calendar-data>", b"</cal:calendar-data>").unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported = Vec::new();
    let decoded = decode_text(
        span,
        TextMode::NormalizedPayload,
        0,
        &mut meter,
        &mut reported,
    )
    .unwrap();

    assert_ne!(decoded.run.as_bytes(), PAYLOAD);
    assert!(!decoded.run.as_bytes().contains(&b'\r'));
    assert_eq!(decoded.line_endings, LineEndings::Folded);
    assert!(!decoded.line_endings.is_as_sent());
    assert_eq!(
        reported
            .first()
            .copied()
            .map(icalkit_conformance::internal::core::Diagnostic::code),
        Some(icalkit_conformance::internal::core::DiagnosticCode::DavCalendarDataLineEndingsFolded)
    );
    // What was lost is exactly the carriage returns and nothing else, which is why RFC 4791
    // section 9.6 can call the omission legal and this crate can still refuse to hide it.
    assert_eq!(
        decoded.run.as_bytes(),
        without_carriage_returns(PAYLOAD).as_slice()
    );
}
