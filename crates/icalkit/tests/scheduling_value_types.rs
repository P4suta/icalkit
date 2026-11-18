// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scheduling preserves opaque values under [RFC 5545 section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).
//! Identifier comparison uses [TEXT values](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11).

#![cfg(test)]

use icalkit::interop::Import;
use icalkit::scheduling::{Actor, Message};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

const SOURCE: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit scheduling values//EN\r\n",
    "METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:series@example.test\r\n",
    "DTSTAMP:20260813T120000Z\r\nDTSTART:20260814T090000Z\r\n",
    "SUMMARY:Planning\r\nORGANIZER:mailto:chair@example.test\r\n",
    "ATTENDEE:mailto:one@example.test\r\nSEQUENCE:1\r\n",
    "END:VEVENT\r\nEND:VCALENDAR\r\n",
);

fn held() -> String {
    SOURCE.replace("METHOD:REQUEST\r\n", "")
}

fn held_kind(kind: &str, uid_line: &str) -> String {
    let end = if kind == "VFREEBUSY" {
        "DTEND:20260814T100000Z\r\n"
    } else {
        ""
    };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit held types//EN\r\n\
         BEGIN:{kind}\r\n{uid_line}\r\nDTSTAMP:20260813T120000Z\r\n\
         DTSTART:20260814T090000Z\r\n{end}ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:one@example.test\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn retained(source: &str) -> Calendar {
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    let import = Import::read(source.as_bytes()).unwrap();
    assert_eq!(import.as_bytes(), source.as_bytes());
    assert_eq!(import.validate().unwrap().to_bytes(), source.as_bytes());
    calendar
}

fn batch(first_uid: &str, second_uid: &str) -> String {
    SOURCE
        .replace("UID:series@example.test", &format!("UID:{first_uid}"))
        .replace(
            "SUMMARY:Planning",
            "RRULE:FREQ=DAILY;COUNT=2\r\nSUMMARY:Planning",
        )
        .replace(
            "END:VCALENDAR\r\n",
            &format!(
                "BEGIN:VEVENT\r\nUID:{second_uid}\r\nDTSTAMP:20260813T120000Z\r\n\
             RECURRENCE-ID:20260815T090000Z\r\nDTSTART:20260815T100000Z\r\nSUMMARY:Moved\r\n\
             ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:one@example.test\r\n\
             SEQUENCE:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            ),
        )
}

#[test]
fn opaque_identity_fields_cannot_build_an_inbound_message() {
    for name in ["METHOD", "UID", "ORGANIZER", "ATTENDEE"] {
        for value_type in ["X-OPAQUE", "FUTURE-TYPE"] {
            let source =
                SOURCE.replace(&format!("{name}:"), &format!("{name};VALUE={value_type}:"));
            retained(&source);
            let Err(error) = Message::read(source.as_bytes()) else {
                panic!("accepted opaque {name} with {value_type}");
            };
            assert_eq!(error.code().as_str(), "icalkit.scheduling.message-invalid");
        }
    }
}

#[test]
fn outbound_builders_refuse_an_opaque_identifier() {
    let source = held().replace("UID:", "UID;VALUE=X-OPAQUE:");
    retained(&source);
    assert!(Message::request(source.as_bytes(), Timestamp::constant(0, 0)).is_err());
}

#[test]
fn opaque_held_identifiers_cannot_imply_an_absent_destination() {
    let request = SOURCE.replace("SEQUENCE:1", "SEQUENCE:2");
    let message = Message::read(request.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    for value in ["series@example.test", "other@example.test", "unparsed\\q"] {
        let source = held().replace(
            "UID:series@example.test",
            &format!("UID;VALUE=X-OPAQUE:{value}"),
        );
        let current = retained(&source);
        let Err(error) = message.review(&current, &actor) else {
            panic!("accepted unreadable held identity {value}");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.scheduling.current-identity-unreadable"
        );
        assert_eq!(current.to_bytes(), source.as_bytes());
        assert_eq!(message.to_bytes(), request.as_bytes());
    }
}

#[test]
fn an_opaque_component_cannot_hide_a_possible_duplicate_identity() {
    let source = held().replace(
        "END:VCALENDAR\r\n",
        concat!(
            "BEGIN:VEVENT\r\nUID;VALUE=X-OPAQUE:other@example.test\r\n",
            "DTSTAMP:20260813T120000Z\r\nDTSTART:20260816T090000Z\r\n",
            "ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:one@example.test\r\n",
            "SEQUENCE:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ),
    );
    let current = retained(&source);
    let message = Message::read(SOURCE.replace("SEQUENCE:1", "SEQUENCE:2").as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let Err(error) = message.review(&current, &actor) else {
        panic!("accepted a possibly duplicated opaque identity");
    };
    assert_eq!(
        error.code().as_str(),
        "icalkit.scheduling.current-identity-unreadable"
    );
    assert_eq!(current.to_bytes(), source.as_bytes());
}

#[test]
fn opaque_held_principals_cannot_authorize_changes() {
    let source = held().replace("ORGANIZER:", "ORGANIZER;VALUE=X-OPAQUE:");
    let current = retained(&source);
    let message = Message::read(SOURCE.replace("SEQUENCE:1", "SEQUENCE:2").as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    assert!(message.review(&current, &actor).is_err());
    assert_eq!(current.to_bytes(), source.as_bytes());

    let source = held().replace("ATTENDEE:", "ATTENDEE;VALUE=X-OPAQUE:");
    let current = retained(&source);
    let reply = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit reply values//EN\r\n",
        "METHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:series@example.test\r\n",
        "DTSTAMP:20260813T130000Z\r\nSEQUENCE:1\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=ACCEPTED:mailto:one@example.test\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let message = Message::read(reply.as_bytes()).unwrap();
    let actor = Actor::new("mailto:one@example.test").unwrap();
    assert!(message.review(&current, &actor).is_err());
    assert_eq!(current.to_bytes(), source.as_bytes());
}

#[test]
fn equivalent_text_escapes_select_the_existing_component() {
    let source = held().replace("UID:series@example.test", "UID:series\\nkey@example.test");
    let current = retained(&source);
    let request = SOURCE
        .replace("UID:series@example.test", "UID:series\\Nkey@example.test")
        .replace("SEQUENCE:1", "SEQUENCE:2")
        .replace("SUMMARY:Planning", "SUMMARY:Updated");
    let engine = Engine::default();
    let mut session = engine.session();
    let message = Message::read_in(&mut session, request.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review_in(&mut session, &current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert_eq!(
        updated.events().count(),
        1,
        "an escaped identifier cannot create another object"
    );
    assert!(
        String::from_utf8(updated.to_bytes())
            .unwrap()
            .contains("SUMMARY:Updated")
    );
    assert_eq!(current.to_bytes(), source.as_bytes());
    assert_eq!(message.to_bytes(), request.as_bytes());
}

#[test]
fn batch_identifiers_compare_decoded_text_once() {
    let source = batch("series\\nkey@example.test", "series\\Nkey@example.test");
    retained(&source);
    let message = Message::read(source.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let created = message
        .review_creation(&actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert_eq!(created.events().count(), 2);
    assert_eq!(message.to_bytes(), source.as_bytes());

    let source = batch("series\\\\nkey@example.test", "series\\nkey@example.test");
    retained(&source);
    assert!(Message::read(source.as_bytes()).is_err());
}

#[test]
fn known_text_declarations_keep_the_source_spelling() {
    let source = SOURCE
        .replace("METHOD:REQUEST", "mEtHoD;VALUE=TEXT:request")
        .replace("UID:", "UID;VALUE=TEXT:")
        .replace("ORGANIZER:", "ORGANIZER;VALUE=CAL-ADDRESS:")
        .replace("ATTENDEE:", "ATTENDEE;VALUE=CAL-ADDRESS:");
    let message = Message::read(source.as_bytes()).unwrap();
    assert_eq!(message.method(), "REQUEST");
    assert_eq!(message.to_bytes(), source.as_bytes());
}

#[test]
fn every_held_scheduling_kind_requires_a_readable_identifier() {
    let message = Message::read(SOURCE.replace("SEQUENCE:1", "SEQUENCE:2").as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        let source = held_kind(kind, "UID;VALUE=X-OPAQUE:unrelated");
        let current = retained(&source);
        let Err(error) = message.review(&current, &actor) else {
            panic!("accepted an unreadable {kind} identity");
        };
        assert_eq!(
            error.code().as_str(),
            "icalkit.scheduling.current-identity-unreadable"
        );
        assert_eq!(current.to_bytes(), source.as_bytes());
    }
}

#[test]
fn one_identifier_cannot_create_a_different_component_kind() {
    let message = Message::read(SOURCE.replace("SEQUENCE:1", "SEQUENCE:2").as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    for kind in ["VTODO", "VJOURNAL"] {
        let source = held().replace("VEVENT", kind);
        let current = retained(&source);
        assert!(message.review(&current, &actor).is_err(), "{kind}");
        assert_eq!(current.to_bytes(), source.as_bytes());
    }
}

#[test]
fn known_held_identifiers_of_every_kind_still_allow_distinct_objects() {
    let message = Message::read(SOURCE.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        let source = held_kind(kind, "UID:other@example.test");
        let current = retained(&source);
        let updated = message
            .review(&current, &actor)
            .unwrap()
            .authorize()
            .apply()
            .unwrap();
        let written = String::from_utf8(updated.to_bytes()).unwrap();

        assert!(written.contains("BEGIN:VEVENT\r\nUID:series@example.test\r\n"));
        assert!(written.contains(&format!("BEGIN:{kind}\r\nUID:other@example.test\r\n")));
        assert_eq!(current.to_bytes(), source.as_bytes());
    }
}
