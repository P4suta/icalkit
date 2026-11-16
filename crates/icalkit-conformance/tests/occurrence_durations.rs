// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Duration placement follows [RFC 5545 section 3.3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.6) and [section 3.8.5.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3).
//! Query results follow [RFC 4791 sections 9.6.5 and 9.9](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.5).

#![cfg(test)]

use icalkit::caldav::{Match, ProjectedCalendar, Query};
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
    let properties = properties.replace(',', ",\r\n ");
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

fn project(source: &str, start: &str, end: &str) -> Result<ProjectedCalendar, icalkit::Error> {
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
    query.project(&mut session, &source)
}

fn projected_document(source: &str, start: &str, end: &str) -> Document {
    let projected = project(source, start, end).unwrap();
    let document = Document::parse(projected.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    if document
        .components()
        .next()
        .unwrap()
        .components()
        .next()
        .is_some()
    {
        Calendar::parse(projected.as_bytes()).unwrap();
    }
    document
}

fn expanded_times(source: &str, start: &str, end: &str) -> Vec<(String, String)> {
    let document = projected_document(source, start, end);
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

#[test]
fn period_inclusions_keep_their_own_lengths_without_a_rule() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD:20240102T090000Z/PT3H,20240103T090000Z/20240103T140000Z\r\n",
    ));
    assert_times(
        &source,
        "20240101T000000Z",
        "20240104T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T090000Z", "20240102T120000Z"),
            ("20240103T090000Z", "20240103T140000Z"),
        ],
    );
}

#[test]
fn period_lengths_replace_rule_lengths_without_duplicating_instances() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n\
         RDATE;VALUE=PERIOD:20240102T090000Z/PT3H\r\n",
    ));
    assert_times(
        &source,
        "20240101T000000Z",
        "20240104T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T090000Z", "20240102T120000Z"),
            ("20240103T090000Z", "20240103T100000Z"),
        ],
    );
}

#[test]
fn exclusions_still_remove_period_inclusions() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD:20240102T090000Z/PT3H\r\nEXDATE:20240102T090000Z\r\n",
    ));
    assert_times(&source, "20240102T000000Z", "20240103T000000Z", &[]);
}

#[test]
fn long_periods_are_found_when_their_starts_precede_the_window() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\n\
         RDATE;VALUE=PERIOD:20240102T090000Z/PT10H\r\n",
    ));
    assert_times(
        &source,
        "20240102T170000Z",
        "20240102T180000Z",
        &[("20240102T090000Z", "20240102T190000Z")],
    );
}

#[test]
fn detached_exceptions_keep_their_stated_lengths() {
    let master = event("DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n");
    let exception = event(
        "RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T110000Z\r\nDTEND:20240102T140000Z\r\n",
    );
    assert_times(
        &calendar(&format!("{master}{exception}")),
        "20240101T000000Z",
        "20240104T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T110000Z", "20240102T140000Z"),
            ("20240103T090000Z", "20240103T100000Z"),
        ],
    );
}

#[test]
fn future_length_changes_leave_separate_exceptions_unchanged() {
    let master = event("DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=4\r\n");
    let anchor = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\n\
         DTSTART:20240102T090000Z\r\nDURATION:PT3H\r\n",
    );
    let exception = event("RECURRENCE-ID:20240104T090000Z\r\nDTSTART:20240104T090000Z\r\n");
    assert_times(
        &calendar(&format!("{master}{anchor}{exception}")),
        "20240101T000000Z",
        "20240105T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T090000Z", "20240102T120000Z"),
            ("20240103T090000Z", "20240103T120000Z"),
            ("20240104T090000Z", "20240104T100000Z"),
        ],
    );
}

#[test]
fn later_stated_lengths_win_in_recurrence_order() {
    let master = event("DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=4\r\n");
    let early = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\nDTSTART:20240102T090000Z\r\nDURATION:PT3H\r\n",
    );
    let later = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20240103T090000Z\r\nDTSTART:20240103T090000Z\r\nDURATION:PT2H\r\n",
    );
    let exact =
        event("RECURRENCE-ID:20240104T090000Z\r\nDTSTART:20240104T090000Z\r\nDURATION:PT4H\r\n");
    assert_times(
        &calendar(&format!("{master}{exact}{later}{early}")),
        "20240101T000000Z",
        "20240105T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T090000Z", "20240102T120000Z"),
            ("20240103T090000Z", "20240103T110000Z"),
            ("20240104T090000Z", "20240104T130000Z"),
        ],
    );
}

#[test]
fn conflicting_period_lengths_remain_explicit_failures() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD:20240102T090000Z/PT2H,20240102T090000Z/PT3H\r\n",
    ));
    Calendar::parse(source.as_bytes()).unwrap();
    assert!(project(&source, "20240101T000000Z", "20240104T000000Z").is_err());
}

#[test]
fn period_inclusions_keep_their_own_zone() {
    let source = calendar(&event(
        "DTSTART:20240309T090000Z\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD;TZID=Test/Daylight:20240310T090000/PT2H\r\n",
    ));
    assert_times(
        &source,
        "20240309T000000Z",
        "20240312T000000Z",
        &[
            ("20240309T090000Z", "20240309T100000Z"),
            ("20240310T130000Z", "20240310T150000Z"),
        ],
    );
}

#[test]
fn exclusions_use_their_stated_zone() {
    let source = calendar(&event(
        "DTSTART:20240309T130000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n\
         EXDATE;TZID=Test/Daylight:20240310T090000\r\n",
    ));
    assert_times(
        &source,
        "20240309T000000Z",
        "20240312T000000Z",
        &[
            ("20240309T130000Z", "20240309T140000Z"),
            ("20240311T130000Z", "20240311T140000Z"),
        ],
    );
}

#[test]
fn detached_exceptions_keep_their_own_duration_clock() {
    let master = event("DTSTART:20240309T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n");
    let exception = event(
        "RECURRENCE-ID:20240309T090000Z\r\nDTSTART;TZID=Test/Daylight:20240309T090000\r\nDURATION:P1D\r\n",
    );
    assert_times(
        &calendar(&format!("{master}{exception}")),
        "20240309T000000Z",
        "20240312T000000Z",
        &[
            ("20240309T140000Z", "20240310T130000Z"),
            ("20240310T090000Z", "20240310T100000Z"),
            ("20240311T090000Z", "20240311T100000Z"),
        ],
    );
}
