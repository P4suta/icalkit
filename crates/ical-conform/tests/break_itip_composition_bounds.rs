// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BREAK. What a scheduling message costs to judge, against what ADR-0010 says it may cost.
//!
//! `ItipMessage` is documented as meaning *already checked and already charged*, and the design
//! document gives that as the reason `evaluate_message` takes no ledger: "by the time a message
//! exists its cardinality is already bounded". Every case here hands the crate a message that
//! reads, then measures what the gate does with it and what the shared ledger was told about
//! it. A case that does not return inside the nextest timeout is a hang and is reported as one.
//!
//! The subjects are generated rather than committed. A hundred thousand `ATTENDEE` lines is four
//! megabytes of fixture that says nothing the loop does not, and the fan-out below cannot be
//! written as a file at all — it is what a `ScheduledComponent` implemented over a store that
//! shares rows produces, which is the implementation the trait exists to admit.

use std::time::Instant as WallClock;

use ical_core::{ComponentKind, Diagnostic, Instant, Limits, Meter, ProposedChange};
use ical_itip::{
    Attendee, AuthorizationDenied, InstanceRef, ItipMessage, MessageError, Party, PartyId,
    PropertyOccurrence, ScheduledComponent, SequenceRead, TransitionReason, evaluate_message,
};

/// The organizer every generated message names.
const CHAIR: &str = "mailto:chair@example.com";
/// The one attendee every generated message names.
const BO: &str = "mailto:bo@example.test";

/// What the gate answered, as a label a table can compare.
fn label(answer: Result<ical_itip::Authorization<'_>, AuthorizationDenied>) -> &'static str {
    match answer {
        Ok(authorized) if authorized.reason() == TransitionReason::CounterProposed => {
            "counter-proposed"
        },
        Ok(_) => "allowed: something else",
        Err(AuthorizationDenied::MethodForbidsField(at)) if at.name() == b"DTSTART" => {
            "refused: DTSTART is the organizer's"
        },
        Err(_) => "refused: something else",
    }
}

// -------------------------------------------------------------------------------------------
// A generated component.
// -------------------------------------------------------------------------------------------

/// One property, as the two things the trait hands back about it.
#[derive(Clone, Debug)]
struct Line {
    /// The upper-cased name.
    name: Vec<u8>,
    /// The whole content line, unfolded and unterminated.
    content: Vec<u8>,
    /// The value alone, which is all the party accessors need.
    value: Vec<u8>,
}

impl Line {
    /// The line `name` and `value` spell.
    fn new(name: &[u8], value: &[u8]) -> Self {
        let mut content = name.to_vec();
        content.push(b':');
        content.extend_from_slice(value);
        Self {
            name: name.to_ascii_uppercase(),
            content,
            value: value.to_vec(),
        }
    }
}

/// A component assembled in memory, with every accessor constant time.
///
/// Constant time matters: a subject whose own `attendee` is a linear scan would make every
/// measurement below a measurement of this file.
#[derive(Debug, Default)]
struct Made {
    /// What the component is.
    kind: Option<ComponentKind>,
    /// Its properties, in document order.
    properties: Vec<Line>,
    /// Which of those are `ATTENDEE` lines.
    attendees: Vec<usize>,
    /// Its children, in document order.
    children: Vec<Made>,
}

impl Made {
    /// A component of `kind` with nothing on it.
    fn of(kind: ComponentKind) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }

    /// The same component with one more property.
    fn with(mut self, name: &[u8], value: &[u8]) -> Self {
        if name.eq_ignore_ascii_case(b"ATTENDEE") {
            self.attendees.push(self.properties.len());
        }
        self.properties.push(Line::new(name, value));
        self
    }

    /// The same component with one more child.
    fn nesting(mut self, child: Self) -> Self {
        self.children.push(child);
        self
    }

    /// The value of the first line named `name`.
    fn value(&self, name: &[u8]) -> Option<&[u8]> {
        self.properties
            .iter()
            .find(|line| line.name.as_slice() == name)
            .map(|line| line.value.as_slice())
    }
}

impl ScheduledComponent for Made {
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
        let Some(digits) = self.value(b"SEQUENCE") else {
            return SequenceRead::Absent;
        };
        let mut total: u32 = 0;
        for byte in digits {
            let Some(digit) = char::from(*byte).to_digit(10) else {
                return SequenceRead::Unreadable;
            };
            total = total.saturating_mul(10).saturating_add(digit);
        }
        SequenceRead::Value(total)
    }

    fn dtstamp(&self) -> Option<Instant> {
        Some(Instant::from_unix_seconds(1_772_000_000))
    }

    fn recurrence_id(&self) -> Option<InstanceRef> {
        None
    }

    fn organizer(&self) -> Option<Party<'_>> {
        Some(Party::read(self.value(b"ORGANIZER")?, None))
    }

    fn attendee_count(&self) -> usize {
        self.attendees.len()
    }

    fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
        let line = self.properties.get(*self.attendees.get(index)?)?;
        Some(Attendee::new(Party::read(&line.value, None)))
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

/// A `VCALENDAR` carrying `method` and one payload.
fn message(method: &[u8], payload: Made) -> Made {
    Made::of(ComponentKind::Calendar)
        .with(b"VERSION", b"2.0")
        .with(b"METHOD", method)
        .nesting(payload)
}

/// The skeleton of a `REQUEST` payload, before whatever a case piles onto it.
fn invitation(uid: &[u8]) -> Made {
    Made::of(ComponentKind::Event)
        .with(b"UID", uid)
        .with(b"DTSTAMP", b"20260302T080000Z")
        .with(b"DTSTART", b"20260410T140000Z")
        .with(b"SUMMARY", b"Budget review")
        .with(b"SEQUENCE", b"0")
        .with(b"ORGANIZER", CHAIR.as_bytes())
        .with(b"ATTENDEE", b"mailto:bo@example.test")
}

// -------------------------------------------------------------------------------------------
// The bounds.
// -------------------------------------------------------------------------------------------

/// A hundred thousand `ATTENDEE` lines is refused whole, at the policy's own ceiling, and the
/// refusal is an `Err` rather than a truncated list — which is `message.rs`'s stated departure
/// from ADR-0009 and the one that keeps an attacker from choosing the authorization answer.
#[test]
fn a_request_with_a_hundred_thousand_attendees_is_refused_rather_than_shortened() {
    let mut payload = invitation(b"crowd@example.test");
    for index in 0..100_000_u32 {
        let address = format!("mailto:a{index}@example.test");
        payload = payload.with(b"ATTENDEE", address.as_bytes());
    }
    let calendar = message(b"REQUEST", payload);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let started = WallClock::now();
    let answer = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink);
    assert_eq!(answer.err(), Some(MessageError::TooManyAttendees));
    assert!(
        started.elapsed().as_secs() < 30,
        "the refusal is not a walk"
    );
}

/// A component that reports the same child at every index is a tree with more nodes than there
/// are atoms, and the walk that charges it returns anyway: an explicit stack, a depth ceiling
/// and a width ceiling, all three read from the caller's own policy.
#[test]
fn a_fan_out_that_shares_one_child_at_every_index_is_refused_and_not_walked() {
    let payload = fan_out(6, 512);
    let calendar = message(b"REQUEST", payload);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let started = WallClock::now();
    let answer = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink);
    assert_eq!(answer.err(), Some(MessageError::TooManyComponents));
    assert!(
        started.elapsed().as_secs() < 30,
        "512^6 components were counted rather than visited"
    );
}

/// A component whose child is itself is a cycle, which the trait's contract does not forbid and
/// a store that shares rows can produce. The depth ceiling ends it.
#[test]
fn a_component_that_is_its_own_child_terminates_at_the_depth_ceiling() {
    let calendar = Ouroboros;
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let started = WallClock::now();
    let answer = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink);
    assert_eq!(answer.err(), Some(MessageError::TooDeep));
    assert!(
        started.elapsed().as_secs() < 30,
        "a cycle is not a loop here"
    );
}

/// Nesting deeper than the caller's policy is `TooDeep`, and the ladder is built and dropped
/// iteratively so that what is measured is the crate's walk rather than this file's.
#[test]
fn nesting_past_the_depth_ceiling_is_refused() {
    let mut payload = invitation(b"deep@example.test");
    for _ in 0..64 {
        payload = Made::of(ComponentKind::Event)
            .with(b"UID", b"deep@example.test")
            .with(b"DTSTAMP", b"20260302T080000Z")
            .nesting(payload);
    }
    let calendar = message(b"REQUEST", payload);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let answer = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink);
    assert_eq!(answer.err(), Some(MessageError::TooDeep));
}

/// A counter-proposal chain a thousand deep terminates, one evaluation per round, with the
/// answer RFC 5546 section 3.2.7 gives every one of them.
#[test]
fn a_counter_chain_a_thousand_deep_terminates_with_a_reported_outcome() {
    let held = invitation(b"chain@example.test");
    let mut answers: Vec<&'static str> = Vec::new();
    let started = WallClock::now();
    for round in 0..1_000_u32 {
        let proposal = Made::of(ComponentKind::Event)
            .with(b"UID", b"chain@example.test")
            .with(b"DTSTAMP", b"20260302T080000Z")
            .with(
                b"DTSTART",
                format!("2026041{}T140000Z", round % 10).as_bytes(),
            )
            .with(b"SUMMARY", b"Budget review")
            .with(b"SEQUENCE", b"0")
            .with(b"ORGANIZER", CHAIR.as_bytes())
            .with(b"ATTENDEE", b"mailto:bo@example.test");
        let calendar = message(b"COUNTER", proposal);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let answer = match ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink) {
            Err(_) => "unread",
            Ok(read) => label(evaluate_message(&read, &held, PartyId::new(BO))),
        };
        if !answers.contains(&answer) {
            answers.push(answer);
        }
    }
    answers.sort_unstable();
    assert_eq!(
        answers,
        vec!["counter-proposed", "refused: DTSTART is the organizer's"],
        "every round of the chain answered: the round that proposes the time already held          changes nothing, and every round that moves it is refused"
    );
    assert!(started.elapsed().as_secs() < 60);
}

/// The cardinality that is "already bounded" by the time a message exists now includes the
/// number of properties on it, which is the cardinality a judgment is proportional to.
///
/// This case found the opposite and is kept as the regression it became. `ItipMessage::read`
/// charged one unit per component and one per attendee, charged nothing per property and
/// checked no property count against `Limits`, so a payload of a hundred thousand lines read
/// for four units and was then described as a hundred thousand `ProposedChange`s — allocated,
/// sorted into a `BTreeMap`, and handed back — by an `evaluate_message` that has no ledger to
/// refuse on. ADR-0010's whole claim is that five thousand individually bounded messages are
/// bounded in aggregate against one meter, and that walked the work past the meter entirely.
///
/// Three rows, and they are the three answers a bound owes: a payload past the ceiling is
/// refused whole and at the policy's own number, a payload inside the ceiling is charged for
/// every line it carries so a ledger that has run out refuses it, and the ordinary message the
/// two are measured against still reads and is still judged.
#[test]
fn a_message_of_a_hundred_thousand_properties_is_refused_at_the_ceiling_and_charged_below_it() {
    const LINES: u32 = 100_000;
    let ceiling = usize::try_from(Limits::DEFAULT.max_payload_properties()).unwrap_or(usize::MAX);
    assert!(
        ceiling < usize::try_from(LINES).unwrap_or(usize::MAX),
        "the fixture has to exceed the policy"
    );

    let mut enormous = invitation(b"enormous@example.test");
    for index in 0..LINES {
        enormous = enormous.with(format!("X-PAD-{index}").as_bytes(), b"padding");
    }
    let calendar = message(b"REQUEST", enormous);
    let held = Made::default();

    // A ledger with almost nothing left in it, and a message whose description would cost a
    // hundred thousand allocations. Refused at read, on the policy's ceiling, before anything
    // is described.
    let started = WallClock::now();
    let mut meter = Meter::with_budget(Limits::DEFAULT, 16);
    let mut sink: Vec<Diagnostic> = Vec::new();
    assert_eq!(
        ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink).err(),
        Some(MessageError::TooManyProperties),
        "a payload past the ceiling is refused whole rather than described in full"
    );

    // Inside the ceiling, the ledger is what refuses: every property is charged, so sixteen
    // units do not buy a description of two thousand lines.
    let mut ordinary = invitation(b"charged@example.test");
    for index in 0..2_000_u32 {
        ordinary = ordinary.with(format!("X-PAD-{index}").as_bytes(), b"padding");
    }
    let calendar = message(b"REQUEST", ordinary);
    let mut meter = Meter::with_budget(Limits::DEFAULT, 16);
    assert_eq!(
        ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink).err(),
        Some(MessageError::BudgetExhausted),
        "the work of judging a message is charged to the ledger that bounds the inbox"
    );

    // And the control: the same message against a ledger that can pay for it reads, is judged,
    // and describes what it says.
    let mut meter = Meter::new(Limits::DEFAULT);
    let read = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink)
        .expect("a message a caller has budget for is read");
    let authorized = evaluate_message(&read, &held, PartyId::new(CHAIR))
        .expect("the organizer the message names may create it");
    assert_eq!(authorized.reason(), TransitionReason::Created);
    assert!(
        matches!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"X-PAD-1999", 0)),
            Some(ProposedChange::Add(_))
        ),
        "and the last of them is in there"
    );
    assert!(started.elapsed().as_secs() < 60);
}

/// The same shape read the way an inbox reads one: one ledger, many messages. ADR-0010 says
/// this is the amplification the meter exists to bound, and the meter never moves.
#[test]
fn a_thousand_such_messages_share_one_ledger_and_never_exhaust_it() {
    const LINES: u32 = 2_000;
    const MESSAGES: u32 = 1_000;
    /// The whole ledger one inbox is given.
    const BUDGET: u32 = 64;

    let mut payload = invitation(b"amplified@example.test");
    for index in 0..LINES {
        payload = payload.with(format!("X-PAD-{index}").as_bytes(), b"padding");
    }
    let calendar = message(b"REQUEST", payload);
    let held = Made::default();
    let mut meter = Meter::with_budget(Limits::DEFAULT, u64::from(BUDGET));
    let mut sink: Vec<Diagnostic> = Vec::new();
    let mut described = 0_u64;
    let mut admitted = 0_u32;
    let started = WallClock::now();
    for _ in 0..MESSAGES {
        let Ok(read) = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut sink) else {
            break;
        };
        let Ok(authorized) = evaluate_message(&read, &held, PartyId::new(CHAIR)) else {
            break;
        };
        admitted = admitted.saturating_add(1);
        described =
            described.saturating_add(u64::try_from(authorized.transition().len()).unwrap_or(0));
    }
    let elapsed = started.elapsed();
    assert!(
        described <= u64::from(BUDGET) * 64,
        "ADR-0010: a ledger of {BUDGET} units admitted {admitted} messages and          {described} described changes in {elapsed:?}. The ledger bounds how many messages are          read; it does not bound what judging one of them costs, and the ratio is the          attacker's to choose"
    );
}

/// A component whose only child is itself.
///
/// The trait says `child(index)` answers "the `index`th nested component" and says nothing that
/// forbids a graph. Nothing here is malformed; a store that hands out shared rows produces it.
#[derive(Debug)]
struct Ouroboros;

impl ScheduledComponent for Ouroboros {
    fn component_kind(&self) -> Option<ComponentKind> {
        Some(ComponentKind::Calendar)
    }

    fn method(&self) -> Option<&[u8]> {
        Some(b"REQUEST")
    }

    fn uid(&self) -> Option<&[u8]> {
        Some(b"cycle@example.test")
    }

    fn sequence(&self) -> SequenceRead {
        SequenceRead::Absent
    }

    fn dtstamp(&self) -> Option<Instant> {
        None
    }

    fn recurrence_id(&self) -> Option<InstanceRef> {
        None
    }

    fn organizer(&self) -> Option<Party<'_>> {
        None
    }

    fn attendee_count(&self) -> usize {
        0
    }

    fn attendee(&self, _index: usize) -> Option<Attendee<'_>> {
        None
    }

    fn attendee_occurrence(&self, _index: usize) -> Option<PropertyOccurrence> {
        None
    }

    fn property_count(&self) -> usize {
        0
    }

    fn property_name(&self, _index: usize) -> Option<&[u8]> {
        None
    }

    fn property_line(&self, _index: usize) -> Option<&[u8]> {
        None
    }

    fn child_count(&self) -> usize {
        1
    }

    fn child(&self, _index: usize) -> Option<&dyn ScheduledComponent> {
        Some(self)
    }
}

/// A component `levels` deep that reports `breadth` children at every level, all of them the
/// same value.
///
/// `breadth.pow(levels)` nodes for `levels` allocations. Written as one owned chain because a
/// borrow cannot point at a node that does not exist.
fn fan_out(levels: usize, breadth: usize) -> Made {
    let mut node = Made::of(ComponentKind::Event)
        .with(b"UID", b"fanned@example.test")
        .with(b"DTSTAMP", b"20260302T080000Z")
        .with(b"ORGANIZER", CHAIR.as_bytes());
    for _ in 0..levels {
        node = Made {
            kind: Some(ComponentKind::Event),
            properties: Vec::new(),
            attendees: Vec::new(),
            children: (0..breadth).map(|_| Made::default()).collect(),
        }
        .nesting(node);
    }
    node
}
