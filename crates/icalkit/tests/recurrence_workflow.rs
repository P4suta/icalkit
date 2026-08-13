// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence through the unified Jiff-based public boundary.

use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Engine, ResourcePolicy};

const NEW_YEAR_2024: i64 = 1_704_067_200;

fn at(seconds: i64) -> Timestamp {
    Timestamp::constant(seconds, 0)
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
