// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Numeric recurrence spelling follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::Engine;
use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn expand(text: &str, start: &str, end: &str) -> Vec<Timestamp> {
    let rule = Rule::parse(text).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = rule
        .occurrences(
            &mut session,
            at(start),
            Window::new(at(start), at(end)).unwrap(),
        )
        .unwrap();
    let mut starts = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        starts.push(occurrence.start());
    }
    starts
}

#[test]
fn padded_clock_selectors_and_scalars_keep_the_same_occurrences() {
    let padded =
        "FREQ=DAILY;BYHOUR=09,10;BYMINUTE=00,05;BYSECOND=00,03;COUNT=000000009;INTERVAL=000000002";
    let canonical = "FREQ=DAILY;BYHOUR=9,10;BYMINUTE=0,5;BYSECOND=0,3;COUNT=9;INTERVAL=2";
    assert_eq!(
        Rule::parse(padded).unwrap(),
        Rule::parse(canonical).unwrap()
    );
    assert_eq!(
        expand(padded, "2024-01-01T09:00:00Z", "2024-01-04T00:00:00Z"),
        [
            "2024-01-01T09:00:00Z",
            "2024-01-01T09:00:03Z",
            "2024-01-01T09:05:00Z",
            "2024-01-01T09:05:03Z",
            "2024-01-01T10:00:00Z",
            "2024-01-01T10:00:03Z",
            "2024-01-01T10:05:00Z",
            "2024-01-01T10:05:03Z",
            "2024-01-03T09:00:00Z",
        ]
        .map(at)
    );
}

#[test]
fn padded_leap_months_preserve_the_rfc_anniversary() {
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYMONTH=05L;BYMONTHDAY=08;SKIP=FORWARD;COUNT=0003",
            "2014-02-08T09:00:00Z",
            "2017-01-01T00:00:00Z"
        ),
        [
            at("2014-02-08T09:00:00Z"),
            at("2015-02-27T09:00:00Z"),
            at("2016-02-17T09:00:00Z")
        ]
    );
}

#[test]
fn public_rules_refuse_selectors_with_excess_digits() {
    for parts in [
        "BYSECOND=000",
        "BYMINUTE=000",
        "BYHOUR=009",
        "BYDAY=001MO",
        "BYMONTHDAY=001",
        "BYYEARDAY=0001",
        "BYWEEKNO=001",
        "BYMONTH=001",
        "BYSETPOS=0001;BYHOUR=9",
        "RSCALE=HEBREW;BYMONTH=005L",
    ] {
        let text = format!("FREQ=YEARLY;{parts}");
        let error = Rule::parse(&text).unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "icalkit.recurrence.invalid-rule",
            "{text}"
        );
        assert!(
            error.issues().iter().any(|issue| issue.is_error()
                && issue.code().as_str() == "recurrence-rule-part-out-of-range"),
            "{text}: {error:?}"
        );
    }
}
