// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Year day corrections follow [RFC 7529 section 4.1](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).

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

fn expand(text: &str, start: &str, bounds: Window) -> Vec<Timestamp> {
    let rule = Rule::parse(text).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    starts(rule.occurrences(&mut session, at(start), bounds).unwrap())
}

#[test]
fn positive_year_days_correct_to_the_year_boundary() {
    for (mode, common) in [
        ("BACKWARD", ["2021-12-31", "2022-12-31", "2023-12-31"]),
        ("FORWARD", ["2022-01-01", "2023-01-01", "2024-01-01"]),
    ] {
        assert_eq!(
            expand(
                &format!("FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY=366;COUNT=5"),
                "2020-12-31T09:00:00Z",
                window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z"),
            ),
            [
                at("2020-12-31T09:00:00Z"),
                at(&format!("{}T09:00:00Z", common[0])),
                at(&format!("{}T09:00:00Z", common[1])),
                at(&format!("{}T09:00:00Z", common[2])),
                at("2024-12-31T09:00:00Z"),
            ],
        );
    }
}

#[test]
fn negative_year_days_correct_before_the_start_of_the_year() {
    for (mode, common) in [
        ("BACKWARD", ["2020-12-31", "2021-12-31", "2022-12-31"]),
        ("FORWARD", ["2021-01-01", "2022-01-01", "2023-01-01"]),
    ] {
        assert_eq!(
            expand(
                &format!("FREQ=YEARLY;RSCALE=GREGORY;SKIP={mode};BYYEARDAY=-366;COUNT=5"),
                "2020-01-01T09:00:00Z",
                window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z"),
            ),
            [
                at("2020-01-01T09:00:00Z"),
                at(&format!("{}T09:00:00Z", common[0])),
                at(&format!("{}T09:00:00Z", common[1])),
                at(&format!("{}T09:00:00Z", common[2])),
                at("2024-01-01T09:00:00Z"),
            ],
        );
    }
}

#[test]
fn month_filters_match_the_source_year_day_before_correction() {
    for (mode, day, month, wanted) in [
        ("FORWARD", "366", "12", "2022-01-01T09:00:00Z"),
        ("BACKWARD", "-366", "1", "2020-12-31T09:00:00Z"),
    ] {
        assert_eq!(
            expand(
                &format!(
                    "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY={day};BYMONTH={month}"
                ),
                "2020-01-01T09:00:00Z",
                window("2020-12-31T00:00:00Z", "2022-01-02T00:00:00Z"),
            ),
            if mode == "FORWARD" {
                vec![at("2020-12-31T09:00:00Z"), at(wanted)]
            } else {
                vec![at(wanted), at("2021-12-31T09:00:00Z")]
            },
        );
    }
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=366;BYMONTH=1",
            "2020-01-01T09:00:00Z",
            window("2021-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
        ),
        Vec::<Timestamp>::new(),
    );
}

#[test]
fn month_days_filter_missing_year_days_before_they_move() {
    for (mode, day, month_day) in [("FORWARD", "366", "1"), ("BACKWARD", "-366", "31")] {
        assert_eq!(
            expand(
                &format!(
                    "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY={day};BYMONTHDAY={month_day}"
                ),
                "2020-01-01T09:00:00Z",
                window("2021-01-01T00:00:00Z", "2024-01-01T00:00:00Z"),
            ),
            Vec::<Timestamp>::new(),
        );
    }
}

#[test]
fn month_day_selection_keeps_the_selected_year_day() {
    for mode in ["BACKWARD", "FORWARD"] {
        assert_eq!(
            expand(
                &format!("FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY=32;BYMONTHDAY=15"),
                "2020-02-01T09:00:00Z",
                window("2020-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
            ),
            Vec::<Timestamp>::new(),
        );
        assert_eq!(
            expand(
                &format!(
                    "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY=32;BYMONTHDAY=1;COUNT=3"
                ),
                "2020-02-01T09:00:00Z",
                window("2020-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
            ),
            [
                "2020-02-01T09:00:00Z",
                "2021-02-01T09:00:00Z",
                "2022-02-01T09:00:00Z"
            ]
            .map(at),
        );
    }
}

#[test]
fn weekdays_match_the_corrected_date() {
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=366;BYDAY=SA",
            "2016-12-31T09:00:00Z",
            window("2021-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
        ),
        [at("2022-01-01T09:00:00Z")],
    );
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=366;BYMONTH=12;BYDAY=1SA",
            "2016-12-31T09:00:00Z",
            window("2021-01-01T00:00:00Z", "2023-01-01T00:00:00Z"),
        ),
        [at("2022-01-01T09:00:00Z")],
    );
}

fn overlapping(mode: &str) -> String {
    let days = if mode == "FORWARD" {
        "1,366"
    } else {
        "-1,-366"
    };
    format!("FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY={days};BYHOUR=0,23;COUNT=10")
}

#[test]
fn overlapping_years_order_times_and_count_each_key_once() {
    for (mode, middle, tail) in [
        ("FORWARD", ["2021-01-01", "2022-01-01"], "2023-01-01"),
        ("BACKWARD", ["2021-12-31", "2022-12-31"], "2023-12-31"),
    ] {
        let dates = ["2020-01-01", "2020-12-31", middle[0], middle[1], tail];
        let expected: Vec<_> = dates
            .into_iter()
            .flat_map(|date| {
                [
                    at(&format!("{date}T00:00:00Z")),
                    at(&format!("{date}T23:00:00Z")),
                ]
            })
            .collect();
        assert_eq!(
            expand(
                &overlapping(mode),
                "2020-01-01T00:00:00Z",
                window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z"),
            ),
            expected,
        );
    }
}

#[test]
fn a_narrow_window_reaches_backward_candidates_from_the_next_year() {
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=BACKWARD;BYYEARDAY=-366",
            "2020-01-01T09:00:00Z",
            window("2020-12-31T00:00:00Z", "2021-01-01T00:00:00Z"),
        ),
        [at("2020-12-31T09:00:00Z")],
    );
}

#[test]
fn periods_keep_the_interval_after_a_correction_crosses_years() {
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=BACKWARD;BYYEARDAY=-366;INTERVAL=2;COUNT=4",
            "2020-01-01T09:00:00Z",
            window("2020-01-01T00:00:00Z", "2027-01-01T00:00:00Z"),
        ),
        [
            "2020-01-01T09:00:00Z",
            "2021-12-31T09:00:00Z",
            "2024-01-01T09:00:00Z",
            "2025-12-31T09:00:00Z"
        ]
        .map(at),
    );
}

#[test]
fn period_positions_and_until_use_corrected_dates() {
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=1,366;BYSETPOS=-1;UNTIL=20230101T090000Z",
            "2020-12-31T09:00:00Z",
            window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z"),
        ),
        [
            "2020-12-31T09:00:00Z",
            "2022-01-01T09:00:00Z",
            "2023-01-01T09:00:00Z"
        ]
        .map(at),
    );
}

#[test]
fn year_day_cursors_reconstruct_every_pending_key() {
    for mode in ["BACKWARD", "FORWARD"] {
        let text = overlapping(mode);
        let bounds = window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z");
        let start = at("2020-01-01T00:00:00Z");
        let expected = expand(&text, "2020-01-01T00:00:00Z", bounds);
        let rule = Rule::parse(&text).unwrap();
        let engine = Engine::default();
        let mut session = engine.session();
        let mut stream = rule.occurrences(&mut session, start, bounds).unwrap();
        for expected in expected {
            assert_eq!(stream.try_next().unwrap().unwrap().start(), expected);
            let cursor = stream.cursor();
            drop(stream);
            stream = rule.resume(&mut session, start, bounds, cursor).unwrap();
        }
        assert!(stream.try_next().unwrap().is_none());
    }
}

#[test]
fn stored_calendars_merge_corrected_year_days_without_changing_bytes() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit year days//EN\r\n\
        BEGIN:VEVENT\r\nUID:year@example.test\r\nDTSTAMP:20200101T000000Z\r\n\
        DTSTART:20201231T090000Z\r\nDURATION:PT1H\r\n\
        RRULE:RSCALE=GREGORIAN;FREQ=YEARLY;SKIP=FORWARD;BYYEARDAY=366;COUNT=3\r\n\
        RDATE:20210701T090000Z\r\nEXDATE:20220101T090000Z\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            calendar
                .occurrences(
                    &mut session,
                    "year@example.test",
                    window("2020-01-01T00:00:00Z", "2024-01-01T00:00:00Z"),
                )
                .unwrap()
        ),
        [
            "2020-12-31T09:00:00Z",
            "2021-07-01T09:00:00Z",
            "2023-01-01T09:00:00Z"
        ]
        .map(at),
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn omit_keeps_missing_year_days_out_of_the_set() {
    assert_eq!(
        expand(
            "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=OMIT;BYYEARDAY=366;COUNT=2",
            "2020-12-31T09:00:00Z",
            window("2020-01-01T00:00:00Z", "2025-01-01T00:00:00Z"),
        ),
        ["2020-12-31T09:00:00Z", "2024-12-31T09:00:00Z"].map(at),
    );
}
