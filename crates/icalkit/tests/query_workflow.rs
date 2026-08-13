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
