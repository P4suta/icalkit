// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone rules use the shared secure value ceiling from ADR 0010.

#![cfg(test)]

use icalkit::recurrence::Rule;
use icalkit_conformance::internal::core::Limits;

fn ceiling() -> usize {
    usize::try_from(Limits::DEFAULT.max_value_bytes()).unwrap()
}

fn sized_rule(prefix: &str, bytes: usize, padding: &str) -> String {
    let remaining = bytes.checked_sub(prefix.len()).unwrap();
    let repetitions = remaining.checked_div(padding.len()).unwrap();
    let remainder = remaining.checked_rem(padding.len()).unwrap();
    let mut value = String::with_capacity(bytes);
    value.push_str(prefix);
    value.push_str(&padding.repeat(repetitions));
    value.push_str(&"x".repeat(remainder));
    assert_eq!(value.len(), bytes);
    value
}

fn refused(value: &str) {
    let Err(error) = Rule::parse(value) else {
        panic!("accepted a rule with {} bytes", value.len());
    };
    assert_eq!(error.code().as_str(), "icalkit.recurrence.resource-limit");
    assert_eq!(error.issues().len(), 1);
    assert!(error.issues().iter().all(icalkit::Issue::is_error));
}

#[test]
fn the_secure_value_ceiling_is_inclusive() {
    let value = sized_rule("FREQ=DAILY;X-PAYLOAD=", ceiling(), "x");
    let rule = Rule::parse(&value).unwrap();
    assert_eq!(rule.issues().len(), 1);
    assert!(rule.issues().iter().all(icalkit::Issue::is_note));
}

#[test]
fn an_ascii_rule_beyond_the_value_ceiling_is_refused() {
    let bytes = ceiling().checked_add(1).unwrap();
    refused(&sized_rule("FREQ=DAILY;X-PAYLOAD=", bytes, "x"));
}

#[test]
fn utf8_values_are_bounded_by_bytes() {
    let value = sized_rule("FREQ=DAILY;X-PAYLOAD=", ceiling(), "\u{00e9}");
    assert!(value.chars().count() < ceiling());
    assert!(Rule::parse(&value).is_ok());
    let bytes = ceiling().checked_add(1).unwrap();
    let value = sized_rule("FREQ=DAILY;X-PAYLOAD=", bytes, "\u{00e9}");
    assert!(value.chars().count() < ceiling());
    refused(&value);
}

#[test]
fn resource_admission_precedes_syntax_and_calendar_semantics() {
    let bytes = ceiling().checked_add(1).unwrap();
    for prefix in [
        "FREQ=UNKNOWN;X-PAYLOAD=",
        "RSCALE=CHINESE;FREQ=YEARLY;X-PAYLOAD=",
        "FREQ=DAILY;COUNT=0;X-PAYLOAD=",
    ] {
        refused(&sized_rule(prefix, bytes, "x"));
    }
}

#[test]
fn dense_diagnostics_cannot_bypass_admission() {
    let part = ";X-NOTE=1";
    let repetitions = ceiling()
        .checked_div(part.len())
        .unwrap()
        .checked_add(1)
        .unwrap();
    let value = format!("FREQ=DAILY{}", part.repeat(repetitions));
    assert!(value.len() > ceiling());
    refused(&value);
}

#[test]
fn admitted_rules_keep_their_validation_diagnostics() {
    let rule = Rule::parse("FREQ=DAILY;X-NOTE=1").unwrap();
    assert_eq!(rule.issues().len(), 1);
    assert!(rule.issues().iter().all(icalkit::Issue::is_note));
    let error = Rule::parse("FREQ=UNKNOWN").unwrap_err();
    assert_eq!(error.code().as_str(), "icalkit.recurrence.invalid-rule");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "malformed-recurrence-rule")
    );
}
