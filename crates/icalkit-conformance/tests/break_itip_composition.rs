// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BREAK. `ical-itip` composed with `ical-recur` and `ical-tz`, on the three seams a scheduling
//! message actually crosses: which instance a message is about, which reading of a zone placed
//! it, and how much of a zone stood behind the answer.
//!
//! Every zone here is transcribed from the rules the zone published, never read off an answer
//! this workspace gave, and every expectation is RFC 5545's or RFC 5546's own text. Where the
//! specification permits two readings the case says so and requires the crate to state which
//! one it took — a value that is the same under both readings, with the same empty report, is
//! not an answer to the question "which meeting is this message about".

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, ComponentKind, DateTimeValue, Diagnostic, DiagnosticCode,
    Instant, Limits, Meter, Severity, UtcOffset, ValueType,
};
use icalkit_conformance::internal::itip::{
    Attendee, AuthorizationDenied, FoldSide, InstanceClock, InstanceRef, ItipMessage, Party,
    PartyId, PropertyOccurrence, ScheduledComponent, SequenceRead, TransitionReason,
    check_exclusions_are_placeable, evaluate_message, resolve_instance,
};
use icalkit_conformance::internal::recur::{
    OverrideRange, OverrideSet, RecurrenceInput, RecurrenceRule, SearchStep, ValueKind, Window,
    parse_recur,
};
use icalkit_conformance::internal::tz::{
    AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, Reading, ResolutionPolicy,
    ResolvedExclusions, ZoneAnswer, ZoneProvenance, ZoneSource, ZonedSeries, nominal,
};

/// The caller's copy of a daily series whose third cadence key falls in a gap.
const HELD_GAP_SERIES: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_gap_series.ics");
/// The override the caller stores for the instance inside the gap.
const HELD_IN_THE_GAP: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_instance_in_the_gap.ics");
/// A `CANCEL` addressed to that instance.
const CANCEL_IN_THE_GAP: &[u8] =
    include_bytes!("fixtures/break_itip_composition/cancel_the_instance_in_the_gap.ics");
/// The override the caller stores for the instance the gated reading counts third.
const HELD_AFTER_THE_GAP: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_instance_after_the_gap.ics");
/// A `CANCEL` addressed to that one.
const CANCEL_AFTER_THE_GAP: &[u8] =
    include_bytes!("fixtures/break_itip_composition/cancel_the_instance_after_the_gap.ics");
/// The override the caller stores for an instance inside the hour New York repeats.
const HELD_FOLDED: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_folded_instance.ics");
/// A `CANCEL` addressed to it, written as the wall clock both halves share.
const CANCEL_FOLDED: &[u8] =
    include_bytes!("fixtures/break_itip_composition/cancel_the_folded_instance.ics");
/// An override six years past the end of its zone's transition table.
const HELD_CONTINUED: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_continued_instance.ics");
/// A `CANCEL` addressed to it.
const CANCEL_CONTINUED: &[u8] =
    include_bytes!("fixtures/break_itip_composition/cancel_the_continued_instance.ics");
/// A series carrying a `Z`-terminated `EXDATE` no zone here can place.
const HELD_UNPLACEABLE: &[u8] =
    include_bytes!("fixtures/break_itip_composition/held_series_with_an_unplaceable_exdate.ics");
/// A `CANCEL` of that series.
const CANCEL_UNPLACEABLE: &[u8] = include_bytes!(
    "fixtures/break_itip_composition/cancel_an_instance_of_the_unplaceable_series.ics"
);
/// The same `CANCEL` written in UTC, naming the instant the shifted instance happens at.
const CANCEL_IN_THE_GAP_IN_UTC: &[u8] =
    include_bytes!("fixtures/break_itip_composition/cancel_the_instance_in_the_gap_in_utc.ics");
/// The caller's copy of an ordinary series, for the envelope cases.
const HELD_PLAIN: &[u8] = include_bytes!("fixtures/break_itip_composition/held_plain_series.ics");
/// A `REQUEST` naming an organizer the caller has never corresponded with.
const REQUEST_FROM_A_STRANGER: &[u8] =
    include_bytes!("fixtures/break_itip_composition/request_from_a_stranger.ics");

/// The organizer every fixture above names.
const CHAIR: &str = "mailto:chair@example.com";
/// The one attendee.
const BO: &str = "mailto:bo@example.com";
/// A party no fixture names anywhere.
const STRANGER: &str = "mailto:zz@example.com";
/// The identifier the gap and fold fixtures are written against.
const NEW_YORK: &str = "America/New_York";
/// The identifier the continuation fixtures are written against.
const EXCHANGE: &str = "W. Europe Standard Time";

// -------------------------------------------------------------------------------------------
// A zone, transcribed.
// -------------------------------------------------------------------------------------------

/// One transition of a real zone: when the clock moved, and what it moved between.
#[derive(Clone, Copy, Debug)]
struct Shift {
    /// The instant the offset changed.
    at: Instant,
    /// Seconds east of UTC before it.
    before: i32,
    /// Seconds east of UTC from it.
    after: i32,
    /// Whether the observance beginning here is the zone's daylight one.
    daylight: bool,
}

/// A zone source built from published transitions, standing in for a read `VTIMEZONE`.
#[derive(Clone, Debug)]
struct HandZone {
    /// The identifier this source answers to, compared by exact bytes.
    tzid: &'static str,
    /// Seconds east of UTC before the first transition.
    base: i32,
    /// The transitions, ascending.
    shifts: Vec<Shift>,
    /// The first date backed by real data, absent when the table reaches back forever.
    known_from: Option<CivilDate>,
    /// The last date backed by real data, absent when the table runs on forever.
    known_through: Option<CivilDate>,
}

impl HandZone {
    /// The offset and daylight flag in force at `instant`.
    fn state_at(&self, instant: Instant) -> (i32, bool) {
        let mut state = (self.base, false);
        for shift in &self.shifts {
            if shift.at <= instant {
                state = (shift.after, shift.daylight);
            }
        }
        state
    }

    /// How much of this zone's data stands behind a question about `instant`.
    fn basis_at(&self, instant: Instant) -> AnswerBasis {
        let before = self
            .known_from
            .filter(|_| self.shifts.first().is_some_and(|first| instant < first.at));
        let past = self
            .known_through
            .filter(|_| self.shifts.last().is_some_and(|last| instant >= last.at));
        match (before, past) {
            (Some(edge), _) => AnswerBasis::BeforeKnownTransitions(edge),
            (_, Some(edge)) => AnswerBasis::BeyondKnownTransitions(edge),
            _ => AnswerBasis::Computed,
        }
    }

    /// Every offset this zone ever runs at.
    fn offsets(&self) -> Vec<i32> {
        let mut seen = vec![self.base];
        for shift in &self.shifts {
            if !seen.contains(&shift.after) {
                seen.push(shift.after);
            }
        }
        seen
    }

    /// Every reading `local` has, ascending — none in a gap, two in a fold.
    fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
        let mut found: Vec<Reading> = Vec::new();
        for seconds in self.offsets() {
            let Some(offset) = UtcOffset::from_seconds(seconds) else {
                continue;
            };
            let Some(instant) = local.at_offset(offset) else {
                continue;
            };
            let (in_force, daylight) = self.state_at(instant);
            if in_force == seconds {
                found.push(Reading::new(instant, offset, daylight));
            }
        }
        found.sort_unstable();
        found
    }

    /// The gap `local` fell in, on the readings `icalkit_conformance::internal::tz::LocalResolution` states.
    fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
        for shift in &self.shifts {
            let offset_before = UtcOffset::from_seconds(shift.before)?;
            let offset_after = UtcOffset::from_seconds(shift.after)?;
            let opened = CivilDateTime::from_instant(shift.at, offset_before)?;
            let closed = CivilDateTime::from_instant(shift.at, offset_after)?;
            if opened <= local && local < closed {
                return Some(LocalResolution::Nonexistent {
                    gap_start: shift.at.checked_add_seconds(-1)?,
                    gap_end: shift.at,
                    offset_before,
                    offset_after,
                    shifted: local.at_offset(offset_before)?,
                });
            }
        }
        None
    }
}

impl ZoneSource for HandZone {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let found = self.readings(local);
        let resolution = match found.as_slice() {
            [one] => LocalResolution::Unique { reading: *one },
            [earlier, later] => LocalResolution::Ambiguous {
                earlier: *earlier,
                later: *later,
            },
            _ => self.gap(local)?,
        };
        let seen = match resolution {
            LocalResolution::Unique { reading } => reading.instant,
            LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
            LocalResolution::Nonexistent { gap_end, .. } => gap_end,
            // This fixture always holds transitions, so it never answers `Undetermined`; the
            // arm is what `#[non_exhaustive]` asks of a match on another crate's enum.
            _ => return None,
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::EmbeddedVtimezone,
            self.basis_at(seen),
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let (seconds, daylight) = self.state_at(instant);
        Some(OffsetAnswer::new(
            UtcOffset::from_seconds(seconds)?,
            daylight,
            ZoneProvenance::EmbeddedVtimezone,
            self.basis_at(instant),
        ))
    }
}

/// `America/New_York` in 2026: forward at 07:00Z on March 8th, back at 06:00Z on November 1st,
/// between EST at -05:00 and EDT at -04:00.
///
/// So local 02:30 on March 8th happens never, and local 01:30 on November 1st happens twice.
fn new_york() -> HandZone {
    HandZone {
        tzid: NEW_YORK,
        base: -18_000,
        shifts: vec![
            shift(utc(2026, 3, 8, 7, 0), -18_000, -14_400, true),
            shift(utc(2026, 11, 1, 6, 0), -14_400, -18_000, false),
        ],
        known_from: None,
        known_through: None,
    }
}

/// Berlin's rules written out as explicit dates for 2027 through 2029 and stopping at both
/// ends, which is what an `RDATE`-driven `VTIMEZONE` from Exchange is.
fn finite_table() -> HandZone {
    HandZone {
        tzid: EXCHANGE,
        base: 3_600,
        shifts: vec![
            shift(utc(2027, 3, 28, 1, 0), 3_600, 7_200, true),
            shift(utc(2027, 10, 31, 1, 0), 7_200, 3_600, false),
            shift(utc(2028, 3, 26, 1, 0), 3_600, 7_200, true),
            shift(utc(2028, 10, 29, 1, 0), 7_200, 3_600, false),
            shift(utc(2029, 3, 25, 1, 0), 3_600, 7_200, true),
            shift(utc(2029, 10, 28, 1, 0), 7_200, 3_600, false),
        ],
        known_from: CivilDate::from_ymd(2027, 3, 28),
        known_through: CivilDate::from_ymd(2029, 10, 28),
    }
}

/// One transition.
const fn shift(at: Instant, before: i32, after: i32, daylight: bool) -> Shift {
    Shift {
        at,
        before,
        after,
        daylight,
    }
}

// -------------------------------------------------------------------------------------------
// Civil arithmetic, spelled once.
// -------------------------------------------------------------------------------------------

/// A wall clock with no zone attached to it yet.
///
/// Answers rather than asserts, per the convention the rest of this corpus uses: a helper below
/// a `#[test]` is production code as far as the lint profile is concerned.
fn clock(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<CivilDateTime> {
    Some(CivilDateTime::new(
        CivilDate::from_ymd(year, month, day)?,
        CivilTime::from_hms(hour, minute, 0)?,
    ))
}

/// The instant a published UTC timestamp names.
fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
    clock(year, month, day, hour, minute)
        .and_then(|civil| civil.at_offset(UtcOffset::UTC))
        .unwrap_or(Instant::from_unix_seconds(0))
}

/// The nominal cadence key a wall clock is, which is what a zoned value carries.
fn key(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
    clock(year, month, day, hour, minute)
        .and_then(nominal)
        .unwrap_or(Instant::from_unix_seconds(0))
}

/// A meter and a sink, which every reporting call needs and no case wants to spell twice.
fn ledger() -> (Meter, Vec<Diagnostic>) {
    (Meter::new(Limits::DEFAULT), Vec::new())
}

/// The codes reported, in the order they were reported.
fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
    reported.iter().map(|entry| entry.code()).collect()
}

// -------------------------------------------------------------------------------------------
// The state a caller holds, as its own `ScheduledComponent`.
// -------------------------------------------------------------------------------------------

/// One content line of a fixture, unfolded and taken apart.
#[derive(Clone, Debug, Default)]
struct Line {
    /// The property name, upper-cased the way RFC 5545 section 3.1 compares one.
    name: Vec<u8>,
    /// The parameters, in document order, as name and value.
    parameters: Vec<(Vec<u8>, Vec<u8>)>,
    /// The value.
    value: Vec<u8>,
    /// The whole line, unfolded and unterminated.
    content: Vec<u8>,
}

impl Line {
    /// The line `content` spells, or `None` when it carries no `:` outside a quoted value.
    fn read(content: &[u8]) -> Option<Self> {
        let mut quoted = false;
        let cut = content.iter().position(|byte| match *byte {
            b'"' => {
                quoted = !quoted;
                false
            },
            b':' => !quoted,
            _ => false,
        })?;
        let (header, tail) = content.split_at(cut);
        let mut parts = header.split(|byte| *byte == b';');
        let name = parts.next()?.to_ascii_uppercase();
        let parameters = parts
            .filter_map(|chunk| {
                let split = chunk.iter().position(|byte| *byte == b'=')?;
                let (attribute, rest) = chunk.split_at(split);
                let raw = rest.get(1..).unwrap_or_default();
                let bare = raw
                    .strip_prefix(b"\"")
                    .and_then(|inner| inner.strip_suffix(b"\""))
                    .unwrap_or(raw);
                Some((attribute.to_ascii_uppercase(), bare.to_vec()))
            })
            .collect();
        Some(Self {
            name,
            parameters,
            value: tail.get(1..).unwrap_or_default().to_vec(),
            content: content.to_vec(),
        })
    }

    /// Whether this line states the property `name`.
    fn is_named(&self, name: &[u8]) -> bool {
        self.name.as_slice() == name
    }

    /// The value of the parameter `name`, absent when the line states none.
    fn parameter(&self, name: &[u8]) -> Option<&[u8]> {
        self.parameters
            .iter()
            .find(|(attribute, _)| attribute.as_slice() == name)
            .map(|(_, value)| value.as_slice())
    }
}

/// One component of a fixture, answering the questions `ical-itip` asks of held state.
///
/// A third implementation of `ScheduledComponent`, written here rather than reached for from
/// `src/itip.rs` because the side of a fold is resolved through `ical-tz` in this file and
/// through a hand-written pair of instants there.
#[derive(Clone, Debug, Default)]
struct Node {
    /// What the `BEGIN` line named, `None` for a name RFC 5545 does not define.
    kind: Option<ComponentKind>,
    /// The properties directly inside this component, in document order.
    properties: Vec<Line>,
    /// Which of those properties are `ATTENDEE` lines, in document order.
    attendees: Vec<usize>,
    /// The components directly inside it, in document order.
    children: Vec<Node>,
    /// The side a zone resolved for this component's `RECURRENCE-ID`.
    side: FoldSide,
}

impl Node {
    /// The component the caller holds: the calendar's first child.
    fn payload(&self) -> &Self {
        self.children.first().unwrap_or(self)
    }

    /// The first line stating `name`.
    fn line(&self, name: &[u8]) -> Option<&Line> {
        self.properties.iter().find(|line| line.is_named(name))
    }

    /// The value of the first line stating `name`.
    fn value(&self, name: &[u8]) -> Option<&[u8]> {
        self.line(name).map(|line| line.value.as_slice())
    }

    /// The reference this component's `RECURRENCE-ID` states, with no side resolved yet.
    fn reference(&self) -> Option<InstanceRef> {
        let line = self.line(b"RECURRENCE-ID")?;
        let named = instant_of(&line.value)?;
        let written = if line.value.last() == Some(&b'Z') {
            InstanceClock::Utc
        } else if line.parameter(b"TZID").is_some() {
            InstanceClock::Zoned
        } else {
            InstanceClock::Floating
        };
        let onwards = line
            .parameter(b"RANGE")
            .is_some_and(|range| range.eq_ignore_ascii_case(b"THISANDFUTURE"));
        let reach = if onwards {
            OverrideRange::ThisAndFuture
        } else {
            OverrideRange::ThisOnly
        };
        Some(InstanceRef::new(named, written, reach))
    }
}

impl ScheduledComponent for Node {
    fn component_kind(&self) -> Option<ComponentKind> {
        self.kind
    }

    fn method(&self) -> Option<&[u8]> {
        self.value(b"METHOD")
    }

    fn uid(&self) -> Option<&[u8]> {
        self.value(b"UID")
    }

    fn sequence(&self) -> SequenceRead {
        match self.value(b"SEQUENCE") {
            None => SequenceRead::Absent,
            Some(digits) => number(digits).map_or(SequenceRead::Unreadable, SequenceRead::Value),
        }
    }

    fn dtstamp(&self) -> Option<Instant> {
        instant_of(self.value(b"DTSTAMP")?)
    }

    fn recurrence_id(&self) -> Option<InstanceRef> {
        Some(self.reference()?.with_side(self.side))
    }

    fn organizer(&self) -> Option<Party<'_>> {
        let line = self.line(b"ORGANIZER")?;
        Some(Party::read(&line.value, line.parameter(b"SENT-BY")))
    }

    fn attendee_count(&self) -> usize {
        self.attendees.len()
    }

    fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
        let line = self.properties.get(*self.attendees.get(index)?)?;
        let mut who = Attendee::new(Party::read(&line.value, line.parameter(b"SENT-BY")));
        if let Some(status) = line.parameter(b"PARTSTAT") {
            who = who.with_part_stat(status);
        }
        if let Some(part) = line.parameter(b"ROLE") {
            who = who.with_role(part);
        }
        if let Some(delegator) = line.parameter(b"DELEGATED-FROM") {
            who = who.with_delegated_from(delegator);
        }
        if let Some(delegate) = line.parameter(b"DELEGATED-TO") {
            who = who.with_delegated_to(delegate);
        }
        Some(who)
    }

    fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
        (index < self.attendees.len()).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
    }

    fn property_count(&self) -> usize {
        self.properties.len()
    }

    fn property_name(&self, index: usize) -> Option<&[u8]> {
        self.properties.get(index).map(|line| line.name.as_slice())
    }

    fn property_line(&self, index: usize) -> Option<&[u8]> {
        self.properties
            .get(index)
            .map(|line| line.content.as_slice())
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

/// The unfolded content lines of `source`, in document order.
fn unfold(source: &[u8]) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for piece in source.split(|byte| *byte == b'\n') {
        let text = piece.strip_suffix(b"\r").unwrap_or(piece);
        let Some((first, rest)) = text.split_first() else {
            continue;
        };
        if matches!(*first, b' ' | b'\t') {
            if let Some(last) = lines.last_mut() {
                last.extend_from_slice(rest);
            }
        } else {
            lines.push(text.to_vec());
        }
    }
    lines
}

/// The calendar `source` spells, with no zone consulted yet.
fn tree(source: &[u8]) -> Node {
    let mut open: Vec<Node> = Vec::new();
    let mut done: Option<Node> = None;
    for content in unfold(source) {
        let Some(line) = Line::read(&content) else {
            continue;
        };
        if line.is_named(b"BEGIN") {
            open.push(Node {
                kind: ComponentKind::from_name(&line.value),
                ..Node::default()
            });
        } else if line.is_named(b"END") {
            let Some(finished) = open.pop() else { continue };
            match open.last_mut() {
                Some(parent) => parent.children.push(finished),
                None => done = Some(finished),
            }
        } else if let Some(current) = open.last_mut() {
            if line.is_named(b"ATTENDEE") {
                current.attendees.push(current.properties.len());
            }
            current.properties.push(line);
        }
    }
    done.unwrap_or_default()
}

/// The same tree with every `RECURRENCE-ID` resolved against `series`.
///
/// This is the composition under test: `ical-itip` resolves no zone of its own, so a caller
/// asks `resolve_instance` and carries the side it answers into the state the gate reads.
fn resolved(
    source: &[u8],
    series: &ZonedSeries<'_, HandZone>,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Node {
    let mut node = tree(source);
    attach(&mut node, series, meter, sink);
    node
}

/// Attach a resolved side to `node` and to every component under it.
fn attach(
    node: &mut Node,
    series: &ZonedSeries<'_, HandZone>,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) {
    if let Some(reference) = node.reference() {
        node.side = resolve_instance(series, reference, meter, sink).side();
    }
    for child in &mut node.children {
        attach(child, series, meter, sink);
    }
}

/// The number `text` spells, or `None` when it is not one.
fn number(text: &[u8]) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut total: u32 = 0;
    for byte in text {
        let digit = char::from(*byte).to_digit(10)?;
        total = total.checked_mul(10)?.checked_add(digit)?;
    }
    Some(total)
}

/// The instant `value` names, projected the way `icalkit_conformance::internal::tz::nominal` projects a wall clock.
fn instant_of(value: &[u8]) -> Option<Instant> {
    let year = u16::try_from(number(value.get(0..4)?)?).ok()?;
    let month = u8::try_from(number(value.get(4..6)?)?).ok()?;
    let day = u8::try_from(number(value.get(6..8)?)?).ok()?;
    if value.get(8) != Some(&b'T') {
        return None;
    }
    let hour = u8::try_from(number(value.get(9..11)?)?).ok()?;
    let minute = u8::try_from(number(value.get(11..13)?)?).ok()?;
    let second = u8::try_from(number(value.get(13..15)?)?).ok()?;
    let civil = CivilDateTime::new(
        CivilDate::from_ymd(year, month, day)?,
        CivilTime::from_hms(hour, minute, second)?,
    );
    nominal(civil)
}

/// What the gate answered about one message against one state, as a comparable value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// The gate authorized it, describing a change of this kind.
    Allowed(TransitionReason),
    /// The gate refused it, naming this reason.
    Refused(AuthorizationDenied),
    /// The message did not read as a scheduling message at all.
    NotAMessage,
}

/// Judge `message` against `held` on behalf of `actor`, under `policy`'s reading of the zone.
///
/// The whole composition in one call: the zone resolves both identities, and the gate is then
/// handed exactly what ADR-0005 gives it — a message, a state, and a party.
fn judge(
    zone: &HandZone,
    tzid: &str,
    policy: ResolutionPolicy,
    fixtures: (&[u8], &[u8], &str),
) -> (Verdict, Vec<DiagnosticCode>) {
    let (state, wire, actor) = fixtures;
    let series = ZonedSeries::new(zone, tzid, policy);
    let (mut meter, mut sink) = ledger();
    let held = resolved(state, &series, &mut meter, &mut sink);
    let calendar = resolved(wire, &series, &mut meter, &mut sink);
    let Ok(message) = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink) else {
        return (Verdict::NotAMessage, codes(&sink));
    };
    let verdict = match evaluate_message(&message, held.payload(), PartyId::new(actor)) {
        Ok(authorized) => Verdict::Allowed(authorized.reason()),
        Err(denied) => Verdict::Refused(denied),
    };
    (verdict, codes(&sink))
}

// -------------------------------------------------------------------------------------------
// Agenda item 2: which instance is "the third one" when one of them falls in a gap.
// -------------------------------------------------------------------------------------------

/// The two readings of a gap disagree about which meeting is the third, which is the premise
/// every case below rests on and is `ical-recur` plus `ical-tz` answering, not `ical-itip`.
///
/// The series is `FREQ=DAILY;COUNT=5` from 02:30 on 2026-03-06 in `America/New_York`. Its third
/// cadence key is 02:30 on March 8th, an hour that zone never showed. ADR-0011's second gate
/// drops it, so the third *delivered* instance is March 9th; without the gate the third is
/// March 8th at the instant RFC 5545 section 3.3.5 reads it.
#[test]
fn the_third_instance_of_a_series_crossing_a_gap_is_two_different_meetings() {
    let zone = new_york();
    let gated = ZonedSeries::new(&zone, NEW_YORK, ResolutionPolicy::DEFAULT);
    let ungated = ZonedSeries::new(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT.with_gaps(GapPolicy::ShiftForward),
    );

    assert_eq!(expansion(&gated).get(2), Some(&key(2026, 3, 9, 2, 30)));
    assert_eq!(expansion(&ungated).get(2), Some(&key(2026, 3, 8, 2, 30)));

    // And the caller that stated the ungated reading has a series that really does have that
    // instance, at exactly one instant: 02:30 EST read with the offset in force before the gap.
    let sprang = key(2026, 3, 8, 2, 30);
    assert!(ungated.admits(sprang), "the caller's own policy admits it");
    assert_eq!(ungated.resolved(sprang), Some(utc(2026, 3, 8, 7, 30)));
    assert!(!gated.admits(sprang), "and the default reading does not");
    assert_eq!(gated.resolved(sprang), None);
}

/// The five cadence keys the series delivers under `series`' own policy.
fn expansion(series: &ZonedSeries<'_, HandZone>) -> Vec<Instant> {
    let (mut meter, mut sink) = ledger();
    let Some(rule) = daily_five(&mut meter, &mut sink) else {
        return Vec::new();
    };
    let admitted = |at: Instant| series.admits(at);
    let anchor = key(2026, 3, 6, 2, 30);
    let Ok(input) = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        OverrideSet::empty(),
        &mut meter,
    ) else {
        return Vec::new();
    };
    let Some(span) = Window::new(key(2026, 3, 1, 0, 0), key(2026, 3, 20, 0, 0)) else {
        return Vec::new();
    };
    let mut emitted = Vec::new();
    for step in input
        .admitting(&admitted)
        .search(span, &mut meter, &mut sink)
    {
        if let SearchStep::Occurrence(occurrence) = step {
            emitted.push(occurrence.key());
        }
    }
    emitted
}

/// The rule the held fixture carries, read out of the fixture and through the grammar rather
/// than rebuilt here, so that the series expanded is the series the caller holds.
fn daily_five(meter: &mut Meter, sink: &mut Vec<Diagnostic>) -> Option<RecurrenceRule> {
    let series = tree(HELD_GAP_SERIES);
    let stated = series.payload().value(b"RRULE")?.to_vec();
    parse_recur(&stated, meter, sink).ok()
}

/// BREAK. `resolve_instance` is handed the `ZonedSeries` the caller built, and the reading of a
/// gap that series carries changes nothing about the answer or the report.
///
/// `ZonedSeries` exists to carry a `ResolutionPolicy`; `ZonedSeries::resolved` and
/// `ZonedSeries::admits` both apply it. `icalkit_conformance::internal::itip::resolve_instance` takes the same value and
/// reaches past the policy to `answer_for`, so an identity in a gap is `FoldSide::Unresolved`
/// under all three readings of `GapPolicy` with an empty sink under all three.
///
/// Both readings are ones RFC 5545 permits, which is exactly why the answer may not be the same
/// value with the same silence: the caller stated which one it wanted and cannot tell from what
/// comes back which one was used.
#[test]
fn a_gap_reading_the_caller_stated_changes_nothing_the_instance_layer_answers() {
    let zone = new_york();
    let sprang = InstanceRef::new(
        key(2026, 3, 8, 2, 30),
        InstanceClock::Zoned,
        OverrideRange::ThisOnly,
    );
    let mut answers = Vec::new();
    for gaps in [
        GapPolicy::Skip,
        GapPolicy::ShiftForward,
        GapPolicy::ClampToTransition,
    ] {
        let series = ZonedSeries::new(&zone, NEW_YORK, ResolutionPolicy::DEFAULT.with_gaps(gaps));
        let (mut meter, mut sink) = ledger();
        let answer = resolve_instance(&series, sprang, &mut meter, &mut sink);
        answers.push((gaps, answer.side(), codes(&sink)));
    }

    let stated = format!("{answers:?}");
    assert_ne!(
        (answers[0].1, answers[0].2.clone()),
        (answers[1].1, answers[1].2.clone()),
        "a gap read as skipped and a gap read as shifted are two different meetings, and \
         nothing that comes back out of the instance layer tells them apart: {stated}"
    );
}

/// BREAK. A `CANCEL` addressed to an instance the caller's own policy places is refused, and
/// refused as *ambiguous* — a claim that two meetings could not be told apart, made about an
/// hour in which the zone showed no meeting at all and the policy named exactly one.
///
/// RFC 5546 section 3.2.5 cancels an instance the recipient holds. Under
/// `GapPolicy::ShiftForward` the recipient holds it, at 07:30Z, and its override says so. The
/// gate answers `AmbiguousInstance` regardless of which reading the caller stated, so the two
/// permitted readings produce one refusal, and the refusal names a condition neither reading
/// produces.
#[test]
fn a_cancel_for_the_instance_in_the_gap_is_refused_as_ambiguous_under_both_readings() {
    let zone = new_york();
    let mut seen = Vec::new();
    for gaps in [
        GapPolicy::Skip,
        GapPolicy::ShiftForward,
        GapPolicy::ClampToTransition,
    ] {
        let policy = ResolutionPolicy::DEFAULT.with_gaps(gaps);
        let answer = judge(
            &zone,
            NEW_YORK,
            policy,
            (HELD_IN_THE_GAP, CANCEL_IN_THE_GAP, CHAIR),
        );
        seen.push((gaps, answer.0, answer.1));
    }
    let stated = format!("{seen:?}");

    assert_eq!(
        seen[1].1,
        Verdict::Allowed(TransitionReason::Cancelled),
        "under `ShiftForward` the caller's own series admits this instance at 07:30Z and its          override says so, and RFC 5546 section 3.2.5 cancels an instance the recipient holds:          {stated}"
    );
    assert_ne!(
        seen[0].1, seen[1].1,
        "and the two readings must not answer the same thing: {stated}"
    );
    assert!(
        !seen[0].2.is_empty(),
        "whichever reading was taken, something must say so: {stated}"
    );
}

/// The other spelling, recorded: a `CANCEL` written in UTC names the instant the shifted
/// instance happens at, and reaches a different cadence key.
///
/// `ZonedSeries::to_nominal` projects 07:30Z through the offset in force *at* 07:30Z, which is
/// the one the transition installed, so the identity it lands on is 03:30 local and not the
/// 02:30 the series is written at. Together with the case above this leaves the instance
/// unaddressable under the reading that has it: the wall-clock spelling is refused as ambiguous
/// and the UTC spelling names its neighbor.
#[test]
fn the_utc_spelling_of_the_same_instance_lands_on_a_different_cadence_key() {
    let zone = new_york();
    let ungated = ResolutionPolicy::DEFAULT.with_gaps(GapPolicy::ShiftForward);
    let (verdict, reported) = judge(
        &zone,
        NEW_YORK,
        ungated,
        (HELD_IN_THE_GAP, CANCEL_IN_THE_GAP_IN_UTC, CHAIR),
    );
    assert_eq!(
        verdict,
        Verdict::Refused(AuthorizationDenied::NoMatchingInstance),
        "{reported:?}"
    );
}

/// The control: the instance the *gated* reading counts third is outside the gap, and a `CANCEL`
/// addressed to it is authorized under both readings. Nothing about the composition is broken in
/// general — only where a gap is involved.
#[test]
fn a_cancel_for_the_instance_after_the_gap_is_authorized_under_either_reading() {
    let zone = new_york();
    for gaps in [GapPolicy::Skip, GapPolicy::ShiftForward] {
        let policy = ResolutionPolicy::DEFAULT.with_gaps(gaps);
        let (verdict, reported) = judge(
            &zone,
            NEW_YORK,
            policy,
            (HELD_AFTER_THE_GAP, CANCEL_AFTER_THE_GAP, CHAIR),
        );
        assert_eq!(
            verdict,
            Verdict::Allowed(TransitionReason::Cancelled),
            "{gaps:?}"
        );
        assert!(reported.is_empty(), "{gaps:?}: {reported:?}");
    }
}

/// The fold, for contrast, and the reason it is not reported as the same defect: a wall clock in
/// the repeated hour is genuinely two meetings in the file, the gate refuses, and
/// `scheduling-instance-ambiguous` travels — so the caller is told why.
///
/// `FoldPolicy::Later` is likewise discarded by `resolve_instance`, but the refusal that follows
/// carries a report naming the condition, which is what the gap case does not.
#[test]
fn a_cancel_for_the_folded_hour_is_refused_with_the_ambiguity_reported() {
    let zone = new_york();
    for folds in [FoldPolicy::Earlier, FoldPolicy::Later] {
        let policy = ResolutionPolicy::DEFAULT.with_folds(folds);
        let (verdict, reported) =
            judge(&zone, NEW_YORK, policy, (HELD_FOLDED, CANCEL_FOLDED, CHAIR));
        assert_eq!(
            verdict,
            Verdict::Refused(AuthorizationDenied::AmbiguousInstance),
            "{folds:?}"
        );
        assert_eq!(
            reported,
            vec![
                DiagnosticCode::SchedulingInstanceAmbiguous,
                DiagnosticCode::SchedulingInstanceAmbiguous,
            ],
            "{folds:?}: the refusal is reported, once per identity resolved"
        );
    }
}

// -------------------------------------------------------------------------------------------
// Agenda item 4: an answer continued past the end of a transition table.
// -------------------------------------------------------------------------------------------

/// A message about a series whose zone answer is continued six years past its table is judged,
/// authorized, and reported as continued — the note travels and the distance is recoverable.
#[test]
fn a_message_about_a_continued_zone_answer_is_judged_and_says_that_it_was_continued() {
    let zone = finite_table();
    let series = ZonedSeries::new(&zone, EXCHANGE, ResolutionPolicy::DEFAULT);
    let (mut meter, mut sink) = ledger();
    let named = InstanceRef::new(
        key(2035, 6, 15, 12, 0),
        InstanceClock::Zoned,
        OverrideRange::ThisOnly,
    );
    let answer = resolve_instance(&series, named, &mut meter, &mut sink);
    assert!(answer.is_continued());
    assert_eq!(answer.nearest_known(), CivilDate::from_ymd(2029, 10, 28));
    assert_eq!(answer.side(), FoldSide::Once);
    assert_eq!(codes(&sink), vec![DiagnosticCode::SchedulingZoneContinued]);
    assert_eq!(sink[0].severity(), Severity::Note);

    let (verdict, reported) = judge(
        &zone,
        EXCHANGE,
        ResolutionPolicy::DEFAULT,
        (HELD_CONTINUED, CANCEL_CONTINUED, CHAIR),
    );
    assert_eq!(verdict, Verdict::Allowed(TransitionReason::Cancelled));
    assert_eq!(
        reported,
        vec![
            DiagnosticCode::SchedulingZoneContinued,
            DiagnosticCode::SchedulingZoneContinued,
        ],
        "one per identity the composition resolved, and the decision rested on both"
    );
}

// -------------------------------------------------------------------------------------------
// Agenda item 3: an exclusion no zone could place.
// -------------------------------------------------------------------------------------------

/// A `Z`-terminated `EXDATE` on a series whose `TZID` nothing recognizes makes the series
/// undecidable, and the precondition says so before the gate is asked anything.
///
/// The second half of this case is the composition's own hole, recorded rather than asserted
/// away: `evaluate_message` is handed no zone and no exclusion list, so it authorizes the
/// `CANCEL` whether or not the caller ran the precondition. `instance.rs` states that as a
/// caller obligation; nothing in the type system holds a caller to it.
#[test]
fn an_exclusion_no_zone_could_place_is_reported_and_the_gate_never_sees_it() {
    let zone = new_york();
    let stranger = ZonedSeries::new(&zone, "Customized Time Zone", ResolutionPolicy::DEFAULT);
    let (mut meter, mut noise) = ledger();
    let exclusions = ResolvedExclusions::read(
        &stranger,
        ValueType::DateTime,
        &[DateTimeValue::Utc(
            clock(2026, 7, 1, 13, 0).expect("a real wall clock"),
        )],
        &mut meter,
        &mut noise,
    );
    assert_eq!(exclusions.unplaced(), [utc(2026, 7, 1, 13, 0)]);
    assert_eq!(codes(&noise), vec![DiagnosticCode::ExdateZoneUnknown]);

    let mut reported: Vec<Diagnostic> = Vec::new();
    assert!(!check_exclusions_are_placeable(
        &exclusions,
        &mut meter,
        &mut reported
    ));
    assert_eq!(
        codes(&reported),
        vec![DiagnosticCode::SchedulingExclusionUnplaced]
    );

    let (verdict, _) = judge(
        &zone,
        "Customized Time Zone",
        ResolutionPolicy::DEFAULT,
        (HELD_UNPLACEABLE, CANCEL_UNPLACEABLE, CHAIR),
    );
    assert_eq!(
        verdict,
        Verdict::Allowed(TransitionReason::Cancelled),
        "the gate holds no zone, so the precondition above is the caller's to run"
    );
}

// -------------------------------------------------------------------------------------------
// RFC 6047 section 2.5: the envelope is not the object.
// -------------------------------------------------------------------------------------------

/// The `From` of the mail that carried a message is whatever the sender typed, and the gate
/// judges the address the caller hands it. A caller that hands over the envelope's `From` gets a
/// refusal for a party the object does not name; a caller that hands over the `ORGANIZER` the
/// object itself names gets an authorization for a message anybody could have sent.
///
/// The second half is not a defect in the gate — for a first `REQUEST` the state names nobody
/// and `SECURITY.md` says the claim rests on the transport — but it is the whole of why the two
/// addresses may never be treated as one, so it is pinned here as evidence rather than prose.
#[test]
fn the_envelope_sender_and_the_organizer_the_object_names_are_two_different_claims() {
    let zone = new_york();
    let nothing: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\n\
                           END:VCALENDAR\r\n";
    let (forged, _) = judge(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT,
        (nothing, REQUEST_FROM_A_STRANGER, STRANGER),
    );
    assert_eq!(
        forged,
        Verdict::Refused(AuthorizationDenied::OrganizerMismatch),
        "the address the mail came from is on nothing this object names"
    );

    let (claimed, _) = judge(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT,
        (nothing, REQUEST_FROM_A_STRANGER, CHAIR),
    );
    assert_eq!(
        claimed,
        Verdict::Allowed(TransitionReason::Created),
        "a first message proves only that the actor is a party the message names"
    );

    // And against state the caller already holds, the organizer line the recipient already has
    // is the one that decides — a forged `ORGANIZER` in the message buys nothing.
    let (against_state, _) = judge(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT,
        (HELD_PLAIN, REQUEST_FROM_A_STRANGER, CHAIR),
    );
    assert_eq!(
        against_state,
        Verdict::Refused(AuthorizationDenied::UidMismatch),
        "and a message about another identity is not a message about this one"
    );
}

/// The attendee side of the same claim: an address that replies is judged against the list the
/// recipient holds and not against the list the message carries.
#[test]
fn an_address_the_recipient_does_not_carry_cannot_reply_however_the_message_names_it() {
    let zone = new_york();
    let forged: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\n\
                          UID:plain-series@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
                          ORGANIZER:mailto:chair@example.com\r\n\
                          ATTENDEE;PARTSTAT=ACCEPTED:mailto:zz@example.com\r\nSEQUENCE:2\r\n\
                          END:VEVENT\r\nEND:VCALENDAR\r\n";
    let (verdict, _) = judge(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT,
        (HELD_PLAIN, forged, STRANGER),
    );
    assert_eq!(
        verdict,
        Verdict::Refused(AuthorizationDenied::UnknownAttendee)
    );

    let honest: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\n\
                          UID:plain-series@example.test\r\nDTSTAMP:20260302T080000Z\r\n\
                          ORGANIZER:mailto:chair@example.com\r\n\
                          ATTENDEE;PARTSTAT=ACCEPTED:mailto:bo@example.com\r\nSEQUENCE:2\r\n\
                          END:VEVENT\r\nEND:VCALENDAR\r\n";
    let (invited, _) = judge(
        &zone,
        NEW_YORK,
        ResolutionPolicy::DEFAULT,
        (HELD_PLAIN, honest, BO),
    );
    assert_eq!(
        invited,
        Verdict::Allowed(TransitionReason::ParticipationChanged),
        "and the party the recipient does carry may answer, which is the control"
    );
}
