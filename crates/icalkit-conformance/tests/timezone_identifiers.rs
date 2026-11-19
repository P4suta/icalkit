// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Definition identifiers follow [RFC 5545 section 3.8.3.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.3.1).
//! Text escapes follow [section 3.3.11](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11).

#![cfg(test)]

use icalkit_conformance::internal::core::{
    ContentLineReader, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics, Limits, Meter,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

fn definition(identifier: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\n{identifier}\r\nBEGIN:STANDARD\r\n\
         DTSTART:20000101T000000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0200\r\n\
         END:STANDARD\r\nEND:VTIMEZONE\r\n"
    )
}

#[test]
fn escaped_reference_storage_is_charged_before_retention() {
    let limits = Limits::DEFAULT;
    let read = |source: &[u8], budget| {
        let mut meter = Meter::with_budget(limits, budget);
        let mut lines = ContentLineReader::new(source, limits.grammar());
        let result = Document::from_tokens(&mut lines, &mut meter, &mut IgnoreDiagnostics);
        (result, meter.spent())
    };
    let plain = b"DTSTART;TZID=Test/AA:20240102T090000\r\n";
    let encoded = b"DTSTART;TZID=Test/^^:20240102T090000\r\n";
    let (plain_result, budget) = read(plain, u64::MAX);
    plain_result.unwrap();
    assert!(read(encoded, budget).0.is_err());
    let (decoded, cost) = read(encoded, u64::MAX);
    assert!(cost > budget);
    assert_eq!(decoded.unwrap().to_bytes(), encoded);
}

fn reference(parameter: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:reference@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART;TZID={parameter}:20240102T090000\r\nEND:VEVENT\r\n"
    )
}

fn read(body: &str, limits: Limits) -> (VtimezoneSet, Vec<Diagnostic>, Meter) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit zone identity//EN\r\n\
         {body}END:VCALENDAR\r\n"
    );
    let document = Document::parse(text.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    assert_eq!(document.to_bytes(), text.as_bytes());
    (zones, diagnostics, meter)
}

#[test]
fn definition_names_decode_text_exactly_once() {
    for (written, decoded) in [
        (r"Test/Comma\,Zone", "Test/Comma,Zone"),
        (r"Test/Semi\;Zone", "Test/Semi;Zone"),
        (r"Test/Back\\Slash", r"Test/Back\Slash"),
        (r"Test/Line\nBreak", "Test/Line\nBreak"),
        (r"Test/Line\NBreak", "Test/Line\nBreak"),
        (r"Test/Double\\n", r"Test/Double\n"),
        ("Test/^n^^^'", "Test/^n^^^'"),
        ("Test/\u{65e5}\u{672c}", "Test/\u{65e5}\u{672c}"),
    ] {
        let (zones, diagnostics, meter) =
            read(&definition(&format!("TZID:{written}")), Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{written}: {diagnostics:?}");
        let table = zones.table(decoded).expect(decoded);
        assert_eq!(table.tzid().as_str(), decoded);
        assert_eq!(table.observances().len(), 1);
        assert_eq!(meter.vtimezone_components(), 1);
        if written != decoded {
            assert!(zones.table(written).is_none(), "raw alias: {written}");
        }
    }
}

#[test]
fn literal_property_quotes_do_not_alias_unquoted_names() {
    let body = ["TZID:Test/Zone", "TZID:\"Test/Zone\"", "TZID:\"\""]
        .map(definition)
        .join("");
    let (zones, diagnostics, meter) = read(&body, Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(zones.len(), 3);
    assert_eq!(meter.vtimezone_components(), 3);
    for name in ["Test/Zone", "\"Test/Zone\"", "\"\""] {
        assert_eq!(zones.definitions(name).len(), 1, "{name}");
    }
}

#[test]
fn references_quote_parameters_instead_of_escaping_property_text() {
    for (written, parameter, decoded) in [
        (
            r"Test/Comma\,Zone",
            "\"Test/Comma,Zone\"",
            "Test/Comma,Zone",
        ),
        (r"Test/Semi\;Zone", "\"Test/Semi;Zone\"", "Test/Semi;Zone"),
        (r"Test/Back\\Slash", r"Test/Back\Slash", r"Test/Back\Slash"),
        ("Test/Colon:Zone", "\"Test/Colon:Zone\"", "Test/Colon:Zone"),
        ("\"Test/Zone\"", "^'Test/Zone^'", "\"Test/Zone\""),
        (
            r"Test/Line\n^Zone",
            "\"Test/Line^n^^Zone\"",
            "Test/Line\n^Zone",
        ),
    ] {
        let body = format!(
            "{}{}",
            definition(&format!("TZID:{written}")),
            reference(parameter)
        );
        let (zones, diagnostics, _) = read(&body, Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{written}: {diagnostics:?}");
        assert!(zones.table(decoded).is_some(), "{decoded}");
        let changed = format!(
            "{}{}",
            definition(&format!("TZID:{written}")),
            reference("\"Test/Undefined,Zone\"")
        );
        let (_, diagnostics, _) = read(&changed, Limits::DEFAULT);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code(),
            DiagnosticCode::MissingTimeZoneDefinition
        );
        assert_eq!(
            diagnostics[0].subject().unwrap().as_bytes(),
            b"Test/Undefined,Zone"
        );
    }
}

#[test]
fn equivalent_text_escapes_share_duplicate_identity() {
    let body = [r"TZID:Test/Line\nBreak", r"TZID:Test/Line\NBreak"]
        .map(definition)
        .join("");
    let (zones, diagnostics, meter) = read(&body, Limits::DEFAULT);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones.definitions("Test/Line\nBreak").len(), 2);
    assert_eq!(meter.vtimezone_components(), 2);
    assert_eq!(
        diagnostics
            .iter()
            .copied()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        [DiagnosticCode::DuplicateTimeZoneIdentifier]
    );
    assert_eq!(
        diagnostics[0].subject().unwrap().as_bytes(),
        b"Test/Line\nBreak"
    );
}

#[test]
fn refused_decoded_definitions_do_not_appear_undefined() {
    let body = format!(
        "{}{}{}{}",
        definition("TZID:Test/Admitted"),
        definition(r"TZID:Test/Refused\,Zone"),
        reference("\"Test/Refused,Zone\""),
        reference("Test/Undefined")
    );
    let (zones, diagnostics, meter) = read(&body, Limits::DEFAULT.with_max_vtimezone_components(1));
    assert_eq!(zones.len(), 1);
    assert_eq!(meter.vtimezone_components(), 1);
    assert_eq!(
        diagnostics
            .iter()
            .copied()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        [
            DiagnosticCode::VtimezoneComponentsTruncated,
            DiagnosticCode::MissingTimeZoneDefinition
        ]
    );
    assert_eq!(
        diagnostics[0].subject().unwrap().as_bytes(),
        b"Test/Refused,Zone"
    );
    assert_eq!(
        diagnostics[1].subject().unwrap().as_bytes(),
        b"Test/Undefined"
    );
}

#[test]
fn unreadable_text_cannot_register_a_definition() {
    for identifier in [
        "TZID;VALUE=X-OPAQUE:Test/Zone",
        "TZID;VALUE=URI:https://example.test/zone",
        "TZID;VALUE=TEXT;VALUE=TEXT:Test/Zone",
        r"TZID:Test/Bad\q",
        "TZID:Test/Unescaped,Comma",
        "TZID:Test/Unescaped;Semi",
        "TZID:",
    ] {
        let (zones, _, meter) = read(&definition(identifier), Limits::DEFAULT);
        assert!(zones.is_empty(), "{identifier}: {zones:?}");
        assert_eq!(meter.vtimezone_components(), 0, "{identifier}");
        assert_eq!(meter.vtimezone_observances(), 0, "{identifier}");
    }
}
