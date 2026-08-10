// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 4 — reading a `VTIMEZONE` out of `ical_core::Component`.
//!
//! Specification: RFC 5545 section 3.6.5, with section 3.8.3.3 and section 3.8.3.4 for the
//! offsets and section 3.8.5.2 and section 3.8.5.3 for the two transition forms.
//!
//! Owed by this unit and by nothing else. The trait impl needs no re-export line; the free
//! function needs one appended to the crate root's block:
//!
//! ```text
//! impl ObservanceReader for ical_core::Component { .. }
//! pub fn read_calendar_zones<S: DiagnosticSink + ?Sized>(
//!     calendar: &Component, meter: &mut Meter, sink: &mut S,
//! ) -> VtimezoneSet;
//! ```
//!
//! The one place untrusted zone text becomes a model, and therefore the only place in this
//! crate that charges a meter. Each `STANDARD` and `DAYLIGHT` subcomponent becomes one
//! `Observance` for its own `DTSTART`, one more for each `RDATE` value it carries, and a
//! `YearlyRule` where its `RRULE` matches a `RuleDay` form. A definition carrying both forms
//! carries both, which is ordinary rather than a conflict: the rule states the cadence and the
//! dates state the exceptions to it.
//!
//! The `TZID` is taken as written, `DQUOTE`s removed and nothing else — no case folding, no
//! prefix stripping, no alias lookup. `docs/adr/0003` makes mapping a vendor identifier onto an
//! IANA one the caller's visible step, and a reader that quietly normalized here would take it
//! back.
//!
//! Codes this unit owns, and no other unit may emit:
//!
//! - `vtimezone-without-observance` — section 3.6.5 requires at least one and this file has
//!   none. The component is kept and the zone answers nothing.
//! - `vtimezone-rule-unsupported` — an observance `RRULE` outside the closed-form vocabulary. A
//!   note, because the file is legal and what is missing is here: the observance's own
//!   `DTSTART` still stands as one transition, so the answer is smaller rather than wrong.
//! - `missing-time-zone-definition` — a `TZID` parameter naming no `VTIMEZONE` in the same
//!   calendar.
//! - `duplicate-time-zone-identifier` — read off `ZoneSetError::diagnostic_code` when
//!   `VtimezoneSet::insert` hands a definition back.
//!
//! `vtimezone-observances-truncated` is emitted by `TransitionTable::new`, which this unit
//! calls and does not reimplement. That is deliberate: the truncation point and the code
//! reporting it are one decision, and two callers deciding it separately is how they drift.
//!
//! # What a value this unit cannot read costs
//!
//! An observance with no readable `TZOFFSETFROM` or `TZOFFSETTO`, a `DTSTART` or `RDATE` entry
//! that is a `DATE` rather than a clock, and a `VTIMEZONE` whose `TZID` is absent, empty,
//! declared twice or not UTF-8 all contribute nothing here and no code of their own. Section
//! 3.6's own reading of a missing or unreadable required property is [`Component::audit`]'s,
//! reported under `missing-required-property`, and a second reading of it written here would be
//! a second place for that answer to live and a second place for the two to disagree. What this
//! unit reports is what it is the only place to notice: a definition with no observance at all,
//! a rule it declines to evaluate, an identifier no definition backs, and a definition arriving
//! twice.
//!
//! # Bounds
//!
//! Two dimensions, charged where `docs/adr/0010` puts them and nowhere else: the observances by
//! [`TransitionTable::new`], the zone count by [`VtimezoneSet::insert`]. The list handed to the
//! former is built whole first, and that is bounded rather than unbounded — an observance costs
//! at least sixteen octets of input, and those octets were charged against this same ledger by
//! the parse that produced the tree being read. Cutting the list short before the table sees it
//! would move the truncation point off the sorted order the table truncates in, which is how a
//! table acquires a hole in the middle instead of an earlier end.
//!
//! The walk looking for identifiers nothing defines keeps its own worklist rather than
//! recursing, so a calendar nested as deeply as the parse admits costs heap rather than stack.
//!
//! [`Component::audit`]: ical_core::Component::audit
//! [`TransitionTable::new`]: crate::TransitionTable::new
//! [`VtimezoneSet::insert`]: crate::VtimezoneSet::insert

use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::str;

use ical_core::{
    CivilDate, CivilDateTime, Component, DateTimeValue, DecodeValue as _, Diagnostic,
    DiagnosticCode, DiagnosticSink, Location, Meter, PropertyId, Severity, UtcOffset, Weekday,
    report_diagnostic,
};

use crate::model::{
    NthWeek, Observance, ObservanceReader, RuleDay, TransitionTable, VtimezoneSet, YearlyRule,
};

/// The most `BYMONTHDAY` values a day form is read from, which is a month's length.
///
/// A rule naming more days than a month has is outside this vocabulary whatever else it says,
/// so the ceiling doubles as the refusal and no buffer on this path ever grows.
const MAX_MONTH_DAYS: usize = 31;

/// How many `BYMONTHDAY` values the window forms are written with.
///
/// Seven, because the shape means "whichever day of one week falls in this window" and a week
/// is seven days long. A shorter or longer run says something else, and this crate does not
/// guess what.
const RUN_LENGTH: usize = 7;

/// Read every `VTIMEZONE` a calendar carries, and report the identifiers nothing defines.
///
/// The zones come back in a set keyed by exact bytes. A second definition of one identifier is
/// reported under [`DiagnosticCode::DuplicateTimeZoneIdentifier`] and handed back rather than
/// preferred, and a `TZID` parameter anywhere in the calendar naming no definition here is
/// reported once under [`DiagnosticCode::MissingTimeZoneDefinition`].
///
/// Both bounds `docs/adr/0010` gives a zone are charged against `meter` on the way through. A
/// calendar declaring more zones than the caller's policy admits keeps the ones that fit; that
/// refusal carries no code of its own, because the golden list of diagnostic codes has none for
/// it, and a caller that needs to know compares the set's length against what it expected.
#[must_use]
pub fn read_calendar_zones<S: DiagnosticSink + ?Sized>(
    calendar: &Component,
    meter: &mut Meter,
    sink: &mut S,
) -> VtimezoneSet {
    let mut zones = VtimezoneSet::new();
    for component in calendar.components() {
        if !component.is_named(b"VTIMEZONE") {
            continue;
        }
        let mut observances = Vec::new();
        let Some(tzid) = read_definition(component, meter, sink, &mut observances) else {
            continue;
        };
        let table = TransitionTable::new(tzid, observances, meter, sink);
        if let Err(refused) = zones.insert(table, meter) {
            if let Some(code) = refused.diagnostic_code() {
                report(sink, meter, code, Severity::Violation);
            }
        }
    }
    report_undefined(calendar, &zones, meter, sink);
    zones
}

impl ObservanceReader for Component {
    fn read_vtimezone(
        &self,
        meter: &mut Meter,
        sink: &mut dyn DiagnosticSink,
        out: &mut Vec<Observance>,
    ) -> Option<Box<str>> {
        read_definition(self, meter, sink, out)
    }
}

/// Read one `VTIMEZONE` into `out`, answering with its identifier.
///
/// Generic over the sink rather than taking the trait object [`ObservanceReader`] declares, so
/// that the free function above does not pay for an erasure it has no use for. `dyn
/// DiagnosticSink` is itself a legal `S`, which is what lets the trait impl be one call.
fn read_definition<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut Vec<Observance>,
) -> Option<Box<str>> {
    let tzid = identifier(component)?;
    let mut declared = 0_usize;
    for inner in component.components() {
        if !inner.is_named(b"STANDARD") && !inner.is_named(b"DAYLIGHT") {
            continue;
        }
        declared = declared.saturating_add(1);
        read_observance(inner, meter, sink, out);
    }
    // Counted over the subcomponents rather than over what they yielded. The code says "carried
    // neither a STANDARD nor a DAYLIGHT subcomponent"; a definition whose one observance was
    // unreadable did carry one, and reporting that under this code would put a second meaning
    // on a frozen one.
    if declared == 0 {
        report(
            sink,
            meter,
            DiagnosticCode::VtimezoneWithoutObservance,
            Severity::Violation,
        );
    }
    Some(tzid)
}

/// The identifier a `VTIMEZONE` answers to, or `None` when it has none this crate can file.
///
/// `None` for an absent, empty, repeated or non-UTF-8 `TZID`. The last is forced rather than
/// chosen: [`ObservanceReader::read_vtimezone`] answers with a `Box<str>`, and octets that are
/// not text cannot become one. The repeated case is `None` for the reason
/// [`ical_core::Component::get`] refuses a duplicate — two identities is not one of them, and
/// picking the first would file a zone under a name the file does not unambiguously give it.
fn identifier(component: &Component) -> Option<Box<str>> {
    let wanted = PropertyId::TZID;
    let mut written = component.properties_named(&wanted);
    let first = written.next()?;
    if written.next().is_some() {
        return None;
    }
    let text = str::from_utf8(unquoted(first.value_text().as_bytes())).ok()?;
    (!text.is_empty()).then(|| Box::from(text))
}

/// `text` with a matched RFC 5545 section 3.2 `DQUOTE` pair removed, and nothing else done.
///
/// The rule [`ical_core::Parameter::unquoted`] applies on the other side of the comparison. The
/// two sides have to strip alike, or a producer that quoted one of them would declare a zone
/// that no lookup could ever reach.
fn unquoted(text: &[u8]) -> &[u8] {
    text.strip_prefix(b"\"")
        .and_then(|inside| inside.strip_suffix(b"\""))
        .unwrap_or(text)
}

/// Read one `STANDARD` or `DAYLIGHT` subcomponent into `out`.
fn read_observance<S: DiagnosticSink + ?Sized>(
    component: &Component,
    meter: &mut Meter,
    sink: &mut S,
    out: &mut Vec<Observance>,
) {
    // The classification is the subcomponent's own name, per RFC 5545 section 3.6.5, and never
    // a comparison of the two offsets: a zone whose daylight offset is the smaller of the pair
    // exists, and inferring the flag from arithmetic gets it backwards there.
    let daylight = component.is_named(b"DAYLIGHT");
    let Some(offset_from) = component
        .get::<UtcOffset>(&PropertyId::TZOFFSETFROM)
        .value()
    else {
        return;
    };
    let Some(offset_to) = component.get::<UtcOffset>(&PropertyId::TZOFFSETTO).value() else {
        return;
    };
    // The rule rides on the `DTSTART` observance and on no other: an `RDATE` names one
    // transition and states no cadence, and repeating the rule on each explicit date would
    // declare the same yearly series once per date.
    if let Some(start) = component
        .dtstart()
        .value()
        .and_then(|value| wall_clock(value, offset_from))
    {
        let rule = read_rule(component, start, offset_from, meter, sink);
        out.push(Observance::new(
            start,
            offset_from,
            offset_to,
            daylight,
            rule,
        ));
    }
    push_dated_transitions(component, offset_from, offset_to, daylight, out);
}

/// Append one observance per `RDATE` value of `component`.
///
/// An `RDATE` is a list per line and may arrive on several lines, and RFC 5545 section 3.8.5.2
/// puts no order on either, which is why [`TransitionTable::new`] sorts what it is handed.
/// Entries this crate cannot read as a wall clock yield nothing; see the module note.
fn push_dated_transitions(
    component: &Component,
    offset_from: UtcOffset,
    offset_to: UtcOffset,
    daylight: bool,
    out: &mut Vec<Observance>,
) {
    let wanted = PropertyId::RDATE;
    for property in component.properties_named(&wanted) {
        for field in property
            .value_text()
            .as_bytes()
            .split(|octet| *octet == b',')
        {
            let Ok(value) = DateTimeValue::decode_value(field) else {
                continue;
            };
            let Some(start) = wall_clock(value, offset_from) else {
                continue;
            };
            out.push(Observance::new(
                start,
                offset_from,
                offset_to,
                daylight,
                None,
            ));
        }
    }
}

/// The wall clock `value` names on the clock running before a transition.
///
/// RFC 5545 section 3.6.5 writes an observance's `DTSTART` and `RDATE` as local times read
/// against `TZOFFSETFROM`, so the ordinary case is the value's own fields taken as written. A
/// value carrying `Z` violates that and still names a real instant, and the reading that
/// recovers the producer's intent is that instant on this observance's own clock. A `DATE`
/// carries no clock at all, and there is no hour to read out of one for a transition to happen
/// at.
fn wall_clock(value: DateTimeValue<'_>, offset_from: UtcOffset) -> Option<CivilDateTime> {
    match value {
        DateTimeValue::Local(stamp) | DateTimeValue::Zoned { stamp, .. } => Some(stamp),
        DateTimeValue::Utc(stamp) => {
            let moment = stamp.at_offset(UtcOffset::UTC)?;
            CivilDateTime::from_instant(moment, offset_from)
        },
        DateTimeValue::Date(_) => None,
    }
}

/// The rule repeating an observance, reporting the ones this crate declines to evaluate.
///
/// A second `RRULE` on one observance is outside the vocabulary rather than a rule to choose
/// between: RFC 5545 section 3.6.5 gives an observance one cadence, and deciding which of two
/// it is would be exactly the silent choice this crate is arranged against.
fn read_rule<S: DiagnosticSink + ?Sized>(
    component: &Component,
    start: CivilDateTime,
    offset_from: UtcOffset,
    meter: &mut Meter,
    sink: &mut S,
) -> Option<YearlyRule> {
    let wanted = PropertyId::RRULE;
    let mut written = component.properties_named(&wanted);
    let first = written.next()?;
    let rule = if written.next().is_some() {
        None
    } else {
        parse_rule(first.value_text().as_bytes(), start, offset_from)
    };
    if rule.is_none() {
        report(
            sink,
            meter,
            DiagnosticCode::VtimezoneRuleUnsupported,
            Severity::Note,
        );
    }
    rule
}

/// One observance `RRULE`, read as the closed form or not at all.
///
/// The month and the day fall back to `DTSTART`'s own, which is what RFC 5545 section 3.3.10
/// says an unstated `BYMONTH` or `BYMONTHDAY` means, and the transition time is `DTSTART`'s
/// because section 3.6.5 gives an observance no other place to write one.
fn parse_rule(text: &[u8], start: CivilDateTime, offset_from: UtcOffset) -> Option<YearlyRule> {
    let parts = collect_parts(text, offset_from)?;
    let month = match parts.month {
        Some(stated) => stated,
        None => start.date().month(),
    };
    let day = day_form(parts, start.date())?;
    YearlyRule::new(month, day, start.time(), parts.until)
}

/// What a `RECUR` value said, as far as this crate reads one.
#[derive(Clone, Copy, Debug)]
struct RuleParts {
    /// Whether `FREQ=YEARLY` was stated. Nothing else is a transition rule.
    yearly: bool,
    /// The one `BYMONTH` value, absent when the rule states none.
    month: Option<u8>,
    /// The one `BYDAY` entry, absent when the rule states none.
    weekday: Option<WeekdayTerm>,
    /// The `BYMONTHDAY` values, in the order they were written.
    month_days: MonthDays,
    /// The last date the rule applies to, from `UNTIL`.
    until: Option<CivilDate>,
}

impl RuleParts {
    /// A reading with nothing in it yet.
    const EMPTY: Self = Self {
        yearly: false,
        month: None,
        weekday: None,
        month_days: MonthDays::EMPTY,
        until: None,
    };
}

/// One `BYDAY` entry: an optional ordinal and the weekday it counts.
#[derive(Clone, Copy, Debug)]
struct WeekdayTerm {
    /// Which occurrence, as `2SU` and `-1SU` write one; absent for a bare `SU`.
    ordinal: Option<i8>,
    /// The weekday.
    weekday: Weekday,
}

/// The `BYMONTHDAY` values of one rule, in a buffer that cannot grow.
#[derive(Clone, Copy, Debug)]
struct MonthDays {
    /// The values as written, of which the first `count` are live.
    values: [i8; MAX_MONTH_DAYS],
    /// How many are live.
    count: usize,
}

impl MonthDays {
    /// No values at all.
    const EMPTY: Self = Self {
        values: [0; MAX_MONTH_DAYS],
        count: 0,
    };

    /// Keep `day`, or answer `false` because there is no room and therefore no reading.
    fn push(&mut self, day: i8) -> bool {
        let Some(slot) = self.values.get_mut(self.count) else {
            return false;
        };
        *slot = day;
        self.count = self.count.saturating_add(1);
        true
    }

    /// The lowest and highest value, present only when they and everything between are here.
    ///
    /// Sorted rather than assumed sorted: a producer writing `31,25,26,27,28,29,30` wrote the
    /// same set as one writing them in order, and what the window forms name is the set.
    fn contiguous_run(mut self) -> Option<(i8, i8)> {
        let used = self.values.get_mut(..self.count)?;
        used.sort_unstable();
        let low = *used.first()?;
        let mut highest = low;
        for value in used.iter().skip(1) {
            if *value != highest.checked_add(1)? {
                return None;
            }
            highest = *value;
        }
        Some((low, highest))
    }
}

/// Read every `name=value` term of a `RECUR` value, or refuse the whole of it.
///
/// Refusing at the first unrecognized term rather than reading around it is the difference
/// between a rule this crate evaluates and one it half evaluates: a `BYSETPOS` nobody read
/// changes which day the rule names, and a transition on the wrong day is worse than none.
fn collect_parts(text: &[u8], offset_from: UtcOffset) -> Option<RuleParts> {
    let mut parts = RuleParts::EMPTY;
    for term in text.split(|octet| *octet == b';') {
        let equals = term.iter().position(|octet| *octet == b'=')?;
        let (name, rest) = term.split_at_checked(equals)?;
        let value = rest.get(1..)?;
        if !take_part(&mut parts, name, value, offset_from) {
            return None;
        }
    }
    parts.yearly.then_some(parts)
}

/// Read one `RECUR` term into `parts`, answering whether it is one this crate evaluates.
///
/// `WKST` is accepted and ignored because it can only change a `BYWEEKNO` reading, and a rule
/// carrying `BYWEEKNO` falls off the end of this function anyway.
fn take_part(parts: &mut RuleParts, name: &[u8], value: &[u8], offset_from: UtcOffset) -> bool {
    if name.eq_ignore_ascii_case(b"FREQ") {
        parts.yearly = value.eq_ignore_ascii_case(b"YEARLY");
        return parts.yearly;
    }
    if name.eq_ignore_ascii_case(b"INTERVAL") {
        return value == b"1";
    }
    if name.eq_ignore_ascii_case(b"WKST") {
        return true;
    }
    if name.eq_ignore_ascii_case(b"BYMONTH") {
        parts.month = month_number(value);
        return parts.month.is_some();
    }
    if name.eq_ignore_ascii_case(b"BYDAY") {
        parts.weekday = weekday_term(value);
        return parts.weekday.is_some();
    }
    if name.eq_ignore_ascii_case(b"BYMONTHDAY") {
        return read_month_days(value, &mut parts.month_days);
    }
    if name.eq_ignore_ascii_case(b"UNTIL") {
        parts.until = until_date(value, offset_from);
        return parts.until.is_some();
    }
    false
}

/// Read a `BYMONTHDAY` list into `out`.
fn read_month_days(value: &[u8], out: &mut MonthDays) -> bool {
    for field in value.split(|octet| *octet == b',') {
        let Some(day) = month_day_number(field) else {
            return false;
        };
        if !out.push(day) {
            return false;
        }
    }
    out.count != 0
}

/// Which day of its month a rule names, or `None` for a shape this crate does not evaluate.
fn day_form(parts: RuleParts, start: CivilDate) -> Option<RuleDay> {
    match (parts.weekday, parts.month_days.count) {
        (Some(term), 0) => nth_form(term),
        (Some(term), _) => window_form(term, parts.month_days),
        // No day part at all: RFC 5545 section 3.3.10 takes the day from `DTSTART`. A rule
        // anchored on the 29th of February names nothing in three years of four, which the
        // evaluation answers `None` to rather than moving to a nearby date.
        (None, 0) => Some(RuleDay::DayOfMonth(start.day())),
        (None, 1) => fixed_form(parts.month_days),
        (None, _) => None,
    }
}

/// `BYDAY=2SU` and `BYDAY=-1SU`, which name one weekday of the month outright.
fn nth_form(term: WeekdayTerm) -> Option<RuleDay> {
    let week = match term.ordinal? {
        1 => NthWeek::First,
        2 => NthWeek::Second,
        3 => NthWeek::Third,
        4 => NthWeek::Fourth,
        5 => NthWeek::Fifth,
        -1 => NthWeek::Last,
        // `-2SU` and the rest are legal `RECUR` and name no transition any zone database
        // generates, so they are a reported gap rather than a form guessed at.
        _ => return None,
    };
    Some(RuleDay::Nth {
        weekday: term.weekday,
        week,
    })
}

/// `BYDAY=SU;BYMONTHDAY=8,..,14` and its two siblings, a weekday inside a window of a week.
///
/// A window of seven consecutive days holds exactly one of each weekday, so the pair names one
/// date. A window ending at the last day a month can have is read as [`RuleDay::OnOrBefore`]
/// rather than [`RuleDay::OnOrAfter`], which is the spelling the model documents for it; the
/// two agree in every month, because a window a short month clips is clipped at that same end
/// either way. A window of negative days ending at `-1` is the last such weekday, which
/// [`NthWeek::Last`] already says exactly and in every month length.
fn window_form(term: WeekdayTerm, month_days: MonthDays) -> Option<RuleDay> {
    if term.ordinal.is_some() || month_days.count != RUN_LENGTH {
        return None;
    }
    let (low, highest) = month_days.contiguous_run()?;
    if highest == -1 {
        return Some(RuleDay::Nth {
            weekday: term.weekday,
            week: NthWeek::Last,
        });
    }
    if low < 1 {
        return None;
    }
    if highest == 31 {
        return Some(RuleDay::OnOrBefore {
            weekday: term.weekday,
            day: 31,
        });
    }
    Some(RuleDay::OnOrAfter {
        weekday: term.weekday,
        day: u8::try_from(low).ok()?,
    })
}

/// `BYMONTHDAY=15` and `BYMONTHDAY=-1` with no `BYDAY` beside them.
fn fixed_form(month_days: MonthDays) -> Option<RuleDay> {
    let (day, _) = month_days.contiguous_run()?;
    if day == -1 {
        return Some(RuleDay::LastDayOfMonth);
    }
    // Only the last day counts backwards. `-2` names the day before the last one, which the
    // model has no form for and which no zone definition writes.
    u8::try_from(day).ok().map(RuleDay::DayOfMonth)
}

/// The last date an `UNTIL` keeps a rule alive, on the observance's own clock.
///
/// RFC 5545 section 3.3.10 requires `UNTIL` to be UTC and section 3.6.5 evaluates an observance
/// in wall-clock terms, so the two have to be reconciled somewhere. Here, once, against
/// `TZOFFSETFROM` — the clock still running when the transition arrives. A rule whose end
/// cannot be placed on that clock is one this crate does not evaluate, rather than one whose
/// end is quietly dropped and which therefore runs forever.
fn until_date(value: &[u8], offset_from: UtcOffset) -> Option<CivilDate> {
    let written = DateTimeValue::decode_value(value).ok()?;
    match written {
        DateTimeValue::Date(date) => Some(date),
        DateTimeValue::Local(_) | DateTimeValue::Utc(_) | DateTimeValue::Zoned { .. } => {
            wall_clock(written, offset_from).map(CivilDateTime::date)
        },
    }
}

/// One `BYMONTH` value, `1` through `12`.
fn month_number(value: &[u8]) -> Option<u8> {
    let number = signed_number(value)?;
    if !(1..=12).contains(&number) {
        return None;
    }
    u8::try_from(number).ok()
}

/// One `BYMONTHDAY` value, `1` through `31` or `-31` through `-1`.
fn month_day_number(value: &[u8]) -> Option<i8> {
    let number = signed_number(value)?;
    (-31..=31).contains(&number).then_some(number)
}

/// One `BYDAY` entry.
///
/// A list — `BYDAY=SA,SU` — is refused here rather than by a check of its own: whatever precedes
/// the two-letter name has to be a number, and `SA,` is not one.
fn weekday_term(value: &[u8]) -> Option<WeekdayTerm> {
    let split = value.len().checked_sub(2)?;
    let (prefix, name) = value.split_at_checked(split)?;
    let weekday = weekday_of(name)?;
    if prefix.is_empty() {
        return Some(WeekdayTerm {
            ordinal: None,
            weekday,
        });
    }
    Some(WeekdayTerm {
        ordinal: Some(signed_number(prefix)?),
        weekday,
    })
}

/// The weekday RFC 5545 section 3.3.10 spells `name`, compared as that section compares one.
fn weekday_of(name: &[u8]) -> Option<Weekday> {
    Weekday::ALL
        .into_iter()
        .find(|day| name.eq_ignore_ascii_case(day.as_bytes()))
}

/// One optionally signed one- or two-digit number, never zero.
///
/// Zero is refused because every part this reads for excludes it: RFC 5545 section 3.3.10 has
/// no month zero, no month day zero and no `0SU`.
fn signed_number(bytes: &[u8]) -> Option<i8> {
    let (negative, digits) = match bytes.split_first() {
        Some((&b'-', tail)) => (true, tail),
        Some((&b'+', tail)) => (false, tail),
        _ => (false, bytes),
    };
    if digits.is_empty() || digits.len() > 2 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut total: i8 = 0;
    for octet in digits {
        let place = i8::try_from(octet.wrapping_sub(b'0')).ok()?;
        total = total.checked_mul(10)?.checked_add(place)?;
    }
    if total == 0 {
        return None;
    }
    if negative {
        total.checked_neg()
    } else {
        Some(total)
    }
}

/// Report each identifier a `TZID` parameter names that no definition in `zones` backs.
///
/// Once per identifier rather than once per property. A [`Diagnostic`] carries a code, a
/// severity and a location, and an [`ical_core::Property`] owns fresh unfolded octets rather
/// than the offsets they were read from, so there is no span to tell two reports of one zone
/// apart by — which makes the second report of it carry nothing the first did not.
fn report_undefined<S: DiagnosticSink + ?Sized>(
    calendar: &Component,
    zones: &VtimezoneSet,
    meter: &mut Meter,
    sink: &mut S,
) {
    let mut undefined: BTreeSet<&[u8]> = BTreeSet::new();
    let mut pending: Vec<&Component> = Vec::new();
    pending.push(calendar);
    while let Some(component) = pending.pop() {
        for property in component.properties() {
            for parameter in property.parameters_named(b"TZID") {
                let named = parameter.unquoted();
                // An empty `TZID` names no zone, which is the reading `ical-core` already gives
                // it when it declines to make such a value a zoned one.
                if !named.is_empty() && !is_defined(zones, named) {
                    undefined.insert(named);
                }
            }
        }
        pending.extend(component.components());
    }
    for _ in &undefined {
        report(
            sink,
            meter,
            DiagnosticCode::MissingTimeZoneDefinition,
            Severity::Violation,
        );
    }
}

/// Whether `named` is an identifier one of `zones` answers to, compared by exact bytes.
///
/// Octets that are not UTF-8 match nothing, because a definition's identifier is text. That is
/// the refusal [`identifier`] makes at the other end, and for the same reason.
fn is_defined(zones: &VtimezoneSet, named: &[u8]) -> bool {
    str::from_utf8(named).is_ok_and(|text| zones.table(text).is_some())
}

/// Offer one diagnostic about this calendar, charging a refusal to `meter`.
///
/// The location is [`Location::NOWHERE`] throughout, and that is a statement rather than an
/// omission: what this unit reads is a tree of properties owning fresh octets, so any span it
/// produced would address a buffer the caller never handed in.
fn report<S: DiagnosticSink + ?Sized>(
    sink: &mut S,
    meter: &mut Meter,
    code: DiagnosticCode,
    severity: Severity,
) {
    report_diagnostic(
        sink,
        meter,
        Diagnostic::new(code, severity, Location::NOWHERE),
    );
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Component, Diagnostic, DiagnosticCode, Document,
        IgnoreDiagnostics, Limits, Meter, Severity, UtcOffset, Weekday,
    };

    use super::read_calendar_zones;
    use crate::model::{NthWeek, Observance, ObservanceReader, RuleDay, VtimezoneSet, YearlyRule};

    /// `America/New_York` as every major client exports it since the 2007 rule change.
    const NEW_YORK: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
TZNAME:EDT
DTSTART:20070311T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
TZNAME:EST
DTSTART:20071104T020000
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU
END:STANDARD
END:VTIMEZONE
";

    /// Europe/Berlin, whose two transitions both happen at 01:00 UTC and are written at
    /// different local hours because of it.
    const BERLIN: &str = "\
BEGIN:VTIMEZONE
TZID:Europe/Berlin
BEGIN:DAYLIGHT
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
TZNAME:CEST
DTSTART:19700329T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:+0200
TZOFFSETTO:+0100
TZNAME:CET
DTSTART:19701025T030000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU
END:STANDARD
END:VTIMEZONE
";

    /// `Australia/Lord_Howe`, whose daylight saving moves the clock by thirty minutes.
    const LORD_HOWE: &str = "\
BEGIN:VTIMEZONE
TZID:Australia/Lord_Howe
BEGIN:STANDARD
TZOFFSETFROM:+1100
TZOFFSETTO:+1030
DTSTART:20270404T020000
RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=1SU
END:STANDARD
BEGIN:DAYLIGHT
TZOFFSETFROM:+1030
TZOFFSETTO:+1100
DTSTART:20271003T020000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=1SU
END:DAYLIGHT
END:VTIMEZONE
";

    /// `America/New_York` written as an explicit table of dates that stops at the end of 2029.
    ///
    /// The dates are the ones the United States rule actually produces: the second Sunday in
    /// March and the first Sunday in November of 2027, 2028 and 2029.
    const NEW_YORK_DATED: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:20270314T020000
RDATE:20280312T020000,20290311T020000
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:20271107T020000
RDATE:20281105T020000,20291104T020000
END:STANDARD
END:VTIMEZONE
";

    /// `America/New_York` with the rules from before the 2007 change beside the ones after it,
    /// which is the shape Lightning and libical write for a zone whose government moved it.
    const NEW_YORK_BOTH_RULES: &str = "\
BEGIN:VTIMEZONE
TZID:America/New_York
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:19671029T020000
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;UNTIL=20061029T060000Z
END:STANDARD
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:19870405T020000
RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=1SU;UNTIL=20060402T070000Z
END:DAYLIGHT
BEGIN:DAYLIGHT
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
DTSTART:20070311T020000
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU
END:DAYLIGHT
BEGIN:STANDARD
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
DTSTART:20071104T020000
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU
END:STANDARD
END:VTIMEZONE
";

    /// One observance as the real zone's published rules describe it.
    #[derive(Clone, Copy, Debug)]
    struct Expected {
        /// The wall clock the transition begins at, read against `offset_from`.
        start: CivilDateTime,
        /// Seconds east of UTC before it.
        offset_from: i32,
        /// Seconds east of UTC from it.
        offset_to: i32,
        /// Whether this is the zone's daylight observance.
        daylight: bool,
        /// Which day of its month the rule names.
        day: RuleDay,
    }

    /// The fixture text with the terminators RFC 5545 section 3.1 requires.
    ///
    /// The fixtures are written with bare line feeds above so they read as the files they were
    /// copied from, and converted here so the reader is handed conforming octets.
    fn crlf(text: &str) -> String {
        let mut out = String::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            out.push_str(line);
            out.push_str("\r\n");
        }
        out
    }

    /// One calendar wrapping `body`.
    fn calendar_text(body: &str) -> String {
        let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//EN\r\n");
        out.push_str(&crlf(body));
        out.push_str("END:VCALENDAR\r\n");
        out
    }

    /// Read `body` as a calendar under `limits`, answering with the zones and what was said.
    fn read_under(body: &str, limits: Limits) -> (VtimezoneSet, Vec<Diagnostic>) {
        let text = calendar_text(body);
        let document = Document::parse(text.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let mut meter = Meter::new(limits);
        let mut reported = Vec::new();
        let zones = read_calendar_zones(calendar, &mut meter, &mut reported);
        (zones, reported)
    }

    /// Read `body` under the default policy.
    fn read(body: &str) -> (VtimezoneSet, Vec<Diagnostic>) {
        read_under(body, Limits::DEFAULT)
    }

    /// The observances of the one zone `body` declares under `tzid`.
    fn observances_of(body: &str, tzid: &str) -> Vec<Observance> {
        let (zones, _) = read(body);
        zones.table(tzid).unwrap().observances().to_vec()
    }

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().copied().map(Diagnostic::code).collect()
    }

    fn first_severity(reported: &[Diagnostic]) -> Option<Severity> {
        reported.first().copied().map(Diagnostic::severity)
    }

    /// The `BYDAY` form every one of these zones is written with.
    fn sunday(week: NthWeek) -> RuleDay {
        RuleDay::Nth {
            weekday: Weekday::Sunday,
            week,
        }
    }

    /// A minimal definition under `tzid`, for the tests that are about the identifier alone.
    fn definition(tzid: &str) -> String {
        format!(
            "BEGIN:VTIMEZONE\nTZID:{tzid}\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\n\
             TZOFFSETTO:+0100\nDTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n"
        )
    }

    /// A definition whose one observance starts on 25 March 2007 at 02:00 and carries `rule`.
    fn ruled(rule: &str) -> String {
        format!(
            "BEGIN:VTIMEZONE\nTZID:Z\nBEGIN:DAYLIGHT\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0200\n\
             DTSTART:20070325T020000\nRRULE:{rule}\nEND:DAYLIGHT\nEND:VTIMEZONE\n"
        )
    }

    /// Assert that `body` reads as the observances a real zone's rules describe.
    fn assert_reads_as(body: &str, tzid: &str, expected: &[Expected]) {
        let (zones, reported) = read(body);
        let table = zones.table(tzid).unwrap();
        assert!(codes(&reported).is_empty(), "{tzid}");
        assert_eq!(table.observances().len(), expected.len(), "{tzid}");
        assert_eq!(
            table.coverage_end(),
            None,
            "{tzid} repeats by rules with no UNTIL, so it knows the future"
        );
        for (observance, wanted) in table.observances().iter().zip(expected) {
            assert_eq!(observance.start(), wanted.start, "{tzid}");
            assert_eq!(
                observance.offset_from(),
                offset(wanted.offset_from),
                "{tzid}"
            );
            assert_eq!(observance.offset_to(), offset(wanted.offset_to), "{tzid}");
            assert_eq!(observance.daylight(), wanted.daylight, "{tzid}");
            let rule = observance.rule().unwrap();
            assert_eq!(rule.day(), wanted.day, "{tzid}");
            assert_eq!(rule.at(), wanted.start.time(), "{tzid}");
            assert_eq!(rule.through(), None, "{tzid}");
        }
    }

    /// How many instants `local` names across `transition`, counted the way a resolver must.
    ///
    /// A reading under an offset counts only where it lands on the side of the transition that
    /// offset actually governs. Unit 2 owns the real one; the definition is restated here so
    /// that what these tests assert is the model this unit built out of a real zone's published
    /// rules, rather than the resolver agreeing with itself.
    fn readings_of(transition: Observance, local: CivilDateTime) -> usize {
        let Some(moment) = transition.start().at_offset(transition.offset_from()) else {
            return 0;
        };
        let earlier = local
            .at_offset(transition.offset_from())
            .filter(|instant| *instant < moment);
        let later = local
            .at_offset(transition.offset_to())
            .filter(|instant| *instant >= moment);
        usize::from(earlier.is_some()).saturating_add(usize::from(later.is_some()))
    }

    #[test]
    fn the_united_states_rule_reads_as_the_two_sundays_it_names() {
        assert_reads_as(
            NEW_YORK,
            "America/New_York",
            &[
                Expected {
                    start: stamp(2007, 3, 11, 2, 0),
                    offset_from: -18_000,
                    offset_to: -14_400,
                    daylight: true,
                    day: sunday(NthWeek::Second),
                },
                Expected {
                    start: stamp(2007, 11, 4, 2, 0),
                    offset_from: -14_400,
                    offset_to: -18_000,
                    daylight: false,
                    day: sunday(NthWeek::First),
                },
            ],
        );
    }

    #[test]
    fn the_european_rule_reads_as_the_last_sundays_it_names() {
        assert_reads_as(
            BERLIN,
            "Europe/Berlin",
            &[
                Expected {
                    start: stamp(1970, 3, 29, 2, 0),
                    offset_from: 3_600,
                    offset_to: 7_200,
                    daylight: true,
                    day: sunday(NthWeek::Last),
                },
                Expected {
                    start: stamp(1970, 10, 25, 3, 0),
                    offset_from: 7_200,
                    offset_to: 3_600,
                    daylight: false,
                    day: sunday(NthWeek::Last),
                },
            ],
        );
    }

    /// A zone whose daylight saving is half an hour, which a reader that assumed an hour reads
    /// as an offset nobody wrote.
    #[test]
    fn a_zone_whose_daylight_saving_is_half_an_hour_reads_as_half_an_hour() {
        assert_reads_as(
            LORD_HOWE,
            "Australia/Lord_Howe",
            &[
                Expected {
                    start: stamp(2027, 4, 4, 2, 0),
                    offset_from: 39_600,
                    offset_to: 37_800,
                    daylight: false,
                    day: sunday(NthWeek::First),
                },
                Expected {
                    start: stamp(2027, 10, 3, 2, 0),
                    offset_from: 37_800,
                    offset_to: 39_600,
                    daylight: true,
                    day: sunday(NthWeek::First),
                },
            ],
        );
    }

    /// Europe/Berlin moves at 01:00 UTC in both directions and writes two different local hours
    /// to say so, which is the pair a reader that mixed up the two offsets gets wrong.
    #[test]
    fn a_zone_that_transitions_at_one_utc_hour_reads_back_as_that_hour() {
        for observance in observances_of(BERLIN, "Europe/Berlin") {
            let moment = observance
                .start()
                .at_offset(observance.offset_from())
                .unwrap();
            let utc = CivilDateTime::from_instant(moment, UtcOffset::UTC).unwrap();
            assert_eq!(utc.time(), CivilTime::from_hms(1, 0, 0).unwrap());
            assert_eq!(utc.date().year(), 1970);
        }
    }

    /// The hour that repeats and the hour that does not exist, both derived from what was read
    /// and both taken from real transitions rather than from this crate's own answer.
    #[test]
    fn an_hour_that_repeats_and_an_hour_that_does_not_are_both_in_what_was_read() {
        let eastern = observances_of(NEW_YORK_DATED, "America/New_York");
        let spring = *eastern.first().unwrap();
        let fall = *eastern.get(1).unwrap();
        assert_eq!(spring.start(), stamp(2027, 3, 14, 2, 0));
        assert_eq!(fall.start(), stamp(2027, 11, 7, 2, 0));
        assert_eq!(
            readings_of(fall, stamp(2027, 11, 7, 1, 30)),
            2,
            "01:30 on the morning the United States falls back names two instants"
        );
        assert_eq!(
            readings_of(spring, stamp(2027, 3, 14, 2, 30)),
            0,
            "02:30 on the morning it springs forward names none"
        );
        assert_eq!(readings_of(spring, stamp(2027, 3, 14, 3, 30)), 1);
        assert_eq!(readings_of(fall, stamp(2027, 11, 7, 3, 30)), 1);
    }

    /// Lord Howe's fold is thirty minutes wide, so an hour-wide reading of it reports an
    /// ambiguity at 01:15 where the zone has none.
    #[test]
    fn a_half_hour_fold_is_half_an_hour_wide_and_not_an_hour() {
        let howe = observances_of(LORD_HOWE, "Australia/Lord_Howe");
        let ends = *howe.first().unwrap();
        let begins = *howe.get(1).unwrap();
        assert_eq!(readings_of(ends, stamp(2027, 4, 4, 1, 45)), 2);
        assert_eq!(readings_of(ends, stamp(2027, 4, 4, 1, 15)), 1);
        assert_eq!(readings_of(begins, stamp(2027, 10, 3, 2, 15)), 0);
        assert_eq!(readings_of(begins, stamp(2027, 10, 3, 1, 45)), 1);
    }

    /// The table of dates that runs out, which is the input this whole crate turns on.
    #[test]
    fn a_table_written_as_dates_stops_where_its_dates_do() {
        let (zones, reported) = read(NEW_YORK_DATED);
        let table = zones.table("America/New_York").unwrap();
        assert!(codes(&reported).is_empty());
        assert!(!table.is_truncated());
        let starts: Vec<CivilDate> = table
            .observances()
            .iter()
            .map(|observance| observance.start().date())
            .collect();
        assert_eq!(
            starts,
            alloc::vec![
                date(2027, 3, 14),
                date(2027, 11, 7),
                date(2028, 3, 12),
                date(2028, 11, 5),
                date(2029, 3, 11),
                date(2029, 11, 4),
            ],
            "one observance for the DTSTART and one for each RDATE value, sorted"
        );
        assert!(
            table
                .observances()
                .iter()
                .all(|observance| observance.rule().is_none()),
            "a date states no cadence, so no RDATE observance carries the rule"
        );
        assert_eq!(
            table.coverage_end(),
            Some(date(2029, 11, 4)),
            "an event in 2035 is past everything this file knows"
        );
        assert!(table.coverage_end() < Some(date(2035, 6, 1)));
    }

    /// A zone whose government moved its rules after 2007 keeps both, and each `UNTIL` is dated
    /// on the observance's own clock rather than on the UTC one it was written in.
    #[test]
    fn a_zone_that_changed_its_rules_keeps_both_and_dates_each_on_its_own_clock() {
        let (zones, reported) = read(NEW_YORK_BOTH_RULES);
        let table = zones.table("America/New_York").unwrap();
        assert!(codes(&reported).is_empty());
        assert_eq!(table.observances().len(), 4);
        let through: Vec<Option<CivilDate>> = table
            .observances()
            .iter()
            .map(|observance| observance.rule().and_then(YearlyRule::through))
            .collect();
        assert_eq!(
            through,
            alloc::vec![Some(date(2006, 10, 29)), Some(date(2006, 4, 2)), None, None,],
            "UNTIL:20061029T060000Z is 02:00 on the 29th read against -0400, not 06:00 UTC"
        );
        assert_eq!(
            table.coverage_end(),
            None,
            "the rules written for 2007 onwards have no UNTIL, so the zone knows the future"
        );
    }

    /// A `TZID` is not an IANA identifier and this reader does not pretend otherwise.
    #[test]
    fn an_identifier_is_filed_exactly_as_it_was_written() {
        let written = [
            "America/New_York",
            "W. Europe Standard Time",
            "/mozilla.org/20050126_1/Europe/Berlin",
            "Customized Time Zone",
            "GMT+9",
            "\"Europe/Paris\"",
        ];
        for tzid in written {
            let (zones, reported) = read(&definition(tzid));
            assert!(codes(&reported).is_empty(), "{tzid}");
            let filed = tzid.trim_matches('"');
            assert!(
                zones.table(filed).is_some(),
                "{tzid} should be filed under {filed}"
            );
            assert_eq!(zones.len(), 1, "{tzid}");
        }
        let (folded, _) = read(&definition("W. Europe Standard Time"));
        assert!(
            folded.table("w. europe standard time").is_none(),
            "lookup is by exact bytes, which is what keeps aliasing the caller's step"
        );
        let (prefixed, _) = read(&definition("/mozilla.org/20050126_1/Europe/Berlin"));
        assert!(
            prefixed.table("Europe/Berlin").is_none(),
            "nothing goes looking for an IANA name inside a vendor identifier"
        );
    }

    /// Section 3.6.5 requires an observance and files without one exist, so the zone is
    /// declared, reported, and answers nothing rather than answering UTC.
    #[test]
    fn a_definition_with_no_observance_is_reported_and_still_a_zone() {
        let (zones, reported) = read("BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nEND:VTIMEZONE\n");
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::VtimezoneWithoutObservance]
        );
        assert_eq!(first_severity(&reported), Some(Severity::Violation));
        let table = zones.table("Europe/Berlin").unwrap();
        assert!(table.is_empty());
        assert_eq!(table.coverage_end(), None);
    }

    /// A rule outside the closed form costs the rule and not the observance.
    #[test]
    fn a_rule_this_crate_does_not_evaluate_is_a_note_and_the_dtstart_still_stands() {
        let refused = [
            "FREQ=MONTHLY;BYMONTHDAY=1",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;BYSETPOS=1",
            "FREQ=YEARLY;BYMONTH=3,10;BYDAY=-1SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;COUNT=5",
            "FREQ=YEARLY;INTERVAL=2;BYMONTH=3;BYDAY=2SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=-2SU",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=SU;BYMONTHDAY=8,9,10",
            "FREQ=YEARLY;BYMONTH=3;BYWEEKNO=13",
            "BYMONTH=3;BYDAY=2SU",
        ];
        for rule in refused {
            let (zones, reported) = read(&ruled(rule));
            assert_eq!(
                codes(&reported),
                alloc::vec![DiagnosticCode::VtimezoneRuleUnsupported],
                "{rule}"
            );
            assert_eq!(first_severity(&reported), Some(Severity::Note), "{rule}");
            let table = zones.table("Z").unwrap();
            assert_eq!(table.observances().len(), 1, "{rule}");
            let observance = *table.observances().first().unwrap();
            assert_eq!(observance.rule(), None, "{rule}");
            assert_eq!(observance.start(), stamp(2007, 3, 25, 2, 0), "{rule}");
            assert_eq!(
                table.coverage_end(),
                Some(date(2007, 3, 25)),
                "the observance stands alone and covers its own date, {rule}"
            );
        }
    }

    /// The day forms the corpus actually writes, including the two window shapes Exchange and
    /// older tz database releases emit instead of a `BYDAY` ordinal.
    #[test]
    fn every_day_form_the_corpus_writes_reads_as_the_one_it_names() {
        let cases = [
            (
                "FREQ=YEARLY;BYMONTH=10;BYDAY=SU;BYMONTHDAY=25,26,27,28,29,30,31",
                10_u8,
                RuleDay::OnOrBefore {
                    weekday: Weekday::Sunday,
                    day: 31,
                },
            ),
            (
                "FREQ=YEARLY;BYMONTH=4;BYDAY=SU;BYMONTHDAY=1,2,3,4,5,6,7",
                4,
                RuleDay::OnOrAfter {
                    weekday: Weekday::Sunday,
                    day: 1,
                },
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYDAY=SU;BYMONTHDAY=-1,-2,-3,-4,-5,-6,-7",
                3,
                sunday(NthWeek::Last),
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=-1",
                3,
                RuleDay::LastDayOfMonth,
            ),
            (
                "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15",
                3,
                RuleDay::DayOfMonth(15),
            ),
            ("freq=yearly;bymonth=3;byday=5su", 3, sunday(NthWeek::Fifth)),
            // Neither a BYMONTH nor a day part: RFC 5545 section 3.3.10 takes both from
            // DTSTART, which in this fixture is the 25th of March.
            ("FREQ=YEARLY", 3, RuleDay::DayOfMonth(25)),
        ];
        for (written, month, expected) in cases {
            let (zones, reported) = read(&ruled(written));
            assert!(codes(&reported).is_empty(), "{written}");
            let observance = *zones.table("Z").unwrap().observances().first().unwrap();
            let rule = observance.rule().unwrap();
            assert_eq!(rule.day(), expected, "{written}");
            assert_eq!(rule.month(), month, "{written}");
            assert_eq!(
                rule.at(),
                CivilTime::from_hms(2, 0, 0).unwrap(),
                "{written}"
            );
            assert_eq!(rule.through(), None, "{written}");
        }
    }

    /// A `TZID` parameter naming nothing is reported once for the identifier, not once for
    /// every property that used it.
    #[test]
    fn an_identifier_no_definition_backs_is_reported_once() {
        let mut body = String::from(BERLIN);
        body.push_str(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=America/Denver:20260810T090000\n\
             DTEND;TZID=America/Denver:20260810T100000\n\
             RDATE;TZID=Europe/Berlin:20260811T090000\nEND:VEVENT\n",
        );
        body.push_str(
            "BEGIN:VEVENT\nUID:b\nDTSTART;TZID=America/Denver:20260812T090000\nEND:VEVENT\n",
        );
        let (zones, reported) = read(&body);
        assert!(zones.table("Europe/Berlin").is_some());
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::MissingTimeZoneDefinition],
            "three properties named one undefined zone, and the defined one said nothing"
        );
    }

    /// A calendar declaring one zone twice reports it and keeps the first, rather than
    /// preferring the later definition out of sight.
    #[test]
    fn a_second_definition_of_one_zone_is_reported_and_not_chosen_between() {
        let mut body = String::from(BERLIN);
        body.push_str(NEW_YORK_DATED);
        body.push_str(&definition("Europe/Berlin"));
        let (zones, reported) = read(&body);
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::DuplicateTimeZoneIdentifier]
        );
        assert_eq!(zones.len(), 2);
        assert_eq!(
            zones.table("Europe/Berlin").unwrap().observances().len(),
            2,
            "the definition that arrived first is the one that stayed"
        );
    }

    /// A bound nobody charges is decoration: the table that owns the truncation decision is the
    /// one that reports it, and this unit calls it rather than repeating it.
    #[test]
    fn observances_past_the_bound_are_dropped_by_the_table_that_owns_the_decision() {
        let limits = Limits::DEFAULT.with_max_vtimezone_observances(4);
        let (zones, reported) = read_under(NEW_YORK_DATED, limits);
        let table = zones.table("America/New_York").unwrap();
        assert!(table.is_truncated());
        assert_eq!(table.observances().len(), 4);
        assert_eq!(
            codes(&reported),
            alloc::vec![DiagnosticCode::VtimezoneObservancesTruncated]
        );
        assert_eq!(
            table.coverage_end(),
            Some(date(2028, 11, 5)),
            "coverage ends earlier rather than the table acquiring a hole"
        );
    }

    /// The zone-count bound is charged too, and the definitions past it are refused rather than
    /// admitted.
    #[test]
    fn a_calendar_declaring_more_zones_than_the_policy_admits_keeps_the_ones_that_fit() {
        let limits = Limits::DEFAULT.with_max_vtimezone_components(1);
        let mut body = String::from(BERLIN);
        body.push_str(NEW_YORK);
        let (zones, reported) = read_under(&body, limits);
        assert_eq!(zones.len(), 1);
        assert!(zones.table("Europe/Berlin").is_some());
        assert!(
            codes(&reported).is_empty(),
            "the golden list has no code for a zone count, and this unit invents none"
        );
    }

    /// The trait is the other door, and a caller holding one component and no calendar uses it.
    #[test]
    fn one_definition_can_be_read_through_the_trait_and_an_erased_sink() {
        let text = calendar_text(NEW_YORK);
        let document =
            Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let vtimezone: &Component = calendar
            .components()
            .find(|component| component.is_named(b"VTIMEZONE"))
            .unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut out = Vec::new();
        let tzid = vtimezone.read_vtimezone(&mut meter, &mut reported, &mut out);
        assert_eq!(tzid, Some(Box::from("America/New_York")));
        assert_eq!(out.len(), 2);
        assert!(reported.is_empty());
        assert_eq!(
            meter.vtimezone_observances(),
            0,
            "the trait reads; admitting the observances is the table's charge and its call"
        );
    }

    /// A definition this crate cannot file anywhere is `None` rather than a zone filed under a
    /// name nobody wrote.
    #[test]
    fn a_definition_with_no_usable_identifier_is_filed_nowhere() {
        let bodies = [
            "BEGIN:VTIMEZONE\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nTZID:Europe/Paris\nBEGIN:STANDARD\n\
             TZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nDTSTART:19700101T000000\nEND:STANDARD\n\
             END:VTIMEZONE\n",
        ];
        for body in bodies {
            let (zones, _) = read(body);
            assert!(zones.is_empty(), "{body}");
        }
    }

    /// An observance with no readable offset contributes nothing and no code of its own, and
    /// the definition around it is still a zone that carried a subcomponent.
    #[test]
    fn an_observance_with_no_readable_offset_yields_nothing_and_no_code_here() {
        let (zones, reported) = read(
            "BEGIN:VTIMEZONE\nTZID:Europe/Berlin\nBEGIN:STANDARD\nTZOFFSETTO:+0100\n\
             DTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n",
        );
        assert!(
            codes(&reported).is_empty(),
            "section 3.6's required-property reading is Component::audit's, not this unit's"
        );
        assert!(zones.table("Europe/Berlin").unwrap().is_empty());
    }

    /// The two dimensions this unit charges, and the only two a zone definition has.
    ///
    /// Pinned before the reader existed because `docs/adr/0010`'s argument is that a bound
    /// nobody charges is decoration: these fields sat in `Limits` through two milestones with
    /// no charge site at all, and this is the milestone that owes them one.
    #[test]
    fn the_bounds_this_unit_charges_are_the_two_a_zone_definition_has() {
        let limits = Limits::DEFAULT;
        assert_eq!(limits.max_vtimezone_observances(), 4096);
        assert_eq!(limits.max_vtimezone_components(), 256);
        assert!(Limits::GENEROUS.max_vtimezone_observances() > limits.max_vtimezone_observances());

        let text = calendar_text(NEW_YORK);
        let document = Document::parse(text.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let calendar = document.components().next().unwrap();
        let mut meter = Meter::new(limits);
        let zones = read_calendar_zones(calendar, &mut meter, &mut IgnoreDiagnostics);
        assert_eq!(zones.len(), 1);
        assert_eq!(meter.vtimezone_observances(), 2);
        assert_eq!(meter.vtimezone_components(), 1);
    }
}
