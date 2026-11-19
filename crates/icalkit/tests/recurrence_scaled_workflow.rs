// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar recurrence follows [RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html).

#![cfg(test)]

use icalkit::recurrence::{CalendarDate, CalendarMonth, CalendarScale, Occurrences, Rule, Window};
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine, ResourcePolicy};
use jiff::civil::Date;

const NAMES: [&str; 16] = [
    "GREGORIAN",
    "BUDDHIST",
    "CHINESE",
    "COPTIC",
    "DANGI",
    "ETHIOPIC",
    "ETHIOAA",
    "HEBREW",
    "INDIAN",
    "ISLAMIC-CIVIL",
    "ISLAMIC-TBLA",
    "ISLAMIC-UMALQURA",
    "ISO8601",
    "JAPANESE",
    "PERSIAN",
    "ROC",
];

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn window(start: &str, end: &str) -> Window {
    Window::new(at(start), at(end)).unwrap()
}

fn starts(mut occurrences: Occurrences<'_>) -> Vec<Timestamp> {
    let mut values = Vec::new();
    while let Some(occurrence) = occurrences.try_next().unwrap() {
        values.push(occurrence.start());
    }
    values
}

fn expand(text: &str, start: &str, end: &str) -> Vec<Timestamp> {
    let rule = Rule::parse(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
    let engine = Engine::default();
    let mut session = engine.session();
    starts(
        rule.occurrences(&mut session, at(start), window(start, end))
            .unwrap(),
    )
}

fn morning(date: Date) -> Timestamp {
    at(&format!("{date}T09:00:00Z"))
}

fn month(number: u8, leap: bool) -> CalendarMonth {
    if leap {
        CalendarMonth::leap(number)
    } else {
        CalendarMonth::regular(number)
    }
    .unwrap()
}

fn date(name: &str, year: i32, number: u8, leap: bool, day: u8) -> Date {
    CalendarDate::new(
        CalendarScale::parse(name).unwrap(),
        year,
        month(number, leap),
        day,
    )
    .unwrap()
    .to_gregorian()
}

#[test]
fn chinese_new_year_occurrences_follow_the_rfc_example() {
    assert_eq!(
        expand(
            "RSCALE=CHINESE;FREQ=YEARLY;COUNT=5",
            "2013-02-10T09:00:00Z",
            "2018-01-01T00:00:00Z"
        ),
        [
            "2013-02-10T09:00:00Z",
            "2014-01-31T09:00:00Z",
            "2015-02-19T09:00:00Z",
            "2016-02-08T09:00:00Z",
            "2017-01-28T09:00:00Z"
        ]
        .map(at)
    );
}

#[test]
fn ethiopic_month_thirteen_limits_monthly_occurrences() {
    assert_eq!(
        expand(
            "RSCALE=ETHIOPIC;FREQ=MONTHLY;BYMONTH=13;COUNT=5",
            "2013-09-06T09:00:00Z",
            "2018-01-01T00:00:00Z"
        ),
        [
            "2013-09-06T09:00:00Z",
            "2014-09-06T09:00:00Z",
            "2015-09-06T09:00:00Z",
            "2016-09-06T09:00:00Z",
            "2017-09-06T09:00:00Z"
        ]
        .map(at)
    );
}

#[test]
fn hebrew_leap_month_correction_follows_the_rfc_anniversary() {
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYMONTH=5L;BYMONTHDAY=8;SKIP=FORWARD;COUNT=5",
            "2014-02-08T09:00:00Z",
            "2019-01-01T00:00:00Z"
        ),
        [
            "2014-02-08T09:00:00Z",
            "2015-02-27T09:00:00Z",
            "2016-02-17T09:00:00Z",
            "2017-03-06T09:00:00Z",
            "2018-02-23T09:00:00Z"
        ]
        .map(at)
    );
}

#[test]
fn every_calendar_yearly_rule_uses_the_seed_calendar_fields() {
    let source = Date::new(2013, 2, 10).unwrap();
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let seed = CalendarDate::from_gregorian(scale, source).unwrap();
        let mut expected = Vec::new();
        for year in seed.year()..seed.year() + 40 {
            if let Ok(candidate) = CalendarDate::new(scale, year, seed.month(), seed.day()) {
                expected.push(morning(candidate.to_gregorian()));
            }
            if expected.len() == 4 {
                break;
            }
        }
        assert_eq!(expected.len(), 4, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=YEARLY;COUNT=4"),
                "2013-02-10T09:00:00Z",
                "2054-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn every_calendar_monthly_rule_uses_calendar_days_without_clamping() {
    let source = Date::new(2013, 2, 10).unwrap();
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let seed = CalendarDate::from_gregorian(scale, source).unwrap();
        let mut expected = Vec::new();
        let mut current = source;
        for _ in 0..1000 {
            if CalendarDate::from_gregorian(scale, current).unwrap().day() == seed.day() {
                expected.push(morning(current));
            }
            if expected.len() == 8 {
                break;
            }
            current = current.checked_add(jiff::ToSpan::days(1)).unwrap();
        }
        assert_eq!(expected.len(), 8, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=MONTHLY;COUNT=8"),
                "2013-02-10T09:00:00Z",
                "2016-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn negative_month_days_select_the_end_of_each_calendar_month() {
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let mut current = Date::new(2013, 2, 10).unwrap();
        let mut expected = Vec::new();
        for _ in 0..300 {
            let next = current.checked_add(jiff::ToSpan::days(1)).unwrap();
            if CalendarDate::from_gregorian(scale, next).unwrap().day() == 1 {
                expected.push(morning(current));
            }
            if expected.len() == 5 {
                break;
            }
            current = next;
        }
        assert_eq!(expected.len(), 5, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=5"),
                "2013-02-10T09:00:00Z",
                "2014-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn monthly_intervals_count_missing_days_and_leap_months_in_the_cadence() {
    let source = Date::new(2014, 2, 8).unwrap();
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let seed = CalendarDate::from_gregorian(scale, source).unwrap();
        let mut current = source;
        let mut previous = (seed.year(), seed.month());
        let mut period = 0;
        let mut expected = Vec::new();
        for _ in 0..1200 {
            let fields = CalendarDate::from_gregorian(scale, current).unwrap();
            let key = (fields.year(), fields.month());
            if key != previous {
                period += 1;
                previous = key;
            }
            if period % 2 == 0 && fields.day() == seed.day() {
                expected.push(morning(current));
            }
            if expected.len() == 8 {
                break;
            }
            current = current.checked_add(jiff::ToSpan::days(1)).unwrap();
        }
        assert_eq!(expected.len(), 8, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=MONTHLY;INTERVAL=2;COUNT=8"),
                "2014-02-08T09:00:00Z",
                "2018-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn yearly_intervals_count_calendar_years_without_moving_the_seed_month() {
    let source = Date::new(2014, 2, 8).unwrap();
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let seed = CalendarDate::from_gregorian(scale, source).unwrap();
        let expected: Vec<_> = (0..40)
            .filter_map(|index| {
                CalendarDate::new(scale, seed.year() + index * 3, seed.month(), seed.day()).ok()
            })
            .take(4)
            .map(|date| morning(date.to_gregorian()))
            .collect();
        assert_eq!(expected.len(), 4, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=YEARLY;INTERVAL=3;COUNT=4"),
                "2014-02-08T09:00:00Z",
                "2135-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn scaled_until_uses_inclusive_gregorian_instants() {
    for (until, expected) in [
        (
            "20140131T090000Z",
            vec![at("2013-02-10T09:00:00Z"), at("2014-01-31T09:00:00Z")],
        ),
        ("20140131T085959Z", vec![at("2013-02-10T09:00:00Z")]),
    ] {
        assert_eq!(
            expand(
                &format!("RSCALE=CHINESE;FREQ=YEARLY;UNTIL={until}"),
                "2013-02-10T09:00:00Z",
                "2020-01-01T00:00:00Z"
            ),
            expected
        );
    }
}

#[test]
fn scaled_searches_clip_periods_at_the_gregorian_and_timestamp_boundaries() {
    for name in NAMES {
        let scale = CalendarScale::parse(name).unwrap();
        let source = Date::new(0, 1, 1).unwrap();
        let seed = CalendarDate::from_gregorian(scale, source).unwrap();
        let expected: Vec<_> = (seed.year()..seed.year() + 40)
            .filter_map(|year| CalendarDate::new(scale, year, seed.month(), seed.day()).ok())
            .take(2)
            .map(|date| morning(date.to_gregorian()))
            .collect();
        assert_eq!(expected.len(), 2, "{name}");
        assert_eq!(
            expand(
                &format!("RSCALE={name};FREQ=YEARLY;COUNT=2"),
                "0000-01-01T09:00:00Z",
                "0041-01-01T00:00:00Z"
            ),
            expected,
            "{name}"
        );
        for frequency in ["MONTHLY", "YEARLY"] {
            assert_eq!(
                expand(
                    &format!("RSCALE={name};FREQ={frequency};COUNT=2"),
                    "9999-12-30T09:00:00Z",
                    "9999-12-30T21:59:59Z"
                ),
                [at("9999-12-30T09:00:00Z")],
                "{name} {frequency}"
            );
        }
    }
}

#[test]
fn hebrew_year_days_extend_beyond_the_gregorian_limit() {
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYYEARDAY=385;COUNT=1",
            "2013-09-05T09:00:00Z",
            "2015-01-01T00:00:00Z"
        ),
        [at("2014-09-24T09:00:00Z")]
    );
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYYEARDAY=-385;COUNT=1",
            "2013-09-05T09:00:00Z",
            "2015-01-01T00:00:00Z"
        ),
        [at("2013-09-05T09:00:00Z")]
    );
    assert!(Rule::parse("RSCALE=HEBREW;FREQ=YEARLY;BYYEARDAY=386").is_err());
    assert!(Rule::parse("FREQ=YEARLY;BYYEARDAY=385").is_err());
}

#[test]
fn hebrew_week_numbers_and_weekday_ordinals_use_the_calendar_year() {
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYWEEKNO=55;BYDAY=MO;COUNT=1",
            "2013-09-05T09:00:00Z",
            "2015-01-01T00:00:00Z"
        ),
        [at("2014-09-15T09:00:00Z")]
    );
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYDAY=55MO;COUNT=1",
            "2013-09-05T09:00:00Z",
            "2015-01-01T00:00:00Z"
        ),
        [at("2014-09-22T09:00:00Z")]
    );
    assert!(Rule::parse("FREQ=YEARLY;BYDAY=55MO").is_err());
}

#[test]
fn set_positions_cover_the_selected_calendar_year() {
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYSETPOS=385;COUNT=1",
            "2013-09-05T09:00:00Z",
            "2015-01-01T00:00:00Z"
        ),
        [at("2014-09-24T09:00:00Z")]
    );
}

#[test]
fn missing_months_are_corrected_before_missing_days() {
    for (mode, expected) in [
        ("BACKWARD", "2015-02-28T09:00:00Z"),
        ("FORWARD", "2015-03-31T09:00:00Z"),
    ] {
        assert_eq!(
            expand(
                &format!(
                    "RSCALE=GREGORIAN;FREQ=YEARLY;BYMONTH=2L;BYMONTHDAY=31;SKIP={mode};COUNT=1"
                ),
                "2015-01-01T09:00:00Z",
                "2016-01-01T00:00:00Z"
            ),
            [at(expected)]
        );
    }
    for (mode, expected) in [
        ("BACKWARD", "2013-09-10T09:00:00Z"),
        ("FORWARD", "2013-10-10T09:00:00Z"),
    ] {
        assert_eq!(
            expand(
                &format!(
                    "RSCALE=ETHIOPIC;FREQ=YEARLY;BYMONTH=13L;BYMONTHDAY=30;SKIP={mode};COUNT=1"
                ),
                "2012-09-11T09:00:00Z",
                "2014-01-01T00:00:00Z"
            ),
            [at(expected)]
        );
    }
    assert_eq!(
        expand(
            "RSCALE=HEBREW;FREQ=YEARLY;BYMONTH=5L;BYMONTHDAY=30;SKIP=FORWARD;COUNT=1",
            "2014-09-25T09:00:00Z",
            "2015-09-01T00:00:00Z"
        ),
        [morning(date("HEBREW", 5775, 7, false, 1))]
    );
}

#[test]
fn default_leap_months_use_the_same_correction_policy() {
    for mode in ["BACKWARD", "FORWARD"] {
        let regular = if mode == "BACKWARD" { 5 } else { 6 };
        let expected = [
            morning(date("HEBREW", 5774, 5, true, 8)),
            morning(date("HEBREW", 5775, regular, false, 8)),
            morning(date("HEBREW", 5776, 5, true, 8)),
        ];
        assert_eq!(
            expand(
                &format!("RSCALE=HEBREW;FREQ=YEARLY;SKIP={mode};COUNT=3"),
                "2014-02-08T09:00:00Z",
                "2018-01-01T00:00:00Z"
            ),
            expected
        );
    }
}

#[test]
fn weekday_filtering_follows_month_and_day_correction() {
    assert_eq!(
        expand(
            "RSCALE=GREGORIAN;FREQ=YEARLY;BYMONTH=2L;BYMONTHDAY=31;SKIP=BACKWARD;BYDAY=SA;COUNT=1",
            "2015-01-01T09:00:00Z",
            "2016-01-01T00:00:00Z"
        ),
        [at("2015-02-28T09:00:00Z")]
    );
    assert_eq!(
        expand(
            "RSCALE=GREGORIAN;FREQ=YEARLY;BYMONTH=2L;BYMONTHDAY=31;SKIP=BACKWARD;BYDAY=MO;COUNT=1",
            "2015-01-01T09:00:00Z",
            "2016-01-01T00:00:00Z"
        ),
        []
    );
}

fn overlap() -> &'static str {
    "RSCALE=ETHIOPIC;FREQ=MONTHLY;BYMONTHDAY=1,30;SKIP=FORWARD;BYHOUR=0,23;COUNT=8"
}

#[test]
fn overlapping_calendar_periods_sort_and_count_each_instant_once() {
    assert_eq!(
        expand(overlap(), "2013-09-06T00:00:00Z", "2014-01-01T00:00:00Z"),
        [
            "2013-09-06T00:00:00Z",
            "2013-09-06T23:00:00Z",
            "2013-09-11T00:00:00Z",
            "2013-09-11T23:00:00Z",
            "2013-10-10T00:00:00Z",
            "2013-10-10T23:00:00Z",
            "2013-10-11T00:00:00Z",
            "2013-10-11T23:00:00Z"
        ]
        .map(at)
    );
}

#[test]
fn scaled_cursors_reconstruct_pending_periods_after_every_occurrence() {
    let start = "2013-09-06T00:00:00Z";
    let end = "2014-01-01T00:00:00Z";
    let expected = expand(overlap(), start, end);
    let rule = Rule::parse(overlap()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut stream = rule
        .occurrences(&mut session, at(start), window(start, end))
        .unwrap();
    for expected in expected {
        assert_eq!(stream.try_next().unwrap().unwrap().start(), expected);
        let cursor = stream.cursor();
        drop(stream);
        stream = rule
            .resume(&mut session, at(start), window(start, end), cursor)
            .unwrap();
    }
    assert!(stream.try_next().unwrap().is_none());
}

#[test]
fn narrow_windows_include_backward_dates_from_the_next_calendar_month() {
    let rule = Rule::parse("RSCALE=ETHIOPIC;FREQ=MONTHLY;BYMONTHDAY=-30;SKIP=BACKWARD").unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        starts(
            rule.occurrences(
                &mut session,
                at("2013-08-07T09:00:00Z"),
                window("2013-09-05T00:00:00Z", "2013-09-06T00:00:00Z")
            )
            .unwrap()
        ),
        [at("2013-09-05T09:00:00Z")]
    );
}

#[test]
fn stored_scaled_rules_keep_gregorian_exclusions_and_override_keys() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit calendar recurrence//EN\r\n\
        BEGIN:VEVENT\r\nUID:calendar@example.test\r\nDTSTAMP:20140201T000000Z\r\n\
        DTSTART:20140208T090000Z\r\nDURATION:PT1H\r\n\
        RRULE:RSCALE=HEBREW;FREQ=YEARLY;BYMONTH=5L;BYMONTHDAY=8;\r\n SKIP=FORWARD;COUNT=5\r\n\
        RDATE:20150228T090000Z\r\nEXDATE:20150227T090000Z\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:calendar@example.test\r\nDTSTAMP:20140201T000000Z\r\n\
        RECURRENCE-ID:20160217T090000Z\r\nDTSTART:20160218T110000Z\r\nDURATION:PT1H\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    let mut stream = calendar
        .occurrences(
            &mut session,
            "calendar@example.test",
            window("2014-01-01T00:00:00Z", "2019-01-01T00:00:00Z"),
        )
        .unwrap();
    let mut actual = Vec::new();
    while let Some(occurrence) = stream.try_next().unwrap() {
        actual.push((occurrence.key(), occurrence.start()));
    }
    assert_eq!(
        actual,
        [
            (at("2014-02-08T09:00:00Z"), at("2014-02-08T09:00:00Z")),
            (at("2015-02-28T09:00:00Z"), at("2015-02-28T09:00:00Z")),
            (at("2016-02-17T09:00:00Z"), at("2016-02-18T11:00:00Z")),
            (at("2017-03-06T09:00:00Z"), at("2017-03-06T09:00:00Z")),
            (at("2018-02-23T09:00:00Z"), at("2018-02-23T09:00:00Z")),
        ]
    );
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn impossible_scaled_rules_exhaust_the_budget_instead_of_ending() {
    let rule = Rule::parse("RSCALE=ETHIOPIC;FREQ=YEARLY;BYMONTH=13;BYMONTHDAY=30").unwrap();
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure().with_max_input_bytes(4096))
        .build();
    let mut session = engine.session();
    let mut stream = rule
        .occurrences(
            &mut session,
            at("2013-09-06T09:00:00Z"),
            window("2013-01-01T00:00:00Z", "5000-01-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(
        stream.try_next().unwrap_err().code().as_str(),
        "icalkit.recurrence.budget-exhausted"
    );
}

#[test]
fn calendar_aliases_and_part_order_preserve_the_same_series() {
    let expected = expand(
        "RSCALE=ISLAMIC-CIVIL;FREQ=YEARLY;COUNT=3",
        "2013-02-10T09:00:00Z",
        "2017-01-01T00:00:00Z",
    );
    assert_eq!(
        expand(
            "COUNT=3;FREQ=YEARLY;RSCALE=islamicc",
            "2013-02-10T09:00:00Z",
            "2017-01-01T00:00:00Z"
        ),
        expected
    );
}

#[test]
fn ambiguous_calendars_and_unspecified_week_corrections_remain_refused() {
    for text in [
        "RSCALE=ISLAMIC;FREQ=YEARLY",
        "RSCALE=ISLAMIC-RGSA;FREQ=YEARLY",
        "RSCALE=X-FUTURE;FREQ=YEARLY",
        "RSCALE=HEBREW;FREQ=YEARLY;BYWEEKNO=55;SKIP=FORWARD",
        "FREQ=YEARLY;BYMONTH=5L",
    ] {
        assert!(Rule::parse(text).is_err(), "{text}");
    }
}
