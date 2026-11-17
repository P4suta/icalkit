// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Weekday unions and intersections follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) and [verified erratum 1913](https://errata.rfc-editor.org/eid1913).
//! Date enumeration checks compiled patterns separately from original anchors.

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, YearlyRule, read_calendar_zones};

const MONTHS: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

#[derive(Clone, Copy, Debug)]
struct Term {
    weekday: Weekday,
    ordinal: Option<i8>,
}

fn bare(weekday: Weekday) -> Term {
    Term {
        weekday,
        ordinal: None,
    }
}

fn numbered(weekday: Weekday, ordinal: i8) -> Term {
    Term {
        weekday,
        ordinal: Some(ordinal),
    }
}

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn day_numbers() -> Vec<i8> {
    (-31_i8..=31).filter(|day| *day != 0).collect()
}

fn rule_parts(months: Option<&[u8]>, days: Option<&[i8]>, terms: &[Term]) -> String {
    let mut parts = Vec::new();
    if let Some(months) = months {
        parts.push(format!(
            "BYMONTH={}",
            months
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if let Some(days) = days {
        parts.push(format!(
            "BYMONTHDAY={}",
            days.iter().map(i8::to_string).collect::<Vec<_>>().join(",")
        ));
    }
    let weekdays = terms
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
    parts.push(format!("BYDAY={weekdays}"));
    parts.join(";")
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit weekday sets//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/WeekdaySets\r\nBEGIN:DAYLIGHT\r\n\
         DTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;{parts}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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

fn admitted(parts: &str) -> VtimezoneSet {
    let (zones, _, diagnostics) = read(parts, "20000101T020000", "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
    zones
}

fn compiled_dates(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    let dates: Vec<_> = zones
        .table("Test/WeekdaySets")
        .unwrap()
        .rules()
        .iter()
        .filter_map(|observance| observance.rule().unwrap().occurrence_in(year))
        .collect();
    let unique: BTreeSet<_> = dates.iter().copied().collect();
    assert_eq!(dates.len(), unique.len(), "duplicate dates in {year}");
    unique
}

fn insert_term(period: &[CivilDate], term: Term, selected: &mut BTreeSet<CivilDate>) {
    let matching: Vec<_> = period
        .iter()
        .copied()
        .filter(|value| value.weekday() == Some(term.weekday))
        .collect();
    if let Some(ordinal) = term.ordinal {
        let index = if ordinal.is_positive() {
            usize::try_from(ordinal).unwrap().checked_sub(1)
        } else {
            matching
                .len()
                .checked_sub(usize::from(ordinal.unsigned_abs()))
        };
        if let Some(value) = index.and_then(|index| matching.get(index)) {
            selected.insert(*value);
        }
    } else {
        selected.extend(matching);
    }
}

fn expected_dates(
    year: u16,
    months: Option<&[u8]>,
    days: Option<&[i8]>,
    terms: &[Term],
) -> BTreeSet<CivilDate> {
    let calendar: Vec<Vec<_>> = MONTHS
        .iter()
        .map(|month| {
            (1..=31)
                .filter_map(|day| CivilDate::from_ymd(year, *month, day))
                .collect()
        })
        .collect();
    let mut selected = BTreeSet::new();
    if let Some(months) = months {
        for month in months {
            let period = &calendar[usize::from(month.checked_sub(1).unwrap())];
            for &term in terms {
                insert_term(period, term, &mut selected);
            }
        }
    } else {
        let period: Vec<_> = calendar.iter().flatten().copied().collect();
        for &term in terms {
            insert_term(&period, term, &mut selected);
        }
    }
    selected
        .into_iter()
        .filter(|value| {
            days.is_none_or(|days| {
                let length = calendar[usize::from(value.month().checked_sub(1).unwrap())].len();
                let positive = i8::try_from(value.day()).unwrap();
                let negative = i16::from(positive)
                    .checked_sub(i16::try_from(length).unwrap())
                    .unwrap()
                    .checked_sub(1)
                    .unwrap();
                days.contains(&positive) || days.contains(&i8::try_from(negative).unwrap())
            })
        })
        .collect()
}

fn year_layout_examples() -> Vec<u16> {
    let mut layouts = BTreeSet::new();
    let mut years = Vec::new();
    for year in 1800..2200 {
        let layout = (
            CivilDate::from_ymd(year, 2, 29).is_some(),
            date(year, 1, 1).weekday().unwrap(),
        );
        if layouts.insert(layout) {
            years.push(year);
        }
    }
    assert_eq!(years.len(), 14);
    years.extend([0, 9999]);
    years
}

#[test]
fn every_bare_weekday_subset_matches_all_year_layouts() {
    let mut subsets = vec![Vec::new()];
    for weekday in Weekday::ALL {
        let added: Vec<_> = subsets
            .iter()
            .cloned()
            .map(|mut values| {
                values.push(weekday);
                values
            })
            .collect();
        subsets.extend(added);
    }
    assert_eq!(subsets.len(), 128);
    for weekdays in subsets.into_iter().filter(|values| !values.is_empty()) {
        let terms: Vec<_> = weekdays.into_iter().map(bare).collect();
        for months in [None, Some([2, 4, 9].as_slice())] {
            let parts = rule_parts(months, None, &terms);
            let zones = admitted(&parts);
            for year in year_layout_examples() {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, months, None, &terms),
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn every_signed_ordinal_intersects_month_days_in_its_written_scope() {
    let days = day_numbers();
    for weekday in Weekday::ALL {
        for ordinal in (-53_i8..=53).filter(|value| *value != 0) {
            let terms = [numbered(weekday, ordinal)];
            for months in [None, Some(MONTHS.as_slice())] {
                let parts = rule_parts(months, Some(&days), &terms);
                let zones = admitted(&parts);
                for year in year_layout_examples() {
                    assert_eq!(
                        compiled_dates(&zones, year),
                        expected_dates(year, months, Some(&days), &terms),
                        "{parts}, year={year}"
                    );
                }
            }
        }
    }
}

#[test]
fn mixed_weekday_sets_follow_a_gregorian_cycle() {
    let terms = [
        bare(Weekday::Tuesday),
        numbered(Weekday::Sunday, 1),
        numbered(Weekday::Friday, -1),
        numbered(Weekday::Monday, 53),
        numbered(Weekday::Tuesday, -53),
    ];
    let days = [1, 2, 8, 15, 28, 29, 30, 31, -1, -2, -28, -31];
    for months in [None, Some(MONTHS.as_slice())] {
        for selected_days in [None, Some(days.as_slice())] {
            let parts = rule_parts(months, selected_days, &terms);
            let zones = admitted(&parts);
            for year in 2000..2400 {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, months, selected_days, &terms),
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn ordinal_scope_is_applied_before_the_month_day_filter() {
    for (days, terms) in [
        (vec![1], vec![numbered(Weekday::Monday, 1)]),
        ((8..=14).collect(), vec![numbered(Weekday::Monday, 1)]),
        ((1..=7).collect(), vec![numbered(Weekday::Monday, 2)]),
        ((13..=19).collect(), vec![numbered(Weekday::Monday, 20)]),
        (
            vec![-1, 1, 15],
            vec![
                numbered(Weekday::Monday, 1),
                bare(Weekday::Friday),
                numbered(Weekday::Sunday, -1),
            ],
        ),
    ] {
        for months in [None, Some(MONTHS.as_slice()), Some([3, 9].as_slice())] {
            let parts = rule_parts(months, Some(&days), &terms);
            let zones = admitted(&parts);
            for year in year_layout_examples() {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, months, Some(&days), &terms),
                    "{parts}, year={year}"
                );
            }
        }
    }
}

#[test]
fn ordinal_unions_deduplicate_coincident_dates() {
    for (months, terms, expected_count) in [
        (
            None,
            vec![numbered(Weekday::Monday, 1), numbered(Weekday::Monday, -53)],
            1,
        ),
        (
            None,
            vec![numbered(Weekday::Monday, 53), numbered(Weekday::Monday, -1)],
            1,
        ),
        (
            Some([3].as_slice()),
            vec![numbered(Weekday::Sunday, 4), numbered(Weekday::Sunday, -1)],
            2,
        ),
        (
            Some([3].as_slice()),
            vec![numbered(Weekday::Sunday, -2), numbered(Weekday::Sunday, -1)],
            2,
        ),
    ] {
        let parts = rule_parts(months, None, &terms);
        let zones = admitted(&parts);
        assert_eq!(
            zones.table("Test/WeekdaySets").unwrap().rules().len(),
            expected_count,
            "{parts}"
        );
        for year in year_layout_examples() {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, months, None, &terms),
                "{parts}, year={year}"
            );
        }
    }
}

#[test]
fn reordered_repeated_and_zero_padded_terms_keep_the_same_patterns() {
    let repeated = (0..80)
        .map(|_| "tu,-01FR,+01MO,tu,1mo,-1fr")
        .collect::<Vec<_>>()
        .join(",");
    let terms = [
        bare(Weekday::Tuesday),
        numbered(Weekday::Friday, -1),
        numbered(Weekday::Monday, 1),
    ];
    for prefix in ["", "BYMONTH=2,4,9;"] {
        let months = if prefix.is_empty() {
            None
        } else {
            Some([2, 4, 9].as_slice())
        };
        let baseline = admitted(&format!("{prefix}BYDAY=1MO,-1FR,TU"));
        for written in [
            "1MO,-1FR,TU",
            "tu,-01FR,+01MO,tu,1mo,-1fr",
            repeated.as_str(),
        ] {
            let zones = admitted(&format!("{prefix}BYDAY={written}"));
            assert_eq!(
                zones.table("Test/WeekdaySets").unwrap().rules().len(),
                baseline.table("Test/WeekdaySets").unwrap().rules().len()
            );
            for year in year_layout_examples() {
                assert_eq!(
                    compiled_dates(&zones, year),
                    expected_dates(year, months, None, &terms)
                );
            }
        }
    }
}

#[test]
fn bare_weekdays_absorb_their_numeric_terms() {
    for written in ["MO,1MO,-53MO", "1MO,-53MO,MO", "-53MO,MO,+01MO,MO"] {
        let zones = admitted(&format!("BYDAY={written}"));
        for year in year_layout_examples() {
            assert_eq!(
                compiled_dates(&zones, year),
                expected_dates(year, None, None, &[bare(Weekday::Monday)])
            );
        }
    }
}

#[test]
fn the_full_weekday_domain_stays_within_366_patterns() {
    let mut terms = Vec::new();
    for weekday in Weekday::ALL {
        terms.push(bare(weekday));
        terms.extend(
            (-53_i8..=53)
                .filter(|value| *value != 0)
                .map(|ordinal| numbered(weekday, ordinal)),
        );
    }
    assert_eq!(terms.len(), 749);
    let (zones, meter, diagnostics) = read(
        &rule_parts(None, None, &terms),
        "20000101T020000",
        "",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(zones.table("Test/WeekdaySets").unwrap().rules().len(), 366);
    assert_eq!(meter.vtimezone_observances(), 366);
    let bare_terms: Vec<_> = Weekday::ALL.into_iter().map(bare).collect();
    for year in year_layout_examples() {
        assert_eq!(
            compiled_dates(&zones, year),
            expected_dates(year, None, None, &bare_terms)
        );
    }
}

#[test]
fn compact_single_patterns_keep_their_scope() {
    for (parts, count, month) in [
        ("BYDAY=20MO", 1, None),
        ("BYMONTH=3;BYDAY=-1SU", 1, Some(3)),
        ("BYMONTH=3;BYDAY=-01SU,-1SU", 1, Some(3)),
    ] {
        let zones = admitted(parts);
        let table = zones.table("Test/WeekdaySets").unwrap();
        assert_eq!(table.rules().len(), count);
        assert_eq!(table.rules()[0].rule().unwrap().month(), month);
    }
}

#[test]
fn compiled_masks_cannot_be_attached_to_another_month() {
    let zones = admitted("BYMONTH=3;BYDAY=1MO,2TU");
    let rule = zones.table("Test/WeekdaySets").unwrap().rules()[0]
        .rule()
        .unwrap();
    assert!(YearlyRule::new(3, rule.day(), rule.at(), rule.through()).is_some());
    assert_eq!(
        YearlyRule::new(4, rule.day(), rule.at(), rule.through()),
        None
    );
}

#[test]
fn compiled_weekday_sets_keep_anchors_clocks_offsets_and_end_bounds() {
    let (zones, meter, diagnostics) = read(
        "BYMONTH=3,9;BYDAY=1SU,3SU;UNTIL=20250921T020000Z",
        "20240303T020000",
        "RDATE:20240820T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table("Test/WeekdaySets").unwrap();
    assert_eq!(table.rules().len(), 4);
    assert_eq!(meter.vtimezone_observances(), 5);
    let clock = CivilTime::from_hms(2, 0, 0).unwrap();
    for observance in table.rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 3, 3), clock)
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(
            observance.offset_to(),
            UtcOffset::from_seconds(3600).unwrap()
        );
        assert_eq!(observance.rule().unwrap().at(), clock);
        assert_eq!(
            observance.rule().unwrap().through(),
            Some(date(2025, 9, 21))
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
        [
            date(2025, 3, 2),
            date(2025, 3, 16),
            date(2025, 9, 7),
            date(2025, 9, 21)
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(compiled_dates(&zones, 2026), BTreeSet::new());
    assert_eq!(table.coverage_end(), Some(date(2025, 9, 21)));
}

#[test]
fn year_scoped_lists_keep_one_pattern_per_possible_onset_month() {
    let zones = admitted("BYDAY=1MO,20MO;UNTIL=20250519T020000Z");
    assert_eq!(zones.table("Test/WeekdaySets").unwrap().rules().len(), 2);
    assert_eq!(
        compiled_dates(&zones, 2025),
        [date(2025, 1, 6), date(2025, 5, 19)].into_iter().collect()
    );
    assert_eq!(compiled_dates(&zones, 2026), BTreeSet::new());
}

#[test]
fn weekday_sets_charge_each_stored_pattern_to_the_table_limit() {
    for cap in [0_u32, 1, 3, 4, 5, 6] {
        let (zones, meter, diagnostics) = read(
            "BYMONTH=3,9;BYDAY=1SU,3SU",
            "20240303T020000",
            "RDATE:20240820T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table("Test/WeekdaySets").unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(5));
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(5)).unwrap()
        );
        assert_eq!(table.is_truncated(), cap < 5);
        if cap < 5 {
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
fn malformed_or_repeated_weekday_parts_keep_one_anchor_and_diagnostic() {
    for parts in [
        "BYDAY=",
        "BYDAY=MO,",
        "BYDAY=,MO",
        "BYDAY=MO,,TU",
        "BYDAY=MO,XX",
        "BYDAY=1",
        "BYDAY=0MO",
        "BYDAY=-0MO",
        "BYDAY=54MO",
        "BYDAY=-54MO",
        "BYDAY=MO,54MO",
        "BYDAY=MO,001MO",
        "BYDAY=MO,+001MO",
        "BYDAY=MO;BYDAY=TU",
        "BYDAY=MO;BYDAY=MO",
        "BYDAY=1MO,-1MO;BYDAY=MO",
        "BYDAY=1MO;BYDAY=2MO",
        "BYDAY=1MO;BYDAY=1MO",
        "BYMONTH=3;BYDAY=1MO;BYDAY=2MO",
    ] {
        let (zones, meter, diagnostics) = read(parts, "20240101T020000", "", Limits::DEFAULT);
        let table = zones.table("Test/WeekdaySets").unwrap();
        assert!(table.rules().is_empty(), "{parts}");
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
