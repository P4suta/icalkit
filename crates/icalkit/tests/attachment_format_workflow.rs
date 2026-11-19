// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Attachment metadata through public editing and scheduling workflows.

#![cfg(test)]

use icalkit::Calendar;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

fn calendar(media_type: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit formats//EN\r\n\
         BEGIN:VEVENT\r\nUID:format@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nSUMMARY:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         ATTACH;FMTTYPE={media_type}:urn:attachment\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn inbound_scheduling_refuses_malformed_attachment_formats() {
    for media_type in ["text", "text/plain/extra", "text/*"] {
        let source =
            calendar(media_type).replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
        assert!(Message::read(source.as_bytes()).is_err());
    }
}

#[test]
fn outbound_scheduling_refuses_malformed_attachment_formats() {
    for media_type in ["text", "text/plain/extra", "text/*"] {
        assert!(
            Message::request(
                calendar(media_type).as_bytes(),
                Timestamp::constant(1_704_067_200, 0)
            )
            .is_err()
        );
    }
}

#[test]
fn text_edits_keep_attachment_format_spelling() {
    let source = calendar("Application/X-Private+Json");
    let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
    let mut edit = calendar.edit();
    edit.set_summary("format@example.test", "Revised").unwrap();
    edit.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
    );
}
