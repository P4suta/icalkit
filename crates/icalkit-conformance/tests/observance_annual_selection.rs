// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Annual day and position selection follows [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
//! Independent date enumeration checks patterns separately from original anchors.

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

#[derive(Clone, Copy)]
struct Term {
    weekday: Weekday,
    ordinal: Option<i8>,
}

#[derive(Clone, Copy)]
struct Spec<'a> {
    months: Option<&'a [u8]>,
    year_days: Option<&'a [i16]>,
    month_days: Option<&'a [i8]>,
    weekdays: &'a [Term],
    positions: Option<&'a [i16]>,
    default_month: u8,
    default_day: u8,
}

impl Spec<'_> {
    fn new() -> Self {
        Self {
            months: None,
            year_days: None,
            month_days: None,
            weekdays: &[],
            positions: None,
            default_month: 1,
            default_day: 1,
        }
    }

    fn parts(self) -> String {
        let mut parts = Vec::new();
        if let Some(months) = self.months {
            parts.push(format!(
                "BYMONTH={}",
                months
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if let Some(days) = self.year_days {
            parts.push(format!("BYYEARDAY={}", list(days)));
        }
        if let Some(days) = self.month_days {
            parts.push(format!(
                "BYMONTHDAY={}",
                days.iter().map(i8::to_string).collect::<Vec<_>>().join(",")
            ));
        }
        if !self.weekdays.is_empty() {
            let terms = self
                .weekdays
                .iter()
                .map(|term| {
                    let name = std::str::from_utf8(term.weekday.as_bytes()).unwrap();
                    match term.ordinal {
                        None => name.to_owned(),
                        Some(number) => format!("{number}{name}"),
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("BYDAY={terms}"));
        }
        if let Some(positions) = self.positions {
            parts.push(format!("BYSETPOS={}", list(positions)));
        }
        parts.join(";")
    }

    fn start(self) -> String {
        format!(
            "2000{:02}{:02}T020000",
            self.default_month, self.default_day
        )
    }
}

fn list(values: &[i16]) -> String {
    values
        .iter()
        .map(i16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn signed_positions() -> Vec<i16> {
    (-366_i16..=366).filter(|value| *value != 0).collect()
}

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(";{parts}")
    };
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit annual selection//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/AnnualSelection\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY{suffix}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    assert_eq!(document.to_bytes(), text.as_bytes());
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    (zones, meter, diagnostics)
}

fn admitted(spec: Spec<'_>) -> VtimezoneSet {
    let parts = spec.parts();
    let (zones, _, diagnostics) = read(&parts, &spec.start(), "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    zones
}

fn compiled_dates(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    let dates: Vec<_> = zones
        .table("Test/AnnualSelection")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|value| value.rule().unwrap().occurrence_in(year))
        .collect();
    let unique: BTreeSet<_> = dates.iter().copied().collect();
    assert_eq!(dates.len(), unique.len(), "duplicate dates in {year}");
    unique
}

fn insert_weekday(period: &[CivilDate], term: Term, found: &mut BTreeSet<CivilDate>) {
    let matching: Vec<_> = period
        .iter()
        .copied()
        .filter(|value| value.weekday() == Some(term.weekday))
        .collect();
    match term.ordinal {
        None => found.extend(matching),
        Some(number) => {
            let index = if number.is_positive() {
                usize::try_from(number).unwrap().checked_sub(1)
            } else {
                matching
                    .len()
                    .checked_sub(usize::from(number.unsigned_abs()))
            };
            if let Some(value) = index.and_then(|index| matching.get(index)) {
                found.insert(*value);
            }
        },
    }
}

fn expected_dates(year: u16, spec: Spec<'_>) -> BTreeSet<CivilDate> {
    let months: Vec<Vec<_>> = (1..=12)
        .map(|month| {
            (1..=31)
                .filter_map(|day| CivilDate::from_ymd(year, month, day))
                .collect()
        })
        .collect();
    let all: Vec<_> = months.iter().flatten().copied().collect();
    let mut weekdays = BTreeSet::new();
    if let Some(selected_months) = spec.months {
        for month in selected_months {
            let period = &months[usize::from(month.checked_sub(1).unwrap())];
            for term in spec.weekdays {
                insert_weekday(period, *term, &mut weekdays);
            }
        }
    } else {
        for term in spec.weekdays {
            insert_weekday(&all, *term, &mut weekdays);
        }
    }
    let candidates: Vec<_> = all
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if spec
                .months
                .is_some_and(|months| !months.contains(&value.month()))
            {
                return None;
            }
            let positive = i16::try_from(index.checked_add(1).unwrap()).unwrap();
            let negative = i16::try_from(index)
                .unwrap()
                .checked_sub(i16::try_from(all.len()).unwrap())
                .unwrap();
            if spec
                .year_days
                .is_some_and(|days| !days.contains(&positive) && !days.contains(&negative))
            {
                return None;
            }
            let month_length = months[usize::from(value.month().checked_sub(1).unwrap())].len();
            let backward = i16::from(value.day())
                .checked_sub(i16::try_from(month_length).unwrap())
                .unwrap()
                .checked_sub(1)
                .unwrap();
            if spec.month_days.is_some_and(|days| {
                !days.contains(&i8::try_from(value.day()).unwrap())
                    && !days.contains(&i8::try_from(backward).unwrap())
            }) {
                return None;
            }
            if !spec.weekdays.is_empty() && !weekdays.contains(value) {
                return None;
            }
            if spec.year_days.is_none()
                && spec.month_days.is_none()
                && spec.weekdays.is_empty()
                && (value.day() != spec.default_day
                    || spec.months.is_none() && value.month() != spec.default_month)
            {
                return None;
            }
            Some(*value)
        })
        .collect();
    if let Some(positions) = spec.positions {
        positions
            .iter()
            .filter_map(|number| {
                let index = if number.is_positive() {
                    usize::try_from(*number).unwrap().checked_sub(1)
                } else {
                    candidates
                        .len()
                        .checked_sub(usize::from(number.unsigned_abs()))
                };
                index.and_then(|index| candidates.get(index)).copied()
            })
            .collect()
    } else {
        candidates.into_iter().collect()
    }
}

fn years() -> Vec<u16> {
    let mut seen = BTreeSet::new();
    let mut years = Vec::new();
    for year in 1800..2200 {
        if seen.insert((
            CivilDate::from_ymd(year, 2, 29).is_some(),
            date(year, 1, 1).weekday().unwrap(),
        )) {
            years.push(year);
        }
    }
    assert_eq!(years.len(), 14);
    years.extend([0, 9999]);
    years
}

#[test]
fn every_signed_year_day_matches_independent_dates() {
    let years = years();
    for value in signed_positions() {
        let days = [value];
        let spec = Spec {
            year_days: Some(&days),
            ..Spec::new()
        };
        let zones = admitted(spec);
        for &year in &years {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, spec),
                "day={value}, year={year}"
            );
        }
    }
}

#[test]
fn every_signed_set_position_counts_existing_candidates() {
    let days: Vec<_> = (1_i16..=366).collect();
    let weekdays = [
        Term {
            weekday: Weekday::Monday,
            ordinal: None,
        },
        Term {
            weekday: Weekday::Sunday,
            ordinal: Some(1),
        },
    ];
    let bases = [
        Spec {
            year_days: Some(&days),
            ..Spec::new()
        },
        Spec {
            months: Some(&[2, 3, 9]),
            month_days: Some(&[1, 15, -1]),
            weekdays: &weekdays,
            ..Spec::new()
        },
        Spec {
            months: Some(&[2, 4]),
            default_month: 2,
            default_day: 29,
            ..Spec::new()
        },
    ];
    let years = years();
    for value in signed_positions() {
        let positions = [value];
        for base in bases {
            let spec = Spec {
                positions: Some(&positions),
                ..base
            };
            let zones = admitted(spec);
            for &year in &years {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, spec),
                    "{}, year={year}",
                    spec.parts()
                );
            }
        }
    }
}

#[test]
fn year_days_intersect_month_days_and_weekdays() {
    let days = [
        1, 32, 60, 61, 100, 150, 200, 250, 365, 366, -1, -2, -306, -366,
    ];
    let weekdays = [
        Term {
            weekday: Weekday::Monday,
            ordinal: None,
        },
        Term {
            weekday: Weekday::Sunday,
            ordinal: Some(1),
        },
        Term {
            weekday: Weekday::Friday,
            ordinal: Some(-1),
        },
    ];
    for months in [
        None,
        Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12].as_slice()),
        Some([2, 3, 9].as_slice()),
    ] {
        for month_days in [None, Some([1, 28, 29, 30, 31, -1, -2].as_slice())] {
            for terms in [weekdays.as_slice(), &[]] {
                for positions in [None, Some([1, 2, -1, -2].as_slice())] {
                    let spec = Spec {
                        months,
                        year_days: Some(&days),
                        month_days,
                        weekdays: terms,
                        positions,
                        ..Spec::new()
                    };
                    let zones = admitted(spec);
                    for year in years() {
                        assert_eq!(
                            compiled_dates(&zones, year),
                            expected_dates(year, spec),
                            "{}, year={year}",
                            spec.parts()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn set_positions_select_from_the_complete_year() {
    let terms = [Term {
        weekday: Weekday::Sunday,
        ordinal: None,
    }];
    let spec = Spec {
        months: Some(&[3, 9]),
        weekdays: &terms,
        positions: Some(&[1, -1]),
        ..Spec::new()
    };
    let zones = admitted(spec);
    assert_eq!(
        compiled_dates(&zones, 2024),
        [date(2024, 3, 3), date(2024, 9, 29)].into_iter().collect()
    );
    assert_eq!(
        zones.table("Test/AnnualSelection").unwrap().rules().len(),
        2
    );
    for year in years() {
        assert_eq!(compiled_dates(&zones, year), expected_dates(year, spec));
    }
}

#[test]
fn duplicates_and_missing_dates_precede_position_selection() {
    for spec in [
        Spec {
            months: Some(&[2, 4]),
            month_days: Some(&[1, -28]),
            positions: Some(&[2]),
            ..Spec::new()
        },
        Spec {
            year_days: Some(&[1, -365]),
            positions: Some(&[2]),
            ..Spec::new()
        },
        Spec {
            months: Some(&[1, 3]),
            month_days: Some(&[15, 1, 1]),
            positions: Some(&[3, -2]),
            ..Spec::new()
        },
    ] {
        let zones = admitted(spec);
        for year in years() {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, spec),
                "{}, year={year}",
                spec.parts()
            );
        }
    }
}

#[test]
fn default_days_exclude_missing_dates_before_position_counting() {
    for spec in [
        Spec {
            months: Some(&[2, 3, 4]),
            positions: Some(&[1, -1]),
            default_month: 3,
            default_day: 31,
            ..Spec::new()
        },
        Spec {
            months: Some(&[2, 4]),
            positions: Some(&[1]),
            default_month: 2,
            default_day: 29,
            ..Spec::new()
        },
    ] {
        let zones = admitted(spec);
        for year in years() {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, spec),
                "{}, year={year}",
                spec.parts()
            );
        }
    }
}

#[test]
fn selection_precedes_anchor_and_until_clipping() {
    let (zones, _, diagnostics) = read(
        "BYMONTHDAY=1;BYSETPOS=3,-1;UNTIL=20250301T020000Z",
        "20240301T020000",
        "",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        compiled_dates(&zones, 2024),
        [date(2024, 3, 1), date(2024, 12, 1)].into_iter().collect()
    );
    assert_eq!(
        compiled_dates(&zones, 2025),
        [date(2025, 3, 1)].into_iter().collect()
    );
    let month_days: Vec<i8> = (1..=31).collect();
    let spec = Spec {
        month_days: Some(&month_days),
        positions: Some(&[3, -2]),
        ..Spec::new()
    };
    let (zones, _, diagnostics) = read(
        &format!("{};UNTIL=20250103T020000Z", spec.parts()),
        "20240103T020000",
        "",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        compiled_dates(&zones, 2024),
        [date(2024, 1, 3), date(2024, 12, 30)].into_iter().collect()
    );
    assert_eq!(
        compiled_dates(&zones, 2025),
        [date(2025, 1, 3)].into_iter().collect()
    );
}

#[test]
fn positions_follow_all_filters_over_a_gregorian_cycle() {
    let terms = [
        Term {
            weekday: Weekday::Tuesday,
            ordinal: None,
        },
        Term {
            weekday: Weekday::Sunday,
            ordinal: Some(1),
        },
        Term {
            weekday: Weekday::Friday,
            ordinal: Some(-1),
        },
    ];
    for months in [None, Some([2, 3, 9, 12].as_slice())] {
        let spec = Spec {
            months,
            year_days: Some(&[1, 2, 60, 61, 100, 200, 365, 366, -1, -2, -306, -366]),
            month_days: Some(&[1, 2, 15, 28, 29, 30, 31, -1, -2]),
            weekdays: &terms,
            positions: Some(&[1, 2, -1]),
            ..Spec::new()
        };
        let zones = admitted(spec);
        for year in 2000..2400 {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, spec),
                "{}, year={year}",
                spec.parts()
            );
        }
    }
}

#[test]
fn duplicate_spellings_and_part_order_keep_unique_dates() {
    let repeated = (0..80)
        .map(|_| "001,100,-001")
        .collect::<Vec<_>>()
        .join(",");
    for parts in [
        String::from("BYYEARDAY=001,100,-001;BYSETPOS=+001,-001,1"),
        String::from("BYSETPOS=-1,1;BYYEARDAY=-1,001,100,100,1"),
        format!("BYYEARDAY={repeated};BYSETPOS=1,-1,-001,+001"),
    ] {
        let (zones, meter, diagnostics) = read(&parts, "20000101T020000", "", Limits::DEFAULT);
        assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
        assert_eq!(meter.vtimezone_observances(), 2);
        for year in years() {
            assert_eq!(
                compiled_dates(&zones, year),
                [date(year, 1, 1), date(year, 12, 31)].into_iter().collect()
            );
        }
    }
}

#[test]
fn complete_signed_domains_stay_within_366_patterns() {
    let domain = signed_positions();
    let spec = Spec {
        year_days: Some(&domain),
        positions: Some(&domain),
        ..Spec::new()
    };
    let (zones, meter, diagnostics) = read(&spec.parts(), &spec.start(), "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(meter.vtimezone_observances(), 366);
    assert_eq!(
        zones.table("Test/AnnualSelection").unwrap().rules().len(),
        366
    );
    for year in years() {
        assert_eq!(compiled_dates(&zones, year), expected_dates(year, spec));
    }
}

#[test]
fn yearly_selection_keeps_metadata_and_explicit_dates() {
    let (zones, meter, diagnostics) = read(
        "BYYEARDAY=1,100,200,-1;BYSETPOS=1,-1;UNTIL=20251231T020000Z",
        "20240101T020000",
        "RDATE:20240820T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/AnnualSelection").unwrap();
    assert_eq!(table.rules().len(), 2);
    assert_eq!(meter.vtimezone_observances(), 3);
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    for observance in table.rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 1, 1), clock)
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(3600).unwrap()
        );
        assert_eq!(observance.rule().unwrap().at(), clock);
        assert_eq!(
            observance.rule().unwrap().through(),
            Some(date(2025, 12, 31))
        );
        assert_eq!(observance.transition_in(2023), None);
        assert_eq!(observance.transition_in(2026), None);
    }
    let explicit: Vec<_> = table
        .observances()
        .iter()
        .filter(|value| value.rule().is_none())
        .map(|value| value.start())
        .collect();
    assert_eq!(explicit, [CivilDateTime::new(date(2024, 8, 20), clock)]);
    assert_eq!(
        compiled_dates(&zones, 2025),
        [date(2025, 1, 1), date(2025, 12, 31)].into_iter().collect()
    );
    assert_eq!(compiled_dates(&zones, 2026), BTreeSet::new());
    assert_eq!(table.coverage_end(), Some(date(2025, 12, 31)));
}

#[test]
fn every_compiled_pattern_is_charged_at_the_table_limit() {
    for cap in [0_u32, 1, 2, 3, 4] {
        let (zones, meter, diagnostics) = read(
            "BYYEARDAY=1,100,200,-1;BYSETPOS=1,-1",
            "20240101T020000",
            "RDATE:20240820T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/AnnualSelection").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(3));
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(3)).unwrap()
        );
        assert_eq!(table.is_truncated(), cap < 3);
        if cap < 3 {
            assert_eq!(
                diagnostics
                    .iter()
                    .copied()
                    .map(Diagnostic::code)
                    .collect::<Vec<_>>(),
                [DiagnosticCode::VtimezoneObservancesTruncated]
            );
            assert_eq!(diagnostics[0].severity(), Severity::LimitReached);
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        }
    }
}

#[test]
fn malformed_or_repeated_annual_parts_keep_one_anchor() {
    let mut cases = Vec::new();
    for field in ["BYYEARDAY", "BYSETPOS"] {
        for value in [
            "", "0", "+0", "-0", "367", "-367", "0001", "+0001", "-0001", "1,", ",1", "1,,2",
            "1,x", "1,0",
        ] {
            cases.push(format!("BYMONTH=1;{field}={value}"));
        }
        cases.push(format!("BYMONTH=1;{field}=1;{field}=-1"));
    }
    for parts in cases {
        let (zones, meter, diagnostics) = read(&parts, "20240101T020000", "", Limits::DEFAULT);
        let table = zones.table("Test/AnnualSelection").unwrap();
        assert_eq!(table.rules(), [], "{parts}");
        assert_eq!(table.observances().len(), 1, "{parts}");
        assert_eq!(meter.vtimezone_observances(), 1, "{parts}");
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported],
            "{parts}"
        );
    }
}

#[test]
fn position_selection_requires_another_selector() {
    for parts in ["BYSETPOS=1", "BYSETPOS=-1,1"] {
        let (zones, _, diagnostics) = read(parts, "20240101T020000", "", Limits::DEFAULT);
        assert_eq!(zones.table("Test/AnnualSelection").unwrap().rules(), []);
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [DiagnosticCode::VtimezoneRuleUnsupported]
        );
    }
}
