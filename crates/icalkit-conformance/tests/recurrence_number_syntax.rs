// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Numeric selectors follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Leap month markers follow [RFC 7529 section 4](https://www.rfc-editor.org/rfc/rfc7529.html#section-4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;
use icalkit::recurrence::Rule;
use icalkit_conformance::internal::core::{
    DecodeValue, Diagnostic, DiagnosticSink, IgnoreDiagnostics, Limits, Meter, Severity,
    SinkOutcome,
};
use icalkit_conformance::internal::recur::{RecurrenceRule, RuleLimit, RulePart, parse_recur};

struct NumberCase {
    part: RulePart,
    legal: &'static str,
    too_wide: &'static str,
    outside: &'static str,
}

const CASES: [NumberCase; 9] = [
    NumberCase {
        part: RulePart::Second,
        legal: "BYSECOND=00",
        too_wide: "BYSECOND=000",
        outside: "BYSECOND=61",
    },
    NumberCase {
        part: RulePart::Minute,
        legal: "BYMINUTE=00",
        too_wide: "BYMINUTE=000",
        outside: "BYMINUTE=60",
    },
    NumberCase {
        part: RulePart::Hour,
        legal: "BYHOUR=09",
        too_wide: "BYHOUR=009",
        outside: "BYHOUR=24",
    },
    NumberCase {
        part: RulePart::Day,
        legal: "BYDAY=01MO",
        too_wide: "BYDAY=001MO",
        outside: "BYDAY=54MO",
    },
    NumberCase {
        part: RulePart::MonthDay,
        legal: "BYMONTHDAY=01",
        too_wide: "BYMONTHDAY=001",
        outside: "BYMONTHDAY=32",
    },
    NumberCase {
        part: RulePart::YearDay,
        legal: "BYYEARDAY=001",
        too_wide: "BYYEARDAY=0001",
        outside: "BYYEARDAY=367",
    },
    NumberCase {
        part: RulePart::WeekNo,
        legal: "BYWEEKNO=01",
        too_wide: "BYWEEKNO=001",
        outside: "BYWEEKNO=54",
    },
    NumberCase {
        part: RulePart::Month,
        legal: "BYMONTH=01",
        too_wide: "BYMONTH=001",
        outside: "BYMONTH=13",
    },
    NumberCase {
        part: RulePart::SetPos,
        legal: "BYSETPOS=001;BYHOUR=09",
        too_wide: "BYSETPOS=0001;BYHOUR=09",
        outside: "BYSETPOS=367;BYHOUR=09",
    },
];

fn rule(parts: &str) -> String {
    format!("FREQ=YEARLY;COUNT=2;{parts}")
}

fn calendar(property: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit numeric syntax//EN\r\n\
         BEGIN:VEVENT\r\nUID:numbers@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\n{property}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn strict_refusal(parts: &str, code: &str) {
    let text = rule(parts);
    let error = RecurrenceRule::decode_value(text.as_bytes()).unwrap_err();
    assert_eq!(error.as_str(), code, "{text}");
}

#[test]
fn selector_digit_widths_are_grammar_constraints() {
    for case in CASES {
        strict_refusal(case.too_wide, "malformed-recurrence-rule");
    }
    for parts in [
        "BYDAY=+001MO",
        "BYDAY=-001MO",
        "BYMONTHDAY=+001",
        "BYMONTHDAY=-001",
        "BYYEARDAY=+0001",
        "BYYEARDAY=-0001",
        "BYWEEKNO=+001",
        "BYWEEKNO=-001",
        "BYSETPOS=+0001;BYHOUR=9",
        "BYSETPOS=-0001;BYHOUR=9",
        "RSCALE=HEBREW;BYMONTH=005L",
    ] {
        strict_refusal(parts, "malformed-recurrence-rule");
    }
}

#[test]
fn malformed_spellings_are_distinct_from_numeric_range_failures() {
    for parts in [
        "BYSECOND=+01",
        "BYMINUTE=-01",
        "BYHOUR=+09",
        "BYHOUR= 09",
        "BYDAY=+MO",
        "BYDAY=MON",
        "BYMONTHDAY=1.0",
        "BYYEARDAY=1e2",
        "BYWEEKNO=MO",
        "BYMONTH=+01",
        "BYMONTH=01L",
        "BYMONTHDAY=０１",
        "BYHOUR=",
        "BYMONTHDAY=1,,2",
        "RSCALE=HEBREW;BYMONTH=L",
        "RSCALE=HEBREW;BYMONTH=5LL",
    ] {
        strict_refusal(parts, "malformed-recurrence-rule");
    }
    for case in CASES {
        strict_refusal(case.outside, "recurrence-rule-part-out-of-range");
    }
}

#[test]
fn permitted_padding_signs_and_leap_markers_stay_valid() {
    for parts in CASES.iter().map(|case| case.legal).chain([
        "BYSECOND=60",
        "BYMONTHDAY=+01",
        "BYMONTHDAY=-01",
        "BYYEARDAY=+001",
        "BYYEARDAY=-001",
        "BYWEEKNO=+01",
        "BYWEEKNO=-01",
        "BYDAY=+01MO",
        "BYDAY=-01MO",
        "BYSETPOS=+001;BYHOUR=09",
        "BYSETPOS=-001;BYHOUR=09",
        "RSCALE=HEBREW;BYMONTH=05L",
        "RSCALE=HEBREW;BYMONTH=05l",
        "RSCALE=HEBREW;BYYEARDAY=385",
        "RSCALE=HEBREW;BYDAY=55MO",
        "RSCALE=ETHIOPIC;BYMONTH=13",
    ]) {
        let text = rule(parts);
        assert!(
            RecurrenceRule::decode_value(text.as_bytes()).is_ok(),
            "{text}"
        );
        assert!(Rule::parse(&text).is_ok(), "{text}");
    }
}

#[test]
fn scalar_digit_counts_do_not_inherit_selector_limits() {
    let text = "FREQ=YEARLY;COUNT=0000000000000000002;INTERVAL=0000000000000001";
    let canonical = RecurrenceRule::decode_value(b"FREQ=YEARLY;COUNT=2;INTERVAL=1").unwrap();
    assert_eq!(
        RecurrenceRule::decode_value(text.as_bytes()).unwrap(),
        canonical
    );
    let source = calendar(&format!("RRULE:{text}"));
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}

#[test]
fn recovering_decoders_drop_bad_parts_and_preserve_their_diagnostics() {
    for case in CASES {
        let text = rule(case.too_wide);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut diagnostics = Vec::new();
        let recovered = parse_recur(text.as_bytes(), &mut meter, &mut diagnostics).unwrap();
        assert!(!recovered.has_part(case.part), "{text}");
        assert!(matches!(recovered.limit(), RuleLimit::Count(count) if count.get() == 2));
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.code().as_str(), diagnostic.severity()))
                .collect::<Vec<_>>(),
            [("recurrence-rule-part-out-of-range", Severity::Violation)],
            "{text}"
        );
    }
    for parts in [
        "BYMONTHDAY=001,2",
        "BYMONTHDAY=1,002",
        "BYMONTHDAY=01,003,02",
    ] {
        let text = rule(&format!("{parts};BYHOUR=09"));
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut diagnostics = Vec::new();
        let recovered = parse_recur(text.as_bytes(), &mut meter, &mut diagnostics).unwrap();
        assert_eq!(recovered.by_month_day().as_slice(), [], "{text}");
        assert_eq!(recovered.by_hour().as_slice(), [9], "{text}");
        assert_eq!(diagnostics.len(), 1, "{text}");
    }
}

struct Refusing;

impl DiagnosticSink for Refusing {
    fn push(&mut self, _diagnostic: Diagnostic) -> SinkOutcome {
        SinkOutcome::Refused
    }
}

#[test]
fn diagnostic_retention_does_not_control_numeric_validation() {
    for case in CASES {
        let text = rule(case.too_wide);
        for sink in [
            &mut IgnoreDiagnostics as &mut dyn DiagnosticSink,
            &mut Refusing,
        ] {
            let mut meter = Meter::new(Limits::DEFAULT);
            let recovered = parse_recur(text.as_bytes(), &mut meter, sink).unwrap();
            assert!(!recovered.has_part(case.part), "{text}");
            assert_eq!(meter.diagnostics_dropped(), 1, "{text}");
        }
    }
}

#[test]
fn calendar_promotion_refuses_wide_selectors_without_changing_imports() {
    for case in CASES {
        let source = calendar(&format!("RRULE:{}", rule(case.too_wide)));
        let error = Calendar::parse(source.as_bytes()).unwrap_err();
        assert!(
            error.issues().iter().any(|issue| issue.is_error()
                && issue.code().as_str() == "icalkit.validation.invalid-recurrence"),
            "{source}: {error:?}"
        );
        let imported = Import::read(source.as_bytes()).unwrap();
        assert!(imported.validate().is_err(), "{source}");
        assert_eq!(imported.as_bytes(), source.as_bytes());
    }
    for case in CASES {
        let source = calendar(&format!("RRULE:{}", rule(case.legal)));
        assert_eq!(
            Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
            source.as_bytes()
        );
    }
}

#[test]
fn opaque_recurrence_values_do_not_acquire_numeric_semantics() {
    for property in [
        "RRULE;VALUE=X-FUTURE:FREQ=YEARLY;BYMONTHDAY=001",
        "X-RRULE:FREQ=YEARLY;BYMONTHDAY=001",
        "BEGIN:X-FUTURE\r\nRRULE:FREQ=YEARLY;BYMONTHDAY=001\r\nEND:X-FUTURE",
    ] {
        let source = calendar(property);
        assert_eq!(
            Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
            source.as_bytes()
        );
    }
}
