// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Create a stored calendar through the scheduling authorization workflow.

use icalkit::scheduling::{Actor, Message};
use icalkit::{Calendar, Engine};

const SOURCE: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit creation//EN\r\n",
    "METHOD:REQUEST\r\nX-CALENDAR:Keep\r\nBEGIN:VEVENT\r\n",
    "UID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n",
    "DTSTART:20260814T090000Z\r\nRRULE:FREQ=WEEKLY;COUNT=2\r\n",
    "SUMMARY:Planning\r\n",
    "ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:one@example.test\r\n",
    "SEQUENCE:1\r\nx-CoLoR;X-Meta=\"Keep\":plum\r\nEND:VEVENT\r\n",
    "BEGIN:X-ARCHIVE\r\nX-NOTE:Kept after the event\r\nEND:X-ARCHIVE\r\nEND:VCALENDAR\r\n",
);

#[test]
fn creating_the_first_object_requires_review_and_preserves_the_message() {
    let message = Message::read(SOURCE.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let review = message.review_creation(&actor).unwrap();
    assert!(review.change_count() > 0);
    let created = review.authorize().apply().unwrap();
    assert_eq!(
        created.to_bytes(),
        SOURCE.replace("METHOD:REQUEST\r\n", "").as_bytes()
    );
    assert_eq!(message.to_bytes(), SOURCE.as_bytes());
    assert_eq!(
        created.events().next().unwrap().uid().as_deref(),
        Some("one@example.test")
    );
}

#[test]
fn creation_can_share_an_engine_session() {
    let message = Message::read(SOURCE.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let created = message
        .review_creation_in(&mut session, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert_eq!(
        created.to_bytes(),
        SOURCE.replace("METHOD:REQUEST\r\n", "").as_bytes()
    );
}

#[test]
fn creation_authorizes_every_component_before_applying_any() {
    let source = SOURCE.replace(
        "END:VCALENDAR\r\n",
        concat!(
            "BEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n",
            "RECURRENCE-ID:20260821T090000Z\r\nDTSTART:20260821T110000Z\r\n",
            "SUMMARY:Moved\r\n",
            "ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:one@example.test\r\n",
            "SEQUENCE:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ),
    );
    let message = Message::read(source.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let created = message
        .review_creation(&actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert_eq!(created.events().count(), 2);
    assert_eq!(
        created.to_bytes(),
        source.replace("METHOD:REQUEST\r\n", "").as_bytes()
    );

    let stranger = Actor::new("mailto:stranger@example.test").unwrap();
    assert_eq!(
        message
            .review_creation(&stranger)
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.scheduling.authorization-denied",
    );
    assert_eq!(message.to_bytes(), source.as_bytes());
}

#[test]
fn cancellation_cannot_create_an_absent_destination() {
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit creation//EN\r\n",
        "METHOD:CANCEL\r\nBEGIN:VEVENT\r\nUID:one@example.test\r\n",
        "DTSTAMP:20260813T120000Z\r\nORGANIZER:mailto:chair@example.test\r\n",
        "SEQUENCE:1\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let message = Message::read(source.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    assert!(message.review_creation(&actor).is_err());
    assert_eq!(message.to_bytes(), source.as_bytes());
}

#[test]
fn an_existing_calendar_can_still_receive_a_different_identity() {
    let source = SOURCE
        .replace("METHOD:REQUEST\r\n", "")
        .replace("one@example.test", "held@example.test");
    let held = Calendar::parse(source.as_bytes()).unwrap();
    let message = Message::read(SOURCE.as_bytes()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review(&held, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert_eq!(updated.events().count(), 2);
    assert_eq!(held.to_bytes(), source.as_bytes());
}
