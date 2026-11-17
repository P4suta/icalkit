// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Provider coverage claims remain visible without refusing usable answers.

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::recurrence::Window;
use icalkit::scheduling::{Actor, Message};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, Session};

#[derive(Clone, Copy)]
enum LocalAnswer {
    Exact,
    Partial,
    Gap,
    Absent,
}

struct CoverageDatabase {
    local_complete: bool,
    offset_complete: bool,
    answer: LocalAnswer,
}

impl ZoneDatabase for CoverageDatabase {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        if tzid != "Test/Coverage" {
            return None;
        }
        match self.answer {
            LocalAnswer::Partial if local.day() == 3 => None,
            LocalAnswer::Exact | LocalAnswer::Partial => Some(ZoneResolution::exact(
                format!("{local}Z").parse().unwrap(),
                "coverage-fixture",
                self.local_complete,
            )),
            LocalAnswer::Gap => Some(ZoneResolution::gap("coverage-fixture", self.local_complete)),
            LocalAnswer::Absent => None,
        }
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        if tzid != "Test/Coverage" || matches!(self.answer, LocalAnswer::Absent) {
            return None;
        }
        OffsetResolution::new(0, "coverage-fixture", self.offset_complete)
    }
}

fn engine(local_complete: bool, offset_complete: bool, answer: LocalAnswer) -> Engine {
    Engine::builder()
        .zone_database(CoverageDatabase {
            local_complete,
            offset_complete,
            answer,
        })
        .build()
}

fn calendar() -> Calendar {
    Calendar::parse(
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit coverage//EN\r\n\
          BEGIN:VEVENT\r\nUID:coverage@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
          DTSTART;TZID=Test/Coverage:20240102T090000\r\nDURATION:PT1H\r\n\
          RRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Coverage review\r\n\
          ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:alice@example.test\r\n\
          SEQUENCE:2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .unwrap()
}

fn query() -> Query {
    Query::parse(br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:expand start="20240102T000000Z" end="20240105T000000Z"/></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="20240103T090000Z" end="20240103T100000Z"/>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#).unwrap()
}

fn window() -> Window {
    Window::new(
        "2024-01-02T00:00:00Z".parse().unwrap(),
        "2024-01-05T00:00:00Z".parse().unwrap(),
    )
    .unwrap()
}

fn expected_starts() -> Vec<Timestamp> {
    [
        "2024-01-02T09:00:00Z",
        "2024-01-03T09:00:00Z",
        "2024-01-04T09:00:00Z",
    ]
    .map(|value| value.parse().unwrap())
    .to_vec()
}

fn starts(session: &mut Session<'_>, calendar: &Calendar) -> Vec<Timestamp> {
    let mut occurrences = calendar
        .occurrences(session, "coverage@example.test", window())
        .unwrap();
    let mut values = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        values.push(occurrence.start());
    }
    values
}

fn assert_coverage_note(session: &Session<'_>, expected: bool) {
    let issues = session.issues();
    assert_eq!(issues.len(), usize::from(expected), "{issues:?}");
    if expected {
        assert_eq!(issues[0].code().as_str(), "time-zone-coverage-unasserted");
        assert!(issues[0].is_note());
    }
}

fn coverage_claims() -> [(bool, bool); 4] {
    [(false, true), (true, false), (false, false), (true, true)]
}

#[test]
fn recurrence_keeps_usable_answers_and_reports_unasserted_coverage() {
    for (local_complete, offset_complete) in coverage_claims() {
        let engine = engine(local_complete, offset_complete, LocalAnswer::Exact);
        let mut session = engine.session();
        assert_eq!(starts(&mut session, &calendar()), expected_starts());
        assert_coverage_note(&session, !(local_complete && offset_complete));
    }
}

#[test]
fn query_matching_keeps_usable_answers_and_reports_unasserted_coverage() {
    for (local_complete, offset_complete) in coverage_claims() {
        let engine = engine(local_complete, offset_complete, LocalAnswer::Exact);
        let mut session = engine.session();
        assert_eq!(
            query().matches(&mut session, &calendar()).unwrap(),
            Match::Matched
        );
        assert_coverage_note(&session, !(local_complete && offset_complete));
    }
}

#[test]
fn projection_keeps_usable_answers_and_reports_unasserted_coverage() {
    for (local_complete, offset_complete) in coverage_claims() {
        let engine = engine(local_complete, offset_complete, LocalAnswer::Exact);
        let mut session = engine.session();
        let calendar = calendar();
        let original = calendar.to_bytes();
        let projected = query().project(&mut session, &calendar).unwrap();
        let projected = Calendar::parse(projected.as_bytes()).unwrap();
        let starts: Vec<_> = projected
            .events()
            .map(|event| event.dtstart().unwrap().as_utc().unwrap())
            .collect();
        assert_eq!(starts, expected_starts());
        assert_eq!(calendar.to_bytes(), original);
        assert_coverage_note(&session, !(local_complete && offset_complete));
    }
}

#[test]
fn notes_are_coalesced_across_workflows_and_isolated_between_sessions() {
    let engine = engine(false, false, LocalAnswer::Exact);
    let mut session = engine.session();
    assert_coverage_note(&session, false);
    let calendar = calendar();
    let query = query();
    for _ in 0..3 {
        assert_eq!(starts(&mut session, &calendar), expected_starts());
        assert_eq!(
            query.matches(&mut session, &calendar).unwrap(),
            Match::Matched
        );
        assert!(query.project(&mut session, &calendar).is_ok());
        assert_coverage_note(&session, true);
    }
    let mut independent = engine.session();
    assert_coverage_note(&independent, false);
    assert_eq!(
        query.matches(&mut independent, &calendar).unwrap(),
        Match::Matched
    );
    assert_coverage_note(&independent, true);
    assert_coverage_note(&session, true);
}

#[test]
fn failed_operations_retain_the_coverage_note() {
    for complete in [false, true] {
        let engine = engine(complete, complete, LocalAnswer::Partial);
        let calendar = calendar();
        let query = query();
        let mut matching = engine.session();
        assert_eq!(
            query.matches(&mut matching, &calendar).unwrap(),
            Match::Undecided
        );
        assert_coverage_note(&matching, !complete);
        let mut projection = engine.session();
        assert!(query.project(&mut projection, &calendar).is_err());
        assert_coverage_note(&projection, !complete);
        let mut recurrence = engine.session();
        let outcome = calendar
            .occurrences(&mut recurrence, "coverage@example.test", window())
            .and_then(|mut occurrences| {
                while occurrences.try_next()?.is_some() {}
                Ok(())
            });
        assert!(outcome.is_err());
        assert_coverage_note(&recurrence, !complete);
    }
}

#[test]
fn gap_answers_keep_their_own_coverage_claim() {
    for complete in [false, true] {
        let engine = engine(complete, true, LocalAnswer::Gap);
        let mut session = engine.session();
        assert_eq!(
            query().matches(&mut session, &calendar()).unwrap(),
            Match::Unmatched
        );
        assert_coverage_note(&session, !complete);
    }
}

#[test]
fn absent_answers_do_not_invent_coverage_claims() {
    let engine = engine(false, false, LocalAnswer::Absent);
    let mut session = engine.session();
    let calendar = calendar();
    assert_eq!(
        query().matches(&mut session, &calendar).unwrap(),
        Match::Undecided
    );
    assert!(query().project(&mut session, &calendar).is_err());
    let outcome = calendar
        .occurrences(&mut session, "coverage@example.test", window())
        .and_then(|mut occurrences| occurrences.try_next());
    assert!(outcome.is_err());
    assert_coverage_note(&session, false);
}

fn request(anchor: &str) -> Message {
    Message::read(
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit coverage//EN\r\n\
             METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:coverage@example.test\r\n\
             RECURRENCE-ID;TZID=Test/Coverage:{anchor}T090000\r\n\
             DTSTAMP:20240101T010000Z\r\n\
             DTSTART;TZID=Test/Coverage:{anchor}T100000\r\nDURATION:PT1H\r\n\
             SUMMARY:Coverage review later\r\nORGANIZER:mailto:chair@example.test\r\n\
             ATTENDEE:mailto:alice@example.test\r\nSEQUENCE:3\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        )
        .as_bytes(),
    )
    .unwrap()
}

#[test]
fn scheduling_retains_coverage_for_accepted_and_refused_membership() {
    let calendar = calendar();
    let actor = Actor::new("mailto:chair@example.test").unwrap();
    for (local_complete, offset_complete) in coverage_claims() {
        let engine = engine(local_complete, offset_complete, LocalAnswer::Exact);
        for (anchor, accepted) in [("20240103", true), ("20240105", false)] {
            let mut session = engine.session();
            let message = request(anchor);
            assert_eq!(
                message.review_in(&mut session, &calendar, &actor).is_ok(),
                accepted
            );
            assert_coverage_note(&session, !(local_complete && offset_complete));
        }
    }
}
