// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence validation follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::Engine;
use icalkit::interop::Import;
use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;

fn calendar(start: &str, rule: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit recurrence validation//EN\r\n\
         BEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         {start}\r\n{rule}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn assert_valid(source: &str) {
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}

fn assert_rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid recurrence: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error() && issue.code().as_str() == "icalkit.validation.invalid-recurrence"
    }));
    let imported = Import::read(source.as_bytes()).unwrap();
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn malformed_rules_fail_calendar_promotion() {
    for rule in [
        "RRULE:invalid",
        "RRULE:COUNT=2",
        "RRULE:FREQ=DAILY;COUNT=0",
        "RRULE:FREQ=DAILY;COUNT=2;UNTIL=20260820T090000Z",
        "RRULE:FREQ=DAILY;BYDAY=2MO",
        "RRULE;VALUE=TEXT:FREQ=DAILY",
    ] {
        assert_rejected(&calendar("DTSTART:20260814T090000Z", rule));
    }
    assert_valid(&calendar(
        "DTSTART:20260814T090000Z",
        "RRULE:COUNT=2;FREQ=DAILY",
    ));
}

#[test]
fn until_matches_the_start_value_type_and_clock() {
    for (start, valid, invalid) in [
        (
            "DTSTART;VALUE=DATE:20260814",
            "20260820",
            "20260820T090000Z",
        ),
        (
            "DTSTART:20260814T090000Z",
            "20260820T090000Z",
            "20260820T090000",
        ),
        (
            "DTSTART:20260814T090000",
            "20260820T090000",
            "20260820T090000Z",
        ),
        (
            "DTSTART;TZID=Test/Zone:20260814T090000",
            "20260820T090000Z",
            "20260820T090000",
        ),
    ] {
        assert_rejected(&calendar(
            start,
            &format!("RRULE:FREQ=DAILY;UNTIL={invalid}"),
        ));
        assert_valid(&calendar(start, &format!("RRULE:FREQ=DAILY;UNTIL={valid}")));
    }
}

#[test]
fn date_rules_cannot_specify_clock_selectors() {
    for selector in ["BYHOUR=9", "BYMINUTE=30", "BYSECOND=0"] {
        assert_rejected(&calendar(
            "DTSTART;VALUE=DATE:20260814",
            &format!("RRULE:FREQ=DAILY;{selector}"),
        ));
    }
    assert_valid(&calendar(
        "DTSTART;VALUE=DATE:20260814",
        "RRULE:FREQ=DAILY;COUNT=2",
    ));
}

#[test]
fn observance_until_is_utc_even_though_start_is_local() {
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit recurrence validation//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n",
        "DTSTART:20260814T090000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;UNTIL=20300814T090000Z\r\n",
        "END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    assert_valid(source);
    assert_rejected(&source.replace("UNTIL=20300814T090000Z", "UNTIL=20300814T090000"));
}

#[test]
fn unknown_rule_value_types_and_component_bodies_remain_opaque() {
    assert_valid(&calendar(
        "DTSTART:20260814T090000Z",
        "RRULE;VALUE=X-PRIVATE:opaque",
    ));
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit recurrence validation//EN\r\n",
        "BEGIN:X-CUSTOM\r\nRRULE:opaque\r\nEND:X-CUSTOM\r\nEND:VCALENDAR\r\n",
    );
    assert_valid(source);
}

#[test]
fn standalone_timestamp_searches_require_utc_date_time_bounds() {
    let start = Timestamp::constant(1_704_067_200, 0);
    let window = Window::new(start, Timestamp::constant(1_704_326_400, 0)).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    for until in ["20240103", "20240103T000000"] {
        let rule = Rule::parse(&format!("FREQ=DAILY;UNTIL={until}")).unwrap();
        let Err(error) = rule.occurrences(&mut session, start, window) else {
            panic!("accepted non-UTC bound {until}");
        };
        assert_eq!(error.code().as_str(), "icalkit.recurrence.invalid-input");
    }
}

#[test]
fn standalone_utc_until_remains_inclusive() {
    let start = Timestamp::constant(1_704_067_200, 0);
    let window = Window::new(start, Timestamp::constant(1_704_326_400, 0)).unwrap();
    let rule = Rule::parse("FREQ=DAILY;UNTIL=20240103T000000Z").unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut search = rule.occurrences(&mut session, start, window).unwrap();
    let mut count = 0;
    while search.try_next().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 3);
}
