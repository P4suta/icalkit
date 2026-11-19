// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedded counts follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

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

fn rule(parts: Option<&str>) -> String {
    parts.map_or_else(String::new, |parts| {
        let text = format!("RRULE:FREQ=YEARLY;{parts}");
        assert!(text.is_ascii());
        let folded = text
            .as_bytes()
            .chunks(74)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join("\r\n ");
        format!("{folded}\r\n")
    })
}

fn timezone(
    daylight_start: &str,
    daylight_parts: &str,
    standard_start: &str,
    standard_parts: Option<&str>,
    explicit_dates: (&str, &str),
) -> String {
    let daylight = rule(Some(daylight_parts));
    let standard = rule(standard_parts);
    let (daylight_dates, standard_dates) = explicit_dates;
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule counts//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/RuleCounts\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{daylight_start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         {daylight}{daylight_dates}END:DAYLIGHT\r\nBEGIN:STANDARD\r\n\
         DTSTART:{standard_start}\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\n\
         {standard}{standard_dates}END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn calendar(start: &str) -> Calendar {
    Calendar::parse(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule counts//EN\r\nBEGIN:VEVENT\r\nUID:counts@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").as_bytes()).unwrap()
}

fn query(zone: &str, start: &str, end: &str) -> Query {
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
    .unwrap()
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
fn counted_transitions_stop_after_their_final_onset() {
    let zone = timezone(
        "20240310T020000",
        "COUNT=2;BYMONTH=3;BYDAY=2SU",
        "20241103T030000",
        Some("COUNT=2;BYMONTH=11;BYDAY=1SU"),
        ("", ""),
    );
    for (start, from, until, expected) in [
        (
            "20250308T090000",
            "20250308T000000Z",
            "20250311T000000Z",
            [
                "2025-03-08T09:00:00Z",
                "2025-03-09T08:00:00Z",
                "2025-03-10T08:00:00Z",
            ],
        ),
        (
            "20260307T090000",
            "20260307T000000Z",
            "20260310T000000Z",
            [
                "2026-03-07T09:00:00Z",
                "2026-03-08T09:00:00Z",
                "2026-03-09T09:00:00Z",
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
fn global_counts_follow_yearly_positions_across_months() {
    let zone = timezone(
        "20240303T020000",
        "COUNT=3;BYMONTH=3,9;BYDAY=SU;BYSETPOS=1,-1",
        "20240407T030000",
        Some("COUNT=3;BYMONTH=4,10;BYDAY=SU;BYSETPOS=1,-1"),
        ("", ""),
    );
    assert_eq!(
        starts(
            &zone,
            "20250301T090000",
            "20250301T000000Z",
            "20250304T000000Z"
        ),
        [
            "2025-03-01T09:00:00Z",
            "2025-03-02T08:00:00Z",
            "2025-03-03T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        starts(
            &zone,
            "20250927T090000",
            "20250927T000000Z",
            "20250930T000000Z"
        ),
        [
            "2025-09-27T09:00:00Z",
            "2025-09-28T09:00:00Z",
            "2025-09-29T09:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn explicit_dates_survive_singleton_counts() {
    let zone = timezone(
        "20240310T020000",
        "COUNT=1;BYMONTH=3;BYDAY=2SU",
        "20241103T030000",
        Some("COUNT=3;BYMONTH=11;BYDAY=1SU"),
        ("RDATE:20250310T020000\r\n", ""),
    );
    assert_eq!(
        starts(
            &zone,
            "20250308T090000",
            "20250308T000000Z",
            "20250311T000000Z"
        ),
        [
            "2025-03-08T09:00:00Z",
            "2025-03-09T09:00:00Z",
            "2025-03-10T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn sparse_centuries_consume_only_existing_aligned_dates() {
    let zone = timezone(
        "20000229T020000",
        "COUNT=2;INTERVAL=100;BYMONTH=2;BYMONTHDAY=29",
        "22000101T030000",
        None,
        ("", "RDATE:26000101T030000\r\n"),
    );
    for (year, expected) in [
        (
            2400,
            [
                "2400-02-28T09:00:00Z",
                "2400-02-29T08:00:00Z",
                "2400-03-01T08:00:00Z",
            ],
        ),
        (
            2800,
            [
                "2800-02-28T09:00:00Z",
                "2800-02-29T09:00:00Z",
                "2800-03-01T09:00:00Z",
            ],
        ),
    ] {
        assert_eq!(
            starts(
                &zone,
                &format!("{year}0228T090000"),
                &format!("{year}0228T000000Z"),
                &format!("{year}0302T000000Z")
            ),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn huge_counts_and_intervals_keep_observable_anchor_history() {
    let zone = timezone(
        "20240310T020000",
        "COUNT=4294967295;INTERVAL=4294967295;BYMONTH=3;BYDAY=2SU",
        "20241103T030000",
        Some("COUNT=4294967295;INTERVAL=4294967295;BYMONTH=11;BYDAY=1SU"),
        ("", ""),
    );
    for year in [2026, 9999] {
        let expected = [1, 2, 3].map(|day| {
            format!("{year}-06-{day:02}T09:00:00Z")
                .parse::<Timestamp>()
                .unwrap()
        });
        assert_eq!(
            starts(
                &zone,
                &format!("{year}0601T090000"),
                &format!("{year}0601T000000Z"),
                &format!("{year}0604T000000Z")
            ),
            expected
        );
    }
}

#[test]
fn padded_counts_drive_matching_and_expansion_without_external_zones() {
    let padding = "0".repeat(100);
    let zone = timezone(
        "20240310T020000",
        &format!("COUNT={padding}2;BYMONTH=3;BYDAY=2SU"),
        "20241103T030000",
        Some(&format!("COUNT={padding}2;BYMONTH=11;BYDAY=1SU")),
        ("", ""),
    );
    let calendar = calendar("20250309T090000");
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    for (from, until, expected) in [
        ("20250309T080000Z", "20250309T080001Z", Match::Matched),
        ("20250309T090000Z", "20250309T090001Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&zone, from, until)
                .matches(&mut engine.session(), &calendar)
                .unwrap(),
            expected
        );
    }
    assert_eq!(calendar.to_bytes(), original);
    assert_eq!(
        starts(
            &zone,
            "20250308T090000",
            "20250308T000000Z",
            "20250311T000000Z"
        ),
        [
            "2025-03-08T09:00:00Z",
            "2025-03-09T08:00:00Z",
            "2025-03-10T08:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}
