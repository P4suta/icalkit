// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Final observance onsets follow [RFC 5545 section 3.6.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.5).

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

fn component(
    kind: &str,
    start: &str,
    offsets: (&str, &str),
    parts: Option<&str>,
    extra: &str,
) -> String {
    let (from, to) = offsets;
    let rule = rule(parts);
    format!(
        "BEGIN:{kind}\r\nDTSTART:{start}\r\nTZOFFSETFROM:{from}\r\nTZOFFSETTO:{to}\r\n{rule}{extra}END:{kind}\r\n"
    )
}

fn zone(daylight_parts: &str, extra: &str) -> String {
    let daylight = component(
        "DAYLIGHT",
        "20240310T020000",
        ("-0500", "-0400"),
        Some(daylight_parts),
        extra,
    );
    let standard = component(
        "STANDARD",
        "20241103T020000",
        ("-0400", "-0500"),
        Some("BYMONTH=11;BYDAY=1SU"),
        "",
    );
    calendar_zone(&format!("{daylight}{standard}"))
}

fn calendar_zone(components: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit final onsets//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/UntilBounds\r\n{components}END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn calendar(start: &str) -> Calendar {
    Calendar::parse(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit final onsets//EN\r\nBEGIN:VEVENT\r\nUID:onsets@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").as_bytes()).unwrap()
}

fn request(zone: &str, start: &str, end: &str) -> String {
    format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{zone}]]></C:timezone></C:calendar-query>"#
    )
}

fn query(zone: &str, start: &str, end: &str) -> Query {
    Query::parse(request(zone, start, end).as_bytes()).unwrap()
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
fn unmatched_final_onsets_cannot_enter_query_plans() {
    for parts in [
        "BYMONTH=3;BYDAY=2SU;UNTIL=20250309T065959Z",
        "BYMONTH=3;BYDAY=2SU;UNTIL=20250309T070001Z",
        "BYMONTH=3;BYDAY=2SU;UNTIL=20250310T070000Z",
        "INTERVAL=2;BYMONTH=3;BYDAY=2SU;UNTIL=20250309T070000Z",
        "BYMONTH=3;BYDAY=2SU;UNTIL=20230312T070000Z",
        "BYMONTH=3;BYDAY=2SU;UNTIL=20250820T060000Z",
    ] {
        let zone = zone(parts, "RDATE:20250820T020000\r\n");
        let parsed =
            Query::parse(request(&zone, "20250308T000000Z", "20250311T000000Z").as_bytes());
        assert!(parsed.is_err(), "{parts}");
    }
}

#[test]
fn matching_final_onsets_keep_inclusive_projection() {
    for (until, expected) in [
        (
            "20250309T070000Z",
            [
                "2025-03-08T14:00:00Z",
                "2025-03-09T13:00:00Z",
                "2025-03-10T13:00:00Z",
            ],
        ),
        (
            "20240310T070000Z",
            [
                "2025-03-08T14:00:00Z",
                "2025-03-09T14:00:00Z",
                "2025-03-10T14:00:00Z",
            ],
        ),
    ] {
        let zone = zone(&format!("BYMONTH=3;BYDAY=2SU;UNTIL={until}"), "");
        assert_eq!(
            starts(
                &zone,
                "20250308T090000",
                "20250308T000000Z",
                "20250311T000000Z"
            ),
            expected.map(|value| value.parse::<Timestamp>().unwrap())
        );
    }
}

#[test]
fn matching_and_projection_use_the_same_final_onset() {
    let zone = zone("BYMONTH=3;BYDAY=2SU;UNTIL=20250309T070000Z", "");
    let calendar = calendar("20250308T090000");
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    for (from, until, expected) in [
        ("20250309T125500Z", "20250309T130500Z", Match::Matched),
        ("20250309T135500Z", "20250309T140500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&zone, from, until)
                .matches(&mut engine.session(), &calendar)
                .unwrap(),
            expected
        );
    }
    assert_eq!(calendar.to_bytes(), original);
}

#[test]
fn explicit_dates_survive_the_final_recurrence_onset() {
    let zone = zone(
        "INTERVAL=2;BYMONTH=3;BYDAY=2SU;UNTIL=20260308T070000Z",
        "RDATE:20270314T020000\r\n",
    );
    assert_eq!(
        starts(
            &zone,
            "20270313T090000",
            "20270313T000000Z",
            "20270316T000000Z"
        ),
        [
            "2027-03-13T14:00:00Z",
            "2027-03-14T13:00:00Z",
            "2027-03-15T13:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        starts(
            &zone,
            "20280311T090000",
            "20280311T000000Z",
            "20280314T000000Z"
        ),
        [
            "2028-03-11T14:00:00Z",
            "2028-03-12T14:00:00Z",
            "2028-03-13T14:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}

#[test]
fn utc_ends_keep_their_local_onset_across_new_year() {
    let daylight = component(
        "DAYLIGHT",
        "20240101T000000",
        ("+0200", "+0300"),
        Some("INTERVAL=2;BYMONTH=1;BYMONTHDAY=1;UNTIL=20251231T220000Z"),
        "",
    );
    let standard = component(
        "STANDARD",
        "20240102T000000",
        ("+0300", "+0200"),
        None,
        "RDATE:20260102T000000\r\n",
    );
    let zone = calendar_zone(&format!("{daylight}{standard}"));
    assert_eq!(
        starts(
            &zone,
            "20251231T090000",
            "20251231T000000Z",
            "20260103T000000Z"
        ),
        [
            "2025-12-31T07:00:00Z",
            "2026-01-01T06:00:00Z",
            "2026-01-02T07:00:00Z"
        ]
        .map(|value| value.parse::<Timestamp>().unwrap())
    );
}
