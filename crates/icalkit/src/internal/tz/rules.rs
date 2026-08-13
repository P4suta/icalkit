// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 1 — evaluating a [`YearlyRule`] in a given year, in closed form.
//!
//! Specification: RFC 5545 section 3.6.5's observance `RRULE`, restricted to the yearly forms
//! [`RuleDay`] names.
//!
//! Owed by this unit and by nothing else, as inherent methods on types the crate root already
//! exports, so no re-export line is needed:
//!
//! ```text
//! impl YearlyRule {
//!     pub fn occurrence_in(self, year: u16) -> Option<CivilDate>;
//!     pub fn applies_in(self, year: u16) -> bool;
//! }
//! impl Observance {
//!     pub fn transition_in(self, year: u16) -> Option<CivilDateTime>;
//! }
//! ```
//!
//! Arithmetic over the weekday of the first of the month and nothing else: no loop over
//! candidate dates, no search, and therefore no budget to charge and no way for a zone lookup
//! to do unbounded work. That structural bound is what lets [`ZoneSource::resolve`] take
//! neither a `Limits` nor a `Meter`, so the trait's shape depends on this unit keeping it.
//!
//! `None` is the answer for a day the month does not have — a fifth Sunday in a four-Sunday
//! month, day 31 of April, a year past a rule's `through` — and never a nearby date, per
//! `docs/adr/0011`. [`NthWeek::Fifth`] and [`NthWeek::Last`] must differ in exactly those
//! months, which is the case a single implementation of both gets wrong first.
//!
//! # How every day form collapses onto one primitive
//!
//! There are two directions and everything else is a starting day. [`on_or_after`] steps
//! forward from a day of the month to the next given weekday, [`on_or_before`] steps back from
//! one, and each step is a difference of two weekday indices reduced modulo seven. The *n*th
//! weekday of a month is then the first one on or after day 1, 8, 15, 22 or 29, and the last is
//! the last one on or before the month's final day. Writing `Fifth` that way is what makes it
//! differ from `Last` for free rather than by a special case somebody has to remember: a month
//! whose 29th does not exist, or whose 29th onward holds no such weekday, has no fifth one and
//! says so, while `Last` is asked a question every month can answer.
//!
//! The one asymmetry is deliberate. `OnOrAfter`'s day is a date that has to exist — the first
//! Sunday on or after April 31st is nothing at all, because there is no April 31st to start
//! from — while `OnOrBefore`'s day is an *upper bound* on a search that begins inside the
//! month, so a bound past the month's end constrains nothing and the answer is the month's last
//! such weekday. That is not `docs/adr/0011`'s clamping, which moves a date somebody named onto
//! a nearby one; nobody named the 31st of February here, and the producers writing
//! `BYDAY=SU;BYMONTHDAY=25,26,27,28,29,30,31` mean exactly "the last Sunday" in every month the
//! run reaches past the end of.
//!
//! # Two questions, not one
//!
//! [`YearlyRule::applies_in`] answers only whether the rule is still in force during a year,
//! which is the `UNTIL` window and nothing else. [`YearlyRule::occurrence_in`] answers whether
//! it names a date, which additionally requires the month to hold such a day and the date to
//! fall on or before `UNTIL` itself. They are kept apart because "this rule stopped in 2006" and
//! "this month has only four Sundays" are different facts about a zone, and a caller deciding
//! whether a table has run out of data needs the first without the second answering for it.
//!
//! [`YearlyRule`]: crate::internal::tz::YearlyRule
//! [`RuleDay`]: crate::internal::tz::RuleDay
//! [`NthWeek::Fifth`]: crate::internal::tz::NthWeek::Fifth
//! [`NthWeek::Last`]: crate::internal::tz::NthWeek::Last
//! [`ZoneSource::resolve`]: crate::internal::tz::ZoneSource::resolve

use crate::internal::core::{CivilDate, CivilDateTime, Weekday};

use crate::internal::tz::model::{NthWeek, Observance, RuleDay, YearlyRule};

impl YearlyRule {
    /// Whether this rule is still in force at any point during `year`.
    ///
    /// The window question by itself: a rule with no `UNTIL` is in force in every year, and one
    /// with an `UNTIL` is in force through the year that `UNTIL` falls in. Necessary but not
    /// sufficient for [`YearlyRule::occurrence_in`] to answer — it is `true` for a year whose
    /// month holds no such day, and for the `UNTIL` year when the rule's month falls after the
    /// `UNTIL` date inside it.
    #[must_use]
    pub fn applies_in(self, year: u16) -> bool {
        match self.through() {
            None => true,
            Some(last) => year <= last.year(),
        }
    }

    /// The date this rule names in `year`, or `None` when it names none there.
    ///
    /// `None` covers three different reasons and deliberately does not distinguish them, because
    /// a caller asking for a transition date has the same thing to do about each: the rule had
    /// stopped, the month has no such day, or the date it would name falls past `UNTIL`. The
    /// distinction that matters — whether the zone still has data at all — is
    /// [`YearlyRule::applies_in`] and `TransitionTable::coverage_end`, not this.
    #[must_use]
    pub fn occurrence_in(self, year: u16) -> Option<CivilDate> {
        if !self.applies_in(year) {
            return None;
        }
        let date = day_in_month(year, self.month(), self.day())?;
        // `UNTIL` is a date, not a year: a rule ending in August names nothing in November of
        // its final year, and answering that year from the year comparison alone would invent a
        // transition the file said had already stopped happening.
        if self.through().is_some_and(|last| date > last) {
            return None;
        }
        Some(date)
    }
}

impl Observance {
    /// The wall clock this observance's transition happens at in `year`, or `None` when it has
    /// none there.
    ///
    /// The clock is read against `TZOFFSETFROM`, per RFC 5545 section 3.6.5: the transition
    /// happens when the clock that is still running reaches that time. Resolving it against an
    /// offset is the caller's step and not this one's.
    ///
    /// A date-driven observance — one `RDATE` out of a table — transitions in its own year and
    /// in no other, which is how a table running out becomes visible here as a plain `None`
    /// rather than as the final state quietly continuing. A rule-driven one transitions in
    /// every year its rule names a date in, except that a date the rule would place before the
    /// `DTSTART` it is anchored at is not one: section 3.8.5.3's recurrence set begins at
    /// `DTSTART` and a rule evaluated in closed form has no memory of that on its own.
    #[must_use]
    pub fn transition_in(self, year: u16) -> Option<CivilDateTime> {
        // The observance's own DTSTART is an onset in its own right, so it is the answer
        // whenever the rule has none to give in that year.
        let anchor = (self.start().date().year() == year).then_some(self.start());
        let Some(rule) = self.rule() else {
            return anchor;
        };
        let Some(date) = rule.occurrence_in(year) else {
            return anchor;
        };
        let onset = CivilDateTime::new(date, rule.at());
        if onset < self.start() {
            anchor
        } else {
            Some(onset)
        }
    }
}

/// The date `form` names inside `month` of `year`, or `None` when it names none.
fn day_in_month(year: u16, month: u8, form: RuleDay) -> Option<CivilDate> {
    match form {
        RuleDay::DayOfMonth(number) => CivilDate::from_ymd(year, month, number),
        RuleDay::LastDayOfMonth => {
            CivilDate::from_ymd(year, month, CivilDate::days_in_month(year, month)?)
        },
        RuleDay::Nth { weekday, week } => nth_weekday_in(year, month, weekday, week),
        RuleDay::OnOrAfter { weekday, day } => on_or_after(year, month, weekday, day),
        RuleDay::OnOrBefore { weekday, day } => on_or_before(year, month, weekday, day),
    }
}

/// The `week`th `weekday` of `month` in `year`, or `None` when the month holds no such day.
fn nth_weekday_in(year: u16, month: u8, weekday: Weekday, week: NthWeek) -> Option<CivilDate> {
    let length = CivilDate::days_in_month(year, month)?;
    // The nth occurrence of a weekday is the first one on or after the nth week's opening day,
    // which is why `Fifth` needs no arm of its own beyond its starting day: a month with only
    // four of that weekday finds nothing from the 29th onward, and `Last`, asked the other
    // direction from a day every month has, finds the fourth.
    let earliest = match week {
        NthWeek::First => 1,
        NthWeek::Second => 8,
        NthWeek::Third => 15,
        NthWeek::Fourth => 22,
        NthWeek::Fifth => 29,
        NthWeek::Last => return on_or_before(year, month, weekday, length),
    };
    on_or_after(year, month, weekday, earliest)
}

/// The first `weekday` falling on or after day `day` of `month` in `year`.
///
/// `None` when `day` is not a day that month has, which is the honest answer rather than a
/// search begun from somewhere else: there is no first Sunday on or after April 31st.
fn on_or_after(year: u16, month: u8, weekday: Weekday, day: u8) -> Option<CivilDate> {
    let anchor = CivilDate::from_ymd(year, month, day)?;
    let shift = days_forward(anchor.weekday()?, weekday);
    CivilDate::from_ymd(year, month, anchor.day().checked_add(shift)?)
}

/// The last `weekday` falling on or before day `day` of `month` in `year`.
///
/// `day` is a bound rather than a date, so one past the end of the month is satisfied by the
/// whole month and the answer is the month's last such weekday — which is what a producer
/// writing a run of `BYMONTHDAY` values through 31 means in February. A bound so early that no
/// such weekday precedes it inside the month is `None`, never the previous month's.
fn on_or_before(year: u16, month: u8, weekday: Weekday, day: u8) -> Option<CivilDate> {
    let length = CivilDate::days_in_month(year, month)?;
    let bound = CivilDate::from_ymd(year, month, day.min(length))?;
    let shift = days_forward(weekday, bound.weekday()?);
    CivilDate::from_ymd(year, month, bound.day().checked_sub(shift)?)
}

/// Days from `from` forward to the next `to`, `0` through `6`.
///
/// Both indices are below seven, so the sum is below fourteen and the difference is not
/// negative. The saturating forms are written for the bound the constructor already holds
/// rather than for one reachable here, in the shape `ical-core`'s own arithmetic uses.
fn days_forward(from: Weekday, to: Weekday) -> u8 {
    to.index()
        .saturating_add(7)
        .saturating_sub(from.index())
        .rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{CivilDate, CivilDateTime, CivilTime, UtcOffset, Weekday};

    use crate::internal::tz::model::{NthWeek, Observance, RuleDay, YearlyRule};

    /// One case: the rule's month, its day form, the year asked about, the date expected.
    ///
    /// The zone each group of cases comes from is named at the call rather than in the row,
    /// because a row wide enough to carry a name is a row too wide to read as a table.
    type Case = (u8, RuleDay, u16, Option<CivilDate>);

    fn ymd(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn at_hour(hour: u8) -> CivilTime {
        CivilTime::from_hms(hour, 0, 0).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8) -> CivilDateTime {
        CivilDateTime::new(ymd(year, month, day), at_hour(hour))
    }

    fn sun(week: NthWeek) -> RuleDay {
        RuleDay::Nth {
            weekday: Weekday::Sunday,
            week,
        }
    }

    fn after(weekday: Weekday, day: u8) -> RuleDay {
        RuleDay::OnOrAfter { weekday, day }
    }

    fn before(weekday: Weekday, day: u8) -> RuleDay {
        RuleDay::OnOrBefore { weekday, day }
    }

    fn sun_after(day: u8) -> RuleDay {
        after(Weekday::Sunday, day)
    }

    fn sun_before(day: u8) -> RuleDay {
        before(Weekday::Sunday, day)
    }

    fn fri_after(day: u8) -> RuleDay {
        after(Weekday::Friday, day)
    }

    fn day_of(number: u8) -> RuleDay {
        RuleDay::DayOfMonth(number)
    }

    /// Evaluates each case's rule, which carries no `UNTIL`, in each case's year.
    fn check(zone: &str, cases: &[Case]) {
        for &(month, form, year, expected) in cases {
            let rule = YearlyRule::new(month, form, at_hour(2), None).unwrap();
            let named = rule.occurrence_in(year);
            assert_eq!(
                named, expected,
                "{zone}: {form:?} of month {month} in {year}"
            );
        }
    }

    /// The one primitive the closed form is written over, pinned before anything is written
    /// over it. Every day form reduces to the weekday of the first of the month and the
    /// month's length, both of which `ical-core` already answers totally.
    #[test]
    fn the_weekday_of_the_first_of_a_month_is_what_every_day_form_reduces_to() {
        let first = CivilDate::from_ymd(2026, 8, 1).unwrap();
        assert_eq!(first.weekday(), Some(Weekday::Saturday));
        assert_eq!(CivilDate::days_in_month(2026, 8), Some(31));
        assert_eq!(
            CivilDate::from_ymd(2026, 4, 31),
            None,
            "a day the month does not have is refused rather than clamped"
        );
    }

    /// Every expectation is a day a real zone really moved its clocks on, taken from the rule
    /// the tz database states for that zone and not from what this code returns: the United
    /// States' second Sunday in March and first Sunday in November since the 2005 Energy
    /// Policy Act took effect in 2007, the same country's first Sunday in April and last
    /// Sunday in October before it, and the European Union's last Sunday in March and October.
    #[test]
    fn the_weekday_rules_of_real_zones_land_on_the_days_those_zones_moved() {
        let modern: &[Case] = &[
            (3, sun(NthWeek::Second), 2007, Some(ymd(2007, 3, 11))),
            (11, sun(NthWeek::First), 2007, Some(ymd(2007, 11, 4))),
            (3, sun(NthWeek::Second), 2026, Some(ymd(2026, 3, 8))),
            (11, sun(NthWeek::First), 2026, Some(ymd(2026, 11, 1))),
        ];
        check("America/New_York since 2007", modern);

        let historic: &[Case] = &[
            (4, sun(NthWeek::First), 2006, Some(ymd(2006, 4, 2))),
            (10, sun(NthWeek::Last), 2006, Some(ymd(2006, 10, 29))),
        ];
        check("America/New_York before 2007", historic);

        let union: &[Case] = &[
            (3, sun(NthWeek::Last), 2007, Some(ymd(2007, 3, 25))),
            (3, sun(NthWeek::Last), 2026, Some(ymd(2026, 3, 29))),
            (10, sun(NthWeek::Last), 2026, Some(ymd(2026, 10, 25))),
        ];
        check("Europe/Berlin", union);
    }

    /// A zone whose daylight saving is half an hour still transitions on a weekday rule, and
    /// the half hour is the offset's business rather than the day's. `Australia/Lord_Howe`
    /// moves on the first Sunday in October and the first Sunday in April, the same day form
    /// its mainland neighbors use, which is why a rule evaluator never has to know about the
    /// thirty minutes at all.
    #[test]
    fn a_zone_whose_daylight_saving_is_half_an_hour_names_its_days_the_same_way() {
        let lord_howe: &[Case] = &[
            (10, sun(NthWeek::First), 2026, Some(ymd(2026, 10, 4))),
            (4, sun(NthWeek::First), 2026, Some(ymd(2026, 4, 5))),
            (10, sun(NthWeek::First), 2027, Some(ymd(2027, 10, 3))),
        ];
        check("Australia/Lord_Howe", lord_howe);

        let half_hour = UtcOffset::from_seconds(37_800).unwrap();
        let full = UtcOffset::from_seconds(39_600).unwrap();
        let rule = YearlyRule::new(10, sun(NthWeek::First), at_hour(2), None).unwrap();
        let starts = Observance::new(stamp(2008, 10, 5, 2), half_hour, full, true, Some(rule));
        assert_eq!(starts.transition_in(2026), Some(stamp(2026, 10, 4, 2)));
        assert!(starts.moves_the_clock());
    }

    /// No identifier reaches this unit, which is the point. Exchange writes
    /// `W. Europe Standard Time` over the same two `BYDAY=-1SU` rules the tz database states
    /// for `Europe/Berlin`, at 02:00 standard and 03:00 daylight — both 01:00 UTC — and the
    /// evaluation cannot tell the two files apart because nothing here is given a `TZID` to
    /// parse. A crate that tried to read one would be wrong on this file.
    #[test]
    fn a_tzid_that_is_not_an_iana_name_changes_nothing_because_none_reaches_this_unit() {
        let begins = YearlyRule::new(3, sun(NthWeek::Last), at_hour(2), None).unwrap();
        let ends = YearlyRule::new(10, sun(NthWeek::Last), at_hour(3), None).unwrap();
        assert_eq!(begins.occurrence_in(2026), Some(ymd(2026, 3, 29)));
        assert_eq!(ends.occurrence_in(2026), Some(ymd(2026, 10, 25)));
        assert_eq!(
            begins.at(),
            at_hour(2),
            "read against TZOFFSETFROM, not UTC"
        );
        assert_eq!(ends.at(), at_hour(3), "read against TZOFFSETFROM, not UTC");
    }

    /// The case one implementation of both gets wrong first. March 2026 opens on a Sunday and
    /// so holds five of them; October 2026 opens on a Thursday and holds four, and a February
    /// of 28 days holds four of every weekday and five of none.
    #[test]
    fn a_fifth_weekday_and_a_last_one_differ_in_exactly_the_months_without_five() {
        let five: &[Case] = &[
            (3, sun(NthWeek::Fifth), 2026, Some(ymd(2026, 3, 29))),
            (3, sun(NthWeek::Last), 2026, Some(ymd(2026, 3, 29))),
            (5, sun(NthWeek::Fifth), 2026, Some(ymd(2026, 5, 31))),
        ];
        check("a month holding five Sundays", five);

        let four: &[Case] = &[
            (10, sun(NthWeek::Fifth), 2026, None),
            (10, sun(NthWeek::Last), 2026, Some(ymd(2026, 10, 25))),
            (10, sun(NthWeek::Fourth), 2026, Some(ymd(2026, 10, 25))),
            (10, sun(NthWeek::Third), 2026, Some(ymd(2026, 10, 18))),
            (2, sun(NthWeek::Fifth), 2026, None),
            (2, sun(NthWeek::Last), 2026, Some(ymd(2026, 2, 22))),
        ];
        check("a month holding four", four);
    }

    /// `Fri>=23` is how the tz database states `Asia/Jerusalem`'s rule — the Friday before the
    /// last Sunday in March, which the rows below check against that Sunday rather than against
    /// each other. The same shape is how iCalendar producers write a rule the database states
    /// as a weekday ordinal: a `BYDAY` paired with a run of `BYMONTHDAY` values, so
    /// `Europe/Berlin`'s last Sunday in March is `SU` on or after the 25th and
    /// `America/New_York`'s first Sunday in November is `SU` on or after the 1st. Both
    /// encodings have to give the day those zones really moved on, and the search has to start
    /// from a day that exists: there is no first Sunday on or after April 31st.
    #[test]
    fn the_first_weekday_on_or_after_a_day_is_where_real_zones_put_their_transition() {
        let jerusalem: &[Case] = &[
            (3, fri_after(23), 2026, Some(ymd(2026, 3, 27))),
            (3, fri_after(23), 2027, Some(ymd(2027, 3, 26))),
            (3, sun(NthWeek::Last), 2026, Some(ymd(2026, 3, 29))),
        ];
        check(
            "Asia/Jerusalem, the Friday before the last Sunday",
            jerusalem,
        );

        let run_form: &[Case] = &[
            (3, sun_after(25), 2026, Some(ymd(2026, 3, 29))),
            (10, sun_after(25), 2026, Some(ymd(2026, 10, 25))),
            (11, sun_after(1), 2026, Some(ymd(2026, 11, 1))),
            (3, sun_after(8), 2026, Some(ymd(2026, 3, 8))),
        ];
        check("the same two zones as a BYMONTHDAY run", run_form);

        let edges: &[Case] = &[
            (10, sun_after(25), 2026, Some(ymd(2026, 10, 25))),
            (2, sun_after(29), 2026, None),
            (4, sun_after(31), 2026, None),
            (3, after(Weekday::Monday, 31), 2026, None),
        ];
        check("the ends of the month", edges);
    }

    /// The bound direction, including the two cases `on_or_before` commits to in its own doc
    /// comment: a bound past the end of the month is satisfied by the whole month and names its
    /// last such weekday, which is what a producer writing `BYMONTHDAY=25,26,27,28,29,30,31`
    /// means in February, and a bound too early for one names nothing rather than reaching back
    /// into the previous month.
    #[test]
    fn the_last_weekday_on_or_before_a_day_stays_inside_its_month_at_both_ends() {
        let run_form: &[Case] = &[
            (3, sun_before(31), 2026, Some(ymd(2026, 3, 29))),
            (2, sun_before(31), 2026, Some(ymd(2026, 2, 22))),
            (10, sun_before(31), 2026, Some(ymd(2026, 10, 25))),
            (2, sun_before(31), 2028, Some(ymd(2028, 2, 27))),
        ];
        check("the BYMONTHDAY run form", run_form);

        let bounds: &[Case] = &[
            (10, sun_before(25), 2026, Some(ymd(2026, 10, 25))),
            (10, sun_before(24), 2026, Some(ymd(2026, 10, 18))),
            (3, sun_before(3), 2026, Some(ymd(2026, 3, 1))),
            (3, before(Weekday::Monday, 1), 2026, None),
            (3, sun_before(0), 2026, None),
        ];
        check("a bound inside the month", bounds);
    }

    /// Fixed days of the month, which is the form a zone whose transitions follow a solar
    /// calendar rather than a weekday needs: Iran's daylight time, while it still kept any, ran
    /// between a day of March and a day of September rather than between weekdays of either.
    /// The rows are the arithmetic of that form and not a claim about which day a given year
    /// fell on, which for that zone is a question about the Persian calendar and not this one.
    #[test]
    fn a_fixed_day_of_the_month_is_the_day_or_it_is_nothing() {
        let iran: &[Case] = &[
            (3, day_of(21), 2026, Some(ymd(2026, 3, 21))),
            (9, day_of(21), 2026, Some(ymd(2026, 9, 21))),
        ];
        check("Asia/Tehran, a fixed day", iran);

        let absent: &[Case] = &[
            (4, day_of(31), 2026, None),
            (2, day_of(29), 2026, None),
            (2, day_of(29), 2028, Some(ymd(2028, 2, 29))),
            (3, day_of(0), 2026, None),
        ];
        check("a day the month may not have", absent);

        let ends: &[Case] = &[
            (4, RuleDay::LastDayOfMonth, 2026, Some(ymd(2026, 4, 30))),
            (2, RuleDay::LastDayOfMonth, 2026, Some(ymd(2026, 2, 28))),
            (2, RuleDay::LastDayOfMonth, 2028, Some(ymd(2028, 2, 29))),
        ];
        check("the last day of the month", ends);
    }

    /// A zone that changed its rules, which is the input the milestone named. `America/New_York`
    /// moved on the first Sunday in April and the last Sunday in October through 2006, and on
    /// the second Sunday in March and the first in November from 2007. The old rule has to stop
    /// answering rather than keep producing plausible April dates forever.
    #[test]
    fn a_rule_that_stopped_answers_nothing_after_the_year_it_stopped_in() {
        let ended = ymd(2006, 10, 29);
        let spring = YearlyRule::new(4, sun(NthWeek::First), at_hour(2), Some(ended)).unwrap();
        assert_eq!(spring.occurrence_in(2006), Some(ymd(2006, 4, 2)));
        assert_eq!(spring.occurrence_in(2007), None);
        assert!(spring.applies_in(2006));
        assert!(!spring.applies_in(2007));
        assert_eq!(spring.through(), Some(ended));

        let fall = YearlyRule::new(10, sun(NthWeek::Last), at_hour(2), Some(ended)).unwrap();
        assert_eq!(
            fall.occurrence_in(2006),
            Some(ended),
            "an UNTIL date is the last date the rule still names"
        );

        let replacement = YearlyRule::new(3, sun(NthWeek::Second), at_hour(2), None).unwrap();
        assert_eq!(replacement.occurrence_in(2007), Some(ymd(2007, 3, 11)));
    }

    /// The window question and the date question are different, and a caller reading coverage
    /// needs the first without the second answering for it. A November rule whose `UNTIL` falls
    /// in October is in force during 2006 and names nothing in it; a rule with no end is in
    /// force in every year and still names nothing in a month without a fifth Sunday.
    #[test]
    fn a_rule_in_force_during_a_year_may_still_name_no_date_in_it() {
        let ended = ymd(2006, 10, 29);
        let late = YearlyRule::new(11, sun(NthWeek::First), at_hour(2), Some(ended)).unwrap();
        assert!(late.applies_in(2006), "in force for most of 2006");
        assert_eq!(
            late.occurrence_in(2006),
            None,
            "and naming nothing in a month that began after it had stopped"
        );

        let endless = YearlyRule::new(10, sun(NthWeek::Fifth), at_hour(2), None).unwrap();
        assert!(endless.applies_in(2026));
        assert_eq!(endless.occurrence_in(2026), None);
        assert!(
            endless.applies_in(9999),
            "a rule with no UNTIL knows every year"
        );
    }

    /// The two local times the resolver has to answer for are named here and nowhere else.
    /// `America/New_York`'s clocks jump from 02:00 to 03:00 on 2026-03-08, so 02:30 that morning
    /// names no instant, and they fall back from 02:00 to 01:00 on 2026-11-01, so 01:30 that
    /// morning names two. Both wall clocks come out of the observance, read against
    /// `TZOFFSETFROM` as RFC 5545 section 3.6.5 requires.
    #[test]
    fn the_days_a_local_time_names_two_instants_and_none_come_out_of_these_rules() {
        let eastern = UtcOffset::from_seconds(-18_000).unwrap();
        let daylight = UtcOffset::from_seconds(-14_400).unwrap();
        let spring = YearlyRule::new(3, sun(NthWeek::Second), at_hour(2), None).unwrap();
        let fall = YearlyRule::new(11, sun(NthWeek::First), at_hour(2), None).unwrap();
        let gap = Observance::new(stamp(2007, 3, 11, 2), eastern, daylight, true, Some(spring));
        let fold = Observance::new(stamp(2007, 11, 4, 2), daylight, eastern, false, Some(fall));

        assert_eq!(gap.transition_in(2026), Some(stamp(2026, 3, 8, 2)));
        assert_eq!(fold.transition_in(2026), Some(stamp(2026, 11, 1, 2)));
        assert!(gap.moves_the_clock() && fold.moves_the_clock());
        assert_eq!(
            gap.transition_in(2007),
            Some(stamp(2007, 3, 11, 2)),
            "the anchor year is the rule's first, and the two agree there"
        );
        assert_eq!(
            gap.transition_in(2006),
            None,
            "a recurrence set begins at its DTSTART and names nothing before it"
        );
    }

    /// A table that ends before the question asked of it, seen from this unit: an `RDATE`
    /// transition is one transition in one year, and every other year gets a plain `None`
    /// rather than the final state quietly continuing under another name.
    #[test]
    fn a_date_driven_observance_transitions_in_its_own_year_and_in_no_other() {
        let winter = UtcOffset::from_seconds(3600).unwrap();
        let summer = UtcOffset::from_seconds(7200).unwrap();
        let dated = Observance::new(stamp(2029, 3, 25, 2), winter, summer, true, None);

        assert_eq!(dated.transition_in(2029), Some(stamp(2029, 3, 25, 2)));
        assert_eq!(dated.transition_in(2028), None);
        assert_eq!(
            dated.transition_in(2035),
            None,
            "a zone whose dates stop in 2029 has nothing to say about 2035"
        );
        assert_eq!(dated.covered_through(), Some(ymd(2029, 3, 25)));
    }

    /// A rule whose evaluated date would fall before the `DTSTART` it is anchored at yields the
    /// `DTSTART` itself, because that onset is real and dropping it would lose a transition the
    /// file wrote. Only the anchor year can be in that position.
    #[test]
    fn an_anchor_the_rule_would_precede_is_still_a_transition_in_its_own_year() {
        let winter = UtcOffset::from_seconds(3600).unwrap();
        let summer = UtcOffset::from_seconds(7200).unwrap();
        let rule = YearlyRule::new(3, sun(NthWeek::Last), at_hour(2), None).unwrap();
        let late = Observance::new(stamp(2007, 3, 30, 2), winter, summer, true, Some(rule));

        assert_eq!(
            late.transition_in(2007),
            Some(stamp(2007, 3, 30, 2)),
            "the rule's own 2007 date is the 25th, which precedes this DTSTART"
        );
        assert_eq!(late.transition_in(2026), Some(stamp(2026, 3, 29, 2)));
        assert_eq!(late.transition_in(2006), None);
    }
}
