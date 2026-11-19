// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar coordinates follow [RFC 7529 sections 3 and 4](https://www.rfc-editor.org/rfc/rfc7529.html#section-3).

#![cfg(test)]

use icalkit::recurrence::{CalendarDate, CalendarMonth, CalendarScale};
use jiff::civil::Date;

fn gregorian(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).unwrap()
}

#[test]
fn registered_names_and_aliases_have_explicit_case_insensitive_meanings() {
    for (name, expected) in [
        ("GREGORIAN", "GREGORIAN"),
        ("gregory", "GREGORIAN"),
        ("bUdDhIsT", "BUDDHIST"),
        ("CHINESE", "CHINESE"),
        ("COPTIC", "COPTIC"),
        ("DANGI", "DANGI"),
        ("ETHIOPIC", "ETHIOPIC"),
        ("ETHIOAA", "ETHIOAA"),
        ("ethiopic-amete-alem", "ETHIOAA"),
        ("HEBREW", "HEBREW"),
        ("INDIAN", "INDIAN"),
        ("ISLAMIC-CIVIL", "ISLAMIC-CIVIL"),
        ("islamicc", "ISLAMIC-CIVIL"),
        ("ISLAMIC-TBLA", "ISLAMIC-TBLA"),
        ("ISLAMIC-UMALQURA", "ISLAMIC-UMALQURA"),
        ("ISO8601", "ISO8601"),
        ("JAPANESE", "JAPANESE"),
        ("PERSIAN", "PERSIAN"),
        ("ROC", "ROC"),
    ] {
        assert_eq!(CalendarScale::parse(name).unwrap().name(), expected);
    }
}

#[test]
fn unsupported_or_ambiguous_calendar_names_are_not_guessed() {
    for name in [
        "",
        "ISLAMIC",
        "ISLAMIC-RGSA",
        "X-FUTURE",
        "bad_name",
        "gregory ",
    ] {
        assert!(CalendarScale::parse(name).is_err(), "{name}");
    }
}

#[test]
fn month_identifiers_keep_leap_identity_and_refuse_numeric_overflow() {
    let regular = CalendarMonth::regular(5).unwrap();
    let leap = CalendarMonth::leap(5).unwrap();
    assert_eq!(regular.number(), 5);
    assert_eq!(leap.number(), 5);
    assert!(!regular.is_leap());
    assert!(leap.is_leap());
    assert_ne!(regular, leap);
    assert_eq!(CalendarMonth::regular(13).unwrap().number(), 13);
    assert_eq!(CalendarMonth::regular(99).unwrap().number(), 99);
    for number in [0, 100, u8::MAX] {
        assert!(CalendarMonth::regular(number).is_err());
        assert!(CalendarMonth::leap(number).is_err());
    }
}

#[test]
fn chinese_new_year_examples_convert_in_both_directions() {
    let scale = CalendarScale::parse("CHINESE").unwrap();
    for (year, month, day) in [
        (2013, 2, 10),
        (2014, 1, 31),
        (2015, 2, 19),
        (2016, 2, 8),
        (2017, 1, 28),
    ] {
        let source = gregorian(year, month, day);
        let converted = CalendarDate::from_gregorian(scale, source).unwrap();
        assert_eq!(converted.year(), i32::from(year));
        assert_eq!(converted.month(), CalendarMonth::regular(1).unwrap());
        assert_eq!(converted.day(), 1);
        assert_eq!(converted.scale(), scale);
        assert_eq!(converted.to_gregorian(), source);
        assert_eq!(
            CalendarDate::new(
                scale,
                i32::from(year),
                CalendarMonth::regular(1).unwrap(),
                1
            )
            .unwrap(),
            converted,
        );
    }
}

#[test]
fn ethiopic_thirteenth_month_examples_convert_in_both_directions() {
    let scale = CalendarScale::parse("ETHIOPIC").unwrap();
    for year in 2013..=2017 {
        let source = gregorian(year, 9, 6);
        let converted = CalendarDate::from_gregorian(scale, source).unwrap();
        assert_eq!(converted.year(), i32::from(year) - 8);
        assert_eq!(converted.month(), CalendarMonth::regular(13).unwrap());
        assert_eq!(converted.day(), 1);
        assert_eq!(
            CalendarDate::new(scale, converted.year(), converted.month(), 1)
                .unwrap()
                .to_gregorian(),
            source,
        );
    }
}

#[test]
fn hebrew_leap_month_examples_keep_regular_month_numbers_stable() {
    let scale = CalendarScale::parse("HEBREW").unwrap();
    for (year, leap, date) in [
        (5774, true, gregorian(2014, 2, 8)),
        (5775, false, gregorian(2015, 2, 27)),
        (5776, true, gregorian(2016, 2, 17)),
        (5777, false, gregorian(2017, 3, 6)),
        (5778, false, gregorian(2018, 2, 23)),
    ] {
        let month = if leap {
            CalendarMonth::leap(5)
        } else {
            CalendarMonth::regular(6)
        }
        .unwrap();
        let converted = CalendarDate::new(scale, year, month, 8).unwrap();
        assert_eq!(converted.to_gregorian(), date);
        assert_eq!(
            CalendarDate::from_gregorian(scale, date).unwrap(),
            converted
        );
    }
}

#[test]
fn nonexistent_dates_and_leap_months_are_refused_without_clamping() {
    for (name, year, month, day) in [
        ("GREGORIAN", 2023, CalendarMonth::regular(2).unwrap(), 29),
        ("GREGORIAN", 2024, CalendarMonth::regular(13).unwrap(), 1),
        ("GREGORIAN", 2024, CalendarMonth::leap(2).unwrap(), 1),
        ("ETHIOPIC", 2005, CalendarMonth::regular(13).unwrap(), 6),
        ("HEBREW", 5775, CalendarMonth::leap(5).unwrap(), 8),
        ("HEBREW", 5774, CalendarMonth::regular(99).unwrap(), 1),
        ("CHINESE", 2013, CalendarMonth::regular(1).unwrap(), 0),
        ("CHINESE", i32::MAX, CalendarMonth::regular(1).unwrap(), 1),
        ("CHINESE", i32::MIN, CalendarMonth::regular(1).unwrap(), 1),
        ("GREGORIAN", 10000, CalendarMonth::regular(1).unwrap(), 1),
        ("HEBREW", 50000, CalendarMonth::regular(1).unwrap(), 1),
    ] {
        assert!(CalendarDate::new(CalendarScale::parse(name).unwrap(), year, month, day).is_err());
    }
}

#[test]
fn gregorian_bounds_include_year_zero_and_refuse_negative_dates() {
    let scale = CalendarScale::parse("GREGORY").unwrap();
    for date in [gregorian(0, 1, 1), gregorian(9999, 12, 31)] {
        let converted = CalendarDate::from_gregorian(scale, date).unwrap();
        assert_eq!(converted.to_gregorian(), date);
        assert_eq!(
            CalendarDate::new(scale, converted.year(), converted.month(), converted.day()).unwrap(),
            converted
        );
    }
    assert!(CalendarDate::from_gregorian(scale, gregorian(-1, 12, 31)).is_err());
    assert!(CalendarDate::new(scale, -1, CalendarMonth::regular(12).unwrap(), 31).is_err());
}

#[test]
fn every_supported_calendar_roundtrips_across_nineteen_years() {
    let names = [
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
    let start = gregorian(2019, 1, 1);
    for name in names {
        let scale = CalendarScale::parse(name).unwrap();
        let mut source = start;
        while source < gregorian(2038, 1, 1) {
            let converted = CalendarDate::from_gregorian(scale, source).unwrap();
            assert_eq!(converted.to_gregorian(), source, "{name} {source}");
            let rebuilt =
                CalendarDate::new(scale, converted.year(), converted.month(), converted.day())
                    .unwrap();
            assert_eq!(rebuilt, converted, "{name} {source}");
            source = source.checked_add(jiff::ToSpan::days(1)).unwrap();
        }
    }
}

#[test]
fn every_supported_calendar_preserves_the_gregorian_range_boundaries() {
    for name in [
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
    ] {
        let scale = CalendarScale::parse(name).unwrap();
        for source in [gregorian(0, 1, 1), gregorian(9999, 12, 31)] {
            let converted = CalendarDate::from_gregorian(scale, source).unwrap();
            assert_eq!(converted.to_gregorian(), source, "{name} {source}");
            let rebuilt =
                CalendarDate::new(scale, converted.year(), converted.month(), converted.day())
                    .unwrap_or_else(|error| panic!("{name} {source} {converted:?}: {error:?}"));
            assert_eq!(rebuilt.to_gregorian(), source, "{name} {source}");
        }
    }
}

#[test]
fn gregorian_based_calendars_use_their_extended_years() {
    let source = gregorian(1992, 9, 2);
    for (name, year, month, day) in [
        ("GREGORIAN", 1992, 9, 2),
        ("ISO8601", 1992, 9, 2),
        ("BUDDHIST", 2535, 9, 2),
        ("ROC", 81, 9, 2),
        ("JAPANESE", 1992, 9, 2),
        ("INDIAN", 1914, 6, 11),
    ] {
        let scale = CalendarScale::parse(name).unwrap();
        let converted = CalendarDate::from_gregorian(scale, source).unwrap();
        assert_eq!(
            (
                converted.year(),
                converted.month().number(),
                converted.day()
            ),
            (year, month, day)
        );
        assert_eq!(converted.to_gregorian(), source);
    }
}

#[test]
fn conversion_and_recurrence_refuse_ambiguous_calendar_algorithms() {
    for name in ["ISLAMIC", "ISLAMIC-RGSA", "X-FUTURE"] {
        assert!(CalendarScale::parse(name).is_err());
        let rule = format!("FREQ=YEARLY;RSCALE={name}");
        let error = icalkit::recurrence::Rule::parse(&rule).unwrap_err();
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code().as_str() == "unsupported-recurrence-semantics")
        );
    }
}
