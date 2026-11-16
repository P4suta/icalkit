// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Temporal lists follow [RFC 5545 sections 3.8.5.1 and 3.8.5.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.1).
//! Periods follow [section 3.3.9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.9) and [section 3.8.2.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.6).
//! Unknown value types follow [section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

#![cfg(test)]

use icalkit::caldav::Query;
use icalkit::interop::Import;
use icalkit::recurrence::Window;
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};
use icalkit_conformance::internal::core::{Document, Instant, Limits, Meter};
use icalkit_conformance::internal::dav::{FreeBusyQuery, TimeRange};
use icalkit_conformance::internal::query::{Budget, Placement, Undecided, free_busy};

fn calendar(kind: &str, property: &str) -> String {
    let property = property.replace(',', ",\r\n ");
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit temporal lists//EN\r\n\
         BEGIN:{kind}\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         DTSTART:20260814T090000Z\r\n{property}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n"
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
        panic!("accepted invalid temporal list: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error() && issue.code().as_str() == "icalkit.validation.invalid-date-time-list"
    }));
    let imported = Import::read(source.as_bytes()).unwrap();
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn every_date_list_field_must_match_its_declared_type() {
    for name in ["RDATE", "EXDATE"] {
        for value in [
            ":20260815",
            ";VALUE=DATE:20260815T090000Z",
            ";VALUE=DATE:20260815,20260816T090000Z",
            ":20260815T090000Z,20260816",
            ";VALUE=TEXT:20260815T090000Z",
        ] {
            assert_rejected(&calendar("VEVENT", &format!("{name}{value}")));
        }
    }
    assert_rejected(&calendar(
        "VEVENT",
        "EXDATE;VALUE=PERIOD:20260815T090000Z/PT1H",
    ));
}

#[test]
fn every_date_list_field_must_be_a_valid_date_or_date_time() {
    for name in ["RDATE", "EXDATE"] {
        for value in [
            "",
            "20260815T090000Z,",
            ",20260815T090000Z",
            "20260815T090000Z,,20260816T090000Z",
            "20260815T090000Z,invalid",
            "20260230T090000Z",
            "20260815T250000Z",
        ] {
            assert_rejected(&calendar("VEVENT", &format!("{name}:{value}")));
        }
    }
}

#[test]
fn date_list_parameters_must_not_repeat_or_contradict_the_values() {
    for name in ["RDATE", "EXDATE"] {
        for value in [
            ";VALUE=DATE-TIME;VALUE=DATE-TIME:20260815T090000Z",
            ";TZID=Test/Zone;TZID=Test/Zone:20260815T090000",
            ";TZID=:20260815T090000",
            ";TZID=Test/Zone:20260815T090000Z",
            ";TZID=Test/Zone:20260815T090000,20260816T090000Z",
            ";VALUE=DATE;TZID=Test/Zone:20260815",
        ] {
            assert_rejected(&calendar("VEVENT", &format!("{name}{value}")));
        }
    }
}

#[test]
fn recurrence_periods_require_valid_bounds_and_positive_lengths() {
    for value in [
        "",
        "20260815T090000Z",
        "20260815/PT1H",
        "20260815T090000Z/20260815T090000Z",
        "20260815T100000Z/20260815T090000Z",
        "20260815T090000/20260815T080000",
        "20260815T090000Z/PT0S",
        "20260815T090000Z/-PT1H",
        "20260815T090000Z/PT1H,",
    ] {
        assert_rejected(&calendar("VEVENT", &format!("RDATE;VALUE=PERIOD:{value}")));
    }
}

#[test]
fn freebusy_periods_require_utc_bounds_and_positive_lengths() {
    for value in [
        ":",
        ":20260815T090000/PT1H",
        ":20260815T090000Z/20260815T100000",
        ":20260815T090000Z/20260815T080000Z",
        ":20260815T090000Z/20260815T090000Z",
        ":20260815T090000Z/PT0S",
        ":20260815T090000Z/-PT1H",
        ":20260815T090000Z/PT1H,invalid",
        ";VALUE=DATE:20260815",
        ";FBTYPE=BUSY;FBTYPE=FREE:20260815T090000Z/PT1H",
    ] {
        assert_rejected(&calendar("VFREEBUSY", &format!("FREEBUSY{value}")));
    }
}

#[test]
fn valid_temporal_list_forms_keep_their_original_bytes() {
    for name in ["RDATE", "EXDATE"] {
        for value in [
            ":20260815T090000Z,20260816T090000Z",
            ":20260815T090000,20260816T090000",
            ";VALUE=DATE:20260815,20260816",
            ";TZID=Test/Zone:20260815T090000,20260816T090000",
        ] {
            assert_valid(&calendar("VEVENT", &format!("{name}{value}")));
        }
    }
    for value in [
        "20260815T090000Z/20260815T100000Z,20260816T090000Z/PT1H",
        "20260815T090000/20260815T100000,20260816T090000/P1D",
    ] {
        assert_valid(&calendar("VEVENT", &format!("RDATE;VALUE=PERIOD:{value}")));
    }
    assert_valid(&calendar(
        "VEVENT",
        "RDATE;VALUE=PERIOD;TZID=Test/Zone:20260815T090000/PT1H",
    ));
    assert_valid(&calendar(
        "VFREEBUSY",
        "FREEBUSY;FBTYPE=BUSY:20260815T090000Z/20260815T100000Z,20260816T090000Z/PT1H",
    ));
}

#[test]
fn unknown_list_value_types_keep_their_opaque_content() {
    for (kind, name) in [
        ("VEVENT", "RDATE"),
        ("VEVENT", "EXDATE"),
        ("VFREEBUSY", "FREEBUSY"),
    ] {
        assert_valid(&calendar(kind, &format!("{name};VALUE=X-PRIVATE:opaque,,")));
    }
}

#[test]
fn unknown_list_value_types_cannot_influence_occurrence_expansion() {
    let start = Timestamp::constant(1_786_656_000, 0);
    let end = Timestamp::constant(1_787_001_600, 0);
    let window = Window::new(start, end).unwrap();
    for name in ["RDATE", "EXDATE"] {
        let source = calendar(
            "VEVENT",
            &format!("{name};VALUE=X-PRIVATE:20260815T090000Z"),
        );
        let calendar = Calendar::parse(source.as_bytes()).unwrap();
        let engine = Engine::default();
        let mut session = engine.session();
        assert!(
            calendar
                .occurrences(&mut session, "one@example.test", window)
                .is_err(),
            "interpreted {name} with an unknown value type"
        );
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn unknown_freebusy_value_types_cannot_be_trimmed_by_projection() {
    let source = calendar(
        "VFREEBUSY",
        "FREEBUSY;VALUE=X-PRIVATE:20260814T090000Z/PT1H,20260815T090000Z/PT1H",
    );
    let query = Query::parse(
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>
<C:limit-freebusy-set start="20260815T000000Z" end="20260816T000000Z"/>
</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#,
    )
    .unwrap();
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let projected = query.project(&mut session, &calendar).unwrap();
    assert_eq!(projected.as_bytes(), source.as_bytes());
}

#[test]
fn unknown_freebusy_value_types_cannot_contribute_inferred_busy_time() {
    let source = calendar(
        "VFREEBUSY",
        "FREEBUSY;VALUE=X-PRIVATE:20260815T090000Z/PT1H",
    );
    let document = Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let component = document
        .components()
        .next()
        .unwrap()
        .components()
        .next()
        .unwrap();
    let placement = Placement {
        component,
        instances: &[],
        incomplete: None,
    };
    let query = FreeBusyQuery {
        range: TimeRange::new(
            Some(Instant::from_unix_seconds(1_786_656_000)),
            Some(Instant::from_unix_seconds(1_787_001_600)),
        )
        .unwrap(),
    };
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
    let answer = free_busy(query, &[placement], &mut budget).unwrap();
    assert!(!answer.is_complete());
    assert_eq!(answer.unplaced().len(), 1);
    assert_eq!(answer.unplaced()[0].reason, Undecided::ValueUnreadable);
    assert_eq!(answer.report().periods(), []);
    assert_eq!(document.to_bytes(), source.as_bytes());
}
