// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Duration placement follows [RFC 5545 section 3.3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.6) and [section 3.8.5.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3).
//! Query results follow [RFC 4791 sections 9.6.5 and 9.9](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.5).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};
use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Document, Limits, UtcOffset,
};

#[derive(Debug)]
struct DaylightZone;

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn with_offset(local: DateTime, offset: i32) -> Option<Timestamp> {
    let date = CivilDate::from_ymd(
        u16::try_from(local.year()).ok()?,
        u8::try_from(local.month()).ok()?,
        u8::try_from(local.day()).ok()?,
    )?;
    let time = CivilTime::from_hms(
        u8::try_from(local.hour()).ok()?,
        u8::try_from(local.minute()).ok()?,
        u8::try_from(local.second()).ok()?,
    )?;
    let instant = CivilDateTime::new(date, time).at_offset(UtcOffset::from_seconds(offset)?)?;
    Timestamp::new(instant.unix_seconds(), 0).ok()
}

impl ZoneDatabase for DaylightZone {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        if tzid != "Test/Daylight" || local.year() != 2024 {
            return None;
        }
        let point = (local.month(), local.day(), local.hour());
        if ((3, 10, 2)..(3, 10, 3)).contains(&point) {
            return Some(ZoneResolution::gap("test-daylight", true));
        }
        if ((11, 3, 1)..(11, 3, 2)).contains(&point) {
            return ZoneResolution::fold(
                with_offset(local, -4 * 3_600)?,
                with_offset(local, -5 * 3_600)?,
                "test-daylight",
                true,
            );
        }
        let offset = if ((3, 10, 3)..(11, 3, 1)).contains(&point) {
            -4 * 3_600
        } else {
            -5 * 3_600
        };
        Some(ZoneResolution::exact(
            with_offset(local, offset)?,
            "test-daylight",
            true,
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Timestamp) -> Option<OffsetResolution> {
        if tzid != "Test/Daylight" {
            return None;
        }
        let offset = if (at("2024-03-10T07:00:00Z")..at("2024-11-03T06:00:00Z")).contains(&instant)
        {
            -4 * 3_600
        } else {
            -5 * 3_600
        };
        OffsetResolution::new(offset, "test-daylight", true)
    }
}

fn event(properties: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {properties}END:VEVENT\r\n"
    )
}

fn calendar(components: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit durations//EN\r\n\
         {components}END:VCALENDAR\r\n"
    )
}

fn expanded_times(source: &str, start: &str, end: &str) -> Vec<(String, String)> {
    let query = Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#
        )
        .as_bytes(),
    )
    .unwrap();
    let source = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(DaylightZone).build();
    let mut session = engine.session();
    let projected = query.project(&mut session, &source).unwrap();
    Calendar::parse(projected.as_bytes()).unwrap();
    let document = Document::parse(projected.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let root = document.components().next().unwrap();
    root.components()
        .filter(|component| component.is_named(b"VEVENT"))
        .map(|component| {
            let value = |name| {
                let property = component
                    .properties()
                    .find(|property| property.is_named(name))
                    .unwrap();
                String::from_utf8(property.value_text().as_bytes().to_vec()).unwrap()
            };
            (value(b"DTSTART"), value(b"DTEND"))
        })
        .collect()
}

fn assert_times(source: &str, start: &str, end: &str, expected: &[(&str, &str)]) {
    let actual = expanded_times(source, start, end);
    let actual: Vec<(&str, &str)> = actual
        .iter()
        .map(|(start, end)| (start.as_str(), end.as_str()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn explicit_ends_keep_elapsed_lengths_across_daylight_changes() {
    for (start, end, window_start, window_end, expected) in [
        (
            "20240309T120000",
            "20240310T120000",
            "20240309T000000Z",
            "20240313T000000Z",
            [
                ("20240309T170000Z", "20240310T160000Z"),
                ("20240310T160000Z", "20240311T150000Z"),
                ("20240311T160000Z", "20240312T150000Z"),
            ],
        ),
        (
            "20241102T120000",
            "20241103T120000",
            "20241102T000000Z",
            "20241106T000000Z",
            [
                ("20241102T160000Z", "20241103T170000Z"),
                ("20241103T170000Z", "20241104T180000Z"),
                ("20241104T170000Z", "20241105T180000Z"),
            ],
        ),
    ] {
        let source = calendar(&event(&format!(
            "DTSTART;TZID=Test/Daylight:{start}\r\nDTEND;TZID=Test/Daylight:{end}\r\n\
             RRULE:FREQ=DAILY;COUNT=3\r\n"
        )));
        assert_times(&source, window_start, window_end, &expected);
    }
}

#[test]
fn day_durations_follow_calendar_days_across_daylight_changes() {
    let source = calendar(&event(
        "DTSTART;TZID=Test/Daylight:20240309T120000\r\nDURATION:P1D\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
    ));
    assert_times(
        &source,
        "20240309T000000Z",
        "20240313T000000Z",
        &[
            ("20240309T170000Z", "20240310T160000Z"),
            ("20240310T160000Z", "20240311T160000Z"),
            ("20240311T160000Z", "20240312T160000Z"),
        ],
    );
}

#[test]
fn time_durations_keep_elapsed_lengths_across_daylight_changes() {
    for (start, window_start, window_end, expected) in [
        (
            "20240309T120000",
            "20240309T000000Z",
            "20240313T000000Z",
            [
                ("20240309T170000Z", "20240310T170000Z"),
                ("20240310T160000Z", "20240311T160000Z"),
                ("20240311T160000Z", "20240312T160000Z"),
            ],
        ),
        (
            "20241102T120000",
            "20241102T000000Z",
            "20241106T000000Z",
            [
                ("20241102T160000Z", "20241103T160000Z"),
                ("20241103T170000Z", "20241104T170000Z"),
                ("20241104T170000Z", "20241105T170000Z"),
            ],
        ),
    ] {
        let source = calendar(&event(&format!(
            "DTSTART;TZID=Test/Daylight:{start}\r\nDURATION:PT24H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n"
        )));
        assert_times(&source, window_start, window_end, &expected);
    }
}

#[test]
fn mixed_durations_add_calendar_days_before_elapsed_time() {
    let source = calendar(&event(
        "DTSTART;TZID=Test/Daylight:20240309T013000\r\nDURATION:P1DT2H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
    ));
    assert_times(
        &source,
        "20240309T000000Z",
        "20240313T000000Z",
        &[
            ("20240309T063000Z", "20240310T083000Z"),
            ("20240310T063000Z", "20240311T073000Z"),
            ("20240311T053000Z", "20240312T073000Z"),
        ],
    );
}

#[test]
fn non_recurring_duration_filters_use_the_same_ordered_arithmetic() {
    let source = calendar(&event(
        "DTSTART;TZID=Test/Daylight:20240310T013000\r\nDURATION:PT2H\r\n",
    ));
    let query = Query::parse(
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240310T074500Z" end="20240310T080000Z"/>
</C:comp-filter></C:comp-filter></C:filter>
</C:calendar-query>"#,
    )
    .unwrap();
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(DaylightZone).build();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
}
