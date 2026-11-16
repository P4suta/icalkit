// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scheduling creation with calendar user addresses from non-email URI schemes.

#![cfg(test)]

use icalkit::scheduling::{Actor, Message};

#[test]
fn a_non_email_actor_can_authorize_creation_for_its_exact_organizer_address() {
    for address in [
        "urn:example:chair",
        "https://calendar.example.test/principals/a,b",
        "x-cua:chair%2Bprimary",
    ] {
        let source = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n\
             METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:actor@example.test\r\n\
             DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nSUMMARY:Planning\r\n\
             ORGANIZER:{address}\r\nATTENDEE:mailto:person@example.test\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let message = Message::read(source.as_bytes()).unwrap();
        let actor = Actor::new(address).unwrap();
        let created = message
            .review_creation(&actor)
            .unwrap()
            .authorize()
            .apply()
            .unwrap();
        assert_eq!(
            created.to_bytes(),
            source.replace("METHOD:REQUEST\r\n", "").as_bytes()
        );
        assert_eq!(message.to_bytes(), source.as_bytes());
    }
}
