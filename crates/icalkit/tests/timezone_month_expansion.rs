// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Month expansion follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Queries select their source through [RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8).

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

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn calendar(start: &str) -> Calendar {
    Calendar::parse(
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month expansion//EN\r\n\
         BEGIN:VEVENT\r\nUID:months@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:{start}\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
        .as_bytes(),
    )
    .unwrap()
}

fn timezone(
    daylight_start: &str,
    daylight_parts: &str,
    standard_start: &str,
    standard_parts: &str,
) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit month expansion//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Months\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{daylight_start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{daylight_parts}\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\n\
         DTSTART:{standard_start}\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\n\
         RRULE:FREQ=YEARLY;{standard_parts}\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
}

fn alternating_month_days() -> String {
    timezone(
        "20240101T020000",
        "BYMONTHDAY=1",
        "20240115T030000",
        "BYMONTHDAY=15",
    )
}

fn query(timezone: &str, start: &str, end: &str) -> Query {
    let text = format!(
        r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>
</C:comp-filter></C:comp-filter></C:filter>
<C:timezone><![CDATA[{timezone}]]></C:timezone></C:calendar-query>"#
    );
    Query::parse(text.as_bytes()).unwrap()
}

fn starts(timezone: &str, start: &str, from: &str, until: &str) -> Vec<Timestamp> {
    let calendar = calendar(start);
    let original = calendar.to_bytes();
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let projection = query(timezone, from, until)
        .project(&mut engine.session(), &calendar)
        .unwrap();
    assert_eq!(calendar.to_bytes(), original);
    Calendar::parse(projection.as_bytes())
        .unwrap()
        .events()
        .map(|event| event.dtstart().unwrap().as_utc().unwrap())
        .collect()
}

#[test]
fn month_day_rules_apply_the_june_transition() {
    assert_eq!(
        starts(
            &alternating_month_days(),
            "20240531T090000",
            "20240531T000000Z",
            "20240603T000000Z"
        ),
        [
            at("2024-05-31T09:00:00Z"),
            at("2024-06-01T08:00:00Z"),
            at("2024-06-02T08:00:00Z")
        ]
    );
}

#[test]
fn month_day_rules_apply_the_july_reverse_transition() {
    assert_eq!(
        starts(
            &alternating_month_days(),
            "20240714T090000",
            "20240714T000000Z",
            "20240717T000000Z"
        ),
        [
            at("2024-07-14T08:00:00Z"),
            at("2024-07-15T09:00:00Z"),
            at("2024-07-16T09:00:00Z")
        ]
    );
}

#[test]
fn queries_use_months_after_the_anchor_month() {
    let calendar = calendar("20240601T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("20240601T075500Z", "20240601T080500Z", Match::Matched),
        ("20240601T085500Z", "20240601T090500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&alternating_month_days(), start, end)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn weekday_windows_apply_in_each_month() {
    let timezone = timezone(
        "20240107T020000",
        "BYDAY=SU;BYMONTHDAY=1,2,3,4,5,6,7",
        "20240128T030000",
        "BYDAY=SU;BYMONTHDAY=-7,-6,-5,-4,-3,-2,-1",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240601T090000",
            "20240601T000000Z",
            "20240604T000000Z"
        ),
        [
            at("2024-06-01T09:00:00Z"),
            at("2024-06-02T08:00:00Z"),
            at("2024-06-03T08:00:00Z")
        ]
    );
}

fn negative_annual_zone() -> String {
    timezone(
        "20240228T020000",
        "BYMONTH=2;BYMONTHDAY=-2",
        "20241030T030000",
        "BYMONTH=10;BYMONTHDAY=-2",
    )
}

#[test]
fn negative_month_days_place_the_nonleap_february_transition() {
    assert_eq!(
        starts(
            &negative_annual_zone(),
            "20250226T090000",
            "20250226T000000Z",
            "20250301T000000Z"
        ),
        [
            at("2025-02-26T09:00:00Z"),
            at("2025-02-27T08:00:00Z"),
            at("2025-02-28T08:00:00Z")
        ]
    );
}

#[test]
fn unrestricted_negative_month_days_apply_in_each_month() {
    let timezone = timezone(
        "20240130T020000",
        "BYMONTHDAY=-2",
        "20240115T030000",
        "BYMONTHDAY=15",
    );
    assert_eq!(
        starts(
            &timezone,
            "20250428T090000",
            "20250428T000000Z",
            "20250501T000000Z"
        ),
        [
            at("2025-04-28T09:00:00Z"),
            at("2025-04-29T08:00:00Z"),
            at("2025-04-30T08:00:00Z")
        ]
    );
}

#[test]
fn queries_follow_negative_month_day_transitions() {
    let calendar = calendar("20250226T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("20250227T075500Z", "20250227T080500Z", Match::Matched),
        ("20250227T085500Z", "20250227T090500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&negative_annual_zone(), start, end)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}

fn selected_month_zone() -> String {
    timezone(
        "20240301T020000",
        "BYMONTH=3,9;BYMONTHDAY=1",
        "20240601T030000",
        "BYMONTH=6,12;BYMONTHDAY=1",
    )
}

#[test]
fn selected_months_apply_the_second_daylight_transition() {
    assert_eq!(
        starts(
            &selected_month_zone(),
            "20240831T090000",
            "20240831T000000Z",
            "20240903T000000Z"
        ),
        [
            at("2024-08-31T09:00:00Z"),
            at("2024-09-01T08:00:00Z"),
            at("2024-09-02T08:00:00Z")
        ]
    );
}

#[test]
fn selected_months_apply_the_second_standard_transition() {
    assert_eq!(
        starts(
            &selected_month_zone(),
            "20241130T090000",
            "20241130T000000Z",
            "20241203T000000Z"
        ),
        [
            at("2024-11-30T08:00:00Z"),
            at("2024-12-01T09:00:00Z"),
            at("2024-12-02T09:00:00Z")
        ]
    );
}

#[test]
fn queries_use_transitions_in_later_selected_months() {
    let calendar = calendar("20240901T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("20240901T075500Z", "20240901T080500Z", Match::Matched),
        ("20240901T085500Z", "20240901T090500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&selected_month_zone(), start, end)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn selected_months_apply_weekday_ordinals_within_each_month() {
    let timezone = timezone(
        "20240331T020000",
        "BYMONTH=3,9;BYDAY=-1SU",
        "20240428T030000",
        "BYMONTH=4,10;BYDAY=-1SU",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240928T090000",
            "20240928T000000Z",
            "20241001T000000Z"
        ),
        [
            at("2024-09-28T09:00:00Z"),
            at("2024-09-29T08:00:00Z"),
            at("2024-09-30T08:00:00Z")
        ]
    );
}

fn alternating_day_sets() -> String {
    timezone(
        "20240101T020000",
        "BYMONTHDAY=1,15",
        "20240108T030000",
        "BYMONTHDAY=8,22",
    )
}

#[test]
fn month_day_sets_apply_the_second_daylight_transition() {
    assert_eq!(
        starts(
            &alternating_day_sets(),
            "20240614T090000",
            "20240614T000000Z",
            "20240617T000000Z"
        ),
        [
            at("2024-06-14T09:00:00Z"),
            at("2024-06-15T08:00:00Z"),
            at("2024-06-16T08:00:00Z")
        ]
    );
}

#[test]
fn month_day_sets_apply_the_second_standard_transition() {
    assert_eq!(
        starts(
            &alternating_day_sets(),
            "20240621T090000",
            "20240621T000000Z",
            "20240624T000000Z"
        ),
        [
            at("2024-06-21T08:00:00Z"),
            at("2024-06-22T09:00:00Z"),
            at("2024-06-23T09:00:00Z")
        ]
    );
}

#[test]
fn queries_use_later_dates_in_month_day_sets() {
    let calendar = calendar("20240615T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("20240615T075500Z", "20240615T080500Z", Match::Matched),
        ("20240615T085500Z", "20240615T090500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&alternating_day_sets(), start, end)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn month_day_sets_intersect_weekdays_in_negative_windows() {
    let timezone = timezone(
        "20240324T020000",
        "BYMONTH=3,9;BYMONTHDAY=-14,-13,-12,-11,-10,-9,-8;BYDAY=SU",
        "20240428T030000",
        "BYMONTH=4,10;BYDAY=-1SU",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240921T090000",
            "20240921T000000Z",
            "20240924T000000Z"
        ),
        [
            at("2024-09-21T09:00:00Z"),
            at("2024-09-22T08:00:00Z"),
            at("2024-09-23T08:00:00Z")
        ]
    );
}

#[test]
fn bare_weekday_lists_apply_each_stated_weekday() {
    let timezone = timezone(
        "20240101T020000",
        "BYDAY=MO,TH",
        "20240102T030000",
        "BYDAY=TU,FR",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240605T090000",
            "20240605T000000Z",
            "20240608T000000Z"
        ),
        [
            at("2024-06-05T09:00:00Z"),
            at("2024-06-06T08:00:00Z"),
            at("2024-06-07T09:00:00Z")
        ]
    );
}

fn alternating_weekday_ordinals() -> String {
    timezone(
        "20240107T020000",
        "BYMONTH=1,2,3,4,5,6,7,8,9,10,11,12;BYDAY=1SU,3SU",
        "20240114T030000",
        "BYMONTH=1,2,3,4,5,6,7,8,9,10,11,12;BYDAY=2SU,4SU",
    )
}

#[test]
fn ordinal_weekday_lists_apply_later_month_transitions() {
    assert_eq!(
        starts(
            &alternating_weekday_ordinals(),
            "20240615T090000",
            "20240615T000000Z",
            "20240618T000000Z"
        ),
        [
            at("2024-06-15T09:00:00Z"),
            at("2024-06-16T08:00:00Z"),
            at("2024-06-17T08:00:00Z")
        ]
    );
}

#[test]
fn ordinal_weekday_lists_apply_later_standard_transitions() {
    assert_eq!(
        starts(
            &alternating_weekday_ordinals(),
            "20240622T090000",
            "20240622T000000Z",
            "20240625T000000Z"
        ),
        [
            at("2024-06-22T08:00:00Z"),
            at("2024-06-23T09:00:00Z"),
            at("2024-06-24T09:00:00Z")
        ]
    );
}

#[test]
fn year_scoped_weekday_lists_apply_later_ordinals() {
    let timezone = timezone(
        "20240101T020000",
        "BYDAY=1MO,20MO",
        "20240304T030000",
        "BYDAY=10MO,30MO",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240512T090000",
            "20240512T000000Z",
            "20240515T000000Z"
        ),
        [
            at("2024-05-12T09:00:00Z"),
            at("2024-05-13T08:00:00Z"),
            at("2024-05-14T08:00:00Z")
        ]
    );
}

#[test]
fn numeric_weekdays_intersect_the_stated_month_days() {
    let timezone = timezone(
        "20240303T020000",
        "BYMONTH=3,9;BYMONTHDAY=1,2,3,4,5,6,7;BYDAY=1SU",
        "20240407T030000",
        "BYMONTH=4,10;BYMONTHDAY=1,2,3,4,5,6,7;BYDAY=1SU",
    );
    assert_eq!(
        starts(
            &timezone,
            "20240831T090000",
            "20240831T000000Z",
            "20240903T000000Z"
        ),
        [
            at("2024-08-31T09:00:00Z"),
            at("2024-09-01T08:00:00Z"),
            at("2024-09-02T08:00:00Z")
        ]
    );
}

#[test]
fn queries_use_later_weekdays_in_a_list() {
    let calendar = calendar("20240616T090000");
    let engine = Engine::builder().zone_database(NoExternalZones).build();
    let mut session = engine.session();
    for (start, end, expected) in [
        ("20240616T075500Z", "20240616T080500Z", Match::Matched),
        ("20240616T085500Z", "20240616T090500Z", Match::Unmatched),
    ] {
        assert_eq!(
            query(&alternating_weekday_ordinals(), start, end)
                .matches(&mut session, &calendar)
                .unwrap(),
            expected
        );
    }
}
