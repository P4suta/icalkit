// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Query definitions follow [RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, ResourcePolicy};
use icalkit_conformance::internal::core::{
    Component, DecodeValue, Document, Duration, Limits, Meter,
};
use icalkit_conformance::internal::dav::{
    DecodeContext, RequestBody, UnknownPolicy, XmlPull, XmlReader,
};
use icalkit_conformance::internal::tz::read_calendar_zones;

fn zone(observance_properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit inline zone//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Inline\r\nBEGIN:STANDARD\r\n\
         DTSTART:20200101T000000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0200\r\n\
         {observance_properties}END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn calendar(kind: &str, properties: &str) -> Calendar {
    Calendar::parse(
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit inline query//EN\r\n\
         BEGIN:{kind}\r\nUID:zone@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {properties}END:{kind}\r\nEND:VCALENDAR\r\n"
        )
        .as_bytes(),
    )
    .unwrap()
}

fn request(timezone: &str, kind: &str, start: &str, end: &str) -> String {
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="{kind}">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{timezone}]]></C:timezone></C:calendar-query>"#
    )
}

fn query(timezone: &str, kind: &str, start: &str, end: &str) -> Query {
    Query::parse(request(timezone, kind, start, end).as_bytes()).unwrap()
}

fn text<'a>(component: &'a Component, name: &[u8]) -> Option<&'a str> {
    let property = component
        .properties()
        .find(|property| property.is_named(name))?;
    std::str::from_utf8(property.value_text().as_bytes()).ok()
}

struct ConflictingZone;

impl ZoneDatabase for ConflictingZone {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        if tzid != "Test/Inline" {
            return None;
        }
        let instant = format!("{local}+09:00").parse::<Timestamp>().ok()?;
        Some(ZoneResolution::exact(
            instant,
            "conflicting-query-zone",
            true,
        ))
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "Test/Inline")
            .then(|| OffsetResolution::new(32_400, "conflicting-query-zone", true))
            .flatten()
    }
}

#[test]
fn xml_line_endings_in_the_query_definition_place_floating_events() {
    let timezone = zone("").replace("\r\n", "\n");
    let query = query(&timezone, "VEVENT", "20240102T070000Z", "20240102T073000Z");
    let resource = calendar("VEVENT", "DTSTART:20240102T090000\r\nDURATION:PT1H\r\n");
    assert_eq!(
        query
            .matches(&mut Engine::default().session(), &resource)
            .unwrap(),
        Match::Matched
    );
}

#[test]
fn floating_values_use_the_inline_definition_without_an_external_database() {
    let query = query(&zone(""), "VEVENT", "20240102T070000Z", "20240102T073000Z");
    let calendar = calendar("VEVENT", "DTSTART:20240102T090000\r\nDURATION:PT1H\r\n");
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    let projected = query.project(&mut session, &calendar).unwrap();
    assert_eq!(
        projected_property(&projected, b"VEVENT", b"DTSTART"),
        "20240102T070000Z"
    );
}

#[test]
fn dates_use_the_inline_definition_without_an_external_database() {
    let query = query(
        &zone(""),
        "VJOURNAL",
        "20240101T223000Z",
        "20240101T230000Z",
    );
    let calendar = calendar("VJOURNAL", "DTSTART;VALUE=DATE:20240102\r\n");
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    let projected = query.project(&mut session, &calendar).unwrap();
    assert_eq!(
        projected_property(&projected, b"VJOURNAL", b"DTSTART"),
        "20240102"
    );
}

#[test]
fn the_inline_definition_wins_for_floating_values_with_a_conflicting_database() {
    let query = query(&zone(""), "VEVENT", "20240102T070000Z", "20240102T073000Z");
    let calendar = calendar("VEVENT", "DTSTART:20240102T090000\r\nDURATION:PT1H\r\n");
    let engine = Engine::builder().zone_database(ConflictingZone).build();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
}

#[test]
fn explicit_zone_values_keep_the_callers_zone_source() {
    let query = query(&zone(""), "VEVENT", "20240102T000000Z", "20240102T003000Z");
    let calendar = calendar(
        "VEVENT",
        "DTSTART;TZID=Test/Inline:20240102T090000\r\nDURATION:PT1H\r\n",
    );
    let engine = Engine::builder().zone_database(ConflictingZone).build();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
}

#[test]
fn floating_recurrences_use_inline_transitions_and_nominal_days() {
    let timezone = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit inline spring//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Test/Inline\r\n",
        "BEGIN:STANDARD\r\nDTSTART:20231105T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\nEND:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:20240310T020000\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r\nEND:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    let query = query(timezone, "VEVENT", "20240309T000000Z", "20240312T000000Z");
    let calendar = calendar(
        "VEVENT",
        "DTSTART:20240309T120000\r\nDURATION:P1D\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    );
    let engine = Engine::default();
    let mut session = engine.session();
    let projected = query.project(&mut session, &calendar).unwrap();
    Calendar::parse(projected.as_bytes()).unwrap();
    let document = Document::parse(projected.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let times: Vec<_> = document
        .components()
        .next()
        .unwrap()
        .components()
        .map(|event| {
            (
                text(event, b"DTSTART").unwrap(),
                text(event, b"DTEND").unwrap(),
            )
        })
        .collect();
    assert_eq!(
        times,
        [
            ("20240309T170000Z", "20240310T160000Z"),
            ("20240310T160000Z", "20240311T160000Z")
        ]
    );
}

#[test]
fn incomplete_inline_definitions_are_rejected() {
    let valid = zone("");
    for invalid in [
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Test/Inline\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n".to_owned(),
        valid.replace("TZOFFSETTO:+0200\r\n", ""),
        valid.replace("TZOFFSETFROM:+0200", "TZOFFSETFROM:+2500"),
    ] {
        assert!(Query::parse(request(&invalid, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()).is_err());
    }
}

#[test]
fn observance_dates_require_local_date_time_values() {
    for property in [
        "RDATE:20240101T000000Z\r\n",
        "RDATE;TZID=Test/Inline:20240101T000000\r\n",
        "RDATE;VALUE=DATE:20240101\r\n",
        "RDATE;VALUE=PERIOD:20240101T000000/20240101T010000\r\n",
    ] {
        let timezone = zone(property);
        assert!(Calendar::parse(timezone.as_bytes()).is_err(), "{property}");
        assert!(
            Query::parse(
                request(&timezone, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()
            )
            .is_err()
        );
    }
}

#[test]
fn local_observance_dates_remain_supported() {
    let timezone = zone("RDATE:20240101T000000\r\n");
    assert!(Calendar::parse(timezone.as_bytes()).is_ok());
    assert!(
        Query::parse(
            request(&timezone, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()
        )
        .is_ok()
    );
}

#[test]
fn other_calendar_components_do_not_change_the_single_zone_definition() {
    let event = concat!(
        "BEGIN:VEVENT\r\nUID:unrelated@example.test\r\nDTSTAMP:20240101T000000Z\r\n",
        "DTSTART:20240101T090000Z\r\nEND:VEVENT\r\n",
    );
    for timezone in [
        zone("").replace("END:VCALENDAR\r\n", &format!("{event}END:VCALENDAR\r\n")),
        zone("").replace(
            "BEGIN:VTIMEZONE\r\n",
            &format!("{event}BEGIN:VTIMEZONE\r\n"),
        ),
    ] {
        let query = query(&timezone, "VEVENT", "20240102T070000Z", "20240102T073000Z");
        let calendar = calendar("VEVENT", "DTSTART:20240102T090000\r\nDURATION:PT1H\r\n");
        let engine = Engine::default();
        let mut session = engine.session();
        assert_eq!(
            query.matches(&mut session, &calendar).unwrap(),
            Match::Matched
        );
    }
}

#[test]
fn query_objects_require_exactly_one_time_zone_definition() {
    let timezone = zone("").replace(
        "END:VCALENDAR\r\n",
        concat!(
            "BEGIN:VTIMEZONE\r\nTZID:Test/Other\r\nBEGIN:STANDARD\r\n",
            "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0000\r\n",
            "END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
        ),
    );
    assert!(Calendar::parse(timezone.as_bytes()).is_ok());
    assert!(
        Query::parse(
            request(&timezone, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()
        )
        .is_err()
    );
}

#[test]
fn unsupported_inline_rules_are_rejected_without_an_external_fallback() {
    let timezone = zone("RRULE:FREQ=DAILY;COUNT=2\r\n");
    assert!(
        Query::parse(
            request(&timezone, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()
        )
        .is_err()
    );
}

#[test]
fn opaque_transition_values_are_not_interpreted() {
    for property in [
        "RRULE;VALUE=X-FUTURE:FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1\r\n",
        "RDATE;VALUE=X-FUTURE:20240101T000000\r\n",
    ] {
        let timezone = zone(property);
        assert!(
            Query::parse(
                request(&timezone, "VEVENT", "20240101T000000Z", "20240103T000000Z").as_bytes()
            )
            .is_err()
        );
    }
}

#[test]
fn opaque_metadata_does_not_prevent_use_of_a_readable_definition() {
    let timezone = zone("X-META;VALUE=X-FUTURE:opaque\r\n");
    let query = query(&timezone, "VEVENT", "20240102T070000Z", "20240102T073000Z");
    let calendar = calendar("VEVENT", "DTSTART:20240102T090000\r\nDURATION:PT1H\r\n");
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
}

#[test]
fn inline_query_parsing_uses_the_session_input_budget() {
    let body = request(&zone(""), "VEVENT", "20240101T000000Z", "20240103T000000Z");
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut notes = Vec::new();
    let mut xml = XmlReader::new(body.as_bytes());
    let mut context = DecodeContext::new(Limits::DEFAULT, &mut meter, &mut notes)
        .with_unknown(UnknownPolicy::Reject);
    RequestBody::read(&mut xml, &mut context).unwrap();
    assert!(xml.next_event(&mut context).unwrap().is_none());
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_max_input_bytes(meter.spent()))
        .build();
    let error = engine.session().query(body.as_bytes()).unwrap_err();
    assert_eq!(
        error.code().as_str(),
        "icalkit.caldav.query-timezone-invalid"
    );
    assert!(Engine::default().session().query(body.as_bytes()).is_ok());
}

fn projected_property(
    projection: &icalkit::caldav::ProjectedCalendar,
    kind: &[u8],
    property: &[u8],
) -> String {
    let doc = Document::parse(projection.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let component = doc
        .components()
        .next()
        .unwrap()
        .components()
        .find(|c| c.is_named(kind))
        .unwrap();
    text(component, property).unwrap().to_string()
}

#[test]
fn explicit_zones_use_the_stored_definition_when_expanded() {
    let event = "BEGIN:VEVENT\r\nUID:stored@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART;TZID=Test/Inline:20240102T090000\r\nDURATION:PT1H\r\nEND:VEVENT\r\n";
    let stored = zone("").replace("END:VCALENDAR", &format!("{event}END:VCALENDAR"));
    let resource = Calendar::parse(stored.as_bytes()).unwrap();
    let inline = zone("").replace("+0200", "+0900");
    let query = query(&inline, "VEVENT", "20240102T070000Z", "20240102T073000Z");
    let engine = Engine::builder().zone_database(ConflictingZone).build();
    let mut session = engine.session();
    let projection = query.project(&mut session, &resource).unwrap();
    assert_eq!(
        projected_property(&projection, b"VEVENT", b"DTSTART"),
        "20240102T070000Z"
    );
    assert_eq!(
        query.matches(&mut session, &resource).unwrap(),
        Match::Matched
    );
}

#[test]
fn standalone_floating_duration_spans_the_query_zone_transition() {
    let timezone = zone("").replace("END:STANDARD\r\n", "END:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:20240331T020000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0300\r\nEND:DAYLIGHT\r\n");
    let query = query(&timezone, "VEVENT", "20240330T000000Z", "20240401T000000Z");
    let resource = calendar("VEVENT", "DTSTART:20240330T090000\r\nDURATION:P1D\r\n");
    let projection = query
        .project(&mut Engine::default().session(), &resource)
        .unwrap();
    assert_eq!(
        projected_property(&projection, b"VEVENT", b"DTSTART"),
        "20240330T070000Z"
    );
    assert_eq!(
        Duration::decode_value(projected_property(&projection, b"VEVENT", b"DURATION").as_bytes())
            .unwrap(),
        Duration::new(0, 23 * 3600)
    );
}

#[test]
fn the_zone_reader_does_not_interpret_opaque_recurrence_rules() {
    let source = zone("RRULE;VALUE=X-FUTURE:FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1\r\n");
    let document = Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut Vec::new(),
    );
    let table = zones.table("Test/Inline").unwrap();
    assert_eq!(table.rules().len(), 0);
    assert_eq!(table.observances().len(), 1);
    assert_eq!(document.to_bytes(), source.as_bytes());
}

#[test]
fn the_zone_reader_does_not_interpret_opaque_transition_dates() {
    let source = zone("RDATE;VALUE=X-FUTURE:20240101T000000\r\n");
    let document = Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut Vec::new(),
    );
    let table = zones.table("Test/Inline").unwrap();
    assert_eq!(table.observances().len(), 1);
    assert_eq!(document.to_bytes(), source.as_bytes());
}
