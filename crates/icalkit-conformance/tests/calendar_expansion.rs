// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Expanded results follow [RFC 4791 section 9.6.5](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.5).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};
use icalkit_conformance::internal::core::{Component, Document, Limits};

const TIMEZONE: &str = concat!(
    "BEGIN:VTIMEZONE\r\nTZID:Test/TwoHours\r\nBEGIN:STANDARD\r\n",
    "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0200\r\n",
    "END:STANDARD\r\nEND:VTIMEZONE\r\n",
);

struct QueryZones;

impl ZoneDatabase for QueryZones {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        let offset = match tzid {
            "Test/TwoHours" => "+02:00",
            "Test/Spring" => match (local.year(), local.month(), local.day(), local.hour()) {
                (2024, 3, 9, _) => "-05:00",
                (2024, 3, 10, 3..=23) => "-04:00",
                _ => return None,
            },
            _ => return None,
        };
        let instant = format!("{local}{offset}").parse::<Timestamp>().ok()?;
        Some(ZoneResolution::exact(instant, "test-query-zone", true))
    }

    fn offset_at(&self, tzid: &str, instant: Timestamp) -> Option<OffsetResolution> {
        if tzid == "Test/Spring" {
            let transition = "2024-03-10T07:00:00Z".parse::<Timestamp>().ok()?;
            let seconds = if instant < transition {
                -18_000
            } else {
                -14_400
            };
            return OffsetResolution::new(seconds, "test-query-zone", true);
        }
        (tzid == "Test/TwoHours")
            .then(|| OffsetResolution::new(7_200, "test-two-hours", true))
            .flatten()
    }
}

fn component(kind: &str, properties: &str) -> String {
    format!(
        "BEGIN:{kind}\r\nUID:expansion@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {properties}END:{kind}\r\n"
    )
}

fn calendar(components: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit expansion//EN\r\n\
         {components}END:VCALENDAR\r\n"
    )
}

fn project(source: &str, start: &str, end: &str) -> Result<Vec<u8>, icalkit::Error> {
    let query = Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#
        )
        .as_bytes(),
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(QueryZones).build();
    let mut session = engine.session();
    let projected = query.project(&mut session, &original);
    assert_eq!(original.to_bytes(), source.as_bytes());
    projected.map(|projected| projected.as_bytes().to_vec())
}

fn document(bytes: &[u8]) -> Document {
    let document = Document::parse(bytes, Limits::DEFAULT, &mut Vec::new()).unwrap();
    if document
        .components()
        .next()
        .unwrap()
        .components()
        .next()
        .is_some()
    {
        Calendar::parse(bytes).unwrap();
    }
    document
}

fn text<'a>(component: &'a Component, name: &[u8]) -> Option<&'a str> {
    let property = component
        .properties()
        .find(|property| property.is_named(name))?;
    std::str::from_utf8(property.value_text().as_bytes()).ok()
}

fn parameter_text<'a>(
    component: &'a Component,
    property_name: &[u8],
    name: &[u8],
) -> Option<&'a str> {
    let property = component
        .properties()
        .find(|property| property.is_named(property_name))?;
    let parameter = property
        .parameters()
        .iter()
        .find(|parameter| parameter.is_named(name))?;
    std::str::from_utf8(parameter.value().as_bytes()).ok()
}

#[test]
fn single_events_outside_the_expansion_window_are_omitted() {
    let source = calendar(&component("VEVENT", "DTSTART:20240101T090000Z\r\n"));
    let bytes = project(&source, "20240102T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    assert_eq!(
        document.components().next().unwrap().components().count(),
        0
    );
}

#[test]
fn single_zoned_events_use_utc_and_keep_unknown_parameters() {
    let source = calendar(&format!(
        "{TIMEZONE}{}",
        component(
            "VEVENT",
            "dtstart;tzid=Test/TwoHours;X-META=kept:20240102T090000\r\n\
             DTEND;TZID=Test/TwoHours:20240102T100000\r\n",
        ),
    ));
    let bytes = project(&source, "20240102T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    let event = document
        .components()
        .next()
        .unwrap()
        .components()
        .find(|c| c.is_named(b"VEVENT"))
        .unwrap();
    assert_eq!(text(event, b"DTSTART"), Some("20240102T070000Z"));
    assert_eq!(text(event, b"DTEND"), Some("20240102T080000Z"));
    let start = event.properties().find(|p| p.is_named(b"DTSTART")).unwrap();
    assert_eq!(start.name().as_bytes(), b"dtstart");
    assert!(start.parameters_named(b"TZID").next().is_none());
    assert_eq!(
        start
            .parameters_named(b"X-META")
            .next()
            .unwrap()
            .value()
            .as_bytes(),
        b"kept"
    );
}

#[test]
fn expanded_calendars_omit_time_zone_components() {
    let source = calendar(&format!(
        "{TIMEZONE}{}",
        component(
            "VEVENT",
            "DTSTART;TZID=Test/TwoHours:20240101T090000\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
        ),
    ));
    let bytes = project(&source, "20240101T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    assert!(
        root.components()
            .all(|component| component.is_named(b"VEVENT"))
    );
}

#[test]
fn detached_instances_are_filtered_without_losing_their_identity() {
    let source = calendar(&component(
        "VEVENT",
        "RECURRENCE-ID:20240101T090000Z\r\nDTSTART:20240102T110000Z\r\nDURATION:PT1H\r\n",
    ));
    let bytes = project(&source, "20240102T110000Z", "20240102T120000Z").unwrap();
    let kept = document(&bytes);
    let event = kept
        .components()
        .next()
        .unwrap()
        .components()
        .next()
        .unwrap();
    assert_eq!(text(event, b"RECURRENCE-ID"), Some("20240101T090000Z"));
    let bytes = project(&source, "20240101T080000Z", "20240101T100000Z").unwrap();
    assert_eq!(
        document(&bytes)
            .components()
            .next()
            .unwrap()
            .components()
            .count(),
        0
    );
}

#[test]
fn tasks_without_a_start_keep_their_due_form_in_utc() {
    let source = calendar(&format!(
        "{TIMEZONE}{}",
        component("VTODO", "DUE;TZID=Test/TwoHours:20240102T090000\r\n"),
    ));
    let bytes = project(&source, "20240102T060000Z", "20240102T080000Z").unwrap();
    let document = document(&bytes);
    let task = document
        .components()
        .next()
        .unwrap()
        .components()
        .find(|c| c.is_named(b"VTODO"))
        .unwrap();
    assert_eq!(text(task, b"DUE"), Some("20240102T070000Z"));
    assert!(text(task, b"DTSTART").is_none());
    assert!(text(task, b"DTEND").is_none());
}

#[test]
fn zoned_extension_values_in_nested_components_use_utc() {
    let source = calendar(&format!(
        "{TIMEZONE}{}",
        component(
            "VEVENT",
            "DTSTART:20240101T090000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nDESCRIPTION:Reminder\r\n\
             X-AT;VALUE=DATE-TIME;TZID=Test/TwoHours;X-META=kept:20240102T090000\r\n\
             END:VALARM\r\n",
        ),
    ));
    let bytes = project(&source, "20240101T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    for event in document
        .components()
        .next()
        .unwrap()
        .components()
        .filter(|c| c.is_named(b"VEVENT"))
    {
        let alarm = event.components().next().unwrap();
        assert_eq!(text(alarm, b"X-AT"), Some("20240102T070000Z"));
        let property = alarm.properties().find(|p| p.is_named(b"X-AT")).unwrap();
        assert!(property.parameters_named(b"TZID").next().is_none());
        assert_eq!(
            property
                .parameters_named(b"X-META")
                .next()
                .unwrap()
                .value()
                .as_bytes(),
            b"kept"
        );
    }
}

#[test]
fn expansion_removes_obsolete_exception_rules() {
    for recurrence in ["", "RRULE:FREQ=DAILY;COUNT=2\r\n"] {
        let source = calendar(&component(
            "VEVENT",
            &format!("DTSTART:20240101T090000Z\r\n{recurrence}EXRULE:FREQ=WEEKLY\r\n"),
        ));
        let bytes = project(&source, "20240101T000000Z", "20240103T000000Z").unwrap();
        let document = document(&bytes);
        let root = document.components().next().unwrap();
        assert!(
            root.components()
                .all(|event| text(event, b"EXRULE").is_none())
        );
    }
}

#[test]
fn opaque_zoned_values_fail_expansion_without_interpretation() {
    for recurrence in ["", "RRULE:FREQ=DAILY;COUNT=2\r\n"] {
        let source = calendar(&format!(
            "{TIMEZONE}{}",
            component(
                "VEVENT",
                &format!(
                    "DTSTART:20240101T090000Z\r\n{recurrence}\
                 X-AT;VALUE=X-FUTURE;TZID=Test/TwoHours:20240102T090000\r\n",
                )
            ),
        ));
        assert!(project(&source, "20240101T000000Z", "20240103T000000Z").is_err());
    }
}

#[test]
fn an_excluded_start_does_not_require_a_rule_to_disappear() {
    let source = calendar(&component(
        "VEVENT",
        "DTSTART:20240101T090000Z\r\nEXDATE:20240101T090000Z\r\n",
    ));
    let bytes = project(&source, "20240101T000000Z", "20240102T000000Z").unwrap();
    assert_eq!(
        document(&bytes)
            .components()
            .next()
            .unwrap()
            .components()
            .count(),
        0
    );
    let query = Query::parse(
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240101T000000Z" end="20240102T000000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &original).unwrap(),
        Match::Unmatched
    );
}

#[test]
fn zone_parameters_do_not_change_text_or_uri_values_into_date_times() {
    for declared_type in ["VALUE=TEXT;", "VALUE=URI;", ""] {
        let source = calendar(&format!(
            "{TIMEZONE}{}",
            component(
                "VEVENT",
                &format!(
                    "DTSTART:20240101T090000Z\r\n\
                 X-AT;{declared_type}TZID=Test/TwoHours:20240102T090000\r\n",
                )
            ),
        ));
        assert!(project(&source, "20240101T000000Z", "20240103T000000Z").is_err());
    }
}

#[test]
fn single_zoned_durations_keep_their_placed_end_across_a_clock_change() {
    let timezone = concat!(
        "BEGIN:VTIMEZONE\r\nTZID:Test/Spring\r\n",
        "BEGIN:STANDARD\r\nDTSTART:20231105T020000\r\n",
        "TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:20240310T020000\r\n",
        "TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nEND:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
    );
    for kind in ["VEVENT", "VTODO"] {
        let source = calendar(&format!(
            "{timezone}{}",
            component(
                kind,
                "DTSTART;TZID=Test/Spring:20240309T120000\r\nDURATION:P1D\r\n"
            ),
        ));
        let original = Calendar::parse(source.as_bytes()).unwrap();
        let bytes = project(&source, "20240309T000000Z", "20240311T000000Z").unwrap();
        let projected = Calendar::parse(&bytes).unwrap();
        let query = Query::parse(
            format!(
                r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="{kind}">
<C:time-range start="20240310T163000Z" end="20240310T170000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
            )
            .as_bytes(),
        )
        .unwrap();
        let engine = Engine::builder().zone_database(QueryZones).build();
        let mut session = engine.session();
        assert_eq!(
            query.matches(&mut session, &original).unwrap(),
            Match::Unmatched
        );
        assert_eq!(
            query.matches(&mut session, &projected).unwrap(),
            Match::Unmatched
        );
    }
}

#[test]
fn recurring_date_time_properties_keep_extension_parameters() {
    let source = calendar(&format!(
        "{TIMEZONE}{}",
        component(
            "VEVENT",
            concat!(
                "dtstart;tzid=Test/TwoHours;X-META=start:20240101T090000\r\n",
                "DTEND;TZID=Test/TwoHours;X-META=end:20240101T100000\r\n",
                "RRULE:FREQ=DAILY;COUNT=2\r\n",
            )
        ),
    ));
    let bytes = project(&source, "20240101T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    for event in root.components() {
        assert_eq!(parameter_text(event, b"DTSTART", b"X-META"), Some("start"));
        assert_eq!(parameter_text(event, b"DTEND", b"X-META"), Some("end"));
        assert!(parameter_text(event, b"DTSTART", b"TZID").is_none());
        assert!(parameter_text(event, b"DTEND", b"TZID").is_none());
    }
}

#[test]
fn range_and_exact_templates_keep_their_own_temporal_parameters() {
    let source = calendar(&format!(
        "{}{}{}",
        component(
            "VEVENT",
            concat!(
                "DTSTART;X-META=base:20240101T090000Z\r\n",
                "DTEND;X-META=base:20240101T100000Z\r\nRRULE:FREQ=DAILY;COUNT=4\r\n",
            )
        ),
        component(
            "VEVENT",
            concat!(
                "RECURRENCE-ID;RANGE=THISANDFUTURE;X-META=range:20240102T090000Z\r\n",
                "DTSTART;X-META=range:20240102T110000Z\r\n",
                "DTEND;X-META=range:20240102T120000Z\r\n",
            )
        ),
        component(
            "VEVENT",
            concat!(
                "RECURRENCE-ID;X-META=exact:20240103T090000Z\r\n",
                "DTSTART;X-META=exact:20240103T150000Z\r\n",
                "DTEND;X-META=exact:20240103T160000Z\r\n",
            )
        ),
    ));
    let bytes = project(&source, "20240101T000000Z", "20240105T000000Z").unwrap();
    let document = document(&bytes);
    let root = document.components().next().unwrap();
    let expected = ["base", "range", "exact", "range"];
    assert_eq!(root.components().count(), expected.len());
    for (event, expected) in root.components().zip(expected) {
        assert_eq!(parameter_text(event, b"DTSTART", b"X-META"), Some(expected));
        assert_eq!(parameter_text(event, b"DTEND", b"X-META"), Some(expected));
        if text(event, b"RECURRENCE-ID").is_some() {
            assert_eq!(
                parameter_text(event, b"RECURRENCE-ID", b"X-META"),
                Some(expected)
            );
            assert!(parameter_text(event, b"RECURRENCE-ID", b"RANGE").is_none());
        }
    }
}

#[test]
fn recurring_task_durations_keep_extension_parameters() {
    let source = calendar(&component(
        "VTODO",
        concat!(
            "DTSTART:20240101T090000Z\r\nDURATION;X-META=kept:PT1H\r\n",
            "RRULE:FREQ=DAILY;COUNT=2\r\n",
        ),
    ));
    let bytes = project(&source, "20240101T000000Z", "20240103T000000Z").unwrap();
    let document = document(&bytes);
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    for task in root.components() {
        assert_eq!(parameter_text(task, b"DURATION", b"X-META"), Some("kept"));
    }
}

#[test]
fn detached_instances_match_property_filters_without_a_master() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        let source = calendar(&component(
            kind,
            concat!(
                "RECURRENCE-ID:20240101T090000Z\r\nDTSTART:20240102T110000Z\r\n",
                "SUMMARY:Detached\r\n",
            ),
        ));
        let query = Query::parse(format!(r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="{kind}">
<C:time-range start="20240102T110000Z" end="20240102T120000Z"/>
<C:prop-filter name="SUMMARY"><C:text-match collation="i;ascii-casemap">Detached</C:text-match></C:prop-filter>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#).as_bytes()).unwrap();
        let original = Calendar::parse(source.as_bytes()).unwrap();
        let engine = Engine::default();
        let mut session = engine.session();
        assert_eq!(
            query.matches(&mut session, &original).unwrap(),
            Match::Matched,
            "{kind}"
        );
    }
}
