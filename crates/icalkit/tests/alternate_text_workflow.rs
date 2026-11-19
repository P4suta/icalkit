// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Alternate text metadata through public editing and scheduling workflows.

#![cfg(test)]

use icalkit::Calendar;
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

fn calendar(uri: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit alternate text//EN\r\n\
         BEGIN:VEVENT\r\nUID:alternate@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nSUMMARY;ALTREP=\"{uri}\";X-SOURCE=kept:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         X-UNRELATED:kept\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn transactional_text_edits_keep_the_alternate_uri_and_unrelated_data() {
    let source = calendar("https://example.test/a,b?x=y;z");
    let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
    let mut edit = calendar.edit();
    edit.set_summary("alternate@example.test", "Revised")
        .unwrap();
    edit.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
    );
}

#[test]
fn inbound_scheduling_refuses_invalid_alternate_representations() {
    let source =
        calendar("relative").replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
    assert!(Message::read(source.as_bytes()).is_err());
}

#[test]
fn outbound_scheduling_refuses_invalid_alternate_representations() {
    let source = calendar("relative");
    assert!(Message::request(source.as_bytes(), Timestamp::constant(1_704_067_200, 0)).is_err());
}
