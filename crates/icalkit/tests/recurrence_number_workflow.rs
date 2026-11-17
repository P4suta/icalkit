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
    let padded = "FREQ=DAILY;BYHOUR=09;BYMINUTE=00;BYSECOND=00;COUNT=000000002;INTERVAL=000000001";
    let canonical = "FREQ=DAILY;BYHOUR=9;BYMINUTE=0;BYSECOND=0;COUNT=2;INTERVAL=1";
    assert_eq!(
        Rule::parse(padded).unwrap(),
        Rule::parse(canonical).unwrap()
    );
    assert_eq!(
        expand(padded, "2024-01-01T09:00:00Z", "2024-01-04T00:00:00Z"),
        [at("2024-01-01T09:00:00Z"), at("2024-01-02T09:00:00Z")]
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
