// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Counted transition sets follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{
    AnswerBasis, VtimezoneSet, ZoneSource, read_calendar_zones,
};

mod support;

use support::observance_dates::Pattern;

const TZID: &str = "Test/RuleCounts";

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn clock(date: CivilDate, hour: u8, minute: u8) -> CivilDateTime {
    CivilDateTime::new(date, CivilTime::from_hms(hour, minute, 0).unwrap())
}

fn stamp(value: CivilDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        value.date().year(),
        value.date().month(),
        value.date().day(),
        value.time().hour(),
        value.time().minute(),
        value.time().second()
    )
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit rule counts//EN\r\nBEGIN:VTIMEZONE\r\nTZID:{TZID}\r\nBEGIN:DAYLIGHT\r\nDTSTART:{start}\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0300\r\nRRULE:FREQ=YEARLY;{parts}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document =
        Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    assert_eq!(document.to_bytes(), text.as_bytes());
    (zones, meter, diagnostics)
}

fn admitted(parts: &str, start: &str) -> VtimezoneSet {
    let (zones, _, diagnostics) = read(parts, start, "", Limits::DEFAULT);
    assert!(diagnostics.is_empty(), "{parts}, {start}: {diagnostics:?}");
    zones
}

fn onsets(zones: &VtimezoneSet, year: u16) -> BTreeSet<CivilDate> {
    zones
        .table(TZID)
        .unwrap()
        .observances()
        .iter()
        .flat_map(|observance| {
            [
                (observance.start().date().year() == year).then_some(observance.start().date()),
                observance.transition_in(year).map(CivilDateTime::date),
            ]
        })
        .flatten()
        .collect()
}

fn forward_dates(
    candidates: &[BTreeSet<CivilDate>],
    anchor: CivilDate,
    interval: u32,
) -> Vec<CivilDate> {
    let mut result = vec![anchor];
    let mut year = u32::from(anchor.year());
    while year <= 9999 {
        let index = usize::try_from(year.checked_sub(u32::from(anchor.year())).unwrap()).unwrap();
        result.extend(
            candidates[index]
                .iter()
                .copied()
                .filter(|value| *value > anchor),
        );
        let Some(next) = year.checked_add(interval) else {
            break;
        };
        year = next;
    }
    result
}

#[test]
fn counted_patterns_match_independent_forward_enumeration() {
    for pattern in Pattern::ALL {
        let anchor = *pattern.dates(2000).first().unwrap();
        let candidates: Vec<_> = (2000..=9999).map(|year| pattern.dates(year)).collect();
        for interval in [1_u32, 2, 3, 4, 100, 400, u32::MAX] {
            let forward = forward_dates(&candidates, anchor, interval);
            for count in [1_u32, 2, 3, 4, 5, 31, 366, 367, 500, u32::MAX] {
                let parts = format!("COUNT={count};INTERVAL={interval};{}", pattern.parts());
                let parts = parts.trim_end_matches(';');
                let zones = admitted(parts, &stamp(clock(anchor, 2, 0)));
                let expected: BTreeSet<_> = forward
                    .iter()
                    .take(usize::try_from(count).unwrap())
                    .copied()
                    .collect();
                let last = expected.last().unwrap().year();
                let probes: BTreeSet<_> = (2000..2401)
                    .chain([0, 1999, 9999, last])
                    .chain(last.checked_sub(1))
                    .chain(last.checked_add(1).filter(|year| *year <= 9999))
                    .collect();
                for year in probes {
                    let selected: BTreeSet<_> = expected
                        .iter()
                        .filter(|value| value.year() == year)
                        .copied()
                        .collect();
                    assert_eq!(
                        onsets(&zones, year),
                        selected,
                        "{pattern:?}, count={count}, interval={interval}, year={year}"
                    );
                }
            }
        }
    }
}

#[test]
fn count_is_global_after_yearly_positions_and_date_deduplication() {
    for parts in [
        "COUNT=3;BYMONTH=3,9;BYDAY=SU;BYSETPOS=1,-1",
        "BYSETPOS=-1,1,1;BYDAY=SU,SU;COUNT=3;BYMONTH=9,3,3",
    ] {
        let zones = admitted(parts, "20240303T023000");
        assert_eq!(
            onsets(&zones, 2024),
            BTreeSet::from([date(2024, 3, 3), date(2024, 9, 29)])
        );
        assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 3, 2)]));
        assert_eq!(onsets(&zones, 2026), BTreeSet::new());
    }
    for parts in [
        "COUNT=3;BYMONTH=2;BYMONTHDAY=1,-29,29,-1",
        "COUNT=3;BYMONTH=2;BYYEARDAY=32,-335,60,-307",
    ] {
        let zones = admitted(parts, "20240201T023000");
        assert_eq!(
            onsets(&zones, 2024),
            BTreeSet::from([date(2024, 2, 1), date(2024, 2, 29)])
        );
        assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 2, 1)]));
    }
}

#[test]
fn singleton_counts_keep_only_the_source_anchor() {
    for parts in [
        "COUNT=1",
        "COUNT=1;BYDAY=MO,TU,WE,TH,FR,SA,SU",
        "COUNT=1;INTERVAL=4294967295;BYMONTHDAY=1",
        "COUNT=1;BYMONTH=3;BYDAY=2SU",
    ] {
        let (zones, meter, diagnostics) = read(
            parts,
            "20240101T023000",
            "",
            Limits::DEFAULT.with_max_vtimezone_observances(1),
        );
        assert!(diagnostics.is_empty(), "{parts}: {diagnostics:?}");
        let table = zones.table(TZID).unwrap();
        assert_eq!(meter.vtimezone_observances(), 1);
        assert_eq!(table.rules(), []);
        assert_eq!(table.coverage_end(), Some(date(2024, 1, 1)));
        assert_eq!(onsets(&zones, 2024), BTreeSet::from([date(2024, 1, 1)]));
        assert_eq!(onsets(&zones, 2025), BTreeSet::new());
    }
}

#[test]
fn only_patterns_with_counted_future_dates_consume_the_bound() {
    for (count, patterns) in [(2_u32, 1_u32), (31, 30), (50, 49)] {
        for cap in [1, patterns, patterns.checked_add(1).unwrap()] {
            let (zones, meter, diagnostics) = read(
                &format!("COUNT={count};BYDAY=MO,TU,WE,TH,FR,SA,SU"),
                "20240101T023000",
                "",
                Limits::DEFAULT.with_max_vtimezone_observances(cap),
            );
            let table = zones.table(TZID).unwrap();
            assert_eq!(meter.vtimezone_observances(), cap.min(patterns));
            assert_eq!(
                table.rules().len(),
                usize::try_from(cap.min(patterns)).unwrap()
            );
            assert_eq!(table.is_truncated(), cap < patterns);
            assert_eq!(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code())
                    .collect::<Vec<_>>(),
                if cap < patterns {
                    vec![DiagnosticCode::VtimezoneObservancesTruncated]
                } else {
                    vec![]
                }
            );
            if cap >= patterns {
                assert_eq!(onsets(&zones, 2024).len(), usize::try_from(count).unwrap());
                assert_eq!(onsets(&zones, 2025), BTreeSet::new());
            }
        }
    }
}

#[test]
fn explicit_dates_stay_outside_the_count_and_interval() {
    let (zones, meter, diagnostics) = read(
        "COUNT=2;INTERVAL=2;BYMONTH=3;BYDAY=2SU",
        "20240310T023000",
        "RDATE:20230312T023000,20250820T023000,20280312T023000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let table = zones.table(TZID).unwrap();
    assert_eq!(meter.vtimezone_observances(), 4);
    assert_eq!(table.rules().len(), 1);
    for (year, values) in [
        (2023, vec![date(2023, 3, 12)]),
        (2024, vec![date(2024, 3, 10)]),
        (2025, vec![date(2025, 8, 20)]),
        (2026, vec![date(2026, 3, 8)]),
        (2027, vec![]),
        (2028, vec![date(2028, 3, 12)]),
        (2029, vec![]),
    ] {
        assert_eq!(onsets(&zones, year), values.into_iter().collect());
    }
    let held = table.rules()[0];
    assert_eq!(held.start(), clock(date(2024, 3, 10), 2, 30));
    assert_eq!(held.offset_from().seconds(), 7200);
    assert_eq!(held.offset_to().seconds(), 10800);
    assert!(held.daylight());
    assert_eq!(held.rule().unwrap().at(), held.start().time());
    assert_eq!(held.rule().unwrap().through(), Some(date(2026, 3, 8)));
}

#[test]
fn sparse_leap_dates_count_occurrences_without_restarting_intervals() {
    for (start, parts, expected) in [
        (
            "20000229T023000",
            "COUNT=2;INTERVAL=100;BYMONTH=2;BYMONTHDAY=29",
            date(2400, 2, 29),
        ),
        (
            "16040229T023000",
            "COUNT=2;INTERVAL=100;BYMONTH=2;BYDAY=5SU",
            date(2004, 2, 29),
        ),
        ("20000229T023000", "COUNT=101;INTERVAL=4", date(2412, 2, 29)),
    ] {
        let zones = admitted(parts, start);
        assert!(
            onsets(&zones, expected.year()).contains(&expected),
            "{parts}"
        );
        assert_eq!(
            zones.table(TZID).unwrap().coverage_end(),
            Some(expected),
            "{parts}"
        );
        assert_eq!(
            onsets(&zones, expected.year().checked_add(4).unwrap()),
            BTreeSet::new()
        );
    }
}

#[test]
fn every_interval_residue_keeps_the_counted_sparse_endpoint() {
    let anchor = date(1604, 2, 29);
    let rare: Vec<_> = (1604..=9999)
        .filter_map(|year| CivilDate::from_ymd(year, 2, 29))
        .filter(|value| value.weekday() == Some(Weekday::Sunday))
        .collect();
    assert_eq!(rare.first(), Some(&anchor));
    for interval in (1_u32..=400).chain([401, 799, 800, 801, 9999, u32::MAX]) {
        let mut years = BTreeSet::new();
        let mut year = u32::from(anchor.year());
        while year <= 9999 {
            years.insert(u16::try_from(year).unwrap());
            let Some(next) = year.checked_add(interval) else {
                break;
            };
            year = next;
        }
        let selected: Vec<_> = rare
            .iter()
            .copied()
            .filter(|value| years.contains(&value.year()))
            .collect();
        for count in [2_u32, 3, 31, u32::MAX] {
            let parts = format!("COUNT={count};INTERVAL={interval};BYMONTH=2;BYDAY=5SU");
            let zones = admitted(&parts, "16040229T023000");
            let expected: BTreeSet<_> = selected
                .iter()
                .copied()
                .take(usize::try_from(count).unwrap())
                .collect();
            for candidate in &selected {
                let dates = if expected.contains(candidate) {
                    BTreeSet::from([*candidate])
                } else {
                    BTreeSet::new()
                };
                assert_eq!(
                    onsets(&zones, candidate.year()),
                    dates,
                    "{parts}, {candidate:?}"
                );
            }
            let end = (expected.len() == usize::try_from(count).unwrap())
                .then(|| *expected.last().unwrap());
            assert_eq!(zones.table(TZID).unwrap().coverage_end(), end, "{parts}");
        }
    }
}

#[test]
fn finite_cutoffs_keep_the_anchor_years_remaining_candidates() {
    let zones = admitted("COUNT=3;BYMONTHDAY=1", "20241001T023000");
    assert_eq!(
        onsets(&zones, 2024),
        BTreeSet::from([date(2024, 10, 1), date(2024, 11, 1), date(2024, 12, 1)])
    );
    assert_eq!(onsets(&zones, 2025), BTreeSet::new());
    assert_eq!(
        zones.table(TZID).unwrap().coverage_end(),
        Some(date(2024, 12, 1))
    );
}

#[test]
fn counted_ends_distinguish_civil_boundaries_from_unrepresentable_ends() {
    for (start, parts, last, future) in [
        ("00000229T023000", "COUNT=2", date(4, 2, 29), false),
        (
            "99981201T023000",
            "COUNT=13;BYMONTHDAY=1",
            date(9999, 12, 1),
            false,
        ),
        (
            "99981201T023000",
            "COUNT=14;BYMONTHDAY=1",
            date(9999, 12, 1),
            true,
        ),
        ("99991231T023000", "COUNT=2", date(9999, 12, 31), true),
        (
            "20240310T023000",
            "COUNT=4294967295;INTERVAL=4294967295;BYMONTH=3;BYDAY=2SU",
            date(2024, 3, 10),
            true,
        ),
        (
            "20240310T023000",
            "COUNT=4294967295;BYMONTH=3;BYDAY=2SU",
            date(9999, 3, 14),
            true,
        ),
    ] {
        let zones = admitted(parts, start);
        let table = zones.table(TZID).unwrap();
        assert_eq!(table.coverage_end(), (!future).then_some(last), "{parts}");
        assert!(onsets(&zones, last.year()).contains(&last), "{parts}");
        let query = clock(date(9999, 12, 31), 12, 0)
            .at_offset(UtcOffset::UTC)
            .unwrap();
        let answer = table.offset_at(TZID, query).unwrap();
        assert_eq!(
            answer.basis,
            if future {
                AnswerBasis::Computed
            } else {
                AnswerBasis::BeyondKnownTransitions(last)
            },
            "{parts}"
        );
    }
}

#[test]
fn zero_padding_and_ascii_case_preserve_positive_counts() {
    for padding in 0..=16 {
        let parts = format!(
            "cOuNt={}3;ByMoNtH=3,9;ByDaY=Su;bYsEtPoS=1,-1",
            "0".repeat(padding)
        );
        let zones = admitted(&parts, "20240303T023000");
        assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 3, 2)]));
        assert_eq!(onsets(&zones, 2026), BTreeSet::new());
    }
}

#[test]
fn malformed_counts_and_incompatible_bounds_refuse_the_whole_rule() {
    for parts in [
        "COUNT=0",
        "COUNT=000",
        "COUNT=",
        "COUNT=+2",
        "COUNT=-2",
        "COUNT=2x",
        "COUNT=4294967296",
        "COUNT=2,3",
        "COUNT=2;count=3",
        "COUNT=2;UNTIL=20250310T003000Z",
        "UNTIL=20250310T003000Z;COUNT=2",
        "COUNT=2;BYSETPOS=1",
        "COUNT=2;INTERVAL=3;WKST=SU;BYSETPOS=1",
        "COUNT=2;BYMONTH=3;BYHOUR=2",
        "COUNT=2;BYWEEKNO=1",
    ] {
        let (zones, meter, diagnostics) = read(
            parts,
            "20240310T023000",
            "RDATE:20260820T023000\r\n",
            Limits::DEFAULT,
        );
        assert_eq!(zones.table(TZID).unwrap().rules(), [], "{parts}");
        assert_eq!(meter.vtimezone_observances(), 2, "{parts}");
        assert_eq!(onsets(&zones, 2024), BTreeSet::from([date(2024, 3, 10)]));
        assert_eq!(onsets(&zones, 2026), BTreeSet::from([date(2026, 8, 20)]));
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code())
                .collect::<Vec<_>>(),
            vec![DiagnosticCode::VtimezoneRuleUnsupported],
            "{parts}"
        );
    }
}
