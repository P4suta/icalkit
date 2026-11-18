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
    CivilDate, CivilDateTime, CivilTime, Component, Document, Limits, UtcOffset,
};

#[derive(Debug)]
struct DaylightZone;

const DAYLIGHT_TIMEZONE: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit query zone//EN\r\n",
    "BEGIN:VTIMEZONE\r\nTZID:Test/Daylight\r\n",
    "BEGIN:STANDARD\r\nDTSTART:20231105T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\n",
    "BEGIN:DAYLIGHT\r\nDTSTART:20240310T020000\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nEND:DAYLIGHT\r\n",
    "BEGIN:STANDARD\r\nDTSTART:20241103T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\n",
    "END:VTIMEZONE\r\nEND:VCALENDAR\r\n",
);

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
    component("VEVENT", properties)
}

fn component(kind: &str, properties: &str) -> String {
    let properties = properties.replace(',', ",\r\n ");
    format!(
        "BEGIN:{kind}\r\nUID:one@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         {properties}END:{kind}\r\n"
    )
}

fn calendar(components: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit durations//EN\r\n\
         {components}END:VCALENDAR\r\n"
    )
}

fn project(source: &str, start: &str, end: &str) -> Result<ProjectedCalendar, icalkit::Error> {
    project_with_timezone(source, start, end, None)
}

fn project_with_timezone(
    source: &str,
    start: &str,
    end: &str,
    timezone: Option<&str>,
) -> Result<ProjectedCalendar, icalkit::Error> {
    let timezone = timezone.map_or_else(String::new, |data| {
        format!("<C:timezone>{data}</C:timezone>")
    });
    let query = Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
{timezone}
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
    checked_projection(&projected)
}

fn checked_projection(projected: &ProjectedCalendar) -> Document {
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

fn property_text<'a>(component: &'a Component, name: &[u8]) -> Option<&'a str> {
    let property = component
        .properties()
        .find(|property| property.is_named(name))?;
    std::str::from_utf8(property.value_text().as_bytes()).ok()
}

fn limited_document(source: &str, start: &str, end: &str) -> Result<Document, icalkit::Error> {
    let query = Query::parse(format!(r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:limit-recurrence-set start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#).as_bytes()).unwrap();
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(DaylightZone).build();
    let mut session = engine.session();
    let projected = query.project(&mut session, &calendar)?;
    Ok(checked_projection(&projected))
}

fn limited_period_source(duration: &str, rdate: &str, count: u8) -> String {
    calendar(&format!(
        "{}{}",
        event(&format!(
            "DTSTART:20240101T090000Z\r\nDURATION:{duration}\r\n\
             RRULE:FREQ=DAILY;COUNT={count}\r\n{rdate}\r\n"
        )),
        event("RECURRENCE-ID:20240103T090000Z\r\nDTSTART:20240104T090000Z\r\nDURATION:PT1H\r\n"),
    ))
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

const FIXED_UTC_TIMEZONE: &str = "BEGIN:VTIMEZONE\r\nTZID:Fixed/UTC\r\nBEGIN:STANDARD\r\n\
        DTSTART:20200101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0000\r\n\
        END:STANDARD\r\nEND:VTIMEZONE\r\n";

fn fold_periods(periods: &str, extra: &str) -> String {
    calendar(&event(&format!(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD:{periods}\r\n{extra}"
    )))
}

#[test]
fn fold_periods_with_distinct_durations_keep_both_instants() {
    let source = fold_periods("20241103T053000Z/PT10M,20241103T063000Z/PT20M", "");
    assert_times(
        &source,
        "20241103T050000Z",
        "20241103T080000Z",
        &[
            ("20241103T053000Z", "20241103T054000Z"),
            ("20241103T063000Z", "20241103T065000Z"),
        ],
    );
}

#[test]
fn fold_periods_with_equal_durations_keep_both_instants() {
    let source = fold_periods("20241103T053000Z/PT10M,20241103T063000Z/PT10M", "");
    let parsed = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(DaylightZone).build();
    let mut session = engine.session();
    let window =
        icalkit::recurrence::Window::new(at("2024-11-03T05:00:00Z"), at("2024-11-03T08:00:00Z"))
            .unwrap();
    let mut occurrences = parsed
        .occurrences(&mut session, "one@example.test", window)
        .unwrap();
    let mut starts = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        starts.push((occurrence.key(), occurrence.start()));
    }
    let earlier = at("2024-11-03T05:30:00Z");
    let later = at("2024-11-03T06:30:00Z");
    assert_eq!(starts, [(earlier, earlier), (later, later)]);
}

#[test]
fn fold_periods_keep_a_lone_later_instant() {
    let source = fold_periods("20241103T063000Z/PT10M", "");
    assert_times(
        &source,
        "20241103T050000Z",
        "20241103T080000Z",
        &[("20241103T063000Z", "20241103T064000Z")],
    );
}

#[test]
fn fold_periods_exclude_only_the_addressed_instant() {
    let source = fold_periods(
        "20241103T053000Z/PT10M,20241103T063000Z/PT10M",
        "EXDATE:20241103T063000Z\r\n",
    );
    assert_times(
        &source,
        "20241103T050000Z",
        "20241103T080000Z",
        &[("20241103T053000Z", "20241103T054000Z")],
    );
}

#[test]
fn fold_periods_keep_the_same_civil_range_shift_as_ordinary_dates() {
    let source = calendar(&format!(
        "{}{}",
        event(
            "DTSTART;TZID=Test/Daylight:20240309T090000\r\nDURATION:PT10M\r\n\
         RDATE;VALUE=PERIOD:20241103T053000Z/PT10M,20241103T063000Z/PT10M\r\n",
        ),
        event(
            "RECURRENCE-ID;RANGE=THISANDFUTURE;TZID=Test/Daylight:20240309T090000\r\n\
         DTSTART;TZID=Test/Daylight:20240310T090000\r\nDURATION:PT20M\r\n",
        )
    ));
    assert_times(
        &source,
        "20241104T040000Z",
        "20241104T080000Z",
        &[
            ("20241104T063000Z", "20241104T065000Z"),
            ("20241104T063000Z", "20241104T065000Z"),
        ],
    );
}

#[test]
fn fold_periods_shift_future_occurrences_by_the_stated_instant_difference() {
    let source = calendar(&format!(
        "{FIXED_UTC_TIMEZONE}{}{}",
        event(
            "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT10M\r\n\
         RRULE:FREQ=DAILY;COUNT=3\r\nRDATE;VALUE=PERIOD:20241103T063000Z/PT10M\r\n",
        ),
        event(
            "RECURRENCE-ID;RANGE=THISANDFUTURE;TZID=Test/Daylight:20241103T013000\r\n\
         DTSTART;TZID=Fixed/UTC:20241103T063000\r\nDURATION:PT20M\r\n",
        )
    ));
    assert_times(
        &source,
        "20241103T050000Z",
        "20241104T090000Z",
        &[
            ("20241103T063000Z", "20241103T065000Z"),
            ("20241103T073000Z", "20241103T075000Z"),
            ("20241104T073000Z", "20241104T075000Z"),
        ],
    );
}

#[test]
fn fold_periods_keep_the_first_exact_override_for_a_repeated_identifier() {
    let master = event(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT10M\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    );
    let first = event(
        "RECURRENCE-ID;TZID=Test/Daylight:20241103T013000\r\nDTSTART;TZID=Test/Daylight:20241103T023000\r\nDURATION:PT10M\r\n",
    );
    let second = event(
        "RECURRENCE-ID;TZID=Test/Daylight:20241103T013000\r\nDTSTART;TZID=Fixed/UTC:20241103T063000\r\nDURATION:PT20M\r\n",
    );
    assert_times(
        &calendar(&format!("{FIXED_UTC_TIMEZONE}{master}{first}{second}")),
        "20241103T050000Z",
        "20241103T080000Z",
        &[("20241103T073000Z", "20241103T074000Z")],
    );
}

#[test]
fn fold_periods_distinguish_a_foreign_zone_identifier_from_the_first_local_reading() {
    let master = event(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT10M\r\n\
        RRULE:FREQ=DAILY;COUNT=2\r\nRDATE;VALUE=PERIOD:20241103T063000Z/PT10M\r\n",
    );
    let exception = |range| {
        event(&format!(
            "RECURRENCE-ID{range};TZID=Fixed/UTC:20241103T063000\r\n\
        DTSTART;TZID=Test/Daylight:20241103T023000\r\nDURATION:PT20M\r\n"
        ))
    };
    assert_times(
        &calendar(&format!("{FIXED_UTC_TIMEZONE}{master}{}", exception(""))),
        "20241103T050000Z",
        "20241103T080000Z",
        &[
            ("20241103T053000Z", "20241103T054000Z"),
            ("20241103T073000Z", "20241103T075000Z"),
        ],
    );
    let source = calendar(&format!(
        "{FIXED_UTC_TIMEZONE}{master}{}",
        exception(";RANGE=THISANDFUTURE")
    ));
    assert_eq!(
        project(&source, "20241103T050000Z", "20241103T080000Z")
            .unwrap_err()
            .code()
            .as_str(),
        "icalkit.caldav.query-projection"
    );
}

#[test]
fn fold_periods_apply_range_changes_in_recurrence_order() {
    let master = event(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT1H\r\n\
         RDATE;VALUE=PERIOD:20241103T053000Z/PT10M,20241103T054500Z/PT10M,20241103T063000Z/PT10M\r\n",
    );
    let range = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE;TZID=Test/Daylight:20241103T014500\r\n\
         DTSTART;TZID=Test/Daylight:20241103T014500\r\nDURATION:PT30M\r\n",
    );
    let source = calendar(&format!("{master}{range}"));
    assert_times(
        &source,
        "20241103T050000Z",
        "20241103T080000Z",
        &[
            ("20241103T053000Z", "20241103T054000Z"),
            ("20241103T054500Z", "20241103T061500Z"),
            ("20241103T063000Z", "20241103T070000Z"),
        ],
    );
    let document = projected_document(&source, "20241103T050000Z", "20241103T080000Z");
    let keys = document
        .components()
        .next()
        .unwrap()
        .components()
        .filter_map(|component| property_text(component, b"RECURRENCE-ID"))
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        ["20241103T053000Z", "20241103T054500Z", "20241103T063000Z"]
    );
}

#[test]
fn fold_periods_match_only_at_the_written_instant() {
    let source = fold_periods("20241103T063000Z/PT10M,20241103T063000Z/PT10M", "");
    let parsed = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder().zone_database(DaylightZone).build();
    for (start, end, expected) in [
        ("20241103T053500Z", "20241103T054000Z", Match::Unmatched),
        ("20241103T063500Z", "20241103T064000Z", Match::Matched),
    ] {
        let query = Query::parse(format!(
            r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{start}" end="{end}"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        ).as_bytes()).unwrap();
        assert_eq!(
            query.matches(&mut engine.session(), &parsed).unwrap(),
            expected
        );
    }
    assert_times(
        &source,
        "20241103T050000Z",
        "20241103T080000Z",
        &[("20241103T063000Z", "20241103T064000Z")],
    );
}

#[test]
fn fold_periods_charge_the_initial_occurrence_once() {
    let engine = Engine::builder()
        .zone_database(DaylightZone)
        .resource_policy(icalkit::ResourcePolicy::secure().with_occurrences_per_search(1))
        .build();
    let window =
        icalkit::recurrence::Window::new(at("2024-11-03T05:00:00Z"), at("2024-11-03T08:00:00Z"))
            .unwrap();
    for (rdate, complete) in [("20241103T053000Z", true), ("20241103T063000Z", false)] {
        let source = calendar(&event(&format!(
            "DTSTART;TZID=Test/Daylight:20241103T013000\r\nDURATION:PT10M\r\nRDATE:{rdate}\r\n",
        )));
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        let mut session = engine.session();
        let mut occurrences = parsed
            .occurrences(&mut session, "one@example.test", window)
            .unwrap();
        if complete {
            assert_eq!(
                occurrences.try_next().unwrap().unwrap().key(),
                at("2024-11-03T05:30:00Z")
            );
            assert_eq!(occurrences.try_next().unwrap(), None);
        } else {
            assert_eq!(
                occurrences.try_next().unwrap_err().code().as_str(),
                "icalkit.recurrence.budget-exhausted"
            );
        }
    }
}

#[test]
fn fold_periods_charge_a_reply_for_either_generated_occurrence() {
    let parties = "ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:alice@example.test\r\n";
    let master = event(&format!(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT10M\r\n\
         RDATE:20241103T053000Z,20241103T063000Z\r\n{parties}"
    ));
    let range = event(&format!(
        "RECURRENCE-ID;RANGE=THISANDFUTURE;TZID=Test/Daylight:20241102T013000\r\n\
         DTSTART;TZID=Test/Daylight:20241102T033000\r\nDURATION:PT10M\r\n{parties}"
    ));
    let held = calendar(&format!("{FIXED_UTC_TIMEZONE}{master}{range}"));
    let parsed = Calendar::parse(held.as_bytes()).unwrap();
    let actor = icalkit::scheduling::Actor::new("mailto:alice@example.test").unwrap();
    for key in ["20241103T053000", "20241103T063000"] {
        let answer = event(&format!(
            "RECURRENCE-ID;TZID=Fixed/UTC:{key}\r\nORGANIZER:mailto:chair@example.test\r\n\
             ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.test\r\n",
        ));
        let source = calendar(&format!("METHOD:REPLY\r\n{FIXED_UTC_TIMEZONE}{answer}"));
        let message = icalkit::scheduling::Message::read(source.as_bytes()).unwrap();
        for (cap, accepted) in [(10, true), (0, false)] {
            let engine = Engine::builder()
                .zone_database(DaylightZone)
                .resource_policy(icalkit::ResourcePolicy::secure().with_occurrences_per_search(cap))
                .build();
            let result = message.review_in(&mut engine.session(), &parsed, &actor);
            assert_eq!(result.is_ok(), accepted, "{key}, cap {cap}");
        }
    }
}

#[test]
fn fold_periods_budget_interrupts_before_publishing_an_unordered_prefix() {
    let source = fold_periods("20241103T053000Z/PT10M,20241103T063000Z/PT10M", "");
    let parsed = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::builder()
        .zone_database(DaylightZone)
        .resource_policy(icalkit::ResourcePolicy::secure().with_occurrences_per_search(1))
        .build();
    let window =
        icalkit::recurrence::Window::new(at("2024-11-03T05:00:00Z"), at("2024-11-03T08:00:00Z"))
            .unwrap();
    let mut session = engine.session();
    let mut occurrences = parsed
        .occurrences(&mut session, "one@example.test", window)
        .unwrap();
    assert_eq!(
        occurrences.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
    let cursor = occurrences.cursor();
    let full = Engine::builder().zone_database(DaylightZone).build();
    let mut session = full.session();
    let mut resumed = parsed
        .resume_occurrences(&mut session, "one@example.test", window, cursor)
        .unwrap();
    assert_eq!(
        resumed.try_next().unwrap().unwrap().key(),
        at("2024-11-03T05:30:00Z")
    );
    assert_eq!(
        resumed.try_next().unwrap().unwrap().key(),
        at("2024-11-03T06:30:00Z")
    );
    assert_eq!(resumed.try_next().unwrap(), None);
}

#[test]
fn fold_periods_keep_the_later_instant_after_a_local_override() {
    let master = event(
        "DTSTART;TZID=Test/Daylight:20241102T013000\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=2\r\n\
         RDATE;VALUE=PERIOD:20241103T063000Z/PT10M\r\n",
    );
    let moved = event(
        "RECURRENCE-ID;TZID=Test/Daylight:20241103T013000\r\nDTSTART;TZID=Test/Daylight:20241103T020000\r\nDURATION:PT20M\r\n",
    );
    let mut times = expanded_times(
        &calendar(&format!("{master}{moved}")),
        "20241103T050000Z",
        "20241103T080000Z",
    );
    times.sort_unstable();
    assert_eq!(
        times,
        [
            ("20241103T063000Z".to_owned(), "20241103T064000Z".to_owned()),
            ("20241103T070000Z".to_owned(), "20241103T072000Z".to_owned()),
        ]
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
    let master = event("DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=6\r\n");
    let anchor = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\n\
         DTSTART:20240102T090000Z\r\nDURATION:PT3H\r\n",
    );
    let exception = event("RECURRENCE-ID:20240104T090000Z\r\nDTSTART:20240104T090000Z\r\n");
    let unstated_range =
        event("RECURRENCE-ID;RANGE=THISANDFUTURE:20240105T090000Z\r\nDTSTART:20240105T090000Z\r\n");
    assert_times(
        &calendar(&format!("{master}{unstated_range}{anchor}{exception}")),
        "20240101T000000Z",
        "20240107T000000Z",
        &[
            ("20240101T090000Z", "20240101T100000Z"),
            ("20240102T090000Z", "20240102T120000Z"),
            ("20240103T090000Z", "20240103T120000Z"),
            ("20240104T090000Z", "20240104T100000Z"),
            ("20240105T090000Z", "20240105T100000Z"),
            ("20240106T090000Z", "20240106T120000Z"),
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

#[test]
fn instantaneous_events_keep_their_implicit_end() {
    let source = calendar(&event(
        "DTSTART:20240101T090000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let document = projected_document(&source, "20240101T000000Z", "20240103T000000Z");
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    for event in root.components() {
        assert!(property_text(event, b"DTEND").is_none());
        assert!(property_text(event, b"DURATION").is_none());
    }
}

#[test]
fn task_instances_use_due_instead_of_an_event_end() {
    let source = calendar(&component(
        "VTODO",
        "DTSTART:20240101T090000Z\r\nDUE:20240101T100000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let document = projected_document(&source, "20240101T000000Z", "20240103T000000Z");
    let root = document.components().next().unwrap();
    let due: Vec<&str> = root
        .components()
        .map(|task| {
            assert!(property_text(task, b"DTEND").is_none());
            assert!(property_text(task, b"DURATION").is_none());
            property_text(task, b"DUE").unwrap()
        })
        .collect();
    assert_eq!(due, ["20240101T100000Z", "20240102T100000Z"]);
}

#[test]
fn journal_instances_do_not_gain_event_end_properties() {
    let source = calendar(&component(
        "VJOURNAL",
        "DTSTART:20240101T090000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let document = projected_document(&source, "20240101T000000Z", "20240103T000000Z");
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    for journal in root.components() {
        assert!(property_text(journal, b"DTEND").is_none());
        assert!(property_text(journal, b"DUE").is_none());
    }
}

#[test]
fn floating_periods_use_the_query_zone_and_keep_day_and_time_units_distinct() {
    let timezone = DAYLIGHT_TIMEZONE;
    for (length, expected_end) in [("P1D", "20240310T160000Z"), ("PT24H", "20240310T170000Z")] {
        let source = calendar(&event(&format!(
            "DTSTART:20240308T120000\r\nDURATION:PT1H\r\n\
             RDATE;VALUE=PERIOD:20240309T120000/{length}\r\n"
        )));
        let projected = project_with_timezone(
            &source,
            "20240308T000000Z",
            "20240311T000000Z",
            Some(timezone),
        )
        .unwrap();
        let document = checked_projection(&projected);
        let root = document.components().next().unwrap();
        let period = root
            .components()
            .find(|component| property_text(component, b"DTSTART") == Some("20240309T170000Z"))
            .unwrap();
        assert_eq!(property_text(period, b"DTEND"), Some(expected_end));
    }
}

#[test]
fn task_duration_instances_keep_their_duration_boundary_semantics() {
    let source = calendar(&component(
        "VTODO",
        "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let filter = Query::parse(
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VTODO">
<C:time-range start="20240102T100000Z" end="20240102T103000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        filter.matches(&mut session, &original).unwrap(),
        Match::Matched
    );
    let projected = project(&source, "20240102T100000Z", "20240102T103000Z").unwrap();
    let projected = Calendar::parse(projected.as_bytes()).unwrap();
    assert_eq!(
        filter.matches(&mut session, &projected).unwrap(),
        Match::Matched
    );
    let document =
        Document::parse(&projected.to_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let root = document.components().next().unwrap();
    let task = root.components().next().unwrap();
    assert!(property_text(task, b"DTEND").is_none());
    assert!(property_text(task, b"DUE").is_none());
    assert!(property_text(task, b"DURATION").is_some());
}

#[test]
fn limiting_a_series_accounts_for_exact_and_range_siblings() {
    let master =
        event("DTSTART:20240101T090000Z\r\nDURATION:PT30M\r\nRRULE:FREQ=DAILY;COUNT=4\r\n");
    let range = event(
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20240102T090000Z\r\nDTSTART:20240102T100000Z\r\nDURATION:PT30M\r\n",
    );
    let exact = event("RECURRENCE-ID:20240103T090000Z\r\nDTSTART:20240103T120000Z\r\n");
    let morning = ("20240103T100000Z", "20240103T110000Z");
    let noon = ("20240103T121500Z", "20240103T124500Z");
    let engine = Engine::default();
    for (siblings, retained, count, (start, end)) in [
        (range.clone(), "20240102T090000Z", 1, morning),
        (format!("{range}{exact}"), "20240103T090000Z", 0, morning),
        (format!("{exact}{range}"), "20240103T090000Z", 0, morning),
        (format!("{range}{exact}"), "20240103T090000Z", 1, noon),
        (exact.clone(), "20240103T090000Z", 1, noon),
    ] {
        let request = format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:limit-recurrence-set start="{start}" end="{end}"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#
        );
        let query = Query::parse(request.as_bytes()).unwrap();
        let source = calendar(&format!("{master}{siblings}"));
        let original = Calendar::parse(source.as_bytes()).unwrap();
        let limited = query.project(&mut engine.session(), &original).unwrap();
        let parsed = checked_projection(&limited);
        let root = parsed.components().next().unwrap();
        let ids = root
            .components()
            .filter_map(|event| property_text(event, b"RECURRENCE-ID"))
            .collect::<Vec<_>>();
        assert_eq!(ids, [retained]);
        let reduced = std::str::from_utf8(limited.as_bytes()).unwrap();
        for input in [source.as_str(), reduced] {
            let placed = projected_document(input, start, end);
            assert_eq!(
                placed.components().next().unwrap().components().count(),
                count
            );
        }
    }
}

#[test]
fn limited_task_overrides_use_the_original_duration_boundary() {
    let source = calendar(&format!(
        "{}{}",
        component(
            "VTODO",
            "DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
        ),
        component(
            "VTODO",
            "RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T110000Z\r\nDURATION:PT2H\r\n",
        ),
    ));
    let query = Query::parse(
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
<C:limit-recurrence-set start="20240102T100000Z" end="20240102T103000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let projected = query.project(&mut session, &original).unwrap();
    let document = checked_projection(&projected);
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    assert_eq!(
        property_text(root.components().nth(1).unwrap(), b"RECURRENCE-ID"),
        Some("20240102T090000Z")
    );
}

#[test]
fn detached_task_instances_remain_searchable_without_a_master() {
    let source = calendar(&component(
        "VTODO",
        "RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T110000Z\r\nDURATION:PT2H\r\n",
    ));
    let query = Query::parse(
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VTODO">
<C:time-range start="20240102T120000Z" end="20240102T123000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &original).unwrap(),
        Match::Matched
    );
}

#[test]
fn journal_periods_still_use_the_journal_point_overlap_rule() {
    let source = calendar(&component(
        "VJOURNAL",
        "DTSTART:20240101T090000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\nRDATE;VALUE=PERIOD:20240102T090000Z/PT3H\r\n",
    ));
    let filter = Query::parse(
        br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VJOURNAL">
<C:time-range start="20240102T100000Z" end="20240102T110000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let original = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        filter.matches(&mut session, &original).unwrap(),
        Match::Unmatched
    );
    let document = projected_document(&source, "20240102T100000Z", "20240102T110000Z");
    let root = document.components().next().unwrap();
    assert!(
        !root
            .components()
            .any(|component| component.is_named(b"VJOURNAL"))
    );
}

#[test]
fn date_journals_keep_their_date_values_after_expansion() {
    let source = calendar(&component(
        "VJOURNAL",
        "DTSTART;VALUE=DATE:20240309\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let projected = project_with_timezone(
        &source,
        "20240309T000000Z",
        "20240311T000000Z",
        Some(DAYLIGHT_TIMEZONE),
    )
    .unwrap();
    let document = checked_projection(&projected);
    let root = document.components().next().unwrap();
    let dates: Vec<&str> = root
        .components()
        .map(|journal| {
            assert!(property_text(journal, b"DTEND").is_none());
            property_text(journal, b"DTSTART").unwrap()
        })
        .collect();
    assert_eq!(dates, ["20240309", "20240310"]);
    assert_eq!(
        property_text(root.components().nth(1).unwrap(), b"RECURRENCE-ID"),
        Some("20240310")
    );
}

#[test]
fn date_journals_keep_extension_parameters_after_expansion() {
    let source = calendar(&component(
        "VJOURNAL",
        "DTSTART;VALUE=DATE;X-META=kept:20240309\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
    ));
    let projected = project_with_timezone(
        &source,
        "20240309T000000Z",
        "20240311T000000Z",
        Some(DAYLIGHT_TIMEZONE),
    )
    .unwrap();
    let document = checked_projection(&projected);
    let root = document.components().next().unwrap();
    assert_eq!(root.components().count(), 2);
    for journal in root.components() {
        let start = journal
            .properties()
            .find(|property| property.is_named(b"DTSTART"))
            .unwrap();
        assert_eq!(
            start
                .parameters_named(b"X-META")
                .next()
                .unwrap()
                .value()
                .as_bytes(),
            b"kept"
        );
        assert_eq!(
            start.parameters_named(b"VALUE").next().unwrap().unquoted(),
            b"DATE"
        );
    }
}

#[test]
fn original_period_lengths_keep_moved_overrides_in_the_limited_set() {
    for count in [2, 3] {
        for period in [
            "20240103T090000Z/20240103T190000Z",
            "20240103T090000Z/PT10H",
        ] {
            let source =
                limited_period_source("PT1H", &format!("RDATE;VALUE=PERIOD:{period}"), count);
            let document =
                limited_document(&source, "20240103T150000Z", "20240103T160000Z").unwrap();
            assert_eq!(
                document.components().next().unwrap().components().count(),
                2
            );
        }
    }
}

#[test]
fn shorter_original_periods_do_not_inherit_the_master_length() {
    let source = limited_period_source("PT10H", "RDATE;VALUE=PERIOD:20240103T090000Z/PT1H", 3);
    let document = limited_document(&source, "20240103T150000Z", "20240103T160000Z").unwrap();
    assert_eq!(
        document.components().next().unwrap().components().count(),
        1
    );
}

#[test]
fn original_zoned_periods_keep_their_own_nominal_day_length() {
    let components = format!(
        "{}{}",
        event(concat!(
            "DTSTART:20240308T170000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
            "RDATE;VALUE=PERIOD;TZID=Test/Daylight:20240309T120000/P1D\r\n",
        )),
        event("RECURRENCE-ID:20240309T170000Z\r\nDTSTART:20240311T170000Z\r\nDURATION:PT1H\r\n"),
    );
    let source = format!(
        "{}{components}END:VCALENDAR\r\n",
        DAYLIGHT_TIMEZONE.strip_suffix("END:VCALENDAR\r\n").unwrap()
    );
    for (start, end, expected) in [
        ("20240310T150000Z", "20240310T153000Z", 2),
        ("20240310T160000Z", "20240310T163000Z", 1),
    ] {
        let document = limited_document(&source, start, end).unwrap();
        let events = document
            .components()
            .next()
            .unwrap()
            .components()
            .filter(|component| component.is_named(b"VEVENT"))
            .count();
        assert_eq!(events, expected);
    }
}

#[test]
fn opaque_original_period_data_does_not_produce_a_negative_decision() {
    let source = limited_period_source("PT1H", "RDATE;VALUE=X-FUTURE:20240103T090000Z/PT1H", 3);
    let document = limited_document(&source, "20240103T150000Z", "20240103T160000Z").unwrap();
    assert_eq!(
        document.components().next().unwrap().components().count(),
        2
    );
}
