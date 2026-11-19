// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence scope through public occurrence and scheduling workflows.

#![cfg(test)]

use icalkit::recurrence::Window;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn calendar(range: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit ranges//EN\r\n\
         BEGIN:VEVENT\r\nUID:range@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T100000Z\r\nSUMMARY:Revised\r\n\
         RECURRENCE-ID;RANGE={range}:20240102T090000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn assert_invalid_scope(error: &icalkit::Error) {
    assert!(
        error.issues().iter().any(|issue| {
            issue.code().as_str() == "icalkit.validation.invalid-recurrence-range"
        })
    );
}

#[test]
fn inbound_scheduling_refuses_invalid_recurrence_scope() {
    let request =
        |range| calendar(range).replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
    for range in ["THISANDPRIOR", "UNKNOWN", "\"THISANDFUTURE\""] {
        assert_invalid_scope(&Message::read(request(range).as_bytes()).unwrap_err());
    }
    Message::read(request("THISANDFUTURE").as_bytes()).unwrap();
}

#[test]
fn outbound_scheduling_refuses_invalid_recurrence_scope() {
    let sent = Timestamp::constant(1_704_067_200, 0);
    for range in ["THISANDPRIOR", "UNKNOWN", "\"THISANDFUTURE\""] {
        assert_invalid_scope(&Message::request(calendar(range).as_bytes(), sent).unwrap_err());
    }
    Message::request(calendar("THISANDFUTURE").as_bytes(), sent).unwrap();
}

#[test]
fn a_valid_range_keeps_its_spelling_and_moves_later_occurrences() {
    let source = calendar("ThisAndFuture").replace(
        "BEGIN:VEVENT\r\n",
        concat!(
            "BEGIN:VEVENT\r\nUID:range@example.test\r\nDTSTAMP:20240101T000000Z\r\n",
            "DTSTART:20240101T090000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
            "SUMMARY:Original\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\n",
        ),
    );
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    let window = Window::new(
        "20240101T000000Z".parse().unwrap(),
        "20240104T000000Z".parse().unwrap(),
    )
    .unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = calendar
        .occurrences(&mut session, "range@example.test", window)
        .unwrap();
    let mut starts = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        starts.push(occurrence.start());
    }
    let expected: Vec<Timestamp> = ["20240101T090000Z", "20240102T100000Z", "20240103T100000Z"]
        .into_iter()
        .map(|text| text.parse().unwrap())
        .collect();
    assert_eq!(starts, expected);
}
