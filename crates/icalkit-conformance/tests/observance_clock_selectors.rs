// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Annual clocks follow RFC 5545 sections 3.3.10 and 3.6.5.

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, Limits, Meter,
    Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

const TZID: &str = "Test/Clocks";

fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> CivilDateTime {
    CivilDateTime::new(
        CivilDate::from_ymd(year, month, day).unwrap(),
        CivilTime::from_hms(hour, minute, second).unwrap(),
    )
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limit: u32,
    spent: u32,
) -> (VtimezoneSet, Vec<Diagnostic>) {
    let rule = format!("RRULE:FREQ=YEARLY;{parts}");
    let folded = rule
        .as_bytes()
        .chunks(74)
        .map(|bytes| std::str::from_utf8(bytes).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n ");
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit clocks//EN\r\nBEGIN:VTIMEZONE\r\nTZID:{TZID}\r\nBEGIN:DAYLIGHT\r\nDTSTART:{start}\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0300\r\n{folded}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document = Document::parse(text.as_bytes(), Limits::GENEROUS, &mut Vec::new()).unwrap();
    let mut meter = Meter::new(Limits::GENEROUS.with_max_vtimezone_observances(limit));
    meter.try_charge_vtimezone_observances(spent).unwrap();
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    assert_eq!(document.to_bytes(), text.as_bytes());
    (zones, diagnostics)
}

fn admitted(parts: &str, start: &str) -> VtimezoneSet {
    let (zones, diagnostics) = read(parts, start, "", 4096, 0);
    assert_eq!(diagnostics.as_slice(), [], "{parts}, {start}");
    zones
}

fn onsets(zones: &VtimezoneSet, years: impl IntoIterator<Item = u16>) -> BTreeSet<CivilDateTime> {
    let mut result = BTreeSet::new();
    for year in years {
        for observance in zones.table(TZID).unwrap().observances() {
            if observance.start().date().year() == year {
                result.insert(observance.start());
            }
            result.extend(observance.transition_in(year));
        }
    }
    result
}

#[test]
fn clock_fields_default_independently_and_form_complete_products() {
    let anchor = stamp(2024, 5, 10, 2, 15, 37);
    for (parts, hours, minutes, seconds) in [
        ("BYHOUR=1,02,3,2", vec![1, 2, 3], vec![15], vec![37]),
        ("BYMINUTE=5,15", vec![2], vec![5, 15], vec![37]),
        ("BYSECOND=0,37", vec![2], vec![15], vec![0, 37]),
        (
            "BYHOUR=1,2,3;BYMINUTE=5,15;BYSECOND=0,37",
            vec![1, 2, 3],
            vec![5, 15],
            vec![0, 37],
        ),
    ] {
        let zones = admitted(parts, "20240510T021537");
        assert_eq!(
            zones.table(TZID).unwrap().rules().len(),
            hours.len() * minutes.len() * seconds.len()
        );
        let mut expected = BTreeSet::from([anchor]);
        for year in 2024..=2026 {
            for &hour in &hours {
                for &minute in &minutes {
                    for &second in &seconds {
                        let value = stamp(year, 5, 10, hour, minute, second);
                        if value >= anchor {
                            expected.insert(value);
                        }
                    }
                }
            }
        }
        assert_eq!(onsets(&zones, 2023..=2026), expected, "{parts}");
    }
}

fn selected_product(year: u16, positions: &[i16]) -> BTreeSet<CivilDateTime> {
    let mut candidates = Vec::new();
    for month in [5, 9] {
        for day in [1, 2] {
            for hour in [1, 3] {
                for minute in [0, 30] {
                    for second in [0, 20] {
                        candidates.push(stamp(year, month, day, hour, minute, second));
                    }
                }
            }
        }
    }
    positions
        .iter()
        .filter_map(|&position| {
            let index = if position > 0 {
                i32::from(position).checked_sub(1).unwrap()
            } else {
                i32::try_from(candidates.len())
                    .unwrap()
                    .checked_add(i32::from(position))
                    .unwrap()
            };
            usize::try_from(index)
                .ok()
                .and_then(|index| candidates.get(index))
                .copied()
        })
        .collect()
}

#[test]
fn positions_and_counts_rank_complete_periods_before_clipping_the_anchor() {
    let anchor = stamp(2024, 5, 1, 3, 0, 0);
    for positions in [
        vec![1, 2, 5, -1, -2, -5],
        vec![1, -32, 32, -1],
        vec![3],
        vec![-3],
    ] {
        let written = positions
            .iter()
            .map(i16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        for interval in [1_u32, 2, 400, u32::MAX] {
            for count in [1_u32, 2, 3, 5, 11, 20] {
                let parts = format!(
                    "BYMONTH=5,9;BYMONTHDAY=1,2;BYHOUR=1,3;BYMINUTE=0,30;BYSECOND=0,20;BYSETPOS={written};INTERVAL={interval};COUNT={count}"
                );
                let zones = admitted(&parts, "20240501T030000");
                let mut expected = BTreeSet::from([anchor]);
                let mut probes = BTreeSet::from([2023, 2024, 9999]);
                for period in 0_u32..=25 {
                    let Some(year) = interval
                        .checked_mul(period)
                        .and_then(|step| 2024_u32.checked_add(step))
                        .filter(|year| *year <= 9999)
                    else {
                        break;
                    };
                    let year = u16::try_from(year).unwrap();
                    probes.insert(year);
                    probes.extend(year.checked_add(1).filter(|year| *year <= 9999));
                    for value in selected_product(year, &positions) {
                        if value > anchor && expected.len() < usize::try_from(count).unwrap() {
                            expected.insert(value);
                        }
                    }
                }
                assert_eq!(onsets(&zones, probes), expected, "{parts}");
            }
        }
    }
}

#[test]
fn numbered_week_clocks_keep_the_source_period_and_partial_first_date() {
    let zones = admitted(
        "BYWEEKNO=1;BYDAY=MO,TU;WKST=SU;BYHOUR=1,3;INTERVAL=2;COUNT=6",
        "20191230T010000",
    );
    let expected = BTreeSet::from([
        stamp(2019, 12, 30, 1, 0, 0),
        stamp(2019, 12, 30, 3, 0, 0),
        stamp(2019, 12, 31, 1, 0, 0),
        stamp(2019, 12, 31, 3, 0, 0),
        stamp(2022, 1, 3, 1, 0, 0),
        stamp(2022, 1, 3, 3, 0, 0),
    ]);
    assert_eq!(onsets(&zones, 2018..=2025), expected);
}

#[test]
fn until_accepts_a_selected_clock_in_utc_and_leaves_explicit_dates_independent() {
    let (zones, diagnostics) = read(
        "BYHOUR=1,3;UNTIL=20250501T010000Z",
        "20240501T010000",
        "RDATE:20270501T040000\r\n",
        4096,
        0,
    );
    assert_eq!(diagnostics.as_slice(), []);
    assert_eq!(
        onsets(&zones, 2023..=2028),
        BTreeSet::from([
            stamp(2024, 5, 1, 1, 0, 0),
            stamp(2024, 5, 1, 3, 0, 0),
            stamp(2025, 5, 1, 1, 0, 0),
            stamp(2025, 5, 1, 3, 0, 0),
            stamp(2027, 5, 1, 4, 0, 0),
        ])
    );
    for parts in [
        "BYHOUR=1,3;UNTIL=20250501T020000Z",
        "BYHOUR=1,3;BYSETPOS=1;UNTIL=20250501T010000Z",
    ] {
        let (zones, diagnostics) = read(
            parts,
            "20240501T010000",
            "RDATE:20270501T040000\r\n",
            4096,
            0,
        );
        assert_eq!(
            diagnostics
                .iter()
                .copied()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            vec![DiagnosticCode::VtimezoneRuleUnsupported]
        );
        assert_eq!(
            onsets(&zones, 2023..=2028),
            BTreeSet::from([stamp(2024, 5, 1, 1, 0, 0), stamp(2027, 5, 1, 4, 0, 0)])
        );
    }
}

#[test]
fn malformed_clock_terms_refuse_the_whole_rule() {
    for (field, bad) in [("BYHOUR", "24"), ("BYMINUTE", "60"), ("BYSECOND", "61")] {
        let malformed = [bad, "+1", "-1", "001", "", "1,", ",1", "1,,2", " 1", "1 "]
            .map(|value| format!("BYMONTH=5;{field}={value}"));
        for parts in malformed
            .into_iter()
            .chain([format!("{field}=1;{field}=2")])
        {
            let (zones, diagnostics) = read(&parts, "20240501T010000", "", 4096, 0);
            let [anchor] = zones.table(TZID).unwrap().observances() else {
                panic!("{parts}")
            };
            assert!(anchor.rule().is_none());
            assert_eq!(anchor.start(), stamp(2024, 5, 1, 1, 0, 0));
            assert_eq!(
                diagnostics
                    .iter()
                    .copied()
                    .map(Diagnostic::code)
                    .collect::<Vec<_>>(),
                vec![DiagnosticCode::VtimezoneRuleUnsupported],
                "{parts}"
            );
        }
    }
}

#[test]
fn leap_second_fields_remain_distinct_before_instant_conversion() {
    let zones = admitted("BYSECOND=59,60;COUNT=3", "19970630T235959");
    assert_eq!(
        onsets(&zones, 1996..=2000),
        BTreeSet::from([
            stamp(1997, 6, 30, 23, 59, 59),
            stamp(1997, 6, 30, 23, 59, 60),
            stamp(1998, 6, 30, 23, 59, 59),
        ])
    );
}

#[test]
fn until_compares_leap_second_onsets_after_the_existing_instant_fallback() {
    let zones = admitted("BYSECOND=60;UNTIL=19980630T215960Z", "19970630T235960");
    assert_eq!(
        onsets(&zones, 1996..=2000),
        BTreeSet::from([
            stamp(1997, 6, 30, 23, 59, 60),
            stamp(1998, 6, 30, 23, 59, 60),
        ])
    );
}

#[test]
fn clock_only_positions_and_explicit_dates_share_one_count() {
    let (zones, diagnostics) = read(
        "BYHOUR=1,3;BYSETPOS=-1;COUNT=2",
        "20240501T030000",
        "RDATE:20230101T040000,20270101T040000\r\n",
        4096,
        0,
    );
    assert_eq!(diagnostics.as_slice(), []);
    assert_eq!(
        onsets(&zones, 2023..=2028),
        BTreeSet::from([
            stamp(2023, 1, 1, 4, 0, 0),
            stamp(2024, 5, 1, 3, 0, 0),
            stamp(2025, 5, 1, 3, 0, 0),
            stamp(2027, 1, 1, 4, 0, 0),
        ])
    );
    let zones = admitted("BYHOUR=23;COUNT=2", "19970630T235960");
    assert_eq!(
        onsets(&zones, 1996..=2000),
        BTreeSet::from([
            stamp(1997, 6, 30, 23, 59, 60),
            stamp(1998, 6, 30, 23, 59, 60),
        ])
    );
}

#[test]
fn leap_anniversary_clocks_skip_missing_dates_across_interval_cycles() {
    let anchor = stamp(2000, 2, 29, 1, 0, 0);
    for interval in [1_u32, 2, 3, 4, 100, 400, u32::MAX] {
        let zones = admitted(
            &format!("BYHOUR=1,3;INTERVAL={interval};COUNT=11"),
            "20000229T010000",
        );
        let mut expected = BTreeSet::from([anchor]);
        for year in 2000_u32..=9999 {
            if (year - 2000) % interval != 0 {
                continue;
            }
            let Some(date) = CivilDate::from_ymd(u16::try_from(year).unwrap(), 2, 29) else {
                continue;
            };
            for hour in [1, 3] {
                let value = CivilDateTime::new(date, CivilTime::from_hms(hour, 0, 0).unwrap());
                if value > anchor && expected.len() < 11 {
                    expected.insert(value);
                }
            }
        }
        assert_eq!(onsets(&zones, 1999..=9999), expected, "{interval}");
    }
}

#[test]
fn sparse_clock_positions_preserve_the_complete_admission_prefix() {
    for parts in [
        "BYMONTH=5,9;BYDAY=MO,TU,WE,TH,FR;BYHOUR=1,3,5;BYMINUTE=0,30;BYSETPOS=1,2,7,-1,-2,-7;COUNT=31",
        "BYWEEKNO=1,-1;BYDAY=MO,TU,WE,TH,FR,SA,SU;WKST=SU;BYHOUR=1,3;BYMINUTE=0,30;BYSETPOS=1,3,7,-1,-3,-7;COUNT=31",
    ] {
        let (complete, diagnostics) = read(parts, "20240501T010000", "", 4096, 0);
        assert_eq!(diagnostics.as_slice(), []);
        let full = complete.tables()[0].observances();
        for limit in 0..=10 {
            for spent in 0..=limit {
                let (zones, diagnostics) = read(parts, "20240501T010000", "", limit, spent);
                let kept = usize::try_from(limit - spent).unwrap().min(full.len());
                assert_eq!(zones.tables()[0].observances(), &full[..kept], "{parts}");
                assert_eq!(zones.tables()[0].is_truncated(), kept < full.len());
                assert_eq!(diagnostics.len(), usize::from(kept < full.len()));
                if kept < full.len() {
                    assert_eq!(
                        diagnostics[0].code(),
                        DiagnosticCode::VtimezoneObservancesTruncated
                    );
                }
            }
        }
    }
}

fn first_week_day(year: u16, start: Weekday) -> CivilDate {
    let fourth = CivilDate::from_ymd(year, 1, 4).unwrap();
    let back = i64::from(fourth.weekday().unwrap().index())
        .checked_sub(i64::from(start.index()))
        .unwrap()
        .rem_euclid(7);
    fourth
        .checked_add_days(back.checked_neg().unwrap())
        .unwrap()
}

#[test]
fn week_year_positions_match_independent_clock_enumeration_for_every_week_start() {
    for week_start in Weekday::ALL {
        let anchor_date = first_week_day(2020, week_start);
        let anchor = CivilDateTime::new(anchor_date, CivilTime::from_hms(3, 0, 0).unwrap());
        let start = format!(
            "{:04}{:02}{:02}T030000",
            anchor_date.year(),
            anchor_date.month(),
            anchor_date.day()
        );
        let token = std::str::from_utf8(week_start.as_bytes()).unwrap();
        let parts = format!(
            "BYWEEKNO=1,-1;BYDAY=MO,TU,WE,TH,FR,SA,SU;WKST={token};BYHOUR=1,3;BYMINUTE=0,30;BYSECOND=0,30;BYSETPOS=1,5,-1,-5;INTERVAL=2;COUNT=11"
        );
        let zones = admitted(&parts, &start);
        let mut expected = BTreeSet::from([anchor]);
        for year in [2020_u16, 2022, 2024, 2026, 2028, 2030] {
            let first = first_week_day(year, week_start);
            let last = first_week_day(year.checked_add(1).unwrap(), week_start)
                .checked_add_days(-7)
                .unwrap();
            let mut candidates = Vec::new();
            for week in [first, last] {
                for day in 0..7 {
                    let date = week.checked_add_days(day).unwrap();
                    for hour in [1, 3] {
                        for minute in [0, 30] {
                            for second in [0, 30] {
                                candidates.push(CivilDateTime::new(
                                    date,
                                    CivilTime::from_hms(hour, minute, second).unwrap(),
                                ));
                            }
                        }
                    }
                }
            }
            let selected: BTreeSet<_> = [
                0,
                4,
                candidates.len().checked_sub(1).unwrap(),
                candidates.len().checked_sub(5).unwrap(),
            ]
            .into_iter()
            .map(|index| candidates[index])
            .collect();
            for value in selected {
                if value > anchor && expected.len() < 11 {
                    expected.insert(value);
                }
            }
        }
        assert_eq!(onsets(&zones, 2018..=2032), expected, "{token}");
    }
}

#[test]
fn admission_matches_the_complete_order_with_spent_and_zero_budgets() {
    let parts = "BYMONTH=5,9;BYMONTHDAY=1,2;BYHOUR=1,2,3;BYMINUTE=0,30;BYSECOND=0,20;COUNT=37";
    let extra = "RDATE:20230101T020000,20260101T020000\r\n";
    let (complete, diagnostics) = read(parts, "20240501T020000", extra, 4096, 0);
    assert_eq!(diagnostics.as_slice(), []);
    let full = complete.tables()[0].observances();
    assert!(full.len() > 10);
    for limit in 0..=10 {
        for spent in 0..=limit {
            let (zones, diagnostics) = read(parts, "20240501T020000", extra, limit, spent);
            let count = usize::try_from(limit - spent).unwrap();
            assert_eq!(zones.tables()[0].observances(), &full[..count]);
            assert!(zones.tables()[0].is_truncated());
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].code(),
                DiagnosticCode::VtimezoneObservancesTruncated
            );
        }
    }
}

#[test]
fn minimum_and_final_civil_dates_keep_later_clocks_on_the_anchor_date() {
    for (start, year, month, day) in [
        ("00000101T000000", 0, 1, 1),
        ("99991231T000000", 9999, 12, 31),
    ] {
        let zones = admitted("BYHOUR=0,1;COUNT=2", start);
        assert_eq!(
            onsets(&zones, [year]),
            BTreeSet::from([
                stamp(year, month, day, 0, 0, 0),
                stamp(year, month, day, 1, 0, 0)
            ])
        );
    }
}

fn list(end: u8) -> String {
    (0..=end)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn large_counts_use_wide_cycle_summaries_without_expanding_products() {
    let parts = format!(
        "BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR={};BYMINUTE={};BYSECOND={};COUNT={}",
        list(23),
        list(59),
        list(60),
        u32::MAX
    );
    let (zones, diagnostics) = read(&parts, "20000101T000000", "", 1, 0);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneObservancesTruncated
    );
    let table = &zones.tables()[0];
    assert!(table.is_truncated());
    let last_index = u64::from(u32::MAX) - 1;
    let last_date = CivilDate::from_ymd(2000, 1, 1)
        .unwrap()
        .checked_add_days(i64::try_from(last_index / (24 * 60 * 61)).unwrap())
        .unwrap();
    assert_eq!(
        table.observances()[0].rule().unwrap().through(),
        Some(last_date)
    );
}
