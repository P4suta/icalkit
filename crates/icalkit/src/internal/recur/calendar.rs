// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;

use crate::internal::core::{
    CalendarCoordinates, CalendarMonth, CalendarScale, CivilDate, Weekday,
};
use crate::internal::recur::rule::Skip;

/// Calendar coordinates could not be represented or reconciled with the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalendarArithmeticError;

#[derive(Clone, Copy, Debug)]
pub(super) struct DateSpan {
    first: i64,
    days: u16,
    end: i64,
}

impl DateSpan {
    pub(super) const fn first(self) -> i64 {
        self.first
    }

    pub(super) const fn days(self) -> u16 {
        self.days
    }

    pub(super) fn new(first: i64, days: u16) -> Result<Self, CalendarArithmeticError> {
        if days == 0 {
            return Err(CalendarArithmeticError);
        }
        let end = first
            .checked_add(i64::from(days))
            .ok_or(CalendarArithmeticError)?;
        Ok(Self { first, days, end })
    }

    pub(super) fn end(self) -> i64 {
        self.end
    }

    fn contains(self, day: i64) -> bool {
        day >= self.first && day < self.end()
    }

    pub(super) fn writable(self) -> Result<Option<(CivilDate, u16)>, CalendarArithmeticError> {
        let minimum = CivilDate::from_ymd(0, 1, 1)
            .and_then(CivilDate::days_from_epoch)
            .ok_or(CalendarArithmeticError)?;
        let maximum = CivilDate::from_ymd(9999, 12, 31)
            .and_then(CivilDate::days_from_epoch)
            .and_then(|day| day.checked_add(1))
            .ok_or(CalendarArithmeticError)?;
        let first = self.first.max(minimum);
        let end = self.end().min(maximum);
        if first >= end {
            return Ok(None);
        }
        let date = CivilDate::from_days_from_epoch(first).ok_or(CalendarArithmeticError)?;
        let days = u16::try_from(end.checked_sub(first).ok_or(CalendarArithmeticError)?)
            .map_err(|_| CalendarArithmeticError)?;
        Ok(Some((date, days)))
    }

    pub(super) fn resolve(
        self,
        value: i32,
        skip: Skip,
    ) -> Result<Option<i64>, CalendarArithmeticError> {
        if value == 0 {
            return Ok(None);
        }
        let length = i32::from(self.days);
        let position = if value.is_positive() {
            value
        } else {
            length
                .checked_add(value)
                .and_then(|rank| rank.checked_add(1))
                .ok_or(CalendarArithmeticError)?
        };
        if (1..=length).contains(&position) {
            return i64::from(position)
                .checked_sub(1)
                .and_then(|offset| self.first.checked_add(offset))
                .map(Some)
                .ok_or(CalendarArithmeticError);
        }
        match (skip, value.is_positive()) {
            (Skip::Omit, _) => Ok(None),
            (Skip::Backward, true) => self
                .end()
                .checked_sub(1)
                .map(Some)
                .ok_or(CalendarArithmeticError),
            (Skip::Backward, false) => self
                .first
                .checked_sub(1)
                .map(Some)
                .ok_or(CalendarArithmeticError),
            (Skip::Forward, true) => Ok(Some(self.end())),
            (Skip::Forward, false) => Ok(Some(self.first)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CalendarFields {
    pub(super) year: i32,
    pub(super) month: CalendarMonth,
    pub(super) day: u8,
    pub(super) year_day: u16,
    pub(super) month_span: DateSpan,
    pub(super) year_span: DateSpan,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CalendarMonthFields {
    pub(super) year: i32,
    pub(super) month: CalendarMonth,
    pub(super) day: u8,
    pub(super) days: u8,
}

#[derive(Clone, Copy, Debug)]
struct MonthSpan {
    month: CalendarMonth,
    span: DateSpan,
}

#[derive(Debug)]
struct YearMetadata {
    year: i32,
    span: DateSpan,
    months: Vec<MonthSpan>,
}

impl YearMetadata {
    fn new(scale: CalendarScale, year: i32) -> Result<Self, CalendarArithmeticError> {
        let first_month = CalendarMonth::from_parts(1, false).ok_or(CalendarArithmeticError)?;
        let mut current =
            CalendarCoordinates::new(scale, year, first_month, 1).ok_or(CalendarArithmeticError)?;
        let limits = scale.ranges();
        let count = current.months_in_year();
        let length = current.days_in_year();
        if count == 0 || count > limits.months || length == 0 || length > limits.year_days {
            return Err(CalendarArithmeticError);
        }
        let span = DateSpan::new(
            current.days_from_epoch().ok_or(CalendarArithmeticError)?,
            length,
        )?;
        let mut months: Vec<MonthSpan> = Vec::with_capacity(usize::from(count));
        let mut reached = span.first;
        for _ in 0..count {
            let month = current.month().ok_or(CalendarArithmeticError)?;
            let days = current.days_in_month();
            let first = current.days_from_epoch().ok_or(CalendarArithmeticError)?;
            if current.year() != year
                || current.day() != 1
                || first != reached
                || days == 0
                || days > limits.month_days
                || months.iter().any(|entry| entry.month == month)
            {
                return Err(CalendarArithmeticError);
            }
            let month_span = DateSpan::new(first, u16::from(days))?;
            months.push(MonthSpan {
                month,
                span: month_span,
            });
            reached = month_span.end();
            current = current
                .checked_add_months(1)
                .ok_or(CalendarArithmeticError)?;
        }
        if reached != span.end() {
            return Err(CalendarArithmeticError);
        }
        Ok(Self { year, span, months })
    }

    fn fields(&self, day: i64) -> Result<CalendarFields, CalendarArithmeticError> {
        let month = self
            .months
            .iter()
            .find(|month| month.span.contains(day))
            .ok_or(CalendarArithmeticError)?;
        let month_day = u8::try_from(
            day.checked_sub(month.span.first)
                .and_then(|value| value.checked_add(1))
                .ok_or(CalendarArithmeticError)?,
        )
        .map_err(|_| CalendarArithmeticError)?;
        let year_day = u16::try_from(
            day.checked_sub(self.span.first)
                .and_then(|value| value.checked_add(1))
                .ok_or(CalendarArithmeticError)?,
        )
        .map_err(|_| CalendarArithmeticError)?;
        Ok(CalendarFields {
            year: self.year,
            month: month.month,
            day: month_day,
            year_day,
            month_span: month.span,
            year_span: self.span,
        })
    }
}

#[derive(Debug)]
pub(super) struct CalendarFrame {
    scale: CalendarScale,
    years: [Option<YearMetadata>; 4],
}

impl CalendarFrame {
    pub(super) fn new(scale: CalendarScale) -> Self {
        Self {
            scale,
            years: core::array::from_fn(|_| None),
        }
    }

    fn year(&mut self, year: i32) -> Result<&YearMetadata, CalendarArithmeticError> {
        let existing = self
            .years
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|entry| entry.year == year));
        let index = if let Some(index) = existing {
            index
        } else {
            let metadata = YearMetadata::new(self.scale, year)?;
            let index = if let Some(index) = self.years.iter().position(Option::is_none) {
                index
            } else {
                self.years.rotate_left(1);
                self.years
                    .len()
                    .checked_sub(1)
                    .ok_or(CalendarArithmeticError)?
            };
            *self.years.get_mut(index).ok_or(CalendarArithmeticError)? = Some(metadata);
            index
        };
        self.years
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(CalendarArithmeticError)
    }

    pub(super) fn month_fields(
        &mut self,
        date: CivilDate,
    ) -> Result<CalendarMonthFields, CalendarArithmeticError> {
        if self.scale == CalendarScale::Gregorian {
            return Ok(CalendarMonthFields {
                year: i32::from(date.year()),
                month: CalendarMonth::from_parts(date.month(), false)
                    .ok_or(CalendarArithmeticError)?,
                day: date.day(),
                days: CivilDate::days_in_month(date.year(), date.month())
                    .ok_or(CalendarArithmeticError)?,
            });
        }
        let fields = self.fields(date)?;
        Ok(CalendarMonthFields {
            year: fields.year,
            month: fields.month,
            day: fields.day,
            days: u8::try_from(fields.month_span.days()).map_err(|_| CalendarArithmeticError)?,
        })
    }

    pub(super) fn fields(
        &mut self,
        date: CivilDate,
    ) -> Result<CalendarFields, CalendarArithmeticError> {
        if self.scale == CalendarScale::Gregorian {
            return gregorian_fields(date);
        }
        let day = date.days_from_epoch().ok_or(CalendarArithmeticError)?;
        if let Some(year) = self
            .years
            .iter()
            .flatten()
            .find(|year| year.span.contains(day))
        {
            return year.fields(day);
        }
        let gregorian = jiff::civil::Date::new(
            i16::try_from(date.year()).map_err(|_| CalendarArithmeticError)?,
            i8::try_from(date.month()).map_err(|_| CalendarArithmeticError)?,
            i8::try_from(date.day()).map_err(|_| CalendarArithmeticError)?,
        )
        .map_err(|_| CalendarArithmeticError)?;
        let converted = CalendarCoordinates::from_gregorian(self.scale, gregorian)
            .ok_or(CalendarArithmeticError)?;
        self.year(converted.year())?.fields(day)
    }

    pub(super) fn year_span(&mut self, year: i32) -> Result<DateSpan, CalendarArithmeticError> {
        if self.scale == CalendarScale::Gregorian && (0..=9999).contains(&year) {
            let year = u16::try_from(year).map_err(|_| CalendarArithmeticError)?;
            let first = CivilDate::from_ymd(year, 1, 1)
                .and_then(CivilDate::days_from_epoch)
                .ok_or(CalendarArithmeticError)?;
            return DateSpan::new(
                first,
                if CivilDate::is_leap_year(year) {
                    366
                } else {
                    365
                },
            );
        }
        Ok(self.year(year)?.span)
    }

    pub(super) fn month_span(
        &mut self,
        year: i32,
        month: CalendarMonth,
    ) -> Result<Option<DateSpan>, CalendarArithmeticError> {
        if self.scale == CalendarScale::Gregorian && (0..=9999).contains(&year) {
            if month.is_leap() {
                return Ok(None);
            }
            let year = u16::try_from(year).map_err(|_| CalendarArithmeticError)?;
            let Some(first) = CivilDate::from_ymd(year, month.number(), 1) else {
                return Ok(None);
            };
            let days =
                CivilDate::days_in_month(year, month.number()).ok_or(CalendarArithmeticError)?;
            return DateSpan::new(
                first.days_from_epoch().ok_or(CalendarArithmeticError)?,
                u16::from(days),
            )
            .map(Some);
        }
        Ok(self
            .year(year)?
            .months
            .iter()
            .find(|entry| entry.month == month)
            .map(|entry| entry.span))
    }

    pub(super) fn resolve_month(
        &mut self,
        year: i32,
        month: CalendarMonth,
        skip: Skip,
    ) -> Result<Option<DateSpan>, CalendarArithmeticError> {
        if let Some(span) = self.month_span(year, month)? {
            return Ok(Some(span));
        }
        if skip == Skip::Omit {
            return Ok(None);
        }
        if !month.is_leap() {
            return Err(CalendarArithmeticError);
        }
        let regular =
            CalendarMonth::from_parts(month.number(), false).ok_or(CalendarArithmeticError)?;
        let base = self
            .month_span(year, regular)?
            .ok_or(CalendarArithmeticError)?;
        if skip == Skip::Backward {
            return Ok(Some(base));
        }
        let next = base.end();
        let entries = &self.year(year)?.months;
        if let Some(entry) = entries.iter().find(|entry| entry.span.first == next) {
            return Ok(Some(entry.span));
        }
        let next_year = year.checked_add(1).ok_or(CalendarArithmeticError)?;
        self.year(next_year)?
            .months
            .first()
            .map(|entry| Some(entry.span))
            .ok_or(CalendarArithmeticError)
    }

    pub(super) fn week_year_span(
        &mut self,
        year: i32,
        wkst: Weekday,
    ) -> Result<DateSpan, CalendarArithmeticError> {
        let first = self.week_one_start(year, wkst)?;
        let next =
            self.week_one_start(year.checked_add(1).ok_or(CalendarArithmeticError)?, wkst)?;
        let days = u16::try_from(next.checked_sub(first).ok_or(CalendarArithmeticError)?)
            .map_err(|_| CalendarArithmeticError)?;
        if !days.is_multiple_of(7) {
            return Err(CalendarArithmeticError);
        }
        DateSpan::new(first, days)
    }

    fn week_one_start(&mut self, year: i32, wkst: Weekday) -> Result<i64, CalendarArithmeticError> {
        let fourth = self
            .year_span(year)?
            .first
            .checked_add(3)
            .ok_or(CalendarArithmeticError)?;
        let index = fourth
            .checked_add(3)
            .ok_or(CalendarArithmeticError)?
            .rem_euclid(7);
        let weekday =
            Weekday::from_index(u8::try_from(index).map_err(|_| CalendarArithmeticError)?)
                .ok_or(CalendarArithmeticError)?;
        fourth
            .checked_sub(i64::from(week_offset(weekday, wkst)))
            .ok_or(CalendarArithmeticError)
    }

    pub(super) fn week_of(
        &mut self,
        date: CivilDate,
        wkst: Weekday,
    ) -> Result<(i32, u8), CalendarArithmeticError> {
        let day = date.days_from_epoch().ok_or(CalendarArithmeticError)?;
        let weekday = date.weekday().ok_or(CalendarArithmeticError)?;
        let offset = 3_i64
            .checked_sub(i64::from(week_offset(weekday, wkst)))
            .ok_or(CalendarArithmeticError)?;
        let pivot = day.checked_add(offset).ok_or(CalendarArithmeticError)?;
        let mut year = self.fields(date)?.year;
        let span = self.year_span(year)?;
        if pivot < span.first {
            year = year.checked_sub(1).ok_or(CalendarArithmeticError)?;
        }
        if pivot >= span.end() {
            year = year.checked_add(1).ok_or(CalendarArithmeticError)?;
        }
        let elapsed = pivot
            .checked_sub(self.year_span(year)?.first)
            .ok_or(CalendarArithmeticError)?;
        let number = u8::try_from(
            elapsed
                .div_euclid(7)
                .checked_add(1)
                .ok_or(CalendarArithmeticError)?,
        )
        .map_err(|_| CalendarArithmeticError)?;
        Ok((year, number))
    }
}

fn gregorian_fields(date: CivilDate) -> Result<CalendarFields, CalendarArithmeticError> {
    let year = date.year();
    let first = CivilDate::from_ymd(year, 1, 1)
        .and_then(CivilDate::days_from_epoch)
        .ok_or(CalendarArithmeticError)?;
    let month_first = CivilDate::from_ymd(year, date.month(), 1)
        .and_then(CivilDate::days_from_epoch)
        .ok_or(CalendarArithmeticError)?;
    let day = date.days_from_epoch().ok_or(CalendarArithmeticError)?;
    let year_day = u16::try_from(
        day.checked_sub(first)
            .and_then(|value| value.checked_add(1))
            .ok_or(CalendarArithmeticError)?,
    )
    .map_err(|_| CalendarArithmeticError)?;
    let year_span = DateSpan::new(
        first,
        if CivilDate::is_leap_year(year) {
            366
        } else {
            365
        },
    )?;
    let month_span = DateSpan::new(
        month_first,
        u16::from(CivilDate::days_in_month(year, date.month()).ok_or(CalendarArithmeticError)?),
    )?;
    Ok(CalendarFields {
        year: i32::from(year),
        month: CalendarMonth::from_parts(date.month(), false).ok_or(CalendarArithmeticError)?,
        day: date.day(),
        year_day,
        month_span,
        year_span,
    })
}

pub(super) fn week_offset(weekday: Weekday, wkst: Weekday) -> u8 {
    let reached = weekday.index();
    let begins = wkst.index();
    let distance = reached.abs_diff(begins);
    if reached >= begins {
        distance
    } else {
        7_u8.abs_diff(distance)
    }
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{
        CalendarCoordinates, CalendarMonth, CalendarScale, CivilDate, Weekday,
    };
    use crate::internal::recur::Skip;

    use super::{CalendarFrame, DateSpan};

    fn civil(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn month(number: u8, leap: bool) -> CalendarMonth {
        CalendarMonth::from_parts(number, leap).unwrap()
    }

    #[test]
    fn calendar_year_metadata_preserves_thirteenth_and_leap_months() {
        let mut ethiopic = CalendarFrame::new(CalendarScale::Ethiopic);
        assert_eq!(ethiopic.year_span(2005).unwrap().days, 365);
        let last = ethiopic
            .month_span(2005, month(13, false))
            .unwrap()
            .unwrap();
        assert_eq!(last.days, 5);
        assert_eq!(
            CivilDate::from_days_from_epoch(last.first),
            Some(civil(2013, 9, 6))
        );
        let mut hebrew = CalendarFrame::new(CalendarScale::Hebrew);
        assert_eq!(hebrew.year_span(5774).unwrap().days, 385);
        assert_eq!(
            hebrew
                .month_span(5774, month(5, true))
                .unwrap()
                .unwrap()
                .days,
            30
        );
        assert!(hebrew.month_span(5775, month(5, true)).unwrap().is_none());
        assert_eq!(
            hebrew
                .month_span(5775, month(6, false))
                .unwrap()
                .unwrap()
                .days,
            29
        );
        let mut chinese = CalendarFrame::new(CalendarScale::Chinese);
        let first = chinese.year_span(2013).unwrap();
        assert_eq!(first.days, 355);
        assert_eq!(
            CivilDate::from_days_from_epoch(first.first),
            Some(civil(2013, 2, 10))
        );
    }

    #[test]
    fn calendar_fields_agree_with_conversion_across_epoch_and_format_boundaries() {
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
            let scale = CalendarScale::from_name(name).unwrap();
            let mut frame = CalendarFrame::new(scale);
            for (year, m, day) in [
                (0, 1, 1),
                (1970, 1, 1),
                (2013, 2, 10),
                (2013, 9, 6),
                (2014, 2, 8),
                (9999, 12, 31),
            ] {
                let date = civil(year, m, day);
                let converted = CalendarCoordinates::from_gregorian(
                    scale,
                    jiff::civil::Date::new(
                        i16::try_from(year).unwrap(),
                        i8::try_from(m).unwrap(),
                        i8::try_from(day).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap();
                let fields = frame.fields(date).unwrap();
                let month_fields = frame.month_fields(date).unwrap();
                assert_eq!(
                    (
                        month_fields.year,
                        month_fields.month,
                        month_fields.day,
                        month_fields.days
                    ),
                    (
                        converted.year(),
                        converted.month().unwrap(),
                        converted.day(),
                        converted.days_in_month()
                    ),
                    "{name} {date:?}"
                );
                assert_eq!(
                    (fields.year, fields.month, fields.day),
                    (
                        converted.year(),
                        converted.month().unwrap(),
                        converted.day()
                    ),
                    "{name} {date:?}"
                );
                assert_eq!(
                    fields.month_span.first + i64::from(fields.day) - 1,
                    date.days_from_epoch().unwrap()
                );
                assert_eq!(
                    fields.year_span.first + i64::from(fields.year_day) - 1,
                    date.days_from_epoch().unwrap()
                );
                assert!(fields.month_span.days <= u16::from(scale.ranges().month_days));
                assert!(fields.year_span.days <= scale.ranges().year_days);
            }
        }
    }

    #[test]
    fn missing_month_resolution_selects_the_regular_neighbors() {
        let mut frame = CalendarFrame::new(CalendarScale::Hebrew);
        assert!(
            frame
                .resolve_month(5775, month(5, true), Skip::Omit)
                .unwrap()
                .is_none()
        );
        let before = frame
            .resolve_month(5775, month(5, true), Skip::Backward)
            .unwrap()
            .unwrap();
        let after = frame
            .resolve_month(5775, month(5, true), Skip::Forward)
            .unwrap()
            .unwrap();
        assert_eq!(
            CivilDate::from_days_from_epoch(before.first),
            Some(civil(2015, 1, 21))
        );
        assert_eq!(
            CivilDate::from_days_from_epoch(after.first),
            Some(civil(2015, 2, 20))
        );
        let mut frame = CalendarFrame::new(CalendarScale::Ethiopic);
        let next = frame
            .resolve_month(2005, month(13, true), Skip::Forward)
            .unwrap()
            .unwrap();
        assert_eq!(
            CivilDate::from_days_from_epoch(next.first),
            Some(civil(2013, 9, 11))
        );
    }

    #[test]
    fn calendar_week_years_use_the_four_day_rule() {
        let mut frame = CalendarFrame::new(CalendarScale::Hebrew);
        let weeks = frame.week_year_span(5774, Weekday::Monday).unwrap();
        assert_eq!(
            CivilDate::from_days_from_epoch(weeks.first),
            Some(civil(2013, 9, 2))
        );
        assert_eq!(weeks.days, 385);
        assert_eq!(
            frame.week_of(civil(2014, 9, 15), Weekday::Monday).unwrap(),
            (5774, 55)
        );
    }

    #[test]
    fn signed_day_correction_chooses_the_nearest_valid_boundary() {
        let span = DateSpan::new(civil(2013, 9, 6).days_from_epoch().unwrap(), 5).unwrap();
        for (value, skip, expected) in [
            (30, Skip::Backward, civil(2013, 9, 10)),
            (30, Skip::Forward, civil(2013, 9, 11)),
            (-30, Skip::Backward, civil(2013, 9, 5)),
            (-30, Skip::Forward, civil(2013, 9, 6)),
        ] {
            assert_eq!(
                span.resolve(value, skip)
                    .unwrap()
                    .and_then(CivilDate::from_days_from_epoch),
                Some(expected)
            );
        }
        assert!(span.resolve(30, Skip::Omit).unwrap().is_none());
        assert!(span.resolve(-30, Skip::Omit).unwrap().is_none());
    }

    #[test]
    fn year_metadata_refuses_backend_overflow() {
        let mut frame = CalendarFrame::new(CalendarScale::Chinese);
        assert!(frame.year_span(i32::MAX).is_err());
        assert!(frame.year_span(i32::MIN).is_err());
    }
}
