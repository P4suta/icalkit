// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ical-itip` attacked by the party `SECURITY.md` names: somebody who was merely invited.
//!
//! The claim under attack is the one the crate's own documentation makes twice — "an attendee
//! cannot move a meeting by replying", and, more broadly, that authorization is the first half
//! of the semantics rather than a layer above them. Every case below supplies the gate a real
//! message, a real prior state, and an actor, and reads back what `evaluate_message` answered.
//! Nothing here reasons about the implementation: the transitions asserted are the ones the
//! gate returned when this file was run.
//!
//! The state every case is judged against is `held_series.ics`: `SEQUENCE:2`, organized by
//! `chair@example.com` through the assistant `pa@example.com`, with `cy@example.com` first and
//! `bo@example.com` second on the attendee list. `bo` is the attacker throughout — an ordinary
//! invitee with no organizer rights whatsoever — and `eve@example.com` is a party the meeting
//! has never named.

/// Every case in this chapter, in a module so that the helpers beside them are test
/// code to the lint table as well as to the compiler.
#[cfg(test)]
mod attacker {
    use icalkit_conformance::internal::core::{
        CivilDate, CivilDateTime, CivilTime, ComponentKind, Diagnostic, Instant, Limits, Meter,
        ProposedChange, RawText,
    };
    use icalkit_conformance::internal::itip::{
        ActorRole, Attendee, AuthorizationDenied, Commitment, InstanceClock, InstanceRef,
        ItipMessage, Party, PartyId, PropertyOccurrence, ScheduleTarget, ScheduledComponent,
        SequenceRead, TransitionReason, WriteRejected, apply_transition, evaluate_message,
    };
    use icalkit_conformance::internal::recur::OverrideRange;
    use icalkit_conformance::internal::tz::nominal;

    /// The weekly series the caller holds: `SEQUENCE:2`, two attendees, one organizer.
    const HELD: &[u8] = include_bytes!("fixtures/break_itip_attacker/held_series.ics");
    /// The same series after the attendee's `COUNTER` rewrote the `ORGANIZER` line.
    const HELD_AFTER_TAKEOVER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/held_after_takeover.ics");
    /// The same series after the attendee's `COUNTER` raised `SEQUENCE` to 99.
    const HELD_AFTER_SEQUENCE_RAISED: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/held_after_sequence_raised.ics");

    /// A `COUNTER` from the attendee naming itself `ORGANIZER` and changing nothing else.
    const COUNTER_REWRITES_ORGANIZER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_rewrites_the_organizer.ics");
    /// A `COUNTER` from the attendee raising `SEQUENCE` to 99 and changing nothing else.
    const COUNTER_RAISES_SEQUENCE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_raises_the_sequence.ics");
    /// A `COUNTER` from the attendee putting a stranger where its own `ATTENDEE` line was.
    const COUNTER_SUBSTITUTES_STRANGER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_substitutes_a_stranger.ics");
    /// A `COUNTER` from the attendee moving `DTSTART` nine hours.
    const COUNTER_MOVES_THE_MEETING: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_moves_the_meeting.ics");
    /// A `COUNTER` from the attendee dropping the other attendee's line.
    const COUNTER_REMOVES_THE_OTHER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_removes_the_other_attendee.ics");
    /// A `COUNTER` from the attendee appending a third attendee nobody invited.
    const COUNTER_ADDS_A_THIRD: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_adds_a_third_attendee.ics");

    /// The same series after one attendee delegated, so `eve` is only a `DELEGATED-TO` value.
    const HELD_WITH_A_DELEGATION: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/held_with_a_delegation.ics");
    /// A `COUNTER` from that delegate, naming itself `ORGANIZER`.
    const COUNTER_FROM_THE_DELEGATE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/counter_from_the_delegate.ics");

    /// A `CANCEL` of the whole series, carrying the real organizer's line.
    const CANCEL_FROM_AN_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/cancel_from_an_attendee.ics");
    /// The same `CANCEL` naming the attacker as `ORGANIZER`.
    const CANCEL_FROM_THE_NEW_ORGANIZER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/cancel_from_the_new_organizer.ics");
    /// A `REQUEST` from the attendee, naming itself `ORGANIZER` and moving the time.
    const REQUEST_FROM_AN_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/request_from_an_attendee.ics");
    /// The organizer's own `REQUEST` at `SEQUENCE:3`, the message the attacker wants to block.
    const REQUEST_FROM_THE_ORGANIZER: &[u8] = include_bytes!(
        "fixtures/break_itip_attacker/request_from_the_organizer_at_sequence_three.ics"
    );
    /// A `DECLINECOUNTER` from the attendee.
    const DECLINECOUNTER_FROM_AN_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/declinecounter_from_an_attendee.ics");
    /// An `ADD` from the attendee, smuggling a `DTSTART` the caller never had.
    const ADD_FROM_AN_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/add_from_an_attendee.ics");

    /// A `REPLY` from the attendee restating `DTSTART`, `LOCATION` and `RRULE`.
    const REPLY_MOVES_THE_MEETING: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_moves_the_meeting.ics");
    /// A `REPLY` from the attendee carrying a second `ATTENDEE` for a party who never existed.
    const REPLY_ADDS_A_STRANGER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_adds_a_stranger.ics");
    /// A `REPLY` whose only `ATTENDEE` is a party the meeting never named.
    const REPLY_FROM_A_STRANGER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_from_a_stranger.ics");
    /// A `REPLY` from the attacker answering on the other attendee's line.
    const REPLY_ANSWERS_FOR_ANOTHER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_answers_for_another_attendee.ics");
    /// A `REPLY` from the attendee that also names itself `ORGANIZER`.
    const REPLY_REWRITES_ORGANIZER: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_rewrites_the_organizer.ics");
    /// A `REPLY` whose address differs from the held one only by the case of its local part.
    const REPLY_LOCAL_PART_CASE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_with_a_local_part_case_shift.ics");
    /// A `REPLY` whose address differs only by the case of its scheme and domain.
    const REPLY_DOMAIN_CASE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_with_a_domain_case_shift.ics");
    /// A `REPLY` whose address drops the `mailto:` scheme.
    const REPLY_NO_SCHEME: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_without_the_mailto_scheme.ics");
    /// A `REPLY` whose address carries one trailing space.
    const REPLY_TRAILING_SPACE: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_with_trailing_whitespace.ics");
    /// A `REPLY` whose domain spells `example` with a Cyrillic `а`.
    const REPLY_HOMOGRAPH: &[u8] =
        include_bytes!("fixtures/break_itip_attacker/reply_with_a_homograph_domain.ics");

    /// The attacker: second on the caller's attendee list, and nothing else.
    const BO: &str = "mailto:bo@example.com";
    /// The other attendee, whose participation the attacker must not be able to state.
    const CY: &str = "mailto:cy@example.com";
    /// The organizer of every held fixture.
    const CHAIR: &str = "mailto:chair@example.com";
    /// A party the meeting has never named.
    const EVE: &str = "mailto:eve@example.com";

    /// One content line, unfolded and taken apart, with its parameter values resolved.
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
                    let (key, rest) = chunk.split_at(split);
                    let raw = rest.get(1..).unwrap_or_default();
                    let bare = raw
                        .strip_prefix(b"\"")
                        .and_then(|inner| inner.strip_suffix(b"\""))
                        .unwrap_or(raw);
                    Some((key.to_ascii_uppercase(), bare.to_vec()))
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
                .find(|(key, _)| key.as_slice() == name)
                .map(|(_, value)| value.as_slice())
        }
    }

    /// One component of a fixture, answering the questions `ical-itip` asks of held state.
    #[derive(Clone, Debug, Default)]
    struct Subject {
        /// What the `BEGIN` line named, `None` for a name RFC 5545 does not define.
        kind: Option<ComponentKind>,
        /// The properties directly inside this component, in document order.
        properties: Vec<Line>,
        /// The components directly inside it, in document order.
        children: Vec<Subject>,
    }

    impl Subject {
        /// The component the caller holds: the calendar's first child.
        fn component(&self) -> &Self {
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

        /// Every `ATTENDEE` line, in document order.
        fn attendee_lines(&self) -> impl Iterator<Item = &Line> {
            self.properties
                .iter()
                .filter(|line| line.is_named(b"ATTENDEE"))
        }
    }

    impl ScheduledComponent for Subject {
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
                Some(digits) => {
                    number(digits).map_or(SequenceRead::Unreadable, SequenceRead::Value)
                },
            }
        }

        fn dtstamp(&self) -> Option<Instant> {
            instant_of(self.value(b"DTSTAMP")?)
        }

        fn recurrence_id(&self) -> Option<InstanceRef> {
            let line = self.line(b"RECURRENCE-ID")?;
            let named = instant_of(&line.value)?;
            let clock = if line.value.last() == Some(&b'Z') {
                InstanceClock::Utc
            } else if line.parameter(b"TZID").is_some() {
                InstanceClock::Zoned
            } else {
                InstanceClock::Floating
            };
            Some(InstanceRef::new(named, clock, OverrideRange::ThisOnly))
        }

        fn organizer(&self) -> Option<Party<'_>> {
            let line = self.line(b"ORGANIZER")?;
            Some(Party::read(&line.value, line.parameter(b"SENT-BY")))
        }

        fn attendee_count(&self) -> usize {
            self.attendee_lines().count()
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            let line = self.attendee_lines().nth(index)?;
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
            (index < self.attendee_count()).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
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

    /// Where an authorized transition is written, for the cases that write one.
    #[derive(Debug, Default)]
    struct Recorder {
        /// What the target took, in the order it was offered.
        written: Vec<(PropertyOccurrence, ProposedChange)>,
    }

    impl ScheduleTarget for Recorder {
        fn write_change(
            &mut self,
            at: &PropertyOccurrence,
            change: &ProposedChange,
        ) -> Result<(), WriteRejected> {
            self.written.push((at.clone(), change.clone()));
            Ok(())
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

    /// The calendar `source` spells.
    fn subject(source: &[u8]) -> Subject {
        let mut open: Vec<Subject> = Vec::new();
        let mut done: Option<Subject> = None;
        for content in unfold(source) {
            let Some(line) = Line::read(&content) else {
                continue;
            };
            if line.is_named(b"BEGIN") {
                open.push(Subject {
                    kind: ComponentKind::from_name(&line.value),
                    ..Subject::default()
                });
            } else if line.is_named(b"END") {
                let Some(finished) = open.pop() else { continue };
                match open.last_mut() {
                    Some(parent) => parent.children.push(finished),
                    None => done = Some(finished),
                }
            } else if let Some(current) = open.last_mut() {
                current.properties.push(line);
            }
        }
        done.unwrap_or_default()
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

    /// What the gate answered about one message, flattened so a case can assert on it whole.
    #[derive(Debug)]
    struct Answer {
        /// The role the gate resolved for the actor, absent when it refused.
        role: Option<ActorRole>,
        /// The kind of change, absent when it refused.
        reason: Option<TransitionReason>,
        /// Every change it authorized, as name, occurrence index and written octets.
        changes: Vec<(String, usize, String)>,
        /// Why it refused, absent when it did not.
        denied: Option<AuthorizationDenied>,
    }

    impl Answer {
        /// Whether the gate let this message through.
        fn allowed(&self) -> bool {
            self.denied.is_none()
        }

        /// Whether the authorized transition writes `text` anywhere.
        fn writes(&self, text: &str) -> bool {
            self.changes.iter().any(|(_, _, line)| line.contains(text))
        }

        /// Whether the authorized transition touches the property `name`.
        fn touches(&self, name: &str) -> bool {
            self.changes.iter().any(|(at, _, _)| at == name)
        }
    }

    /// Judge `message` against `held` on behalf of `actor`, and also write what was authorized.
    ///
    /// The write is the point of the case rather than an extra: a transition that is merely
    /// described has changed nothing, and a break is only a break once the octets land.
    fn judge(held_source: &[u8], message_source: &[u8], actor: &str) -> Answer {
        let held = subject(held_source);
        let calendar = subject(message_source);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let message = match ItipMessage::read(&calendar, limits, &mut meter, &mut sink) {
            Ok(message) => message,
            Err(error) => panic!("the message did not read at all: {error:?}"),
        };
        match evaluate_message(&message, held.component(), PartyId::new(actor)) {
            Ok(authorized) => {
                let role = authorized.actor();
                let reason = authorized.reason();
                let mut recorder = Recorder::default();
                let report = apply_transition(&mut recorder, authorized);
                assert!(report.is_complete(), "the recorder refuses nothing");
                let changes = recorder
                    .written
                    .iter()
                    .map(|(at, change)| {
                        let text = match change {
                            ProposedChange::Add(line) | ProposedChange::Replace(line) => {
                                String::from_utf8_lossy(line.as_bytes()).into_owned()
                            },
                            ProposedChange::SetParameters(edits) => edits
                                .iter()
                                .map(|edit| {
                                    format!(
                                        "{}={}",
                                        String::from_utf8_lossy(edit.name()),
                                        String::from_utf8_lossy(
                                            edit.value().unwrap_or(b"<removed>")
                                        )
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(";"),
                            ProposedChange::Remove => "<removed>".to_owned(),
                        };
                        (
                            String::from_utf8_lossy(at.name()).into_owned(),
                            at.index(),
                            text,
                        )
                    })
                    .collect();
                Answer {
                    role: Some(role),
                    reason: Some(reason),
                    changes,
                    denied: None,
                }
            },
            Err(denied) => Answer {
                role: None,
                reason: None,
                changes: Vec::new(),
                denied: Some(denied),
            },
        }
    }

    /// An attendee's `COUNTER` may rewrite the `ORGANIZER` line, and the gate authorizes it.
    ///
    /// `field_rule` gives `ORGANIZER` [`icalkit_conformance::internal::itip::FieldRule::EitherParty`], which is a statement
    /// about a party *echoing* the organizer's own line. The octet diff makes no distinction
    /// between echoing a line and replacing it, so an attendee-authored `COUNTER` whose every other
    /// property is byte-identical to the held copy describes exactly one change — `ORGANIZER`
    /// becoming the attacker — and passes the field gate.
    ///
    /// The second half of the case is what that buys: applied, the meeting's organizer is the
    /// attacker, and a `CANCEL` from the attacker against the resulting state is authorized. RFC
    /// 5546 section 3.2.7 gives an attendee no authority over the `ORGANIZER` property at all.
    #[test]
    fn a_counter_cannot_make_the_attendee_the_organizer() {
        let answer = judge(HELD, COUNTER_REWRITES_ORGANIZER, BO);
        assert!(
            !answer.allowed(),
            "an attendee rewrote the ORGANIZER line and the gate allowed it: {answer:?}"
        );
    }

    /// The consequence, recorded separately so it survives whatever the fix to the case above is.
    ///
    /// If the state the attendee's `COUNTER` produced is what the caller then holds, the attacker
    /// is the organizer of a meeting it was invited to, and cancelling it is authorized.
    ///
    /// The fix closes that at its cause and not at its effect, and the difference is worth
    /// stating because it is the whole of what this case now proves. A component whose
    /// `ORGANIZER` line names `bo` says that `bo` organizes it, and RFC 5546 section 1.3 lets one
    /// calendar user be both organizer and attendee — every real invitation lists the organizer
    /// on the attendee list — so no rule readable from that file alone can refuse `bo` there
    /// without refusing every organizer who attends their own meeting. What the gate can refuse
    /// is the message that would have written that line, and the state below is therefore
    /// reached the only way a caller can reach it: by applying what the gate authorized.
    ///
    /// `held_after_takeover.ics` stays in the corpus and stays judged, as the record of what the
    /// old answer bought: the attacker's `CANCEL` against it is still authorized, and the reason
    /// it never arrives is that the state cannot be produced.
    #[test]
    fn an_attendee_cannot_cancel_a_meeting_it_does_not_organize() {
        let direct = judge(HELD, CANCEL_FROM_AN_ATTENDEE, BO);
        assert_eq!(
            direct.denied,
            Some(AuthorizationDenied::MethodForbidsSender(
                ActorRole::Attendee
            )),
            "a CANCEL from an attendee must be refused on the sender rule"
        );

        // The takeover, offered. `judge` writes whatever it authorizes through
        // `apply_transition`, so a refusal is a state that did not move: the copy the next
        // message is judged against still names the organizer it always named.
        let after = judge(HELD, COUNTER_REWRITES_ORGANIZER, BO);
        assert!(!after.allowed(), "{after:?}");
        assert!(
            !after.touches("ORGANIZER"),
            "an attendee's COUNTER reached the ORGANIZER line of the caller's own copy: {after:?}"
        );

        let then = judge(HELD, CANCEL_FROM_THE_NEW_ORGANIZER, BO);
        assert_eq!(
            then.denied,
            Some(AuthorizationDenied::MethodForbidsSender(
                ActorRole::Attendee
            )),
            "the attacker is still an attendee of the meeting it tried to take over: {then:?}"
        );

        // And the state the old answer produced, judged for the record: a file that names `bo`
        // as its `ORGANIZER` authorizes `bo` to cancel it, which is why the fix is that no
        // message may write that line and not that this file is read some other way.
        let counterfactual = judge(HELD_AFTER_TAKEOVER, CANCEL_FROM_THE_NEW_ORGANIZER, BO);
        assert!(
            counterfactual.allowed(),
            "a component's own ORGANIZER line is the statement about who runs it: {counterfactual:?}"
        );
    }

    /// An attendee's `COUNTER` may raise `SEQUENCE`, which freezes the real organizer out.
    ///
    /// `field_rule` gave `SEQUENCE` [`icalkit_conformance::internal::itip::FieldRule::EitherParty`]. RFC 5546 section 2.1.4
    /// makes `SEQUENCE` the ordering of the *organizer's* revisions, and the gate's own replay
    /// defense reads it: once an attendee has written 99 into the caller's copy, the organizer's
    /// genuine `SEQUENCE:3` update is refused as stale. The attacker cannot forge a newer version,
    /// but it can make every real one look older.
    ///
    /// The lockout is closed at its cause. Nothing in a file distinguishes a `SEQUENCE:99` an
    /// attendee wrote from one the organizer wrote, and a rule that let a lower revision through
    /// on some other evidence — a fresher `DTSTAMP`, say — would be the replay defense answering
    /// to a number the sender chooses. So what this case now proves is that the revision the
    /// organizer's update is judged against is one no attendee could have moved: the `COUNTER` is
    /// refused, `judge` therefore writes nothing, and the organizer's `SEQUENCE:3` applies against
    /// the `SEQUENCE:2` the caller still holds.
    #[test]
    fn a_counter_cannot_raise_the_revision_an_attendee_does_not_own() {
        let answer = judge(HELD, COUNTER_RAISES_SEQUENCE, BO);
        assert!(
            !answer.allowed(),
            "an attendee raised SEQUENCE to 99 and the gate allowed it: {answer:?}"
        );
        assert!(
            !answer.touches("SEQUENCE"),
            "an attendee's COUNTER reached the revision of the caller's own copy: {answer:?}"
        );

        let organizer = judge(HELD, REQUEST_FROM_THE_ORGANIZER, CHAIR);
        assert!(
            organizer.allowed(),
            "the organizer's own update is refused against the revision it last wrote: \
             {organizer:?}"
        );

        // The state the old answer produced, judged for the record. Against a stored 99 the
        // organizer's SEQUENCE:3 is an older version and stays refused, which is section 2.1.4
        // working exactly as it must — and is why the attendee may not write that number.
        let locked = judge(
            HELD_AFTER_SEQUENCE_RAISED,
            REQUEST_FROM_THE_ORGANIZER,
            CHAIR,
        );
        assert_eq!(
            locked.denied,
            Some(AuthorizationDenied::SequenceStale { have: 99 }),
            "{locked:?}"
        );
    }

    /// An attendee's `COUNTER` may put a stranger on its own `ATTENDEE` occurrence.
    ///
    /// `FieldRule::AttendeeOwn` is checked as "the occurrence this actor sits at", not as "a line
    /// that still names this actor". A `COUNTER` replacing the attacker's own line with
    /// `eve@example.com` therefore passes: the meeting acquires a participant it never invited and
    /// loses the one it did, on one authorized change.
    #[test]
    fn a_counter_cannot_put_a_stranger_on_the_attendees_own_line() {
        let answer = judge(HELD, COUNTER_SUBSTITUTES_STRANGER, BO);
        assert!(
            !answer.writes(EVE),
            "an attendee wrote a stranger onto the attendee list: {answer:?}"
        );
    }

    /// The first attack `SECURITY.md` names, from both methods an attendee may send.
    ///
    /// A `REPLY` restating `DTSTART`, `LOCATION` and `RRULE` must describe none of them, and a
    /// `COUNTER` moving `DTSTART` must be refused outright.
    #[test]
    fn an_attendee_cannot_move_a_meeting() {
        let reply = judge(HELD, REPLY_MOVES_THE_MEETING, BO);
        assert!(reply.allowed(), "a plain acceptance is a normal reply");
        assert_eq!(reply.reason, Some(TransitionReason::ParticipationChanged));
        assert!(
            !reply.touches("DTSTART") && !reply.touches("RRULE") && !reply.touches("LOCATION"),
            "a REPLY described a change to a property that says when the meeting is: {reply:?}"
        );

        let counter = judge(HELD, COUNTER_MOVES_THE_MEETING, BO);
        assert_eq!(
            counter.denied,
            Some(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(b"DTSTART", 0)
            )),
            "a COUNTER that moves the meeting must be refused on the field rule"
        );
    }

    /// Every other way an attendee was asked to reach a line that is not its own.
    ///
    /// These are the cases that held. They are the security claim: a list of attacks that a gate
    /// refused is worth more than the absence of one.
    #[test]
    fn the_attendee_list_is_not_reachable_from_an_attendee() {
        let removes = judge(HELD, COUNTER_REMOVES_THE_OTHER, BO);
        assert!(!removes.allowed(), "{removes:?}");

        let adds = judge(HELD, COUNTER_ADDS_A_THIRD, BO);
        assert!(!adds.allowed(), "{adds:?}");

        let answers_for_another = judge(HELD, REPLY_ANSWERS_FOR_ANOTHER, BO);
        assert_eq!(
            answers_for_another.denied,
            Some(AuthorizationDenied::MethodForbidsField(
                PropertyOccurrence::named(b"ATTENDEE", 0)
            ))
        );

        let stranger = judge(HELD, REPLY_FROM_A_STRANGER, EVE);
        assert_eq!(
            stranger.denied,
            Some(AuthorizationDenied::UnknownAttendee),
            "a REPLY from an address nobody invited must be refused"
        );

        let second_line = judge(HELD, REPLY_ADDS_A_STRANGER, BO);
        assert!(
            !second_line.writes(EVE),
            "a REPLY's second ATTENDEE line reached the caller's copy: {second_line:?}"
        );
    }

    /// The methods RFC 5546 reserves to the organizer, sent by an attendee.
    #[test]
    fn an_attendee_may_not_send_an_organizer_authored_method() {
        for (name, source) in [
            ("REQUEST", REQUEST_FROM_AN_ATTENDEE),
            ("DECLINECOUNTER", DECLINECOUNTER_FROM_AN_ATTENDEE),
            ("ADD", ADD_FROM_AN_ATTENDEE),
            ("CANCEL", CANCEL_FROM_AN_ATTENDEE),
        ] {
            let answer = judge(HELD, source, BO);
            assert_eq!(
                answer.denied,
                Some(AuthorizationDenied::MethodForbidsSender(
                    ActorRole::Attendee
                )),
                "{name} from an attendee"
            );
        }
    }

    /// Identity, in the four shapes that differ from the held address by something invisible.
    ///
    /// RFC 5321 section 2.4 makes the domain the receiver's to case-fold and the mailbox nobody's,
    /// so `BO@` is a different party and `EXAMPLE.COM` is the same one. A missing scheme, a
    /// trailing space and a Cyrillic `а` are all different parties.
    #[test]
    fn an_address_that_is_not_the_attendees_matches_nobody() {
        for (name, source, actor) in [
            (
                "local part case",
                REPLY_LOCAL_PART_CASE,
                "mailto:BO@example.com",
            ),
            ("no scheme", REPLY_NO_SCHEME, "bo@example.com"),
            (
                "trailing space",
                REPLY_TRAILING_SPACE,
                "mailto:bo@example.com ",
            ),
            ("homograph", REPLY_HOMOGRAPH, "mailto:bo@exаmple.com"),
        ] {
            let answer = judge(HELD, source, actor);
            assert_eq!(
                answer.denied,
                Some(AuthorizationDenied::UnknownAttendee),
                "{name} was accepted as the attendee it is not: {answer:?}"
            );
        }

        let folded = judge(HELD, REPLY_DOMAIN_CASE, "MAILTO:bo@EXAMPLE.COM");
        assert!(
            folded.allowed(),
            "the scheme and the domain are case-insensitive: {folded:?}"
        );
        assert_eq!(folded.role, Some(ActorRole::Attendee));
    }

    /// A `REPLY` that also rewrites `ORGANIZER` is the same attack through the other method.
    ///
    /// It survives, and for a reason unrelated to the field rule: `describe_reply` looks at the
    /// answering `ATTENDEE` line and nothing else, so the reply's `ORGANIZER` is never diffed. The
    /// case is here so that a later change to the reply diff cannot open the hole the `COUNTER`
    /// case above already shows.
    #[test]
    fn a_reply_never_describes_the_organizer_line() {
        let answer = judge(HELD, REPLY_REWRITES_ORGANIZER, BO);
        assert!(
            !answer.touches("ORGANIZER"),
            "a REPLY described a change to the ORGANIZER line: {answer:?}"
        );
        assert_eq!(answer.reason, Some(TransitionReason::ParticipationChanged));
    }

    /// The same takeover, reached by a party who is not on the attendee list at all.
    ///
    /// `ActorRole::Delegate` is the window RFC 5546 section 2.1.2 leaves open: a party the held
    /// copy names only inside another attendee's `DELEGATED-TO` value satisfies
    /// `SenderRule::Attendee` and may therefore send a `COUNTER`. `attendee_occurrence_of` answers
    /// `None` for such a party, so it owns no `ATTENDEE` line — but `ORGANIZER` is `EitherParty`
    /// and does not consult ownership, so the takeover is open to somebody who was never invited.
    #[test]
    fn a_party_named_only_in_a_delegation_cannot_become_the_organizer() {
        let answer = judge(HELD_WITH_A_DELEGATION, COUNTER_FROM_THE_DELEGATE, EVE);
        assert!(
            !answer.allowed(),
            "a party who is only somebody else's DELEGATED-TO value rewrote the ORGANIZER line: \
             {answer:?}"
        );
    }

    /// The byte boundary, asked the two ways an attacker can ask it.
    ///
    /// An `Authorization` borrows both inputs and has no owned form, so there is nothing to
    /// serialize and nothing to forge — that is a compile-time fact and the reason no case here
    /// can even be written. What does cross bytes is a `Commitment`, and it is checked here for
    /// the two properties its own documentation claims: it grants nothing, and it notices when the
    /// thing being confirmed is not the thing that was described.
    ///
    /// The replay row is the other half: a genuine attendee `COUNTER` at `SEQUENCE:2` replayed
    /// against a state that has moved to `SEQUENCE:99` is refused as stale rather than reapplied.
    #[test]
    fn a_vetted_transition_does_not_replay_against_a_state_that_moved() {
        let stale = judge(HELD_AFTER_SEQUENCE_RAISED, COUNTER_REWRITES_ORGANIZER, BO);
        assert_eq!(
            stale.denied,
            Some(AuthorizationDenied::SequenceStale { have: 99 }),
            "an older revision was reapplied against a newer state: {stale:?}"
        );

        let held = subject(HELD);
        let moved = subject(HELD_AFTER_SEQUENCE_RAISED);
        // An attendee's answer rather than an attendee's counter: every `COUNTER` in this
        // chapter is one the field rule now refuses, and a commitment is only made about a
        // decision the gate reached.
        let calendar = subject(REPLY_MOVES_THE_MEETING);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let message = ItipMessage::read(&calendar, limits, &mut meter, &mut sink)
            .expect("the reply reads as a message");

        let first = evaluate_message(&message, held.component(), PartyId::new(BO))
            .expect("the gate authorized this reply when it was first described");
        let commitment = Commitment::of(&first);
        assert!(
            first.honors(&commitment),
            "a commitment describes the decision it was taken from"
        );

        // The confirm turn, against state that moved underneath. The gate refuses on the
        // revision, so there is nothing left for the commitment to be asked about — which is the
        // ordering its documentation claims: the gate ran fresh either way.
        let confirm = evaluate_message(&message, moved.component(), PartyId::new(BO));
        assert_eq!(
            confirm.err(),
            Some(AuthorizationDenied::SequenceStale { have: 99 }),
            "the confirm turn accepted a message against state it was never judged against"
        );
    }

    /// What the transitions above actually contain, printed so a reader can check the claim.
    ///
    /// A break reported without the message, the prior state and the returned transition beside it
    /// is a suspicion. This prints all three for every case in this file.
    #[test]
    fn the_gate_answers_are_recorded_in_full() {
        for (name, held, message, actor) in [
            (
                "counter-rewrites-organizer",
                HELD,
                COUNTER_REWRITES_ORGANIZER,
                BO,
            ),
            ("counter-raises-sequence", HELD, COUNTER_RAISES_SEQUENCE, BO),
            (
                "counter-substitutes-stranger",
                HELD,
                COUNTER_SUBSTITUTES_STRANGER,
                BO,
            ),
            (
                "counter-moves-the-meeting",
                HELD,
                COUNTER_MOVES_THE_MEETING,
                BO,
            ),
            (
                "counter-removes-the-other",
                HELD,
                COUNTER_REMOVES_THE_OTHER,
                BO,
            ),
            ("counter-adds-a-third", HELD, COUNTER_ADDS_A_THIRD, BO),
            ("cancel-from-an-attendee", HELD, CANCEL_FROM_AN_ATTENDEE, BO),
            (
                "cancel-after-takeover",
                HELD_AFTER_TAKEOVER,
                CANCEL_FROM_THE_NEW_ORGANIZER,
                BO,
            ),
            (
                "organizer-update-after-sequence-raised",
                HELD_AFTER_SEQUENCE_RAISED,
                REQUEST_FROM_THE_ORGANIZER,
                CHAIR,
            ),
            (
                "request-from-an-attendee",
                HELD,
                REQUEST_FROM_AN_ATTENDEE,
                BO,
            ),
            ("reply-moves-the-meeting", HELD, REPLY_MOVES_THE_MEETING, BO),
            ("reply-adds-a-stranger", HELD, REPLY_ADDS_A_STRANGER, BO),
            (
                "reply-rewrites-organizer",
                HELD,
                REPLY_REWRITES_ORGANIZER,
                BO,
            ),
            (
                "reply-answers-for-another",
                HELD,
                REPLY_ANSWERS_FOR_ANOTHER,
                BO,
            ),
            ("reply-from-a-stranger", HELD, REPLY_FROM_A_STRANGER, EVE),
            (
                "reply-from-the-other-attendee",
                HELD,
                REPLY_ANSWERS_FOR_ANOTHER,
                CY,
            ),
            (
                "counter-from-the-delegate",
                HELD_WITH_A_DELEGATION,
                COUNTER_FROM_THE_DELEGATE,
                EVE,
            ),
        ] {
            let answer = judge(held, message, actor);
            println!("{name}: {answer:?}");
        }
    }

    /// The vocabulary this file borrows but does not otherwise name, kept honest by one use.
    #[test]
    fn the_borrowed_vocabulary_still_spells_what_it_did() {
        assert_eq!(
            ProposedChange::Replace(RawText::from_bytes(b"X:1")),
            ProposedChange::Replace(RawText::from_bytes(b"X:1"))
        );
    }
}
