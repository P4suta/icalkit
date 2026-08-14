// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The scheduling chapter of the corpus: RFC 5546 and RFC 6047, case by case.
//!
//! Every case here is addressed to a section of RFC 5546 and its expectation is read from that
//! section's own text — its constraint tables, its per-method prose, and sections 2.1.4 and
//! 2.1.5 for which of two versions wins. None of them is read off an answer this workspace
//! gave. Where the answer this workspace gives differs from the answer the specification
//! states, the case says so in its own note rather than being quietly relaxed to match, which
//! is what makes a corpus evidence instead of a regression suite.
//!
//! # The subject, and why it is written by hand
//!
//! `ical-itip` judges a message against *the state the caller holds*, and the trait it takes
//! that state through — `ScheduledComponent` — exists so a server whose calendar is a database
//! row never has to build an `icalkit_conformance::internal::core::Component` in order to answer "who may change this".
//! The design document was candid that until a second implementation appears, the trait "earns
//! its cost as insurance rather than as demonstrated demand". The subject below is that second
//! implementation: it reads a committed `.ics` fixture into its own small tree and answers the
//! seventeen questions directly, the same way `break_tz_seam` writes a zone source by hand
//! because ADR-0003 says a zone answer is the caller's to supply. Nothing in the crate under
//! test is bypassed by that — the messages, the identities, the revisions and the gate are all
//! its own.
//!
//! # The policy every case runs under
//!
//! `Limits::DEFAULT`, with a `Meter` budgeted at that policy's own input bound, unless the case
//! states otherwise in its table — the bounded cases narrow exactly one bound each and say
//! which, because "the message was refused" is not reproducible against an unstated policy
//! (ADR-0010). The bounded cases are also the ones that make this crate's deliberate departure
//! from ADR-0009 visible: a scheduling message that crosses a bound is an `Err` refusing the
//! whole message, never a truncation plus a diagnostic, because dropping the last attendee of
//! an over-long list turns "this party may reply" into "this party is unknown" and lets whoever
//! padded the list choose which of the two a server believes.
//!
//! # Synthetic interoperability hypotheses
//!
//! The three shapes below came from implementation folklore, not from reduced captures committed
//! with producer/version/date provenance. They are useful robustness cases and are deliberately
//! labeled synthetic; they do not establish what Google Calendar, Microsoft 365, or Apple
//! Calendar emits or accepts, and cannot justify a `CommonClientsV1` repair.
//!
//! - **An `ATTENDEE` in a `PUBLISH`.** RFC 5546 section 3.2.1's table says `0`. The synthetic
//!   `publish_with_attendee.ics` fixture pins the strict answer: refuse the whole message.
//! - **A `VALARM` in a `REPLY`.** Section 3.2.3's table says `0` for the subcomponent, and the
//!   reason is not aesthetic: an alarm is a component the recipient's client will act on, and
//!   an attendee's reply is not a place to install one. The synthetic fixture is refused.
//! - **A `REPLY` with no `SEQUENCE`.** RFC 5546 section 3.2 reads an absent `SEQUENCE` as zero,
//!   which is a revision and not an unknown. Reading absent as "unknown" and accepting it is how
//!   a stale reply overwrites a newer state, so this project reads zero and the case pins it.
//!
//! # The two-turn exchange
//!
//! ADR-0005 requires the corpus to carry the propose-then-confirm exchange as a fixture, and
//! both of its shapes are here. The right one carries the *message* across the boundary and
//! calls `evaluate_message` again against state it read fresh; when the organizer moved the
//! meeting in between, the second call refuses the message as stale and nothing is applied. The
//! wrong one — recorded, not merely described — replays the snapshot the first turn was judged
//! against, gets a genuine authorization over state that no longer exists, and writes over the
//! organizer's newer version. No lifetime can see that, which is exactly what the ADR says: the
//! flow is not safe against a racing organizer update and must not be described as if it were.
//! `Commitment` narrows the window and is compared only to refuse; the case asserts that the
//! confirm turn re-evaluates either way, so that a forged one buys an attacker nothing but the
//! ability to decline to be told that the target moved.
//!
//! # Three cases this chapter found, and the gate now answers
//!
//! The three below were written the specification's way against a gate that answered otherwise,
//! on the principle that a corpus edited to agree with the implementation it measures has
//! stopped measuring anything. All three failed on the first green build and all three were
//! closed in the implementation rather than here; ADR-0005 amendments 4 to 6 record what
//! changed. They are kept named because a case whose provenance is a defect is worth more than
//! one nobody can date.
//!
//! - `reply-carrying-an-alarm`. The gate counted a payload's *properties* against section 3's
//!   table and never its components, so the `VALARM` row that reads `0` went unenforced and an
//!   attendee's answer could install a component the recipient's client will act on. It is now
//!   `AuthorizationDenied::MethodForbidsComponent`, a refusal of its own because a nested
//!   component is not a property occurrence.
//! - `request-creating-an-event-the-caller-does-not-hold`. The sending party was resolved
//!   against the state the caller holds, and for a `PUBLISH` or a `REQUEST` about something it
//!   does not hold yet that state names nobody — so the two methods whose whole purpose is to
//!   arrive first were refused, and `TransitionReason::Created` was unreachable. The lookup now
//!   falls back to the payload when the prior state is absent, and `SECURITY.md` states what
//!   rests on the transport as a result.
//! - `refresh-asking-for-the-latest-version`. A `REFRESH` was diffed like a restatement of the
//!   component, so it described the removal of every property it does not echo — and the field
//!   rule then refused the attendee for removals the diff invented. It describes nothing now,
//!   and the revision gate is skipped for any method whose own table forbids a `SEQUENCE`.
//!
//! # Current evidence boundary
//!
//! iMIP is now an unconditional facade workflow rather than a feature on this former kernel.
//! `crates/icalkit/tests/scheduling_workflow.rs` drives RFC 6047 media-type, charset, method,
//! aggregate-budget, and authenticated-actor boundaries through the public API. In particular,
//! email `From` is never substituted for the `ORGANIZER` or `ATTENDEE` identity the calendar
//! names.
//!
//! The `scheduling-*` diagnostics are exercised on their emitting paths across this chapter,
//! the composition/attacker suites, and the facade workflow tests. The table below additionally
//! freezes each spelling, severity, and channel; it is a registry assertion rather than a list
//! of unfinished emitters.

#[cfg(test)]
mod tests {
    use icalkit_conformance::internal::core::{
        CivilDate, CivilDateTime, CivilTime, ComponentKind, Diagnostic, DiagnosticCode, Instant,
        Limits, Location, Meter, PropertyId, ProposedChange, Severity, UtcOffset,
    };
    use icalkit_conformance::internal::itip::{
        ActorRole, ApplyReport, Attendee, AuthorizationDenied, Commitment, FoldSide, InstanceClock,
        InstanceRef, ItipMessage, MessageError, Party, PartyId, PriorState, PropertyOccurrence,
        Revision, ScheduleTarget, ScheduledComponent, SequenceRead, TransitionReason,
        WriteRejected, apply_transition, describe_message, evaluate_message,
    };
    use icalkit_conformance::internal::recur::OverrideRange;
    use icalkit_conformance::internal::tz::{LocalResolution, Reading, nominal};

    // The state a caller already holds. A held fixture carries no `METHOD`: it is a calendar,
    // not a message, and the component judged against is its first child.

    /// A weekly series the caller holds at `SEQUENCE:2`, with two attendees.
    const HELD_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/held_series.ics");
    /// The same series after the organizer moved it again, at `SEQUENCE:4`.
    const HELD_MOVED: &[u8] = include_bytes!("../tests/fixtures/itip/held_moved.ics");
    /// The same series at the same revision with one property edited underneath.
    const HELD_SUMMARY_EDITED: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_summary_edited.ics");
    /// A posted event with no attendee list, which is what a `PUBLISH` may act on.
    const HELD_PUBLISHED: &[u8] = include_bytes!("../tests/fixtures/itip/held_published.ics");
    /// The caller holds nothing under this identity: a shell with no `UID`.
    const HELD_NOTHING: &[u8] = include_bytes!("../tests/fixtures/itip/held_nothing.ics");
    /// One override of the series, addressed by a `RECURRENCE-ID` reaching one instance.
    const HELD_INSTANCE: &[u8] = include_bytes!("../tests/fixtures/itip/held_instance.ics");
    /// The same override, stored with `RANGE=THISANDFUTURE`.
    const HELD_INSTANCE_ONWARDS: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_instance_this_and_future.ics");
    /// An override landing in the hour `America/New_York` repeats in 2026.
    const HELD_FOLDED_INSTANCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_folded_instance.ics");
    /// The series after one attendee delegated, so the delegate is only a `DELEGATED-TO` value.
    const HELD_AFTER_DELEGATION: &[u8] =
        include_bytes!("../tests/fixtures/itip/held_after_delegation.ics");

    // The messages.

    /// A `PUBLISH` restating the posted event, section 3.2.1.
    const PUBLISH_UPDATE: &[u8] = include_bytes!("../tests/fixtures/itip/publish_update.ics");
    /// The same `PUBLISH` carrying an `ATTENDEE`, which section 3.2.1's table gives `0`.
    const PUBLISH_WITH_ATTENDEE: &[u8] =
        include_bytes!("../tests/fixtures/itip/publish_with_attendee.ics");
    /// A `REQUEST` at `SEQUENCE:3` moving the series, section 3.2.2.1.
    const REQUEST_RESCHEDULES: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_reschedules.ics");
    /// The same `REQUEST` at `SEQUENCE:1`, older than what the caller holds.
    const REQUEST_OLDER_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_older_sequence.ics");
    /// The same `REQUEST` at the held `SEQUENCE` with an earlier `DTSTAMP`.
    const REQUEST_OLDER_DTSTAMP: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_same_sequence_older_dtstamp.ics");
    /// The same `REQUEST` with no `SEQUENCE` at all, which section 3.2 reads as zero.
    const REQUEST_WITHOUT_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/request_without_sequence.ics");
    /// A `CANCEL` of the whole series, section 3.2.5.
    const CANCEL_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/cancel_series.ics");
    /// A `REFRESH` asking the organizer to resend, section 3.2.6.
    const REFRESH_SERIES: &[u8] = include_bytes!("../tests/fixtures/itip/refresh_series.ics");
    /// The same `REFRESH` carrying a `SEQUENCE`, which section 3.2.6's table gives `0`.
    const REFRESH_WITH_SEQUENCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/refresh_with_sequence.ics");
    /// A `COUNTER` proposing a different `DTSTART`, section 3.2.7.
    const COUNTER_NEW_TIME: &[u8] =
        include_bytes!("../tests/fixtures/itip/counter_proposes_a_new_time.ics");
    /// A `REPLY` accepting, from the second attendee on the caller's list.
    const REPLY_ACCEPTED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_accepted.ics");
    /// A `REPLY` declining, from the first.
    const REPLY_DECLINED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_declined.ics");
    /// A `REPLY` delegating, which writes two parameters on one line (section 2.1.2).
    const REPLY_DELEGATED: &[u8] = include_bytes!("../tests/fixtures/itip/reply_delegated.ics");
    /// A `REPLY` from the delegate, who is on the list only as a `DELEGATED-TO` value.
    const REPLY_FROM_DELEGATE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_from_the_delegate.ics");
    /// A `REPLY` carrying a `VALARM`, which section 3.2.3's table gives `0`.
    const REPLY_WITH_ALARM: &[u8] = include_bytes!("../tests/fixtures/itip/reply_with_alarm.ics");
    /// A `REPLY` with no `DTSTAMP`, which section 3.2.3's table requires exactly once.
    const REPLY_WITHOUT_DTSTAMP: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_without_dtstamp.ics");
    /// A `REPLY` restating a `DTSTART` the request never carried.
    const REPLY_WITH_A_NEW_DTSTART: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_restates_a_new_dtstart.ics");
    /// A `REPLY` whose `UID` differs from the held one only by case.
    const REPLY_UID_CASE_SHIFTED: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_uid_case_shifted.ics");
    /// A `REPLY` whose `UID` differs from the held one only by a leading space.
    const REPLY_UID_SPACED: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_uid_with_a_leading_space.ics");
    /// A `REPLY` naming an instance the caller's copy does not have.
    const REPLY_ABSENT_INSTANCE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_names_an_absent_instance.ics");
    /// A `REPLY` whose `RECURRENCE-ID` reaches every later instance.
    const REPLY_THIS_AND_FUTURE: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_this_and_future.ics");
    /// A `REPLY` naming the earlier half of a repeated wall clock.
    const REPLY_FOLDED_HOUR: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_to_the_folded_hour.ics");
    /// A `REPLY` naming the later half of the same wall clock.
    const REPLY_OTHER_HALF: &[u8] =
        include_bytes!("../tests/fixtures/itip/reply_to_the_other_half.ics");
    /// A calendar with no `METHOD`, which is an ordinary calendar and not a message.
    const WITHOUT_METHOD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_without_method.ics");
    /// A `METHOD` naming nothing RFC 5546 defines.
    const UNKNOWN_METHOD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_with_an_unknown_method.ics");
    /// Two payloads under two identities, which section 3.1.1 forbids.
    const TWO_UIDS: &[u8] = include_bytes!("../tests/fixtures/itip/message_with_two_uids.ics");
    /// A `VALARM` at the top level of a message, where no table admits one.
    const ALARM_PAYLOAD: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_with_an_alarm_payload.ics");
    /// A `REPLY` addressed to a `VJOURNAL`, a pair RFC 5546 states no table for.
    const REPLY_TO_A_JOURNAL: &[u8] =
        include_bytes!("../tests/fixtures/itip/message_reply_to_a_journal.ics");

    /// The organizer of every fixture above.
    const CHAIR: &str = "mailto:chair@example.com";
    /// The assistant the organizer's `SENT-BY` names.
    const ASSISTANT: &str = "mailto:pa@example.com";
    /// The attendee the caller's list carries second.
    const BO: &str = "mailto:bo@example.com";
    /// The attendee it carries first.
    const CY: &str = "mailto:cy@example.com";
    /// A party on neither the organizer line nor the attendee list.
    const STRANGER: &str = "mailto:zz@example.com";

    /// The hour `America/New_York` repeats on the first Sunday of November 2026.
    ///
    /// Transcribed from the rule the zone publishes — `-04:00` daylight moving to `-05:00`
    /// standard at 02:00 local on 2026-11-01 — and not read off an answer this workspace gave.
    /// Local 01:30 that morning happens twice: at 05:30Z under the offset before the
    /// transition and at 06:30Z under the offset after it.
    static NEW_YORK_FALL_BACK: Fold = Fold {
        earlier: b"20261101T053000Z",
        later: b"20261101T063000Z",
        before: -14_400,
        after: -18_000,
    };

    /// One repeated wall clock, as a caller's own zone database states it.
    ///
    /// `ical-itip` resolves no zone: a fold side is derived from the `LocalResolution` a caller
    /// already holds, and a caller with none gets `FoldSide::Unresolved` and the conservative
    /// answer that follows. This is the smallest thing that can play the caller's part.
    #[derive(Debug)]
    struct Fold {
        /// The first of the two instants, under the offset in force before the transition.
        earlier: &'static [u8],
        /// The second, under the offset in force after it.
        later: &'static [u8],
        /// Seconds east of UTC before the transition.
        before: i32,
        /// Seconds east of UTC from it.
        after: i32,
    }

    impl Fold {
        /// Which half of the repeated hour `named` is, or `Once` for every other hour.
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

    /// One content line of a fixture, unfolded and taken apart.
    ///
    /// `content` is the whole line as the file has it, because that is the unit
    /// `ProposedChange::Replace` takes and the unit an octet diff compares. `value` and the
    /// parameters are *values*: the quotes are gone and RFC 6868's caret encoding is resolved,
    /// which is the contract `ical-itip` states from the other end.
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
        /// The line `content` spells, or `None` when it carries no `:` at all.
        ///
        /// The `:` is found outside quoted parameter values, because
        /// `ORGANIZER;SENT-BY="mailto:pa@example.com":mailto:chair@example.com` carries three
        /// of them and only the last one ends the header. The `;` between parameters is split
        /// plainly: no fixture in this chapter puts one inside a quoted value, and a reader
        /// that says so is more honest than one that pretends to a generality it never runs.
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
    ///
    /// The second implementation of `ScheduledComponent` this workspace has, and the reason the
    /// trait exists rather than a concrete type: nothing here is an `icalkit_conformance::internal::core::Component`.
    #[derive(Clone, Debug, Default)]
    struct Subject {
        /// What the `BEGIN` line named, `None` for a name RFC 5545 does not define.
        kind: Option<ComponentKind>,
        /// The properties directly inside this component, in document order.
        properties: Vec<Line>,
        /// The components directly inside it, in document order.
        children: Vec<Subject>,
        /// The repeated hour this case supplied, if it supplied one.
        zone: Option<&'static Fold>,
    }

    impl Subject {
        /// The component the caller holds: the calendar's first child.
        fn component(&self) -> &Self {
            self.children.first().unwrap_or(self)
        }

        /// The same subject, and every component under it, resolving instances against `zone`.
        fn with_zone(mut self, zone: Option<&'static Fold>) -> Self {
            self.zone = zone;
            self.children = self
                .children
                .into_iter()
                .map(|child| child.with_zone(zone))
                .collect();
            self
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

    /// Where an authorized transition is written, for the cases that write one.
    #[derive(Debug, Default)]
    struct Recorder {
        /// What the target took, in the order it was offered.
        written: Vec<(PropertyOccurrence, ProposedChange)>,
        /// A property name this target will not have written at all.
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

    /// The value RFC 6868 spells `raw`, with a quoted parameter's quotes removed.
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
            // A `^` with nothing after it escapes nothing and stands for itself. Answered
            // before the match rather than inside it, because RFC 6868's `^^` and a trailing
            // caret write the same octet for opposite reasons and one arm would hide that.
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

    /// The calendar `source` spells, as a subject resolving instances against `zone`.
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
    ///
    /// A `Z`-terminated value names a real instant and the projection is the identity. A value
    /// written without one names a wall clock, and the nominal timeline is the only thing this
    /// corpus can place it on without a zone — which is exactly the seam M2 landed.
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

    /// What RFC 5546 says must happen to one case.
    #[derive(Debug)]
    enum Outcome {
        /// The gate authorizes it, describing a change of this kind.
        ///
        /// The count is `Some` only where the specification states the extent of the change as
        /// well as its kind — a `REPLY` answers for one attendee and states nothing else — and
        /// `None` where it states the kind and leaves the extent to the diff.
        Allowed(TransitionReason, Option<usize>),
        /// The gate refuses it, naming this reason.
        Refused(AuthorizationDenied),
    }

    /// One conformance case: what the caller holds, what arrives, and who applies it.
    #[derive(Debug)]
    struct Case {
        /// The case identity, never reused for a different question.
        id: &'static str,
        /// The RFC 5546 subsection whose constraint table governs the message.
        section: &'static str,
        /// The calendar the caller already holds.
        held: &'static [u8],
        /// The message that arrives.
        message: &'static [u8],
        /// The party applying it.
        actor: &'static str,
        /// Whether the case supplies the zone that tells a repeated hour's two halves apart.
        zoned: bool,
        /// What RFC 5546 states must happen.
        outcome: Outcome,
    }

    /// Judge every case, saying which specification section the expectation came from.
    ///
    /// Every case here runs under `Limits::DEFAULT` with a ledger budgeted at that policy's own
    /// input bound; the cases that narrow a bound have their own table and state which.
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

    /// The occurrence a `REPLY` writes to, in the recipient's own numbering.
    fn attendee_at(index: usize) -> PropertyOccurrence {
        PropertyOccurrence::named(b"ATTENDEE", index)
    }

    /// RFC 5546 section 3's constraint tables, in the rows that say `0` and the rows that
    /// require a property, exercised through whole messages.
    ///
    /// This is the evidence the transcribed table is a table and not a switch: each case names
    /// the subsection it is addressed to, and `judge` holds that name against the section the
    /// implementation's own row carries, so a case and its table cannot drift apart silently.
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
                // Section 3.2.1's table gives `ATTENDEE` the value `0`. This synthetic
                // interoperability shape is therefore refused whole.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(attendee_at(0))),
            },
            Case {
                id: "reply-carrying-an-alarm",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_WITH_ALARM,
                actor: BO,
                zoned: false,
                // Section 3.2.3's SUBCOMPONENTS table gives `VALARM` the value `0`: a reply is
                // not a place to install a component the recipient's client will act on.
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
                // Section 3.2.3's table gives `DTSTAMP` the value `1`.
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
                // Section 3.2.6's table gives `SEQUENCE` the value `0`: a refresh asks for the
                // latest version and states no version of its own.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(
                    PropertyOccurrence::named(b"SEQUENCE", 0),
                )),
            },
        ]);
    }

    /// RFC 5546 section 3's prose about who may send a method, and what a caller must already
    /// hold for one to mean anything.
    ///
    /// No constraint table states either, which is why these rows are the likeliest place a
    /// transcription is wrong and the reason each is a case rather than a comment.
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
                // Section 3.2.2: a `REQUEST` is the organizer's. An attendee sending one moves
                // somebody else's meeting, which is the first attack `SECURITY.md` names.
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
                // Section 3.2.3: a reply comes from an attendee. An address the component names
                // nowhere is a refusal, never a silently added participant.
                outcome: Outcome::Refused(AuthorizationDenied::UnknownAttendee),
            },
            Case {
                id: "cancel-of-something-nobody-holds",
                section: "3.2.5",
                held: HELD_NOTHING,
                message: CANCEL_SERIES,
                actor: CHAIR,
                zoned: false,
                // Section 3.2.5 cancels an existing component. There is nothing to cancel.
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
                // Section 3.2.7 lets an attendee counter with a different time, and this
                // project's field rule does not: `DTSTART` is the organizer's under every
                // method. Recorded rather than relaxed — the design document names this exact
                // refusal as the interoperability cost it prefers to a permissive default.
                outcome: Outcome::Refused(AuthorizationDenied::MethodForbidsField(
                    PropertyOccurrence::named(b"DTSTART", 0),
                )),
            },
        ]);
    }

    /// RFC 5546 sections 2.1.4 and 2.1.5: `SEQUENCE` orders versions and `DTSTAMP` breaks ties.
    ///
    /// The whole of this protocol's replay defense, and it is weak — nothing signs either
    /// number — so every row here is stated in the refusing direction.
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
                // Section 3.2 reads an absent `SEQUENCE` as zero, which is a revision and not
                // an unknown. Reading it as unknown is how a stale message wins.
                outcome: Outcome::Refused(AuthorizationDenied::SequenceStale { have: 2 }),
            },
            Case {
                id: "request-at-a-newer-sequence",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_RESCHEDULES,
                actor: CHAIR,
                zoned: false,
                // Section 3.2.2.1: an update that moves the time is a reschedule.
                outcome: Outcome::Allowed(TransitionReason::Rescheduled, None),
            },
            Case {
                id: "reply-at-the-revision-it-answers",
                section: "3.2.3",
                held: HELD_SERIES,
                message: REPLY_ACCEPTED,
                actor: BO,
                zoned: false,
                // Two equal revisions supersede each other in neither direction, which is
                // exactly the shape of a reply: it answers the invitation it was sent.
                outcome: Outcome::Allowed(TransitionReason::ParticipationChanged, Some(1)),
            },
            Case {
                id: "request-from-the-organizers-assistant",
                section: "3.2.2",
                held: HELD_SERIES,
                message: REQUEST_RESCHEDULES,
                actor: ASSISTANT,
                zoned: false,
                // RFC 5545 section 3.2.18: `SENT-BY` names an agent, and an agent satisfies its
                // principal's rule. "The assistant sent this" never becomes "the organizer is".
                outcome: Outcome::Allowed(TransitionReason::Rescheduled, None),
            },
        ]);
    }

    /// RFC 5546 section 3.2.3: a `REPLY` states one attendee's participation and nothing else.
    ///
    /// The occurrence written is the *recipient's* numbering, matched by `CAL-ADDRESS`, so the
    /// sender's position in its own list buys it nothing: `bo` is second on the caller's list
    /// and first in every reply below.
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

    /// RFC 5546 section 2.1.2: a delegation writes `PARTSTAT` and `DELEGATED-TO` on one line.
    ///
    /// The reason the change vocabulary needs a parameter *list*: a single-parameter edit type
    /// cannot express a legal reply. The delegate's own reply is the recorded limitation on the
    /// other side of it — until the delegator's reply has been applied, the delegate is on the
    /// caller's list only as somebody else's `DELEGATED-TO` value, and a reply from them
    /// describes nothing rather than inventing the participant the gate exists to refuse.
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

    /// Identity is `UID` plus `RECURRENCE-ID`, and both halves are attacked here.
    ///
    /// RFC 5545 section 3.8.4.7 gives a `UID` no case folding and no whitespace stripping, so
    /// two identifiers differing by either are two identifiers — and the direction that merges
    /// them is how a `CANCEL` for one meeting cancels another. Section 3.2.13's `RANGE` is part
    /// of what a reference claims. The two halves of a repeated wall clock are one cadence key
    /// and two meetings, and an ambiguous match is not a match.
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
                // A message about one instance is not a message about the series.
                outcome: Outcome::Refused(AuthorizationDenied::NoMatchingInstance),
            },
            Case {
                id: "reply-reaching-further-than-the-override-it-names",
                section: "3.2.3",
                held: HELD_INSTANCE,
                message: REPLY_THIS_AND_FUTURE,
                actor: BO,
                zoned: true,
                // Same instant, different `RANGE`: a different claim, and so a different thing.
                outcome: Outcome::Refused(AuthorizationDenied::NoMatchingInstance),
            },
            Case {
                id: "reply-carrying-range-this-and-future",
                section: "3.2.3",
                held: HELD_INSTANCE_ONWARDS,
                message: REPLY_THIS_AND_FUTURE,
                actor: BO,
                zoned: true,
                // Section 3.2.3 answers one instance. A reply that reaches every later one is
                // an attendee answering for meetings nobody has invited them to yet.
                outcome: Outcome::Refused(AuthorizationDenied::RangeNotPermitted),
            },
            Case {
                id: "reply-to-a-repeated-hour-with-no-zone-supplied",
                section: "3.2.3",
                held: HELD_FOLDED_INSTANCE,
                message: REPLY_FOLDED_HOUR,
                actor: BO,
                zoned: false,
                // One cadence key and two real meetings. Nothing resolved which, and a guess
                // cancels somebody else's.
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

    /// What each method describes once it is authorized, in RFC 5546's own terms.
    ///
    /// A caller renders a prompt from the kind of change, not from the method, because an
    /// update that moved the time and one that fixed a typo are two prompts and one `REQUEST`.
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
                // Section 3.2.2's first sentence: a `REQUEST` invites attendees to an event
                // they do not have, and section 3.2.1 posts one to a calendar that does not
                // have it either. Both are the whole point of those two methods, and both name
                // their parties in the message, because the state the caller holds names none.
                outcome: Outcome::Allowed(TransitionReason::Created, None),
            },
            Case {
                id: "cancel-of-a-series-the-caller-holds",
                section: "3.2.5",
                held: HELD_SERIES,
                message: CANCEL_SERIES,
                actor: CHAIR,
                zoned: false,
                // Section 3.2.5 states the kind and not the extent. What this project describes
                // is the octet diff, which for a cancellation states a removal for every
                // property the message does not restate, plus the explicit cancelled state:
                // eight occurrences here.
                outcome: Outcome::Allowed(TransitionReason::Cancelled, Some(8)),
            },
            Case {
                id: "refresh-asking-for-the-latest-version",
                section: "3.2.6",
                held: HELD_SERIES,
                message: REFRESH_SERIES,
                actor: BO,
                zoned: false,
                // Section 3.2.6: a refresh asks the organizer to resend and changes nothing in
                // the recipient's copy. Describing it as a diff would have an attendee's
                // request for information state the removal of the organizer's own properties.
                outcome: Outcome::Allowed(TransitionReason::RefreshRequested, Some(0)),
            },
        ]);
    }

    /// ADR-0005's two-turn exchange, in the shape that is right and the shape that is wrong.
    ///
    /// There is no session (ADR-0004), so a propose-then-confirm exchange crosses a request
    /// boundary. What crosses it is the *message*: `Authorization` borrows both of its inputs
    /// and has no owned form, so a caller that tries to carry one across gets a compile error
    /// rather than a forgeable token. The confirm turn therefore reads state again and
    /// re-evaluates — and this case records what happens when it does not.
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

        // The confirm turn. The organizer moved the meeting again in between, and the message
        // the user approved is now the older version.
        let fresh = subject(HELD_MOVED, None);
        let again = ItipMessage::read(&proposal, limits, &mut meter, &mut sink).unwrap();
        match evaluate_message(&again, fresh.component(), PartyId::new(CHAIR)) {
            Err(AuthorizationDenied::SequenceStale { have }) => assert_eq!(have, 4),
            answer => panic!("re-reading must refuse the older version: {answer:?}"),
        }

        // The wrong shape, recorded rather than described: the confirm turn replays the
        // snapshot it was judged against. The gate answers honestly about a state that no
        // longer exists, and the organizer's newer version is written over. No lifetime can
        // see this, which is why ADR-0005 calls it a caller obligation and not a guarantee.
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

    /// `Commitment` crosses bytes, carries no authority, and is compared only to refuse.
    ///
    /// The confirm turn re-evaluates whether or not one is presented, so forging one buys an
    /// attacker exactly one thing: the ability to decline to be told that the target moved.
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

        // Somebody edited the caller's copy without changing its revision, so nothing the
        // replay defense can see has moved. What has moved is the description the user
        // approved, and that is the whole of what a commitment is for.
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

    /// A stream of components that is not a scheduling message, refused whole.
    ///
    /// Each row states the policy it ran under. The bounded rows are where this crate departs
    /// from ADR-0009 on purpose: a limit breach is an `Err` and never a truncation, because a
    /// shortened attendee list is a different authorization answer rather than a degraded one.
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

    /// A case per diagnostic code and per channel, as ADR-0009 requires.
    ///
    /// The spelling is the frozen part: a code's meaning may not be edited without a rename, so
    /// a case that asserts the string is the thing an assertion can outlive. The channel column
    /// says where each one travels, and the note says which of them has an emitter today —
    /// `evaluate_message` takes no sink, so the conditions it refuses on travel as errors until
    /// the surfaces that will report them exist.
    #[test]
    fn every_scheduling_diagnostic_code_has_a_case_and_a_channel() {
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

        // The one code with an emitter today, asserted on the channel it actually travels.
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

    /// Applying an authorized transition, and the report that says what a target took.
    ///
    /// A partial application is reported and never hidden: this crate owns no transaction and
    /// cannot roll one back, so a caller needing all-or-nothing asks before committing.
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
            // By value: a vetted transition is a single-use capability rather than something
            // replayable against a second target once the state it was vetted against moved.
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

    /// A denied message stays inspectable without becoming applicable.
    ///
    /// ADR-0005 asks that a rejected reply's attempted changes stay showable. `describe_message`
    /// is that path, and what it hands back is inert: applying anything needs an
    /// `Authorization`, and no route leads from a `Transition` to one.
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
