// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar controls follow [RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4).
//! Gregorian aliases follow the [Unicode calendar registry](https://raw.githubusercontent.com/unicode-org/cldr/main/common/bcp47/calendar.xml).

#![cfg(test)]

use icalkit::interop::Import;
use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};
use icalkit_conformance::internal::core::{
    DecodeValue, Diagnostic, DiagnosticSink, IgnoreDiagnostics, Limits, Meter, Severity,
    SinkOutcome,
};
use icalkit_conformance::internal::recur::{RecurrenceRule, parse_recur};

const UNSUPPORTED: &[&str] = &[
    "FREQ=YEARLY;RSCALE=CHINESE",
    "RSCALE=HEBREW;FREQ=YEARLY",
    "FREQ=DAILY;RSCALE=JAPANESE",
    "FREQ=YEARLY;RSCALE=ISO8601",
    "FREQ=DAILY;RSCALE=X-FUTURE",
    "FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=BACKWARD",
    "RSCALE=GREGORY;SKIP=FORWARD;FREQ=MONTHLY",
    "FREQ=YEARLY;RSCALE=GREGORIAN;BYMONTH=2L",
    "FREQ=YEARLY;RSCALE=GREGORY;BYMONTH=1,2l",
];

const INVALID: &[(&str, &str)] = &[
    ("FREQ=DAILY;RSCALE=", "malformed-recurrence-rule"),
    ("FREQ=DAILY;RSCALE=UNDER_SCORE", "malformed-recurrence-rule"),
    (
        "FREQ=DAILY;RSCALE=\"GREGORIAN\"",
        "malformed-recurrence-rule",
    ),
    ("FREQ=DAILY;SKIP=OMIT", "malformed-recurrence-rule"),
    ("FREQ=DAILY;SKIP=FORWARD", "malformed-recurrence-rule"),
    (
        "FREQ=DAILY;RSCALE=GREGORIAN;SKIP=",
        "malformed-recurrence-rule",
    ),
    (
        "RSCALE=GREGORIAN;SKIP=UPWARD;FREQ=DAILY",
        "malformed-recurrence-rule",
    ),
    (
        "RSCALE=GREGORIAN;rscale=GREGORY;FREQ=DAILY",
        "duplicate-recurrence-rule-part",
    ),
    (
        "RSCALE=CHINESE;RSCALE=GREGORIAN;FREQ=DAILY",
        "duplicate-recurrence-rule-part",
    ),
    (
        "SKIP=OMIT;skip=OMIT;RSCALE=GREGORIAN;FREQ=DAILY",
        "duplicate-recurrence-rule-part",
    ),
];

const NATIVE: &[&str] = &[
    "FREQ=MONTHLY;COUNT=3;RSCALE=GREGORIAN",
    "RSCALE=GREGORY;FREQ=MONTHLY;COUNT=3",
    "COUNT=3;RSCALE=GREGORY;FREQ=MONTHLY",
    "COUNT=3;FREQ=MONTHLY",
    "SKIP=OMIT;RSCALE=GREGORIAN;FREQ=MONTHLY;COUNT=3",
    "rscale=gregory;skip=omit;freq=monthly;count=3",
];

fn calendar(properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit calendar controls//EN\r\n\
         BEGIN:VEVENT\r\nUID:calendar@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240131T090000Z\r\n{properties}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn rejected_rule(value: &str, expected: &str) {
    let Err(error) = Rule::parse(value) else {
        panic!("accepted calendar controls: {value}");
    };
    assert_eq!(error.code().as_str(), "icalkit.recurrence.invalid-rule");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error() && issue.code().as_str() == expected),
        "missing {expected}: {error:?}"
    );
}

fn rejected_calendar(value: &str, expected: &str) {
    let source = calendar(&format!("RRULE:{value}"));
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("promoted calendar controls: {value}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error() && issue.code().as_str() == expected),
        "missing {expected}: {error:?}"
    );
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn public_rules_refuse_unsupported_calendar_semantics() {
    for value in UNSUPPORTED {
        rejected_rule(value, "unsupported-recurrence-semantics");
    }
}

#[test]
fn public_rules_report_malformed_and_repeated_controls() {
    for (value, code) in INVALID {
        rejected_rule(value, code);
    }
}

#[test]
fn calendar_promotion_refuses_unsupported_semantics_without_changing_imports() {
    for value in UNSUPPORTED {
        rejected_calendar(value, "unsupported-recurrence-semantics");
    }
    for (value, code) in INVALID {
        rejected_calendar(value, code);
    }
}

#[test]
fn typed_kernel_reading_recognizes_native_calendar_controls() {
    for value in NATIVE {
        assert!(
            RecurrenceRule::decode_value(value.as_bytes()).is_ok(),
            "{value}"
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut diagnostics = Vec::new();
        assert!(parse_recur(value.as_bytes(), &mut meter, &mut diagnostics).is_ok());
        assert_eq!(diagnostics, []);
    }
}

#[test]
fn typed_kernel_reading_identifies_control_failures() {
    for (value, expected) in UNSUPPORTED
        .iter()
        .map(|value| (*value, "unsupported-recurrence-semantics"))
        .chain(INVALID.iter().copied())
    {
        assert_eq!(
            RecurrenceRule::decode_value(value.as_bytes())
                .unwrap_err()
                .as_str(),
            expected,
            "{value}"
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut diagnostics = Vec::new();
        assert!(
            parse_recur(value.as_bytes(), &mut meter, &mut diagnostics).is_err(),
            "{value}"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code().as_str() == expected
                    && diagnostic.severity() == Severity::Violation),
            "{value}: {diagnostics:?}"
        );
    }
}

struct Refusing;

impl DiagnosticSink for Refusing {
    fn push(&mut self, _diagnostic: Diagnostic) -> SinkOutcome {
        SinkOutcome::Refused
    }
}

#[test]
fn discarding_or_refusing_diagnostics_cannot_enable_unsupported_semantics() {
    for value in UNSUPPORTED {
        let mut meter = Meter::new(Limits::DEFAULT);
        assert!(
            parse_recur(value.as_bytes(), &mut meter, &mut IgnoreDiagnostics).is_err(),
            "{value}"
        );
        let mut refused = Refusing;
        assert!(
            parse_recur(value.as_bytes(), &mut meter, &mut refused).is_err(),
            "{value}"
        );
        assert_ne!(meter.diagnostics_dropped(), 0);
    }
}

#[test]
fn native_omission_preserves_rules_and_calendar_occurrences() {
    let start: Timestamp = "2024-01-31T09:00:00Z".parse().unwrap();
    let end: Timestamp = "2024-06-01T00:00:00Z".parse().unwrap();
    let expected: Vec<Timestamp> = [
        "2024-01-31T09:00:00Z",
        "2024-03-31T09:00:00Z",
        "2024-05-31T09:00:00Z",
    ]
    .into_iter()
    .map(|value| value.parse().unwrap())
    .collect();
    for value in NATIVE {
        let rule = Rule::parse(value).unwrap();
        assert_eq!(rule.issues(), []);
        let source = calendar(&format!("RRULE:{value}"));
        let calendar = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(calendar.to_bytes(), source.as_bytes());
        let engine = Engine::default();
        let mut session = engine.session();
        let mut occurrences = rule
            .occurrences(&mut session, start, Window::new(start, end).unwrap())
            .unwrap();
        let mut starts = Vec::new();
        while let Some(occurrence) = occurrences.try_next().unwrap() {
            starts.push(occurrence.start());
        }
        assert_eq!(starts, expected, "{value}");
        drop(occurrences);
        let mut session = engine.session();
        let mut occurrences = calendar
            .occurrences(
                &mut session,
                "calendar@example.test",
                Window::new(start, end).unwrap(),
            )
            .unwrap();
        let mut starts = Vec::new();
        while let Some(occurrence) = occurrences.try_next().unwrap() {
            starts.push(occurrence.start());
        }
        assert_eq!(starts, expected, "{value}");
    }
}

#[test]
fn unrelated_extensions_and_opaque_values_keep_their_existing_boundaries() {
    let rule = Rule::parse("FREQ=DAILY;X-RSCALE=CHINESE;X-SKIP=FORWARD").unwrap();
    assert_eq!(rule.issues().len(), 2);
    assert!(rule.issues().iter().all(icalkit::Issue::is_note));
    for properties in [
        "RRULE;VALUE=X-FUTURE:FREQ=YEARLY;RSCALE=CHINESE",
        "BEGIN:X-OUTER\r\nRRULE:FREQ=YEARLY;RSCALE=CHINESE\r\nEND:X-OUTER",
        "BEGIN:X-OUTER\r\nBEGIN:VTODO\r\nRRULE:FREQ=YEARLY;RSCALE=CHINESE\r\nEND:VTODO\r\nEND:X-OUTER",
        "X-RRULE:FREQ=YEARLY;RSCALE=CHINESE",
    ] {
        let source = calendar(properties);
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(parsed.to_bytes(), source.as_bytes());
        let imported = Import::read(source.as_bytes()).unwrap();
        assert_eq!(imported.validate().unwrap().to_bytes(), source.as_bytes());
    }
}
