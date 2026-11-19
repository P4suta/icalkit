// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Source week periods follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use core::num::NonZeroU32;

use icalkit::recurrence::{CalendarDate, CalendarMonth, CalendarScale as PublicScale};
use icalkit_conformance::internal::core::{
    CalendarScale, CivilDate, CivilDateTime, CivilTime, DecodeValue, IgnoreDiagnostics, Limits,
    Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::recur::{
    ByList, Freq, OverrideSet, Period, PeriodWalk, RecurrenceInput, RecurrenceRule,
    RecurrenceRuleBuilder, SearchOutcome, SearchStep, ValueKind, Window, expand_period, select,
};

const SCALES: [CalendarScale; 16] = [
    CalendarScale::Gregorian,
    CalendarScale::Buddhist,
    CalendarScale::Chinese,
    CalendarScale::Coptic,
    CalendarScale::Dangi,
    CalendarScale::Ethiopic,
    CalendarScale::EthiopicAmeteAlem,
    CalendarScale::Hebrew,
    CalendarScale::Indian,
    CalendarScale::IslamicCivil,
    CalendarScale::IslamicTabular,
    CalendarScale::IslamicUmmAlQura,
    CalendarScale::Iso8601,
    CalendarScale::Japanese,
    CalendarScale::Persian,
    CalendarScale::Roc,
];

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn stamp(date: CivilDate) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::from_hms(9, 30, 0).unwrap())
}

fn midnight(date: CivilDate) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::MIDNIGHT)
}

fn converted(scale: CalendarScale, date: CivilDate) -> CalendarDate {
    CalendarDate::from_gregorian(
        PublicScale::parse(scale.name()).unwrap(),
        jiff::civil::Date::new(
            i16::try_from(date.year()).unwrap(),
            i8::try_from(date.month()).unwrap(),
            i8::try_from(date.day()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn year_start(scale: CalendarScale, year: i32) -> CivilDate {
    let gregorian = CalendarDate::new(
        PublicScale::parse(scale.name()).unwrap(),
        year,
        CalendarMonth::regular(1).unwrap(),
        1,
    )
    .unwrap()
    .to_gregorian();
    date(
        u16::try_from(gregorian.year()).unwrap(),
        u8::try_from(gregorian.month()).unwrap(),
        u8::try_from(gregorian.day()).unwrap(),
    )
}

fn first_week(scale: CalendarScale, year: i32, start: Weekday) -> CivilDate {
    let boundary = year_start(scale, year);
    let first = (-6..=0)
        .map(|offset| boundary.checked_add_days(offset).unwrap())
        .find(|date| date.weekday() == Some(start))
        .unwrap();
    let inside = (0..7)
        .map(|offset| first.checked_add_days(offset).unwrap())
        .filter(|date| converted(scale, *date).year() == year)
        .count();
    if inside >= 4 {
        first
    } else {
        first.checked_add_days(7).unwrap()
    }
}

fn walk_rule(start: Weekday, interval: u32) -> RecurrenceRule {
    RecurrenceRuleBuilder::new(Freq::Yearly)
        .wkst(start)
        .interval(NonZeroU32::new(interval).unwrap())
        .by_week_no(ByList::from_slice(&[1, -1]))
        .build()
        .unwrap()
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
fn week_periods_follow_four_day_boundaries_in_every_calendar() {
    for scale in SCALES {
        let origin = converted(scale, date(2019, 1, 1)).year();
        for year in origin..origin.checked_add(28).unwrap() {
            for start in Weekday::ALL {
                let first = first_week(scale, year, start);
                let next = first_week(scale, year.checked_add(1).unwrap(), start);
                let rule = walk_rule(start, 1);
                for source in [
                    first.checked_add_days(1).unwrap(),
                    next.checked_add_days(-1).unwrap(),
                ] {
                    let mut walk = PeriodWalk::in_calendar(stamp(source), &rule, scale);
                    assert_eq!(
                        walk.next().unwrap().anchor(),
                        midnight(first),
                        "{scale:?}, {year}, {start:?}, {source:?}"
                    );
                    assert_eq!(walk.next().unwrap().anchor(), midnight(next));
                }
            }
        }
    }
}

#[test]
fn week_intervals_and_resumed_indices_share_the_sources_numbered_year() {
    for scale in SCALES {
        let year = converted(scale, date(2019, 1, 1)).year();
        for start in Weekday::ALL {
            let source = stamp(first_week(scale, year, start).checked_add_days(1).unwrap());
            for interval in [1_u32, 2, 7, 400] {
                let rule = walk_rule(start, interval);
                let mut walk = PeriodWalk::in_calendar(source, &rule, scale);
                for index in 0..=9 {
                    let numbered = year
                        .checked_add(
                            i32::try_from(index)
                                .unwrap()
                                .checked_mul(i32::try_from(interval).unwrap())
                                .unwrap(),
                        )
                        .unwrap();
                    let expected = midnight(first_week(scale, numbered, start));
                    let reached = walk.next().unwrap();
                    assert_eq!(reached.anchor(), expected, "{scale:?}, {interval}, {index}");
                    let mut resumed = PeriodWalk::resume_in_calendar(source, &rule, scale, index);
                    assert_eq!(resumed.next(), Some(reached));
                    assert_eq!(resumed.index(), walk.index());
                }
            }
        }
    }
}

#[test]
fn complete_week_extents_feed_calendar_filters_and_position_selection() {
    for scale in SCALES {
        let year = converted(scale, date(2019, 1, 1)).year();
        for (start, token) in [(Weekday::Monday, "MO"), (Weekday::Sunday, "SU")] {
            let first = first_week(scale, year, start);
            let end = first_week(scale, year.checked_add(1).unwrap(), start);
            let source = stamp(first.checked_add_days(1).unwrap());
            let rule = RecurrenceRule::decode_value(
                format!("RSCALE={};FREQ=YEARLY;INTERVAL=2;BYWEEKNO=1,-1;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYSETPOS=2,-2;WKST={token}", scale.name()).as_bytes(),
            )
            .unwrap();
            let period = PeriodWalk::new(source, &rule).next().unwrap();
            assert_eq!(
                selected(period, &rule, source),
                [
                    first.checked_add_days(1).unwrap(),
                    end.checked_add_days(-2).unwrap()
                ],
                "{scale:?}, {start:?}"
            );
        }
    }
}

#[test]
fn civil_edges_clip_week_extents_before_position_selection() {
    let rule = RecurrenceRule::decode_value(
        b"FREQ=YEARLY;BYWEEKNO=1;BYDAY=TH,FR,SA;BYSETPOS=2,-2;WKST=WE",
    )
    .unwrap();
    let source = stamp(date(9999, 1, 1));
    let mut walk = PeriodWalk::new(source, &rule);
    assert_eq!(
        selected(walk.next().unwrap(), &rule, source),
        [date(9999, 1, 1)]
    );
    assert_eq!(
        selected(walk.next().unwrap(), &rule, source),
        [date(9999, 12, 30), date(9999, 12, 31)]
    );
    assert_eq!(walk.next(), None);
    assert_eq!(walk.next(), None);
    let rule = RecurrenceRule::decode_value(b"FREQ=YEARLY;BYWEEKNO=-1;BYDAY=SA,SU").unwrap();
    let source = stamp(date(0, 1, 1));
    let period = PeriodWalk::new(source, &rule).next().unwrap();
    assert_eq!(period.anchor(), midnight(date(0, 1, 1)));
    assert_eq!(
        selected(period, &rule, source),
        [date(0, 1, 1), date(0, 1, 2)]
    );
}

#[test]
fn source_week_counts_reach_remaining_dates_at_both_civil_edges() {
    for (text, source, through, expected) in [
        (
            "FREQ=YEARLY;BYWEEKNO=-1;BYDAY=SA,SU;COUNT=2",
            date(0, 1, 1),
            date(0, 1, 3),
            vec![date(0, 1, 1), date(0, 1, 2)],
        ),
        (
            "FREQ=YEARLY;BYWEEKNO=1;BYDAY=TH,FR,SA;BYSETPOS=2,-2;WKST=WE;COUNT=3",
            date(9999, 1, 1),
            date(9999, 12, 31),
            vec![date(9999, 1, 1), date(9999, 12, 30), date(9999, 12, 31)],
        ),
    ] {
        let rule = RecurrenceRule::decode_value(text.as_bytes()).unwrap();
        let source = stamp(source).at_offset(UtcOffset::UTC).unwrap();
        let end = CivilDateTime::new(through, CivilTime::from_hms(10, 0, 0).unwrap())
            .at_offset(UtcOffset::UTC)
            .unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
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
        let actual: Vec<_> = search
            .by_ref()
            .map(|step| match step {
                SearchStep::Occurrence(occurrence) => occurrence.start(),
                SearchStep::BudgetExhausted(error) => panic!("{error:?}"),
                SearchStep::CalendarArithmeticFailed => panic!("calendar arithmetic failed"),
                _ => panic!("unexpected terminal search step"),
            })
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|date| stamp(date).at_offset(UtcOffset::UTC).unwrap())
            .collect();
        assert_eq!(actual, expected, "{text}");
        assert_eq!(search.outcome(), SearchOutcome::RuleEnded, "{text}");
    }
}

#[test]
fn huge_week_intervals_and_indices_end_without_wrapping() {
    for scale in SCALES {
        let year = converted(scale, date(2019, 1, 1)).year();
        let first = first_week(scale, year, Weekday::Monday);
        let source = stamp(first.checked_add_days(1).unwrap());
        let rule = walk_rule(Weekday::Monday, u32::MAX);
        let mut walk = PeriodWalk::in_calendar(source, &rule, scale);
        assert_eq!(walk.next().unwrap().anchor(), midnight(first));
        assert_eq!(walk.next(), None);
        assert_eq!(walk.next(), None);
        let mut resumed = PeriodWalk::resume_in_calendar(source, &rule, scale, u64::MAX);
        assert_eq!(resumed.next(), None);
        assert_eq!(resumed.index(), u64::MAX);
    }
}
