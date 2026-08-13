// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What is wrong with a scheduling message, reported rather than refused.
//!
//! Specification: RFC 5546 section 3's constraint tables and the per-method prose beside them,
//! RFC 5545 section 3.3.3 (`CAL-ADDRESS`) and section 3.8.7.4 (`SEQUENCE`).
//!
//! # The division this module exists to draw
//!
//! [`crate::internal::core::Component::audit`] reads RFC 5545 section 3.6 and answers *is this property
//! here, and how often*. It cannot answer *is what it carries usable*, because usable is a
//! question about a reading somebody has to have done: an `ORGANIZER` whose `CAL-ADDRESS` is
//! not UTF-8 is **present**, and it identifies nobody; a `SEQUENCE` of `later` is **present**,
//! and it orders no versions. `docs/adr/0009` amendment 1 names that state, and this pass is
//! where it is reported for the four properties a scheduling decision reads — `METHOD`,
//! `ORGANIZER`, `ATTENDEE` and `SEQUENCE`. The first of them belongs to
//! [`ItipMessage::read`], because a message whose `METHOD` names nothing cannot be read at
//! all; the other three are here.
//!
//! The second half of the pass is [`crate::internal::itip::table`] evaluated rather than consulted. RFC 5546
//! section 3's tables are transcribed there as data, and a transcription nothing exercises is
//! a claim rather than a fact. Every `0` row and every required row is walked here against a
//! real payload, so a reviewer checking the table against the specification is checking
//! something with an observable consequence.
//!
//! The three scheduling codes this module does **not** emit are the three that need a zone
//! answer: `scheduling-instance-ambiguous`, `scheduling-zone-continued` and
//! `scheduling-exclusion-unplaced` belong to `crate::internal::itip::instance`, which takes the series and the
//! caller's source. Nothing here resolves a zone, so nothing here may claim that a wall clock
//! repeats — a payload this module cannot tell from its neighbor might be a fold, and might
//! equally be a producer that wrote one `RECURRENCE-ID` twice.
//!
//! # It reports, and it decides nothing
//!
//! Every condition below is one [`crate::internal::itip::evaluate_message`] already refuses as an `Err`, and
//! that gate does not consult this module. Running this pass or not running it cannot change
//! an authorization answer, which is what makes it safe to run over a message a caller has
//! not decided anything about yet — and what makes it useless as a substitute for the gate.
//! A diagnostic here is something to show a person, never something to branch a decision on.
//!
//! # What it cannot say
//!
//! The gate judges an actor against the state the caller **holds**; this pass has only the
//! message, so its sender check reads the parties the message itself claims. A message that
//! names the sender as its own `ORGANIZER` while the caller's copy names somebody else is
//! silent here and denied there, and no amount of reporting closes that gap — which is the
//! point. Silence here is likewise not a claim that a party is entitled to anything: an actor
//! the message names in neither role gets no diagnostic, because the roles a message asserts
//! about itself are not evidence about who sent it.

use alloc::collections::BTreeMap;

use crate::internal::core::{
    Diagnostic, DiagnosticCode, DiagnosticSink, Location, Meter, PropertyId, Severity, Subject,
    report_diagnostic,
};

use crate::internal::itip::authorize::actor_role;
use crate::internal::itip::identity::{InstanceRef, SequenceRead};
use crate::internal::itip::message::ItipMessage;
use crate::internal::itip::method::Method;
use crate::internal::itip::party::{Party, PartyId};
use crate::internal::itip::state::ScheduledComponent;
use crate::internal::itip::table::MethodRule;

/// The methods RFC 5546 does not permit `RANGE=THISANDFUTURE` under.
///
/// Data rather than a `match`, for the reason [`crate::internal::itip::table`] is data: section 3.2.3's
/// `REPLY` table admits one `RECURRENCE-ID` referring to one instance, and a reply reaching
/// every later instance answers for meetings the sender was never asked about. Section 3.2.6's
/// `REFRESH` is the same shape. The rows are the ones [`crate::internal::itip::evaluate_message`] refuses on,
/// and the two lists must stay the same list.
static RANGE_FORBIDDEN: &[Method] = &[Method::Reply, Method::Refresh];

/// Report what is wrong with `message`, walking every payload it carries.
///
/// Six codes, every one of them a [`Severity::Violation`]:
/// `scheduling-calendar-address-unreadable`, `scheduling-sequence-unreadable`,
/// `scheduling-property-not-allowed`, `scheduling-required-property-missing`,
/// `scheduling-range-not-permitted` and `scheduling-sender-not-permitted`.
///
/// `actor` is the party whose entitlement to send this method is being asked about, and is
/// optional because most callers of a reporting pass have not got one yet: an inbox rendering
/// what arrived is inspecting the file, not judging a sender. Where it is supplied, the answer
/// is the weaker one this module's own documentation describes.
///
/// Every property and every attendee visited is charged to `meter`. A ledger that runs out
/// stops the walk where it ran out, so what was not reported after that is not a claim that
/// there was nothing to report — [`Meter::is_exhausted`] latches, and is how a caller tells
/// the two apart.
pub fn inspect_message<S: DiagnosticSink + ?Sized>(
    message: &ItipMessage<'_>,
    actor: Option<PartyId<'_>>,
    meter: &mut Meter,
    sink: &mut S,
) {
    for index in 0..message.payload_count() {
        if meter.is_exhausted() {
            return;
        }
        let Some(payload) = message.payload(index) else {
            continue;
        };
        inspect_payload(message, payload, actor, meter, sink);
    }
}

/// Report what one payload of `message` states.
fn inspect_payload<S: DiagnosticSink + ?Sized>(
    message: &ItipMessage<'_>,
    payload: &dyn ScheduledComponent,
    actor: Option<PartyId<'_>>,
    meter: &mut Meter,
    sink: &mut S,
) {
    inspect_parties(payload, meter, sink);
    inspect_sequence(payload, meter, sink);
    inspect_properties(message.rule(), payload, meter, sink);
    if meter.is_exhausted() {
        // A pass that ran out of ledger has not seen every property of this payload, and the
        // two checks below would then report about a component only partly read.
        return;
    }
    inspect_range(message.method(), payload, meter, sink);
    inspect_sender(message.rule(), payload, actor, meter, sink);
}

/// Report every `ORGANIZER` or `ATTENDEE` that is present and identifies nobody.
///
/// Only when the property is present: an absent `ORGANIZER` is RFC 5545 section 3.6's
/// question and `crate::internal::core::Component::audit` answers it. A `SENT-BY` that did not decode is
/// not reported, because [`Party`] cannot tell an absent parameter from an unreadable one and
/// a diagnostic that fires on both would name the wrong fact half the time.
fn inspect_parties<S: DiagnosticSink + ?Sized>(
    payload: &dyn ScheduledComponent,
    meter: &mut Meter,
    sink: &mut S,
) {
    if let Some(organizer) = payload.organizer() {
        inspect_address(organizer, b"ORGANIZER", meter, sink);
    }
    for index in 0..payload.attendee_count() {
        if !meter.charge(1) {
            return;
        }
        if let Some(attendee) = payload.attendee(index) {
            inspect_address(attendee.party(), b"ATTENDEE", meter, sink);
        }
    }
}

/// Report `party` when its `CAL-ADDRESS` did not decode.
fn inspect_address<S: DiagnosticSink + ?Sized>(
    party: Party<'_>,
    name: &[u8],
    meter: &mut Meter,
    sink: &mut S,
) {
    if party.is_readable() {
        return;
    }
    about(
        sink,
        meter,
        DiagnosticCode::SchedulingCalendarAddressUnreadable,
        Severity::Violation,
        name,
    );
}

/// Report a `SEQUENCE` that was present and was not an integer.
///
/// An absent one is RFC 5546 section 3.2's zero and is not reported: zero is a revision, and
/// this code is the absence of one.
fn inspect_sequence<S: DiagnosticSink + ?Sized>(
    payload: &dyn ScheduledComponent,
    meter: &mut Meter,
    sink: &mut S,
) {
    if matches!(payload.sequence(), SequenceRead::Unreadable) {
        about(
            sink,
            meter,
            DiagnosticCode::SchedulingSequenceUnreadable,
            Severity::Violation,
            b"SEQUENCE",
        );
    }
}

/// Evaluate `constraints` against the properties `payload` carries.
///
/// Counted per name and reported per name: what a `0` row forbids is the name, so a payload
/// carrying three of one forbidden property has broken one row and not three.
///
/// A required row is reported only where the payload carries **none** of it, which is
/// narrower than the row's own `admits`. A second `DTSTAMP` fails `1` and is not a payload
/// that *lacked* one, and `scheduling-required-property-missing` is the only code the closed
/// golden list has for that row — so the cardinality half stays with
/// `crate::internal::core::Component::audit`'s `duplicate-property`, which is a claim this pass can make
/// without over-claiming. [`crate::internal::itip::evaluate_message`] refuses the over-count either way.
fn inspect_properties<S: DiagnosticSink + ?Sized>(
    constraints: MethodRule,
    payload: &dyn ScheduledComponent,
    meter: &mut Meter,
    sink: &mut S,
) {
    let mut counts: BTreeMap<PropertyId, usize> = BTreeMap::new();
    for index in 0..payload.property_count() {
        if !meter.charge(1) {
            return;
        }
        let Some(name) = payload.property_name(index) else {
            continue;
        };
        let seen = counts.entry(PropertyId::from_name(name)).or_insert(0);
        *seen = seen.saturating_add(1);
    }
    for id in counts.keys() {
        if constraints.presence_of(id.as_bytes()).is_forbidden() {
            about(
                sink,
                meter,
                DiagnosticCode::SchedulingPropertyNotAllowed,
                Severity::Violation,
                id.as_bytes(),
            );
        }
    }
    for row in constraints.properties() {
        let carried = counts.keys().any(|id| row.is_named(id.as_bytes()));
        if row.presence().is_required() && !carried {
            about(
                sink,
                meter,
                DiagnosticCode::SchedulingRequiredPropertyMissing,
                Severity::Violation,
                row.name(),
            );
        }
    }
}

/// Report a `RECURRENCE-ID` reaching further than `method` may.
fn inspect_range<S: DiagnosticSink + ?Sized>(
    method: Method,
    payload: &dyn ScheduledComponent,
    meter: &mut Meter,
    sink: &mut S,
) {
    let reaching = payload
        .recurrence_id()
        .is_some_and(InstanceRef::is_this_and_future);
    if reaching && RANGE_FORBIDDEN.contains(&method) {
        about(
            sink,
            meter,
            DiagnosticCode::SchedulingRangeNotPermitted,
            Severity::Violation,
            b"RECURRENCE-ID",
        );
    }
}

/// Report an actor RFC 5546 section 3's prose does not permit to send this method.
///
/// Reported only where the payload resolves the actor into an [`crate::internal::itip::ActorRole`] that fails
/// the rule. An actor the payload names in no role at all is somebody this message says
/// nothing about, and the party who may answer that — the caller's own copy of the component
/// — is not here. The subject is the method, because what was violated is the method's row.
fn inspect_sender<S: DiagnosticSink + ?Sized>(
    constraints: MethodRule,
    payload: &dyn ScheduledComponent,
    actor: Option<PartyId<'_>>,
    meter: &mut Meter,
    sink: &mut S,
) {
    let Some(who) = actor else {
        return;
    };
    let Some(role) = actor_role(payload, who) else {
        return;
    };
    if role.satisfies(constraints.sender()) {
        return;
    }
    about(
        sink,
        meter,
        DiagnosticCode::SchedulingSenderNotPermitted,
        Severity::Violation,
        constraints.method().as_bytes(),
    );
}

/// Offer one diagnostic about the name `subject`, charging a refusal to `meter`.
///
/// Every emission site here names its subject, because a diagnostic that says a property is
/// forbidden without saying which one tells a caller that something is wrong and not what to
/// go and look at. The location is [`Location::NOWHERE`]: a payload reached through
/// [`ScheduledComponent`] owns its octets and has no span back into anybody's buffer.
fn about<S: DiagnosticSink + ?Sized>(
    sink: &mut S,
    meter: &mut Meter,
    code: DiagnosticCode,
    severity: Severity,
    subject: &[u8],
) {
    report_diagnostic(
        sink,
        meter,
        Diagnostic::new(code, severity, Location::NOWHERE).about(Subject::new(subject)),
    );
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::internal::core::{
        ComponentKind, Diagnostic, DiagnosticCode, Instant, Limits, Meter,
    };
    use crate::internal::recur::OverrideRange;

    use super::inspect_message;
    use crate::internal::itip::authorize::{AuthorizationDenied, evaluate_message};
    use crate::internal::itip::identity::{FoldSide, InstanceClock, InstanceRef, SequenceRead};
    use crate::internal::itip::message::ItipMessage;
    use crate::internal::itip::party::{Attendee, Party, PartyId};
    use crate::internal::itip::state::{PropertyOccurrence, ScheduledComponent};

    /// A `VEVENT` that satisfies RFC 5546 section 3.2.2's `REQUEST` table exactly.
    const REQUEST_LINES: &[&[u8]] = &[
        b"UID:m3-agenda@example.com",
        b"DTSTAMP:20260810T120000Z",
        b"DTSTART:20260901T090000Z",
        b"ORGANIZER:mailto:chair@example.com",
        b"ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:ann@example.com",
        b"SUMMARY:Design review",
    ];

    /// A `VEVENT` that satisfies RFC 5546 section 3.2.3's `REPLY` table exactly.
    const REPLY_LINES: &[&[u8]] = &[
        b"UID:m3-agenda@example.com",
        b"DTSTAMP:20260810T130000Z",
        b"ORGANIZER:mailto:chair@example.com",
        b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:ann@example.com",
        b"SEQUENCE:2",
    ];

    /// A `VEVENT` that satisfies RFC 5546 section 3.2.1's `PUBLISH` table exactly.
    const PUBLISH_LINES: &[&[u8]] = &[
        b"UID:m3-agenda@example.com",
        b"DTSTAMP:20260810T120000Z",
        b"DTSTART:20260901T090000Z",
        b"ORGANIZER:mailto:chair@example.com",
        b"SUMMARY:Design review",
    ];

    /// The organizer's own copy: the state a message is judged against.
    const HELD_LINES: &[&[u8]] = &[
        b"UID:m3-agenda@example.com",
        b"DTSTAMP:20260810T100000Z",
        b"DTSTART:20260901T090000Z",
        b"ORGANIZER:mailto:chair@example.com",
        b"ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:ann@example.com",
        b"SUMMARY:Design review",
        b"SEQUENCE:2",
    ];

    const CHAIR: &str = "mailto:chair@example.com";
    const ANN: &str = "mailto:ann@example.com";
    const STRANGER: &str = "mailto:zz@example.org";

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// The name a content line states, up to its first `;` or `:`.
    fn line_name(line: &[u8]) -> &[u8] {
        let end = line
            .iter()
            .position(|octet| *octet == b';' || *octet == b':')
            .unwrap_or(line.len());
        line.get(..end).unwrap_or(line)
    }

    /// The value a content line states, after its first `:`.
    fn line_value(line: &[u8]) -> &[u8] {
        line.iter()
            .position(|octet| *octet == b':')
            .and_then(|colon| line.get(colon.saturating_add(1)..))
            .unwrap_or(&[])
    }

    /// The value of the parameter `wanted` on `line`, if it carries one.
    fn line_param<'a>(line: &'a [u8], wanted: &[u8]) -> Option<&'a [u8]> {
        let colon = line.iter().position(|octet| *octet == b':')?;
        let head = line.get(..colon)?;
        head.split(|octet| *octet == b';').find_map(|part| {
            let split = part.iter().position(|octet| *octet == b'=')?;
            let key = part.get(..split)?;
            key.eq_ignore_ascii_case(wanted)
                .then(|| part.get(split.saturating_add(1)..))?
        })
    }

    /// One component built from content lines.
    ///
    /// The `ical-core` bridge is M3's other half and has not landed, so the tests hold a
    /// component of their own — which is also the second implementation of
    /// [`ScheduledComponent`] the trait was written to make possible.
    #[derive(Debug, Default)]
    struct Fake {
        kind: Option<ComponentKind>,
        method: Option<&'static [u8]>,
        lines: Vec<&'static [u8]>,
        children: Vec<Fake>,
        sequence: SequenceRead,
        dtstamp: Option<Instant>,
        instance: Option<InstanceRef>,
    }

    impl Fake {
        /// A `VEVENT` carrying `lines`, with no revision and about no instance.
        fn event(lines: &[&'static [u8]]) -> Self {
            Self {
                kind: Some(ComponentKind::Event),
                lines: lines.to_vec(),
                ..Self::default()
            }
        }

        /// The same event with the revision `sequence` and `dtstamp` state.
        fn revised(self, sequence: SequenceRead, dtstamp: Option<Instant>) -> Self {
            Self {
                sequence,
                dtstamp,
                ..self
            }
        }

        /// The same event, about `instance` rather than about the whole series.
        fn about(self, instance: InstanceRef) -> Self {
            Self {
                instance: Some(instance),
                ..self
            }
        }

        /// A `VCALENDAR` stating `METHOD:method` and carrying `payloads`.
        fn calendar(method: &'static [u8], payloads: Vec<Self>) -> Self {
            Self {
                kind: Some(ComponentKind::Calendar),
                method: Some(method),
                children: payloads,
                ..Self::default()
            }
        }

        /// The first line whose name is `name`.
        fn line_of(&self, name: &[u8]) -> Option<&'static [u8]> {
            self.lines
                .iter()
                .copied()
                .find(|line| line_name(line).eq_ignore_ascii_case(name))
        }

        /// Every `ATTENDEE` line, in document order.
        fn attendee_lines(&self) -> impl Iterator<Item = &'static [u8]> + '_ {
            self.lines
                .iter()
                .copied()
                .filter(|line| line_name(line).eq_ignore_ascii_case(b"ATTENDEE"))
        }
    }

    impl ScheduledComponent for Fake {
        fn component_kind(&self) -> Option<ComponentKind> {
            self.kind
        }

        fn method(&self) -> Option<&[u8]> {
            self.method
        }

        fn uid(&self) -> Option<&[u8]> {
            self.line_of(b"UID").map(line_value)
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
            self.line_of(b"ORGANIZER")
                .map(|line| Party::read(line_value(line), line_param(line, b"SENT-BY")))
        }

        fn attendee_count(&self) -> usize {
            self.attendee_lines().count()
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            let line = self.attendee_lines().nth(index)?;
            let party = Party::read(line_value(line), line_param(line, b"SENT-BY"));
            let attendee = Attendee::new(party);
            Some(match line_param(line, b"PARTSTAT") {
                Some(status) => attendee.with_part_stat(status),
                None => attendee,
            })
        }

        fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
            (index < self.attendee_count()).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
        }

        fn property_count(&self) -> usize {
            self.lines.len()
        }

        fn property_name(&self, index: usize) -> Option<&[u8]> {
            self.lines.get(index).copied().map(line_name)
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

    /// `base` with `extra` appended, which is how a case states one line more than a table
    /// admits.
    fn plus(base: &[&'static [u8]], extra: &'static [u8]) -> Vec<&'static [u8]> {
        let mut lines = base.to_vec();
        lines.push(extra);
        lines
    }

    /// The codes `found` carries, in the order they were reported.
    fn codes(found: &[Diagnostic]) -> Vec<DiagnosticCode> {
        found.iter().copied().map(Diagnostic::code).collect()
    }

    /// Read `calendar` as a message and report on it, answering the codes that came out.
    fn codes_of(calendar: &Fake, actor: Option<&str>) -> Vec<DiagnosticCode> {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut read: Vec<Diagnostic> = Vec::new();
        let message = ItipMessage::read(calendar, Limits::DEFAULT, &mut meter, &mut read).unwrap();
        let mut found: Vec<Diagnostic> = Vec::new();
        inspect_message(&message, actor.map(PartyId::new), &mut meter, &mut found);
        codes(&found)
    }

    /// Run each row and assert the codes it was read from the specification to produce.
    fn judge(cases: Vec<(&str, Fake, Option<&str>, Vec<DiagnosticCode>)>) {
        for (section, calendar, actor, expected) in cases {
            assert_eq!(codes_of(&calendar, actor), expected, "{section}");
        }
    }

    /// RFC 5546 section 3's presence rows: what a method's table forbids and what it requires,
    /// each row read from the specification rather than from what this pass happens to answer.
    #[test]
    fn a_payload_is_reported_against_the_table_its_method_is_stated_in() {
        judge(vec![
            (
                "3.2.2: a REQUEST from its organizer satisfies every row",
                Fake::calendar(b"REQUEST", vec![Fake::event(REQUEST_LINES)]),
                Some(CHAIR),
                vec![],
            ),
            (
                "3.2.2: REQUEST-STATUS is a 0 row for REQUEST",
                Fake::calendar(
                    b"REQUEST",
                    vec![Fake::event(&plus(
                        REQUEST_LINES,
                        b"REQUEST-STATUS:2.0;Success",
                    ))],
                ),
                Some(CHAIR),
                vec![DiagnosticCode::SchedulingPropertyNotAllowed],
            ),
            (
                "3.2.2: SUMMARY is a 1 row for REQUEST",
                Fake::calendar(
                    b"REQUEST",
                    vec![Fake::event(REQUEST_LINES.get(..5).unwrap())],
                ),
                Some(CHAIR),
                vec![DiagnosticCode::SchedulingRequiredPropertyMissing],
            ),
            (
                "3.2.1: ATTENDEE is a 0 row for PUBLISH",
                Fake::calendar(
                    b"PUBLISH",
                    vec![Fake::event(&plus(
                        PUBLISH_LINES,
                        b"ATTENDEE:mailto:ann@example.com",
                    ))],
                ),
                None,
                vec![DiagnosticCode::SchedulingPropertyNotAllowed],
            ),
        ]);
    }

    /// The rows a presence count cannot reach: a value that is present and unusable, a
    /// parameter no method admits, and a party RFC 5546 section 3's prose does not let send.
    ///
    /// Separated from the presence rows above because the division is the point — agenda item
    /// 7 asks where `Component::audit` stops, and audit answers every row of the table above
    /// and none of these.
    #[test]
    fn a_value_that_is_present_and_unusable_is_reported_where_no_count_can_see_it() {
        judge(vec![
            (
                "3.3.3 of RFC 5545: an ATTENDEE present and identifying nobody",
                Fake::calendar(
                    b"REPLY",
                    vec![Fake::event(&[
                        b"UID:m3-agenda@example.com",
                        b"DTSTAMP:20260810T130000Z",
                        b"ORGANIZER:mailto:chair@example.com",
                        b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:a\xffn@example.com",
                    ])],
                ),
                None,
                vec![DiagnosticCode::SchedulingCalendarAddressUnreadable],
            ),
            (
                "2.1.4: a SEQUENCE present and not an integer orders nothing",
                Fake::calendar(
                    b"REPLY",
                    vec![
                        Fake::event(REPLY_LINES).revised(SequenceRead::Unreadable, Some(at(2_000))),
                    ],
                ),
                None,
                vec![DiagnosticCode::SchedulingSequenceUnreadable],
            ),
            (
                "3.2.3: a REPLY may not carry RANGE=THISANDFUTURE",
                Fake::calendar(
                    b"REPLY",
                    vec![
                        Fake::event(&plus(
                            REPLY_LINES,
                            b"RECURRENCE-ID;RANGE=THISANDFUTURE:20260901T090000Z",
                        ))
                        .about(InstanceRef::new(
                            at(1_787_000_000),
                            InstanceClock::Utc,
                            OverrideRange::ThisAndFuture,
                        )),
                    ],
                ),
                None,
                vec![DiagnosticCode::SchedulingRangeNotPermitted],
            ),
            (
                "3.2.3: a REPLY is the attendee's to send, not the organizer's",
                Fake::calendar(b"REPLY", vec![Fake::event(REPLY_LINES)]),
                Some(CHAIR),
                vec![DiagnosticCode::SchedulingSenderNotPermitted],
            ),
            (
                "3.2.3: and the attendee named on it may send it",
                Fake::calendar(b"REPLY", vec![Fake::event(REPLY_LINES)]),
                Some(ANN),
                vec![],
            ),
            (
                "3.2.3: a party the message names in no role is nothing this pass claims",
                Fake::calendar(b"REPLY", vec![Fake::event(REPLY_LINES)]),
                Some(STRANGER),
                vec![],
            ),
        ]);
    }

    /// The reporting pass and the gate answer different questions, and running the first one
    /// cannot move the second. The five shapes below are the ones RFC 5546 sections 2.1.4,
    /// 2.1.5 and 3.7.1 turn on, and none of them has a diagnostic code at all: a stale
    /// `SEQUENCE` is a refusal and not a defect in the file.
    #[test]
    fn a_report_never_moves_an_authorization_answer() {
        let held = Fake::event(HELD_LINES).revised(SequenceRead::Value(2), Some(at(1_000)));
        let missing = InstanceRef::new(at(500), InstanceClock::Utc, OverrideRange::ThisOnly)
            .with_side(FoldSide::Once);
        let reply = |sequence: u32, stamp: i64| {
            Fake::calendar(
                b"REPLY",
                vec![
                    Fake::event(REPLY_LINES)
                        .revised(SequenceRead::Value(sequence), Some(at(stamp))),
                ],
            )
        };
        let cases: Vec<(&str, Fake, &str, Option<AuthorizationDenied>)> = vec![
            (
                "2.1.4: the answer to the invitation held",
                reply(2, 2_000),
                ANN,
                None,
            ),
            (
                "1.3: a reply from an address nobody invited",
                reply(2, 2_000),
                STRANGER,
                Some(AuthorizationDenied::UnknownAttendee),
            ),
            (
                "2.1.4: an older SEQUENCE never overwrites a newer state",
                reply(1, 9_999),
                ANN,
                Some(AuthorizationDenied::SequenceStale { have: 2 }),
            ),
            (
                "2.1.5: an equal SEQUENCE with an older DTSTAMP does not either",
                reply(2, 999),
                ANN,
                Some(AuthorizationDenied::DtstampStale { have: at(1_000) }),
            ),
            (
                "3.7.1: a RECURRENCE-ID naming an instance the series does not have",
                Fake::calendar(
                    b"REPLY",
                    vec![
                        Fake::event(&plus(REPLY_LINES, b"RECURRENCE-ID:19700101T000820Z"))
                            .revised(SequenceRead::Value(2), Some(at(2_000)))
                            .about(missing),
                    ],
                ),
                ANN,
                Some(AuthorizationDenied::NoMatchingInstance),
            ),
        ];

        for (section, calendar, actor, denial) in cases {
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut read: Vec<Diagnostic> = Vec::new();
            let message =
                ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut read).unwrap();
            let who = PartyId::new(actor);

            let before = evaluate_message(&message, &held, who);
            let mut found: Vec<Diagnostic> = Vec::new();
            inspect_message(&message, Some(who), &mut meter, &mut found);
            let after = evaluate_message(&message, &held, who);

            assert_eq!(before.err(), denial, "{section}");
            assert_eq!(after.err(), denial, "{section}");
            assert!(
                codes(&found).is_empty(),
                "{section}: a conforming file with a refused message reports nothing"
            );
        }
    }

    /// A message whose payloads are all conforming still has to be paid for, and a ledger that
    /// runs out stops the walk rather than reporting a partial reading as a whole one.
    #[test]
    fn every_property_visited_is_charged_and_an_empty_ledger_stops_the_walk() {
        let calendar = Fake::calendar(
            b"REQUEST",
            vec![Fake::event(REQUEST_LINES.get(..5).unwrap())],
        );
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut read: Vec<Diagnostic> = Vec::new();
        let message = ItipMessage::read(&calendar, Limits::DEFAULT, &mut meter, &mut read).unwrap();

        let before = meter.spent();
        let mut found: Vec<Diagnostic> = Vec::new();
        inspect_message(&message, None, &mut meter, &mut found);
        assert!(
            meter.spent() >= before.saturating_add(5),
            "five properties and one attendee are six units of work"
        );
        assert_eq!(
            codes(&found),
            vec![DiagnosticCode::SchedulingRequiredPropertyMissing]
        );
        assert!(!meter.is_exhausted());

        let mut tight = Meter::with_budget(Limits::DEFAULT, 2);
        let mut cut: Vec<Diagnostic> = Vec::new();
        inspect_message(&message, None, &mut tight, &mut cut);
        assert!(tight.is_exhausted(), "the refusal latches on the ledger");
        assert!(
            codes(&cut).is_empty(),
            "silence after exhaustion is not a claim that nothing was wrong"
        );
    }
}
