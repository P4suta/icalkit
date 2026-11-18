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
fn a_cursor_can_be_copied_sent_and_shared() {
    fn check<T: Copy + Send + Sync>() {}
    check::<icalkit::recurrence::Cursor>();
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
fn calendar_cursors_resume_in_start_and_key_order_across_windows() {
    let calendar = Calendar::parse(concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit cursor tests//EN\r\n",
        "BEGIN:VEVENT\r\nUID:one\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:20240102T090000Z\r\n",
        "RDATE:20240111T090000Z,20240112T090000Z\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:one\r\nDTSTAMP:20240101T000000Z\r\nRECURRENCE-ID:20240112T090000Z\r\n",
        "DTSTART:20240102T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ).as_bytes()).unwrap();
    let first = Window::new(january_2024(1, 0), january_2024(10, 0)).unwrap();
    let empty = Window::new(first.end(), january_2024(11, 0)).unwrap();
    let later = Window::new(empty.end(), january_2024(14, 0)).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = calendar.occurrences(&mut session, "one", first).unwrap();
    assert_eq!(
        occurrences.try_next().unwrap().unwrap().key(),
        january_2024(2, 9)
    );
    let cursor = occurrences.cursor();
    drop(occurrences);
    let mut same = calendar
        .resume_occurrences(&mut session, "one", first, cursor)
        .unwrap();
    let tied = same.try_next().unwrap().unwrap();
    assert_eq!(
        (tied.start(), tied.key()),
        (january_2024(2, 9), january_2024(12, 9))
    );
    assert_eq!(same.try_next().unwrap(), None);
    let cursor = same.cursor();
    drop(same);

    let mut no_events = calendar
        .resume_occurrences(&mut session, "one", empty, cursor)
        .unwrap();
    assert_eq!(no_events.try_next().unwrap(), None);
    assert_eq!(no_events.cursor(), cursor);
    drop(no_events);
    let mut next = calendar
        .resume_occurrences(&mut session, "one", later, cursor)
        .unwrap();
    assert_eq!(next.try_next().unwrap().unwrap().key(), january_2024(11, 9));
    assert_eq!(next.try_next().unwrap(), None);
}

#[test]
fn missing_calendar_uids_spend_the_lookup_budget() {
    let bounds = Window::new(january_2024(1, 0), january_2024(2, 0)).unwrap();
    for (uid, first_code) in [
        ("a", "icalkit.recurrence.component-not-found"),
        (
            r"an\,escaped\,identifier\,longer\,than\,the\,budget",
            "icalkit.recurrence.budget-exhausted",
        ),
    ] {
        let source = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//tests//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:20240101T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let calendar = Calendar::parse(source.as_bytes()).unwrap();
        let engine = Engine::builder()
            .resource_policy(ResourcePolicy::secure().with_max_input_bytes(32))
            .build();
        let mut session = engine.session();
        let mut lookup = || {
            calendar
                .occurrences(&mut session, "absent", bounds)
                .unwrap_err()
        };
        assert_eq!(lookup().code().as_str(), first_code);
        assert!((0..32).any(|_| lookup().code().as_str() == "icalkit.recurrence.budget-exhausted"));
    }
}

#[test]
fn exhausted_sessions_keep_the_budget_error_for_valid_calendars_and_rules() {
    let calendar = Calendar::parse(
        concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n",
            "BEGIN:VEVENT\r\nUID:limit@example.test\r\nDTSTAMP:20240101T000000Z\r\n",
            "DTSTART:20240101T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .as_bytes(),
    )
    .unwrap();
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_occurrences_per_search(0))
        .build();
    let mut session = engine.session();
    let bounds = Window::new(january_2024(1, 0), january_2024(2, 0)).unwrap();
    let rule = Rule::parse("FREQ=DAILY;COUNT=1").unwrap();
    let mut search = rule
        .occurrences(&mut session, january_2024(1, 9), bounds)
        .unwrap();
    assert_eq!(
        search.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
    drop(search);
    let error = calendar
        .occurrences(&mut session, "limit@example.test", bounds)
        .unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.recurrence.budget-exhausted");
    let error = rule
        .occurrences(&mut session, january_2024(1, 9), bounds)
        .unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.recurrence.budget-exhausted");
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
