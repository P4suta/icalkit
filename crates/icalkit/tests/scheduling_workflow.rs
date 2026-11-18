// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iTIP read-review-authorize-apply and outbound builders through the facade.

use icalkit::caldav::{Match, Query};
use icalkit::interop::{Import, RfcRepairV1};
use icalkit::scheduling::{Actor, Message};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, ResourcePolicy};

const WRAP_HEAD: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n";
const WRAP_TAIL: &str = "END:VCALENDAR\r\n";

fn payload(lines: &str) -> Vec<u8> {
    format!("{WRAP_HEAD}BEGIN:VEVENT\r\n{lines}END:VEVENT\r\n{WRAP_TAIL}").into_bytes()
}

fn scheduling_payload(
    method: &str,
    uid: &str,
    dtstamp: &str,
    sequence: u32,
    fields: &str,
) -> Vec<u8> {
    format!(
        "{WRAP_HEAD}METHOD:{method}\r\nBEGIN:VEVENT\r\nUID:{uid}\r\n\
         DTSTAMP:{dtstamp}\r\n{fields}SEQUENCE:{sequence}\r\nEND:VEVENT\r\n{WRAP_TAIL}"
    )
    .into_bytes()
}

fn supplied_time() -> Timestamp {
    Timestamp::constant(0, 0)
}

fn repaired(bytes: &[u8]) -> Result<Vec<u8>, icalkit::Error> {
    let imported = Import::read(bytes)?;
    let normalized = imported.normalize(RfcRepairV1)?;
    Ok(normalized.output().as_bytes().to_vec())
}

fn time_range_query(start: &str, end: &str) -> Vec<u8> {
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:time-range start="{start}" end="{end}"/>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
</C:calendar-query>"#
    )
    .into_bytes()
}

fn time_range_summary_query(start: &str, end: &str, summary: &str) -> Vec<u8> {
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:time-range start="{start}" end="{end}"/>
      <C:prop-filter name="SUMMARY"><C:text-match>{summary}</C:text-match></C:prop-filter>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
</C:calendar-query>"#
    )
    .into_bytes()
}

fn two_component_series(
    method: Option<&str>,
    sequence: u32,
    master_summary: &str,
    detached_summary: &str,
) -> Vec<u8> {
    let method = method.map_or(String::new(), |method| format!("METHOD:{method}\r\n"));
    format!(
        "{WRAP_HEAD}{method}\
BEGIN:VEVENT\r\n\
UID:batch@example.test\r\n\
DTSTAMP:20260302T080000Z\r\n\
DTSTART:20260310T140000Z\r\n\
RRULE:FREQ=WEEKLY;COUNT=2\r\n\
SUMMARY:{master_summary}\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE:mailto:alice@example.test\r\n\
SEQUENCE:{sequence}\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:batch@example.test\r\n\
RECURRENCE-ID:20260317T140000Z\r\n\
DTSTAMP:20260302T080000Z\r\n\
DTSTART:20260317T150000Z\r\n\
SUMMARY:{detached_summary}\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE:mailto:alice@example.test\r\n\
SEQUENCE:{sequence}\r\n\
END:VEVENT\r\n\
{WRAP_TAIL}"
    )
    .into_bytes()
}

fn assert_revised_range_is_queryable(calendar: &Calendar) -> Result<(), icalkit::Error> {
    let moved_future = Query::parse(&time_range_query("20260324T170000Z", "20260324T170001Z"))?;
    let stale_future = Query::parse(&time_range_query("20260324T140000Z", "20260324T140001Z"))?;
    let moved_summary = Query::parse(&time_range_summary_query(
        "20260324T170000Z",
        "20260324T170001Z",
        "Latest sync",
    ))?;
    let stale_summary = Query::parse(&time_range_summary_query(
        "20260324T170000Z",
        "20260324T170001Z",
        "Original sync",
    ))?;
    let engine = Engine::default();
    let mut session = engine.session();

    assert_eq!(
        moved_future.matches(&mut session, calendar)?,
        Match::Matched,
        "the detached range anchor moves later generated occurrences"
    );
    assert_eq!(
        stale_future.matches(&mut session, calendar)?,
        Match::Unmatched,
        "the pre-split start no longer matches future occurrences"
    );
    assert_eq!(
        moved_summary.matches(&mut session, calendar)?,
        Match::Matched,
        "range property changes and time shifts belong to the same future occurrence"
    );
    assert_eq!(
        stale_summary.matches(&mut session, calendar)?,
        Match::Unmatched,
        "a matching time and a property from a different recurrence instance are not joined"
    );
    Ok(())
}

mod range_series {
    pub(crate) const HELD: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range@example.test\r\n",
        "DTSTAMP:20260301T120000Z\r\n",
        "DTSTART:20260310T140000Z\r\n",
        "SUMMARY:Original sync\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.test\r\n",
        "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
        "SEQUENCE:2\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    pub(crate) const REQUEST: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range@example.test\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z\r\n",
        "DTSTAMP:20260302T120000Z\r\n",
        "DTSTART:20260317T160000Z\r\n",
        "SUMMARY:Moved sync\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.test\r\n",
        "SEQUENCE:3\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    pub(crate) const REVISED_REQUEST: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range@example.test\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z\r\n",
        "DTSTAMP:20260303T120000Z\r\n",
        "DTSTART:20260317T170000Z\r\n",
        "SUMMARY:Latest sync\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.test\r\n",
        "SEQUENCE:4\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
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
         ORGANIZER:mailto:chair@example.test\r\nSEQUENCE:2\r\nSTATUS:CANCELLED\r\n",
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
fn imip_content_type_is_checked_against_the_decoded_calendar_body() {
    let request = Message::read(range_series::REQUEST).unwrap();
    let content_type = request.imip_content_type();
    assert_eq!(content_type, "text/calendar; charset=UTF-8; method=REQUEST");
    let received = Message::read_imip(content_type.as_bytes(), &request.to_bytes()).unwrap();
    assert_eq!(received, request);

    for (header, code) in [
        (
            b"text/plain; charset=UTF-8; method=REQUEST".as_slice(),
            "icalkit.scheduling.imip-media-type",
        ),
        (
            b"text/calendar; charset=UTF-8; method=REPLY",
            "icalkit.scheduling.imip-method-mismatch",
        ),
        (
            b"text/calendar; charset=windows-1252; method=REQUEST",
            "icalkit.scheduling.imip-charset-mismatch",
        ),
        (
            b"text/calendar; charset=UTF-8; method=REQUEST; method=CANCEL",
            "icalkit.scheduling.imip-content-type-invalid",
        ),
    ] {
        let error = Message::read_imip(header, range_series::REQUEST).unwrap_err();
        assert_eq!(error.code().as_str(), code, "{header:?}");
    }
}

#[test]
fn imip_charset_gate_sees_the_decoded_octets_and_uses_the_session_budget() {
    let utf8 = payload(
        "UID:utf8@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:Café\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:1\r\n",
    );
    let message = Message::request(&utf8, supplied_time()).unwrap();
    let body = message.to_bytes();

    let absent = Message::read_imip(b"text/calendar; method=REQUEST", &body).unwrap_err();
    assert_eq!(
        absent.code().as_str(),
        "icalkit.scheduling.imip-charset-mismatch"
    );
    Message::read_imip(b"text/calendar; charset=UTF-8; method=REQUEST", &body).unwrap();

    let header = b"text/calendar; charset=UTF-8; method=REQUEST";
    let engine = Engine::builder()
        .resource_policy(
            ResourcePolicy::secure().with_max_input_bytes(u64::try_from(header.len() - 1).unwrap()),
        )
        .build();
    let mut session = engine.session();
    let exhausted = Message::read_imip_in(&mut session, header, &body).unwrap_err();
    assert_eq!(
        exhausted.code().as_str(),
        "icalkit.scheduling.imip-content-type-invalid"
    );
}

#[test]
fn a_cancel_must_identify_a_component_instance_or_affected_attendee() {
    let missing_target = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:CANCEL\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260302T080000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:2\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let error = Message::read(missing_target).unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.scheduling.message-invalid");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "scheduling-required-property-missing")
    );

    let wrong_status = payload(
        "UID:one@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         ORGANIZER:mailto:chair@example.test\r\nSEQUENCE:2\r\nSTATUS:CONFIRMED\r\n",
    );
    let error = Message::read(&{
        let mut bytes = wrong_status;
        let at = bytes
            .windows(b"BEGIN:VEVENT".len())
            .position(|window| window == b"BEGIN:VEVENT")
            .unwrap();
        bytes.splice(at..at, b"METHOD:CANCEL\r\n".iter().copied());
        bytes
    })
    .unwrap_err();
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "scheduling-cancellation-status-invalid")
    );
}

#[test]
fn cancellation_preserves_a_single_event_end() {
    for (stored, offered) in [
        ("DURATION:PT1H", "DTEND:20260310T150000Z"),
        ("DTEND:20260310T150000Z", "DURATION:PT1H"),
    ] {
        let source = str::from_utf8(range_series::HELD).unwrap().replace(
            "SUMMARY:Original sync",
            &format!("{stored}\r\nSUMMARY:Original sync"),
        );
        let current = Calendar::parse(source.as_bytes()).unwrap();
        let cancel = format!(
            "{WRAP_HEAD}METHOD:CANCEL\r\nBEGIN:VEVENT\r\nUID:range@example.test\r\n\
             DTSTAMP:20260302T120000Z\r\nDTSTART:20260310T140000Z\r\n{offered}\r\n\
             ORGANIZER:mailto:chair@example.test\r\nSTATUS:CANCELLED\r\nSEQUENCE:3\r\nEND:VEVENT\r\n{WRAP_TAIL}"
        );
        let message = Message::read(cancel.as_bytes()).unwrap();
        let actor = Actor::new("mailto:chair@example.test").unwrap();
        let updated = message
            .review(&current, &actor)
            .unwrap()
            .authorize()
            .apply()
            .unwrap();
        let written = String::from_utf8(updated.to_bytes()).unwrap();
        assert!(written.contains("STATUS:CANCELLED\r\n"));
        assert!(written.contains("SUMMARY:Original sync\r\n"));
        assert!(written.lines().any(|line| line == stored));
        assert!(!written.lines().any(|line| line == offered));
        assert_eq!(current.to_bytes(), source.as_bytes());
    }
}

#[test]
fn a_statusless_cancel_for_an_affected_attendee_cancels_the_recipient_copy() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let cancel = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:CANCEL\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE:mailto:alice@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(cancel).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    assert!(
        updated
            .windows(b"STATUS:CANCELLED".len())
            .any(|window| window == b"STATUS:CANCELLED")
    );
}

#[test]
fn an_initial_request_can_create_the_event_the_recipient_does_not_hold() {
    let invitation = payload(
        "UID:new@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:First invitation\r\n\
         ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:1\r\n",
    );
    let message = Message::request(&invitation, supplied_time()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let review = message.review_creation(&actor).unwrap();
    assert!(review.change_count() > 0);
    let created = review.authorize().apply().unwrap();
    let event = created.events().next().unwrap();
    assert_eq!(event.uid().as_deref(), Some("new@example.test"));
    assert_eq!(
        event.property("SUMMARY").unwrap().value(),
        b"First invitation"
    );
}

#[test]
fn an_initial_request_creates_every_authorized_component_atomically() {
    let request = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:REQUEST\r\n\
BEGIN:VEVENT\r\n\
UID:new-series@example.test\r\n\
DTSTAMP:20260302T080000Z\r\n\
DTSTART:20260310T140000Z\r\n\
RRULE:FREQ=WEEKLY;COUNT=2\r\n\
SUMMARY:Series\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE:mailto:alice@example.test\r\n\
SEQUENCE:1\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:new-series@example.test\r\n\
RECURRENCE-ID:20260317T140000Z\r\n\
DTSTAMP:20260302T080000Z\r\n\
DTSTART:20260317T150000Z\r\n\
SUMMARY:Moved instance\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE:mailto:alice@example.test\r\n\
SEQUENCE:1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(request).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let created = message
        .review_creation(&actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let bytes = created.to_bytes();
    assert_eq!(
        bytes
            .windows(b"BEGIN:VEVENT".len())
            .filter(|part| *part == b"BEGIN:VEVENT")
            .count(),
        2
    );
    assert!(
        bytes
            .windows(22)
            .any(|part| part == b"SUMMARY:Moved instance")
    );
}

#[test]
fn initial_publish_creation_is_not_limited_to_vevent() {
    let journal = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VJOURNAL\r\n\
UID:journal@example.test\r\n\
DTSTAMP:20260302T080000Z\r\n\
DTSTART:20260310T140000Z\r\n\
DESCRIPTION:Published journal\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
END:VJOURNAL\r\n\
END:VCALENDAR\r\n";
    let message = Message::publish(journal, supplied_time()).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let created = message
        .review_creation(&actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    assert!(
        created
            .windows(b"BEGIN:VJOURNAL".len())
            .any(|part| part == b"BEGIN:VJOURNAL")
    );
}

#[test]
fn a_multi_component_creation_is_refused_whole_when_one_payload_names_another_actor() {
    let mut request =
        two_component_series(Some("REQUEST"), 1, "Authorized master", "Forged detached");
    let second_organizer = request
        .windows(b"ORGANIZER:mailto:chair@example.test".len())
        .enumerate()
        .filter(|(_, part)| *part == b"ORGANIZER:mailto:chair@example.test")
        .nth(1)
        .map(|(at, _)| at)
        .unwrap();
    request.splice(
        second_organizer..second_organizer + b"ORGANIZER:mailto:chair@example.test".len(),
        b"ORGANIZER:mailto:other@example.test".iter().copied(),
    );
    let message = Message::read(&request).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let rejection = message.review_creation(&actor).unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
    assert_eq!(message.to_bytes(), request);
}

#[test]
fn one_review_applies_every_matching_component_in_a_scheduling_object() {
    let current =
        Calendar::parse(&two_component_series(None, 1, "Old master", "Old detached")).unwrap();
    let message = Message::read(&two_component_series(
        Some("REQUEST"),
        2,
        "New master",
        "New detached",
    ))
    .unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let bytes = updated.to_bytes();
    for expected in [b"SUMMARY:New master".as_slice(), b"SUMMARY:New detached"] {
        assert!(
            bytes.windows(expected.len()).any(|part| part == expected),
            "{expected:?}"
        );
    }
    assert!(!bytes.windows(11).any(|part| part == b"SUMMARY:Old"));
}

#[test]
fn a_counter_proposes_a_new_time_until_the_organizer_accepts_it() {
    const HELD: &[u8] =
        include_bytes!("../../icalkit-conformance/tests/fixtures/itip/held_series.ics");
    const COUNTER: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/itip/counter_proposes_a_new_time.ics"
    );
    let current = Calendar::parse(HELD).unwrap();
    let message = Message::read(COUNTER).unwrap();
    let attendee = Actor::new("mailto:bo@example.com").unwrap();
    let review = message.review(&current, &attendee).unwrap();
    assert_eq!(review.change_count(), 2);
    assert_eq!(review.message().method(), "COUNTER");
    assert_eq!(current.to_bytes(), HELD);

    let accepted = review.authorize();
    assert_eq!(current.to_bytes(), HELD);
    let updated = String::from_utf8(accepted.apply().unwrap().to_bytes()).unwrap();
    let expected = std::str::from_utf8(HELD)
        .unwrap()
        .replace(
            "DTSTART:20260310T140000Z\r\n",
            "DTSTART:20260310T160000Z\r\n",
        )
        .replace(
            "DTSTAMP:20260301T120000Z\r\n",
            "DTSTAMP:20260302T080000Z\r\n",
        );
    assert_eq!(updated, expected);
}

#[test]
fn a_reply_can_echo_or_omit_the_start_but_cannot_reschedule() {
    const HELD: &[u8] =
        include_bytes!("../../icalkit-conformance/tests/fixtures/itip/held_series.ics");
    const REPLY: &str = include_str!(
        "../../icalkit-conformance/tests/fixtures/itip/reply_restates_a_new_dtstart.ics"
    );
    let current = Calendar::parse(HELD).unwrap();
    let attendee = Actor::new("mailto:bo@example.com").unwrap();
    let moved = "DTSTART:20260315T220000Z\r\n";
    let held_start = "DTSTART:20260310T140000Z\r\n";
    let message = Message::read(REPLY.as_bytes()).unwrap();
    let denial = message.review(&current, &attendee).unwrap_err();
    assert_eq!(
        denial.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );

    for echo in ["", held_start] {
        let message = Message::read(REPLY.replace(moved, echo).as_bytes()).unwrap();
        let review = message.review(&current, &attendee).unwrap();
        assert_eq!(review.change_count(), 1);
        let updated = String::from_utf8(review.authorize().apply().unwrap().to_bytes()).unwrap();
        assert!(updated.contains(held_start));
        assert!(updated.contains("SUMMARY:Weekly sync\r\n"));
    }
}

#[test]
fn a_reply_may_omit_the_organizers_agent_but_cannot_nominate_one() {
    const HELD: &[u8] =
        include_bytes!("../../icalkit-conformance/tests/fixtures/itip/held_series.ics");
    const REPLY: &str =
        include_str!("../../icalkit-conformance/tests/fixtures/itip/reply_accepted.ics");
    let current = Calendar::parse(HELD).unwrap();
    let attendee = Actor::new("mailto:bo@example.com").unwrap();
    let agent = ";SENT-BY=\"mailto:pa@example.com\"";
    let bare = Message::read(REPLY.replace(agent, "").as_bytes()).unwrap();
    let updated = bare
        .review(&current, &attendee)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let written = String::from_utf8(updated.to_bytes()).unwrap();
    assert!(
        written
            .contains("ORGANIZER;SENT-BY=\"mailto:pa@example.com\":mailto:chair@example.com\r\n")
    );

    let other = REPLY.replace(agent, ";SENT-BY=\"mailto:bo@example.com\"");
    let proposed = Message::read(other.as_bytes()).unwrap();
    assert_eq!(
        proposed
            .review(&current, &attendee)
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.scheduling.authorization-denied"
    );
}

#[test]
fn review_authorization_borrows_inputs_and_apply_consumes_it() {
    const HELD: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_methods/held_series.ics"
    );
    const REQUEST: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_methods/request_reschedules.ics"
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
fn a_later_reply_persists_its_ordering_witness_and_refuses_an_earlier_replay() {
    const HELD: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_sequence/held_series.ics"
    );
    const EARLIER: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_sequence/reply_cy_accepted_earlier.ics"
    );
    const LATER: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_sequence/reply_cy_declined_later.ics"
    );
    let current = Calendar::parse(HELD).unwrap();
    let actor = Actor::new("mailto:cy@example.com").unwrap();
    let updated = Message::read(LATER)
        .unwrap()
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let bytes = updated.to_bytes();
    assert!(
        bytes
            .windows(b"PARTSTAT=DECLINED".len())
            .any(|window| window == b"PARTSTAT=DECLINED")
    );
    assert!(
        bytes
            .windows(b"X-ICALKIT-ANSWERED-AT=20260301T140000Z".len())
            .any(|window| window == b"X-ICALKIT-ANSWERED-AT=20260301T140000Z")
    );

    let rejection = Message::read(EARLIER)
        .unwrap()
        .review(&updated, &actor)
        .unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
    assert_eq!(
        updated.to_bytes(),
        bytes,
        "reviewing a replay cannot mutate the accepted later answer"
    );
}

#[test]
fn a_delegate_reply_is_held_until_an_organizer_request_adds_the_delegate() {
    let held = payload(
        "UID:delegation@example.test\r\nDTSTAMP:20260301T100000Z\r\n\
         DTSTART:20260310T140000Z\r\nSUMMARY:Delegation\r\n\
         ORGANIZER:mailto:c@x\r\n\
         ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:a@x\r\nSEQUENCE:1\r\n",
    );
    let delegated = scheduling_payload(
        "REPLY",
        "delegation@example.test",
        "20260301T120000Z",
        1,
        "ORGANIZER:mailto:c@x\r\n\
         ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:b@x\":mailto:a@x\r\n",
    );
    let waiting_reply = scheduling_payload(
        "REPLY",
        "delegation@example.test",
        "20260301T130000Z",
        1,
        "ORGANIZER:mailto:c@x\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED;DELEGATED-FROM=\"mailto:a@x\":mailto:b@x\r\n",
    );
    let organizer_request = scheduling_payload(
        "REQUEST",
        "delegation@example.test",
        "20260301T140000Z",
        2,
        "DTSTART:20260310T140000Z\r\nSUMMARY:Delegation\r\n\
         ORGANIZER:mailto:c@x\r\n\
         ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:b@x\":mailto:a@x\r\n\
         ATTENDEE;PARTSTAT=NEEDS-ACTION;DELEGATED-FROM=\"mailto:a@x\":mailto:b@x\r\n",
    );
    let released_reply = scheduling_payload(
        "REPLY",
        "delegation@example.test",
        "20260301T150000Z",
        2,
        "ORGANIZER:mailto:c@x\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED;DELEGATED-FROM=\"mailto:a@x\":mailto:b@x\r\n",
    );
    let alice = Actor::new("mailto:a@x").unwrap();
    let bob = Actor::new("mailto:b@x").unwrap();
    let chair = Actor::new("mailto:c@x").unwrap();
    let delegated_state = Message::read(&delegated)
        .unwrap()
        .review(&Calendar::parse(&held).unwrap(), &alice)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();

    let waiting_state = Message::read(&waiting_reply)
        .unwrap()
        .review(&delegated_state, &bob)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    assert!(
        !waiting_state
            .to_bytes()
            .windows(b"ATTENDEE;PARTSTAT=ACCEPTED".len())
            .any(|window| window == b"ATTENDEE;PARTSTAT=ACCEPTED"),
        "the delegate is not yet an attendee the organizer can update"
    );

    let released_state = Message::read(&organizer_request)
        .unwrap()
        .review(&waiting_state, &chair)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let accepted = Message::read(&released_reply)
        .unwrap()
        .review(&released_state, &bob)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    for expected in [
        b"ATTENDEE;PARTSTAT=ACCEPTED;DELEGATED-FROM=\"mailto:a@x\"".as_slice(),
        b"AT=20260301T150000Z:mailto:b@x".as_slice(),
    ] {
        assert!(
            accepted
                .windows(expected.len())
                .any(|window| window == expected),
            "{}",
            String::from_utf8_lossy(&accepted)
        );
    }
}

#[test]
fn a_this_and_future_request_splits_the_series_into_a_detached_anchor() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let message = Message::read(range_series::REQUEST).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let bytes = updated.to_bytes();

    assert_eq!(
        bytes
            .windows(b"BEGIN:VEVENT".len())
            .filter(|window| *window == b"BEGIN:VEVENT")
            .count(),
        2,
        "the series master and one detached anchor remain"
    );
    assert!(
        bytes
            .windows(range_series::HELD.len() - WRAP_TAIL.len())
            .any(|window| {
                window == &range_series::HELD[..range_series::HELD.len() - WRAP_TAIL.len()]
            }),
        "the existing master stays byte-identical"
    );
    assert!(
        bytes
            .windows(b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z".len())
            .any(|window| window == b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z")
    );
    assert!(
        bytes
            .windows(b"DTSTART:20260317T160000Z".len())
            .any(|window| window == b"DTSTART:20260317T160000Z")
    );

    let revised_calendar = Message::read(range_series::REVISED_REQUEST)
        .unwrap()
        .review(&updated, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let revised = revised_calendar.to_bytes();
    assert_eq!(
        revised
            .windows(b"BEGIN:VEVENT".len())
            .filter(|window| *window == b"BEGIN:VEVENT")
            .count(),
        2,
        "a later message for the same anchor updates it rather than splitting again"
    );
    assert!(
        revised
            .windows(b"DTSTART:20260317T170000Z".len())
            .any(|window| window == b"DTSTART:20260317T170000Z")
    );
    assert!(
        !revised
            .windows(b"DTSTART:20260317T160000Z".len())
            .any(|window| window == b"DTSTART:20260317T160000Z")
    );

    assert_revised_range_is_queryable(&revised_calendar).unwrap();
}

#[test]
fn a_vtodo_this_and_future_request_materializes_a_detached_anchor() {
    const HELD: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "BEGIN:VTODO\r\n",
        "UID:todo-range@example.test\r\n",
        "DTSTAMP:20260301T120000Z\r\n",
        "DTSTART:20260310T140000Z\r\n",
        "PRIORITY:5\r\n",
        "SUMMARY:Original task\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.test\r\n",
        "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
        "SEQUENCE:2\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    const REQUEST: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\n",
        "BEGIN:VTODO\r\n",
        "UID:todo-range@example.test\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z\r\n",
        "DTSTAMP:20260302T120000Z\r\n",
        "DTSTART:20260317T160000Z\r\n",
        "PRIORITY:5\r\n",
        "SUMMARY:Moved task\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.test\r\n",
        "SEQUENCE:3\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    let current = Calendar::parse(HELD).unwrap();
    let message = Message::read(REQUEST).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    assert_eq!(
        updated
            .windows(b"BEGIN:VTODO".len())
            .filter(|window| *window == b"BEGIN:VTODO")
            .count(),
        2
    );
    assert!(
        updated
            .windows(b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z".len())
            .any(|window| window == b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z")
    );
}

#[test]
fn a_message_cannot_create_two_answers_for_the_same_occurrence() {
    let fields = "RECURRENCE-ID:20260317T140000Z\r\nORGANIZER:mailto:chair@example.test\r\n\
                  ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n";
    let body = scheduling_payload("REPLY", "range@example.test", "20260302T120000Z", 2, fields);
    let text = str::from_utf8(&body).unwrap();
    let event = text
        .split_once("BEGIN:VEVENT\r\n")
        .unwrap()
        .1
        .strip_suffix(WRAP_TAIL)
        .unwrap();
    let duplicate = text.replace(WRAP_TAIL, &format!("BEGIN:VEVENT\r\n{event}{WRAP_TAIL}"));
    let current = Calendar::parse(range_series::HELD).unwrap();
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    let reply = Message::read(duplicate.as_bytes()).unwrap();
    assert!(reply.review(&current, &actor).is_err());
}

#[test]
fn an_instance_reply_can_echo_the_occurrences_start_and_end() {
    check_generated_reply_times(true).unwrap();
}

#[test]
fn a_sparse_instance_reply_keeps_the_occurrences_start_and_end() {
    check_generated_reply_times(false).unwrap();
}

fn check_generated_reply_times(echo: bool) -> Result<(), Box<dyn std::error::Error>> {
    let held = str::from_utf8(range_series::HELD)?.replace(
        "DTSTART:20260310T140000Z\r\n",
        "DTSTART:20260310T140000Z\r\nDTEND:20260310T150000Z\r\nX-STORED:keep me\r\n",
    );
    let current = Calendar::parse(held.as_bytes())?;
    let times = "DTSTART:20260317T140000Z\r\nDTEND:20260317T150000Z\r\n";
    let fields = format!(
        "RECURRENCE-ID:20260317T140000Z\r\n{}ORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n",
        if echo { times } else { "" }
    );
    let bytes = scheduling_payload(
        "REPLY",
        "range@example.test",
        "20260302T120000Z",
        2,
        &fields,
    );
    let message = Message::read(&bytes)?;
    let actor = Actor::new("mailto:alice@example.test").ok_or("invalid attendee")?;
    let review = message.review(&current, &actor)?;
    let result = String::from_utf8(review.authorize().apply()?.to_bytes())?;
    assert!(result.starts_with(held.strip_suffix(WRAP_TAIL).ok_or("missing calendar end")?));
    let detached = result
        .rsplit_once("BEGIN:VEVENT\r\n")
        .ok_or("missing detached event")?
        .1;
    for line in times.lines() {
        assert!(detached.lines().any(|found| found == line), "{detached}");
    }
    assert!(detached.contains("RECURRENCE-ID:20260317T140000Z\r\n"));
    assert!(detached.contains("X-STORED:keep me\r\n"));
    assert!(detached.contains("PARTSTAT=ACCEPTED"));
    assert!(!detached.contains("RRULE:"));
    Ok(())
}

#[test]
fn an_instance_reply_keeps_date_and_floating_occurrences_in_their_clock() {
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    for (start, id, echo) in [
        (
            "DTSTART;VALUE=DATE:20260310",
            "RECURRENCE-ID;VALUE=DATE:20260317",
            "DTSTART;VALUE=DATE:20260317",
        ),
        (
            "DTSTART:20260310T140000",
            "RECURRENCE-ID:20260317T140000",
            "DTSTART:20260317T140000",
        ),
    ] {
        let held = str::from_utf8(range_series::HELD)
            .unwrap()
            .replace("DTSTART:20260310T140000Z", start);
        let current = Calendar::parse(held.as_bytes()).unwrap();
        let reply = instance_reply(id, echo).unwrap();
        let bytes = reply
            .review(&current, &actor)
            .unwrap()
            .authorize()
            .apply()
            .unwrap()
            .to_bytes();
        let detached = str::from_utf8(&bytes)
            .unwrap()
            .rsplit_once("BEGIN:VEVENT\r\n")
            .unwrap()
            .1;
        for line in [id, echo] {
            assert!(detached.lines().any(|found| found == line), "{detached}");
        }
    }
}

fn instance_reply(identity: &str, times: &str) -> Result<Message, icalkit::Error> {
    let fields = format!(
        "{identity}\r\n{times}\r\nORGANIZER:mailto:chair@example.test\r\n\
                          ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n"
    );
    Message::read(&scheduling_payload(
        "REPLY",
        "range@example.test",
        "20260302T120000Z",
        2,
        &fields,
    ))
}

#[test]
fn an_instance_reply_cannot_change_an_occurrence_or_target_an_excluded_key() {
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    let original = str::from_utf8(range_series::HELD).unwrap();
    let held = original.replace("SUMMARY:", "DTEND:20260310T150000Z\r\nSUMMARY:");
    let id = "RECURRENCE-ID:20260317T140000Z";
    let changed = instance_reply(id, "DTEND:20260317T160000Z").unwrap();
    assert!(
        changed
            .review(&Calendar::parse(held.as_bytes()).unwrap(), &actor)
            .is_err()
    );
    let excluded = held.replace("SUMMARY:", "EXDATE:20260317T140000Z\r\nSUMMARY:");
    let echo = instance_reply(id, "DTEND:20260317T150000Z").unwrap();
    assert!(
        echo.review(&Calendar::parse(excluded.as_bytes()).unwrap(), &actor)
            .is_err()
    );
}

#[test]
fn an_instance_reply_uses_the_duration_of_an_included_period() {
    let held = str::from_utf8(range_series::HELD).unwrap().replace(
        "RRULE:",
        "RDATE;VALUE=PERIOD:20260320T140000Z/PT2H\r\nRRULE:",
    );
    let current = Calendar::parse(held.as_bytes()).unwrap();
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    let time = "DTSTART:20260320T140000Z\r\nDTEND:20260320T160000Z";
    let reply = instance_reply("RECURRENCE-ID:20260320T140000Z", time).unwrap();
    let bytes = reply
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    let detached = str::from_utf8(&bytes)
        .unwrap()
        .rsplit_once("BEGIN:VEVENT\r\n")
        .unwrap()
        .1;
    for line in time.lines() {
        assert!(detached.lines().any(|found| found == line), "{detached}");
    }
    assert!(!detached.contains("RDATE"));
}

#[test]
fn a_generated_instance_reply_cannot_choose_a_side_of_a_repeated_hour() {
    let zone = "BEGIN:VTIMEZONE\r\nTZID:Private/Fold\r\nBEGIN:STANDARD\r\n\
                DTSTART:20260317T150000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
                END:STANDARD\r\nEND:VTIMEZONE\r\n";
    let held = str::from_utf8(range_series::HELD)
        .unwrap()
        .replace("BEGIN:VEVENT\r\n", &format!("{zone}BEGIN:VEVENT\r\n"))
        .replace(
            "DTSTART:20260310T140000Z",
            "DTSTART;TZID=Private/Fold:20260310T143000",
        );
    let current = Calendar::parse(held.as_bytes()).unwrap();
    let reply = instance_reply(
        "RECURRENCE-ID;TZID=Private/Fold:20260317T143000",
        "SUMMARY:Original sync",
    )
    .unwrap();
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    assert!(reply.review(&current, &actor).is_err());
}

#[test]
fn an_instance_reply_requires_the_original_date_or_time_form_of_the_key() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let actor = Actor::new("mailto:alice@example.test").unwrap();
    let reply = instance_reply("RECURRENCE-ID:20260317T140000", "SUMMARY:Original sync").unwrap();
    assert!(reply.review(&current, &actor).is_err());
}

#[test]
fn an_instance_reply_uses_the_schedule_from_the_latest_range_request() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let organizer = Actor::new("mailto:chair@example.test").unwrap();
    let changed = Message::read(range_series::REQUEST)
        .unwrap()
        .review(&current, &organizer)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let attendee = Actor::new("mailto:alice@example.test").unwrap();
    let fields = "RECURRENCE-ID:20260324T140000Z\r\nDTSTART:20260324T160000Z\r\n\
                  SUMMARY:Moved sync\r\nORGANIZER:mailto:chair@example.test\r\n\
                  ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n";
    let message = Message::read(&scheduling_payload(
        "REPLY",
        "range@example.test",
        "20260303T120000Z",
        3,
        fields,
    ))
    .unwrap();
    let written = String::from_utf8(
        message
            .review(&changed, &attendee)
            .unwrap()
            .authorize()
            .apply()
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let detached = written.rsplit_once("BEGIN:VEVENT\r\n").unwrap().1;
    for line in [
        "RECURRENCE-ID:20260324T140000Z",
        "DTSTART:20260324T160000Z",
        "SUMMARY:Moved sync",
        "SEQUENCE:3",
    ] {
        assert!(detached.lines().any(|found| found == line), "{detached}");
    }
    assert!(!detached.contains("RANGE="));
    assert!(!detached.contains("RRULE:"));
}

#[test]
fn a_detached_reply_keeps_the_stored_time_zone_and_local_clock() {
    let zone = "BEGIN:VTIMEZONE\r\nTZID:Private/Schedule\r\nBEGIN:STANDARD\r\n\
                DTSTART:19700101T000000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0400\r\n\
                END:STANDARD\r\nEND:VTIMEZONE\r\n";
    let held = str::from_utf8(range_series::HELD).unwrap()
        .replace("BEGIN:VEVENT\r\n", &format!("{zone}BEGIN:VEVENT\r\n"))
        .replace("DTSTART:20260310T140000Z", "DTSTART;TZID=Private/Schedule:20260310T100000\r\nDTEND;TZID=Private/Schedule:20260310T110000");
    let current = Calendar::parse(held.as_bytes()).unwrap();
    let fields = "RECURRENCE-ID;TZID=Private/Schedule:20260317T100000\r\n\
                  DTSTART;TZID=Private/Schedule:20260317T100000\r\n\
                  DTEND;TZID=Private/Schedule:20260317T110000\r\n\
                  ORGANIZER:mailto:chair@example.test\r\n\
                  ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n";
    let bytes = scheduling_payload("REPLY", "range@example.test", "20260302T120000Z", 2, fields);
    let message = Message::read(&bytes).unwrap();
    let attendee = Actor::new("mailto:alice@example.test").unwrap();
    let result = String::from_utf8(
        message
            .review(&current, &attendee)
            .unwrap()
            .authorize()
            .apply()
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let detached = result.rsplit_once("BEGIN:VEVENT\r\n").unwrap().1;
    for line in fields.lines().take(3) {
        assert!(detached.lines().any(|found| found == line), "{detached}");
    }
    assert!(result.contains(zone));
    let foreign_zone = zone.replace("-0400", "-0500");
    let divergent = str::from_utf8(&bytes).unwrap().replace(
        "BEGIN:VEVENT\r\n",
        &format!("{foreign_zone}BEGIN:VEVENT\r\n"),
    );
    assert!(
        Message::read(divergent.as_bytes())
            .unwrap()
            .review(&current, &attendee)
            .is_err()
    );
    let repeat = scheduling_payload(
        "REPLY",
        "range@example.test",
        "20260303T120000Z",
        2,
        "RECURRENCE-ID:20260317T140000Z\r\nORGANIZER:mailto:chair@example.test\r\n\
         ATTENDEE;PARTSTAT=DECLINED:mailto:alice@example.test\r\n",
    );
    let updated = Calendar::parse(result.as_bytes()).unwrap();
    let same_key = str::from_utf8(&repeat).unwrap().replace(
        "RECURRENCE-ID:20260317T140000Z",
        "RECURRENCE-ID;TZID=Private/Schedule:20260317T100000",
    );
    for input in [same_key.as_bytes(), repeat.as_slice()] {
        let reapplied = Message::read(input)
            .unwrap()
            .review(&updated, &attendee)
            .unwrap()
            .authorize()
            .apply()
            .unwrap();
        let bytes = reapplied.to_bytes();
        let text = str::from_utf8(&bytes).unwrap();
        assert_eq!(text.matches("BEGIN:VEVENT\r\n").count(), 2);
        let detached = text.rsplit_once("BEGIN:VEVENT\r\n").unwrap().1;
        let unfolded = detached.replace("\r\n ", "");
        assert!(
            unfolded
                .lines()
                .any(|line| line.starts_with("ATTENDEE;PARTSTAT=DECLINED;")
                    && line.ends_with(":mailto:alice@example.test")),
            "{detached}"
        );
        assert!(detached.contains("RECURRENCE-ID;TZID=Private/Schedule:20260317T100000\r\n"));
    }
}

#[test]
fn a_reply_for_an_unmaterialized_instance_creates_an_addressed_detached_state() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let reply = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:REPLY\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID:20260317T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n\
SEQUENCE:2\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(reply).unwrap();
    let actor = Actor::new("mailto:alice@example.test").unwrap();

    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();
    assert_eq!(
        updated
            .windows(b"BEGIN:VEVENT".len())
            .filter(|window| *window == b"BEGIN:VEVENT")
            .count(),
        2
    );
    for expected in [
        b"RECURRENCE-ID:20260317T140000Z".as_slice(),
        b"ATTENDEE;PARTSTAT=ACCEPTED".as_slice(),
    ] {
        assert!(
            updated
                .windows(expected.len())
                .any(|window| window == expected),
            "{expected:?}"
        );
    }
}

#[test]
fn a_this_and_future_cancel_is_stored_as_cancelled_for_later_occurrences() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let cancel = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:CANCEL\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(cancel).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap();
    let bytes = updated.to_bytes();

    for expected in [
        b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260317T140000Z".as_slice(),
        b"STATUS:CANCELLED".as_slice(),
    ] {
        assert!(
            bytes
                .windows(expected.len())
                .any(|window| window == expected),
            "{expected:?}"
        );
    }

    let query = Query::parse(
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:time-range start="20260324T140000Z" end="20260324T140001Z"/>
      <C:prop-filter name="STATUS"><C:text-match>CANCELLED</C:text-match></C:prop-filter>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
</C:calendar-query>"#,
    )
    .unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &updated).unwrap(),
        Match::Matched,
        "the cancellation anchor governs later recurrence instances"
    );
}

#[test]
fn one_cancel_materializes_multiple_instances_atomically() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let cancel = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:CANCEL\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID:20260317T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID:20260324T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(cancel).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let updated = message
        .review(&current, &actor)
        .unwrap()
        .authorize()
        .apply()
        .unwrap()
        .to_bytes();

    assert_eq!(
        updated
            .windows(b"BEGIN:VEVENT".len())
            .filter(|window| *window == b"BEGIN:VEVENT")
            .count(),
        3
    );
    assert_eq!(
        updated
            .windows(b"STATUS:CANCELLED".len())
            .filter(|window| *window == b"STATUS:CANCELLED")
            .count(),
        2
    );
    for recurrence_id in [
        b"RECURRENCE-ID:20260317T140000Z".as_slice(),
        b"RECURRENCE-ID:20260324T140000Z".as_slice(),
    ] {
        assert!(
            updated
                .windows(recurrence_id.len())
                .any(|window| window == recurrence_id)
        );
    }
}

#[test]
fn a_multi_instance_cancel_is_refused_whole_when_one_key_is_not_in_the_series() {
    let current = Calendar::parse(range_series::HELD).unwrap();
    let cancel = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
METHOD:CANCEL\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID:20260317T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:range@example.test\r\n\
RECURRENCE-ID:20260318T140000Z\r\n\
DTSTAMP:20260302T120000Z\r\n\
ORGANIZER:mailto:chair@example.test\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let message = Message::read(cancel).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let rejection = message.review(&current, &actor).unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
    assert_eq!(
        current.to_bytes(),
        range_series::HELD,
        "review cannot partially materialize the first valid cancellation"
    );
}

#[test]
fn a_new_instance_is_not_attached_to_an_ambiguous_duplicate_master() {
    let mut duplicated = range_series::HELD.to_vec();
    let event_start = range_series::HELD
        .windows(b"BEGIN:VEVENT".len())
        .position(|window| window == b"BEGIN:VEVENT")
        .unwrap();
    let event_end = range_series::HELD
        .windows(b"END:VEVENT\r\n".len())
        .position(|window| window == b"END:VEVENT\r\n")
        .map(|at| at + b"END:VEVENT\r\n".len())
        .unwrap();
    let duplicate = range_series::HELD[event_start..event_end].to_vec();
    let calendar_end = duplicated
        .windows(b"END:VCALENDAR".len())
        .position(|window| window == b"END:VCALENDAR")
        .unwrap();
    duplicated.splice(calendar_end..calendar_end, duplicate);
    let current = Calendar::parse(&duplicated).unwrap();
    let message = Message::read(range_series::REQUEST).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let rejection = message.review(&current, &actor).unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
}

#[test]
fn a_this_and_future_anchor_must_belong_to_the_series_recurrence_set() {
    const HELD: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range-membership@example.test\r\n",
        "DTSTAMP:20260301T120000Z\r\n",
        "DTSTART:20260310T140000Z\r\n",
        "SUMMARY:Weekly sync\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE:mailto:alice@example.test\r\n",
        "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
        "SEQUENCE:2\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    const REQUEST: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range-membership@example.test\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260318T140000Z\r\n",
        "DTSTAMP:20260302T120000Z\r\n",
        "DTSTART:20260318T160000Z\r\n",
        "SUMMARY:Weekly sync later\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE:mailto:alice@example.test\r\n",
        "SEQUENCE:3\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();

    let current = Calendar::parse(HELD).unwrap();
    let message = Message::read(REQUEST).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();

    let rejection = message.review(&current, &actor).unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
}

#[test]
fn a_range_anchor_in_a_dst_gap_is_refused_through_the_session_zone_port() {
    const HELD: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range-gap@example.test\r\n",
        "DTSTAMP:20260301T120000Z\r\n",
        "DTSTART;TZID=Test/Gap:20260307T023000\r\n",
        "SUMMARY:Daily sync\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE:mailto:alice@example.test\r\n",
        "RRULE:FREQ=DAILY;COUNT=3\r\n",
        "SEQUENCE:2\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();
    const REQUEST: &[u8] = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:range-gap@example.test\r\n",
        "RECURRENCE-ID;TZID=Test/Gap;RANGE=THISANDFUTURE:20260308T023000\r\n",
        "DTSTAMP:20260302T120000Z\r\n",
        "DTSTART;TZID=Test/Gap:20260308T033000\r\n",
        "SUMMARY:Daily sync later\r\n",
        "ORGANIZER:mailto:chair@example.test\r\n",
        "ATTENDEE:mailto:alice@example.test\r\n",
        "SEQUENCE:3\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    )
    .as_bytes();

    #[derive(Debug)]
    struct GapZone;

    impl ZoneDatabase for GapZone {
        fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
            if tzid != "Test/Gap" {
                return None;
            }
            if local.month() == 3 && local.day() == 8 && local.hour() == 2 {
                Some(ZoneResolution::gap("test-gap", true))
            } else {
                Some(ZoneResolution::exact(
                    Timestamp::constant(0, 0),
                    "test-gap",
                    true,
                ))
            }
        }

        fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
            (tzid == "Test/Gap")
                .then(|| OffsetResolution::new(0, "test-gap", true))
                .flatten()
        }
    }

    let current = Calendar::parse(HELD).unwrap();
    let message = Message::read(REQUEST).unwrap();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    let engine = Engine::builder().zone_database(GapZone).build();
    let mut session = engine.session();

    let rejection = message
        .review_in(&mut session, &current, &actor)
        .unwrap_err();
    assert_eq!(
        rejection.code().as_str(),
        "icalkit.scheduling.authorization-denied"
    );
}

#[test]
fn a_stranger_cannot_authorize_an_organizer_request() {
    const HELD: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_methods/held_series.ics"
    );
    const REQUEST: &[u8] = include_bytes!(
        "../../icalkit-conformance/tests/fixtures/break_itip_methods/request_reschedules.ics"
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
