// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence through the unified Jiff-based public boundary.

use icalkit::recurrence::{Rule, Window};
use icalkit::time::{DateTime, OffsetResolution, Timestamp, ZoneDatabase, ZoneResolution};
use icalkit::{Calendar, Engine, ResourcePolicy};

const NEW_YEAR_2024: i64 = 1_704_067_200;

fn at(seconds: i64) -> Timestamp {
    Timestamp::constant(seconds, 0)
}

fn january_2024(day: i64, hour: i64) -> Timestamp {
    let days = day.saturating_sub(1).saturating_mul(86_400);
    let hours = hour.saturating_mul(3_600);
    at(NEW_YEAR_2024.saturating_add(days).saturating_add(hours))
}

#[derive(Debug)]
struct TestZone;

impl ZoneDatabase for TestZone {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        (tzid == "Test/Zone").then(|| {
            ZoneResolution::exact(
                january_2024(i64::from(local.day()), i64::from(local.hour())),
                "test-zone",
                true,
            )
        })
    }

    fn offset_at(&self, tzid: &str, _instant: Timestamp) -> Option<OffsetResolution> {
        (tzid == "Test/Zone")
            .then(|| OffsetResolution::new(0, "test-zone", true))
            .flatten()
    }
}

#[test]
fn a_rule_is_strictly_parsed_and_unknown_extensions_remain_notes() {
    let rule = Rule::parse("FREQ=DAILY;COUNT=3;X-OWNER-HINT=PLUM").unwrap();
    assert!(rule.issues().iter().any(icalkit::Issue::is_note));

    let error = Rule::parse("FREQ=DAILY;INTERVAL=0").unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.recurrence.invalid-rule");
    assert!(error.issues().iter().any(icalkit::Issue::is_error));
}

#[test]
fn occurrences_are_windowed_lazy_and_resumable_with_an_opaque_cursor() {
    let rule = Rule::parse("FREQ=DAILY;COUNT=3").unwrap();
    let window = Window::new(at(NEW_YEAR_2024), at(NEW_YEAR_2024 + 7 * 86_400)).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();

    let mut occurrences = rule
        .occurrences(&mut session, at(NEW_YEAR_2024), window)
        .unwrap();
    let first = occurrences.try_next().unwrap().unwrap();
    assert_eq!(first.key(), at(NEW_YEAR_2024));
    assert_eq!(first.start(), at(NEW_YEAR_2024));
    let cursor = occurrences.cursor();
    drop(occurrences);

    let mut resumed = rule
        .resume(&mut session, at(NEW_YEAR_2024), window, cursor)
        .unwrap();
    assert_eq!(
        resumed.try_next().unwrap().unwrap().start(),
        at(NEW_YEAR_2024 + 86_400)
    );
    assert_eq!(
        resumed.try_next().unwrap().unwrap().start(),
        at(NEW_YEAR_2024 + 2 * 86_400)
    );
    assert!(resumed.try_next().unwrap().is_none());
}

#[test]
fn occurrence_budget_exhaustion_cannot_be_mistaken_for_the_end() {
    let policy = ResourcePolicy::secure().with_occurrences_per_search(1);
    let engine = Engine::builder().resource_policy(policy).build();
    let mut session = engine.session();
    let rule = Rule::parse("FREQ=DAILY;COUNT=3").unwrap();
    let window = Window::new(at(NEW_YEAR_2024), at(NEW_YEAR_2024 + 7 * 86_400)).unwrap();
    let mut occurrences = rule
        .occurrences(&mut session, at(NEW_YEAR_2024), window)
        .unwrap();

    assert!(occurrences.try_next().unwrap().is_some());
    assert_eq!(
        occurrences.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
}

#[test]
fn recurrence_windows_refuse_fractional_seconds_instead_of_rounding_them() {
    assert!(
        Window::new(
            Timestamp::new(NEW_YEAR_2024, 1).unwrap(),
            at(NEW_YEAR_2024 + 1),
        )
        .is_none()
    );
}

#[test]
fn calendar_occurrences_integrate_rdate_exdate_and_overrides_in_effective_start_order() {
    let calendar = Calendar::parse(
        b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:set@example.test\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART:20240101T090000Z\r\n\
RRULE:FREQ=DAILY;COUNT=4\r\n\
RDATE:20240105T090000Z\r\n\
EXDATE:20240102T090000Z\r\n\
SUMMARY:Base\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:set@example.test\r\n\
RECURRENCE-ID:20240103T090000Z\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART:20240101T080000Z\r\n\
SUMMARY:Moved earlier\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .unwrap();
    let window = Window::new(january_2024(1, 0), january_2024(6, 0)).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = calendar
        .occurrences(&mut session, "set@example.test", window)
        .unwrap();

    let first = occurrences.try_next().unwrap().unwrap();
    assert_eq!(first.key(), january_2024(3, 9));
    assert_eq!(first.start(), january_2024(1, 8));
    let cursor = occurrences.cursor();
    drop(occurrences);

    let mut resumed = calendar
        .resume_occurrences(&mut session, "set@example.test", window, cursor)
        .unwrap();
    let rest = core::iter::from_fn(|| resumed.try_next().transpose())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rest.iter().map(|item| item.start()).collect::<Vec<_>>(),
        [january_2024(1, 9), january_2024(4, 9), january_2024(5, 9),]
    );
}

#[test]
fn calendar_recurrence_budget_exhaustion_is_a_terminal_error_not_a_short_set() {
    let calendar = Calendar::parse(
        b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:rare@example.test\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART:20240101T090000Z\r\n\
RRULE:FREQ=DAILY;COUNT=10\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .unwrap();
    let policy = ResourcePolicy::secure().with_occurrences_per_search(1);
    let engine = Engine::builder().resource_policy(policy).build();
    let mut session = engine.session();
    let window = Window::new(january_2024(1, 0), january_2024(20, 0)).unwrap();
    let mut occurrences = calendar
        .occurrences(&mut session, "rare@example.test", window)
        .unwrap();

    assert!(occurrences.try_next().unwrap().is_some());
    assert_eq!(
        occurrences.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
}

#[test]
fn cursors_cannot_cross_between_rule_and_calendar_streams() {
    let rule = Rule::parse("FREQ=DAILY;COUNT=1").unwrap();
    let window = Window::new(at(NEW_YEAR_2024), at(NEW_YEAR_2024 + 86_400)).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let rule_cursor = rule
        .occurrences(&mut session, at(NEW_YEAR_2024), window)
        .unwrap()
        .cursor();
    let calendar = Calendar::parse(
        b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:cursor@example.test\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART:20240101T000000Z\r\n\
RRULE:FREQ=DAILY;COUNT=1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .unwrap();

    let error = calendar
        .resume_occurrences(&mut session, "cursor@example.test", window, rule_cursor)
        .unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.recurrence.cursor-mismatch");
}

#[test]
fn calendar_occurrences_resolve_tzid_through_the_session_zone_database() {
    let calendar = Calendar::parse(
        b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:zoned@example.test\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART;TZID=Test/Zone:20240101T090000\r\n\
RRULE:FREQ=DAILY;COUNT=2\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .unwrap();
    let window = Window::new(january_2024(1, 0), january_2024(3, 0)).unwrap();

    let default_engine = Engine::default();
    let mut unresolved_session = default_engine.session();
    let mut unresolved = calendar
        .occurrences(&mut unresolved_session, "zoned@example.test", window)
        .unwrap();
    assert_eq!(
        unresolved.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.zone-unresolved"
    );

    let engine = Engine::builder().zone_database(TestZone).build();
    let mut session = engine.session();
    let mut occurrences = calendar
        .occurrences(&mut session, "zoned@example.test", window)
        .unwrap();
    assert_eq!(
        occurrences.try_next().unwrap().unwrap().start(),
        january_2024(1, 9)
    );
    assert_eq!(
        occurrences.try_next().unwrap().unwrap().start(),
        january_2024(2, 9)
    );
    assert!(occurrences.try_next().unwrap().is_none());
}
