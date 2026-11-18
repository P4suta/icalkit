// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{CivilDate, Weekday};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Pattern {
    LeapAnniversary,
    MarchSunday,
    FebruaryEnd,
    MonthStarts,
    YearMonday,
    SelectedSundays,
    YearDays,
    FilteredYearDays,
}

impl Pattern {
    pub(crate) const ALL: [Self; 8] = [
        Self::LeapAnniversary,
        Self::MarchSunday,
        Self::FebruaryEnd,
        Self::MonthStarts,
        Self::YearMonday,
        Self::SelectedSundays,
        Self::YearDays,
        Self::FilteredYearDays,
    ];

    pub(crate) fn parts(self) -> &'static str {
        match self {
            Self::LeapAnniversary => "",
            Self::MarchSunday => "BYMONTH=3;BYDAY=2SU",
            Self::FebruaryEnd => "BYMONTH=2;BYMONTHDAY=-1",
            Self::MonthStarts => "BYMONTHDAY=1",
            Self::YearMonday => "BYDAY=20MO",
            Self::SelectedSundays => "BYMONTH=3,9;BYDAY=SU;BYSETPOS=1,-1",
            Self::YearDays => "BYYEARDAY=1,60,100,366,-1",
            Self::FilteredYearDays => {
                "BYMONTH=2,3,9;BYYEARDAY=60,61,62,244,245,246;BYMONTHDAY=1,2,3,-1;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1,-1"
            },
        }
    }

    pub(crate) fn dates(self, year: u16) -> BTreeSet<CivilDate> {
        let all: Vec<_> = (1..=12)
            .flat_map(|month| (1..=31).filter_map(move |day| CivilDate::from_ymd(year, month, day)))
            .collect();
        match self {
            Self::LeapAnniversary => CivilDate::from_ymd(year, 2, 29).into_iter().collect(),
            Self::MarchSunday => all
                .into_iter()
                .filter(|value| {
                    value.month() == 3
                        && (8..=14).contains(&value.day())
                        && value.weekday() == Some(Weekday::Sunday)
                })
                .collect(),
            Self::FebruaryEnd => all
                .into_iter()
                .rfind(|value| value.month() == 2)
                .into_iter()
                .collect(),
            Self::MonthStarts => all.into_iter().filter(|value| value.day() == 1).collect(),
            Self::YearMonday => all
                .into_iter()
                .filter(|value| value.weekday() == Some(Weekday::Monday))
                .nth(19)
                .into_iter()
                .collect(),
            Self::SelectedSundays => {
                let selected: Vec<_> = all
                    .into_iter()
                    .filter(|value| {
                        [3, 9].contains(&value.month()) && value.weekday() == Some(Weekday::Sunday)
                    })
                    .collect();
                selected
                    .first()
                    .into_iter()
                    .chain(selected.last())
                    .copied()
                    .collect()
            },
            Self::YearDays => all
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    [1, 60, 100, 366, all.len()].contains(&index.checked_add(1).unwrap())
                })
                .map(|(_, value)| *value)
                .collect(),
            Self::FilteredYearDays => {
                let selected: Vec<_> = all
                    .iter()
                    .enumerate()
                    .filter(|(index, value)| {
                        let ordinal = index.checked_add(1).unwrap();
                        [60, 61, 62, 244, 245, 246].contains(&ordinal)
                            && [2, 3, 9].contains(&value.month())
                            && ([1, 2, 3].contains(&value.day())
                                || all
                                    .get(ordinal)
                                    .is_none_or(|next| next.month() != value.month()))
                            && [
                                Weekday::Monday,
                                Weekday::Tuesday,
                                Weekday::Wednesday,
                                Weekday::Thursday,
                                Weekday::Friday,
                            ]
                            .contains(&value.weekday().unwrap())
                    })
                    .map(|(_, value)| *value)
                    .collect();
                selected
                    .first()
                    .into_iter()
                    .chain(selected.last())
                    .copied()
                    .collect()
            },
        }
    }
}
