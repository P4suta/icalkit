// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Final onsets follow [RFC 5545 section 3.6.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.5).

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, Severity, UtcOffset,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

mod support;

use support::observance_dates::Pattern;

const TZID: &str = "Test/UntilBounds";

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
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

fn offset_text(seconds: i32) -> String {
    let sign = if seconds.is_negative() { '-' } else { '+' };
    let magnitude = seconds.unsigned_abs();
    format!(
        "{sign}{:02}{:02}{:02}",
        magnitude / 3600,
        (magnitude / 60) % 60,
        magnitude % 60
    )
}

fn read(
    parts: &str,
    start: &str,
    offsets: (&str, &str),
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let (from, to) = offsets;
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit final onsets//EN\r\nBEGIN:VTIMEZONE\r\nTZID:{TZID}\r\nBEGIN:DAYLIGHT\r\nDTSTART:{start}\r\nTZOFFSETFROM:{from}\r\nTZOFFSETTO:{to}\r\nRRULE:FREQ=YEARLY;{parts}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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

fn assert_refused(zones: &VtimezoneSet, diagnostics: &[Diagnostic]) {
    assert_eq!(zones.table(TZID).unwrap().rules().len(), 0);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect::<Vec<_>>(),
        [DiagnosticCode::VtimezoneRuleUnsupported]
    );
    assert_eq!(diagnostics[0].severity(), Severity::Note);
}

#[test]
fn every_selector_requires_the_written_utc_end_to_be_an_onset() {
    for pattern in Pattern::ALL {
        let anchor = *pattern.dates(2000).first().unwrap();
        let last = *pattern.dates(2024).first().unwrap();
        for at in [
            CivilTime::MIDNIGHT,
            CivilTime::from_hms(2, 30, 15).unwrap(),
            CivilTime::from_hms(23, 59, 60).unwrap(),
        ] {
            let start = CivilDateTime::new(anchor, at);
            for seconds in [-86399, -18000, 0, 7201, 86399] {
                let offset = UtcOffset::from_seconds(seconds).unwrap();
                let spelling = offset_text(seconds);
                let final_onset = CivilDateTime::new(last, at).at_offset(offset).unwrap();
                for delta in [-1, 0, 1] {
                    let end = final_onset.checked_add_seconds(delta).unwrap();
                    let until = stamp(CivilDateTime::from_instant(end, UtcOffset::UTC).unwrap());
                    let parts = format!("UNTIL={until}Z;{}", pattern.parts());
                    let (zones, _, diagnostics) = read(
                        parts.trim_end_matches(';'),
                        &stamp(start),
                        (&spelling, &spelling),
                        "",
                        Limits::DEFAULT,
                    );
                    if delta != 0 {
                        assert_refused(&zones, &diagnostics);
                        assert_eq!(onsets(&zones, 2000), BTreeSet::from([anchor]));
                        assert_eq!(onsets(&zones, 2024), BTreeSet::new());
                        continue;
                    }
                    assert!(
                        diagnostics.is_empty(),
                        "{pattern:?}, {at:?}, {seconds}: {diagnostics:?}"
                    );
                    for year in [1999, 2000, 2023, 2024, 2025] {
                        let mut expected: BTreeSet<_> = pattern
                            .dates(year)
                            .into_iter()
                            .filter(|value| {
                                *value >= anchor
                                    && CivilDateTime::new(*value, at).at_offset(offset).unwrap()
                                        <= end
                            })
                            .collect();
                        if year == anchor.year() {
                            expected.insert(anchor);
                        }
                        assert_eq!(
                            onsets(&zones, year),
                            expected,
                            "{pattern:?}, {at:?}, {seconds}, {year}"
                        );
                    }
                    for observance in zones.table(TZID).unwrap().rules() {
                        assert_eq!(observance.start(), start);
                        assert_eq!(observance.rule().unwrap().at(), at);
                        assert_eq!(observance.rule().unwrap().through(), Some(last));
                    }
                }
            }
        }
    }
}

#[test]
fn unselected_dates_and_interval_phases_refuse_the_whole_rule() {
    for (parts, start) in [
        ("BYMONTHDAY=1;UNTIL=20250102T020000Z", "20240101T020000"),
        (
            "BYMONTH=3;BYDAY=2SU;UNTIL=20250310T020000Z",
            "20240310T020000",
        ),
        (
            "BYYEARDAY=1,100,-1;BYSETPOS=1,-1;UNTIL=20250410T020000Z",
            "20240101T020000",
        ),
        (
            "BYMONTH=3,9;BYDAY=SU;BYSETPOS=1,-1;UNTIL=20250309T020000Z",
            "20240303T020000",
        ),
        (
            "INTERVAL=2;BYMONTH=3;BYDAY=2SU;UNTIL=20250309T020000Z",
            "20240310T020000",
        ),
        (
            "INTERVAL=400;BYMONTH=2;BYDAY=5SU;UNTIL=17040229T020000Z",
            "16040229T020000",
        ),
        (
            "INTERVAL=4294967295;BYMONTH=3;BYDAY=2SU;UNTIL=20250309T020000Z",
            "20240310T020000",
        ),
        (
            "BYMONTH=3;BYDAY=2SU;UNTIL=20230312T020000Z",
            "20240310T020000",
        ),
        (
            "BYMONTH=3;BYDAY=2SU;UNTIL=20250820T020000Z",
            "20240310T020000",
        ),
    ] {
        let (zones, meter, diagnostics) = read(
            parts,
            start,
            ("+0000", "+0100"),
            "RDATE:20250820T020000\r\n",
            Limits::DEFAULT,
        );
        assert_refused(&zones, &diagnostics);
        assert_eq!(meter.vtimezone_observances(), 2, "{parts}");
        assert!(onsets(&zones, 2025).contains(&date(2025, 8, 20)));
        let written: Vec<_> = zones
            .table(TZID)
            .unwrap()
            .observances()
            .iter()
            .map(|entry| stamp(entry.start()))
            .collect();
        assert_eq!(written, [start, "20250820T020000"]);
    }
}

#[test]
fn final_onsets_follow_the_shared_leap_second_projection() {
    for start in ["20151231T235959", "20151231T235960"] {
        for until in ["20161231T235959Z", "20161231T235960Z"] {
            let (zones, _, diagnostics) = read(
                &format!("BYMONTH=12;BYMONTHDAY=31;UNTIL={until}"),
                start,
                ("+0000", "+0100"),
                "",
                Limits::DEFAULT,
            );
            assert!(diagnostics.is_empty(), "{start}, {until}: {diagnostics:?}");
            assert_eq!(onsets(&zones, 2016), BTreeSet::from([date(2016, 12, 31)]));
            assert_eq!(onsets(&zones, 2017), BTreeSet::new());
        }
        let (zones, _, diagnostics) = read(
            "BYMONTH=12;BYMONTHDAY=31;UNTIL=20161231T235958Z",
            start,
            ("+0000", "+0100"),
            "",
            Limits::DEFAULT,
        );
        assert_refused(&zones, &diagnostics);
    }
}

#[test]
fn inclusive_ends_keep_clocks_offsets_and_independent_explicit_dates() {
    let (zones, meter, diagnostics) = read(
        "INTERVAL=2;BYMONTH=1,4;BYMONTHDAY=1;UNTIL=20260331T235930Z",
        "20240101T020000",
        ("+020030", "+030030"),
        "RDATE:20250820T020000,20280820T020000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(meter.vtimezone_observances(), 4);
    let table = zones.table(TZID).unwrap();
    assert_eq!(table.rules().len(), 2);
    for observance in table.rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2024, 1, 1), CivilTime::from_hms(2, 0, 0).unwrap())
        );
        assert_eq!(observance.offset_from().seconds(), 7230);
        assert_eq!(observance.offset_to().seconds(), 10830);
        assert!(observance.daylight());
        assert_eq!(observance.rule().unwrap().through(), Some(date(2026, 4, 1)));
    }
    assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 8, 20)]));
    assert_eq!(
        onsets(&zones, 2026),
        BTreeSet::from([date(2026, 1, 1), date(2026, 4, 1)])
    );
    assert_eq!(onsets(&zones, 2027), BTreeSet::new());
    assert_eq!(onsets(&zones, 2028), BTreeSet::from([date(2028, 8, 20)]));
}

#[test]
fn civil_year_edges_accept_only_representable_final_onsets() {
    for (start, offset, until, last) in [
        (
            "00000101T235959",
            "+235959",
            "00000101T000000Z",
            date(0, 1, 1),
        ),
        (
            "00000101T000000",
            "-235959",
            "00000101T235959Z",
            date(0, 1, 1),
        ),
        (
            "99981231T000000",
            "-235959",
            "99991231T235959Z",
            date(9999, 12, 31),
        ),
        (
            "99981231T235959",
            "+235959",
            "99991231T000000Z",
            date(9999, 12, 31),
        ),
    ] {
        let (zones, _, diagnostics) = read(
            &format!("UNTIL={until}"),
            start,
            (offset, offset),
            "",
            Limits::DEFAULT,
        );
        assert!(
            diagnostics.is_empty(),
            "{start}, {offset}, {until}: {diagnostics:?}"
        );
        assert_eq!(zones.table(TZID).unwrap().coverage_end(), Some(last));
        assert!(onsets(&zones, last.year()).contains(&last));
    }
    for (start, offset, until) in [
        ("00000101T000000", "-235959", "00000101T000000Z"),
        ("99980101T000000", "+235959", "99991231T235959Z"),
    ] {
        let (zones, _, diagnostics) = read(
            &format!("UNTIL={until}"),
            start,
            (offset, offset),
            "",
            Limits::DEFAULT,
        );
        assert_refused(&zones, &diagnostics);
    }
}

#[test]
fn refused_ends_preserve_canonical_storage_bounds() {
    for cap in [1_u32, 2, 3] {
        let (zones, meter, diagnostics) = read(
            "BYMONTH=3;BYDAY=2SU;UNTIL=20250309T065959Z",
            "20240310T020000",
            ("-0500", "-0400"),
            "RDATE:20250820T020000\r\n",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table(TZID).unwrap();
        assert_eq!(table.rules(), []);
        assert_eq!(
            table.observances().len(),
            usize::try_from(cap.min(2)).unwrap()
        );
        assert_eq!(meter.vtimezone_observances(), cap.min(2));
        assert_eq!(table.is_truncated(), cap < 2);
        assert_eq!(onsets(&zones, 2024), BTreeSet::from([date(2024, 3, 10)]));
        let codes: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect();
        assert!(codes.contains(&DiagnosticCode::VtimezoneRuleUnsupported));
        assert_eq!(
            codes.contains(&DiagnosticCode::VtimezoneObservancesTruncated),
            cap < 2
        );
        if cap >= 2 {
            assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 8, 20)]));
        }
    }
}

#[test]
fn every_gregorian_layout_keeps_matching_final_dates() {
    for pattern in Pattern::ALL {
        let anchor = *pattern.dates(2000).first().unwrap();
        let at = CivilTime::from_hms(2, 0, 0).unwrap();
        for year in 2000..2400 {
            let selected = pattern.dates(year);
            let last = selected
                .first()
                .copied()
                .unwrap_or_else(|| date(year, 1, 1));
            let parts = format!(
                "UNTIL={}Z;{}",
                stamp(CivilDateTime::new(last, at)),
                pattern.parts()
            );
            let (zones, _, diagnostics) = read(
                parts.trim_end_matches(';'),
                &stamp(CivilDateTime::new(anchor, at)),
                ("+0000", "+0100"),
                "",
                Limits::DEFAULT,
            );
            if selected.is_empty() {
                assert_refused(&zones, &diagnostics);
            } else {
                assert!(
                    diagnostics.is_empty(),
                    "{pattern:?}, {year}: {diagnostics:?}"
                );
                assert_eq!(
                    onsets(&zones, year),
                    BTreeSet::from([last]),
                    "{pattern:?}, {year}"
                );
                assert_eq!(
                    onsets(&zones, year.checked_add(1).unwrap()),
                    BTreeSet::new()
                );
            }
        }
    }
}
