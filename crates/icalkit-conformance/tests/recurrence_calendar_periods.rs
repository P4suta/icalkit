// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar period anchors follow [RFC 7529 section 3](https://www.rfc-editor.org/rfc/rfc7529.html#section-3).

#![cfg(test)]

use core::num::NonZeroU32;

use icalkit::recurrence::{CalendarDate, CalendarMonth, CalendarScale as PublicScale};
use icalkit_conformance::internal::core::{CalendarScale, CivilDate, CivilDateTime, CivilTime};
use icalkit_conformance::internal::recur::{Freq, PeriodWalk, RecurrenceRuleBuilder};

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

fn stamp(year: u16, month: u8, day: u8) -> CivilDateTime {
    CivilDateTime::new(
        CivilDate::from_ymd(year, month, day).unwrap(),
        CivilTime::from_hms(9, 30, 0).unwrap(),
    )
}

fn midnight(date: CivilDate) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::MIDNIGHT)
}

fn coordinates(scale: CalendarScale, year: i32, month: u8, leap: bool) -> CivilDate {
    let month = if leap {
        CalendarMonth::leap(month)
    } else {
        CalendarMonth::regular(month)
    }
    .unwrap();
    let date = CalendarDate::new(PublicScale::parse(scale.name()).unwrap(), year, month, 1)
        .unwrap()
        .to_gregorian();
    CivilDate::from_ymd(
        u16::try_from(date.year()).unwrap(),
        u8::try_from(date.month()).unwrap(),
        u8::try_from(date.day()).unwrap(),
    )
    .unwrap()
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

#[test]
fn chinese_yearly_periods_follow_the_rfc_new_year_dates() {
    let rule = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
    let mut walk = PeriodWalk::in_calendar(stamp(2013, 2, 10), &rule, CalendarScale::Chinese);
    for (year, month, day) in [
        (2013, 2, 10),
        (2014, 1, 31),
        (2015, 2, 19),
        (2016, 2, 8),
        (2017, 1, 28),
    ] {
        assert_eq!(
            walk.next().unwrap().anchor(),
            midnight(stamp(year, month, day).date())
        );
    }
}

#[test]
fn ethiopic_monthly_periods_include_the_thirteenth_month() {
    let scale = CalendarScale::Ethiopic;
    let rule = RecurrenceRuleBuilder::new(Freq::Monthly).build().unwrap();
    let mut walk = PeriodWalk::in_calendar(stamp(2013, 9, 6), &rule, scale);
    for (year, month) in [(2005, 13), (2006, 1), (2006, 2), (2006, 3)] {
        assert_eq!(
            walk.next().unwrap().anchor(),
            midnight(coordinates(scale, year, month, false))
        );
    }
}

#[test]
fn hebrew_monthly_periods_visit_the_leap_month_before_regular_month_six() {
    let scale = CalendarScale::Hebrew;
    let rule = RecurrenceRuleBuilder::new(Freq::Monthly).build().unwrap();
    for (year, expected) in [
        (5774, vec![(5, false), (5, true), (6, false), (7, false)]),
        (5775, vec![(5, false), (6, false), (7, false), (8, false)]),
    ] {
        let start = midnight(coordinates(scale, year, 5, false));
        let mut walk = PeriodWalk::in_calendar(start, &rule, scale);
        for (month, leap) in expected {
            assert_eq!(
                walk.next().unwrap().anchor(),
                midnight(coordinates(scale, year, month, leap))
            );
        }
    }
}

#[test]
fn every_calendar_monthly_walk_has_adjacent_normalized_months() {
    let rule = RecurrenceRuleBuilder::new(Freq::Monthly).build().unwrap();
    for scale in SCALES {
        let mut walk = PeriodWalk::in_calendar(stamp(2013, 2, 10), &rule, scale);
        let first = walk.next().unwrap().anchor();
        let mut previous = converted(scale, first.date());
        assert_eq!(previous.day(), 1, "{scale:?}");
        assert_eq!(first.time(), CivilTime::MIDNIGHT);
        for _ in 0..300 {
            let anchor = walk.next().unwrap().anchor();
            let current = converted(scale, anchor.date());
            let before = converted(scale, anchor.date().checked_add_days(-1).unwrap());
            assert_eq!(current.day(), 1, "{scale:?} {current:?}");
            assert_eq!(
                (before.year(), before.month()),
                (previous.year(), previous.month()),
                "{scale:?}"
            );
            assert_ne!(
                (current.year(), current.month()),
                (previous.year(), previous.month()),
                "{scale:?}"
            );
            assert_eq!(anchor.time(), CivilTime::MIDNIGHT);
            previous = current;
        }
    }
}

#[test]
fn every_calendar_yearly_walk_begins_at_the_first_regular_month() {
    let rule = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
    for scale in SCALES {
        let mut walk = PeriodWalk::in_calendar(stamp(2013, 2, 10), &rule, scale);
        let first_year = converted(scale, stamp(2013, 2, 10).date()).year();
        for expected_year in (first_year..).take(100) {
            let period = walk.next().unwrap();
            let current = converted(scale, period.anchor().date());
            assert_eq!(current.year(), expected_year, "{scale:?}");
            assert_eq!(
                current.month(),
                CalendarMonth::regular(1).unwrap(),
                "{scale:?}"
            );
            assert_eq!(current.day(), 1, "{scale:?}");
            assert_eq!(period.anchor().time(), CivilTime::MIDNIGHT);
        }
    }
}

#[test]
fn calendar_period_indices_and_intervals_resume_without_traversal_history() {
    for scale in SCALES {
        for freq in [Freq::Monthly, Freq::Yearly] {
            for interval in [1, 2, 7] {
                let rule = RecurrenceRuleBuilder::new(freq)
                    .interval(NonZeroU32::new(interval).unwrap())
                    .build()
                    .unwrap();
                let start = stamp(2014, 2, 8);
                let mut walk = PeriodWalk::in_calendar(start, &rule, scale);
                for index in 0..=100 {
                    let reached = walk.next();
                    if [0, 1, 2, 12, 13, 19, 100].contains(&index) {
                        let mut resumed =
                            PeriodWalk::resume_in_calendar(start, &rule, scale, index);
                        assert_eq!(
                            resumed.next(),
                            reached,
                            "{scale:?} {freq:?} {interval} {index}"
                        );
                        assert_eq!(resumed.index(), walk.index());
                    }
                }
            }
        }
    }
}

#[test]
fn yearly_periods_keep_extended_years_above_9999() {
    let rule = RecurrenceRuleBuilder::new(Freq::Yearly).build().unwrap();
    let mut walk = PeriodWalk::in_calendar(stamp(9999, 12, 31), &rule, CalendarScale::Buddhist);
    assert_eq!(
        walk.next().unwrap().anchor(),
        midnight(stamp(9999, 1, 1).date())
    );
    assert!(walk.next().is_none());
    assert!(walk.next().is_none());
}

#[test]
fn calendar_periods_clip_the_initial_boundary_and_keep_the_last_period() {
    for scale in SCALES {
        for freq in [Freq::Monthly, Freq::Yearly] {
            let rule = RecurrenceRuleBuilder::new(freq).build().unwrap();
            let mut first = PeriodWalk::in_calendar(stamp(0, 1, 1), &rule, scale);
            assert_eq!(
                first.next().unwrap().anchor(),
                midnight(stamp(0, 1, 1).date()),
                "{scale:?} {freq:?}"
            );
            assert!(first.next().unwrap().anchor() > midnight(stamp(0, 1, 1).date()));
            let mut last = PeriodWalk::in_calendar(stamp(9999, 12, 31), &rule, scale);
            assert!(last.next().unwrap().anchor() <= stamp(9999, 12, 31));
            assert!(last.next().is_none(), "{scale:?} {freq:?}");
            assert!(last.next().is_none());
        }
    }
}

#[test]
fn extreme_calendar_period_indices_stop_without_wrapping() {
    for scale in SCALES {
        for freq in [Freq::Monthly, Freq::Yearly] {
            for interval in [i32::MAX.unsigned_abs(), u32::MAX] {
                let rule = RecurrenceRuleBuilder::new(freq)
                    .interval(NonZeroU32::new(interval).unwrap())
                    .build()
                    .unwrap();
                let start = stamp(2014, 2, 8);
                let mut walk = PeriodWalk::in_calendar(start, &rule, scale);
                assert!(walk.next().is_some());
                assert!(walk.next().is_none());
                assert!(
                    PeriodWalk::resume_in_calendar(start, &rule, scale, u64::MAX)
                        .next()
                        .is_none()
                );
            }
        }
    }
}

#[test]
fn submonthly_periods_keep_the_same_week_and_clock_arithmetic() {
    let start = stamp(2014, 2, 8);
    for scale in SCALES {
        for freq in [
            Freq::Weekly,
            Freq::Daily,
            Freq::Hourly,
            Freq::Minutely,
            Freq::Secondly,
        ] {
            let rule = RecurrenceRuleBuilder::new(freq).build().unwrap();
            let native: Vec<_> = PeriodWalk::new(start, &rule).take(20).collect();
            let selected: Vec<_> = PeriodWalk::in_calendar(start, &rule, scale)
                .take(20)
                .collect();
            assert_eq!(selected, native, "{scale:?} {freq:?}");
        }
    }
}
