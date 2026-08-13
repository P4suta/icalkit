// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV query parsing and evaluation through the public facade only.

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};

const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
DTSTART:20260814T090000\r\n\
SUMMARY:Planning\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

fn summary_query(text: &str) -> Vec<u8> {
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:prop-filter name="SUMMARY"><C:text-match>{text}</C:text-match></C:prop-filter>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
</C:calendar-query>"#
    )
    .into_bytes()
}

#[test]
fn a_query_is_read_and_evaluated_without_exposing_xml_vocabulary() {
    let calendar = Calendar::parse(CALENDAR).unwrap();
    let matching = Query::parse(&summary_query("plan")).unwrap();
    let missing = Query::parse(&summary_query("finance")).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    assert_eq!(
        matching.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    assert_eq!(
        missing.matches(&mut session, &calendar).unwrap(),
        Match::Unmatched
    );
}

#[test]
fn an_unplaced_floating_time_is_undecided_not_unmatched() {
    const TIME_RANGE: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:time-range start="20260814T000000Z" end="20260815T000000Z"/>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
</C:calendar-query>"#;

    let query = Query::parse(TIME_RANGE).unwrap();
    let calendar = Calendar::parse(CALENDAR).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Undecided
    );
}

#[test]
fn unsafe_xml_is_refused_at_the_facade_boundary() {
    let error = Query::parse(
        br#"<!DOCTYPE calendar-query [<!ENTITY xxe SYSTEM "file:///secret">]>
<calendar-query xmlns="urn:ietf:params:xml:ns:caldav"/>"#,
    )
    .unwrap_err();

    assert_eq!(error.code().as_str(), "icalkit.caldav.query-invalid");
}

#[test]
fn partial_calendar_data_is_returned_as_a_non_persistable_projection() {
    const PARTIAL: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:comp name="VCALENDAR">
  <C:prop name="VERSION"/>
  <C:comp name="VEVENT"><C:prop name="UID"/><C:prop name="SUMMARY"/></C:comp>
</C:comp></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(PARTIAL).unwrap();
    let calendar = Calendar::parse(CALENDAR).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    assert!(
        projected
            .as_bytes()
            .windows(b"SUMMARY:Planning".len())
            .any(|part| part == b"SUMMARY:Planning")
    );
    assert!(
        projected
            .as_bytes()
            .windows(b"UID:one@example.test".len())
            .any(|part| part == b"UID:one@example.test")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(7)
            .any(|part| part == b"DTSTAMP")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(7)
            .any(|part| part == b"DTSTART")
    );
}

#[test]
fn freebusy_projection_keeps_only_periods_inside_the_requested_window() {
    const BUSY: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VFREEBUSY\r\n\
UID:busy@example.test\r\n\
DTSTAMP:19970901T120000Z\r\n\
FREEBUSY:19970308T160000Z/PT3H,19970308T200000Z/PT1H,19970308T230000Z/1997\r\n\
 \x200309T000000Z\r\n\
END:VFREEBUSY\r\n\
END:VCALENDAR\r\n";
    const QUERY: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
  <C:limit-freebusy-set start="19970308T200000Z" end="19970308T230000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(QUERY).unwrap();
    let calendar = Calendar::parse(BUSY).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    assert!(
        projected
            .as_bytes()
            .windows(b"FREEBUSY:19970308T200000Z/PT1H".len())
            .any(|part| part == b"FREEBUSY:19970308T200000Z/PT1H")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(15)
            .any(|part| part == b"19970308T160000Z")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(15)
            .any(|part| part == b"19970308T230000Z")
    );
}

#[test]
fn recurrence_projection_keeps_the_master_and_only_overrides_impacting_the_window() {
    const RECURRING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:series@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060102T120000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
SUMMARY:Master\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:series@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060104T140000Z\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID:20060104T120000Z\r\n\
SUMMARY:Inside override\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:series@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060106T140000Z\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID:20060106T120000Z\r\n\
SUMMARY:Outside override\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    const QUERY: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
  <C:limit-recurrence-set start="20060103T000000Z" end="20060105T000000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(QUERY).unwrap();
    let calendar = Calendar::parse(RECURRING).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    assert!(
        projected
            .as_bytes()
            .windows(b"SUMMARY:Master".len())
            .any(|part| part == b"SUMMARY:Master")
    );
    assert!(
        projected
            .as_bytes()
            .windows(b"SUMMARY:Inside override".len())
            .any(|part| part == b"SUMMARY:Inside override")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(b"SUMMARY:Outside override".len())
            .any(|part| part == b"SUMMARY:Outside override")
    );
}

#[test]
fn this_and_future_override_is_kept_when_a_later_instance_impacts_the_window() {
    const RECURRING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:future@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060102T120000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
LOCATION:Headquarters\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:future@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060103T120000Z\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;RANGE=THISANDFUTURE:20060103T120000Z\r\n\
LOCATION:Remote\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    const QUERY: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
  <C:limit-recurrence-set start="20060105T000000Z" end="20060106T000000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(QUERY).unwrap();
    let calendar = Calendar::parse(RECURRING).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    assert!(
        projected
            .as_bytes()
            .windows(b"LOCATION:Remote".len())
            .any(|part| part == b"LOCATION:Remote")
    );
}

#[test]
fn expand_projection_replaces_a_rule_with_bounded_utc_instances() {
    const RECURRING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:expand@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060102T120000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
SUMMARY:Master\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:expand@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060104T140000Z\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID:20060104T120000Z\r\n\
SUMMARY:Moved override\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    const QUERY: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
  <C:expand start="20060103T000000Z" end="20060105T000000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(QUERY).unwrap();
    let calendar = Calendar::parse(RECURRING).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    let bytes = projected.as_bytes();
    assert!(!bytes.windows(6).any(|part| part == b"RRULE:"));
    assert_eq!(
        bytes
            .windows(b"BEGIN:VEVENT".len())
            .filter(|part| *part == b"BEGIN:VEVENT")
            .count(),
        2
    );
    assert!(
        bytes
            .windows(24)
            .any(|part| part == b"DTSTART:20060103T120000Z")
    );
    assert!(
        bytes
            .windows(22)
            .any(|part| part == b"DTEND:20060103T130000Z")
    );
    assert!(
        bytes
            .windows(24)
            .any(|part| part == b"DTSTART:20060104T140000Z")
    );
    assert!(
        bytes
            .windows(b"SUMMARY:Moved override".len())
            .any(|part| part == b"SUMMARY:Moved override")
    );
}

#[test]
fn expand_projection_applies_this_and_future_property_changes() {
    const RECURRING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit query tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:expand-future@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060102T120000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
LOCATION:Headquarters\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:expand-future@example.test\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART:20060103T120000Z\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;RANGE=THISANDFUTURE:20060103T120000Z\r\n\
LOCATION:Remote\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    const QUERY: &[u8] =
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
  <C:expand start="20060105T000000Z" end="20060106T000000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;

    let query = Query::parse(QUERY).unwrap();
    let calendar = Calendar::parse(RECURRING).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let projected = query.project(&mut session, &calendar).unwrap();
    assert!(
        projected
            .as_bytes()
            .windows(b"LOCATION:Remote".len())
            .any(|part| part == b"LOCATION:Remote")
    );
    assert!(
        !projected
            .as_bytes()
            .windows(b"LOCATION:Headquarters".len())
            .any(|part| part == b"LOCATION:Headquarters")
    );
}

struct FixedNewYork;

impl ZoneDatabase for FixedNewYork {
    fn resolve_local(&self, tzid: &str, _local: DateTime) -> Option<ZoneResolution> {
        if tzid != "America/New_York" {
            return None;
        }
        let instant = "2026-08-14T13:00:00Z".parse::<Timestamp>().ok()?;
        Some(ZoneResolution::exact(instant, "test-fixed-zone", true))
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "America/New_York")
            .then_some(())
            .and_then(|()| OffsetResolution::new(-14_400, "test-fixed-zone", true))
    }
}

#[test]
fn a_query_timezone_places_floating_values_through_the_engine_zone_port() {
    let timezone = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:America/New_York\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
    let body = format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data/></D:prop>
<C:filter>
  <C:comp-filter name="VCALENDAR">
    <C:comp-filter name="VEVENT">
      <C:time-range start="20260814T120000Z" end="20260814T140000Z"/>
    </C:comp-filter>
  </C:comp-filter>
</C:filter>
<C:timezone>{timezone}</C:timezone>
</C:calendar-query>"#
    );
    let query = Query::parse(body.as_bytes()).unwrap();
    let calendar = Calendar::parse(CALENDAR).unwrap();
    let engine = Engine::builder().zone_database(FixedNewYork).build();
    let mut session = engine.session();

    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
}
