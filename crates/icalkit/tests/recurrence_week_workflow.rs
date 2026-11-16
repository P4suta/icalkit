// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Week-numbering years follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::recurrence::{Occurrences, Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn window(start: &str, end: &str) -> Window {
    Window::new(at(start), at(end)).unwrap()
}

fn starts(mut occurrences: Occurrences<'_>) -> Vec<Timestamp> {
    let mut output = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        output.push(occurrence.start());
    }
    output
}

fn rule_starts(rule: &str, dtstart: &str, bounds: Window) -> Vec<Timestamp> {
    let rule = Rule::parse(rule).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    starts(rule.occurrences(&mut session, at(dtstart), bounds).unwrap())
}

#[test]
fn narrow_windows_include_candidates_before_the_next_calendar_year() {
    for end in ["2019-12-31T10:00:00Z", "2020-01-01T00:00:00Z"] {
        assert_eq!(
            rule_starts(
                "FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU",
                "2019-01-01T09:00:00Z",
                window("2019-12-31T00:00:00Z", end),
            ),
            [at("2019-12-31T09:00:00Z")],
        );
    }
}

#[test]
fn alternate_week_starts_move_the_week_year_boundary() {
    assert_eq!(
        rule_starts(
            "FREQ=YEARLY;BYWEEKNO=1;BYDAY=SU;WKST=SU",
            "2017-01-01T09:00:00Z",
            window("2019-12-29T00:00:00Z", "2019-12-30T00:00:00Z"),
        ),
        [at("2019-12-29T09:00:00Z")],
    );
}

#[test]
fn intervals_and_period_selection_keep_the_week_numbering_year() {
    for rule in [
        "FREQ=YEARLY;INTERVAL=2;BYWEEKNO=1;BYDAY=TU",
        "FREQ=YEARLY;INTERVAL=2;BYWEEKNO=1;BYDAY=MO,TU;BYSETPOS=-1",
    ] {
        assert_eq!(
            rule_starts(
                rule,
                "2018-01-02T09:00:00Z",
                window("2019-12-31T00:00:00Z", "2020-01-01T00:00:00Z"),
            ),
            [at("2019-12-31T09:00:00Z")],
            "{rule}",
        );
    }
}

#[test]
fn rule_limits_apply_to_candidates_before_the_calendar_anchor() {
    for limit in ["COUNT=2", "UNTIL=20191231T090000Z"] {
        assert_eq!(
            rule_starts(
                &format!("FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU;{limit}"),
                "2019-01-01T09:00:00Z",
                window("2019-12-31T00:00:00Z", "2020-01-01T00:00:00Z"),
            ),
            [at("2019-12-31T09:00:00Z")],
            "{limit}",
        );
    }
    for limit in ["COUNT=1", "UNTIL=20191231T085959Z"] {
        assert_eq!(
            rule_starts(
                &format!("FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU;{limit}"),
                "2019-01-01T09:00:00Z",
                window("2019-12-31T00:00:00Z", "2020-01-01T00:00:00Z"),
            ),
            Vec::<Timestamp>::new(),
            "{limit}",
        );
    }
}

#[test]
fn a_cursor_keeps_the_remaining_candidates_before_january() {
    let rule = Rule::parse("FREQ=YEARLY;BYWEEKNO=1;BYDAY=MO,TU;COUNT=2").unwrap();
    let bounds = window("2019-12-30T00:00:00Z", "2020-01-01T00:00:00Z");
    let dtstart = at("2019-12-30T09:00:00Z");
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = rule.occurrences(&mut session, dtstart, bounds).unwrap();
    assert_eq!(occurrences.try_next().unwrap().unwrap().start(), dtstart);
    let cursor = occurrences.cursor();
    drop(occurrences);
    assert_eq!(
        starts(rule.resume(&mut session, dtstart, bounds, cursor).unwrap()),
        [at("2019-12-31T09:00:00Z")],
    );
}

#[test]
fn resuming_a_completed_count_keeps_the_cursor_position() {
    let rule = Rule::parse("FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU;COUNT=2").unwrap();
    let bounds = window("2019-01-01T00:00:00Z", "2022-01-01T00:00:00Z");
    let dtstart = at("2019-01-01T09:00:00Z");
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = rule.occurrences(&mut session, dtstart, bounds).unwrap();
    assert_eq!(occurrences.try_next().unwrap().unwrap().start(), dtstart);
    assert_eq!(
        occurrences.try_next().unwrap().unwrap().start(),
        at("2019-12-31T09:00:00Z")
    );
    assert!(occurrences.try_next().unwrap().is_none());
    let cursor = occurrences.cursor();
    drop(occurrences);
    let mut resumed = rule.resume(&mut session, dtstart, bounds, cursor).unwrap();
    assert!(resumed.try_next().unwrap().is_none());
    assert_eq!(resumed.cursor(), cursor);
}

fn stored(extra: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit week years//EN\r\n\
         BEGIN:VEVENT\r\nUID:week@example.test\r\nDTSTAMP:20170101T000000Z\r\n\
         DTSTART:20170101T090000Z\r\nDURATION:PT1H\r\n\
         RRULE:FREQ=YEARLY;BYWEEKNO=1;BYDAY=SU;WKST=SU\r\n\
         {extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn stored_calendars_use_the_same_week_year_bounds() {
    let source = stored("");
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            calendar
                .occurrences(
                    &mut session,
                    "week@example.test",
                    window("2019-12-29T00:00:00Z", "2019-12-30T00:00:00Z")
                )
                .unwrap()
        ),
        [at("2019-12-29T09:00:00Z")],
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn stored_week_years_still_merge_additions_and_exclusions() {
    let source = stored("RDATE:20191228T090000Z\r\nEXDATE:20191229T090000Z\r\n");
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            calendar
                .occurrences(
                    &mut session,
                    "week@example.test",
                    window("2019-12-28T00:00:00Z", "2019-12-30T00:00:00Z")
                )
                .unwrap()
        ),
        [at("2019-12-28T09:00:00Z")],
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn week_year_windows_still_exclude_their_upper_edge() {
    assert_eq!(
        rule_starts(
            "FREQ=YEARLY;BYWEEKNO=1;BYDAY=TU",
            "2019-01-01T09:00:00Z",
            window("2019-12-31T00:00:00Z", "2019-12-31T09:00:00Z"),
        ),
        Vec::<Timestamp>::new(),
    );
}

#[test]
fn ordinary_years_and_hourly_windows_keep_their_boundaries() {
    assert_eq!(
        rule_starts(
            "FREQ=YEARLY",
            "2019-01-01T09:00:00Z",
            window("2019-12-31T00:00:00Z", "2020-01-01T00:00:00Z"),
        ),
        Vec::<Timestamp>::new(),
    );
    assert_eq!(
        rule_starts(
            "FREQ=HOURLY",
            "2019-12-31T09:00:00Z",
            window("2019-12-31T10:00:00Z", "2019-12-31T11:00:00Z"),
        ),
        [at("2019-12-31T10:00:00Z")],
    );
}
