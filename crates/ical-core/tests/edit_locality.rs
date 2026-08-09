// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a scoped write costs, measured on a document the reader actually built.
//!
//! `docs/adr/0001` promises that editing one property rewrites one line. The mutation unit
//! could only assert that against a tree it wrote itself, where every layout was whatever the
//! test chose; the claim a caller relies on is about a layout a *producer* chose, which only
//! exists after a real parse. So this file is the parse-edit-serialize path, and the
//! assertions are about the octets that did **not** move.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, DateTimeValue, Document, Limits, MutationError,
    ParseError, PropertyId, TextValue, View,
};

/// A calendar carrying every kind of octet an edit must not disturb.
///
/// The vendor property, the fold, the bare `LF` and the quoted parameter are all things this
/// crate does not interpret, which is exactly why they are the ones at risk.
const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\n\
    VERSION:2.0\r\n\
    BEGIN:VEVENT\r\n\
    UID:9b1c@example.test\r\n\
    DTSTART;TZID=America/New_York:19960401T093000\r\n\
    SUMMARY:Lunch with the\r\n  team\r\n\
    X-MICROSOFT-CDO-BUSYSTATUS:FREE\n\
    DESCRIPTION;ALTREP=\"cid:part1.0001@example.test\":one\\ntwo\r\n\
    END:VEVENT\r\n\
    END:VCALENDAR\r\n";

/// Parse `CALENDAR` under the default policy, discarding what it diagnosed.
fn calendar() -> Result<Document, ParseError> {
    let mut diagnostics = Vec::new();
    Document::parse(CALENDAR, Limits::DEFAULT, &mut diagnostics)
}

/// The first `VEVENT` of the first calendar.
fn event(document: &mut Document) -> Option<&mut Component> {
    document
        .components_mut()
        .flat_map(Component::components_mut)
        .find(|component| component.is_named(b"VEVENT"))
}

#[test]
fn moving_one_start_rewrites_one_line_and_no_other() {
    let mut document = calendar().unwrap();
    let noon = CivilDateTime::new(
        CivilDate::from_ymd(1996, 4, 1).unwrap(),
        CivilTime::from_hms(12, 0, 0).unwrap(),
    );
    let mut handle = event(&mut document).unwrap().dtstart_mut().unwrap();
    handle.set(&DateTimeValue::Utc(noon)).unwrap();

    let written = document.to_bytes();
    let text = String::from_utf8_lossy(&written).into_owned();

    // The edit happened, and it took the parameters its own written form governs with it:
    // a UTC date-time carries neither `VALUE` nor `TZID`.
    assert!(text.contains("DTSTART:19960401T120000Z\r\n"), "{text}");
    assert!(!text.contains("TZID"), "{text}");

    // Nothing else moved. The vendor property kept its value and its bare `LF`, the folded
    // `SUMMARY` kept the fold where the producer put it, and the quoted `ALTREP` kept both
    // its quotes and the escape inside the value beside it.
    assert!(text.contains("X-MICROSOFT-CDO-BUSYSTATUS:FREE\n"), "{text}");
    assert!(
        text.contains("SUMMARY:Lunch with the\r\n  team\r\n"),
        "{text}"
    );
    assert!(
        text.contains("DESCRIPTION;ALTREP=\"cid:part1.0001@example.test\":one\\ntwo\r\n"),
        "{text}"
    );
}

#[test]
fn a_typed_read_is_a_view_and_leaves_the_text_it_read_alone() {
    let mut document = calendar().unwrap();
    let subject = event(&mut document).unwrap();

    match subject.dtstart() {
        View::Valid { value, source } => {
            assert_eq!(
                value,
                DateTimeValue::Local(CivilDateTime::new(
                    CivilDate::from_ymd(1996, 4, 1).unwrap(),
                    CivilTime::from_hms(9, 30, 0).unwrap(),
                ))
            );
            // The `TZID` is a parameter and not part of the value, which is what lets the
            // zone stay `ical-tz`'s question.
            assert_eq!(source.value_text().as_bytes(), b"19960401T093000");
        },
        View::Malformed { .. } | View::Absent => panic!("the fixture has a readable DTSTART"),
    }

    // An escape is storage. Decoding resolves it; the property still holds what was written.
    let summary = subject.summary().value().unwrap();
    assert_eq!(summary.decode().unwrap(), "Lunch with the team");
    let description = subject
        .get::<TextValue<'_>>(&PropertyId::from_name(b"DESCRIPTION"))
        .value()
        .unwrap();
    assert_eq!(description.decode().unwrap(), "one\ntwo");
    assert_eq!(description.as_bytes(), b"one\\ntwo");

    // And reading changed nothing at all.
    assert_eq!(document.to_bytes(), CALENDAR);
}

#[test]
fn a_write_that_would_inject_a_second_line_is_refused_and_changes_nothing() {
    let mut document = calendar().unwrap();
    let mut handle = event(&mut document)
        .unwrap()
        .get_mut::<TextValue<'_>>(&PropertyId::SUMMARY)
        .unwrap();
    assert_eq!(
        handle.set_raw(b"hi\r\nATTENDEE:mailto:eve@example.test"),
        Err(MutationError::IllegalControlCharacter)
    );
    // A refusal is not a partial write: the fold the producer chose is still recorded, which
    // it would not be if the layout had been discarded before the octets were checked.
    assert_eq!(document.to_bytes(), CALENDAR);
}
