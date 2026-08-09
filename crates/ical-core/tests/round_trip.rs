// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The round-trip claim of `docs/adr/0001`, asserted across the whole stack at once.
//!
//! Every unit below this file tested itself against a stand-in for its neighbor: the tree
//! builder against a scripted token source, the serializer against a tree written by hand, the
//! reader against a test-local re-emitter. Each of those is a claim about one unit and a
//! guess about the seam. This file is the only place the real reader, the real builder and
//! the real serializer meet, so it is the only place `parse -> serialize` is byte-identity
//! rather than agreement between two authors.
//!
//! The inputs are the ones the corpus exists for — a fold inside a name, a bare `CR` a
//! producer emitted for a terminator, a line with no `:`, an `END` whose case disagrees, a
//! `SUMMARY` that is not UTF-8 — and each of them is an input this crate must not repair.
//! Whether the octets are *right* is `ical-conform`'s question; whether they are *unchanged*
//! is this one.

use ical_core::{Diagnostic, Document, Limits, ParseError};

/// Parse `input` and write it back, alongside whatever was diagnosed on the way.
///
/// The refusal is carried rather than unwrapped, so an input that crosses a bound fails an
/// assertion naming the bound instead of panicking inside a helper.
fn round_trip(input: &[u8]) -> (Result<Vec<u8>, ParseError>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let written = Document::parse(input, Limits::DEFAULT, &mut diagnostics)
        .map(|document| document.to_bytes());
    (written, diagnostics)
}

/// Assert that `input` survives a parse and a serialize with no octet changed.
#[track_caller]
fn assert_identical(input: &[u8]) {
    let (written, _) = round_trip(input);
    assert_eq!(
        written.as_deref(),
        Ok(input),
        "\n  read {:?}",
        String::from_utf8_lossy(input),
    );
}

#[test]
fn an_ordinary_calendar_comes_back_octet_for_octet() {
    assert_identical(
        b"BEGIN:VCALENDAR\r\n\
          VERSION:2.0\r\n\
          PRODID:-//Example Corp//Example Client 1.0//EN\r\n\
          BEGIN:VEVENT\r\n\
          UID:19960401T080045Z-4000F192713-0052@example.test\r\n\
          DTSTAMP:19960401T080045Z\r\n\
          DTSTART;TZID=America/New_York:19960401T093000\r\n\
          SUMMARY:Lunch\r\n\
          END:VEVENT\r\n\
          END:VCALENDAR\r\n",
    );
}

#[test]
fn every_place_a_producer_may_fold_is_put_back_where_it_was() {
    // A fold inside the name, inside a parameter value, inside the value, and one sitting at
    // exactly the end of the line. The last is the one that is easiest to drop, because the
    // octets it separates are empty and a serializer that writes folds as it passes them can
    // walk off the end before it gets there.
    assert_identical(b"SUM\r\n MARY:Lunch\r\n");
    assert_identical(b"SUMMARY;LANGUAGE=en\r\n -US:Lunch\r\n");
    assert_identical(b"SUMMARY:Lunch with the\r\n  team\r\n");
    assert_identical(b"X:v\r\n \r\n");
    assert_identical(b"SUMMARY:\r\n\tLunch\r\n");
}

#[test]
fn a_terminator_a_producer_chose_is_never_rewritten() {
    // RFC 5545 section 3.1 spells a terminator `CRLF` only. All three arrive in the wild, and
    // normalizing one is a diff against a file nobody asked to change.
    assert_identical(b"A:1\nB:2\n");
    assert_identical(b"A:1\rB:2\r");
    assert_identical(b"A:1\r\nB:2\nC:3\rD:4\r\n");
    assert_identical(b"A:1\r\n C\n D\r E\r\n");
}

#[test]
fn a_final_line_with_no_terminator_does_not_acquire_one() {
    assert_identical(b"BEGIN:VCALENDAR\r\nEND:VCALENDAR");
    assert_identical(b"SUMMARY:Lunch");
    assert_identical(b"X");
}

#[test]
fn an_empty_input_is_an_empty_document_and_writes_nothing_back() {
    let (written, diagnostics) = round_trip(b"");
    assert_eq!(written.as_deref(), Ok(&b""[..]));
    assert!(diagnostics.is_empty());
}

#[test]
fn every_structural_anomaly_survives_as_the_octets_it_arrived_with() {
    // Each of these is diagnosed rather than repaired, and the diagnosis costs no octet. A
    // line with no `:`, an `END` with no `BEGIN`, an `END` whose name disagrees, an `END`
    // whose case disagrees, a blank line, and a `BEGIN` whose `END` never arrives.
    let input = b"BEGIN:VCALENDAR\r\n\
                  THIS-LINE-HAS-NO-COLON\r\n\
                  END:VTODO\r\n\
                  \r\n\
                  BEGIN:VEVENT\r\n\
                  SUMMARY:Lunch\r\n\
                  end:vevent\r\n\
                  BEGIN:VALARM\r\n\
                  END:VCALENDAR\r\n";
    let (written, diagnostics) = round_trip(input);
    assert_eq!(written.as_deref(), Ok(&input[..]));
    assert!(
        !diagnostics.is_empty(),
        "anomalies are preserved and still reported"
    );
}

#[test]
fn octets_that_are_not_utf8_are_carried_rather_than_replaced() {
    // A CP1252 `SUMMARY` is in the corpus, and the layer that must never reject a calendar is
    // not the layer that demands UTF-8. No decode happens on this path at all.
    assert_identical(b"BEGIN:VEVENT\r\nSUMMARY:\xe9t\xe9 \x92\r\nEND:VEVENT\r\n");
    // A fold that splits a multi-byte codepoint is legal per section 3.1 and is exactly the
    // thing a `String`-shaped storage cannot hold.
    assert_identical(b"SUMMARY:\xc3\r\n \xa9t\xc3\xa9\r\n");
}

#[test]
fn a_parameter_keeps_its_quotes_its_case_and_its_order() {
    // Nothing here is interpreted, so nothing here may be normalized: not the quoting, not
    // the case of a name, not the order two parameters were written in, and not a `:` that
    // arrived inside a quoted value.
    assert_identical(
        b"ATTENDEE;delegated-to=\"mailto:a@example.test\",\"mailto:b@example.test\"\
          ;CN=Ann:mailto:c@example.test\r\n",
    );
    assert_identical(b"X-PROP;B=2;A=1;B=3:v\r\n");
    assert_identical(b"X-PROP;NOVALUE;EMPTY=:v\r\n");
    assert_identical(b"X-PROP;CN=\"unterminated:still text\r\n");
}

#[test]
fn a_value_that_is_empty_is_a_value_and_not_an_absence() {
    assert_identical(b"SUMMARY:\r\n");
    assert_identical(b"SUMMARY;LANGUAGE=en:\r\n");
    assert_identical(b":\r\n");
}

#[test]
fn text_escaping_is_storage_and_is_never_resolved_on_the_way_through() {
    // `\n`, `\N`, `\,`, `\;`, `\\` and an escape section 3.3.11 gives no meaning to. Decoding
    // is a view a caller asks for; the octets written back are the octets read.
    assert_identical(b"DESCRIPTION:one\\ntwo\\Nthree\\, four\\; five\\\\ six \\q seven\\\r\n");
}

#[test]
fn deep_nesting_within_the_bound_keeps_every_boundary_in_place() {
    let input = b"BEGIN:VCALENDAR\r\n\
                  BEGIN:VTIMEZONE\r\n\
                  TZID:America/New_York\r\n\
                  BEGIN:DAYLIGHT\r\n\
                  TZOFFSETFROM:-0500\r\n\
                  TZOFFSETTO:-0400\r\n\
                  END:DAYLIGHT\r\n\
                  BEGIN:STANDARD\r\n\
                  TZOFFSETFROM:-0400\r\n\
                  TZOFFSETTO:-0500\r\n\
                  END:STANDARD\r\n\
                  END:VTIMEZONE\r\n\
                  END:VCALENDAR\r\n";
    assert_identical(input);
}
