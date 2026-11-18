// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Query identity refuses unreadable [TEXT values](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11).

#![cfg(test)]

use icalkit_conformance::internal::core::{
    Document, IgnoreDiagnostics, Instant, Limits, Meter, UtcOffset,
};
use icalkit_conformance::internal::dav::{CompFilter, TimeRange};
use icalkit_conformance::internal::query::{
    self, Budget, Match, QueryError, Reduction, Selection, Undecided, Zones,
};
use icalkit_conformance::internal::tz::FixedOffsetSource;

fn source(uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit query identity//EN\r\n\
         BEGIN:VEVENT\r\n{uid}\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n\
         END:VEVENT\r\nBEGIN:VEVENT\r\n{uid}\r\nDTSTAMP:20240101T000000Z\r\n\
         RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T120000Z\r\n\
         DURATION:PT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn window() -> TimeRange {
    // 2024-01-02 from 09:00Z through 10:00Z.
    TimeRange::new(
        Some(Instant::from_unix_seconds(1_704_186_000)),
        Some(Instant::from_unix_seconds(1_704_189_600)),
    )
    .unwrap()
}

fn assert_unreadable(source: &[u8]) {
    let limits = Limits::DEFAULT;
    let document = Document::parse(source, limits, &mut IgnoreDiagnostics).unwrap();
    assert_eq!(document.to_bytes(), source);
    let mut scratch = Meter::new(limits);
    let mut event = CompFilter::new(b"VEVENT", limits, &mut scratch).unwrap();
    event.time_range = Some(window());
    let mut filter = CompFilter::new(b"VCALENDAR", limits, &mut scratch).unwrap();
    filter.push_comp(event, limits, &mut scratch).unwrap();
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let zones = Zones::new(&utc);
    let mut meter = Meter::new(limits);
    assert_eq!(
        query::matches(
            &filter,
            &document,
            zones,
            &mut Budget::new(limits, &mut meter)
        )
        .unwrap(),
        Match::Undecided(Undecided::ValueUnreadable)
    );
    let selection = Selection::new(document, Reduction::FAITHFUL);
    let mut meter = Meter::new(limits);
    assert!(matches!(
        query::expand_calendar(
            &selection,
            window(),
            zones,
            &mut Budget::new(limits, &mut meter)
        ),
        Err(QueryError::Unrepresentable)
    ));
    assert_eq!(selection.calendar().to_bytes(), source);
}

#[test]
fn malformed_text_cannot_correlate_a_recurrence_set() {
    for uid in [
        "UID:bad\\q",
        "UID:bad;value",
        "UID:bad,value",
        "UID:dangling\\",
        "UID:bad\0value",
        "UID;VALUE=:series",
        "UID;VALUE=INTEGER:123",
        "UID;VALUE=TEXT;VALUE=TEXT:series",
    ] {
        assert_unreadable(source(uid).as_bytes());
    }
}

#[test]
fn repeated_identifiers_do_not_select_one_claim() {
    for uid in ["UID:series\r\nUID:other", "UID:series\r\nUID:series"] {
        assert_unreadable(source(uid).as_bytes());
    }
}

#[test]
fn invalid_utf8_does_not_grant_a_series_identity() {
    let mut source = source("UID:invalid").into_bytes();
    let at = source
        .windows(b"invalid".len())
        .position(|part| part == b"invalid")
        .unwrap();
    source[at] = 0xff;
    assert_unreadable(&source);
}
