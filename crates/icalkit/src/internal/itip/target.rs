// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writing an authorized transition into an [`ical_core::Component`].
//!
//! Specification: none of RFC 5546. This is ADR-0005's application half, and every refusal
//! reported here comes from ADR-0001's mutation boundary rather than from a scheduling rule.
//!
//! # This is not a second gate
//!
//! [`crate::internal::itip::evaluate_message`] has already decided whether the change may be made, and decided
//! it whole: there is no partial success there, because applying a message's permitted half
//! would leave the caller holding a component no party ever described. A target that refused on
//! *scheduling* grounds would be a second and weaker authorization check running after the
//! first one passed, so nothing here reads a `METHOD`, an `ORGANIZER` or an `ATTENDEE` address.
//! What is refused here is refused for a reason the model has — there is no such occurrence,
//! the octets are not storable as one content line, the line names a component boundary — and a
//! refusal is a *report* rather than a denial, because this crate owns no transaction and
//! cannot roll one back.
//!
//! # The address, and why the occurrence door
//!
//! A change is addressed to a [`crate::internal::itip::PropertyOccurrence`] — the second `ATTENDEE`, not
//! `ATTENDEE` — so it goes through [`ical_core::Component::apply_to_occurrence`] and never
//! through [`ical_core::Component::apply`]. The identity-addressed door writes *every*
//! occurrence of a name, which is the right rule for a caller naming an identity and the wrong
//! one for a `REPLY`: it would answer for every attendee on the list at once.
//!
//! # The two doors, and the policy each writes under
//!
//! [`ical_core::Component::apply_to_occurrence`] needs a [`Limits`], because a replacement is
//! octets off the wire like any other and is read through the same content-line reader a file
//! is. [`ScheduleTarget::write_change`] takes none, because a transition is a value and a
//! caller's policy is not part of one. So the policy lives on the target:
//!
//! - [`ComponentTarget`] carries the caller's own bounds, which is what ADR-0010 asks of every
//!   surface that reads untrusted octets.
//! - `impl ScheduleTarget for Component` writes under [`Limits::DEFAULT`], because a bare
//!   component carries no policy and the trait method has nowhere to take one from. It is here
//!   because a caller holding a `Component` should not have to name a wrapper for the ordinary
//!   case, and the ordinary case is safe: those octets came out of an [`crate::internal::itip::ItipMessage`]
//!   already read under the caller's own bounds, so the write-side ceiling is a second check on
//!   octets that already cleared a first one.
//!
//! That last sentence is the whole of the argument, and it stops holding in one place worth
//! naming: [`crate::internal::itip::Transition::new`] and [`crate::internal::itip::Transition::record`] are public, so a
//! hand-built transition's octets have cleared nothing. A caller that builds its own transition,
//! or whose policy is not the default one, uses [`ComponentTarget`].
//!
//! # What a [`MutationError`] becomes
//!
//! | [`MutationError`] | [`WriteRejected`] | why |
//! | --- | --- | --- |
//! | `Absent` | `UnknownProperty` | the occurrence the change named is not in this component |
//! | `MalformedReplacement` | `ValueTypeMismatch` | the octets are not one content line |
//! | `NotRepresentable` | `ValueTypeMismatch` | RFC 5545 section 3.2 cannot write that value |
//! | `ValueTooLarge` | `ValueTypeMismatch` | the octets are past the target's own ceiling |
//! | `ComponentBoundary` | `ReadOnly` | `BEGIN` and `END` are not properties to write |
//! | `IllegalControlCharacter` | `ReadOnly` | the target will not hold a value carrying one |
//!
//! The three that become `ValueTypeMismatch` are the ones where the *octets* are the problem
//! and a different message could carry the same change; the two that become `ReadOnly` are the
//! ones where the *line* is the problem and no message may write it here at all. An `Add`
//! landing at an index that is not the append position is `Absent` and therefore
//! `UnknownProperty`, which reads oddly until it is read as what it is: an addition names the
//! position it will occupy, and no other position exists for it to occupy.
//!
//! [`MutationError`] is `#[non_exhaustive]`, so the mapping is total by way of `ReadOnly`:
//! "the target did not take the change" is what every refusal has in common, and a refusal
//! this crate has never seen is not evidence about the caller's octets.

use ical_core::{Component, Limits, MutationError, ProposedChange};

use crate::internal::itip::state::PropertyOccurrence;
use crate::internal::itip::transition::{ScheduleTarget, WriteRejected};

/// An [`ical_core::Component`] to write into, under the caller's own bounds.
///
/// The door for a caller whose [`Limits`] are not the default ones, and the door for a caller
/// applying a transition it built itself rather than one an [`crate::internal::itip::ItipMessage`] produced.
/// See this module's own documentation for why the bare `impl ScheduleTarget for Component`
/// writes under [`Limits::DEFAULT`] instead.
///
/// It borrows the component rather than owning it, so the caller still holds it afterwards and
/// nothing here decides when a write becomes durable.
#[derive(Debug)]
pub struct ComponentTarget<'a> {
    /// Where the changes are written.
    component: &'a mut Component,
    /// The bounds every replacement line is read under.
    limits: Limits,
}

impl<'a> ComponentTarget<'a> {
    /// A target writing into `component` under `limits`.
    #[must_use]
    pub fn new(component: &'a mut Component, limits: Limits) -> Self {
        Self { component, limits }
    }

    /// The bounds every replacement line is read under.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// The component being written into.
    ///
    /// The same shape [`ical_core::PropertyMut::property`] takes: while this target lives it
    /// holds the only reach into the component, so reading it has to go through here.
    #[must_use]
    pub fn component(&self) -> &Component {
        self.component
    }
}

impl ScheduleTarget for ComponentTarget<'_> {
    /// Write one change through [`ical_core::Component::apply_to_occurrence`].
    ///
    /// # Errors
    ///
    /// [`WriteRejected`], as this module's table maps it. A refused change writes nothing at
    /// all: every refusal below is made before the property is reached, so a target that
    /// refused one change is exactly what it was before that change arrived.
    fn write_change(
        &mut self,
        at: &PropertyOccurrence,
        change: &ProposedChange,
    ) -> Result<(), WriteRejected> {
        write_occurrence(self.component, at, change, self.limits)
    }
}

impl ScheduleTarget for Component {
    /// Write one change under [`Limits::DEFAULT`].
    ///
    /// The bare component carries no policy of its own, and a trait method taking a description
    /// has nowhere to take one from. [`ComponentTarget`] is the door that carries the caller's.
    ///
    /// # Errors
    ///
    /// [`WriteRejected`], as this module's table maps it.
    fn write_change(
        &mut self,
        at: &PropertyOccurrence,
        change: &ProposedChange,
    ) -> Result<(), WriteRejected> {
        write_occurrence(self, at, change, Limits::DEFAULT)
    }
}

/// Apply one change to the occurrence `at` names, under `limits`.
///
/// One function behind both doors, so the two cannot come to disagree about which occurrence a
/// change addresses or about which refusals a target makes — the divergence ADR-0008 describes
/// on the reading side, arriving here as two write paths instead of two grammars.
fn write_occurrence(
    component: &mut Component,
    at: &PropertyOccurrence,
    change: &ProposedChange,
    limits: Limits,
) -> Result<(), WriteRejected> {
    component
        .apply_to_occurrence(at.id(), at.index(), change, limits)
        .map_err(refusal_for)
}

/// What one mutation refusal is, in the vocabulary a scheduling caller reports.
///
/// A total function over [`MutationError`] rather than a judgment made per call site, so the
/// table in this module's documentation is the whole mapping and a reviewer checks it once.
const fn refusal_for(error: MutationError) -> WriteRejected {
    match error {
        // The occurrence the change named is not in this component. An `Add` at an index that
        // is not the append position arrives here too, because the position it named is the
        // one thing about it that does not exist.
        MutationError::Absent => WriteRejected::UnknownProperty,
        // The octets are the problem, and a differently written message could carry the same
        // change: one content line rather than none or two, a parameter value RFC 5545
        // section 3.2 can spell, a value inside the target's own ceiling.
        MutationError::MalformedReplacement
        | MutationError::NotRepresentable
        | MutationError::ValueTooLarge { .. } => WriteRejected::ValueTypeMismatch,
        // The *line* is the problem and no message may write it here at all:
        // `ComponentBoundary` is a `BEGIN` or an `END`, which is a component and not a
        // property, and `IllegalControlCharacter` is a value that would end its own line.
        //
        // Everything a later version of `MutationError` adds arrives here as well, since the
        // enum is `#[non_exhaustive]`. That is the conservative reading: "the target did not
        // take the change" is what every refusal has in common, while `ValueTypeMismatch`
        // would be a claim about the caller's octets that nothing here could support.
        _ => WriteRejected::ReadOnly,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use ical_core::{
        Component, Document, IgnoreDiagnostics, Limits, MutationError, ParameterEdit, Property,
        PropertyId, ProposedChange, RawText,
    };

    use super::{ComponentTarget, refusal_for};
    use crate::internal::itip::state::PropertyOccurrence;
    use crate::internal::itip::transition::{
        ApplyReport, ScheduleTarget, Transition, TransitionReason, WriteRejected,
    };

    /// The line an addition writes, short enough to survive a refold as one physical line.
    const ADDED: &[u8] = b"ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:eve@x.test";

    /// One `VEVENT` with three `ATTENDEE` lines, the middle one carrying a parameter of the
    /// recipient's own that answering a `REPLY` must not cost it.
    ///
    /// Every line is under the 74 octets a rewritten line is refolded at, so a line this crate
    /// rewrites comes back as one physical line and "byte-identical" is a claim about the
    /// octets rather than about a fold.
    fn invitation() -> [&'static [u8]; 14] {
        [
            b"BEGIN:VCALENDAR",
            b"VERSION:2.0",
            b"BEGIN:VEVENT",
            b"UID:4088@x.test",
            b"SEQUENCE:2",
            b"DTSTAMP:20260810T120000Z",
            b"DTSTART:20260815T090000Z",
            b"SUMMARY:Standup",
            b"ORGANIZER:mailto:ann@x.test",
            b"ATTENDEE;PARTSTAT=NEEDS-ACTION;CN=Bo:mailto:bo@x.test",
            b"ATTENDEE;PARTSTAT=NEEDS-ACTION;X-VENDOR=kept:mailto:cy@x.test",
            b"ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:di@x.test",
            b"END:VEVENT",
            b"END:VCALENDAR",
        ]
    }

    /// Those lines as octets, each terminated the way RFC 5545 section 3.1 requires.
    fn ics(lines: &[&[u8]]) -> Vec<u8> {
        let mut text = Vec::new();
        for line in lines {
            text.extend_from_slice(line);
            text.extend_from_slice(b"\r\n");
        }
        text
    }

    /// The document those lines parse as.
    fn document_of(lines: &[&[u8]]) -> Document {
        Document::parse(&ics(lines), Limits::DEFAULT, &mut IgnoreDiagnostics)
            .expect("the fixture is a document")
    }

    /// The `VEVENT` inside the `VCALENDAR`.
    fn event_of(document: &mut Document) -> &mut Component {
        let calendar = document
            .components_mut()
            .next()
            .expect("the fixture holds one VCALENDAR");
        calendar
            .components_mut()
            .next()
            .expect("the VCALENDAR holds one VEVENT")
    }

    /// The `index`th `ATTENDEE` of `component`, in document order.
    fn attendee_of(component: &Component, index: usize) -> &Property {
        // Filtered rather than `properties_named`, whose iterator borrows the `&PropertyId`
        // for as long as the answer lives: `PropertyId::ATTENDEE` is a `const` carrying a
        // `Vec` arm, so `&PropertyId::ATTENDEE` is a temporary and cannot be promoted.
        component
            .properties()
            .filter(|property| property.has_id(&PropertyId::ATTENDEE))
            .nth(index)
            .expect("the fixture holds three ATTENDEE lines")
    }

    /// Every parameter of a property as `(name, stored spelling)`, in order.
    fn parameters_of(property: &Property) -> Vec<(&[u8], &[u8])> {
        property
            .parameters()
            .iter()
            .map(|held| (held.name().as_bytes(), held.value().as_bytes()))
            .collect()
    }

    /// Apply `transition` the way [`crate::internal::itip::apply_transition`] does.
    ///
    /// That function takes an `Authorization`, which is sealed inside `crate::internal::itip::authorize` and
    /// has no constructor this module can reach, so its loop is repeated here. The report is
    /// assembled through the same two public recorders, so what a caller sees is what this
    /// produces.
    fn apply(target: &mut dyn ScheduleTarget, transition: &Transition) -> ApplyReport {
        let mut report = ApplyReport::new();
        for (at, change) in transition.changes() {
            match target.write_change(at, change) {
                Ok(()) => report.note_applied(),
                Err(reason) => report.note_rejected(at.clone(), reason),
            }
        }
        report
    }

    /// RFC 5546 section 3.2.3: a `REPLY` states one attendee's `PARTSTAT` on that attendee's own
    /// `ATTENDEE` line. The occurrence door reaches exactly that line, so the other two come
    /// back octet for octet — which is the difference between this and `Component::apply`,
    /// whose identity-addressed rule would have answered for all three at once.
    #[test]
    fn a_reply_reaches_one_attendee_line_and_leaves_its_neighbors_byte_identical() {
        // (which ATTENDEE the reply answers on, what that line becomes)
        let cases: [(usize, &[u8]); 3] = [
            (0, b"ATTENDEE;PARTSTAT=ACCEPTED;CN=Bo:mailto:bo@x.test"),
            (
                1,
                b"ATTENDEE;PARTSTAT=ACCEPTED;X-VENDOR=kept:mailto:cy@x.test",
            ),
            (2, b"ATTENDEE;PARTSTAT=ACCEPTED:mailto:di@x.test"),
        ];

        for (index, answered) in cases {
            let mut document = document_of(&invitation());
            let change =
                ProposedChange::SetParameters(vec![ParameterEdit::set(b"PARTSTAT", b"ACCEPTED")]);
            let at = PropertyOccurrence::new(PropertyId::ATTENDEE, index);
            assert_eq!(event_of(&mut document).write_change(&at, &change), Ok(()));

            let mut expected = invitation();
            // The three ATTENDEE lines are the tenth, eleventh and twelfth of the fixture.
            expected[index.saturating_add(9)] = answered;
            assert_eq!(
                document.to_bytes(),
                ics(&expected),
                "answering attendee {index} rewrote something else as well"
            );
        }
    }

    /// RFC 5546 section 2.1.2: delegating writes `PARTSTAT=DELEGATED` and `DELEGATED-TO` on one
    /// `ATTENDEE` line, which is why the change vocabulary carries a *list* of parameter edits
    /// and not one edit. The recipient's own `X-` parameter survives, and the value's text is
    /// untouched — the whole reason a `REPLY` is a `SetParameters` rather than a `Replace`.
    #[test]
    fn a_delegating_reply_writes_both_parameters_on_one_line_and_keeps_the_recipients_own() {
        let mut document = document_of(&invitation());
        let change = ProposedChange::SetParameters(vec![
            ParameterEdit::set(b"PARTSTAT", b"DELEGATED"),
            ParameterEdit::set(b"DELEGATED-TO", b"mailto:ed@x.test"),
        ]);
        let at = PropertyOccurrence::new(PropertyId::ATTENDEE, 1);
        assert_eq!(event_of(&mut document).write_change(&at, &change), Ok(()));

        let event = event_of(&mut document);
        assert_eq!(
            parameters_of(attendee_of(event, 1)),
            vec![
                (&b"PARTSTAT"[..], &b"DELEGATED"[..]),
                (&b"X-VENDOR"[..], &b"kept"[..]),
                // Quoted because RFC 5545 section 3.2 excludes `:` from `SAFE-CHAR`.
                (&b"DELEGATED-TO"[..], &b"\"mailto:ed@x.test\""[..]),
            ],
            "the assignment kept its place, the recipient's own parameter stayed"
        );
        assert_eq!(
            attendee_of(event, 1).value_text().as_bytes(),
            b"mailto:cy@x.test",
            "a parameter edit is not a rewrite of the address it is written beside"
        );
        assert_eq!(
            parameters_of(attendee_of(event, 0)),
            vec![
                (&b"PARTSTAT"[..], &b"NEEDS-ACTION"[..]),
                (&b"CN"[..], &b"Bo"[..])
            ]
        );
        assert_eq!(
            parameters_of(attendee_of(event, 2)),
            vec![(&b"PARTSTAT"[..], &b"NEEDS-ACTION"[..])]
        );
    }

    /// `ProposedChange::Add` has no occurrence to name yet, so the index it names is where it
    /// will land and must be the append position. Any other index is refused rather than
    /// written somewhere near it, because an addition landing elsewhere would renumber every
    /// occurrence after it — and every other change in the same transition is addressed by
    /// exactly those numbers.
    #[test]
    fn an_addition_that_does_not_name_the_append_position_is_refused_rather_than_misplaced() {
        // (the index the addition names, what the write answers)
        let cases: [(usize, Result<(), WriteRejected>); 5] = [
            (0, Err(WriteRejected::UnknownProperty)),
            (1, Err(WriteRejected::UnknownProperty)),
            (2, Err(WriteRejected::UnknownProperty)),
            (3, Ok(())),
            (4, Err(WriteRejected::UnknownProperty)),
        ];

        for (index, expected) in cases {
            let mut document = document_of(&invitation());
            let change = ProposedChange::Add(RawText::from_bytes(&ics(&[ADDED])));
            let at = PropertyOccurrence::new(PropertyId::ATTENDEE, index);
            assert_eq!(
                event_of(&mut document).write_change(&at, &change),
                expected,
                "an addition naming occurrence {index} of three"
            );

            let mut lines: Vec<&[u8]> = invitation().to_vec();
            if expected.is_ok() {
                // After the last property of the component and ahead of its `END`, which is
                // where RFC 5545 section 3.6 puts a property.
                lines.insert(12, ADDED);
            }
            assert_eq!(
                document.to_bytes(),
                ics(&lines),
                "a refused addition writes nothing at all"
            );
        }
    }

    /// The occurrence door removes the line it names. `Component::apply` would have taken all
    /// three, which is the right rule for a caller naming an identity and the wrong one for a
    /// `CANCEL` that drops one participant.
    #[test]
    fn a_removal_takes_the_occurrence_it_names_and_no_other() {
        for index in 0..3_usize {
            let mut document = document_of(&invitation());
            let at = PropertyOccurrence::new(PropertyId::ATTENDEE, index);
            assert_eq!(
                event_of(&mut document).write_change(&at, &ProposedChange::Remove),
                Ok(())
            );

            let mut lines: Vec<&[u8]> = invitation().to_vec();
            lines.remove(index.saturating_add(9));
            assert_eq!(document.to_bytes(), ics(&lines));
        }
    }

    /// A partial application is reported rather than hidden: this crate owns no transaction and
    /// cannot roll one back, so a caller that needs all-or-nothing reads `is_complete` before
    /// committing its own storage. The rejections come back in occurrence order, which is the
    /// order the transition is walked in.
    #[test]
    fn a_partial_application_is_reported_rather_than_hidden() {
        let mut transition = Transition::new(TransitionReason::ParticipationChanged);
        transition.record(
            PropertyOccurrence::new(PropertyId::ATTENDEE, 1),
            ProposedChange::SetParameters(vec![ParameterEdit::set(b"PARTSTAT", b"ACCEPTED")]),
        );
        // An occurrence three attendees past the end of the list.
        transition.record(
            PropertyOccurrence::new(PropertyId::ATTENDEE, 6),
            ProposedChange::Remove,
        );
        // A line that would open a component rather than write a property.
        transition.record(
            PropertyOccurrence::named(b"BEGIN", 0),
            ProposedChange::Replace(RawText::from_bytes(b"BEGIN:VALARM\r\n")),
        );
        // Two content lines, which describe two changes where this key names one property.
        transition.record(
            PropertyOccurrence::new(PropertyId::SUMMARY, 0),
            ProposedChange::Replace(RawText::from_bytes(b"SUMMARY:one\r\nSUMMARY:two\r\n")),
        );

        let mut document = document_of(&invitation());
        let report = apply(event_of(&mut document), &transition);

        assert_eq!(report.applied(), 1);
        assert!(!report.is_complete());
        let refused: Vec<(&[u8], usize, WriteRejected)> = report
            .rejected()
            .iter()
            .map(|entry| (entry.at().name(), entry.at().index(), entry.reason()))
            .collect();
        assert_eq!(
            refused,
            vec![
                (&b"ATTENDEE"[..], 6, WriteRejected::UnknownProperty),
                (&b"BEGIN"[..], 0, WriteRejected::ReadOnly),
                (&b"SUMMARY"[..], 0, WriteRejected::ValueTypeMismatch),
            ]
        );

        let mut expected = invitation();
        expected[10] = b"ATTENDEE;PARTSTAT=ACCEPTED;X-VENDOR=kept:mailto:cy@x.test";
        assert_eq!(
            document.to_bytes(),
            ics(&expected),
            "the permitted change landed and each refused one wrote nothing"
        );
    }

    /// The whole mapping, in one place, so a reviewer checks it against the table in this
    /// module's documentation rather than against four call sites.
    #[test]
    fn every_mutation_refusal_has_one_reading_on_the_write_side() {
        let cases: [(MutationError, WriteRejected); 6] = [
            (MutationError::Absent, WriteRejected::UnknownProperty),
            (
                MutationError::MalformedReplacement,
                WriteRejected::ValueTypeMismatch,
            ),
            (
                MutationError::NotRepresentable,
                WriteRejected::ValueTypeMismatch,
            ),
            (
                MutationError::ValueTooLarge { limit: 8 },
                WriteRejected::ValueTypeMismatch,
            ),
            (MutationError::ComponentBoundary, WriteRejected::ReadOnly),
            (
                MutationError::IllegalControlCharacter,
                WriteRejected::ReadOnly,
            ),
        ];
        for (refusal, expected) in cases {
            assert_eq!(refusal_for(refusal), expected, "{refusal:?}");
        }
    }

    /// The bound a replacement is read under is the target's, and `ComponentTarget` carries the
    /// caller's own. The bare `impl ScheduleTarget for Component` writes under the default
    /// policy instead, which is the difference this test exists to make visible rather than to
    /// leave in prose.
    #[test]
    fn the_target_writes_under_the_bounds_it_carries() {
        // (the replacement line, what a target bounded at eight octets answers)
        let cases: [(&[u8], Result<(), WriteRejected>); 2] = [
            (b"SUMMARY:12345678\r\n", Ok(())),
            (
                b"SUMMARY:123456789\r\n",
                Err(WriteRejected::ValueTypeMismatch),
            ),
        ];
        let at = PropertyOccurrence::new(PropertyId::SUMMARY, 0);

        for (replacement, expected) in cases {
            let change = ProposedChange::Replace(RawText::from_bytes(replacement));

            let mut document = document_of(&invitation());
            let mut bounded = ComponentTarget::new(
                event_of(&mut document),
                Limits::DEFAULT.with_max_value_bytes(8),
            );
            assert_eq!(bounded.limits().max_value_bytes(), 8);
            assert_eq!(bounded.write_change(&at, &change), expected);
            assert_eq!(
                bounded
                    .component()
                    .properties_named(&PropertyId::SUMMARY)
                    .count(),
                1,
                "a refused replacement neither wrote nor removed anything"
            );

            let mut wider = document_of(&invitation());
            assert_eq!(
                event_of(&mut wider).write_change(&at, &change),
                Ok(()),
                "the bare component writes under Limits::DEFAULT, which admits both"
            );
        }
    }
}
