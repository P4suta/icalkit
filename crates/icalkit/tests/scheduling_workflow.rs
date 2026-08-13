// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iTIP read-review-authorize-apply and outbound builders through the facade.

use icalkit::caldav::{Match, Query};
use icalkit::interop::{Import, RfcRepairV1};
use icalkit::scheduling::{Actor, Message};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};

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
