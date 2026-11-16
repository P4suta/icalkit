// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Descriptive metadata through public editing and scheduling workflows.

#![cfg(test)]

use icalkit::Calendar;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

fn calendar(language: &str, common_name: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit metadata//EN\r\n\
         BEGIN:VEVENT\r\nUID:metadata@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nSUMMARY;LANGUAGE={language}:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE;CN={common_name}:\r\n \
         mailto:person@example.test\r\nX-UNRELATED:kept\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn transactional_text_edits_keep_descriptive_metadata() {
    let source = calendar("EN-us", "\"Doe, Jane^'s Team\"");
    let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
    let mut edit = calendar.edit();
    edit.set_summary("metadata@example.test", "Revised")
        .unwrap();
    edit.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
    );
}

#[test]
fn inbound_scheduling_refuses_invalid_descriptive_metadata() {
    for source in [calendar("en_US", "Alice"), calendar("en", "Alice,Bob")] {
        let request = source.replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
        assert!(Message::read(request.as_bytes()).is_err());
    }
}

#[test]
fn outbound_scheduling_refuses_invalid_descriptive_metadata() {
    for source in [calendar("en_US", "Alice"), calendar("en", "Alice,Bob")] {
        assert!(
            Message::request(source.as_bytes(), Timestamp::constant(1_704_067_200, 0)).is_err()
        );
    }
}
