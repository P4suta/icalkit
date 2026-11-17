// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Weekly boundary workflows follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::recurrence::{Occurrences, Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn window() -> Window {
    Window::new(at("0000-01-01T00:00:00Z"), at("0000-02-01T00:00:00Z")).unwrap()
}

fn starts(mut occurrences: Occurrences<'_>) -> Vec<Timestamp> {
    let mut result = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        result.push(occurrence.start());
    }
    result
}

fn rule_starts(text: &str, source: &str) -> Vec<Timestamp> {
    let rule = Rule::parse(text).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    starts(
        rule.occurrences(&mut session, at(source), window())
            .unwrap(),
    )
}

#[test]
fn every_week_start_keeps_default_weekdays_in_registered_calendars() {
    for scale in [
        "GREGORIAN",
        "BUDDHIST",
        "CHINESE",
        "COPTIC",
        "DANGI",
        "ETHIOPIC",
        "ETHIOAA",
        "HEBREW",
        "INDIAN",
        "ISLAMIC-CIVIL",
        "ISLAMIC-TBLA",
        "ISLAMIC-UMALQURA",
        "ISO8601",
        "JAPANESE",
        "PERSIAN",
        "ROC",
    ] {
        for start in ["MO", "TU", "WE", "TH", "FR", "SA", "SU"] {
            assert_eq!(
                rule_starts(
                    &format!("RSCALE={scale};FREQ=WEEKLY;INTERVAL=2;COUNT=3;WKST={start}"),
                    "0000-01-01T09:00:00Z"
                ),
                [
                    at("0000-01-01T09:00:00Z"),
                    at("0000-01-15T09:00:00Z"),
                    at("0000-01-29T09:00:00Z")
                ],
                "{scale}, {start}",
            );
        }
    }
}

#[test]
fn source_week_limits_keep_remaining_dates_with_huge_intervals() {
    for interval in [1_u32, 2, u32::MAX] {
        for end in ["COUNT=2", "UNTIL=00000102T090000Z"] {
            assert_eq!(
                rule_starts(
                    &format!("FREQ=WEEKLY;INTERVAL={interval};BYDAY=SA,SU;{end}"),
                    "0000-01-01T09:00:00Z"
                ),
                [at("0000-01-01T09:00:00Z"), at("0000-01-02T09:00:00Z")],
                "{interval}, {end}",
            );
        }
    }
}

#[test]
fn clocks_and_positions_use_the_writable_source_week() {
    assert_eq!(
        rule_starts(
            "FREQ=WEEKLY;BYDAY=SA,SU;BYHOUR=9,12;BYSETPOS=2,-1;COUNT=2",
            "0000-01-01T12:00:00Z"
        ),
        [at("0000-01-01T12:00:00Z"), at("0000-01-02T12:00:00Z")],
    );
}

#[test]
fn cursors_resume_inside_the_first_writable_week() {
    let rule = Rule::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=SA,SU;COUNT=4").unwrap();
    let source = at("0000-01-01T09:00:00Z");
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = rule.occurrences(&mut session, source, window()).unwrap();
    assert_eq!(occurrences.try_next().unwrap().unwrap().start(), source);
    let cursor = occurrences.cursor();
    drop(occurrences);
    assert_eq!(
        starts(rule.resume(&mut session, source, window(), cursor).unwrap()),
        [
            at("0000-01-02T09:00:00Z"),
            at("0000-01-15T09:00:00Z"),
            at("0000-01-16T09:00:00Z")
        ],
    );
}

#[test]
fn stored_weekly_series_preserve_additions_exclusions_and_bytes() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit weekly edges//EN\r\n\
        BEGIN:VEVENT\r\nUID:weekly@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
        DTSTART:00000101T090000Z\r\nDURATION:PT1H\r\n\
        RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=SA,SU;COUNT=4\r\n\
        RDATE:00000109T090000Z\r\nEXDATE:00000115T090000Z\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            calendar
                .occurrences(&mut session, "weekly@example.test", window())
                .unwrap()
        ),
        [
            at("0000-01-01T09:00:00Z"),
            at("0000-01-02T09:00:00Z"),
            at("0000-01-09T09:00:00Z"),
            at("0000-01-16T09:00:00Z")
        ],
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}
