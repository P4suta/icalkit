// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Trigger and encoding controls through public editing and scheduling workflows.

#![cfg(test)]

use icalkit::Calendar;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

const CONTROLS: &str = concat!(
    "ATTACH;VALUE=\"BINARY\";ENCODING=Base64:QQ==\r\n",
    "ATTACH;VALUE=URI;ENCODING=8bIt:urn:attachment\r\n",
    "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n",
    "TRIGGER;RELATED\r\n =End:-PT15M\r\nEND:VALARM\r\n",
);

fn calendar(properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit controls//EN\r\n\
         BEGIN:VEVENT\r\nUID:control@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nDURATION:PT1H\r\nSUMMARY:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         {properties}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn malformed_controls() -> [String; 4] {
    [
        "ATTACH;VALUE=BINARY;ENCODING=\"BASE64\":QQ==\r\n".to_owned(),
        "ATTACH;ENCODING=\"8BIT\":urn:attachment\r\n".to_owned(),
        CONTROLS.replace("RELATED\r\n =End", "RELATED=\"START\""),
        CONTROLS.replace("RELATED\r\n =End", "RELATED=\"END\""),
    ]
}

#[test]
fn inbound_scheduling_refuses_quoted_native_controls() {
    for properties in malformed_controls() {
        let source =
            calendar(&properties).replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
        assert!(Message::read(source.as_bytes()).is_err(), "{properties:?}");
    }
}

#[test]
fn outbound_scheduling_refuses_quoted_native_controls() {
    for properties in malformed_controls() {
        assert!(
            Message::request(
                calendar(&properties).as_bytes(),
                Timestamp::constant(1_704_067_200, 0)
            )
            .is_err(),
            "{properties:?}"
        );
    }
}

#[test]
fn text_edits_keep_native_control_spelling_and_folding() {
    let source = calendar(CONTROLS);
    let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
    let mut edit = calendar.edit();
    edit.set_summary("control@example.test", "Revised").unwrap();
    edit.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
    );
}

#[test]
fn scheduling_keeps_native_control_spelling_and_folding() {
    let source = calendar(CONTROLS);
    let request = source.replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
    assert_eq!(
        Message::read(request.as_bytes()).unwrap().to_bytes(),
        request.as_bytes()
    );
    let message =
        Message::request(source.as_bytes(), Timestamp::constant(1_704_067_200, 0)).unwrap();
    let bytes = message.to_bytes();
    assert!(
        bytes
            .windows(CONTROLS.len())
            .any(|part| part == CONTROLS.as_bytes())
    );
    assert!(Message::read(&bytes).is_ok());
}
