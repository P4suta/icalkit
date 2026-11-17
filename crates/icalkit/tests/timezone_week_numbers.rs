// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedded week numbering follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::caldav::{ProjectedCalendar, Query};
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

fn component(
    kind: &str,
    start: &str,
    offsets: (&str, &str),
    parts: Option<&str>,
    extra: &str,
) -> String {
    let (from, to) = offsets;
    let rule = parts.map_or_else(String::new, |parts| {
        let text = format!("RRULE:FREQ=YEARLY;{parts}");
        assert!(text.is_ascii());
        format!(
            "{}\r\n",
            text.as_bytes()
                .chunks(74)
                .map(|chunk| std::str::from_utf8(chunk).unwrap())
                .collect::<Vec<_>>()
                .join("\r\n ")
        )
    });
    format!(
        "BEGIN:{kind}\r\nDTSTART:{start}\r\nTZOFFSETFROM:{from}\r\nTZOFFSETTO:{to}\r\n{rule}{extra}END:{kind}\r\n"
    )
}

fn timezone(daylight: &str, standard: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit week numbers//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/WeekNumbers\r\n{daylight}{standard}END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn query(zone: &str, start: &str, end: &str) -> Result<Query, icalkit::Error> {
    Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{zone}]]></C:timezone></C:calendar-query>"#
        )
        .as_bytes(),
    )
}

fn projected(zone: &str, start: &str, from: &str, until: &str) -> ProjectedCalendar {
    let calendar = Calendar::parse(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit week numbers//EN\r\nBEGIN:VEVENT\r\nUID:weeks@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").as_bytes()).unwrap();
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let projected = query(zone, from, until)
        .unwrap()
        .project(&mut engine.session(), &calendar)
        .unwrap();
    assert_eq!(calendar.to_bytes(), original);
    projected
}

fn starts(zone: &str, start: &str, from: &str, until: &str) -> Vec<Timestamp> {
    let projected = projected(zone, start, from, until);
    Calendar::parse(projected.as_bytes())
        .unwrap_or_else(|error| {
            panic!(
                "{error:?}: {}",
                String::from_utf8_lossy(projected.as_bytes())
            )
        })
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

fn wire_starts(calendar: &ProjectedCalendar) -> Vec<String> {
    String::from_utf8_lossy(calendar.as_bytes())
        .lines()
        .filter_map(|line| line.strip_prefix("DTSTART:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn week_starts_place_transitions_before_the_next_calendar_year() {
    for (first, parts, reset, reset_parts, event, from, until, expected) in [
        (
            "20170101T020000",
            "BYWEEKNO=1;BYDAY=SU;WKST=SU",
            "20170108T030000",
            "BYWEEKNO=2;BYDAY=SU;WKST=SU",
            "20191228T090000",
            "20191228T000000Z",
            "20191231T000000Z",
            [
                "2019-12-28T09:00:00Z",
                "2019-12-29T08:00:00Z",
                "2019-12-30T08:00:00Z",
            ],
        ),
        (
            "20170102T020000",
            "BYWEEKNO=1;BYDAY=MO",
            "20170109T030000",
            "BYWEEKNO=2;BYDAY=MO",
            "20191229T090000",
            "20191229T000000Z",
            "20200101T000000Z",
            [
                "2019-12-29T09:00:00Z",
                "2019-12-30T08:00:00Z",
                "2019-12-31T08:00:00Z",
            ],
        ),
    ] {
        let zone = timezone(
            &component("DAYLIGHT", first, ("+0000", "+0100"), Some(parts), ""),
            &component("STANDARD", reset, ("+0100", "+0000"), Some(reset_parts), ""),
        );
        assert_eq!(
            starts(&zone, event, from, until),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn intervals_begin_in_the_source_week_numbering_year() {
    let zone = timezone(
        &component(
            "DAYLIGHT",
            "20191230T020000",
            ("+0000", "+0100"),
            Some("INTERVAL=2;BYWEEKNO=1;BYDAY=MO"),
            "",
        ),
        &component(
            "STANDARD",
            "20200106T030000",
            ("+0100", "+0000"),
            Some("INTERVAL=2;BYWEEKNO=2;BYDAY=MO"),
            "",
        ),
    );
    for (start, from, until, expected) in [
        (
            "20210103T090000",
            "20210103T000000Z",
            "20210106T000000Z",
            [
                "2021-01-03T09:00:00Z",
                "2021-01-04T09:00:00Z",
                "2021-01-05T09:00:00Z",
            ],
        ),
        (
            "20220102T090000",
            "20220102T000000Z",
            "20220105T000000Z",
            [
                "2022-01-02T09:00:00Z",
                "2022-01-03T08:00:00Z",
                "2022-01-04T08:00:00Z",
            ],
        ),
    ] {
        assert_eq!(
            starts(&zone, start, from, until),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn counts_apply_across_the_source_weeks_calendar_boundary() {
    let zone = timezone(
        &component(
            "DAYLIGHT",
            "20191230T020000",
            ("+0000", "+0100"),
            Some("BYWEEKNO=1;BYDAY=MO,TU;COUNT=3"),
            "",
        ),
        &component(
            "STANDARD",
            "20191231T010000",
            ("+0100", "+0000"),
            None,
            "RDATE:20200101T030000,20210105T010000\r\n",
        ),
    );
    assert_eq!(
        starts(
            &zone,
            "20210103T090000",
            "20210103T000000Z",
            "20210106T000000Z"
        ),
        [
            "2021-01-03T09:00:00Z",
            "2021-01-04T08:00:00Z",
            "2021-01-05T09:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn utc_ends_use_the_generating_years_interval_phase() {
    let standard = component(
        "STANDARD",
        "20200106T030000",
        ("+0100", "+0000"),
        Some("INTERVAL=2;BYWEEKNO=2;BYDAY=MO"),
        "",
    );
    let valid = timezone(
        &component(
            "DAYLIGHT",
            "20191230T020000",
            ("+0000", "+0100"),
            Some("INTERVAL=2;BYWEEKNO=1;BYDAY=MO;UNTIL=20251229T020000Z"),
            "",
        ),
        &standard,
    );
    assert_eq!(
        starts(
            &valid,
            "20251228T090000",
            "20251228T000000Z",
            "20251231T000000Z"
        ),
        [
            "2025-12-28T09:00:00Z",
            "2025-12-29T08:00:00Z",
            "2025-12-30T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
    let invalid = timezone(
        &component(
            "DAYLIGHT",
            "20191230T020000",
            ("+0000", "+0100"),
            Some("INTERVAL=2;BYWEEKNO=1;BYDAY=MO;UNTIL=20241230T020000Z"),
            "",
        ),
        &standard,
    );
    assert!(query(&invalid, "20241229T000000Z", "20250101T000000Z").is_err());
}

#[test]
fn week_fifty_three_distinguishes_matching_calendar_layouts() {
    let zone = timezone(
        &component(
            "DAYLIGHT",
            "20050101T020000",
            ("+0000", "+0100"),
            Some("BYWEEKNO=53;BYDAY=SA"),
            "",
        ),
        &component(
            "STANDARD",
            "20050103T030000",
            ("+0100", "+0000"),
            None,
            "RDATE:20100103T030000,20160103T030000,20210103T030000\r\n",
        ),
    );
    for (event, from, until, expected) in [
        (
            "20201231T090000",
            "20201231T000000Z",
            "20210103T000000Z",
            [
                "2020-12-31T09:00:00Z",
                "2021-01-01T09:00:00Z",
                "2021-01-02T08:00:00Z",
            ],
        ),
        (
            "20211231T090000",
            "20211231T000000Z",
            "20220103T000000Z",
            [
                "2021-12-31T09:00:00Z",
                "2022-01-01T09:00:00Z",
                "2022-01-02T09:00:00Z",
            ],
        ),
    ] {
        assert_eq!(
            starts(&zone, event, from, until),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn civil_clipping_precedes_positions_and_global_counts() {
    let standard = component(
        "STANDARD",
        "99990102T030000",
        ("+0100", "+0000"),
        None,
        "RDATE:99991231T010000\r\n",
    );
    for (count, last) in [(2_u32, "99991231T090000Z"), (3, "99991231T080000Z")] {
        let zone = timezone(
            &component(
                "DAYLIGHT",
                "99990101T020000",
                ("+0000", "+0100"),
                Some(&format!(
                    "BYWEEKNO=1;BYDAY=TH,FR,SA;BYSETPOS=2,-2;WKST=WE;COUNT={count}"
                )),
                "",
            ),
            &standard,
        );
        assert_eq!(
            wire_starts(&projected(
                &zone,
                "99991229T090000",
                "99991229T000000Z",
                "99991231T235959Z"
            )),
            ["99991229T090000Z", "99991230T080000Z", last]
        );
    }
}

#[test]
fn huge_controls_preserve_anchor_history_without_overflow() {
    let zone = timezone(
        &component(
            "DAYLIGHT",
            "20191230T020000",
            ("+0000", "+0100"),
            Some("BYWEEKNO=1;BYDAY=MO;INTERVAL=4294967295;COUNT=4294967295"),
            "",
        ),
        &component(
            "STANDARD",
            "20200106T030000",
            ("+0100", "+0000"),
            Some("BYWEEKNO=2;BYDAY=MO;INTERVAL=4294967295;COUNT=4294967295"),
            "",
        ),
    );
    assert_eq!(
        starts(
            &zone,
            "99990601T090000",
            "99990601T000000Z",
            "99990604T000000Z"
        ),
        [
            "9999-06-01T09:00:00Z",
            "9999-06-02T09:00:00Z",
            "9999-06-03T09:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}
