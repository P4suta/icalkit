// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Week periods follow [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).

#![cfg(test)]

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    Limits, Meter, UtcOffset, Weekday,
};
use icalkit_conformance::internal::tz::{VtimezoneSet, read_calendar_zones};

const TZID: &str = "Test/WeekNumbers";

fn date(year: u16, month: u8, day: u8) -> CivilDate {
    CivilDate::from_ymd(year, month, day).unwrap()
}

fn stamp(value: CivilDate) -> String {
    format!(
        "{:04}{:02}{:02}T020000",
        value.year(),
        value.month(),
        value.day()
    )
}

fn weekday(value: Weekday) -> &'static str {
    match value {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

fn numbers<T: ToString>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Clone, Copy, Debug)]
struct Spec {
    weeks: &'static [i8],
    start: Weekday,
    weekdays: Option<&'static [Weekday]>,
    months: Option<&'static [u8]>,
    month_days: Option<&'static [i8]>,
    year_days: Option<&'static [i16]>,
    positions: Option<&'static [i16]>,
}

impl Spec {
    const fn new(weeks: &'static [i8]) -> Self {
        Self {
            weeks,
            start: Weekday::Monday,
            weekdays: None,
            months: None,
            month_days: None,
            year_days: None,
            positions: None,
        }
    }

    fn cases() -> [Self; 6] {
        [
            Self::new(&[1, -1]),
            Self {
                weekdays: Some(&[Weekday::Monday, Weekday::Friday]),
                ..Self::new(&[20, 53, -1])
            },
            Self {
                weekdays: Some(&Weekday::ALL),
                months: Some(&[1, 12]),
                positions: Some(&[1, -1]),
                ..Self::new(&[1, -1])
            },
            Self {
                weekdays: Some(&[Weekday::Monday, Weekday::Wednesday, Weekday::Sunday]),
                months: Some(&[2, 3, 12]),
                month_days: Some(&[1, 29, -1]),
                ..Self::new(&[8, 9, 10, 52, 53])
            },
            Self {
                year_days: Some(&[1, 60, 366, -1]),
                month_days: Some(&[1, 29, -1]),
                ..Self::new(&[1, 9, -1])
            },
            Self {
                weekdays: Some(&Weekday::ALL),
                positions: Some(&[2, -2]),
                ..Self::new(&[1, 20, -1])
            },
        ]
    }

    fn parts(self) -> String {
        let mut parts = vec![
            format!("BYWEEKNO={}", numbers(self.weeks)),
            format!("WKST={}", weekday(self.start)),
        ];
        if let Some(values) = self.weekdays {
            parts.push(format!(
                "BYDAY={}",
                values
                    .iter()
                    .map(|value| weekday(*value))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if let Some(values) = self.months {
            parts.push(format!("BYMONTH={}", numbers(values)));
        }
        if let Some(values) = self.month_days {
            parts.push(format!("BYMONTHDAY={}", numbers(values)));
        }
        if let Some(values) = self.year_days {
            parts.push(format!("BYYEARDAY={}", numbers(values)));
        }
        if let Some(values) = self.positions {
            parts.push(format!("BYSETPOS={}", numbers(values)));
        }
        parts.join(";")
    }

    fn dates(self, year: u16) -> Vec<CivilDate> {
        let first = first_week(year, self.start);
        let end = first_week(year.checked_add(1).unwrap(), self.start);
        let extent = distance(first, end);
        let weeks = extent / 7;
        let mut selected = Vec::new();
        for offset in 0..extent {
            let value = first.checked_add_days(offset).unwrap();
            let number = offset.checked_div(7).unwrap().checked_add(1).unwrap();
            if !self
                .weeks
                .iter()
                .any(|named| names(i16::from(*named), number, weeks))
            {
                continue;
            }
            if self
                .months
                .is_some_and(|months| !months.contains(&value.month()))
            {
                continue;
            }
            if self.month_days.is_some_and(|days| {
                !days.iter().any(|named| {
                    names(
                        i16::from(*named),
                        i64::from(value.day()),
                        i64::from(CivilDate::days_in_month(value.year(), value.month()).unwrap()),
                    )
                })
            }) {
                continue;
            }
            let year_first = date(value.year(), 1, 1);
            let year_end = date(value.year(), 12, 31);
            let ordinal = distance(year_first, value).checked_add(1).unwrap();
            let year_length = distance(year_first, year_end).checked_add(1).unwrap();
            if self
                .year_days
                .is_some_and(|days| !days.iter().any(|named| names(*named, ordinal, year_length)))
            {
                continue;
            }
            let day = value.weekday().unwrap();
            if let Some(weekdays) = self.weekdays {
                if !weekdays.contains(&day) {
                    continue;
                }
            } else if self.month_days.is_none()
                && self.year_days.is_none()
                && day != Weekday::Monday
            {
                continue;
            }
            selected.push(value);
        }
        if let Some(positions) = self.positions {
            let extent = i64::try_from(selected.len()).unwrap();
            selected = selected
                .into_iter()
                .enumerate()
                .filter(|(index, _)| {
                    positions.iter().any(|named| {
                        names(
                            *named,
                            i64::try_from(index.checked_add(1).unwrap()).unwrap(),
                            extent,
                        )
                    })
                })
                .map(|(_, value)| value)
                .collect();
        }
        selected
    }
}

fn names(value: i16, position: i64, extent: i64) -> bool {
    let named = if value > 0 {
        i64::from(value)
    } else {
        extent
            .checked_add(i64::from(value))
            .unwrap()
            .checked_add(1)
            .unwrap()
    };
    named > 0 && named <= extent && named == position
}

fn distance(first: CivilDate, last: CivilDate) -> i64 {
    last.days_from_epoch()
        .unwrap()
        .checked_sub(first.days_from_epoch().unwrap())
        .unwrap()
}

fn first_week(year: u16, start: Weekday) -> CivilDate {
    let january = date(year, 1, 1);
    let first = (-6..=0)
        .map(|offset| january.checked_add_days(offset).unwrap())
        .find(|value| value.weekday() == Some(start))
        .unwrap();
    let inside = (0..7)
        .map(|offset| first.checked_add_days(offset).unwrap())
        .filter(|value| value.year() == year)
        .count();
    if inside >= 4 {
        first
    } else {
        first.checked_add_days(7).unwrap()
    }
}

fn read(
    parts: &str,
    start: &str,
    extra: &str,
    limits: Limits,
) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit week numbers//EN\r\nBEGIN:VTIMEZONE\r\nTZID:{TZID}\r\nBEGIN:DAYLIGHT\r\nDTSTART:{start}\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;{parts}\r\n{extra}END:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
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

fn assert_refused(parts: &str) {
    let (zones, meter, diagnostics) = read(
        parts,
        "20200106T020000",
        "RDATE:20250820T020000\r\n",
        Limits::DEFAULT,
    );
    assert_eq!(zones.table(TZID).unwrap().rules().len(), 0, "{parts}");
    assert_eq!(meter.vtimezone_observances(), 2, "{parts}");
    assert_eq!(onsets(&zones, 2020), BTreeSet::from([date(2020, 1, 6)]));
    assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 8, 20)]));
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect::<Vec<_>>(),
        [DiagnosticCode::VtimezoneRuleUnsupported],
        "{parts}"
    );
}

#[test]
fn week_starts_and_signed_selectors_match_independent_forward_years() {
    for start in Weekday::ALL {
        for mut spec in Spec::cases() {
            spec.start = start;
            let candidates: Vec<_> = (2000..=2412).map(|year| (year, spec.dates(year))).collect();
            let (origin, first) = candidates
                .iter()
                .find(|(_, values)| !values.is_empty())
                .unwrap();
            let anchor = first[0];
            for interval in [1_u32, 2, 3, 100, 400, u32::MAX] {
                let parts = format!("{};INTERVAL={interval}", spec.parts());
                let zones = admitted(&parts, &stamp(anchor));
                let mut expected = BTreeSet::from([anchor]);
                let mut year = u32::from(*origin);
                while year <= 2412 {
                    let index = usize::try_from(year.checked_sub(2000).unwrap()).unwrap();
                    expected.extend(
                        candidates[index]
                            .1
                            .iter()
                            .copied()
                            .filter(|value| *value >= anchor),
                    );
                    let Some(next) = year.checked_add(interval) else {
                        break;
                    };
                    year = next;
                }
                for year in anchor.year()..=anchor.year().checked_add(400).unwrap() {
                    let dates = expected
                        .iter()
                        .copied()
                        .filter(|value| value.year() == year)
                        .collect();
                    assert_eq!(
                        onsets(&zones, year),
                        dates,
                        "{spec:?}, interval={interval}, year={year}"
                    );
                }
            }
        }
    }
}

#[test]
fn adjacent_year_leaps_change_week_and_year_day_selection() {
    let zones = admitted("BYWEEKNO=53;BYDAY=SA", "19990102T020000");
    assert_eq!(onsets(&zones, 2005), BTreeSet::from([date(2005, 1, 1)]));
    assert_eq!(onsets(&zones, 2022), BTreeSet::new());
    let previous = admitted("BYWEEKNO=1;BYYEARDAY=366", "20081231T020000");
    assert!(onsets(&previous, 2008).contains(&date(2008, 12, 31)));
    assert_eq!(onsets(&previous, 2014), BTreeSet::new());
    let next = admitted("BYWEEKNO=-1;BYYEARDAY=-366", "20000101T020000");
    assert!(onsets(&next, 2000).contains(&date(2000, 1, 1)));
    assert_eq!(onsets(&next, 2011), BTreeSet::new());
}

#[test]
fn interval_phases_begin_in_the_week_year_containing_the_source() {
    let zones = admitted("BYWEEKNO=1;BYDAY=MO;INTERVAL=2", "20191230T020000");
    assert_eq!(onsets(&zones, 2021), BTreeSet::new());
    assert_eq!(onsets(&zones, 2022), BTreeSet::from([date(2022, 1, 3)]));
    assert_eq!(onsets(&zones, 2025), BTreeSet::from([date(2025, 12, 29)]));
    let first = admitted(
        "BYWEEKNO=53;BYDAY=FR,SA;INTERVAL=2;COUNT=2",
        "20210101T020000",
    );
    assert_eq!(
        onsets(&first, 2021),
        BTreeSet::from([date(2021, 1, 1), date(2021, 1, 2)])
    );
    assert_eq!(onsets(&first, 2022), BTreeSet::new());
}

#[test]
fn counts_cross_calendar_years_after_complete_period_selection() {
    let zones = admitted("BYWEEKNO=1;BYDAY=MO,TU;COUNT=3", "20191230T020000");
    assert_eq!(
        onsets(&zones, 2019),
        BTreeSet::from([date(2019, 12, 30), date(2019, 12, 31)])
    );
    assert_eq!(onsets(&zones, 2020), BTreeSet::new());
    assert_eq!(onsets(&zones, 2021), BTreeSet::from([date(2021, 1, 4)]));
    assert_eq!(onsets(&zones, 2022), BTreeSet::new());
    for count in [1_u32, 2, 3, 31, 400, 401, u32::MAX] {
        let spec = Spec {
            weekdays: Some(&Weekday::ALL),
            positions: Some(&[1, 366, -1]),
            ..Spec::new(&[
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
                24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44,
                45, 46, 47, 48, 49, 50, 51, 52, 53,
            ])
        };
        let anchor = date(2003, 12, 29);
        let expected: BTreeSet<_> = (2004..=2410)
            .flat_map(|year| spec.dates(year))
            .filter(|value| *value >= anchor)
            .take(usize::try_from(count).unwrap())
            .collect();
        let counted = admitted(&format!("{};COUNT={count}", spec.parts()), &stamp(anchor));
        for year in 2003..=2404 {
            assert_eq!(
                onsets(&counted, year),
                expected
                    .iter()
                    .copied()
                    .filter(|value| value.year() == year)
                    .collect(),
                "count={count}, year={year}"
            );
        }
    }
}

#[test]
fn utc_ends_match_the_generating_week_year_and_selected_position() {
    let valid = admitted(
        "BYWEEKNO=1;BYDAY=MO;INTERVAL=2;UNTIL=20251229T020000Z",
        "20191230T020000",
    );
    assert_eq!(onsets(&valid, 2025), BTreeSet::from([date(2025, 12, 29)]));
    assert_eq!(onsets(&valid, 2026), BTreeSet::new());
    for part in [
        "BYWEEKNO=1;BYDAY=MO;INTERVAL=2;UNTIL=20241230T020000Z",
        "BYWEEKNO=1;BYDAY=MO;UNTIL=20251229T015959Z",
        "BYWEEKNO=1;BYDAY=MO,TU;BYSETPOS=-1;UNTIL=20251229T020000Z",
    ] {
        assert_refused(part);
    }
}

#[test]
fn numeric_weekdays_are_refused_before_redundant_terms_disappear() {
    for days in ["1MO", "MO,1MO", "1MO,MO", "MO,+01MO", "-1SU,SU", "SU,-1SU"] {
        for parts in [
            format!("BYWEEKNO=1;BYDAY={days}"),
            format!("BYDAY={days};BYWEEKNO=1;COUNT=1"),
        ] {
            assert_refused(&parts);
        }
    }
}

#[test]
fn malformed_and_repeated_week_numbers_preserve_explicit_data() {
    for value in [
        "", "0", "+0", "-0", "54", "-54", "001", "+001", "-001", "1,", ",1", "1,,2", "1x", "MO",
    ] {
        assert_refused(&format!("BYWEEKNO={value}"));
    }
    assert_refused("BYWEEKNO=1;byweekno=-1");
    for spelling in ["1,-1", "+01,-01,1,-1,01", "01,-1,01,-01"] {
        let zones = admitted(
            &format!("bYwEeKnO={spelling};bYdAy=mO;wKsT=mO"),
            "20191230T020000",
        );
        assert_eq!(
            onsets(&zones, 2021),
            BTreeSet::from([date(2021, 1, 4), date(2021, 12, 27)])
        );
    }
}

#[test]
fn explicit_dates_keep_their_clocks_and_years_outside_rule_limits() {
    let (zones, meter, diagnostics) = read(
        "BYWEEKNO=1;BYDAY=MO;INTERVAL=2;COUNT=2",
        "20191230T020000",
        "RDATE:20210317T043000,20270317T043000\r\n",
        Limits::DEFAULT,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(meter.vtimezone_observances(), 3);
    assert_eq!(onsets(&zones, 2021), BTreeSet::from([date(2021, 3, 17)]));
    assert_eq!(onsets(&zones, 2022), BTreeSet::from([date(2022, 1, 3)]));
    assert_eq!(onsets(&zones, 2027), BTreeSet::from([date(2027, 3, 17)]));
    let explicit: Vec<_> = zones
        .table(TZID)
        .unwrap()
        .observances()
        .iter()
        .filter(|entry| entry.rule().is_none())
        .map(|entry| entry.start())
        .collect();
    assert_eq!(
        explicit,
        [2021, 2027].map(|year| CivilDateTime::new(
            date(year, 3, 17),
            CivilTime::from_hms(4, 30, 0).unwrap()
        ))
    );
    for observance in zones.table(TZID).unwrap().rules() {
        assert_eq!(
            observance.start(),
            CivilDateTime::new(date(2019, 12, 30), CivilTime::from_hms(2, 0, 0).unwrap())
        );
        assert_eq!(observance.offset_from(), UtcOffset::UTC);
        assert_eq!(observance.offset_to().seconds(), 3600);
        assert_eq!(observance.rule().unwrap().through(), Some(date(2022, 1, 3)));
    }
}

#[test]
fn civil_edges_clip_dates_before_selecting_positions_and_counting() {
    let parts = "BYWEEKNO=1;BYDAY=TH,FR,SA;BYSETPOS=2,-2;WKST=WE";
    for (count, expected, end) in [
        (
            2_u32,
            BTreeSet::from([date(9999, 1, 1), date(9999, 12, 30)]),
            Some(date(9999, 12, 30)),
        ),
        (
            3,
            BTreeSet::from([date(9999, 1, 1), date(9999, 12, 30), date(9999, 12, 31)]),
            Some(date(9999, 12, 31)),
        ),
        (
            4,
            BTreeSet::from([date(9999, 1, 1), date(9999, 12, 30), date(9999, 12, 31)]),
            None,
        ),
    ] {
        let zones = admitted(&format!("{parts};COUNT={count}"), "99990101T020000");
        assert_eq!(onsets(&zones, 9999), expected);
        assert_eq!(zones.table(TZID).unwrap().coverage_end(), end);
    }
    for (count, end) in [
        (9601_u32, Some(date(9999, 12, 30))),
        (9602, Some(date(9999, 12, 31))),
        (9603, None),
    ] {
        let zones = admitted(&format!("{parts};COUNT={count}"), "03991231T020000");
        assert_eq!(
            zones.table(TZID).unwrap().coverage_end(),
            end,
            "count={count}"
        );
        assert!(onsets(&zones, 9999).contains(&date(9999, 12, 30)));
    }
    let lower = admitted("BYWEEKNO=-1;BYDAY=SA,SU;COUNT=2", "00000101T020000");
    assert_eq!(
        onsets(&lower, 0),
        BTreeSet::from([date(0, 1, 1), date(0, 1, 2)])
    );
    assert_eq!(
        lower.table(TZID).unwrap().coverage_end(),
        Some(date(0, 1, 2))
    );
}

fn every_week() -> String {
    format!(
        "BYWEEKNO={};BYDAY=MO,TU,WE,TH,FR,SA,SU",
        numbers(&(1_i8..=53).collect::<Vec<_>>())
    )
}

#[test]
fn cycle_summaries_keep_the_exact_last_writable_date() {
    for (count, end) in [
        (3_652_425_u32, Some(date(9999, 12, 31))),
        (3_652_426, None),
        (u32::MAX, None),
    ] {
        let zones = admitted(
            &format!("{};COUNT={count}", every_week()),
            "00000101T020000",
        );
        assert_eq!(
            zones.table(TZID).unwrap().coverage_end(),
            end,
            "count={count}"
        );
        assert_eq!(onsets(&zones, 9999).len(), 365);
    }
    let huge = admitted(
        "BYWEEKNO=1;BYDAY=MO;INTERVAL=4294967295;COUNT=4294967295",
        "20191230T020000",
    );
    assert_eq!(onsets(&huge, 9999), BTreeSet::new());
    assert_eq!(huge.table(TZID).unwrap().coverage_end(), None);
}

#[test]
fn stored_patterns_keep_canonical_bounds_and_drop_unused_counted_dates() {
    for cap in [1_u32, 371, 372, 373] {
        let (zones, meter, diagnostics) = read(
            &every_week(),
            "20000103T020000",
            "",
            Limits::DEFAULT.with_max_vtimezone_observances(cap),
        );
        let table = zones.table(TZID).unwrap();
        assert_eq!(meter.vtimezone_observances(), cap.min(372));
        assert_eq!(table.rules().len(), usize::try_from(cap.min(372)).unwrap());
        assert_eq!(table.is_truncated(), cap < 372);
        assert_eq!(
            diagnostics.iter().any(
                |diagnostic| diagnostic.code() == DiagnosticCode::VtimezoneObservancesTruncated
            ),
            cap < 372
        );
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::VtimezoneRuleUnsupported)
        );
        if cap >= 372 {
            assert_eq!(onsets(&zones, 2001).len(), 365);
        }
    }
    for count in [1_u32, 2] {
        let (zones, meter, diagnostics) = read(
            &format!("{};COUNT={count}", every_week()),
            "20191230T020000",
            "",
            Limits::DEFAULT.with_max_vtimezone_observances(1),
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(meter.vtimezone_observances(), 1);
        assert_eq!(onsets(&zones, 2019).len(), usize::try_from(count).unwrap());
    }
}
