// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The bridge from `ical-core`'s model to the protocol's questions.
//!
//! Specification: RFC 5545 section 3.8.4.1 (`ATTENDEE`), section 3.8.4.3 (`ORGANIZER`),
//! section 3.8.4.4 (`RECURRENCE-ID`), section 3.2.13 (`RANGE`), section 3.8.7.2 (`DTSTAMP`),
//! section 3.8.7.4 (`SEQUENCE`); RFC 6868 section 2 for the caret encoding this module is the
//! one reader of.
//!
//! [`ScheduledComponent`] is what the rest of this crate reads a calendar through, and
//! [`ScheduledView`] is the implementation for a caller that holds an
//! [`ical_core::Component`]. One walk builds it and every later question is a lookup, which is
//! what the trait asks for: a diff walks every property of both sides, so nothing on that path
//! may allocate per call.
//!
//! # Why this is a value and not a bare implementation on `Component`
//!
//! Two of the trait's answers are *derived* octets rather than stored ones, and both are
//! returned by reference:
//!
//! - [`ScheduledComponent::property_line`] hands back a whole content line, because that is the
//!   unit [`ical_core::ProposedChange::Replace`] takes. A [`ical_core::Component`] stores a
//!   name, an ordered parameter list and a value separately, with the producer's folds recorded
//!   beside them; the line as one span of octets exists nowhere in the tree.
//! - Every parameter value handed to [`Party`] or [`Attendee`] is a **value** and not a
//!   spelling, so RFC 6868's caret encoding is resolved first. Resolving `^'` into `"` produces
//!   octets the file does not contain.
//!
//! A method returning `&[u8]` can only hand back octets something already owns, and a
//! `Component` owns neither of these. So the derivation is done once, into storage this type
//! owns, and the borrows point into it. That is the cost the trait was introduced to make
//! payable — `ScheduledComponent` exists precisely so that state which is not a `Component`
//! can answer these questions — and it is charged once per component rather than once per
//! question.
//!
//! # Values, not spellings
//!
//! The contract [`crate::party`] states from the other end. `SENT-BY`, `PARTSTAT`, `ROLE`,
//! `DELEGATED-FROM` and `DELEGATED-TO` are read as *values*: the section 3.2 `DQUOTE` pair is
//! removed and then [`ical_core::decode_caret`] is applied, in that order, because that is the
//! order a writer applies them in and a reader has to undo them in the reverse one.
//! [`ical_core::ParameterEdit`] takes a value and picks the spelling itself, so a value that
//! skipped the decode here would be written back as `^^'` where the file had `^'` — a
//! corruption no other gate in this workspace catches, which is why there is a test for exactly
//! that round trip below.
//!
//! # Present and unusable is an answer
//!
//! A name RFC 5545 declares at most once, arriving more than once with **two different lines**,
//! is not one value — [`ical_core::Component::get`] reports that as `View::Malformed` and this
//! reports it in the only vocabulary the trait has. `UID`, `METHOD`, `DTSTAMP`, `RECURRENCE-ID`
//! and `ORGANIZER` answer `None`, which refuses the message rather than picking a winner out of
//! two. `SEQUENCE` has a third state of its own and answers [`SequenceRead::Unreadable`], which
//! is not [`SequenceRead::Absent`]: an absent `SEQUENCE` is revision zero and an unreadable one
//! is no revision at all.
//!
//! Two occurrences whose whole content lines are byte-identical are **one statement written
//! twice**, and they answer with that statement. Refusing them was the more conservative
//! reading of the two and it was not the safer one: a stored copy whose `UID` line is
//! duplicated — which ADR-0001's lossless reading preserves, and which any producer can leave
//! behind — then reported as a component the caller does not hold, and a message about it was
//! judged against itself instead of against the recipient's `ORGANIZER` line. There is no
//! winner to pick between two identical claims, and reading one is not a guess.
//!
//! A `RECURRENCE-ID` that is present and does not decode is the sharpest of these. Answering
//! `None` would make a message about one instance look like a message about the whole series,
//! which is how a `CANCEL` for Tuesday cancels the year. It answers an instance reference whose
//! fold side is [`crate::FoldSide::Unresolved`] instead, and an unresolved side can never
//! compare [`crate::InstanceMatch::Same`] — so the gate above denies rather than guesses.
//!
//! # A wall clock is the series' clock, whether or not the file repeats the zone
//!
//! A `RECURRENCE-ID` written with a trailing `Z` names an instant and falls in no fold. Every
//! other spelling names a wall clock, and this crate resolves no zone, so both the `TZID` form
//! and the bare form answer [`crate::FoldSide::Unresolved`] and a caller holding the zone
//! attaches a side with [`crate::resolve_instance`]. The bare form used to answer
//! [`crate::FoldSide::Once`] on the reading that a floating value projects onto the nominal
//! timeline as itself. That is true of a series that runs on no zone and false of the value
//! several producers actually emit — a bare override of a *zoned* series — and the difference
//! was one reply answering both halves of a repeated hour, which the zoned spelling of the same
//! pair was already refused for.
//!
//! # Names are normalized and lines are not
//!
//! [`ScheduledComponent::property_name`] answers the ASCII-uppercased name, while
//! [`ScheduledComponent::property_line`] reproduces the octets the producer wrote. The asymmetry
//! is deliberate and it is a security property: RFC 5546 section 3's tables are counted per
//! name, and a table row of `1` counted over the octets as written admits `DTSTART` beside
//! `dtstart` as two names appearing once each. Normalizing the name closes that, and it is also
//! what makes an occurrence found by a diff address the same line
//! [`ical_core::Component::apply_to_occurrence`] writes, since that door counts by identity.
//! The line stays as written because ADR-0001 says the octets are the producer's.

use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::mem;

use ical_core::{
    CivilDateTime, CivilTime, Component, ComponentKind, DateTimeValue, DecodeValue, Instant,
    Property, PropertyId, RawText, decode_caret,
};
use ical_recur::OverrideRange;
use ical_tz::nominal;

use crate::identity::{FoldSide, InstanceClock, InstanceRef, SequenceRead};
use crate::party::{ANSWERED_AT, Attendee, Party};
use crate::state::{PropertyOccurrence, ScheduledComponent};

/// The RFC 6868-resolved parameter values one `ORGANIZER` or `ATTENDEE` line states.
///
/// Owned, because resolving a caret pair produces octets the file does not hold. Only the five
/// parameters the scheduling vocabulary reads are kept; every other parameter on the line stays
/// where it is and is written back untouched, which is what makes a reply preserve the
/// recipient's own `X-` parameters.
#[derive(Debug)]
struct PartyValues {
    /// The `SENT-BY` value, absent when the line states none.
    sent_by: Option<RawText>,
    /// The `PARTSTAT` value, absent when the line states none.
    part_stat: Option<RawText>,
    /// The `ROLE` value, absent when the line states none.
    role: Option<RawText>,
    /// The `DELEGATED-FROM` value, absent when the line states none.
    delegated_from: Option<RawText>,
    /// The `DELEGATED-TO` value, absent when the line states none.
    delegated_to: Option<RawText>,
    /// When this party's own answer was written, absent when the line records no time.
    answered_at: Option<Instant>,
}

/// One property, with the two things a scheduling decision needs and the tree does not store.
#[derive(Debug)]
struct PropertyLine<'a> {
    /// The property itself, for the octets it does store.
    property: &'a Property,
    /// The name, ASCII-uppercased. See this module's own documentation for why.
    name: RawText,
    /// The whole content line, unfolded, with no terminator.
    line: RawText,
    /// The party values, present only for an `ORGANIZER` or an `ATTENDEE`.
    values: Option<PartyValues>,
}

impl<'a> PropertyLine<'a> {
    /// Read everything this crate will later want to know about `property`.
    fn read(property: &'a Property) -> Self {
        Self {
            property,
            name: uppercased(property.name().as_bytes()),
            line: RawText::from_vec(content_line(property)),
            values: party_values(property),
        }
    }
}

/// One [`ical_core::Component`], read as the state a scheduling message is judged against.
///
/// Built once with [`ScheduledView::of`] and then only looked up. It borrows the component, so
/// the octets a value keeps are not copied twice; what it owns is exactly what the component
/// does not store — the reconstructed content lines and the resolved parameter values.
///
/// A caller that holds a `Component` builds one of these and hands it to
/// [`crate::evaluate_message`]; a caller whose state is a database row implements
/// [`ScheduledComponent`] against its rows and never builds one at all.
pub struct ScheduledView<'a> {
    /// The component this is a reading of.
    component: &'a Component,
    /// Its own properties, in document order.
    lines: Vec<PropertyLine<'a>>,
    /// Which of `lines` is the `ORGANIZER`, absent when there is not exactly one.
    organizer: Option<usize>,
    /// Which of `lines` are `ATTENDEE`s, in document order.
    attendees: Vec<usize>,
    /// The components nested directly inside, in document order.
    children: Vec<ScheduledView<'a>>,
}

/// One component whose nested components are still being read.
///
/// The explicit stack [`ScheduledView::of`] walks with. Recursion here would be bounded by
/// [`ical_core::Limits::max_component_depth`], which is a `u16` a caller raises through a public
/// builder while the stack gives out several thousand frames sooner — and a stack overflow is an
/// abort rather than an unwind, so a server that read an untrusted attachment loses the process
/// instead of the request. `ical-core`'s own tree walks are written this way for the same
/// reason.
#[derive(Debug)]
struct Frame<'a> {
    /// The component being read.
    component: &'a Component,
    /// Its nested components, in document order.
    nested: Vec<&'a Component>,
    /// How many of them have been read.
    taken: usize,
    /// What reading them produced, in the same order.
    built: Vec<ScheduledView<'a>>,
}

impl<'a> Frame<'a> {
    /// A frame that has read none of `component`'s nested components yet.
    fn open(component: &'a Component) -> Self {
        Self {
            component,
            nested: component.components().collect(),
            taken: 0,
            built: Vec::new(),
        }
    }

    /// The next nested component to read, `None` once they have all been taken.
    fn next_child(&mut self) -> Option<&'a Component> {
        let child = self.nested.get(self.taken).copied()?;
        self.taken = self.taken.saturating_add(1);
        Some(child)
    }
}

impl<'a> ScheduledView<'a> {
    /// Read `component` and everything nested inside it.
    ///
    /// One walk. Every later question is a lookup, because a diff walks every property of both
    /// sides and the trait forbids allocating on that path.
    #[must_use]
    pub fn of(component: &'a Component) -> Self {
        let mut stack = alloc::vec![Frame::open(component)];
        loop {
            if let Some(child) = stack.last_mut().and_then(Frame::next_child) {
                stack.push(Frame::open(child));
                continue;
            }
            let Some(closed) = stack.pop() else {
                // Unreachable: the loop returns as the last frame is closed, so the stack is
                // never empty here. The fallback is a reading rather than a panic, because a
                // state this code has never reached is not a reason to take the process down.
                return Self::assemble(component, Vec::new());
            };
            let view = Self::assemble(closed.component, closed.built);
            match stack.last_mut() {
                Some(parent) => parent.built.push(view),
                None => return view,
            }
        }
    }

    /// One component's own reading, once its nested components have theirs.
    fn assemble(component: &'a Component, children: Vec<Self>) -> Self {
        let lines: Vec<PropertyLine<'a>> = component.properties().map(PropertyLine::read).collect();
        let attendees = positions_of(&lines, &PropertyId::ATTENDEE);
        let found = positions_of(&lines, &PropertyId::ORGANIZER);
        // One claim, however many times it was written. Two `ORGANIZER` lines that differ are
        // two claims about who owns this component, and picking the first is how the second one
        // gets to be the one that counts; two that are byte-identical are one claim restated.
        let organizer = found.first().copied().filter(|first| {
            let stated = lines.get(*first).map(|entry| &entry.line);
            found
                .iter()
                .all(|at| lines.get(*at).map(|entry| &entry.line) == stated)
        });
        Self {
            component,
            lines,
            organizer,
            attendees,
            children,
        }
    }

    /// The one property of this component with the identity `id`, absent when there are two.
    ///
    /// Two occurrences whose whole content lines are byte-identical are one statement stated
    /// twice, and answering them is not picking a winner out of two — there is one value, and
    /// a reader that refused it would report a component the caller plainly holds as one it
    /// holds nothing about. Two that differ anywhere, in a parameter as much as in a value,
    /// are two claims, and those stay `None`.
    fn single(&self, id: &PropertyId) -> Option<&'a Property> {
        let mut found = self.lines.iter().filter(|entry| entry.property.has_id(id));
        let first = found.next()?;
        for later in found {
            if later.line != first.line {
                return None;
            }
        }
        Some(first.property)
    }

    /// The value octets of the one property with the identity `id`.
    fn single_value(&self, id: &PropertyId) -> Option<&'a [u8]> {
        self.single(id)
            .map(|property| property.value_text().as_bytes())
    }

    /// The party values of the `index`th entry of `lines`, and the address beside them.
    fn party_at(&self, index: usize) -> Option<(Party<'_>, &PartyValues)> {
        let entry = self.lines.get(index)?;
        let values = entry.values.as_ref()?;
        let party = Party::read(
            entry.property.value_text().as_bytes(),
            values.sent_by.as_ref().map(RawText::as_bytes),
        );
        Some((party, values))
    }
}

/// Which entries of `lines` carry the identity `id`, in document order.
fn positions_of(lines: &[PropertyLine<'_>], id: &PropertyId) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.property.has_id(id))
        .map(|(at, _)| at)
        .collect()
}

/// The octets `name` normalizes to.
fn uppercased(name: &[u8]) -> RawText {
    let mut owned = Vec::from(name);
    owned.make_ascii_uppercase();
    RawText::from_vec(owned)
}

/// One whole content line: name, parameters and value, unfolded and unterminated.
///
/// Assembled exactly as `ical-core`'s own writer assembles one, so that a line handed back here
/// and the same line written to a file are the same octets: a parameter that arrived with no `=`
/// is written with none, a value that arrived quoted keeps its `DQUOTE`s, and a line whose
/// producer wrote no `:` gets none.
fn content_line(property: &Property) -> Vec<u8> {
    let mut written = Vec::new();
    written.extend_from_slice(property.name().as_bytes());
    for parameter in property.parameters() {
        written.push(b';');
        written.extend_from_slice(parameter.name().as_bytes());
        if parameter.has_value() {
            written.push(b'=');
            written.extend_from_slice(parameter.value().as_bytes());
        }
    }
    if property.layout().has_separator() {
        written.push(b':');
        written.extend_from_slice(property.value_text().as_bytes());
    }
    written
}

/// The scheduling parameter values of an `ORGANIZER` or an `ATTENDEE`, `None` for anything else.
fn party_values(property: &Property) -> Option<PartyValues> {
    if !property.has_id(&PropertyId::ORGANIZER) && !property.has_id(&PropertyId::ATTENDEE) {
        return None;
    }
    Some(PartyValues {
        sent_by: parameter_value(property, b"SENT-BY"),
        part_stat: parameter_value(property, b"PARTSTAT"),
        role: parameter_value(property, b"ROLE"),
        delegated_from: parameter_value(property, b"DELEGATED-FROM"),
        delegated_to: parameter_value(property, b"DELEGATED-TO"),
        answered_at: parameter_value(property, ANSWERED_AT).and_then(|stated| {
            // Read exactly as a `DTSTAMP` is read, because that is what it was: a value under a
            // `TZID` or written as a `DATE` orders nothing, and a time nothing could read is
            // the absence of one rather than a guess at one.
            match DateTimeValue::decode_value(stated.as_bytes()).ok()? {
                DateTimeValue::Utc(stamp) | DateTimeValue::Local(stamp) => nominal(stamp),
                DateTimeValue::Date(_) | DateTimeValue::Zoned { .. } => None,
            }
        }),
    })
}

/// The value the first parameter named `name` states, unquoted and with its carets resolved.
///
/// The first, because RFC 5545 section 3.2 puts no repeat limit on a parameter and none of the
/// five read here means anything stated twice; taking the first is what every reading path in
/// `ical-core` does.
///
/// The whole value, without splitting on `,`. RFC 5545 writes `DELEGATED-TO` as a
/// comma-separated list of quoted addresses, and [`Attendee`] has one slot for it: splitting
/// would let an answer to a two-delegate line write only the first delegate back, while keeping
/// the list whole makes it match nobody — the conservative direction, and the same one
/// [`crate::PartyId`] takes for an address that does not decode.
fn parameter_value(property: &Property, name: &[u8]) -> Option<RawText> {
    let held = property
        .parameters_named(name)
        .find(|entry| entry.has_value())?;
    // Quoting is undone first and the caret encoding second, because a writer applies them the
    // other way round: the `DQUOTE`s delimit the value rather than belong to it.
    Some(RawText::from_bytes(decode_caret(held.unquoted()).as_ref()))
}

/// The instant a `DTSTAMP` names, `None` when the value is not one.
///
/// `Utc` is what RFC 5545 section 3.8.7.2 requires and `Local` is what producers write instead;
/// a floating value is read on the nominal timeline, which is `ical-tz`'s own projection and the
/// only reading available for a wall clock nobody placed. A `DATE`, and a value under a `TZID`,
/// answer `None`: comparing one of those against a UTC `DTSTAMP` would order two different
/// timelines against each other, and RFC 5546 section 2.1.5 breaks a tie with this number.
fn timestamp_of(property: &Property) -> Option<Instant> {
    match property.value::<DateTimeValue<'_>>().value()? {
        DateTimeValue::Utc(stamp) | DateTimeValue::Local(stamp) => nominal(stamp),
        DateTimeValue::Date(_) | DateTimeValue::Zoned { .. } => None,
    }
}

/// The clock and the instant a `RECURRENCE-ID` states, `None` when the value does not read.
fn instance_reading(property: &Property) -> Option<(Instant, InstanceClock)> {
    let value = property.value::<DateTimeValue<'_>>().value()?;
    // A `DATE` loses its `TZID` on the way through the value type, so the parameter is asked
    // directly: a whole-day override under a named zone is a wall clock like any other.
    let placed = property.parameters_named(b"TZID").next().is_some();
    match value {
        DateTimeValue::Utc(stamp) => Some((nominal(stamp)?, InstanceClock::Utc)),
        DateTimeValue::Zoned { stamp, .. } => Some((nominal(stamp)?, InstanceClock::Zoned)),
        DateTimeValue::Local(stamp) => Some((nominal(stamp)?, InstanceClock::Floating)),
        DateTimeValue::Date(date) => {
            let midnight = CivilDateTime::new(date, CivilTime::MIDNIGHT);
            let clock = if placed {
                InstanceClock::Zoned
            } else {
                InstanceClock::Floating
            };
            Some((nominal(midnight)?, clock))
        },
    }
}

/// Which half of a repeated wall clock a value written in `clock` names.
///
/// A UTC value names a real instant, so it falls inside no fold and is [`FoldSide::Once`]
/// whatever zone the series runs on.
///
/// Everything else is a wall clock, and a wall clock is the *series'* clock whether or not the
/// value repeats the `TZID`: producers emit a bare `RECURRENCE-ID` for an override of a zoned
/// series, and reading that as a value on no zone at all was how one reply answered both halves
/// of a repeated hour. So a floating value is [`FoldSide::Unresolved`] exactly as a zoned one
/// is, and a caller holding the zone attaches a side with [`crate::resolve_instance`] and
/// [`InstanceRef::with_side`]. The cost lands where it should: a message whose instance nobody
/// placed is refused rather than applied to a guess.
const fn side_of(clock: InstanceClock) -> FoldSide {
    match clock {
        InstanceClock::Utc => FoldSide::Once,
        InstanceClock::Zoned | InstanceClock::Floating => FoldSide::Unresolved,
    }
}

/// How far forward a `RANGE` parameter reaches.
///
/// Only `THISANDFUTURE` reaches further than this instance. RFC 5545 section 3.2.13 registers
/// that one value, and a spelling nothing registers — RFC 2445's withdrawn `THISANDPRIOR`
/// included — is read as the narrower claim rather than as the wider one.
fn range_of(property: &Property) -> OverrideRange {
    let Some(held) = property
        .parameters_named(b"RANGE")
        .find(|entry| entry.has_value())
    else {
        return OverrideRange::ThisOnly;
    };
    let stated = decode_caret(held.unquoted());
    if stated.as_ref().eq_ignore_ascii_case(b"THISANDFUTURE") {
        OverrideRange::ThisAndFuture
    } else {
        OverrideRange::ThisOnly
    }
}

impl ScheduledComponent for ScheduledView<'_> {
    fn component_kind(&self) -> Option<ComponentKind> {
        ComponentKind::from_name(self.component.name().as_bytes())
    }

    fn method(&self) -> Option<&[u8]> {
        self.single_value(&PropertyId::METHOD)
    }

    fn uid(&self) -> Option<&[u8]> {
        self.single_value(&PropertyId::UID)
    }

    fn sequence(&self) -> SequenceRead {
        if !self
            .lines
            .iter()
            .any(|entry| entry.property.has_id(&PropertyId::SEQUENCE))
        {
            // RFC 5546 section 3.2 reads an absent `SEQUENCE` as zero, which is a revision.
            return SequenceRead::Absent;
        }
        // Two revisions stated at once are no revision, and the same line stated twice is one:
        // `single` draws that division once for every name read here.
        let Some(stated) = self.single(&PropertyId::SEQUENCE) else {
            return SequenceRead::Unreadable;
        };
        // A negative `SEQUENCE` is an integer RFC 5545 section 3.8.7.4 does not admit, and it is
        // no more a revision than a value that is not an integer at all.
        stated
            .value::<i32>()
            .value()
            .and_then(|stated| u32::try_from(stated).ok())
            .map_or(SequenceRead::Unreadable, SequenceRead::Value)
    }

    fn dtstamp(&self) -> Option<Instant> {
        timestamp_of(self.single(&PropertyId::DTSTAMP)?)
    }

    fn recurrence_id(&self) -> Option<InstanceRef> {
        let property = self.single(&PropertyId::RECURRENCE_ID)?;
        let range = range_of(property);
        let Some((named, clock)) = instance_reading(property) else {
            // Present and unusable. Answering `None` would turn a message about one instance
            // into a message about the whole series, so this answers a reference nothing can
            // resolve: an unresolved side never compares `Same`, so the gate denies instead.
            return Some(InstanceRef::new(
                Instant::EPOCH,
                InstanceClock::Zoned,
                range,
            ));
        };
        Some(InstanceRef::new(named, clock, range).with_side(side_of(clock)))
    }

    fn organizer(&self) -> Option<Party<'_>> {
        let (party, _) = self.party_at(self.organizer?)?;
        Some(party)
    }

    fn attendee_count(&self) -> usize {
        self.attendees.len()
    }

    fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
        let (party, values) = self.party_at(*self.attendees.get(index)?)?;
        let mut who = Attendee::new(party);
        if let Some(stated) = values.part_stat.as_ref() {
            who = who.with_part_stat(stated.as_bytes());
        }
        if let Some(stated) = values.role.as_ref() {
            who = who.with_role(stated.as_bytes());
        }
        if let Some(stated) = values.delegated_from.as_ref() {
            who = who.with_delegated_from(stated.as_bytes());
        }
        if let Some(stated) = values.delegated_to.as_ref() {
            who = who.with_delegated_to(stated.as_bytes());
        }
        Some(who)
    }

    fn attendee_answered_at(&self, index: usize) -> Option<Instant> {
        let entry = self.lines.get(*self.attendees.get(index)?)?;
        entry.values.as_ref()?.answered_at
    }

    fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
        // The list is collected in document order over the `ATTENDEE` identity, which is the
        // counting `Component::apply_to_occurrence` uses, so the two indexes agree here. The
        // trait states the question anyway because an implementation is free to order its list
        // some other way, and a reply has to name the line it changed.
        self.attendees
            .get(index)
            .map(|_| PropertyOccurrence::new(PropertyId::ATTENDEE, index))
    }

    fn property_count(&self) -> usize {
        self.lines.len()
    }

    fn property_name(&self, index: usize) -> Option<&[u8]> {
        self.lines.get(index).map(|entry| entry.name.as_bytes())
    }

    fn property_line(&self, index: usize) -> Option<&[u8]> {
        self.lines.get(index).map(|entry| entry.line.as_bytes())
    }

    fn child_count(&self) -> usize {
        self.children.len()
    }

    fn child(&self, index: usize) -> Option<&dyn ScheduledComponent> {
        self.children
            .get(index)
            .map(|view| view as &dyn ScheduledComponent)
    }
}

impl Debug for ScheduledView<'_> {
    /// A summary rather than the tree, for the reason the walk above gives: a derived `Debug`
    /// recurses one stack frame per level, and the nesting is attacker-chosen.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScheduledView")
            .field("name", &self.component.name().as_bytes())
            .field("properties", &self.lines.len())
            .field("attendees", &self.attendees.len())
            .field("children", &self.children.len())
            .finish()
    }
}

impl Drop for ScheduledView<'_> {
    /// Drop the nesting over an explicit stack, for the reason the walk above gives.
    ///
    /// Each node's children are taken before that node is dropped, so the drop glue that runs
    /// afterwards has an empty vector to walk and never re-enters this.
    fn drop(&mut self) {
        let mut pending = alloc::vec![mem::take(&mut self.children)];
        while let Some(mut level) = pending.pop() {
            while let Some(mut node) = level.pop() {
                pending.push(mem::take(&mut node.children));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use ical_core::{
        Component, ComponentKind, Document, IgnoreDiagnostics, Item, Limits, Meter, MutationError,
        ProposedChange,
    };

    use super::ScheduledView;
    use crate::authorize::{AuthorizationDenied, apply_transition, evaluate_message};
    use crate::identity::{InstanceMatch, SequenceRead};
    use crate::message::ItipMessage;
    use crate::party::{PartStat, PartyId, Role};
    use crate::state::{PropertyOccurrence, ScheduledComponent};
    use crate::transition::{ScheduleTarget, WriteRejected};

    /// The recipient's own copy: two attendees, an organizer with an assistant, and a
    /// `DELEGATED-TO` whose value carries a `"` in RFC 6868's spelling of it.
    const HELD: &str = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//icalkit//test//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:4f1b-9a@x.te\r\n",
        "DTSTAMP:20260810T110000Z\r\n",
        "DTSTART:20260901T120000Z\r\n",
        "SEQUENCE:2\r\n",
        "SUMMARY:Review\r\n",
        "ORGANIZER;SENT-BY=\"mailto:pa@x.te\":mailto:c@x.te\r\n",
        "ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:a^'b@x.te\":mailto:b@x.te\r\n",
        "ATTENDEE;ROLE=CHAIR:mailto:c@x.te\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );

    /// The `ATTENDEE` line of `HELD`, as the file spells it.
    const HELD_ATTENDEE: &[u8] =
        b"ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:a^'b@x.te\":mailto:b@x.te";

    /// One `REPLY`, assembled from the parts each test varies.
    ///
    /// RFC 5546 section 3.2.3's table is what decides the fixed lines: `ATTENDEE`, `DTSTAMP`,
    /// `ORGANIZER` and `UID` are each `1`, and `SEQUENCE` and `RECURRENCE-ID` are each `0 or 1`.
    fn reply(sequence: &str, dtstamp: &str, extra: &str, attendee: &str) -> Vec<u8> {
        let mut text = alloc::string::String::new();
        text.push_str("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit//test//EN\r\n");
        text.push_str("METHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:4f1b-9a@x.te\r\n");
        text.push_str("DTSTAMP:");
        text.push_str(dtstamp);
        text.push_str("\r\nSEQUENCE:");
        text.push_str(sequence);
        text.push_str("\r\nORGANIZER:mailto:c@x.te\r\n");
        text.push_str(extra);
        text.push_str(attendee);
        text.push_str("\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
        text.into_bytes()
    }

    /// The `ATTENDEE` line a reply from `b@x.te` states, echoing the delegation.
    const REPLY_ATTENDEE: &str =
        "ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:a^'b@x.te\":mailto:b@x.te";

    /// A document, read under the default policy with nothing reported.
    fn read(text: &[u8]) -> Document {
        Document::parse(text, Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// The one `VCALENDAR` a fixture holds.
    fn calendar_of(document: &Document) -> &Component {
        document.components().next().unwrap()
    }

    /// The one `VEVENT` inside it.
    fn event_of(document: &Document) -> &Component {
        calendar_of(document).components().next().unwrap()
    }

    /// A place an authorized transition is written, so a test can go through the real door.
    ///
    /// `ical-itip` ships no [`ScheduleTarget`] for a `Component` from this unit; this is the
    /// same routing one would do, kept local so that the test exercises
    /// [`apply_transition`] rather than a shortcut around it.
    #[derive(Debug)]
    struct Target<'a>(&'a mut Component);

    impl ScheduleTarget for Target<'_> {
        fn write_change(
            &mut self,
            at: &PropertyOccurrence,
            change: &ProposedChange,
        ) -> Result<(), WriteRejected> {
            self.0
                .apply_to_occurrence(at.id(), at.index(), change, Limits::DEFAULT)
                .map_err(|refused| match refused {
                    MutationError::Absent => WriteRejected::UnknownProperty,
                    MutationError::ComponentBoundary => WriteRejected::ReadOnly,
                    _ => WriteRejected::ValueTypeMismatch,
                })
        }
    }

    /// What the bridge reads out of a component, against what the file says.
    ///
    /// The columns come from RFC 5545: the `SENT-BY` of section 3.2.18 is a second identity and
    /// not the first, an absent `ROLE` is `REQ-PARTICIPANT` by section 3.2.16, and the content
    /// line of section 3.1 is name, parameters and value together.
    #[test]
    fn a_component_reads_as_the_state_a_message_is_judged_against() {
        let document = read(HELD.as_bytes());
        let current = ScheduledView::of(event_of(&document));

        assert_eq!(current.component_kind(), Some(ComponentKind::Event));
        assert_eq!(current.uid(), Some(&b"4f1b-9a@x.te"[..]));
        assert_eq!(current.sequence(), SequenceRead::Value(2));
        assert!(current.dtstamp().is_some());
        assert_eq!(current.recurrence_id(), None);
        assert_eq!(current.method(), None, "a VEVENT states no METHOD");
        assert_eq!(current.child_count(), 0);

        let organizer = current.organizer().unwrap();
        assert!(organizer.is(PartyId::new("mailto:c@x.te")));
        assert!(
            organizer.is_agent_of(PartyId::new("mailto:pa@x.te")),
            "SENT-BY is a second identity and never the first"
        );
        assert!(!organizer.is(PartyId::new("mailto:pa@x.te")));

        assert_eq!(current.attendee_count(), 2);
        let first = current.attendee(0).unwrap();
        assert_eq!(first.part_stat(), PartStat::Delegated);
        assert_eq!(first.role(), Role::RequiredParticipant);
        let second = current.attendee(1).unwrap();
        assert_eq!(second.role(), Role::Chair);
        assert_eq!(second.part_stat(), PartStat::NeedsAction);
        assert_eq!(
            current.attendee_occurrence(1),
            Some(PropertyOccurrence::named(b"ATTENDEE", 1))
        );
        assert_eq!(current.attendee_occurrence(2), None);
        assert_eq!(current.attendee(2), None);

        let at = attendee_line(&current, 0);
        assert_eq!(current.property_line(at), Some(HELD_ATTENDEE));
        assert_eq!(current.property_name(at), Some(&b"ATTENDEE"[..]));
    }

    /// Which property index carries the `count`th `ATTENDEE`.
    fn attendee_line(current: &ScheduledView<'_>, count: usize) -> usize {
        let mut seen = 0_usize;
        for index in 0..current.property_count() {
            if current.property_name(index) == Some(&b"ATTENDEE"[..]) {
                if seen == count {
                    return index;
                }
                seen = seen.saturating_add(1);
            }
        }
        panic!("the fixture states two ATTENDEE lines");
    }

    /// A name is normalized and a line is not, which is what stops a table row of `1` from being
    /// satisfied twice by one property written in two cases.
    #[test]
    fn a_name_is_normalized_and_the_line_it_names_is_not() {
        const MIXED: &str = concat!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n",
            "uid:4f1b-9a@x.te\r\n",
            "Dtstart;value=DATE:20260901\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let document = read(MIXED.as_bytes());
        let current = ScheduledView::of(event_of(&document));

        assert_eq!(current.property_name(0), Some(&b"UID"[..]));
        assert_eq!(current.property_line(0), Some(&b"uid:4f1b-9a@x.te"[..]));
        assert_eq!(current.property_name(1), Some(&b"DTSTART"[..]));
        assert_eq!(
            current.property_line(1),
            Some(&b"Dtstart;value=DATE:20260901"[..]),
            "the octets are the producer's and the identity is the specification's"
        );
        assert_eq!(current.uid(), Some(&b"4f1b-9a@x.te"[..]));
        assert_eq!(current.property_line(2), None);
    }

    /// RFC 5546 section 3.2: an absent `SEQUENCE` is zero, and nothing else is.
    #[test]
    fn a_sequence_is_absent_a_value_or_unreadable_and_never_a_guess() {
        let cases: [(&str, SequenceRead); 6] = [
            ("", SequenceRead::Absent),
            ("SEQUENCE:0\r\n", SequenceRead::Value(0)),
            ("SEQUENCE:7\r\n", SequenceRead::Value(7)),
            ("SEQUENCE:later\r\n", SequenceRead::Unreadable),
            ("SEQUENCE:-1\r\n", SequenceRead::Unreadable),
            ("SEQUENCE:1\r\nSEQUENCE:9\r\n", SequenceRead::Unreadable),
        ];
        for (stated, expected) in cases {
            let mut text = alloc::string::String::from("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n");
            text.push_str("UID:4f1b-9a@x.te\r\n");
            text.push_str(stated);
            text.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
            let document = read(text.as_bytes());
            let current = ScheduledView::of(event_of(&document));
            assert_eq!(current.sequence(), expected, "{stated:?}");
        }
        assert_eq!(SequenceRead::Absent.value(), Some(0));
        assert_eq!(SequenceRead::Unreadable.value(), None);
    }

    /// A name RFC 5545 declares once, arriving twice, is not one value.
    #[test]
    fn a_property_stated_twice_is_no_value_rather_than_the_first_one() {
        const DOUBLED: &str = concat!(
            "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nMETHOD:CANCEL\r\nBEGIN:VEVENT\r\n",
            "UID:one\r\nUID:two\r\n",
            "ORGANIZER:mailto:c@x.te\r\nORGANIZER:mailto:eve@x.te\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let document = read(DOUBLED.as_bytes());
        let calendar = ScheduledView::of(calendar_of(&document));
        let current = ScheduledView::of(event_of(&document));

        assert_eq!(calendar.method(), None, "two METHODs are two messages");
        assert_eq!(current.uid(), None);
        assert_eq!(
            current.organizer(),
            None,
            "picking the first is how the second one gets to be the one that counts"
        );
    }

    /// Agenda item 6, in both directions. The value reaches the party with its carets resolved,
    /// and the reply writes back the octets the file had.
    #[test]
    fn a_delegated_to_carrying_a_caret_pair_survives_read_reply_and_apply() {
        let held_document = read(HELD.as_bytes());
        let held = event_of(&held_document);
        let current = ScheduledView::of(held);

        let delegate = current.attendee(0).unwrap().delegated_to().unwrap();
        assert_eq!(
            delegate.as_bytes(),
            &b"mailto:a\"b@x.te"[..],
            "RFC 6868's ^' is a DQUOTE, and a party is given a value and not a spelling"
        );

        let message_bytes = reply("2", "20260810T120000Z", "", REPLY_ATTENDEE);
        let message_document = read(&message_bytes);
        let calendar = ScheduledView::of(calendar_of(&message_document));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = ItipMessage::read(
            &calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();

        let authorized =
            evaluate_message(&message, &current, PartyId::new("mailto:b@x.te")).unwrap();
        assert_eq!(authorized.transition().len(), 1);
        assert!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"ATTENDEE", 0))
                .is_some(),
            "a reply names the recipient's own numbering and not the sender's"
        );

        let mut written = held.clone();
        let report = apply_transition(&mut Target(&mut written), authorized);
        assert!(report.is_complete() && report.applied() == 1);
        // The reply restates the delegation it already carries and records when it was
        // answered, and nothing else on the line moves: the value the file spelled with `^'`
        // is written back with `^'`, where encoding a value that skipped the decode would
        // write `^^'`. Compared through the reader rather than as file octets, because a line
        // that grew past 75 octets is folded by the writer and a fold is not a difference.
        let after = ScheduledView::of(&written);
        let at = attendee_line(&after, 0);
        assert_eq!(
            after.property_line(at),
            Some(
                &b"ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO=\"mailto:a^'b@x.te\";\
                   X-ICALKIT-ANSWERED-AT=20260810T120000Z:mailto:b@x.te"[..]
            )
        );
        let untouched = attendee_line(&after, 1);
        assert_eq!(
            after.property_line(untouched),
            Some(&b"ATTENDEE;ROLE=CHAIR:mailto:c@x.te"[..]),
            "a reply reaches one line"
        );
    }

    /// The parameter the reply diff writes, and the ordering it exists for.
    ///
    /// RFC 5546 section 2.1.5 orders two messages at one revision by `DTSTAMP`, and two replies
    /// from one attendee are exactly that. The component's own `DTSTAMP` is the organizer's and
    /// is older than both, so the time each answer was written at is recorded on the line it
    /// answers for — and the attendee's own earlier answer, replayed afterwards, is then
    /// refused rather than silently reverting the later one.
    #[test]
    fn an_earlier_answer_replayed_after_a_later_one_is_refused() {
        let held_document = read(HELD.as_bytes());
        let held = event_of(&held_document);

        let mut store = held.clone();
        for (stamp, expected) in [("20260810T140000Z", true), ("20260810T130000Z", false)] {
            let snapshot = store.clone();
            let current = ScheduledView::of(&snapshot);
            let message_bytes = reply("2", stamp, "", "ATTENDEE;PARTSTAT=ACCEPTED:mailto:b@x.te");
            let message_document = read(&message_bytes);
            let calendar = ScheduledView::of(calendar_of(&message_document));
            let mut meter = Meter::new(Limits::DEFAULT);
            let message = ItipMessage::read(
                &calendar,
                Limits::DEFAULT,
                &mut meter,
                &mut IgnoreDiagnostics,
            )
            .unwrap();

            match evaluate_message(&message, &current, PartyId::new("mailto:b@x.te")) {
                Ok(authorized) => {
                    assert!(expected, "the replayed earlier answer was authorized");
                    let report = apply_transition(&mut Target(&mut store), authorized);
                    assert!(report.is_complete());
                },
                Err(denied) => {
                    assert!(!expected, "the first answer was refused: {denied:?}");
                    assert!(matches!(denied, AuthorizationDenied::DtstampStale { .. }));
                },
            }
        }

        let after = ScheduledView::of(&store);
        assert_eq!(
            after.attendee_answered_at(0),
            ScheduledView::of(event_of(&read(&reply(
                "2",
                "20260810T140000Z",
                "",
                "ATTENDEE:mailto:b@x.te"
            ))))
            .dtstamp(),
            "the line records the answer that was applied and not the one that was refused"
        );
        assert_eq!(after.attendee(0).unwrap().part_stat(), PartStat::Accepted);
    }

    /// RFC 5546 section 2.1.4: an older revision never overwrites a newer one.
    #[test]
    fn an_older_sequence_is_refused_with_the_revision_the_recipient_holds() {
        let held_document = read(HELD.as_bytes());
        let current = ScheduledView::of(event_of(&held_document));
        let message_bytes = reply("1", "20260810T120000Z", "", REPLY_ATTENDEE);
        let message_document = read(&message_bytes);
        let calendar = ScheduledView::of(calendar_of(&message_document));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = ItipMessage::read(
            &calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();

        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:b@x.te")).unwrap_err(),
            AuthorizationDenied::SequenceStale { have: 2 }
        );
    }

    /// RFC 5546 section 2.1.5: `DTSTAMP` breaks the tie, and it breaks it towards refusal.
    #[test]
    fn an_equal_sequence_with_an_older_dtstamp_is_refused() {
        let held_document = read(HELD.as_bytes());
        let current = ScheduledView::of(event_of(&held_document));
        let message_bytes = reply("2", "20260810T100000Z", "", REPLY_ATTENDEE);
        let message_document = read(&message_bytes);
        let calendar = ScheduledView::of(calendar_of(&message_document));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = ItipMessage::read(
            &calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();

        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:b@x.te")).unwrap_err(),
            AuthorizationDenied::DtstampStale {
                have: current.dtstamp().unwrap()
            }
        );
    }

    /// A `REPLY` naming an instance the recipient's copy does not have is not a reply to it.
    #[test]
    fn a_reply_naming_an_instance_the_series_does_not_have_is_refused() {
        const SERIES: &str = concat!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n",
            "UID:4f1b-9a@x.te\r\n",
            "DTSTAMP:20260810T110000Z\r\n",
            "RECURRENCE-ID:20260901T120000Z\r\n",
            "SEQUENCE:2\r\n",
            "ORGANIZER:mailto:c@x.te\r\n",
            "ATTENDEE:mailto:b@x.te\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let held_document = read(SERIES.as_bytes());
        let current = ScheduledView::of(event_of(&held_document));
        let message_bytes = reply(
            "2",
            "20260810T120000Z",
            "RECURRENCE-ID:20260908T120000Z\r\n",
            "ATTENDEE;PARTSTAT=ACCEPTED:mailto:b@x.te",
        );
        let message_document = read(&message_bytes);
        let calendar = ScheduledView::of(calendar_of(&message_document));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = ItipMessage::read(
            &calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();

        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:b@x.te")).unwrap_err(),
            AuthorizationDenied::NoMatchingInstance,
            "two UTC instances are told apart without a zone, and these are two"
        );
    }

    /// A `REPLY` from an address nobody invited is a refused message, with a reason to show.
    #[test]
    fn a_reply_from_an_address_nobody_invited_is_refused() {
        let held_document = read(HELD.as_bytes());
        let current = ScheduledView::of(event_of(&held_document));
        let message_bytes = reply("2", "20260810T120000Z", "", REPLY_ATTENDEE);
        let message_document = read(&message_bytes);
        let calendar = ScheduledView::of(calendar_of(&message_document));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = ItipMessage::read(
            &calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();

        assert_eq!(
            evaluate_message(&message, &current, PartyId::new("mailto:eve@x.te")).unwrap_err(),
            AuthorizationDenied::UnknownAttendee
        );
    }

    /// A `RECURRENCE-ID` that does not read is an instance nothing matches, never the series.
    #[test]
    fn an_unreadable_recurrence_id_is_an_instance_and_not_the_whole_series() {
        const BROKEN: &str = concat!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n",
            "UID:4f1b-9a@x.te\r\n",
            "RECURRENCE-ID:whenever\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let document = read(BROKEN.as_bytes());
        let current = ScheduledView::of(event_of(&document));
        let named = current.recurrence_id().unwrap();
        assert!(
            !named.side().is_resolved(),
            "an instance nothing resolved can never compare Same, so the gate denies"
        );
        assert_eq!(named.compare(named), InstanceMatch::Ambiguous);
        assert!(!named.compare(named).is_same());
    }

    /// A `RANGE` reaching every later instance is represented, and a spelling nothing registers
    /// is read as the narrower claim.
    #[test]
    fn a_range_is_read_only_where_the_specification_registers_one() {
        let cases: [(&str, bool); 3] = [
            (
                "RECURRENCE-ID;RANGE=THISANDFUTURE:20260901T120000Z\r\n",
                true,
            ),
            (
                "RECURRENCE-ID;RANGE=THISANDPRIOR:20260901T120000Z\r\n",
                false,
            ),
            ("RECURRENCE-ID:20260901T120000Z\r\n", false),
        ];
        for (stated, reaching) in cases {
            let mut text = alloc::string::String::from("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n");
            text.push_str("UID:4f1b-9a@x.te\r\n");
            text.push_str(stated);
            text.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
            let document = read(text.as_bytes());
            let current = ScheduledView::of(event_of(&document));
            let named = current.recurrence_id().unwrap();
            assert_eq!(named.is_this_and_future(), reaching, "{stated:?}");
            assert!(named.side().is_resolved(), "a UTC value falls in no fold");
        }
    }

    /// Nesting is walked, dropped and printed over an explicit stack, so a tree deeper than the
    /// call stack costs memory rather than the process.
    #[test]
    fn a_deeply_nested_component_is_read_and_dropped_without_recursion() {
        const DEPTH: usize = 3_000;
        // Assembled rather than parsed, so the depth is this test's rather than a policy's.
        let mut root = Component::create(b"VALARM", Vec::new()).unwrap();
        for _ in 0..DEPTH {
            root = Component::create(b"VALARM", vec![Item::Component(root)]).unwrap();
        }

        let view = ScheduledView::of(&root);
        assert_eq!(view.child_count(), 1);
        assert_eq!(view.component_kind(), Some(ComponentKind::Alarm));
        assert_eq!(view.property_count(), 0);
        // The summary form: printing the tree would recurse exactly as building it would.
        assert!(alloc::format!("{view:?}").contains("children"));
        drop(view);
    }
}
