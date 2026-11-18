// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Gregorian corrections follow [RFC 7529 section 4.1](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).

#![cfg(test)]

use icalkit::recurrence::{Occurrences, Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
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

fn expand(text: &str, start: &str, end: &str) -> Vec<Timestamp> {
    let rule = Rule::parse(text).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    starts(
        rule.occurrences(&mut session, at(start), window(start, end))
            .unwrap(),
    )
}

#[test]
fn yearly_leap_days_move_to_the_nearest_valid_date() {
    for (mode, common_date) in [("BACKWARD", "02-28"), ("FORWARD", "03-01")] {
        let expected = [
            at("2020-02-29T09:00:00Z"),
            at(&format!("2021-{common_date}T09:00:00Z")),
            at(&format!("2022-{common_date}T09:00:00Z")),
            at(&format!("2023-{common_date}T09:00:00Z")),
            at("2024-02-29T09:00:00Z"),
        ];
        assert_eq!(
            expand(
                &format!("FREQ=YEARLY;COUNT=5;RSCALE=GREGORIAN;SKIP={mode}"),
                "2020-02-29T09:00:00Z",
                "2025-01-01T00:00:00Z",
            ),
            expected,
        );
    }
}

#[test]
fn monthly_defaults_adjust_once_without_changing_the_anchor_day() {
    for (mode, february, april) in [
        ("BACKWARD", "2021-02-28", "2021-04-30"),
        ("FORWARD", "2021-03-01", "2021-05-01"),
    ] {
        assert_eq!(
            expand(
                &format!("RSCALE=GREGORY;SKIP={mode};FREQ=MONTHLY;COUNT=5"),
                "2021-01-31T09:00:00Z",
                "2021-07-01T00:00:00Z",
            ),
            [
                at("2021-01-31T09:00:00Z"),
                at(&format!("{february}T09:00:00Z")),
                at("2021-03-31T09:00:00Z"),
                at(&format!("{april}T09:00:00Z")),
                at("2021-05-31T09:00:00Z"),
            ],
        );
    }
}

#[test]
fn month_filters_precede_correction_and_weekday_filters_follow_it() {
    for weekday in ["MO", "1MO"] {
        assert_eq!(
            expand(
                &format!(
                    "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTH=2;BYMONTHDAY=31;BYDAY={weekday}"
                ),
                "2021-03-01T09:00:00Z",
                "2022-01-01T00:00:00Z",
            ),
            [at("2021-03-01T09:00:00Z")],
        );
    }
    assert_eq!(
        starts(
            Rule::parse(
                "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTH=2;BYMONTHDAY=31;BYDAY=SU"
            )
            .unwrap()
            .occurrences(
                &mut Engine::default().session(),
                at("2020-03-01T09:00:00Z"),
                window("2021-01-01T00:00:00Z", "2022-01-01T00:00:00Z"),
            )
            .unwrap(),
        ),
        Vec::<Timestamp>::new(),
    );
}

#[test]
fn negative_month_days_adjust_at_the_start_of_the_month() {
    for (mode, expected) in [
        ("BACKWARD", "2021-01-31T09:00:00Z"),
        ("FORWARD", "2021-02-01T09:00:00Z"),
    ] {
        assert_eq!(
            expand(
                &format!("FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYMONTH=2;BYMONTHDAY=-31"),
                expected,
                "2022-01-01T00:00:00Z",
            ),
            [at(expected)],
        );
    }
}

fn overlapping(mode: &str) -> String {
    let days = if mode == "FORWARD" { "1,31" } else { "1,-31" };
    format!("FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP={mode};BYMONTHDAY={days};BYHOUR=0,23;COUNT=10")
}

#[test]
fn overlapping_periods_sort_times_and_count_each_instant_once() {
    assert_eq!(
        expand(
            &overlapping("FORWARD"),
            "2021-01-01T00:00:00Z",
            "2022-01-01T00:00:00Z"
        ),
        [
            "2021-01-01T00:00:00Z",
            "2021-01-01T23:00:00Z",
            "2021-01-31T00:00:00Z",
            "2021-01-31T23:00:00Z",
            "2021-02-01T00:00:00Z",
            "2021-02-01T23:00:00Z",
            "2021-03-01T00:00:00Z",
            "2021-03-01T23:00:00Z",
            "2021-03-31T00:00:00Z",
            "2021-03-31T23:00:00Z",
        ]
        .map(at),
    );
    assert_eq!(
        expand(
            &overlapping("BACKWARD"),
            "2021-01-01T00:00:00Z",
            "2022-01-01T00:00:00Z"
        ),
        [
            "2021-01-01T00:00:00Z",
            "2021-01-01T23:00:00Z",
            "2021-01-31T00:00:00Z",
            "2021-01-31T23:00:00Z",
            "2021-02-01T00:00:00Z",
            "2021-02-01T23:00:00Z",
            "2021-03-01T00:00:00Z",
            "2021-03-01T23:00:00Z",
            "2021-03-31T00:00:00Z",
            "2021-03-31T23:00:00Z",
        ]
        .map(at),
    );
}

#[test]
fn period_positions_select_corrected_dates_before_counting() {
    assert_eq!(
        expand(
            "FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTHDAY=1,31;BYSETPOS=-1;COUNT=3",
            "2021-01-31T09:00:00Z",
            "2022-01-01T00:00:00Z",
        ),
        [
            "2021-01-31T09:00:00Z",
            "2021-03-01T09:00:00Z",
            "2021-03-31T09:00:00Z"
        ]
        .map(at),
    );
}

#[test]
fn a_narrow_window_includes_backward_candidates_from_the_next_month() {
    let rule = Rule::parse("FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=BACKWARD;BYMONTHDAY=-31").unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            rule.occurrences(
                &mut session,
                at("2020-12-01T09:00:00Z"),
                window("2021-01-31T00:00:00Z", "2021-02-01T00:00:00Z"),
            )
            .unwrap()
        ),
        [at("2021-01-31T09:00:00Z")],
    );
}

#[test]
fn until_includes_the_corrected_boundary_once() {
    assert_eq!(
        expand(
            "FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTHDAY=1,31;UNTIL=20210301T090000Z",
            "2021-02-01T09:00:00Z",
            "2022-01-01T00:00:00Z",
        ),
        ["2021-02-01T09:00:00Z", "2021-03-01T09:00:00Z"].map(at),
    );
}

#[test]
fn cursors_reconstruct_pending_periods_after_every_occurrence() {
    for mode in ["BACKWARD", "FORWARD"] {
        let text = overlapping(mode);
        let start = "2021-01-01T00:00:00Z";
        let end = "2022-01-01T00:00:00Z";
        let expected = expand(&text, start, end);
        let rule = Rule::parse(&text).unwrap();
        let engine = Engine::default();
        let mut session = engine.session();
        let mut stream = rule
            .occurrences(&mut session, at(start), window(start, end))
            .unwrap();
        for expected in expected {
            assert_eq!(stream.try_next().unwrap().unwrap().start(), expected);
            let cursor = stream.cursor();
            drop(stream);
            stream = rule
                .resume(&mut session, at(start), window(start, end), cursor)
                .unwrap();
        }
        assert!(stream.try_next().unwrap().is_none());
    }
}

#[test]
fn stored_calendars_preserve_corrections_additions_and_exclusions() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit skip//EN\r\n\
        BEGIN:VEVENT\r\nUID:skip@example.test\r\nDTSTAMP:20200101T000000Z\r\n\
        DTSTART:20200229T090000Z\r\nDURATION:PT1H\r\n\
        RRULE:RSCALE=GREGORIAN;FREQ=YEARLY;SKIP=FORWARD;COUNT=3\r\n\
        RDATE:20210302T090000Z\r\nEXDATE:20210301T090000Z\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            calendar
                .occurrences(
                    &mut session,
                    "skip@example.test",
                    window("2020-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
                )
                .unwrap()
        ),
        [
            "2020-02-29T09:00:00Z",
            "2021-03-02T09:00:00Z",
            "2022-03-01T09:00:00Z"
        ]
        .map(at),
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn omit_keeps_rfc5545_missing_date_behavior() {
    assert_eq!(
        expand(
            "FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=OMIT;COUNT=3",
            "2021-01-31T09:00:00Z",
            "2022-01-01T00:00:00Z",
        ),
        [
            "2021-01-31T09:00:00Z",
            "2021-03-31T09:00:00Z",
            "2021-05-31T09:00:00Z"
        ]
        .map(at),
    );
}
