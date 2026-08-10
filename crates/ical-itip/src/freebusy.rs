// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Free/busy time: the interval a request asks about, and the busy time an answer states.
//!
//! Specification: RFC 5546 section 3.3 (`PUBLISH`, `REQUEST` and `REPLY` applied to a
//! `VFREEBUSY`), RFC 5545 section 3.6.4 (the component), section 3.8.2.6 (`FREEBUSY`), section
//! 3.2.9 (`FBTYPE`), section 3.3.9 (`PERIOD`) and section 3.8.2.2 (`DTEND`).
//!
//! Behind the `freebusy` feature. Without it a `VFREEBUSY` payload is refused outright at
//! [`crate::ItipMessage::read`] rather than ignored, because a scheduling message a build
//! cannot reason about is not a message it may accept.
//!
//! # The two questions this module answers
//!
//! They are the whole of section 3.3 that the rest of the crate cannot already answer. A
//! `REQUEST` names an interval and asks who is busy inside it, and the interval is its
//! `DTSTART` and `DTEND` — [`requested_window`], with [`window_of`] the same answer carrying
//! the reason it was refused. A `PUBLISH` or a `REPLY` answers with `FREEBUSY` properties,
//! which section 3.3 requires be readable both as a comma-separated list on one line and as
//! repeated lines — [`busy_periods`], which reads both and keeps them apart by occurrence.
//!
//! Everything else about a `VFREEBUSY` message already lives somewhere. Which properties each
//! of the three methods admits is [`crate::table`]'s transcription of the sections 3.3.1 to
//! 3.3.3 tables; whether the sender may send it is [`crate::evaluate_message`]; what the
//! message would change is [`crate::describe_message`]. None of that is restated here, and a
//! `FREEBUSY` property on a `REQUEST` — which section 3.3.2's table forbids — is read by this
//! module and refused by the gate, because reading a value and permitting it are two answers.
//!
//! # Why every refusal here is an error and never an empty answer
//!
//! A window that does not read is not a window of no length, and a busy list that does not
//! read is not a free calendar. Both silences say *available* to whatever schedules against
//! them, so a `DTEND` that does not follow its `DTSTART`, a bound written in a clock the
//! message does not name, and a period that runs backwards each refuse the whole component.
//! That is the direction [`crate::message`] takes with a limit breach, for the same reason: a
//! degraded answer here is not a worse answer but a *different* one, and an attacker who can
//! shape the message picks which of the two the reader believes.
//!
//! # What bounds the reading
//!
//! One `FREEBUSY` line may carry an unbounded list and a component may carry unbounded lines,
//! so [`busy_periods`] charges every period to the caller's [`Meter`] — both as work and, with
//! [`Meter::charge_item`], as a retained item against `Limits::max_items`. There is no
//! `Limits` parameter beside the meter: the ledger already carries the caller's policy, and a
//! second copy of it here is a second place for the two to disagree. The ledger is shared, so
//! a thousand messages read under one meter are bounded in aggregate rather than a thousand
//! times individually, which is the amplification ADR-0010 exists to bound.

use alloc::vec::Vec;

use ical_core::{
    ComponentKind, DateTimeValue, DecodeValue, Duration, Instant, Meter, Period, PropertyId,
    UtcOffset,
};

use crate::state::{PropertyOccurrence, ScheduledComponent};

/// Seconds in the day a [`Duration`]'s day field counts.
const SECONDS_PER_DAY: i64 = 86_400;

/// What one `FREEBUSY` period claims about the time it covers, from RFC 5545 section 3.2.9.
///
/// The four values that section registers, plus the class it leaves open. An absent `FBTYPE`
/// is [`FreeBusyKind::Busy`], which is the default section 3.2.9 states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FreeBusyKind {
    /// `FREE`: the time is available.
    Free,
    /// `BUSY`: the time is taken. The value an absent `FBTYPE` means.
    #[default]
    Busy,
    /// `BUSY-UNAVAILABLE`: taken, and the party cannot be scheduled at all.
    BusyUnavailable,
    /// `BUSY-TENTATIVE`: taken by something not yet confirmed.
    BusyTentative,
    /// A value section 3.2.9 does not register: an `iana-token` or an `x-name`.
    ///
    /// [`FreeBusyKind::is_busy`] answers `true` for it, because section 3.2.9 says an
    /// unrecognized value is to be treated exactly as `BUSY` is. That instruction runs in the
    /// safe direction and this crate has no reason to improve on it: the alternative reading
    /// lets a producer mark a slot free with a name the reader has never heard of.
    Other,
}

impl FreeBusyKind {
    /// The kind `value` names, [`FreeBusyKind::Other`] for anything section 3.2.9 leaves open.
    #[must_use]
    pub fn read(value: &[u8]) -> Self {
        for (spelling, kind) in [
            (&b"FREE"[..], Self::Free),
            (b"BUSY", Self::Busy),
            (b"BUSY-UNAVAILABLE", Self::BusyUnavailable),
            (b"BUSY-TENTATIVE", Self::BusyTentative),
        ] {
            if spelling.eq_ignore_ascii_case(value) {
                return kind;
            }
        }
        Self::Other
    }

    /// Whether this kind says the time is taken.
    ///
    /// Everything but [`FreeBusyKind::Free`], the unrecognized class included.
    #[must_use]
    pub const fn is_busy(self) -> bool {
        !matches!(self, Self::Free)
    }
}

/// Why a `VFREEBUSY` could not be read as one.
///
/// `#[non_exhaustive]`, so a caller's `match` keeps its `_` arm and a new refusal is not a
/// major version. Every variant refuses the whole component: see this module's own
/// documentation for why a partial reading is the more dangerous answer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FreeBusyError {
    /// The component is not a `VFREEBUSY`, so it states no free/busy time at all.
    NotFreeBusy,
    /// A property RFC 5546 section 3.3's tables require exactly once is not there.
    Absent(PropertyId),
    /// A property required exactly once appears more than once.
    ///
    /// Two `DTSTART`s name no single window. Which of them a reader takes is a choice the
    /// message left to the reader, which is the shape of a file meant to be read two ways.
    Repeated(PropertyId),
    /// A value that is not the date-time or the period its property must carry.
    Unreadable(PropertyOccurrence),
    /// A bound not written in UTC.
    ///
    /// RFC 5545 sections 3.8.2.2, 3.8.2.4 and 3.8.2.6 all require UTC inside a `VFREEBUSY`. A
    /// floating bound would place the window under whichever zone the reader happens to be in,
    /// so two recipients of one message would answer about two different intervals.
    NotUtc(PropertyOccurrence),
    /// An interval whose end does not follow its start.
    ///
    /// RFC 5545 section 3.8.2.2 requires a `VFREEBUSY`'s `DTEND` to be later in time than its
    /// `DTSTART`, and section 3.3.9 requires a period to have positive length. An empty
    /// interval is refused with them: it asks about nothing while looking like a question.
    NotLaterThanStart(PropertyOccurrence),
    /// More periods than the caller's policy retains, from `Limits::max_items`.
    TooManyPeriods,
    /// The caller's shared ledger ran out.
    BudgetExhausted,
}

/// One period of one `FREEBUSY` property, with what it claims and where it was written.
///
/// Both bounds are resolved to instants: a `period-start` written as a start and a duration
/// and a `period-explicit` written as two date-times are the same claim about time, and a
/// caller comparing two messages should not have to know which spelling each producer chose.
/// Which spelling it *was* stays in the component, which this crate never rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyPeriod {
    /// Which `FREEBUSY` property of the component stated it.
    at: PropertyOccurrence,
    /// What it claims about the time it covers.
    kind: FreeBusyKind,
    /// When it begins.
    start: Instant,
    /// When it ends, always later than [`BusyPeriod::start`].
    end: Instant,
}

impl BusyPeriod {
    /// Which `FREEBUSY` property stated this period.
    ///
    /// Several periods share one occurrence when the property carried a list, which is the
    /// point of keeping it: a caller reporting a bad period can name the line it was on.
    #[must_use]
    pub const fn at(&self) -> &PropertyOccurrence {
        &self.at
    }

    /// What this period claims about the time it covers.
    #[must_use]
    pub const fn kind(&self) -> FreeBusyKind {
        self.kind
    }

    /// Whether this period says the time is taken.
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        self.kind.is_busy()
    }

    /// When it begins.
    #[must_use]
    pub const fn start(&self) -> Instant {
        self.start
    }

    /// When it ends.
    #[must_use]
    pub const fn end(&self) -> Instant {
        self.end
    }
}

/// The interval `component` asks about, from its `DTSTART` and `DTEND`.
///
/// RFC 5546 section 3.3.2's `REQUEST` is what names one, and sections 3.3.1 and 3.3.3 require
/// the same pair of an answer, so this reads all three.
///
/// `None` when the component states no interval this crate can read, which includes the
/// interval that runs backwards: see [`window_of`] for which of those it was. A caller showing
/// a person why a request was refused wants that function; a caller that only needs the
/// interval wants this one.
#[must_use]
pub fn requested_window(component: &dyn ScheduledComponent) -> Option<(Instant, Instant)> {
    window_of(component).ok()
}

/// The interval `component` asks about, or the reason it states none.
///
/// # Errors
///
/// [`FreeBusyError`], naming the first refusal: the component kind, then `DTSTART`, then
/// `DTEND`, then the order of the two.
pub fn window_of(component: &dyn ScheduledComponent) -> Result<(Instant, Instant), FreeBusyError> {
    if component.component_kind() != Some(ComponentKind::FreeBusy) {
        return Err(FreeBusyError::NotFreeBusy);
    }
    let opens = read_bound(component, PropertyId::DTSTART)?;
    let closes = read_bound(component, PropertyId::DTEND)?;
    if closes <= opens {
        return Err(FreeBusyError::NotLaterThanStart(first(PropertyId::DTEND)));
    }
    Ok((opens, closes))
}

/// Every period every `FREEBUSY` property of `component` states, in document order.
///
/// RFC 5545 section 3.8.2.6 lets one property carry a comma-separated list and lets a
/// component carry the property more than once, and RFC 5546 section 3.3's tables admit both;
/// this reads both and records which property each period came from.
///
/// Every period is charged to `meter`, so a component whose list is longer than the caller's
/// policy retains is refused rather than truncated. Truncation is the unsafe direction here
/// for the reason [`crate::message`] gives about an attendee list: a dropped period turns
/// "busy" into "free", and a producer that can pad a list chooses which of the two is believed.
///
/// # Errors
///
/// [`FreeBusyError`], refusing the whole component. A single unreadable period is not skipped:
/// the periods around it would then describe a calendar with a hole in it that nothing reports.
pub fn busy_periods(
    component: &dyn ScheduledComponent,
    meter: &mut Meter,
) -> Result<Vec<BusyPeriod>, FreeBusyError> {
    if component.component_kind() != Some(ComponentKind::FreeBusy) {
        return Err(FreeBusyError::NotFreeBusy);
    }
    let mut found = Vec::new();
    let mut seen = 0_usize;
    for index in 0..component.property_count() {
        let Some(name) = component.property_name(index) else {
            continue;
        };
        if !PropertyId::FREEBUSY.matches(name) {
            continue;
        }
        let at = PropertyOccurrence::new(PropertyId::FREEBUSY, seen);
        seen = seen.saturating_add(1);
        let Some(line) = component.property_line(index) else {
            return Err(FreeBusyError::Unreadable(at));
        };
        read_periods(line, &at, &mut found, meter)?;
    }
    Ok(found)
}

/// The occurrence naming the first property carrying `id`.
const fn first(id: PropertyId) -> PropertyOccurrence {
    PropertyOccurrence::new(id, 0)
}

/// The instant the one property of `component` named `id` states.
fn read_bound(
    component: &dyn ScheduledComponent,
    id: PropertyId,
) -> Result<Instant, FreeBusyError> {
    let line = only_line(component, &id)?;
    let at = first(id);
    let Some((_, value)) = split_line(line) else {
        return Err(FreeBusyError::Unreadable(at));
    };
    let Ok(written) = DateTimeValue::decode_value(value) else {
        return Err(FreeBusyError::Unreadable(at));
    };
    instant_of(written, at)
}

/// The whole content line of the one property of `component` named `id`.
///
/// Absence and repetition are two different refusals and neither is a value.
fn only_line<'a>(
    component: &'a dyn ScheduledComponent,
    id: &PropertyId,
) -> Result<&'a [u8], FreeBusyError> {
    let mut found: Option<&[u8]> = None;
    for index in 0..component.property_count() {
        let Some(name) = component.property_name(index) else {
            continue;
        };
        if !id.matches(name) {
            continue;
        }
        if found.is_some() {
            return Err(FreeBusyError::Repeated(id.clone()));
        }
        let Some(line) = component.property_line(index) else {
            return Err(FreeBusyError::Unreadable(first(id.clone())));
        };
        found = Some(line);
    }
    found.ok_or_else(|| FreeBusyError::Absent(id.clone()))
}

/// The instant a bound names, refusing one written in any clock but UTC.
fn instant_of(
    written: DateTimeValue<'_>,
    at: PropertyOccurrence,
) -> Result<Instant, FreeBusyError> {
    match written {
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(FreeBusyError::Unreadable(at)),
        // A `DATE`, a floating date-time and a zoned one alike. RFC 5545 requires UTC
        // throughout a `VFREEBUSY`, and reading anything else would put the answer under a
        // zone the message never named.
        _ => Err(FreeBusyError::NotUtc(at)),
    }
}

/// Read one `FREEBUSY` line's comma-separated periods into `found`.
fn read_periods(
    line: &[u8],
    at: &PropertyOccurrence,
    found: &mut Vec<BusyPeriod>,
    meter: &mut Meter,
) -> Result<(), FreeBusyError> {
    let Some((header, value)) = split_line(line) else {
        return Err(FreeBusyError::Unreadable(at.clone()));
    };
    let kind = line_parameter(header, b"FBTYPE").map_or(FreeBusyKind::Busy, FreeBusyKind::read);
    // A `PERIOD` cannot contain a comma: section 3.3.9 writes it from digits, `T`, `Z`, `P`
    // and `/`, so splitting on one separates list items and can divide nothing else.
    for text in value.split(|octet| *octet == b',') {
        meter
            .try_charge(1)
            .map_err(|_spent| FreeBusyError::BudgetExhausted)?;
        meter
            .charge_item()
            .map_err(|_full| FreeBusyError::TooManyPeriods)?;
        let (start, end) = period_window(text, at)?;
        found.push(BusyPeriod {
            at: at.clone(),
            kind,
            start,
            end,
        });
    }
    Ok(())
}

/// The two instants one period covers, in whichever of section 3.3.9's forms it was written.
fn period_window(
    text: &[u8],
    at: &PropertyOccurrence,
) -> Result<(Instant, Instant), FreeBusyError> {
    let Ok(period) = Period::decode_value(text) else {
        return Err(FreeBusyError::Unreadable(at.clone()));
    };
    let start = instant_of(period.start(), at.clone())?;
    let end = match period {
        Period::Explicit { end, .. } => instant_of(end, at.clone())?,
        Period::Starting { duration, .. } => length(duration)
            .and_then(|seconds| start.checked_add_seconds(seconds))
            .ok_or_else(|| FreeBusyError::Unreadable(at.clone()))?,
    };
    if end <= start {
        return Err(FreeBusyError::NotLaterThanStart(at.clone()));
    }
    Ok((start, end))
}

/// A span in seconds, or `None` when it does not fit in one.
const fn length(span: Duration) -> Option<i64> {
    match span.days().checked_mul(SECONDS_PER_DAY) {
        Some(days) => days.checked_add(span.seconds()),
        None => None,
    }
}

/// One content line as its header and its value, or `None` when it carries no `:`.
///
/// The first `:` outside a `DQUOTE` pair, because RFC 5545 section 3.1 lets a quoted parameter
/// value carry one and a reader that took the first `:` of all would cut a line in the middle
/// of a parameter.
fn split_line(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut quoted = false;
    for (at, octet) in line.iter().enumerate() {
        match *octet {
            b'"' => quoted = !quoted,
            b':' if !quoted => {
                return Some((line.get(..at)?, line.get(at.saturating_add(1)..)?));
            },
            _ => {},
        }
    }
    None
}

/// The value of the parameter `name` on `header`, or `None` when it states none.
///
/// The first occurrence wins, which is the rule `ical-core` already applies to a repeated
/// `TZID`: one line states one answer, and taking the last would let a producer who wrote two
/// decide which reader sees which.
///
/// One surrounding `DQUOTE` pair is removed and nothing else is decoded. RFC 6868's caret
/// encoding cannot appear in an `FBTYPE`, whose value RFC 5545 section 3.2.9 makes an
/// `iana-token` or an `x-name`; a value carrying a caret is therefore one this crate does not
/// recognize, and section 3.2.9 has it read as `BUSY`, which is where it lands.
fn line_parameter<'a>(header: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut quoted = false;
    let mut from = 0_usize;
    let mut found = None;
    for (at, octet) in header.iter().enumerate() {
        match *octet {
            b'"' => quoted = !quoted,
            b';' if !quoted => {
                if found.is_none() {
                    found = header
                        .get(from..at)
                        .and_then(|part| value_if_named(part, name));
                }
                from = at.saturating_add(1);
            },
            _ => {},
        }
    }
    found.or_else(|| {
        header
            .get(from..)
            .and_then(|part| value_if_named(part, name))
    })
}

/// The value `part` states when it is the parameter `name`, otherwise `None`.
///
/// The property name itself carries no `=` and so is never mistaken for a parameter.
fn value_if_named<'a>(part: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let at = part.iter().position(|octet| *octet == b'=')?;
    if !part.get(..at)?.eq_ignore_ascii_case(name) {
        return None;
    }
    Some(unquoted(part.get(at.saturating_add(1)..)?))
}

/// `value` with one surrounding `DQUOTE` pair removed.
///
/// A lone `DQUOTE` is left alone: it opens a pair nothing closed, and removing half of one
/// would turn a malformed parameter into a well-formed value that says something else.
fn unquoted(value: &[u8]) -> &[u8] {
    value
        .strip_prefix(b"\"")
        .and_then(|rest| rest.strip_suffix(b"\""))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        ComponentKind, DateTimeValue, DecodeValue, IgnoreDiagnostics, Instant, Limits, Meter,
        PropertyId, UtcOffset,
    };
    use ical_recur::OverrideRange;

    use super::{
        BusyPeriod, FreeBusyError, FreeBusyKind, busy_periods, line_parameter, requested_window,
        split_line, window_of,
    };
    use crate::authorize::{Authorization, AuthorizationDenied, evaluate_message};
    use crate::identity::{FoldSide, InstanceClock, InstanceRef, SequenceRead};
    use crate::message::ItipMessage;
    use crate::method::Method;
    use crate::party::{Attendee, Party, PartyId};
    use crate::state::{PropertyOccurrence, ScheduledComponent};
    use crate::table::MethodRule;
    use crate::transition::TransitionReason;

    /// The properties RFC 5546 section 3.3.2 requires of a `REQUEST`, as the exchange in its
    /// section 4.4 writes them: an organizer asking one attendee about one day.
    const ASKED: [&[u8]; 6] = [
        b"UID:fb-1@example.com",
        b"DTSTAMP:20260901T083000Z",
        b"DTSTART:20260901T000000Z",
        b"DTEND:20260902T000000Z",
        b"ORGANIZER:mailto:ann@example.com",
        b"ATTENDEE:mailto:bo@example.com",
    ];

    /// The instant a UTC `DATE-TIME` value names.
    fn stamp(value: &[u8]) -> Option<Instant> {
        match DateTimeValue::decode_value(value).ok()? {
            DateTimeValue::Utc(when) => when.at_offset(UtcOffset::UTC),
            _ => None,
        }
    }

    /// The unsigned integer `value` spells, or `None` when it does not spell one.
    fn digits(value: &[u8]) -> Option<u32> {
        if value.is_empty() {
            return None;
        }
        let mut total = 0_u32;
        for octet in value {
            let digit = u32::from(octet.wrapping_sub(b'0'));
            if digit > 9 {
                return None;
            }
            total = total.checked_mul(10)?.checked_add(digit)?;
        }
        Some(total)
    }

    /// A component built from the content lines a message would carry.
    ///
    /// Every accessor is derived from those lines rather than from a field beside them, so a
    /// fixture cannot state one thing to the diff and another to the gate.
    #[derive(Debug)]
    struct Fake {
        /// What kind of component this is.
        kind: ComponentKind,
        /// Its properties, in document order.
        lines: Vec<&'static [u8]>,
        /// The components nested inside it.
        children: Vec<Fake>,
    }

    impl Fake {
        /// A component of `kind` carrying `lines`.
        fn new(kind: ComponentKind, lines: &[&'static [u8]]) -> Self {
            Self {
                kind,
                lines: lines.to_vec(),
                children: Vec::new(),
            }
        }

        /// A `VCALENDAR` stating `method` and carrying `payload`.
        fn calendar(method: &'static [u8], payload: Self) -> Self {
            Self {
                kind: ComponentKind::Calendar,
                lines: alloc::vec![method],
                children: alloc::vec![payload],
            }
        }

        /// The header and value of the first line named `id`.
        fn pair(&self, id: &PropertyId) -> Option<(&'static [u8], &'static [u8])> {
            self.every(id).first().copied()
        }

        /// The value of the first line named `id`.
        fn value(&self, id: &PropertyId) -> Option<&'static [u8]> {
            self.pair(id).map(|(_, value)| value)
        }

        /// The header and value of every line named `id`, in document order.
        fn every(&self, id: &PropertyId) -> Vec<(&'static [u8], &'static [u8])> {
            self.lines
                .iter()
                .filter_map(|line| {
                    let (header, value) = split_line(line)?;
                    let name = header.split(|octet| *octet == b';').next()?;
                    id.matches(name).then_some((header, value))
                })
                .collect()
        }
    }

    impl ScheduledComponent for Fake {
        fn component_kind(&self) -> Option<ComponentKind> {
            Some(self.kind)
        }

        fn method(&self) -> Option<&[u8]> {
            self.value(&PropertyId::METHOD)
        }

        fn uid(&self) -> Option<&[u8]> {
            self.value(&PropertyId::UID)
        }

        fn sequence(&self) -> SequenceRead {
            match self.value(&PropertyId::SEQUENCE) {
                None => SequenceRead::Absent,
                Some(value) => digits(value).map_or(SequenceRead::Unreadable, SequenceRead::Value),
            }
        }

        fn dtstamp(&self) -> Option<Instant> {
            self.value(&PropertyId::DTSTAMP).and_then(stamp)
        }

        fn recurrence_id(&self) -> Option<InstanceRef> {
            let named = stamp(self.value(&PropertyId::from_name(b"RECURRENCE-ID"))?)?;
            Some(
                InstanceRef::new(named, InstanceClock::Utc, OverrideRange::ThisOnly)
                    .with_side(FoldSide::Once),
            )
        }

        fn organizer(&self) -> Option<Party<'_>> {
            let (header, value) = self.pair(&PropertyId::ORGANIZER)?;
            Some(Party::read(value, line_parameter(header, b"SENT-BY")))
        }

        fn attendee_count(&self) -> usize {
            self.every(&PropertyId::ATTENDEE).len()
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            let (header, value) = *self.every(&PropertyId::ATTENDEE).get(index)?;
            let who = Attendee::new(Party::read(value, line_parameter(header, b"SENT-BY")));
            Some(match line_parameter(header, b"PARTSTAT") {
                Some(status) => who.with_part_stat(status),
                None => who,
            })
        }

        fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
            (index < self.attendee_count())
                .then(|| PropertyOccurrence::new(PropertyId::ATTENDEE, index))
        }

        fn property_count(&self) -> usize {
            self.lines.len()
        }

        fn property_name(&self, index: usize) -> Option<&[u8]> {
            let (header, _) = split_line(self.lines.get(index)?)?;
            header.split(|octet| *octet == b';').next()
        }

        fn property_line(&self, index: usize) -> Option<&[u8]> {
            self.lines.get(index).copied()
        }

        fn child_count(&self) -> usize {
            self.children.len()
        }

        fn child(&self, index: usize) -> Option<&dyn ScheduledComponent> {
            self.children
                .get(index)
                .map(|child| child as &dyn ScheduledComponent)
        }
    }

    /// RFC 5546 section 3.3.2: the interval a `REQUEST` asks about is its `DTSTART` and its
    /// `DTEND`, both in UTC per RFC 5545 sections 3.8.2.4 and 3.8.2.2.
    #[test]
    fn a_request_asks_about_the_interval_its_two_bounds_name() {
        let asking = Fake::new(ComponentKind::FreeBusy, &ASKED);
        let (opens, closes) = requested_window(&asking).unwrap();
        assert_eq!(
            opens.checked_seconds_until(closes),
            Some(86_400),
            "one whole day, which is what the two bounds state"
        );
        assert_eq!(window_of(&asking), Ok((opens, closes)));

        let epoch = Fake::new(
            ComponentKind::FreeBusy,
            &[b"DTSTART:19700101T000000Z", b"DTEND:19700101T010000Z"],
        );
        assert_eq!(
            requested_window(&epoch),
            Some((Instant::EPOCH, Instant::from_unix_seconds(3_600))),
            "the bounds are absolute instants and not offsets from anything the reader holds"
        );
    }

    /// Every way a window can fail to be one, refused by name. `DTEND` before `DTSTART` is the
    /// case RFC 5545 section 3.8.2.2 states outright, and none of these answers an empty
    /// interval: an interval of no length asks about nothing while looking like a question.
    #[test]
    fn a_window_that_is_not_one_is_refused_and_never_silently_empty() {
        // The end precedes the start.
        static BACKWARDS: [&[u8]; 2] = [b"DTSTART:20260902T000000Z", b"DTEND:20260901T000000Z"];
        // An interval of no length, which asks about nothing while looking like a question.
        static EMPTY: [&[u8]; 2] = [b"DTSTART:20260901T000000Z", b"DTEND:20260901T000000Z"];
        // A floating bound, which would place the window under whichever zone the reader is in.
        static FLOATING: [&[u8]; 2] = [b"DTSTART:20260901T000000", b"DTEND:20260902T000000Z"];
        // No end at all.
        static NO_END: [&[u8]; 1] = [b"DTSTART:20260901T000000Z"];
        // Two starts, which name no single window.
        static TWO_STARTS: [&[u8]; 3] = [
            b"DTSTART:20260901T000000Z",
            b"DTSTART:20260901T060000Z",
            b"DTEND:20260902T000000Z",
        ];
        // A start that is not a date-time at all.
        static UNREADABLE: [&[u8]; 2] = [b"DTSTART:not-a-time", b"DTEND:20260902T000000Z"];

        let start = PropertyOccurrence::new(PropertyId::DTSTART, 0);
        let end = PropertyOccurrence::new(PropertyId::DTEND, 0);
        let cases: [(&[&[u8]], FreeBusyError); 6] = [
            (&BACKWARDS, FreeBusyError::NotLaterThanStart(end.clone())),
            (&EMPTY, FreeBusyError::NotLaterThanStart(end)),
            (&FLOATING, FreeBusyError::NotUtc(start.clone())),
            (&NO_END, FreeBusyError::Absent(PropertyId::DTEND)),
            (&TWO_STARTS, FreeBusyError::Repeated(PropertyId::DTSTART)),
            (&UNREADABLE, FreeBusyError::Unreadable(start)),
        ];
        for (lines, expected) in cases {
            let component = Fake::new(ComponentKind::FreeBusy, lines);
            assert_eq!(window_of(&component), Err(expected), "{lines:?}");
            assert_eq!(requested_window(&component), None, "{lines:?}");
        }

        let event = Fake::new(ComponentKind::Event, &ASKED);
        assert_eq!(window_of(&event), Err(FreeBusyError::NotFreeBusy));
    }

    /// RFC 5546 section 3.3 requires both spellings of a busy list: several periods on one
    /// property, and several properties. Both are read, and each period says which line it was
    /// on so that the two do not become one anonymous heap.
    #[test]
    fn busy_time_is_read_from_a_list_and_from_repeated_properties_alike() {
        let published = Fake::new(
            ComponentKind::FreeBusy,
            &[
                b"UID:fb-2@example.com",
                b"DTSTAMP:20260901T083000Z",
                b"DTSTART:20260901T000000Z",
                b"DTEND:20260902T000000Z",
                b"ORGANIZER:mailto:ann@example.com",
                b"FREEBUSY:20260901T090000Z/20260901T100000Z,20260901T130000Z/20260901T140000Z",
                b"FREEBUSY;FBTYPE=FREE:20260901T110000Z/PT1H",
                b"FREEBUSY;FBTYPE=\"BUSY-TENTATIVE\":20260901T150000Z/20260901T153000Z",
            ],
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let found = busy_periods(&published, &mut meter).unwrap();

        let shape: Vec<(usize, FreeBusyKind, i64)> = found
            .iter()
            .map(|period| {
                (
                    period.at().index(),
                    period.kind(),
                    period.start().checked_seconds_until(period.end()).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            alloc::vec![
                (0, FreeBusyKind::Busy, 3_600),
                (0, FreeBusyKind::Busy, 3_600),
                (1, FreeBusyKind::Free, 3_600),
                (2, FreeBusyKind::BusyTentative, 1_800),
            ],
            "two periods from the first line, one from each of the other two"
        );
        assert_eq!(found.first().map(BusyPeriod::is_busy), Some(true));
        assert_eq!(found.get(2).map(BusyPeriod::is_busy), Some(false));
        assert_eq!(
            found.first().map(BusyPeriod::at),
            found.get(1).map(BusyPeriod::at)
        );
    }

    /// RFC 5545 section 3.2.9: an `FBTYPE` a reader does not recognize is treated exactly as
    /// `BUSY`. Reading it as free is how a padded slot is booked over.
    #[test]
    fn an_unrecognized_free_busy_type_is_read_as_busy() {
        assert_eq!(FreeBusyKind::read(b"X-OUT-OF-OFFICE"), FreeBusyKind::Other);
        assert!(FreeBusyKind::read(b"X-OUT-OF-OFFICE").is_busy());
        assert!(FreeBusyKind::read(b"busy-unavailable").is_busy());
        assert!(!FreeBusyKind::read(b"Free").is_busy());
        assert_eq!(FreeBusyKind::default(), FreeBusyKind::Busy);

        let odd = Fake::new(
            ComponentKind::FreeBusy,
            &[b"FREEBUSY;FBTYPE=X-OUT:20260901T090000Z/PT1H"],
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let found = busy_periods(&odd, &mut meter).unwrap();
        assert_eq!(
            found.first().map(BusyPeriod::kind),
            Some(FreeBusyKind::Other)
        );
        assert_eq!(found.first().map(BusyPeriod::is_busy), Some(true));
    }

    /// A period that is not a period refuses the whole component. Skipping one would describe
    /// a calendar with a hole in it that nothing reports, and a hole reads as free.
    #[test]
    fn a_period_that_states_no_span_refuses_the_whole_component() {
        let cases: [(&'static [u8], FreeBusyError); 4] = [
            (
                b"FREEBUSY:20260901T100000Z/20260901T090000Z",
                FreeBusyError::NotLaterThanStart(PropertyOccurrence::new(PropertyId::FREEBUSY, 0)),
            ),
            (
                b"FREEBUSY:20260901T090000Z/20260901T090000Z",
                FreeBusyError::NotLaterThanStart(PropertyOccurrence::new(PropertyId::FREEBUSY, 0)),
            ),
            (
                b"FREEBUSY:20260901T090000/20260901T100000",
                FreeBusyError::NotUtc(PropertyOccurrence::new(PropertyId::FREEBUSY, 0)),
            ),
            (
                b"FREEBUSY:20260901T090000Z",
                FreeBusyError::Unreadable(PropertyOccurrence::new(PropertyId::FREEBUSY, 0)),
            ),
        ];
        for (line, expected) in cases {
            let stated = Fake::new(ComponentKind::FreeBusy, &[line]);
            let mut meter = Meter::new(Limits::DEFAULT);
            assert_eq!(busy_periods(&stated, &mut meter), Err(expected), "{line:?}");
        }
    }

    /// ADR-0010 at the boundary: a list exactly at `Limits::max_items` is admitted whole, and
    /// one period past it refuses the message rather than returning the shorter list. The
    /// shorter list is the dangerous answer — every dropped period reads as free time.
    #[test]
    fn a_busy_list_at_the_item_ceiling_is_admitted_and_one_past_it_is_refused() {
        let lines: [&[u8]; 2] = [
            b"FREEBUSY:20260901T090000Z/PT1H,20260901T110000Z/PT1H",
            b"FREEBUSY:20260901T130000Z/PT1H,20260901T150000Z/PT1H",
        ];
        let published = Fake::new(ComponentKind::FreeBusy, &lines);

        let mut exact = Meter::new(Limits::DEFAULT.with_max_items(4));
        assert_eq!(
            busy_periods(&published, &mut exact).map(|found| found.len()),
            Ok(4)
        );

        let mut tight = Meter::new(Limits::DEFAULT.with_max_items(3));
        assert_eq!(
            busy_periods(&published, &mut tight),
            Err(FreeBusyError::TooManyPeriods)
        );

        // The ledger is shared, so a second component read under a spent meter is refused even
        // though its own list is short. That aggregate bound is the whole point of the meter.
        let mut shared = Meter::new(Limits::DEFAULT.with_max_items(4));
        assert!(busy_periods(&published, &mut shared).is_ok());
        assert_eq!(
            busy_periods(&published, &mut shared),
            Err(FreeBusyError::TooManyPeriods)
        );
    }

    /// RFC 5546 section 3.3.3: the attendee the organizer asked answers with their busy time,
    /// and the answer changes that attendee's own line and nothing else. A sender the held
    /// copy does not name is refused with a reason a person can be shown.
    #[test]
    fn the_attendee_who_was_asked_may_answer_and_a_stranger_may_not() {
        let current = Fake::new(ComponentKind::FreeBusy, &ASKED);
        let calendar = Fake::calendar(
            b"METHOD:REPLY",
            Fake::new(
                ComponentKind::FreeBusy,
                &[
                    b"UID:fb-1@example.com",
                    b"DTSTAMP:20260901T090000Z",
                    b"DTSTART:20260901T000000Z",
                    b"DTEND:20260902T000000Z",
                    b"ORGANIZER:mailto:ann@example.com",
                    b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:bo@example.com",
                    b"FREEBUSY:20260901T090000Z/20260901T100000Z",
                ],
            ),
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let message = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink).unwrap();

        let answered = evaluate_message(&message, &current, PartyId::new("mailto:bo@example.com"));
        assert_eq!(
            answered.as_ref().map(Authorization::reason),
            Ok(TransitionReason::ParticipationChanged)
        );
        assert_eq!(
            answered.map(|vetted| vetted.transition().len()),
            Ok(1),
            "an answer states the one attendee line it is about and nothing else"
        );

        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:zz@example.com"))
                .map(|_vetted| ())
                .unwrap_err(),
            AuthorizationDenied::UnknownAttendee,
            "an answer from an address nobody asked adds no participant"
        );
    }

    /// RFC 5546 sections 2.1.4 and 2.1.5, over a `VFREEBUSY`: an older `SEQUENCE` never
    /// overwrites a newer one, and an equal `SEQUENCE` with an older `DTSTAMP` does not either.
    #[test]
    fn an_older_revision_never_overwrites_a_newer_free_busy_answer() {
        let held: [&[u8]; 6] = [
            b"UID:fb-3@example.com",
            b"DTSTAMP:20260901T090000Z",
            b"DTSTART:20260901T000000Z",
            b"DTEND:20260902T000000Z",
            b"ORGANIZER:mailto:ann@example.com",
            b"SEQUENCE:2",
        ];
        let offered: [(&[u8], AuthorizationDenied); 2] = [
            (
                b"SEQUENCE:1",
                AuthorizationDenied::SequenceStale { have: 2 },
            ),
            (
                b"SEQUENCE:2",
                AuthorizationDenied::DtstampStale {
                    have: stamp(b"20260901T090000Z").unwrap(),
                },
            ),
        ];
        let current = Fake::new(ComponentKind::FreeBusy, &held);
        for (revision, expected) in offered {
            let calendar = Fake::calendar(
                b"METHOD:PUBLISH",
                Fake::new(
                    ComponentKind::FreeBusy,
                    &[
                        b"UID:fb-3@example.com",
                        b"DTSTAMP:20260901T080000Z",
                        b"DTSTART:20260901T000000Z",
                        b"DTEND:20260902T000000Z",
                        b"ORGANIZER:mailto:ann@example.com",
                        revision,
                    ],
                ),
            );
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut sink = IgnoreDiagnostics;
            let message =
                ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink).unwrap();
            assert_eq!(
                evaluate_message(&message, &current, PartyId::new("mailto:ann@example.com"))
                    .map(|_vetted| ())
                    .unwrap_err(),
                expected,
                "{revision:?}"
            );
        }
    }

    /// A message naming an instance the caller does not hold is refused, and the refusal comes
    /// before any question about the sender: identity is judged first, so what a person is
    /// shown is that the message is about something else.
    #[test]
    fn a_message_naming_an_instance_the_caller_does_not_hold_is_refused() {
        let current = Fake::new(ComponentKind::FreeBusy, &ASKED);
        let calendar = Fake::calendar(
            b"METHOD:PUBLISH",
            Fake::new(
                ComponentKind::FreeBusy,
                &[
                    b"UID:fb-1@example.com",
                    b"DTSTAMP:20260901T090000Z",
                    b"DTSTART:20260901T000000Z",
                    b"DTEND:20260902T000000Z",
                    b"ORGANIZER:mailto:ann@example.com",
                    b"RECURRENCE-ID:20260901T120000Z",
                ],
            ),
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let message = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink).unwrap();
        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:ann@example.com"))
                .map(|_vetted| ())
                .unwrap_err(),
            AuthorizationDenied::NoMatchingInstance
        );
    }

    /// Reading a value and permitting it are two answers. Section 3.3.2's table forbids
    /// `FREEBUSY` on a `REQUEST` and section 3.3.1's admits it on a `PUBLISH`; this module
    /// reads either, and the transcribed table is what refuses the first.
    #[test]
    fn reading_a_busy_list_is_not_permitting_one() {
        let question = MethodRule::lookup(Method::Request, ComponentKind::FreeBusy).unwrap();
        let told = MethodRule::lookup(Method::Publish, ComponentKind::FreeBusy).unwrap();
        assert!(question.presence_of(b"FREEBUSY").is_forbidden());
        assert!(!told.presence_of(b"FREEBUSY").is_forbidden());

        let component = Fake::new(
            ComponentKind::FreeBusy,
            &[b"FREEBUSY:20260901T090000Z/PT1H"],
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        assert_eq!(
            busy_periods(&component, &mut meter).map(|found| found.len()),
            Ok(1),
            "the value is readable whatever the table says about the method carrying it"
        );
    }

    /// The two content-line readings this module does for itself: the first `:` outside a
    /// quoted parameter, and the first spelling of a repeated parameter.
    #[test]
    fn a_content_line_splits_outside_its_quoted_parameters() {
        assert_eq!(
            split_line(b"FREEBUSY;X-NOTE=\"a:b\":20260901T090000Z/PT1H"),
            Some((
                &b"FREEBUSY;X-NOTE=\"a:b\""[..],
                &b"20260901T090000Z/PT1H"[..]
            ))
        );
        assert_eq!(split_line(b"FREEBUSY"), None);

        let header = &b"FREEBUSY;FBTYPE=BUSY;X-NOTE=\"a;b\";FBTYPE=FREE"[..];
        assert_eq!(line_parameter(header, b"fbtype"), Some(&b"BUSY"[..]));
        assert_eq!(line_parameter(header, b"X-NOTE"), Some(&b"a;b"[..]));
        assert_eq!(line_parameter(header, b"TZID"), None);
        assert_eq!(line_parameter(b"FREEBUSY", b"FBTYPE"), None);
    }
}
