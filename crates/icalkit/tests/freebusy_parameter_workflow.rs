// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Free busy classification through public scheduling replies.

#![cfg(test)]

use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

fn calendar(classification: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit classifications//EN\r\n\
         BEGIN:VFREEBUSY\r\nUID:busy@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         FREEBUSY;FBTYPE={classification}:20240102T090000Z/PT1H\r\n\
         END:VFREEBUSY\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn inbound_replies_refuse_malformed_classification_tokens() {
    for value in ["FREE,BUSY", "BUSY_PRIVATE", "\"FREE\""] {
        let source = calendar(value).replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REPLY\r\n");
        assert!(Message::read(source.as_bytes()).is_err());
    }
}

#[test]
fn outbound_replies_refuse_malformed_classification_tokens() {
    for value in ["FREE,BUSY", "BUSY_PRIVATE", "\"FREE\""] {
        assert!(
            Message::reply(
                calendar(value).as_bytes(),
                Timestamp::constant(1_704_067_200, 0)
            )
            .is_err()
        );
    }
}

#[test]
fn unknown_valid_tokens_remain_readable_without_rewriting_the_reply() {
    let source =
        calendar("X-PRIVATE").replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REPLY\r\n");
    assert_eq!(
        Message::read(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}
