// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iTIP scheduling tests for RFC 5546 event methods.
//!
//! Synthetic messages exercise method constraints and icalkit policy.
//! The public scheduling workflow tests cover iMIP transport under RFC 6047.

#[cfg(test)]
mod tests {
    use icalkit_conformance::internal::core::{
        CivilDate, CivilDateTime, CivilTime, ComponentKind, Diagnostic, DiagnosticCode, Instant,
        Limits, Location, Meter, PropertyId, ProposedChange, RawText, Severity, UtcOffset,
    };
    use icalkit_conformance::internal::itip::{
        ActorRole, ApplyReport, Attendee, AuthorizationDenied, Commitment, FoldSide, InstanceClock,
        InstanceRef, ItipMessage, MessageError, Party, PartyId, PriorState, PropertyOccurrence,
        Revision, ScheduleTarget, ScheduledComponent, SequenceRead, TransitionReason,
        WriteRejected, apply_transition, describe_message, evaluate_message,
    };
    use icalkit_conformance::internal::recur::OverrideRange;
    use icalkit_conformance::internal::tz::{LocalResolution, Reading, nominal};

    // Held calendars provide the state before a scheduling message arrives.

    const HELD_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/held_series.ics");
    const HELD_MOVED: &[u8] = include_bytes!("../tests/fixtures/itip/held_moved.ics");
    const HELD_SUMMARY_EDITED: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_summary_edited.ics");
    const HELD_PUBLISHED: &[u8] = include_bytes!("../tests/fixtures/itip/held_published.ics");
    /// A component without a `UID` represents a missing event in the fixture.
    const HELD_NOTHING: &[u8] = include_bytes!("../tests/fixtures/itip/held_nothing.ics");
    const HELD_INSTANCE: &[u8] = include_bytes!("../tests/fixtures/itip/held_instance.ics");
    const HELD_INSTANCE_ONWARDS: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_instance_this_and_future.ics");
    const HELD_FOLDED_INSTANCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_folded_instance.ics");
    const HELD_AFTER_DELEGATION: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_after_delegation.ics");

    // RFC 5546 section 3.2.1 forbids ATTENDEE in a PUBLISH message for a VEVENT.

    const PUBLISH_UPDATE: &[u8] = include_bytes!("../tests/fixtures/itip/publish_update.ics");
    const PUBLISH_WITH_ATTENDEE: &[u8] =
        include_bytes!("../tests/fixtures/itip/publish_with_attendee.ics");
    const REQUEST_RESCHEDULES: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_reschedules.ics");
    const REQUEST_OLDER_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_older_sequence.ics");
    const REQUEST_OLDER_DTSTAMP: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_same_sequence_older_dtstamp.ics");
    const REQUEST_WITHOUT_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_without_sequence.ics");
    const CANCEL_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/cancel_series.ics");
    const CANCEL_WITHOUT_ATTENDEES: &[u8] =
        include_bytes!("../tests/fixtures/itip/cancel_series_without_attendees.ics");
    const REFRESH_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/refresh_series.ics");
    const REFRESH_WITH_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/refresh_with_sequence.ics");
    const COUNTER_NEW_TIME: &[u8] =
        include_bytes!("../tests/fixtures/itip/counter_proposes_a_new_time.ics");
    const REPLY_ACCEPTED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_accepted.ics");
    const REPLY_DECLINED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_declined.ics");
    const REPLY_DELEGATED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_delegated.ics");
    const REPLY_FROM_DELEGATE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_from_the_delegate.ics");
    const REPLY_WITH_ALARM: &[u8] = include_bytes!("../tests/fixtures/itip/reply_with_alarm.ics");
    const REPLY_WITHOUT_DTSTAMP: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_without_dtstamp.ics");
    const REPLY_WITH_A_NEW_DTSTART: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_restates_a_new_dtstart.ics");
    const REPLY_UID_CASE_SHIFTED: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_uid_case_shifted.ics");
    const REPLY_UID_SPACED: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_uid_with_a_leading_space.ics");
    const REPLY_ABSENT_INSTANCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_names_an_absent_instance.ics");
    const REPLY_THIS_AND_FUTURE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_this_and_future.ics");
    const REPLY_FOLDED_HOUR: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_to_the_folded_hour.ics");
    const REPLY_OTHER_HALF: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_to_the_other_half.ics");
    const WITHOUT_METHOD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_without_method.ics");
    const UNKNOWN_METHOD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_with_an_unknown_method.ics");
    const TWO_UIDS: &[u8] = include_bytes!("../tests/fixtures/itip/message_with_two_uids.ics");
    const ALARM_PAYLOAD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_with_an_alarm_payload.ics");
    const REPLY_TO_A_JOURNAL: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_reply_to_a_journal.ics");

    const CHAIR: &str = "mailto:chair@example.com";
    const ASSISTANT: &str = "mailto:pa@example.com";
    const BO: &str = "mailto:bo@example.com";
    const CY: &str = "mailto:cy@example.com";
    const STRANGER: &str = "mailto:zz@example.com";

    /// Both fixture instants represent 01:30 local time across the November transition.
    static NEW_YORK_FALL_BACK: Fold = Fold {
        earlier: b"20261101T053000Z",
        later: b"20261101T063000Z",
        before: -14_400,
        after: -18_000,
    };

    #[derive(Debug)]
    struct Fold {
        earlier: &'static [u8],
        later: &'static [u8],
        before: i32,
        after: i32,
    }

    impl Fold {
        fn side_of(&self, named: Instant) -> FoldSide {
            let (Some(first), Some(second)) = (instant_of(self.earlier), instant_of(self.later))
            else {
                return FoldSide::Unresolved;
            };
            let (Some(daylight), Some(standard)) = (
                UtcOffset::from_seconds(self.before),
                UtcOffset::from_seconds(self.after),
            ) else {
                return FoldSide::Unresolved;
            };
            let resolution = if named == first || named == second {
                LocalResolution::Ambiguous {
                    earlier: Reading::new(first, daylight, true),
                    later: Reading::new(second, standard, false),
                }
            } else {
                LocalResolution::Unique {
                    reading: Reading::new(named, standard, false),
                }
            };
            FoldSide::from_resolution(resolution, Some(named))
        }
    }

    /// The fixture keeps the unfolded line for replacements and the decoded parameters for identity checks.
    #[derive(Clone, Debug, Default)]
    struct Line {
        name: Vec<u8>,
        parameters: Vec<(Vec<u8>, Vec<u8>)>,
        value: Vec<u8>,
        content: Vec<u8>,
    }

    impl Line {
        /// The fixture grammar permits colons in quoted parameters and uses semicolons only as parameter separators.
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
                    Some((key.to_ascii_uppercase(), parameter_value(raw)))
                })
                .collect();
            Some(Self {
                name,
                parameters,
                value: tail.get(1..).unwrap_or_default().to_vec(),
                content: content.to_vec(),
            })
        }

        fn is_named(&self, name: &[u8]) -> bool {
            self.name.as_slice() == name
        }

        fn parameter(&self, name: &[u8]) -> Option<&[u8]> {
            self.parameters
                .iter()
                .find(|(key, _)| key.as_slice() == name)
                .map(|(_, value)| value.as_slice())
        }
    }

    /// The fixture implements the caller's calendar storage interface.
    #[derive(Clone, Debug, Default)]
    struct Subject {
        kind: Option<ComponentKind>,
        properties: Vec<Line>,
        children: Vec<Subject>,
        zone: Option<&'static Fold>,
    }

    impl Subject {
        fn component(&self) -> &Self {
            self.children.first().unwrap_or(self)
        }

        fn with_zone(mut self, zone: Option<&'static Fold>) -> Self {
            self.zone = zone;
            self.children = self
                .children
                .into_iter()
                .map(|child| child.with_zone(zone))
                .collect();
            self
        }

        fn line(&self, name: &[u8]) -> Option<&Line> {
            self.properties.iter().find(|line| line.is_named(name))
        }

        fn value(&self, name: &[u8]) -> Option<&[u8]> {
            self.line(name).map(|line| line.value.as_slice())
        }

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
            let onwards = line
                .parameter(b"RANGE")
                .is_some_and(|range| range.eq_ignore_ascii_case(b"THISANDFUTURE"));
            let range = if onwards {
                OverrideRange::ThisAndFuture
            } else {
                OverrideRange::ThisOnly
            };
            let reference = InstanceRef::new(named, clock, range);
            Some(match self.zone {
                Some(zone) => reference.with_side(zone.side_of(named)),
                None => reference,
            })
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

    #[derive(Debug, Default)]
    struct Recorder {
        written: Vec<(PropertyOccurrence, ProposedChange)>,
        refuses: Option<&'static [u8]>,
    }

    impl ScheduleTarget for Recorder {
        fn write_change(
            &mut self,
            at: &PropertyOccurrence,
            change: &ProposedChange,
        ) -> Result<(), WriteRejected> {
            if self.refuses == Some(at.name()) {
                return Err(WriteRejected::ReadOnly);
            }
            self.written.push((at.clone(), change.clone()));
            Ok(())
        }
    }

    fn parameter_value(raw: &[u8]) -> Vec<u8> {
        let bare = raw
            .strip_prefix(b"\"")
            .and_then(|rest| rest.strip_suffix(b"\""))
            .unwrap_or(raw);
        let mut value = Vec::with_capacity(bare.len());
        let mut source = bare.iter().copied();
        while let Some(byte) = source.next() {
            if byte != b'^' {
                value.push(byte);
                continue;
            }
            let Some(escaped) = source.next() else {
                value.push(b'^');
                break;
            };
            match escaped {
                b'n' => value.push(b'\n'),
                b'\'' => value.push(b'"'),
                b'^' => value.push(b'^'),
                other => value.extend_from_slice(&[b'^', other]),
            }
        }
        value
    }

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

    fn subject(source: &[u8], zone: Option<&'static Fold>) -> Subject {
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
        done.unwrap_or_default().with_zone(zone)
    }

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

    /// Fixture comparisons project civil times onto the nominal timeline.
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

    #[derive(Debug)]
    enum Outcome {
        /// The optional count checks the number of proposed changes when the case fixes their extent.
        Allowed(TransitionReason, Option<usize>),
        Refused(AuthorizationDenied),
    }

    #[derive(Debug)]
    struct Case {
        id: &'static str,
        section: &'static str,
        held: &'static [u8],
        message: &'static [u8],
        actor: &'static str,
        zoned: bool,
        outcome: Outcome,
    }

    fn judge(table: &[Case]) {
        for case in table {
            let limits = Limits::DEFAULT;
            let mut meter = Meter::new(limits);
            let mut sink: Vec<Diagnostic> = Vec::new();
            let zone = case.zoned.then_some(&NEW_YORK_FALL_BACK);
            let held = subject(case.held, zone);
            let calendar = subject(case.message, zone);
            let message = match ItipMessage::read(&calendar, limits, &mut meter, &mut sink) {
                Ok(message) => message,
                Err(error) => panic!("{}: the message did not read at all: {error:?}", case.id),
            };
            assert_eq!(
                message.rule().section(),
                case.section,
                "{}: addressed to a different table than the method selects",
                case.id
            );
            let answer = evaluate_message(&message, held.component(), PartyId::new(case.actor));
            match (answer, &case.outcome) {
                (Ok(authorized), Outcome::Allowed(reason, changes)) => {
                    assert_eq!(authorized.reason(), *reason, "{}", case.id);
                    if let Some(count) = *changes {
                        assert_eq!(authorized.transition().len(), count, "{}", case.id);
                    }
                },
                (Err(denied), Outcome::Refused(expected)) => {
                    assert_eq!(&denied, expected, "{}", case.id);
                },
                (answer, expected) => panic!(
                    "{} (RFC 5546 section {}): the specification states {expected:?}, this \
                     answered {answer:?}",
                    case.id, case.section
                ),
            }
        }
    }

    fn attendee_at(index: usize) -> PropertyOccurrence {
        PropertyOccurrence::named(b"ATTENDEE", index)
    }

    #[test]
    fn the_section_3_property_tables_refuse_what_they_say_they_refuse() {
        judge(&[
            Case {
                id: "publish-carrying-an-attendee",
                section: "3.2.1",
                held: HELD_PUBLISHED,
                message: PUBLISH_WITH_ATTENDEE,
                actor: CHAIR,
                zoned: false,
                // RFC 5546 section 3.2.1 forbids `ATTENDEE` in an event publication.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(attendee_at(0))),
            },
            Case {
                id: "reply-carrying-an-alarm",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_WITH_ALARM,
                actor: BO,
                zoned: false,
                // RFC 5546 section 3.2.3 forbids `VALARM` in an event reply.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsComponent(
                    ComponentKind::Alarm,
                )),
            },
            Case {
                id: "reply-without-a-dtstamp",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_WITHOUT_DTSTAMP,
                actor: BO,
                zoned: false,
                // RFC 5546 section 3.2.3 requires one `DTSTAMP`.
                outcome: Outcome::Refused(AuthorizationDenied::MethodRequiresField(
                    PropertyId::DTSTAMP,
                )),
            },
            Case {
                id: "refresh-carrying-a-sequence",
                section: "3.2.6",
                held: HELD_SERIES,
                message: REFRESH_WITH_SEQUENCE,
                actor: BO,
                zoned: false,
                // RFC 5546 section 3.2.6 forbids `SEQUENCE` in an event refresh request.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(
                    PropertyOccurrence::named(b"SEQUENCE", 0),
                )),
            },
        ]);
    }

    #[test]
    fn a_method_is_refused_when_the_wrong_party_sends_it() {
        judge(&[
            Case {
                id: "request-from-an-attendee",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_RESCHEDULES,
                actor: BO,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsSender(
                    ActorRole::Attendee,
                )),
            },
            Case {
                id: "reply-from-a-party-nobody-invited",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_FROM_DELEGATE,
                actor: STRANGER,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::UnknownAttendee),
            },
            Case {
                id: "cancel-of-something-nobody-holds",
                section: "3.2.5",
                held: HELD_NOTHING,
                message: CANCEL_SERIES,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::PriorStateForbidden(
                    PriorState::Absent,
                )),
            },
            Case {
                id: "counter-proposing-a-new-time",
                section: "3.2.7",
                held: HELD_SERIES,
                message: COUNTER_NEW_TIME,
                actor: BO,
                zoned: false,
                // RFC 5546 section 3.2.7 permits a counterproposal with a different start time.
                // The icalkit authorization policy reserves `DTSTART` changes for the organizer.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(
                    PropertyOccurrence::named(b"DTSTART", 0),
                )),
            },
        ]);
    }

    /// RFC 5546 section 2.1.5 orders revisions by `SEQUENCE` and uses `DTSTAMP` to break ties.
    #[test]
    fn an_older_revision_never_overwrites_a_newer_one() {
        judge(&[
            Case {
                id: "request-at-an-older-sequence",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_OLDER_SEQUENCE,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::SequenceStale { have: 2 }),
            },
            Case {
                id: "request-at-the-same-sequence-with-an-older-dtstamp",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_OLDER_DTSTAMP,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::DtstampStale {
                    have: instant_of(b"20260301T120000Z").unwrap(),
                }),
            },
            Case {
                id: "request-with-no-sequence-at-all",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_WITHOUT_SEQUENCE,
                actor: CHAIR,
                zoned: false,
                // An omitted `SEQUENCE` defaults to zero under RFC 5546 section 3.2.
                outcome: Outcome::Refused(AuthorizationDenied::SequenceStale { have: 2 }),
            },
            Case {
                id: "request-at-a-newer-sequence",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_RESCHEDULES,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Allowed(TransitionReason::Rescheduled, None),
            },
            Case {
                id: "reply-at-the-revision-it-answers",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_ACCEPTED,
                actor: BO,
                zoned: false,
                outcome: Outcome::Allowed(TransitionReason::ParticipationChanged, Some(1)),
            },
            Case {
                id: "request-from-the-organizers-assistant",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_RESCHEDULES,
                actor: ASSISTANT,
                zoned: false,
                // RFC 5545 section 3.2.18 identifies an agent acting for the organizer through `SENT-BY`.
                outcome: Outcome::Allowed(TransitionReason::Rescheduled, None),
            },
        ]);
    }

    /// The attendee address identifies the property occurrence in the recipient's calendar.
    #[test]
    fn a_reply_answers_for_one_attendee_and_moves_nothing() {
        let limits = Limits::DEFAULT;
        for (fixture, actor, index, status) in [
            (REPLY_ACCEPTED, BO, 1, &b"ACCEPTED"[..]),
            (REPLY_DECLINED, CY, 0, b"DECLINED"),
            (REPLY_WITH_A_NEW_DTSTART, BO, 1, b"ACCEPTED"),
        ] {
            let mut meter = Meter::new(limits);
            let mut sink: Vec<Diagnostic> = Vec::new();
            let held = subject(HELD_SERIES, None);
            let calendar = subject(fixture, None);
            let message = ItipMessage::read(&calendar, limits, &mut meter, &mut sink).unwrap();
            let authorized =
                evaluate_message(&message, held.component(), PartyId::new(actor)).unwrap();
            let transition = authorized.transition();
            assert_eq!(transition.len(), 1, "a reply changes one line");
            assert_eq!(
                transition.change(&PropertyOccurrence::named(b"DTSTART", 0)),
                None,
                "a reply that restates a time still describes no change to it"
            );
            let Some(ProposedChange::SetParameters(edits)) = transition.change(&attendee_at(index))
            else {
                panic!("a reply is a parameter edit on the recipient's own attendee line");
            };
            assert!(
                edits
                    .iter()
                    .any(|edit| edit.name() == b"PARTSTAT".as_slice()
                        && edit.value() == Some(status)),
                "the answer itself"
            );
        }
    }

    /// RFC 5546 section 2.1.2 records delegation on the delegator's attendee property.
    /// The fixture lists the delegate only in `DELEGATED-TO`.
    #[test]
    fn a_delegation_writes_two_parameters_and_the_delegate_describes_nothing_yet() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let held = subject(HELD_SERIES, None);
        let calendar = subject(REPLY_DELEGATED, None);
        let message = ItipMessage::read(&calendar, limits, &mut meter, &mut sink).unwrap();
        let authorized = evaluate_message(&message, held.component(), PartyId::new(BO)).unwrap();
        let Some(ProposedChange::SetParameters(edits)) =
            authorized.transition().change(&attendee_at(1))
        else {
            panic!("a delegation is a parameter edit on the delegator's own line");
        };
        assert!(
            edits
                .iter()
                .any(|edit| edit.name() == b"PARTSTAT".as_slice()
                    && edit.value() == Some(&b"DELEGATED"[..]))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit.name() == b"DELEGATED-TO".as_slice()
                    && edit.value() == Some(STRANGER.as_bytes())),
            "the value, not the spelling: the quotes are the writer's to add"
        );

        judge(&[Case {
            id: "reply-from-the-delegate-before-the-delegation-is-applied",
            section: "3.2.3",
            held: HELD_AFTER_DELEGATION,
            message: REPLY_FROM_DELEGATE,
            actor: STRANGER,
            zoned: false,
            outcome: Outcome::Allowed(TransitionReason::ParticipationChanged, Some(0)),
        }]);
    }

    /// RFC 5546 section 2.1.5 identifies an instance by `UID` and `RECURRENCE-ID`.
    #[test]
    fn a_message_is_about_one_identity_and_a_guess_is_not_a_match() {
        judge(&[
            Case {
                id: "reply-whose-uid-differs-only-by-case",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_UID_CASE_SHIFTED,
                actor: BO,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::UidMismatch),
            },
            Case {
                id: "reply-whose-uid-differs-only-by-a-space",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_UID_SPACED,
                actor: BO,
                zoned: false,
                outcome: Outcome::Refused(AuthorizationDenied::UidMismatch),
            },
            Case {
                id: "reply-naming-an-instance-the-series-does-not-have",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_ABSENT_INSTANCE,
                actor: BO,
                zoned: true,
                outcome: Outcome::Refused(AuthorizationDenied::NoMatchingInstance),
            },
            Case {
                id: "reply-reaching-further-than-the-override-it-names",
                section: "3.2.3",
                held: HELD_INSTANCE,
                message: REPLY_THIS_AND_FUTURE,
                actor: BO,
                zoned: true,
                outcome: Outcome::Refused(AuthorizationDenied::NoMatchingInstance),
            },
            Case {
                id: "reply-carrying-range-this-and-future",
                section: "3.2.3",
                held: HELD_INSTANCE_ONWARDS,
                message: REPLY_THIS_AND_FUTURE,
                actor: BO,
                zoned: true,
                outcome: Outcome::Refused(AuthorizationDenied::RangeNotPermitted),
            },
            Case {
                id: "reply-to-a-repeated-hour-with-no-zone-supplied",
                section: "3.2.3",
                held: HELD_FOLDED_INSTANCE,
                message: REPLY_FOLDED_HOUR,
                actor: BO,
                zoned: false,
                // Both instants share a local time until the caller supplies a zone resolution.
                outcome: Outcome::Refused(AuthorizationDenied::AmbiguousInstance),
            },
            Case {
                id: "reply-to-a-repeated-hour-with-the-zone-supplied",
                section: "3.2.3",
                held: HELD_FOLDED_INSTANCE,
                message: REPLY_FOLDED_HOUR,
                actor: BO,
                zoned: true,
                outcome: Outcome::Allowed(TransitionReason::ParticipationChanged, Some(1)),
            },
            Case {
                id: "reply-to-the-other-half-of-the-repeated-hour",
                section: "3.2.3",
                held: HELD_FOLDED_INSTANCE,
                message: REPLY_OTHER_HALF,
                actor: BO,
                zoned: true,
                outcome: Outcome::Refused(AuthorizationDenied::NoMatchingInstance),
            },
        ]);
    }

    #[test]
    fn each_method_describes_the_kind_of_change_its_section_names() {
        judge(&[
            Case {
                id: "publish-restating-a-posted-event",
                section: "3.2.1",
                held: HELD_PUBLISHED,
                message: PUBLISH_UPDATE,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Allowed(TransitionReason::Published, Some(3)),
            },
            Case {
                id: "request-creating-an-event-the-caller-does-not-hold",
                section: "3.2.2",
                held: HELD_NOTHING,
                message: REQUEST_RESCHEDULES,
                actor: CHAIR,
                zoned: false,
                // For an initial invitation, the incoming message supplies the organizer identity.
                outcome: Outcome::Allowed(TransitionReason::Created, None),
            },
            Case {
                id: "cancel-of-a-series-the-caller-holds",
                section: "3.2.5",
                held: HELD_SERIES,
                message: CANCEL_SERIES,
                actor: CHAIR,
                zoned: false,
                outcome: Outcome::Allowed(TransitionReason::Cancelled, Some(3)),
            },
            Case {
                id: "refresh-asking-for-the-latest-version",
                section: "3.2.6",
                held: HELD_SERIES,
                message: REFRESH_SERIES,
                actor: BO,
                zoned: false,
                // RFC 5546 section 3.2.6 asks the organizer to resend the current event.
                outcome: Outcome::Allowed(TransitionReason::RefreshRequested, Some(0)),
            },
        ]);
    }

    #[test]
    fn whole_series_cancellation_names_the_affected_attendees() {
        let calendar = subject(CANCEL_SERIES, None);
        let mut recipients: Vec<_> = calendar
            .component()
            .attendee_lines()
            .map(|line| line.value.as_slice())
            .collect();
        recipients.sort_unstable();
        assert_eq!(recipients, [BO.as_bytes(), CY.as_bytes()]);
        assert_cancellation_preserves_held_details(&calendar);
    }

    /// Compatibility behavior: accept a whole-event cancellation without the attendees required by RFC 5546 section 3.2.5.
    #[test]
    fn sparse_cancellation_changes_status_and_revision_and_preserves_omitted_details() {
        let calendar = subject(CANCEL_WITHOUT_ATTENDEES, None);
        assert_eq!(calendar.component().attendee_count(), 0);
        assert_cancellation_preserves_held_details(&calendar);
    }

    fn assert_cancellation_preserves_held_details(calendar: &Subject) {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let held = subject(HELD_SERIES, None);
        let message = ItipMessage::read(calendar, limits, &mut meter, &mut Vec::new()).unwrap();
        let authorized = evaluate_message(&message, held.component(), PartyId::new(CHAIR)).unwrap();
        assert_eq!(authorized.reason(), TransitionReason::Cancelled);
        let transition = authorized.transition();
        assert_eq!(transition.len(), 3);
        for (name, expected) in [
            (
                b"STATUS".as_slice(),
                ProposedChange::Add(RawText::from_bytes(b"STATUS:CANCELLED")),
            ),
            (
                b"SEQUENCE".as_slice(),
                ProposedChange::Replace(RawText::from_bytes(b"SEQUENCE:3")),
            ),
            (
                b"DTSTAMP".as_slice(),
                ProposedChange::Replace(RawText::from_bytes(b"DTSTAMP:20260302T080000Z")),
            ),
        ] {
            assert_eq!(
                transition.change(&PropertyOccurrence::named(name, 0)),
                Some(&expected)
            );
        }
        for name in [
            b"UID".as_slice(),
            b"DTSTART",
            b"SUMMARY",
            b"ORGANIZER",
            b"RRULE",
        ] {
            assert_eq!(
                transition.change(&PropertyOccurrence::named(name, 0)),
                None,
                "{name:?}"
            );
        }
        for index in 0..2 {
            assert_eq!(transition.change(&attendee_at(index)), None);
        }
    }

    /// ADR 0005 requires the caller to read current calendar state before confirming a proposal.
    #[test]
    fn the_confirm_turn_re_reads_and_the_recorded_wrong_shape_replays_a_snapshot() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let snapshot = subject(HELD_SERIES, None);
        let proposal = subject(REQUEST_RESCHEDULES, None);
        let first = ItipMessage::read(&proposal, limits, &mut meter, &mut sink).unwrap();
        let described =
            evaluate_message(&first, snapshot.component(), PartyId::new(CHAIR)).unwrap();
        assert_eq!(described.reason(), TransitionReason::Rescheduled);
        let commitment = Commitment::of(&described);
        assert!(described.honors(&commitment));

        let fresh = subject(HELD_MOVED, None);
        let again = ItipMessage::read(&proposal, limits, &mut meter, &mut sink).unwrap();
        match evaluate_message(&again, fresh.component(), PartyId::new(CHAIR)) {
            Err(AuthorizationDenied::SequenceStale { have }) => assert_eq!(have, 4),
            answer => panic!("re-reading must refuse the older version: {answer:?}"),
        }

        // Reusing the proposal's snapshot can overwrite an intervening organizer update.
        let replayed = evaluate_message(&again, snapshot.component(), PartyId::new(CHAIR)).unwrap();
        assert!(
            replayed.honors(&commitment),
            "the snapshot still agrees with itself"
        );
        let mut target = Recorder::default();
        let report = apply_transition(&mut target, replayed);
        assert!(report.is_complete());
        assert!(
            target
                .written
                .iter()
                .any(|(at, _)| at.name() == b"DTSTART".as_slice()),
            "the replayed turn overwrites the time the organizer had already moved"
        );
    }

    /// A commitment detects changes to the proposal and grants no authority to apply it.
    #[test]
    fn a_commitment_notices_that_the_target_moved_and_grants_nothing() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let snapshot = subject(HELD_SERIES, None);
        let proposal = subject(REQUEST_RESCHEDULES, None);
        let message = ItipMessage::read(&proposal, limits, &mut meter, &mut sink).unwrap();
        let described =
            evaluate_message(&message, snapshot.component(), PartyId::new(CHAIR)).unwrap();
        let commitment = Commitment::of(&described);
        assert_eq!(commitment.changes(), 3);
        assert_eq!(commitment.reason(), TransitionReason::Rescheduled);
        assert_eq!(commitment.offered().sequence(), 3);
        assert_eq!(commitment.held().map(Revision::sequence), Some(2));

        let edited = subject(HELD_SUMMARY_EDITED, None);
        let confirmed =
            evaluate_message(&message, edited.component(), PartyId::new(CHAIR)).unwrap();
        assert_eq!(confirmed.transition().len(), 4);
        assert!(
            !confirmed.honors(&commitment),
            "the thing being confirmed is no longer the thing that was described"
        );
        assert_ne!(Commitment::of(&confirmed).digest(), commitment.digest());
    }

    /// A scheduling limit rejects the whole message because truncating attendees can change authorization.
    #[test]
    fn what_is_not_a_message_is_refused_rather_than_partly_read() {
        for (id, fixture, limits, budget, expected) in [
            (
                "no-method-at-all",
                WITHOUT_METHOD,
                Limits::DEFAULT,
                None,
                MessageError::MissingMethod,
            ),
            (
                "a-method-rfc-5546-does-not-define",
                UNKNOWN_METHOD,
                Limits::DEFAULT,
                None,
                MessageError::UnknownMethod,
            ),
            (
                "two-payloads-under-two-identities",
                TWO_UIDS,
                Limits::DEFAULT,
                None,
                MessageError::MixedUids,
            ),
            (
                "an-alarm-at-the-top-level",
                ALARM_PAYLOAD,
                Limits::DEFAULT,
                None,
                MessageError::UnsupportedPayload(ComponentKind::Alarm),
            ),
            (
                "a-reply-to-a-journal",
                REPLY_TO_A_JOURNAL,
                Limits::DEFAULT,
                None,
                MessageError::UndefinedForComponent(ComponentKind::Journal),
            ),
            (
                "an-attendee-list-past-the-stated-policy",
                REQUEST_RESCHEDULES,
                Limits::DEFAULT.with_max_attendees(1),
                None,
                MessageError::TooManyAttendees,
            ),
            (
                "more-payload-components-than-the-stated-policy",
                TWO_UIDS,
                Limits::DEFAULT.with_max_payload_components(1),
                None,
                MessageError::TooManyComponents,
            ),
            (
                "a-shared-ledger-with-nothing-left",
                REQUEST_RESCHEDULES,
                Limits::DEFAULT,
                Some(0),
                MessageError::BudgetExhausted,
            ),
        ] {
            let mut meter = budget.map_or_else(
                || Meter::new(limits),
                |ceiling| Meter::with_budget(limits, ceiling),
            );
            let mut sink: Vec<Diagnostic> = Vec::new();
            let calendar = subject(fixture, None);
            let answer = ItipMessage::read(&calendar, limits, &mut meter, &mut sink);
            assert_eq!(answer.err(), Some(expected), "{id}");
        }
    }

    /// The registry test checks diagnostic spellings and constructs the expected severities.
    /// The unknown-method case observes the diagnostic from the reader.
    #[test]
    fn registry_codes_and_unknown_method_emission_are_checked_separately() {
        for (code, spelling, severity, carried_by) in [
            (
                DiagnosticCode::SchedulingMethodUnknown,
                "scheduling-method-unknown",
                Severity::Violation,
                "a-method-rfc-5546-does-not-define",
            ),
            (
                DiagnosticCode::SchedulingCalendarAddressUnreadable,
                "scheduling-calendar-address-unreadable",
                Severity::Violation,
                "reply-from-a-party-nobody-invited",
            ),
            (
                DiagnosticCode::SchedulingSequenceUnreadable,
                "scheduling-sequence-unreadable",
                Severity::Violation,
                "request-with-no-sequence-at-all",
            ),
            (
                DiagnosticCode::SchedulingPropertyNotAllowed,
                "scheduling-property-not-allowed",
                Severity::Violation,
                "publish-carrying-an-attendee",
            ),
            (
                DiagnosticCode::SchedulingRequiredPropertyMissing,
                "scheduling-required-property-missing",
                Severity::Violation,
                "reply-without-a-dtstamp",
            ),
            (
                DiagnosticCode::SchedulingCancellationStatusInvalid,
                "scheduling-cancellation-status-invalid",
                Severity::Violation,
                "cancel-with-a-non-cancelled-status",
            ),
            (
                DiagnosticCode::SchedulingInstanceAmbiguous,
                "scheduling-instance-ambiguous",
                Severity::Violation,
                "reply-to-a-repeated-hour-with-no-zone-supplied",
            ),
            (
                DiagnosticCode::SchedulingRangeNotPermitted,
                "scheduling-range-not-permitted",
                Severity::Violation,
                "reply-carrying-range-this-and-future",
            ),
            (
                DiagnosticCode::SchedulingExclusionUnplaced,
                "scheduling-exclusion-unplaced",
                Severity::Violation,
                "emitted while placing exclusions in the recurrence composition path",
            ),
            (
                DiagnosticCode::SchedulingZoneContinued,
                "scheduling-zone-continued",
                Severity::Note,
                "emitted when instance placement continues a finite zone answer",
            ),
            (
                DiagnosticCode::SchedulingSenderNotPermitted,
                "scheduling-sender-not-permitted",
                Severity::Violation,
                "request-from-an-attendee",
            ),
        ] {
            assert_eq!(code.as_str(), spelling, "{carried_by}");
            let reported = Diagnostic::new(code, severity, Location::NOWHERE);
            assert_eq!(reported.severity(), severity, "{spelling}");
        }

        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let calendar = subject(UNKNOWN_METHOD, None);
        let answer = ItipMessage::read(&calendar, limits, &mut meter, &mut sink);
        assert_eq!(answer.err(), Some(MessageError::UnknownMethod));
        assert_eq!(sink.len(), 1);
        assert_eq!(sink[0].code(), DiagnosticCode::SchedulingMethodUnknown);
        assert_eq!(sink[0].severity(), Severity::Violation);
        assert_eq!(meter.diagnostics_dropped(), 0);
    }

    /// The target can reject individual changes after it accepts earlier writes.
    #[test]
    fn an_authorized_transition_is_written_once_and_reports_what_it_took() {
        let limits = Limits::DEFAULT;
        for (refuses, applied, complete) in [(None, 1, true), (Some(&b"ATTENDEE"[..]), 0, false)] {
            let mut meter = Meter::new(limits);
            let mut sink: Vec<Diagnostic> = Vec::new();
            let held = subject(HELD_SERIES, None);
            let calendar = subject(REPLY_ACCEPTED, None);
            let message = ItipMessage::read(&calendar, limits, &mut meter, &mut sink).unwrap();
            let authorized =
                evaluate_message(&message, held.component(), PartyId::new(BO)).unwrap();
            let mut target = Recorder {
                written: Vec::new(),
                refuses,
            };
            let report = apply_transition(&mut target, authorized);
            assert_eq!(report.applied(), applied);
            assert_eq!(report.is_complete(), complete);
            assert_eq!(target.written.len(), usize::try_from(applied).unwrap());
            if !complete {
                assert_eq!(report.rejected()[0].reason(), WriteRejected::ReadOnly);
                assert_eq!(report.rejected()[0].at(), &attendee_at(1));
            }
        }
        assert!(ApplyReport::new().is_complete());
    }

    /// ADR 0005 permits callers to inspect proposed changes after authorization fails.
    #[test]
    fn a_refused_message_can_still_be_shown_to_the_person_it_was_refused_for() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let held = subject(HELD_PUBLISHED, None);
        let calendar = subject(PUBLISH_WITH_ATTENDEE, None);
        let message = ItipMessage::read(&calendar, limits, &mut meter, &mut sink).unwrap();
        let denial = evaluate_message(&message, held.component(), PartyId::new(CHAIR)).unwrap_err();
        assert_eq!(
            denial,
            AuthorizationDenied::MethodForbidsField(attendee_at(0))
        );
        let attempted = describe_message(&message, held.component());
        assert_eq!(attempted.reason(), TransitionReason::Published);
        assert!(
            attempted.change(&attendee_at(0)).is_some(),
            "what the refused message tried to do is what a person is shown"
        );
    }
}
