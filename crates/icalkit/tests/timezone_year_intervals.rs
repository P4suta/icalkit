// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedded year intervals follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine};

struct NoExternalZones;

impl ZoneDatabase for NoExternalZones {
    fn resolve_local(&self, _tzid: &str, _local: DateTime) -> Option<ZoneResolution> {
        None
    }

    fn offset_at(&self, _tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        None
    }
}

fn fold_line(text: &str) -> String {
    assert!(text.is_ascii());
    text.as_bytes()
        .chunks(74)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n ")
}

fn rule(parts: Option<&str>) -> String {
    parts.map_or_else(String::new, |parts| {
        format!("{}\r\n", fold_line(&format!("RRULE:FREQ=YEARLY;{parts}")))
    })
}

fn timezone(
    daylight_start: &str,
    daylight_parts: &str,
    standard_start: &str,
    standard_parts: Option<&str>,
    explicit_dates: &str,
) -> String {
    let daylight = rule(Some(daylight_parts));
    let standard = rule(standard_parts);
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit year intervals//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/YearIntervals\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{daylight_start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         {daylight}{explicit_dates}END:DAYLIGHT\r\nBEGIN:STANDARD\r\n\
         DTSTART:{standard_start}\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\n\
         {standard}END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn biennial(interval: &str) -> String {
    timezone(
        "20240310T020000",
        &format!("INTERVAL={interval};BYMONTH=3;BYDAY=2SU"),
        "20241103T030000",
        Some(&format!("INTERVAL={interval};BYMONTH=11;BYDAY=1SU")),
        "",
    )
}

fn calendar(start: &str) -> Calendar {
    Calendar::parse(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit year intervals//EN\r\nBEGIN:VEVENT\r\nUID:intervals@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").as_bytes()).unwrap()
}

fn query(zone: &str, start: &str, end: &str) -> Query {
    let text = format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{zone}]]></C:timezone></C:calendar-query>"#
    );
    Query::parse(text.as_bytes()).unwrap()
}

fn starts(zone: &str, start: &str, from: &str, until: &str) -> Vec<Timestamp> {
    let calendar = calendar(start);
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let projected = query(zone, from, until)
        .project(&mut engine.session(), &calendar)
        .unwrap();
    assert_eq!(calendar.to_bytes(), original);
    Calendar::parse(projected.as_bytes())
        .unwrap()
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

#[test]
fn biennial_transitions_apply_only_in_aligned_years() {
    for (start, from, until, expected) in [
        (
            "20250308T090000",
            "20250308T000000Z",
            "20250311T000000Z",
            [
                "2025-03-08T09:00:00Z",
                "2025-03-09T09:00:00Z",
                "2025-03-10T09:00:00Z",
            ],
        ),
        (
            "20260307T090000",
            "20260307T000000Z",
            "20260310T000000Z",
            [
                "2026-03-07T09:00:00Z",
                "2026-03-08T08:00:00Z",
                "2026-03-09T08:00:00Z",
            ],
        ),
    ] {
        assert_eq!(
            starts(&biennial("2"), start, from, until),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn set_positions_keep_their_year_scope_with_larger_intervals() {
    let zone = timezone(
        "20240303T020000",
        "INTERVAL=2;BYMONTH=3,9;BYDAY=SU;BYSETPOS=1,-1",
        "20240407T030000",
        Some("INTERVAL=2;BYMONTH=4,10;BYDAY=SU;BYSETPOS=1,-1"),
        "",
    );
    assert_eq!(
        starts(
            &zone,
            "20260926T090000",
            "20260926T000000Z",
            "20260929T000000Z"
        ),
        [
            "2026-09-26T09:00:00Z",
            "2026-09-27T08:00:00Z",
            "2026-09-28T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn sparse_centuries_keep_the_latest_transition_across_long_gaps() {
    for (reset, reset_rule, hour) in [
        (
            "22000101T030000",
            "INTERVAL=400;BYMONTH=1;BYMONTHDAY=1",
            8_u8,
        ),
        ("20000301T030000", "INTERVAL=400;BYMONTH=3;BYMONTHDAY=1", 9),
    ] {
        let zone = timezone(
            "20000229T020000",
            "INTERVAL=100;BYMONTH=2;BYMONTHDAY=29",
            reset,
            Some(reset_rule),
            "",
        );
        let values = starts(
            &zone,
            "24990601T090000",
            "24990601T000000Z",
            "24990604T000000Z",
        );
        assert_eq!(
            values,
            [
                format!("2499-06-01T{hour:02}:00:00Z"),
                format!("2499-06-02T{hour:02}:00:00Z"),
                format!("2499-06-03T{hour:02}:00:00Z")
            ]
            .map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn query_windows_use_the_same_interval_phase() {
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    for (start, from, until, expected) in [
        (
            "20250309T090000",
            "20250309T085500Z",
            "20250309T090500Z",
            Match::Matched,
        ),
        (
            "20250309T090000",
            "20250309T075500Z",
            "20250309T080500Z",
            Match::Unmatched,
        ),
        (
            "20260308T090000",
            "20260308T075500Z",
            "20260308T080500Z",
            Match::Matched,
        ),
        (
            "20260308T090000",
            "20260308T085500Z",
            "20260308T090500Z",
            Match::Unmatched,
        ),
    ] {
        assert_eq!(
            query(&biennial("0002"), from, until)
                .matches(&mut engine.session(), &calendar(start))
                .unwrap(),
            expected
        );
    }
}

#[test]
fn huge_intervals_keep_original_anchors_without_wrapping() {
    assert_eq!(
        starts(
            &biennial("4294967295"),
            "20240309T090000",
            "20240309T000000Z",
            "20240312T000000Z"
        ),
        [
            "2024-03-09T09:00:00Z",
            "2024-03-10T08:00:00Z",
            "2024-03-11T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        starts(
            &biennial("4294967295"),
            "20250308T090000",
            "20250308T000000Z",
            "20250311T000000Z"
        ),
        [
            "2025-03-08T09:00:00Z",
            "2025-03-09T09:00:00Z",
            "2025-03-10T09:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn explicit_dates_do_not_inherit_the_rule_interval() {
    let zone = timezone(
        "20000229T020000",
        "INTERVAL=100;BYMONTH=2;BYMONTHDAY=29",
        "20250101T030000",
        None,
        "RDATE:20250701T020000\r\n",
    );
    assert_eq!(
        starts(
            &zone,
            "20250630T090000",
            "20250630T000000Z",
            "20250703T000000Z"
        ),
        [
            "2025-06-30T09:00:00Z",
            "2025-07-01T08:00:00Z",
            "2025-07-02T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}
