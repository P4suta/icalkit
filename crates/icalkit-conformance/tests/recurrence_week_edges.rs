// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Weekly extents follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, DecodeValue, IgnoreDiagnostics, Limits, Meter, UtcOffset,
    Weekday,
};
use icalkit_conformance::internal::recur::{
    OverrideSet, Period, PeriodWalk, RecurrenceInput, RecurrenceRule, SearchOutcome, SearchStep,
    ValueKind, Window, expand_period, select,
};

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn stamp(date: CivilDate) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::from_hms(9, 0, 0).unwrap())
}

fn weekday(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

fn selected(period: Period, rule: &RecurrenceRule, source: CivilDateTime) -> Vec<CivilDate> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let set = expand_period(period, rule, source, &mut meter, &mut IgnoreDiagnostics).unwrap();
    select(&set, rule.by_set_pos())
        .as_slice()
        .iter()
        .map(|value| value.date())
        .collect()
}

#[test]
fn weekly_extents_and_indices_match_independent_day_enumeration() {
    let source = stamp(date(0, 1, 1));
    for start in Weekday::ALL {
        let elapsed = i64::from(Weekday::Saturday.index())
            .checked_sub(i64::from(start.index()))
            .unwrap()
            .rem_euclid(7);
        for interval in [1_u32, 2, 7, u32::MAX] {
            let rule = RecurrenceRule::decode_value(
                format!(
                    "FREQ=WEEKLY;INTERVAL={interval};BYMONTH=1;BYDAY=SA,SU;BYSETPOS=1,-1;WKST={}",
                    weekday(start),
                )
                .as_bytes(),
            )
            .unwrap();
            let mut walk = PeriodWalk::new(source, &rule);
            for index in 0_u64..=12 {
                let first = i64::try_from(index)
                    .unwrap()
                    .checked_mul(i64::from(interval))
                    .unwrap()
                    .checked_mul(7)
                    .unwrap()
                    .checked_sub(elapsed)
                    .unwrap();
                let anchor = source.date().checked_add_days(first.max(0));
                let reached = walk.next();
                let mut resumed = PeriodWalk::resume_at(source, &rule, index);
                assert_eq!(resumed.next(), reached, "{start:?}, {interval}, {index}");
                let Some(anchor) = anchor else {
                    assert_eq!(reached, None);
                    assert_eq!(walk.next(), None);
                    break;
                };
                let period = reached.unwrap();
                assert_eq!(
                    period.anchor().date(),
                    anchor,
                    "{start:?}, {interval}, {index}"
                );
                assert_eq!(period.anchor().time(), CivilTime::MIDNIGHT);
                let candidates: Vec<_> = (0..7)
                    .filter_map(|offset| {
                        source
                            .date()
                            .checked_add_days(first.checked_add(offset).unwrap())
                    })
                    .filter(|date| {
                        date.month() == 1
                            && matches!(date.weekday(), Some(Weekday::Saturday | Weekday::Sunday))
                    })
                    .collect();
                let mut expected: Vec<_> = candidates
                    .first()
                    .into_iter()
                    .chain(candidates.last())
                    .copied()
                    .collect();
                expected.dedup();
                assert_eq!(
                    selected(period, &rule, source),
                    expected,
                    "{start:?}, {interval}, {index}"
                );
            }
            let mut far = PeriodWalk::resume_at(source, &rule, u64::MAX);
            assert_eq!(far.next(), None);
            assert_eq!(far.index(), u64::MAX);
        }
    }
}

#[test]
fn civil_edges_clip_weekly_dates_before_position_selection() {
    let rule =
        RecurrenceRule::decode_value(b"FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYSETPOS=2,-2")
            .unwrap();
    for (source, expected) in [
        (date(0, 1, 1), vec![date(0, 1, 1), date(0, 1, 2)]),
        (
            date(9999, 12, 31),
            vec![date(9999, 12, 28), date(9999, 12, 30)],
        ),
    ] {
        let source = stamp(source);
        let period = PeriodWalk::new(source, &rule).next().unwrap();
        assert_eq!(selected(period, &rule, source), expected);
    }
}

#[test]
fn clipped_weekly_work_reports_budget_exhaustion() {
    let rule = RecurrenceRule::decode_value(b"FREQ=WEEKLY;BYDAY=SA,SU;COUNT=2").unwrap();
    let source = stamp(date(0, 1, 1)).at_offset(UtcOffset::UTC).unwrap();
    let end = stamp(date(0, 1, 3)).at_offset(UtcOffset::UTC).unwrap();
    let mut meter = Meter::with_budget(Limits::DEFAULT, 1);
    let input = RecurrenceInput::new(
        source,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    )
    .unwrap();
    let mut sink = IgnoreDiagnostics;
    let mut search = input.search(Window::new(source, end).unwrap(), &mut meter, &mut sink);
    assert!(matches!(
        search.next(),
        Some(SearchStep::BudgetExhausted(_))
    ));
    assert!(search.next().is_none());
    assert!(matches!(
        search.outcome(),
        SearchOutcome::BudgetExhausted(_)
    ));
    drop(search);
    assert!(meter.is_exhausted());
}
