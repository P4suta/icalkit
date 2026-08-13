// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iTIP read-review-authorize-apply and outbound builders through the facade.

use icalkit::Calendar;
use icalkit::interop::{Import, RfcRepairV1};
use icalkit::scheduling::{Actor, Message};
use icalkit::time::Timestamp;

const WRAP_HEAD: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n";
const WRAP_TAIL: &str = "END:VCALENDAR\r\n";

fn payload(lines: &str) -> Vec<u8> {
    format!("{WRAP_HEAD}BEGIN:VEVENT\r\n{lines}END:VEVENT\r\n{WRAP_TAIL}").into_bytes()
}

fn supplied_time() -> Timestamp {
    Timestamp::constant(0, 0)
}

fn repaired(bytes: &[u8]) -> Result<Vec<u8>, icalkit::Error> {
    let imported = Import::read(bytes)?;
    let normalized = imported.normalize(RfcRepairV1)?;
    Ok(normalized.output().as_bytes().to_vec())
}

#[test]
fn all_eight_outbound_methods_are_built_with_caller_supplied_time() {
    let publish = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:One\r\n\
         ORGANIZER:mailto:chair@example.test\r\n",
    );
    let request = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:One\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:1\r\n",
    );
    let reply = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\nSEQUENCE:1\r\n",
    );
    let add = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:One\r\n\
         ORGANIZER:mailto:chair@example.test\r\nSEQUENCE:2\r\n",
    );
    let cancel = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\nSEQUENCE:2\r\n",
    );
    let refresh = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\n",
    );
    let counter = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T150000Z\r\nSUMMARY:One\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:2\r\n",
    );
    let decline = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:2\r\n",
    );

    let messages = [
        Message::publish(&publish, supplied_time()).unwrap(),
        Message::request(&request, supplied_time()).unwrap(),
        Message::reply(&reply, supplied_time()).unwrap(),
        Message::add(&add, supplied_time()).unwrap(),
        Message::cancel(&cancel, supplied_time()).unwrap(),
        Message::refresh(&refresh, supplied_time()).unwrap(),
        Message::counter(&counter, supplied_time()).unwrap(),
        Message::decline_counter(&decline, supplied_time()).unwrap(),
    ];
    let methods = [
        "PUBLISH",
        "REQUEST",
        "REPLY",
        "ADD",
        "CANCEL",
        "REFRESH",
        "COUNTER",
        "DECLINECOUNTER",
    ];
    for (message, method) in messages.iter().zip(methods) {
        assert_eq!(message.method(), method);
        assert!(
            message
                .to_bytes()
                .windows(b"DTSTAMP:19700101T000000Z".len())
                .any(|window| window == b"DTSTAMP:19700101T000000Z")
        );
    }
}

#[test]
fn review_authorization_borrows_inputs_and_apply_consumes_it() {
    const HELD: &[u8] =
        include_bytes!("../../ical-conform/tests/fixtures/break_itip_methods/held_series.ics");
    const REQUEST: &[u8] = include_bytes!(
        "../../ical-conform/tests/fixtures/break_itip_methods/request_reschedules.ics"
    );
    let current = Calendar::parse(&repaired(HELD).unwrap()).unwrap();
    let message = Message::read(&repaired(REQUEST).unwrap()).unwrap();
    let actor = Actor::new("mailto:chair@example.com").unwrap();

    let review = message.review(&current, &actor).unwrap();
    assert!(review.change_count() > 0);
    let updated = review.authorize().apply().unwrap();
    let bytes = updated.to_bytes();
    assert!(
        bytes
            .windows(b"DTSTART:20260310T160000Z".len())
            .any(|window| window == b"DTSTART:20260310T160000Z")
    );
    assert!(
        bytes
            .windows(b"SEQUENCE:3".len())
            .any(|window| window == b"SEQUENCE:3")
    );
}

#[test]
fn a_stranger_cannot_authorize_an_organizer_request() {
    const HELD: &[u8] =
        include_bytes!("../../ical-conform/tests/fixtures/break_itip_methods/held_series.ics");
    const REQUEST: &[u8] = include_bytes!(
        "../../ical-conform/tests/fixtures/break_itip_methods/request_reschedules.ics"
    );
    let current = Calendar::parse(&repaired(HELD).unwrap()).unwrap();
    let message = Message::read(&repaired(REQUEST).unwrap()).unwrap();
    let stranger = Actor::new("mailto:eve@example.com").unwrap();

    let rejection = message.review(&current, &stranger).unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
}
