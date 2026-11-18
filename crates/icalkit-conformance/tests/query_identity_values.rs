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

#[test]
fn an_unreadable_master_prevents_calendar_expansion() {
    let wire = concat!(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:detached\r\n",
        "RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T090000Z\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID;VALUE=INTEGER:123\r\nDTSTART:20240101T090000Z\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(wire.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let source = Selection::new(document, Reduction::FAITHFUL);
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let mut meter = Meter::new(Limits::DEFAULT);
    let result = query::expand_calendar(
        &source,
        window(),
        Zones::new(&utc),
        &mut Budget::new(Limits::DEFAULT, &mut meter),
    );
    assert_eq!(result.unwrap_err(), QueryError::Unrepresentable);
}

#[test]
fn limiting_detached_instances_scales_with_the_sibling_count() {
    use std::fmt::Write;

    let limits = Limits::DEFAULT;
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let cost = |count, masters: &[String]| {
        let mut wire = String::from("BEGIN:VCALENDAR\r\n");
        for uid in masters {
            write!(
                wire,
                "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20240101T090000Z\r\nEND:VEVENT\r\n"
            )
            .unwrap();
        }
        for id in 0..count {
            write!(
                wire,
                "BEGIN:VEVENT\r\nUID:orphan-{id:04}\r\nRECURRENCE-ID:20240102T090000Z\r\nEND:VEVENT\r\n"
            )
            .unwrap();
        }
        wire.push_str("END:VCALENDAR\r\n");
        let doc = Document::parse(wire.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let source = Selection::new(doc, Reduction::FAITHFUL);
        let mut meter = Meter::new(limits);
        let result = query::limit_recurrence_set_in_window(
            &source,
            window(),
            Zones::new(&utc),
            &mut Budget::new(limits, &mut meter),
        )
        .unwrap();
        assert_eq!(result.calendar().to_bytes(), wire.as_bytes());
        meter.spent()
    };
    let masters = |count| {
        (0..count)
            .map(|id| format!("master-{id:04}"))
            .collect::<Vec<_>>()
    };
    let small = cost(64, &masters(64));
    let large = cost(128, &masters(128));
    assert!(
        large > small && large < small.saturating_mul(3),
        "{small} and {large}"
    );
    assert!(cost(128, &["a".repeat(32_768)]) < 1_000_000);
}

#[test]
fn an_unreadable_master_keeps_an_override_even_after_a_matching_uid() {
    let wire = concat!(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:series\r\nRECURRENCE-ID:20230102T090000Z\r\n",
        "DTSTART:20230102T090000Z\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:series\r\nDTSTART:20230101T090000Z\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID;VALUE=INTEGER:123\r\nDTSTART:20230101T090000Z\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let limits = Limits::DEFAULT;
    let doc = Document::parse(wire.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
    let source = Selection::new(doc, Reduction::FAITHFUL);
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let mut meter = Meter::new(limits);
    let result = query::limit_recurrence_set_in_window(
        &source,
        window(),
        Zones::new(&utc),
        &mut Budget::new(limits, &mut meter),
    )
    .unwrap();
    assert_eq!(result.calendar().to_bytes(), wire.as_bytes());
}
