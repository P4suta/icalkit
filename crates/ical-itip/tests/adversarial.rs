// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An adversary's pass over RFC 5546 authorization: one test per named attack.
//!
//! Specification: RFC 5546 section 1.3 (the two roles), section 2.1.4 (`SEQUENCE`), section
//! 2.1.5 (`DTSTAMP`), section 3.2 (the per-method constraint tables); `SECURITY.md` for the
//! three positions this crate exists to hold, and `docs/adr/0010` for the bounds.
//!
//! Every expected column below is read off RFC 5546's own text and written beside the row it
//! justifies, never off what the implementation happens to answer. Where the two diverge the
//! divergence is stated in the test rather than smoothed over: an attendee's `DTSTART` inside a
//! `REPLY` is *ignored* rather than refused, and the test asserts the security property (the
//! meeting does not move) while saying so out loud.
//!
//! # The state carrier
//!
//! The fixtures implement [`ScheduledComponent`] directly rather than going through
//! `ical_core::Component`. That is the seam `docs/design/ical-itip-api.md` names for a server
//! whose state is a database row, and it is the only carrier that can reach two of these
//! attacks at all: a fold side is something a *zone* resolved, so a component built from octets
//! alone can only ever report [`FoldSide::Unresolved`], and a hundred thousand `ATTENDEE` lines
//! are a claim about a list's length rather than about four megabytes of text.
//!
//! # What is asserted here that no unit test can be
//!
//! The gate's *order*. Each case below is built so that exactly one rule can fire, and the
//! variant asserted is the first reason a caller could act on. A refusal that arrived for the
//! right reason by accident — a stale `SEQUENCE` caught by a `UID` check — would pass a unit
//! test of either half and fail here.

#[cfg(test)]
mod tests {
    use core::fmt::Debug;

    use ical_core::{ComponentKind, IgnoreDiagnostics, Instant, UtcOffset};
    use ical_itip::{
        ActorRole, Attendee, AuthorizationDenied, Commitment, FoldSide, InstanceClock, InstanceRef,
        ItipMessage, Limits, MessageError, Meter, Party, PartyId, PriorState, PropertyOccurrence,
        ProposedChange, Revision, ScheduledComponent, SequenceRead, TransitionReason,
        describe_message, evaluate_message,
    };
    use ical_recur::OverrideRange;
    use ical_tz::{LocalResolution, Reading};

    /// The `UID` every fixture shares unless the attack is about the identifier itself.
    const MEETING: &str = "4f1b-9a@example.com";
    /// The same identifier as the content line a file carries.
    const MEETING_LINE: &str = "UID:4f1b-9a@example.com";
    /// The organizer of everything here.
    const CHAIR: &str = "mailto:chair@example.com";
    /// An invited attendee, and the first `ATTENDEE` line of the state.
    const ANN: &str = "mailto:ann@example.com";
    /// A second invited attendee, and the second `ATTENDEE` line.
    const BO: &str = "mailto:bo@example.com";
    /// An address on nobody's list.
    const MALLORY: &str = "mailto:mallory@example.net";
    /// The `ATTENDEE` line a crowded payload repeats.
    const CROWD_LINE: &str = "ATTENDEE:mailto:ann@example.com";

    /// The wall clock `20261101T013000` in `America/New_York`, projected onto the nominal
    /// timeline the seam walks. Both halves of that night's repeated hour carry this key.
    const REPEATED_HOUR: i64 = 1_793_496_600;
    /// `20261101T053000Z`: the first of the two instants, still on the daylight offset.
    const BEFORE_THE_FALL: i64 = 1_793_511_000;
    /// `20261101T063000Z`: the second, an hour later on the standard offset.
    const AFTER_THE_FALL: i64 = 1_793_514_600;
    /// `20260901T150000Z`, the instance the `RANGE` cases address.
    const SEPTEMBER: i64 = 1_788_274_800;

    /// A `DTSTAMP`: the octets a file carries and the instant they name.
    ///
    /// Both halves, because the trait hands the gate a typed instant while the diff compares
    /// the line — a fixture that carried only one of them could not be wrong in the way a real
    /// message is wrong.
    #[derive(Clone, Copy, Debug)]
    struct Stamp {
        /// The value as a file spells it.
        written: &'static str,
        /// The instant those octets name, in seconds from the epoch.
        at: i64,
    }

    /// `20260801T090000Z`, the stamp on the state a caller already holds.
    const EARLY: Stamp = Stamp {
        written: "20260801T090000Z",
        at: 1_785_574_800,
    };
    /// `20260802T090000Z`, a day later: the stamp on a message answering it.
    const LATE: Stamp = Stamp {
        written: "20260802T090000Z",
        at: 1_785_661_200,
    };

    /// One `ATTENDEE` line of a fixture.
    #[derive(Clone, Debug)]
    struct Guest {
        /// The `CAL-ADDRESS` value.
        address: String,
        /// The `PARTSTAT` value, absent when the line states none.
        part_stat: Option<String>,
    }

    impl Guest {
        /// An attendee who states no participation status, which RFC 5545 section 3.2.12
        /// reads as `NEEDS-ACTION`.
        fn new(address: &str) -> Self {
            Self {
                address: address.to_owned(),
                part_stat: None,
            }
        }

        /// The same attendee answering `status`.
        fn answering(mut self, status: &str) -> Self {
            self.part_stat = Some(status.to_owned());
            self
        }

        /// The content line this attendee is, so that the octet diff and the typed accessors
        /// cannot disagree about one fixture.
        fn written(&self) -> String {
            match &self.part_stat {
                Some(status) => format!("ATTENDEE;PARTSTAT={status}:{}", self.address),
                None => format!("ATTENDEE:{}", self.address),
            }
        }
    }

    /// One component, as much of it as [`ScheduledComponent`] asks for.
    #[derive(Debug, Default)]
    struct Node {
        /// What kind of component this is.
        kind: Option<ComponentKind>,
        /// The `METHOD` value, which only a `VCALENDAR` states.
        method: Option<String>,
        /// The `UID` value.
        uid: Option<String>,
        /// What the `SEQUENCE` property was.
        sequence: SequenceRead,
        /// The `DTSTAMP`.
        dtstamp: Option<Instant>,
        /// The `RECURRENCE-ID`, with whatever side a zone resolved.
        instance: Option<InstanceRef>,
        /// The `ORGANIZER` line.
        organizer: Option<String>,
        /// The `ATTENDEE` lines, in the order they appear.
        guests: Vec<Guest>,
        /// Every property, as a name and the whole content line.
        lines: Vec<(String, String)>,
        /// The components nested directly inside this one.
        children: Vec<Box<dyn ScheduledComponent>>,
    }

    impl Node {
        /// A `VCALENDAR` stating `method` and carrying one scheduling payload.
        fn calendar(method: &str, payload: impl ScheduledComponent + 'static) -> Self {
            Self {
                kind: Some(ComponentKind::Calendar),
                method: Some(method.to_owned()),
                children: vec![Box::new(payload)],
                ..Self::default()
            }
        }

        /// A `VEVENT` stating nothing yet.
        fn event() -> Self {
            Self {
                kind: Some(ComponentKind::Event),
                ..Self::default()
            }
        }

        /// A `VALARM`, which the nesting cases use as the thing that nests.
        fn alarm() -> Self {
            Self {
                kind: Some(ComponentKind::Alarm),
                ..Self::default()
            }
        }

        /// Set the single-valued property `name` to the whole line `written`.
        ///
        /// Replaces rather than appends, so that a builder call may be re-applied by a case
        /// that changes one property: appending would leave two `UID` lines and turn every
        /// later assertion into a test of the fixture.
        fn set_line(&mut self, name: &str, written: &str) {
            match self.lines.iter_mut().find(|(had, _)| had.as_str() == name) {
                Some(slot) => slot.1 = written.to_owned(),
                None => self.lines.push((name.to_owned(), written.to_owned())),
            }
        }

        /// The same component under `uid`.
        fn about(mut self, uid: &str) -> Self {
            self.uid = Some(uid.to_owned());
            self.set_line("UID", &format!("UID:{uid}"));
            self
        }

        /// The same component stamped `stamp`.
        fn stamped(mut self, stamp: Stamp) -> Self {
            self.dtstamp = Some(Instant::from_unix_seconds(stamp.at));
            self.set_line("DTSTAMP", &format!("DTSTAMP:{}", stamp.written));
            self
        }

        /// The same component at revision `value`.
        fn at_revision(mut self, value: u32) -> Self {
            self.sequence = SequenceRead::Value(value);
            self.set_line("SEQUENCE", &format!("SEQUENCE:{value}"));
            self
        }

        /// The same component with a `SEQUENCE` that is present and is not an integer.
        fn unreadable_sequence(mut self) -> Self {
            self.sequence = SequenceRead::Unreadable;
            self.set_line("SEQUENCE", "SEQUENCE:soon");
            self
        }

        /// The same component organized by `address`.
        fn organized_by(mut self, address: &str) -> Self {
            self.organizer = Some(address.to_owned());
            self.set_line("ORGANIZER", &format!("ORGANIZER:{address}"));
            self
        }

        /// The same component with `guest` appended to its attendee list.
        fn guest(mut self, guest: Guest) -> Self {
            self.lines.push((String::from("ATTENDEE"), guest.written()));
            self.guests.push(guest);
            self
        }

        /// The same component addressing `instance`, whose line a file spells `written`.
        fn addressing(mut self, instance: InstanceRef, written: &str) -> Self {
            self.instance = Some(instance);
            self.set_line("RECURRENCE-ID", written);
            self
        }

        /// The same component with the property `name` set to `value`.
        fn property(mut self, name: &str, value: &str) -> Self {
            self.set_line(name, &format!("{name}:{value}"));
            self
        }

        /// The same component with `nested` inside it.
        fn containing(mut self, nested: impl ScheduledComponent + 'static) -> Self {
            self.children.push(Box::new(nested));
            self
        }
    }

    impl ScheduledComponent for Node {
        fn component_kind(&self) -> Option<ComponentKind> {
            self.kind
        }

        fn method(&self) -> Option<&[u8]> {
            self.method.as_deref().map(str::as_bytes)
        }

        fn uid(&self) -> Option<&[u8]> {
            self.uid.as_deref().map(str::as_bytes)
        }

        fn sequence(&self) -> SequenceRead {
            self.sequence
        }

        fn dtstamp(&self) -> Option<Instant> {
            self.dtstamp
        }

        fn recurrence_id(&self) -> Option<InstanceRef> {
            self.instance
        }

        fn organizer(&self) -> Option<Party<'_>> {
            self.organizer
                .as_ref()
                .map(|address| Party::read(address.as_bytes(), None))
        }

        fn attendee_count(&self) -> usize {
            self.guests.len()
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            let guest = self.guests.get(index)?;
            let listed = Attendee::new(Party::read(guest.address.as_bytes(), None));
            Some(match guest.part_stat.as_deref() {
                Some(status) => listed.with_part_stat(status.as_bytes()),
                None => listed,
            })
        }

        fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
            // Every attendee is appended together with its own `ATTENDEE` line, so the two
            // numberings agree by construction rather than by coincidence.
            (index < self.guests.len()).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
        }

        fn property_count(&self) -> usize {
            self.lines.len()
        }

        fn property_name(&self, index: usize) -> Option<&[u8]> {
            self.lines.get(index).map(|(name, _)| name.as_bytes())
        }

        fn property_line(&self, index: usize) -> Option<&[u8]> {
            self.lines.get(index).map(|(_, line)| line.as_bytes())
        }

        fn child_count(&self) -> usize {
            self.children.len()
        }

        fn child(&self, index: usize) -> Option<&dyn ScheduledComponent> {
            self.children.get(index).map(|nested| &**nested)
        }
    }

    /// A payload claiming a hundred thousand attendees without allocating one.
    ///
    /// Every line is the same address on purpose: what is under test is the *length* of the
    /// list, and a fixture that built a hundred thousand distinct addresses would be measuring
    /// itself rather than the bound.
    #[derive(Debug)]
    struct Crowd {
        /// How many `ATTENDEE` lines this payload claims.
        count: usize,
    }

    impl ScheduledComponent for Crowd {
        fn component_kind(&self) -> Option<ComponentKind> {
            Some(ComponentKind::Event)
        }

        fn method(&self) -> Option<&[u8]> {
            None
        }

        fn uid(&self) -> Option<&[u8]> {
            Some(MEETING.as_bytes())
        }

        fn sequence(&self) -> SequenceRead {
            SequenceRead::Value(2)
        }

        fn dtstamp(&self) -> Option<Instant> {
            Some(Instant::from_unix_seconds(LATE.at))
        }

        fn recurrence_id(&self) -> Option<InstanceRef> {
            None
        }

        fn organizer(&self) -> Option<Party<'_>> {
            Some(Party::read(CHAIR.as_bytes(), None))
        }

        fn attendee_count(&self) -> usize {
            self.count
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            (index < self.count).then(|| Attendee::new(Party::read(ANN.as_bytes(), None)))
        }

        fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
            (index < self.count).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
        }

        fn property_count(&self) -> usize {
            self.count.saturating_add(1)
        }

        fn property_name(&self, index: usize) -> Option<&[u8]> {
            match index {
                0 => Some(&b"UID"[..]),
                seen if seen <= self.count => Some(&b"ATTENDEE"[..]),
                _ => None,
            }
        }

        fn property_line(&self, index: usize) -> Option<&[u8]> {
            match index {
                0 => Some(MEETING_LINE.as_bytes()),
                seen if seen <= self.count => Some(CROWD_LINE.as_bytes()),
                _ => None,
            }
        }

        fn child_count(&self) -> usize {
            0
        }

        fn child(&self, _index: usize) -> Option<&dyn ScheduledComponent> {
            None
        }
    }

    /// The event the recipient already holds, with no `SEQUENCE` property at all.
    fn held_meeting() -> Node {
        Node::event()
            .about(MEETING)
            .stamped(EARLY)
            .property("DTSTART", "20260901T150000Z")
            .property("SUMMARY", "Quarterly review")
            .organized_by(CHAIR)
            .guest(Guest::new(ANN))
            .guest(Guest::new(BO))
    }

    /// The same event at revision `sequence`.
    fn held_at(sequence: u32) -> Node {
        held_meeting().at_revision(sequence)
    }

    /// A component the caller does not hold: no `UID`, which is what `prior_state` reads.
    fn holds_nothing() -> Node {
        Node::event()
    }

    /// A `REPLY` payload from `who`, accepting, echoing the revision it answers.
    fn reply_from(who: &str) -> Node {
        Node::event()
            .about(MEETING)
            .stamped(LATE)
            .at_revision(2)
            .organized_by(CHAIR)
            .guest(Guest::new(who).answering("ACCEPTED"))
    }

    /// An organizer's update of the summary, at revision `sequence`.
    ///
    /// Stamped later than the state it updates, which is what a message is: RFC 5545 section
    /// 3.8.7.2 makes `DTSTAMP` the time the object was created, and RFC 5546 section 2.1.4 only
    /// requires `SEQUENCE` to move for a *significant* change — so a summary edit is ordered by
    /// the stamp alone, and a message carrying neither a newer number nor a newer stamp is the
    /// version already held rather than an update of it.
    fn request_at(sequence: u32) -> Node {
        held_at(sequence)
            .stamped(LATE)
            .property("SUMMARY", "Quarterly review, room 2")
    }

    /// The identity `address` names.
    fn party(address: &str) -> PartyId<'_> {
        PartyId::new(address)
    }

    /// The offset `seconds` names, falling back on UTC so that a helper needs no unwrap.
    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap_or(UtcOffset::UTC)
    }

    /// The zone answer for the night `America/New_York` falls back through `01:30`.
    fn fold() -> LocalResolution {
        LocalResolution::Ambiguous {
            earlier: Reading::new(
                Instant::from_unix_seconds(BEFORE_THE_FALL),
                offset(-14_400),
                true,
            ),
            later: Reading::new(
                Instant::from_unix_seconds(AFTER_THE_FALL),
                offset(-18_000),
                false,
            ),
        }
    }

    /// The half of that repeated hour whose real instant is `named`.
    ///
    /// One key and one side: the key is the wall clock both halves share, and the side is
    /// something the zone answered — never something the octets said.
    fn half(named: i64) -> InstanceRef {
        unresolved_half().with_side(FoldSide::from_resolution(
            fold(),
            Some(Instant::from_unix_seconds(named)),
        ))
    }

    /// The same key with nothing resolved, which is what a caller holding no zone has.
    fn unresolved_half() -> InstanceRef {
        InstanceRef::new(
            Instant::from_unix_seconds(REPEATED_HOUR),
            InstanceClock::Zoned,
            OverrideRange::ThisOnly,
        )
    }

    /// The line both halves of the fold carry, which is the same octets for either.
    const FOLD_LINE: &str = "RECURRENCE-ID;TZID=America/New_York:20261101T013000";

    /// An override reaching this instance and every later one.
    fn onwards() -> InstanceRef {
        InstanceRef::new(
            Instant::from_unix_seconds(SEPTEMBER),
            InstanceClock::Utc,
            OverrideRange::ThisAndFuture,
        )
        .with_side(FoldSide::Once)
    }

    /// The line such an override carries.
    const ONWARDS_LINE: &str = "RECURRENCE-ID;RANGE=THISANDFUTURE:20260901T150000Z";

    /// Read `calendar` as a message under the default policy, or say the fixture is not one.
    fn message_of<'a>(calendar: &'a Node, meter: &mut Meter) -> ItipMessage<'a> {
        let mut quiet = IgnoreDiagnostics;
        match ItipMessage::read(calendar, Limits::DEFAULT, meter, &mut quiet) {
            Ok(message) => message,
            Err(error) => panic!("the fixture is not a scheduling message: {error:?}"),
        }
    }

    /// Why `calendar` was refused as a message under the default policy.
    fn refusal_of(calendar: &Node) -> Option<MessageError> {
        let mut quiet = IgnoreDiagnostics;
        let mut meter = Meter::new(Limits::DEFAULT);
        ItipMessage::read(calendar, Limits::DEFAULT, &mut meter, &mut quiet).err()
    }

    /// The one value here designed to cross a request boundary.
    ///
    /// `T: 'static` is the machine-checked half of this crate's claim: a [`Commitment`] owns
    /// everything it holds and can be encoded, and an `Authorization` cannot be handed to this
    /// function at all — it borrows both of its inputs, so no lifetime satisfies the bound.
    fn crosses_bytes<T: 'static + Clone + PartialEq + Debug>(value: &T) {
        assert_eq!(value.clone(), *value);
    }

    /// The baseline: the party RFC 5546 section 1.3 names may make the change, and the
    /// transition says which kind of change section 3.2.2 calls it.
    #[test]
    fn a_request_from_the_organizer_is_authorized_and_names_the_change_it_makes() {
        let current = held_at(2);
        let mut meter = Meter::new(Limits::DEFAULT);

        let renamed = Node::calendar("REQUEST", request_at(2));
        let message = message_of(&renamed, &mut meter);
        let authorized = evaluate_message(&message, &current, party(CHAIR))
            .expect("the organizer may update the component the organizer organizes");
        assert_eq!(authorized.reason(), TransitionReason::Updated);
        assert_eq!(authorized.actor(), ActorRole::Organizer);
        // The summary, and the stamp that says this is a newer statement of one revision.
        assert_eq!(authorized.transition().len(), 2);
        assert!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"SUMMARY", 0))
                .is_some()
        );
        assert!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"DTSTAMP", 0))
                .is_some()
        );

        // Section 3.2.2.1: a rescheduling `REQUEST` increments `SEQUENCE`, so the transition
        // carries both changes and is reported as a move rather than as an edit.
        let moved = Node::calendar(
            "REQUEST",
            held_at(3).property("DTSTART", "20260902T150000Z"),
        );
        let message = message_of(&moved, &mut meter);
        let authorized = evaluate_message(&message, &current, party(CHAIR))
            .expect("the organizer may move the meeting");
        assert_eq!(authorized.reason(), TransitionReason::Rescheduled);
        assert_eq!(authorized.transition().len(), 2);

        // RFC 5546 section 2.1.4: two messages at one revision are one version, and one of them
        // is not the one the organizer sent. Neither newer nor older is not an update, so the
        // same edit carrying neither a newer number nor a newer stamp describes nothing at all
        // — which is also what makes a message that has already been applied idempotent when it
        // arrives a second time.
        let restated = Node::calendar("REQUEST", request_at(2).stamped(EARLY));
        let message = message_of(&restated, &mut meter);
        let authorized = evaluate_message(&message, &current, party(CHAIR))
            .expect("a restatement is not a refusal, it is a message with nothing new in it");
        assert!(
            authorized.transition().is_empty(),
            "an equal revision restating a different summary described a change"
        );
        assert_eq!(authorized.reason(), TransitionReason::Updated);
    }

    /// `SECURITY.md`'s first named attack, in both of the shapes it has.
    ///
    /// Through a `REPLY` the meeting does not move, and it does not move because nothing in a
    /// reply but the sender's own answer is ever described — RFC 5546 section 3.2.3 says an
    /// attendee's other properties MUST NOT differ, and this crate's answer to one that does
    /// is to ignore it rather than to refuse the message. That divergence is worth stating:
    /// the security property holds, the strict-conformance one does not.
    ///
    /// Through a `COUNTER`, where the diff does describe the whole component, the same
    /// overreach is refused whole and names the occurrence it was refused for.
    #[test]
    fn an_attendee_cannot_move_a_meeting_by_replying_to_it() {
        let current = held_at(2);
        let mut meter = Meter::new(Limits::DEFAULT);

        let calendar = Node::calendar(
            "REPLY",
            reply_from(ANN).property("DTSTART", "20260902T150000Z"),
        );
        let message = message_of(&calendar, &mut meter);
        let authorized = evaluate_message(&message, &current, party(ANN))
            .expect("an attendee may answer an invitation");
        assert_eq!(authorized.reason(), TransitionReason::ParticipationChanged);
        assert_eq!(
            authorized.transition().len(),
            1,
            "a reply describes one answer and nothing else"
        );
        assert!(matches!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"ATTENDEE", 0)),
            Some(ProposedChange::SetParameters(_))
        ));
        assert!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"DTSTART", 0))
                .is_none(),
            "the meeting moves only if something describes it moving"
        );

        let countered = Node::calendar(
            "COUNTER",
            held_at(2)
                .stamped(LATE)
                .property("DTSTART", "20260902T150000Z"),
        );
        let message = message_of(&countered, &mut meter);
        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(b"DTSTART", 0)
            )),
            "section 3.2.2's restriction table: the start time is the organizer's"
        );

        // ADR-0005: a refused message stays inspectable, so a user can be shown what it tried.
        let attempted = describe_message(&message, &current);
        match attempted.change(&PropertyOccurrence::named(b"DTSTART", 0)) {
            Some(ProposedChange::Replace(text)) => {
                assert_eq!(text.as_bytes(), b"DTSTART:20260902T150000Z");
            },
            other => panic!("the refused counter must stay describable, and was {other:?}"),
        }
    }

    /// `SECURITY.md`'s second named attack. RFC 5546 section 3.2.3: a `REPLY` comes from an
    /// attendee of the component, so an address on neither list is a refusal and never a
    /// silently added participant.
    #[test]
    fn a_reply_from_an_address_on_no_list_is_refused_rather_than_added() {
        let current = held_at(2);
        let calendar = Node::calendar("REPLY", reply_from(ANN));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(MALLORY));
        assert_eq!(verdict.err(), Some(AuthorizationDenied::UnknownAttendee));
    }

    /// The same attack from the inside: a sender who *is* invited, naming somebody who is not.
    ///
    /// Two shapes. A reply about a stranger describes nothing, because inventing the
    /// participant is exactly what the gate exists to refuse. A reply that states two
    /// `ATTENDEE` lines fails section 3.2.3's table outright, which prints `1` for that row.
    #[test]
    fn a_reply_cannot_add_a_participant_nobody_invited() {
        let current = held_at(2);
        let mut meter = Meter::new(Limits::DEFAULT);

        let about_a_stranger = Node::calendar("REPLY", reply_from(MALLORY));
        let message = message_of(&about_a_stranger, &mut meter);
        let authorized = evaluate_message(&message, &current, party(ANN))
            .expect("an invited attendee may send a reply");
        assert!(
            authorized.transition().is_empty(),
            "an address the local copy does not carry is described as no change at all"
        );

        let two_lines = Node::calendar(
            "REPLY",
            reply_from(ANN).guest(Guest::new(MALLORY).answering("ACCEPTED")),
        );
        let message = message_of(&two_lines, &mut meter);
        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(b"ATTENDEE", 1)
            )),
            "section 3.2.3 prints ATTENDEE as 1, and the second line is the one it does not \
             admit — the row is read in both directions, so a message with too many of a name \
             is refused at the occurrence that is one too many rather than reported as lacking \
             the name it plainly carries"
        );
    }

    /// RFC 5546 section 3.2.3 again, from the other side: the `ATTENDEE` of a `REPLY` is the
    /// replying attendee. Answering for somebody else is refused at their own occurrence.
    #[test]
    fn an_attendee_may_not_answer_for_another() {
        let current = held_at(2);
        let calendar = Node::calendar("REPLY", reply_from(BO));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(b"ATTENDEE", 1)
            )),
            "the second ATTENDEE line is not the sender's own"
        );
    }

    /// `SECURITY.md`'s third named attack, and RFC 5546 section 2.1.4: a lower `SEQUENCE` is an
    /// older version and an older version never overwrites a newer one.
    #[test]
    fn an_older_sequence_never_overwrites_a_newer_one() {
        let current = held_at(2);
        let calendar = Node::calendar("REQUEST", request_at(1).stamped(LATE));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(CHAIR));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::SequenceStale { have: 2 }),
            "a later DTSTAMP does not buy back a lower SEQUENCE"
        );
    }

    /// RFC 5546 section 2.1.5: `DTSTAMP` breaks the tie `SEQUENCE` leaves, and it breaks it
    /// towards the version already held.
    #[test]
    fn an_equal_sequence_with_an_older_dtstamp_never_overwrites_one() {
        let current = held_at(2).stamped(LATE);
        let calendar = Node::calendar("REQUEST", request_at(2).stamped(EARLY));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(CHAIR));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::DtstampStale {
                have: Instant::from_unix_seconds(LATE.at)
            })
        );
    }

    /// RFC 5546 section 3.2 reads an absent `SEQUENCE` as zero. Zero is a revision, so such a
    /// message is *stale* against a held revision 3 rather than unknown — and it is accepted
    /// against a held revision that is also zero, which is what makes it a number and not a
    /// missing value.
    #[test]
    fn a_message_with_no_sequence_is_revision_zero_and_not_unknown() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let calendar = Node::calendar("REQUEST", held_meeting().stamped(LATE));
        let message = message_of(&calendar, &mut meter);

        let numbered = held_at(3);
        let verdict = evaluate_message(&message, &numbered, party(CHAIR));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::SequenceStale { have: 3 })
        );

        let unnumbered = held_meeting();
        assert!(
            evaluate_message(&message, &unnumbered, party(CHAIR)).is_ok(),
            "zero against zero is a tie the DTSTAMP settles, not a refusal"
        );
    }

    /// A `SEQUENCE` that is present and is not an integer is the absence of a revision, and a
    /// message with no revision cannot be held against the one a caller has.
    #[test]
    fn a_sequence_that_is_not_a_number_is_no_revision_at_all() {
        let current = held_at(2);
        let calendar = Node::calendar("REQUEST", request_at(2).unreadable_sequence());
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(CHAIR));
        assert_eq!(verdict.err(), Some(AuthorizationDenied::SequenceUnreadable));
    }

    /// RFC 5545 section 3.8.4.7 gives a `UID` no case folding and no whitespace stripping, so
    /// an identifier differing by either is another meeting. Folding them is how a `CANCEL` for
    /// one meeting cancels a different one.
    #[test]
    fn a_uid_that_differs_by_case_or_by_whitespace_is_another_meeting() {
        let current = held_at(2);
        let mut meter = Meter::new(Limits::DEFAULT);
        let disguises = [
            ("4F1B-9A@EXAMPLE.COM", "only the case differs"),
            ("4f1b-9a@example.com ", "only a trailing space differs"),
        ];

        for (uid, why) in disguises {
            let calendar = Node::calendar("REQUEST", request_at(3).about(uid));
            let message = message_of(&calendar, &mut meter);
            let verdict = evaluate_message(&message, &current, party(CHAIR));
            assert_eq!(
                verdict.err(),
                Some(AuthorizationDenied::UidMismatch),
                "{why}"
            );
        }
    }

    /// A `REPLY` answers the instance it names and not every later one, so a `RECURRENCE-ID`
    /// reaching `THISANDFUTURE` is refused under that method. The same reference under a
    /// `REQUEST`, which is the organizer's to make, is not.
    #[test]
    fn a_reply_may_not_reach_this_and_future() {
        let current = held_at(2).addressing(onwards(), ONWARDS_LINE);
        let mut meter = Meter::new(Limits::DEFAULT);

        let answered = Node::calendar("REPLY", reply_from(ANN).addressing(onwards(), ONWARDS_LINE));
        let message = message_of(&answered, &mut meter);
        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(verdict.err(), Some(AuthorizationDenied::RangeNotPermitted));

        let organized = Node::calendar("REQUEST", held_at(2).addressing(onwards(), ONWARDS_LINE));
        let message = message_of(&organized, &mut meter);
        assert!(
            evaluate_message(&message, &current, party(CHAIR)).is_ok(),
            "the range is a claim the organizer is entitled to make"
        );
    }

    /// RFC 5546 section 1.3 divides the exchange into two roles, and section 3's prose names
    /// which role authors each method. An organizer does not reply and an attendee does not
    /// cancel; the second is how one participant cancels everybody's meeting.
    #[test]
    fn only_the_party_rfc_5546_names_may_send_a_method() {
        let current = held_at(2);
        let mut meter = Meter::new(Limits::DEFAULT);

        let replied = Node::calendar("REPLY", reply_from(ANN));
        let message = message_of(&replied, &mut meter);
        let verdict = evaluate_message(&message, &current, party(CHAIR));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::MethodForbidsSender(
                ActorRole::Organizer
            )),
            "section 3.2.3 is an attendee's method"
        );

        let cancelled = Node::calendar("CANCEL", held_at(3));
        let message = message_of(&cancelled, &mut meter);
        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::MethodForbidsSender(
                ActorRole::Attendee
            )),
            "section 3.2.5 is the organizer's"
        );
    }

    /// RFC 5546 section 3.2.5 cancels a component the recipient has. A `CANCEL` for a meeting
    /// nobody holds is refused at the first gate, before any of the rest is consulted.
    #[test]
    fn a_cancel_for_a_meeting_the_caller_does_not_hold_is_refused() {
        let current = holds_nothing();
        let calendar = Node::calendar("CANCEL", held_at(3));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(CHAIR));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::PriorStateForbidden(PriorState::Absent))
        );
    }

    /// A message whose `UID` matches and whose `RECURRENCE-ID` names an instance the series
    /// does not have addresses nothing, and is refused rather than applied to the series.
    #[test]
    fn a_reply_naming_an_instance_the_series_does_not_have_is_refused() {
        let current = held_at(2);
        let calendar = Node::calendar("REPLY", reply_from(ANN).addressing(onwards(), ONWARDS_LINE));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::NoMatchingInstance),
            "the series and one of its instances are two things to send a message about"
        );
    }

    /// Agenda item 1 as an attack. The two halves of the hour `America/New_York` repeats are
    /// one cadence key and two meetings: resolved, a reply reaches the half it names and no
    /// other, and the octets of the two `RECURRENCE-ID` lines are identical.
    #[test]
    fn the_two_halves_of_one_repeated_hour_are_two_meetings() {
        assert_eq!(
            half(BEFORE_THE_FALL).named(),
            half(AFTER_THE_FALL).named(),
            "one key, which is what makes this an attack rather than a lookup"
        );
        assert_eq!(half(BEFORE_THE_FALL).side(), FoldSide::Earlier);
        assert_eq!(half(AFTER_THE_FALL).side(), FoldSide::Later);

        let current = held_at(2).addressing(half(BEFORE_THE_FALL), FOLD_LINE);
        let mut meter = Meter::new(Limits::DEFAULT);

        let answered = Node::calendar(
            "REPLY",
            reply_from(ANN).addressing(half(BEFORE_THE_FALL), FOLD_LINE),
        );
        let message = message_of(&answered, &mut meter);
        let authorized = evaluate_message(&message, &current, party(ANN))
            .expect("the reply that names the meeting the caller holds");
        assert_eq!(authorized.reason(), TransitionReason::ParticipationChanged);

        let across = Node::calendar(
            "REPLY",
            reply_from(ANN).addressing(half(AFTER_THE_FALL), FOLD_LINE),
        );
        let message = message_of(&across, &mut meter);
        let verdict = evaluate_message(&message, &current, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::NoMatchingInstance),
            "the other half of the fold is somebody else's meeting"
        );
    }

    /// The same collision with nothing to resolve it, from either side. An ambiguous match is
    /// not a match: a guess between the two halves cancels or moves the wrong meeting.
    #[test]
    fn an_instance_nothing_resolved_is_refused_rather_than_guessed() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let resolved = held_at(2).addressing(half(BEFORE_THE_FALL), FOLD_LINE);
        let unresolved = held_at(2).addressing(unresolved_half(), FOLD_LINE);

        let vague = Node::calendar(
            "REPLY",
            reply_from(ANN).addressing(unresolved_half(), FOLD_LINE),
        );
        let message = message_of(&vague, &mut meter);
        let verdict = evaluate_message(&message, &resolved, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::AmbiguousInstance),
            "the message named a key and no side"
        );

        let precise = Node::calendar(
            "REPLY",
            reply_from(ANN).addressing(half(BEFORE_THE_FALL), FOLD_LINE),
        );
        let message = message_of(&precise, &mut meter);
        let verdict = evaluate_message(&message, &unresolved, party(ANN));
        assert_eq!(
            verdict.err(),
            Some(AuthorizationDenied::AmbiguousInstance),
            "a caller holding no zone gets the conservative answer, not a pick"
        );
    }

    /// ADR-0010, and the reason this crate refuses a limit breach instead of reporting one:
    /// truncating the list would turn "this party may reply" into "this party is unknown", and
    /// an attacker who can pad a list past the threshold would pick which the server believes.
    #[test]
    fn a_hundred_thousand_attendees_refuse_the_whole_message() {
        assert!(MEETING_LINE.ends_with(MEETING) && CROWD_LINE.ends_with(ANN));
        let ceiling = usize::try_from(Limits::DEFAULT.max_attendees()).unwrap_or(usize::MAX);
        assert!(ceiling < 100_000, "the fixture has to exceed the policy");

        let calendar = Node::calendar("REQUEST", Crowd { count: 100_000 });
        assert_eq!(refusal_of(&calendar), Some(MessageError::TooManyAttendees));

        let admitted = Node::calendar("REQUEST", Crowd { count: ceiling });
        assert_eq!(
            refusal_of(&admitted),
            None,
            "the bound is the policy's number and not a number this test invented"
        );
    }

    /// Nesting is attacker-chosen, so the ceiling is the caller's policy and one level past it
    /// is a refusal of the whole message rather than a deep walk that reports afterwards.
    #[test]
    fn a_payload_nested_past_the_ceiling_is_refused() {
        let ceiling = usize::from(Limits::DEFAULT.max_component_depth());

        let deep = Node::calendar("REQUEST", nested_payload(ceiling));
        assert_eq!(refusal_of(&deep), Some(MessageError::TooDeep));

        let admitted = Node::calendar("REQUEST", nested_payload(ceiling.saturating_sub(1)));
        assert_eq!(
            refusal_of(&admitted),
            None,
            "one level shallower is inside the policy and must still read"
        );
    }

    /// The payload the recipient holds, with `alarms` components chained beneath it.
    ///
    /// The chain is built inner-first because a component owns the ones inside it, and a
    /// hostile message's nesting is exactly this shape: legal at every level, and unbounded.
    fn nested_payload(alarms: usize) -> Node {
        let mut deepest = Node::alarm();
        for _ in 1..alarms {
            deepest = Node::alarm().containing(deepest);
        }
        held_at(2).containing(deepest)
    }

    /// ADR-0010's aggregate bound, which is the whole reason [`Meter`] outlives one call: a
    /// thousand individually bounded messages are bounded together only if they share a
    /// ledger. The second half is the documented negative — moving the ledger inside the loop
    /// reproduces the attack exactly, and no gate in this workspace sees that caller's code.
    #[test]
    fn a_thousand_counters_are_bounded_only_by_a_shared_ledger() {
        let counters: Vec<Node> = (0..1000_u32)
            .map(|step| Node::calendar("COUNTER", held_at(step).stamped(LATE)))
            .collect();

        let mut shared = Meter::with_budget(Limits::DEFAULT, 1_000);
        let mut quiet = IgnoreDiagnostics;
        let mut taken = 0_usize;
        for link in &counters {
            match ItipMessage::read(link, Limits::DEFAULT, &mut shared, &mut quiet) {
                Ok(_) => taken = taken.saturating_add(1),
                Err(error) => {
                    assert_eq!(error, MessageError::BudgetExhausted);
                    break;
                },
            }
        }
        assert!(
            taken < counters.len(),
            "a shared ledger has to stop the chain somewhere"
        );

        let mut spent = 0_usize;
        for link in &counters {
            let mut fresh = Meter::with_budget(Limits::DEFAULT, 1_000);
            if ItipMessage::read(link, Limits::DEFAULT, &mut fresh, &mut quiet).is_ok() {
                spent = spent.saturating_add(1);
            }
        }
        assert_eq!(
            spent,
            counters.len(),
            "a ledger per message bounds each and bounds nothing in aggregate"
        );
    }

    /// The byte boundary, in both directions.
    ///
    /// An `Authorization` borrows the message and the state, so the propose turn below can let
    /// exactly one value out of the scope its inputs live in — the [`Commitment`], which is
    /// owned and carries no authority. Writing `let carried = { .. authorized }` instead does
    /// not compile, and that is the whole improvement over a wrapper documented as
    /// unserializable: there is no owned form to encode, to store in a session, or to forge.
    ///
    /// The confirm turn then re-evaluates against freshly read state. It gets a decision from
    /// the gate either way — the commitment is never consulted to grant — and the commitment
    /// is what tells it that the target moved while the user was deciding.
    #[test]
    fn an_authorization_cannot_cross_a_request_boundary_and_a_commitment_refuses_a_moved_target() {
        let shown = {
            let calendar = Node::calendar("REQUEST", request_at(5));
            let current = held_at(2);
            let mut meter = Meter::new(Limits::DEFAULT);
            let message = message_of(&calendar, &mut meter);
            let authorized = evaluate_message(&message, &current, party(CHAIR))
                .expect("the organizer's own update is authorized");
            let commitment = Commitment::of(&authorized);
            assert!(
                authorized.honors(&commitment),
                "a decision honors the record made of it"
            );
            commitment
        };
        crosses_bytes(&shown);
        assert_eq!(shown.held().map(Revision::sequence), Some(2));
        assert_eq!(shown.offered().sequence(), 5);
        assert_eq!(shown.reason(), TransitionReason::Updated);

        let calendar = Node::calendar("REQUEST", request_at(5));
        let mut meter = Meter::new(Limits::DEFAULT);
        let message = message_of(&calendar, &mut meter);

        let moved = held_at(3);
        let authorized = evaluate_message(&message, &moved, party(CHAIR))
            .expect("the gate runs fresh whatever the commitment says");
        assert!(
            !authorized.honors(&shown),
            "the target moved, and the confirm turn is told rather than left to overwrite"
        );

        let unmoved = held_at(2);
        let steady = evaluate_message(&message, &unmoved, party(CHAIR))
            .expect("the same message against the same state is still authorized");
        assert!(
            steady.honors(&shown),
            "the refusal above is about the target moving and not about the turn being second"
        );
    }
}
