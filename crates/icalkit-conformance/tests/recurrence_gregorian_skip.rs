// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Gregorian skipping follows [RFC 7529 section 4.1](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).

#![cfg(test)]

use icalkit::Calendar;
use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, DecodeValue, IgnoreDiagnostics, Instant, Limits, Meter,
    UtcOffset,
};
use icalkit_conformance::internal::recur::{
    ByList, Freq, OverrideSet, RecurrenceInput, RecurrenceRule, RecurrenceRuleBuilder, RuleError,
    SearchOutcome, SearchStep, Skip, ValueKind, Window as NativeWindow, parse_recur,
};

fn instant(year: u16, month: u8, day: u8) -> Instant {
    CivilDateTime::new(
        CivilDate::from_ymd(year, month, day).unwrap(),
        CivilTime::MIDNIGHT,
    )
    .at_offset(UtcOffset::UTC)
    .unwrap()
}

#[test]
fn strict_and_recovering_readers_keep_skipping_independent_of_part_order() {
    for mode in ["BACKWARD", "FORWARD"] {
        let canonical = format!("FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP={mode};BYMONTHDAY=31");
        let varied = format!("skip={mode};BYMONTHDAY=31;rscale=gregory;freq=monthly");
        let strict = RecurrenceRule::decode_value(canonical.as_bytes()).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let recovered = parse_recur(varied.as_bytes(), &mut meter, &mut IgnoreDiagnostics).unwrap();
        assert_eq!(strict, recovered);
    }
}

#[test]
fn native_builders_keep_year_days_and_refuse_unimplemented_week_numbers() {
    for skip in [Skip::Backward, Skip::Forward] {
        let builder = RecurrenceRuleBuilder::new(Freq::Yearly).skip(skip);
        assert!(
            builder
                .clone()
                .by_year_day(ByList::from_slice(&[366]))
                .build()
                .is_ok(),
        );
        assert_eq!(
            builder.by_week_no(ByList::from_slice(&[53])).build(),
            Err(RuleError::UnsupportedCalendarSemantics),
        );
    }
    assert!(
        RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_year_day(ByList::from_slice(&[366]))
            .build()
            .is_ok()
    );
}

#[test]
fn date_only_anniversaries_use_the_same_correction() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit skip//EN\r\n\
        BEGIN:VEVENT\r\nUID:date@example.test\r\nDTSTAMP:20200101T000000Z\r\n\
        DTSTART;VALUE=DATE:20200229\r\n\
        RRULE:RSCALE=GREGORIAN;FREQ=YEARLY;SKIP=FORWARD;COUNT=3\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let rule =
        RecurrenceRule::decode_value(b"RSCALE=GREGORIAN;FREQ=YEARLY;SKIP=FORWARD;COUNT=3").unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let input = RecurrenceInput::new(
        instant(2020, 2, 29),
        ValueKind::Date,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(2020, 1, 1), instant(2023, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    for expected in [
        instant(2020, 2, 29),
        instant(2021, 3, 1),
        instant(2022, 3, 1),
    ] {
        let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
            panic!("missing corrected date");
        };
        assert_eq!(occurrence.start(), expected);
    }
    assert!(search.next().is_none());
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn correction_work_exhaustion_remains_explicit() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 1);
    let rule =
        RecurrenceRule::decode_value(b"FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTHDAY=31")
            .unwrap();
    let input = RecurrenceInput::new(
        instant(2021, 1, 31),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(2021, 1, 1), instant(2022, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    assert!(matches!(
        search.next(),
        Some(SearchStep::BudgetExhausted(_))
    ));
    assert!(search.next().is_none());
    assert!(matches!(
        search.outcome(),
        SearchOutcome::BudgetExhausted(_)
    ));
}

#[test]
fn local_time_admission_remains_separate_from_calendar_correction() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let rule = RecurrenceRule::decode_value(b"FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;COUNT=3")
        .unwrap();
    let gate = |key| key != instant(2021, 3, 1);
    let input = RecurrenceInput::new(
        instant(2021, 1, 31),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap()
    .admitting(&gate);
    let bounds = NativeWindow::new(instant(2021, 1, 1), instant(2022, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    for expected in [
        instant(2021, 1, 31),
        instant(2021, 3, 31),
        instant(2021, 5, 1),
    ] {
        let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
            panic!("missing admitted occurrence");
        };
        assert_eq!(occurrence.start(), expected);
    }
    assert!(search.next().is_none());
    assert_eq!(search.outcome(), SearchOutcome::RuleEnded);
}

#[test]
fn resuming_after_an_addition_keeps_a_corrected_key_on_offer() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let rule = RecurrenceRule::decode_value(b"FREQ=MONTHLY;RSCALE=GREGORIAN;SKIP=FORWARD;COUNT=2")
        .unwrap();
    let additions = [instant(2021, 2, 15)];
    let input = RecurrenceInput::new(
        instant(2021, 1, 31),
        ValueKind::DateTime,
        Some(&rule),
        &additions,
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(2021, 1, 1), instant(2022, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    for expected in [instant(2021, 1, 31), instant(2021, 2, 15)] {
        let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
            panic!("missing occurrence before continuation");
        };
        assert_eq!(occurrence.start(), expected);
    }
    let cursor = search.cursor();
    drop(search);
    let mut resumed = input.resume(cursor, bounds, &mut meter, &mut sink);
    let Some(SearchStep::Occurrence(occurrence)) = resumed.next() else {
        panic!("lost corrected key during continuation");
    };
    assert_eq!(occurrence.start(), instant(2021, 3, 1));
    assert!(resumed.next().is_none());
    assert_eq!(resumed.outcome(), SearchOutcome::RuleEnded);
}

#[test]
fn a_completed_backward_count_does_not_expand_the_next_year() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 368);
    let rule = RecurrenceRule::decode_value(
        b"FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=BACKWARD;BYMONTH=12;BYMONTHDAY=31;COUNT=1",
    )
    .unwrap();
    let input = RecurrenceInput::new(
        instant(2021, 12, 31),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(2021, 1, 1), instant(2023, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
        panic!("spent a completed count's budget on the next year");
    };
    assert_eq!(occurrence.start(), instant(2021, 12, 31));
    assert!(search.next().is_none());
    assert_eq!(search.outcome(), SearchOutcome::RuleEnded);
}

#[test]
fn both_readers_preserve_signed_year_day_correction() {
    for mode in ["BACKWARD", "FORWARD"] {
        let text = format!("RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY=-366,366;FREQ=YEARLY");
        let strict = RecurrenceRule::decode_value(text.as_bytes()).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let recovered = parse_recur(text.as_bytes(), &mut meter, &mut IgnoreDiagnostics).unwrap();
        assert_eq!(strict, recovered);
        assert_eq!(strict.by_year_day().as_slice(), [-366, 366]);
        assert_ne!(strict.skip(), Skip::Omit);
    }
}

#[test]
fn a_forward_year_day_count_needs_no_extra_year_expansion() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 737);
    let rule = RecurrenceRule::decode_value(
        b"FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=366;COUNT=2",
    )
    .unwrap();
    let input = RecurrenceInput::new(
        instant(2020, 12, 31),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(2020, 1, 1), instant(2025, 1, 1)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    for expected in [instant(2020, 12, 31), instant(2022, 1, 1)] {
        let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
            panic!("spent a year day count's budget on a later year");
        };
        assert_eq!(occurrence.start(), expected);
    }
    assert!(search.next().is_none());
    assert_eq!(search.outcome(), SearchOutcome::RuleEnded);
}

#[test]
fn a_year_day_limit_does_not_generate_an_absent_day() {
    for mode in ["BACKWARD", "FORWARD"] {
        let text = format!("FREQ=HOURLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY=366;COUNT=1");
        let rule = RecurrenceRule::decode_value(text.as_bytes()).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let input = RecurrenceInput::new(
            instant(2021, 12, 31),
            ValueKind::DateTime,
            Some(&rule),
            &[],
            &[],
            OverrideSet::empty(),
            &mut meter,
        )
        .unwrap();
        let bounds = NativeWindow::new(instant(2021, 12, 31), instant(2022, 1, 2)).unwrap();
        let mut sink = IgnoreDiagnostics;
        let mut search = input.search(bounds, &mut meter, &mut sink);
        assert!(search.next().is_none());
        assert_eq!(search.outcome(), SearchOutcome::WindowEnded);
    }
}

#[test]
fn a_year_day_correction_never_saturates_at_the_calendar_end() {
    let rule = RecurrenceRule::decode_value(
        b"FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYYEARDAY=366;COUNT=1",
    )
    .unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let input = RecurrenceInput::new(
        instant(9999, 1, 1),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let bounds = NativeWindow::new(instant(9999, 12, 30), instant(9999, 12, 31)).unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(bounds, &mut meter, &mut sink);
    assert!(search.next().is_none());
    assert_eq!(search.outcome(), SearchOutcome::CalendarEnded);
}

#[test]
fn signed_date_ordinals_correct_at_both_year_boundaries() {
    for (mode, ordinal, first, second) in [
        ("FORWARD", "366", instant(2020, 12, 31), instant(2022, 1, 1)),
        (
            "BACKWARD",
            "366",
            instant(2020, 12, 31),
            instant(2021, 12, 31),
        ),
        ("FORWARD", "-366", instant(2020, 1, 1), instant(2021, 1, 1)),
        (
            "BACKWARD",
            "-366",
            instant(2020, 1, 1),
            instant(2020, 12, 31),
        ),
    ] {
        let text = format!("FREQ=YEARLY;RSCALE=GREGORIAN;SKIP={mode};BYYEARDAY={ordinal};COUNT=2");
        let rule = RecurrenceRule::decode_value(text.as_bytes()).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let input = RecurrenceInput::new(
            first,
            ValueKind::Date,
            Some(&rule),
            &[],
            &[],
            OverrideSet::empty(),
            &mut meter,
        )
        .unwrap();
        let bounds = NativeWindow::new(instant(2020, 1, 1), instant(2024, 1, 1)).unwrap();
        let mut sink = IgnoreDiagnostics;
        let mut search = input.search(bounds, &mut meter, &mut sink);
        for expected in [first, second] {
            let Some(SearchStep::Occurrence(occurrence)) = search.next() else {
                panic!("missing corrected Gregorian year day");
            };
            assert_eq!(occurrence.start(), expected);
        }
        assert!(search.next().is_none());
        assert_eq!(search.outcome(), SearchOutcome::RuleEnded);
    }
}
