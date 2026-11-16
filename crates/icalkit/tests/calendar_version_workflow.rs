// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar format versions through public scheduling and editing workflows.

#![cfg(test)]

use icalkit::Calendar;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

fn calendar(version: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n{version}\r\nPRODID:-//icalkit version workflows//EN\r\n\
         BEGIN:VEVENT\r\nUID:version@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nSUMMARY:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn inbound_scheduling_refuses_unsupported_calendar_formats() {
    for version in [
        "VERSION:banana",
        "VERSION:1.0",
        "VERSION:2.0;3.0",
        "VERSION;VALUE=URI:2.0",
    ] {
        let source = calendar(&format!("{version}\r\nMETHOD:REQUEST"));
        assert!(Message::read(source.as_bytes()).is_err(), "{version}");
    }
}

#[test]
fn outbound_scheduling_refuses_unsupported_calendar_formats() {
    for version in [
        "VERSION:banana",
        "VERSION:1.0",
        "VERSION:2.0;3.0",
        "VERSION;VALUE=URI:2.0",
    ] {
        assert!(
            Message::request(
                calendar(version).as_bytes(),
                Timestamp::constant(1_704_067_200, 0)
            )
            .is_err(),
            "{version}"
        );
    }
}

#[test]
fn supported_version_ranges_survive_text_edits_and_scheduling_reads() {
    for version in [
        "VERSION:2.0",
        "VERSION:2.0;2.0",
        "VeRsIoN;VALUE=TEXT;X-SOURCE=example:2.0;\r\n 2.0",
    ] {
        let source = calendar(version);
        let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
        let mut edit = calendar.edit();
        edit.set_summary("version@example.test", "Revised").unwrap();
        edit.commit().unwrap();
        assert_eq!(
            calendar.to_bytes(),
            source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
        );
        let request = source.replace("BEGIN:VEVENT\r\n", "METHOD:REQUEST\r\nBEGIN:VEVENT\r\n");
        let message = Message::read(request.as_bytes()).unwrap();
        assert_eq!(message.to_bytes(), request.as_bytes());
    }
}
