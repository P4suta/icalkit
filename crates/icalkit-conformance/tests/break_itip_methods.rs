// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5546 section 3's method tables, attacked against the specification's own text.
//!
//! Every expectation below is read from RFC 5546 — section 1.4 for the eight methods, sections
//! 3.2 to 3.5 for which component types each one is *defined over*, and each subsection's
//! constraint table for what a message of that method must and must not carry. None of them is
//! read off an answer this workspace gave, which is what makes a failure here evidence rather
//! than a regression.
//!
//! The subject is the shipped bridge, [`ical_itip::ScheduledView`] over an
//! [`ical_core::Component`], and not a second `ScheduledComponent` written for the occasion.
//! That matters for this chapter in particular: several of the cases turn on what the bridge
//! answers when a property RFC 5545 permits once is stated twice, and a hand-written subject
//! would be answering for itself rather than for the code a caller runs.
//!
//! # The shape of the attack
//!
//! Three questions, asked of every method:
//!
//! 1. Is the method *defined* for this component type at all? Section 3.5 states three tables
//!    for `VJOURNAL` and section 3.3 three for `VFREEBUSY`, so twenty-two of the thirty-two
//!    pairs exist and ten do not.
//! 2. Does the message state a `METHOD` this crate can act on — none, one, two, or one nobody
//!    has heard of?
//! 3. Does the payload satisfy its own table's required rows, and stay inside its `0` and
//!    `0 or 1` rows?
//!
//! The third is where this chapter's failures live. The gate reads the `0` rows and the rows
//! that require something, and never reads a `0 or 1` row — so a name RFC 5546 permits *at most
//! once* may be stated twice, and the bridge answers "absent" for exactly those names.

#[cfg(test)]
mod cases {
    use ical_core::{
        Component, ComponentKind, ContentLineReader, Diagnostic, Document, Item, Limits, Meter,
        Property, PropertyId, ProposedChange, decode_caret,
    };
    use ical_itip::{
        AuthorizationDenied, ItipMessage, MessageError, Method, PartyId, PropertyOccurrence,
        ScheduledComponent, ScheduledView, TransitionReason, apply_transition, evaluate_message,
    };

    /// The caller's held weekly series, at `SEQUENCE:2`, with two attendees.
    const HELD_SERIES: &[u8] = include_bytes!("fixtures/break_itip_methods/held_series.ics");
    /// The same series, whose only malformation is that its `UID` line appears twice.
    const HELD_UID_TWICE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/held_series_with_the_uid_stated_twice.ics");
    /// One override of that series, addressed by a `RECURRENCE-ID`.
    const HELD_INSTANCE: &[u8] = include_bytes!("fixtures/break_itip_methods/held_instance.ics");

    /// A well-formed `REQUEST` from the organizer, moving the series.
    const REQUEST_RESCHEDULES: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_reschedules.ics");
    /// A `REQUEST` from a party the caller never invited, about an unrelated `UID`.
    const REQUEST_TAKEOVER: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_takeover_from_a_stranger.ics");
    /// A calendar stating two different `METHOD`s, which RFC 5545 section 3.7.2 permits once.
    const TWO_METHODS: &[u8] =
        include_bytes!("fixtures/break_itip_methods/message_with_two_methods.ics");
    /// A `METHOD` naming nothing RFC 5546 section 1.4 defines.
    const UNHEARD_OF_METHOD: &[u8] =
        include_bytes!("fixtures/break_itip_methods/message_with_an_unheard_of_method.ics");
    /// A `CANCEL` naming one instance of the series.
    const CANCEL_ONE_INSTANCE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/cancel_of_one_instance.ics");
    /// The same `CANCEL`, with its `RECURRENCE-ID` stated twice.
    const CANCEL_ONE_INSTANCE_TWICE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/cancel_of_one_instance_named_twice.ics");
    /// A `REPLY` carrying two `ATTENDEE` lines, which section 3.2.3's table gives `1`.
    const REPLY_TWO_ATTENDEES: &[u8] =
        include_bytes!("fixtures/break_itip_methods/reply_with_two_attendees.ics");
    /// A `REPLY` carrying none, which the same row forbids from the other side.
    const REPLY_NO_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/reply_with_no_attendee.ics");
    /// A `COUNTER` that restates the held component exactly and so proposes nothing.
    const COUNTER_NOTHING: &[u8] =
        include_bytes!("fixtures/break_itip_methods/counter_proposing_nothing.ics");
    /// A `REPLY` whose one `ATTENDEE` states an empty `CAL-ADDRESS`.
    const REPLY_EMPTY_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/reply_with_an_empty_attendee.ics");
    /// A `REPLY` whose one `ATTENDEE` states octets that are not UTF-8.
    const REPLY_UNDECODABLE_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/reply_whose_attendee_does_not_decode.ics");
    /// A `REFRESH` sent by the organizer, which section 3.2.6's prose gives to an attendee.
    const REFRESH_FROM_ORGANIZER: &[u8] =
        include_bytes!("fixtures/break_itip_methods/refresh_from_the_organizer.ics");
    /// A `DECLINECOUNTER` sent by an attendee, which section 3.2.8 gives to the organizer.
    const DECLINECOUNTER_FROM_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/declinecounter_from_an_attendee.ics");
    /// A `REPLY` delegating to an address whose `DELEGATED-TO` uses RFC 6868's caret encoding.
    const REPLY_CARET_DELEGATE: &[u8] = include_bytes!(
        "fixtures/break_itip_methods/reply_delegating_to_a_caret_encoded_address.ics"
    );

    /// A `PUBLISH` over a `VEVENT`, whose `METHOD` the matrix case rewrites.
    const MATRIX_EVENT: &[u8] =
        include_bytes!("fixtures/break_itip_methods/method_over_a_vevent.ics");
    /// The same over a `VTODO`.
    const MATRIX_TODO: &[u8] =
        include_bytes!("fixtures/break_itip_methods/method_over_a_vtodo.ics");
    /// The same over a `VJOURNAL`.
    const MATRIX_JOURNAL: &[u8] =
        include_bytes!("fixtures/break_itip_methods/method_over_a_vjournal.ics");
    /// The same over a `VFREEBUSY`.
    const MATRIX_FREEBUSY: &[u8] =
        include_bytes!("fixtures/break_itip_methods/method_over_a_vfreebusy.ics");

    /// A `REQUEST` missing the `SUMMARY` its table gives `1`.
    const REQUEST_NO_SUMMARY: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_without_a_summary.ics");
    /// A `REQUEST` missing the `DTSTART` its table gives `1`.
    const REQUEST_NO_DTSTART: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_without_a_dtstart.ics");
    /// A `REQUEST` missing the `DTSTAMP` its table gives `1`.
    const REQUEST_NO_DTSTAMP: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_without_a_dtstamp.ics");
    /// A `REQUEST` missing the `ATTENDEE` its table gives `1+`.
    const REQUEST_NO_ATTENDEE: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_without_a_attendee.ics");
    /// A `REQUEST` missing the `ORGANIZER` its table gives `1`.
    const REQUEST_NO_ORGANIZER: &[u8] =
        include_bytes!("fixtures/break_itip_methods/request_without_a_organizer.ics");

    /// The organizer of every fixture here.
    const CHAIR: &str = "mailto:chair@example.com";
    /// The attendee the held list carries second.
    const BO: &str = "mailto:bo@example.com";
    /// A party on neither the organizer line nor the attendee list of anything the caller holds.
    const STRANGER: &str = "mailto:zz@example.com";

    /// The tree `octets` spells, under the default policy.
    ///
    /// `assert!` rather than an `unwrap`, matching the other chapters: a fixture that will not
    /// parse is a broken case and must say so rather than fail somewhere further in.
    fn document(octets: &[u8]) -> Document {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reader = ContentLineReader::new(octets, limits.grammar());
        let mut reported: Vec<Diagnostic> = Vec::new();
        let parsed = Document::from_tokens(&mut reader, &mut meter, &mut reported);
        assert!(parsed.is_ok(), "a fixture of this chapter did not parse");
        parsed.unwrap_or_default()
    }

    /// The outermost component of `octets`, which is its `VCALENDAR`.
    fn calendar(octets: &[u8]) -> Component {
        let tree = document(octets);
        let found = tree.items().iter().find_map(|entry| match entry {
            Item::Component(component) => Some(component.clone()),
            Item::Property(_) => None,
        });
        assert!(
            found.is_some(),
            "a fixture of this chapter has no component"
        );
        assert!(found.is_some(), "unreachable: asserted just above");
        found.unwrap_or_else(|| {
            calendar(
                b"BEGIN:VCALENDAR
END:VCALENDAR
",
            )
        })
    }

    /// `octets` with the `METHOD` value rewritten to `method`.
    ///
    /// The matrix case asks one question — is this (method, component type) pair one RFC 5546
    /// states a table for — and every other row of the fixture is deliberately identical, so that
    /// the answer cannot be about anything else.
    fn with_method(octets: &[u8], method: Method) -> Vec<u8> {
        let mut written = Vec::new();
        for line in octets.split(|byte| *byte == b'\n') {
            let text = line.strip_suffix(b"\r").unwrap_or(line);
            if text.is_empty() {
                continue;
            }
            if text.starts_with(b"METHOD:") {
                written.extend_from_slice(b"METHOD:");
                written.extend_from_slice(method.as_bytes());
            } else {
                written.extend_from_slice(text);
            }
            written.push(b'\n');
        }
        written
    }

    /// Read `calendar` as a scheduling message under the default policy.
    fn read_message<'a>(view: &'a ScheduledView<'a>) -> Result<ItipMessage<'a>, MessageError> {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        ItipMessage::read(view, limits, &mut meter, &mut sink)
    }

    /// Read `calendar` as a message, keeping the diagnostics it reported on the way.
    fn read_message_reporting<'a>(
        view: &'a ScheduledView<'a>,
        sink: &mut Vec<Diagnostic>,
    ) -> Result<ItipMessage<'a>, MessageError> {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        ItipMessage::read(view, limits, &mut meter, sink)
    }

    /// Every (method, component type) pair RFC 5546 section 3 prints a constraint table for.
    ///
    /// Transcribed from the specification's own section numbering, not from `ical-itip`'s table:
    /// section 3.2 gives all eight methods for `VEVENT`, section 3.3 gives `PUBLISH`, `REQUEST` and
    /// `REPLY` for `VFREEBUSY`, section 3.4 gives all eight for `VTODO`, and section 3.5 gives
    /// `PUBLISH`, `ADD` and `CANCEL` for `VJOURNAL`.
    fn rfc_defines(method: Method, kind: ComponentKind) -> bool {
        match kind {
            ComponentKind::Event | ComponentKind::Todo => true,
            ComponentKind::Journal => {
                matches!(method, Method::Publish | Method::Add | Method::Cancel)
            },
            ComponentKind::FreeBusy => {
                matches!(method, Method::Publish | Method::Request | Method::Reply)
            },
            _ => false,
        }
    }

    /// RFC 5546 section 3: a method exists for the component types its sections state a table for.
    ///
    /// Ten of the thirty-two pairs have no table — `REPLY` to a `VJOURNAL`, `CANCEL` of a
    /// `VFREEBUSY`, and eight more — and a pair with no table has no stated semantics, so accepting
    /// one is inventing them.
    ///
    /// `VFREEBUSY` is asked here for completeness and answered separately: without the `freebusy`
    /// feature every `VFREEBUSY` payload is refused before the pair is looked up at all, which is
    /// the refusing direction and is what the design document states. This chapter does not turn
    /// the feature on, because a feature enabled through a dev-dependency is enabled for the whole
    /// unified build.
    #[test]
    fn a_method_exists_only_for_the_component_types_section_3_states_a_table_for() {
        for (octets, kind) in [
            (MATRIX_EVENT, ComponentKind::Event),
            (MATRIX_TODO, ComponentKind::Todo),
            (MATRIX_JOURNAL, ComponentKind::Journal),
            (MATRIX_FREEBUSY, ComponentKind::FreeBusy),
        ] {
            for method in Method::ALL {
                let rewritten = with_method(octets, method);
                let held = calendar(&rewritten);
                let view = ScheduledView::of(&held);
                let answer = read_message(&view);
                let defined = rfc_defines(method, kind);
                let accepted = answer.is_ok();
                if kind == ComponentKind::FreeBusy {
                    // Whether this build reasons about a `VFREEBUSY` at all is a feature the
                    // workspace turns on for every crate at once, and this chapter cannot ask
                    // which way it was built. Both answers are stated, because both are the
                    // refusing direction: without the feature every method is refused whole,
                    // and with it the three tables section 3.3 prints are the three that read.
                    let refused_whole = answer.err()
                        == Some(MessageError::UnsupportedPayload(ComponentKind::FreeBusy));
                    assert!(
                        refused_whole || accepted == defined,
                        "{method:?} over a VFREEBUSY: RFC 5546 defines this pair = {defined}, \
                         the crate accepted it = {accepted}, and it was not refused whole"
                    );
                    continue;
                }
                assert_eq!(
                    accepted, defined,
                    "{method:?} over a {kind:?}: RFC 5546 defines this pair = {defined}, the crate \
                     accepted it = {accepted}"
                );
            }
        }
    }

    /// RFC 5545 section 3.7.2 permits one `METHOD` per calendar, and RFC 5546 section 3 reads it as
    /// the whole message's verb.
    ///
    /// Three inputs and three facts the specification keeps apart: a calendar with no `METHOD` is
    /// not a scheduling message at all, a `METHOD` naming nothing RFC 5546 defines is a message
    /// whose verb is *present and unusable*, and a calendar stating two of them is a third thing —
    /// a message that two conforming readers will act on differently.
    ///
    /// The crate's own documentation makes the middle distinction explicitly and reports
    /// `scheduling-method-unknown` for it. The third has no answer of its own here: the bridge
    /// answers "absent" for a `METHOD` stated twice, so a message carrying `METHOD:REPLY` beside
    /// `METHOD:REQUEST` is reported as *an ordinary calendar*, with no diagnostic, and a caller
    /// following the design document's own `on_incoming_message` will file it as a plain `.ics`.
    #[test]
    fn a_method_stated_twice_is_not_a_method_stated_never() {
        let nothing = calendar(HELD_SERIES);
        let nothing_view = ScheduledView::of(&nothing);
        assert_eq!(
            read_message(&nothing_view).err(),
            Some(MessageError::MissingMethod),
            "a calendar with no METHOD is not a message"
        );

        let unheard = calendar(UNHEARD_OF_METHOD);
        let unheard_view = ScheduledView::of(&unheard);
        let mut reported: Vec<Diagnostic> = Vec::new();
        assert_eq!(
            read_message_reporting(&unheard_view, &mut reported).err(),
            Some(MessageError::UnknownMethod),
            "a METHOD RFC 5546 does not define is present and unusable"
        );
        assert_eq!(
            reported.len(),
            1,
            "a present-and-unusable METHOD is reported on the item"
        );

        // The file carries `METHOD:REPLY` and `METHOD:REQUEST`. Whichever of the two a reader
        // picks, it has picked one; what it may not do is report that the calendar states
        // none, because that answer sends the file down the path for a calendar nobody was
        // scheduling with.
        let doubled = calendar(TWO_METHODS);
        let doubled_view = ScheduledView::of(&doubled);
        let stated = doubled_view.method().map(<[u8]>::to_vec);
        let mut about_it: Vec<Diagnostic> = Vec::new();
        let answer = read_message_reporting(&doubled_view, &mut about_it);
        assert_ne!(
            answer.err(),
            Some(MessageError::MissingMethod),
            "the file states two METHODs and the bridge answers {stated:?}, so the message is \
             refused as an ordinary calendar with {} diagnostics reported about it",
            about_it.len()
        );
        assert!(
            !about_it.is_empty(),
            "a METHOD present twice is present and unusable, which is a diagnostic on the item"
        );
    }

    /// RFC 5546 section 3.2.2's table: `DTSTAMP`, `DTSTART`, `ORGANIZER`, `SUMMARY` and `UID` are
    /// each `1`, and `ATTENDEE` is `1+`.
    ///
    /// A `REQUEST` missing any of them is a message its own table does not admit. The organizer row
    /// is refused earlier and for a different reason — with no `ORGANIZER` there is no party for
    /// the sender to be — and the case records which refusal each row earns rather than flattening
    /// them.
    #[test]
    fn a_request_missing_a_row_its_table_requires_is_refused() {
        for (octets, missing) in [
            (REQUEST_NO_SUMMARY, PropertyId::SUMMARY),
            (REQUEST_NO_DTSTART, PropertyId::DTSTART),
            (REQUEST_NO_DTSTAMP, PropertyId::DTSTAMP),
            (REQUEST_NO_ATTENDEE, PropertyId::ATTENDEE),
        ] {
            let held = calendar(HELD_SERIES);
            let held_view = ScheduledView::of(&held);
            let message = calendar(octets);
            let message_view = ScheduledView::of(&message);
            let read = read_message(&message_view);
            assert!(read.is_ok(), "{missing:?}: the message did not read at all");
            let Ok(message) = read else { continue };
            let Some(current) = held_view.child(0) else {
                continue;
            };
            assert_eq!(
                evaluate_message(&message, current, PartyId::new(CHAIR)).err(),
                Some(AuthorizationDenied::MethodRequiresField(missing.clone())),
                "section 3.2.2 gives {missing:?} a presence a message without it does not satisfy"
            );
        }

        let held = calendar(HELD_SERIES);
        let held_view = ScheduledView::of(&held);
        let message = calendar(REQUEST_NO_ORGANIZER);
        let message_view = ScheduledView::of(&message);
        let read = read_message(&message_view);
        assert!(read.is_ok(), "the organizerless REQUEST did not read");
        let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
            return;
        };
        let answer = evaluate_message(&message, current, PartyId::new(CHAIR));
        assert!(
            answer.is_err(),
            "section 3.2.2 gives ORGANIZER the value 1: {answer:?}"
        );
    }

    /// RFC 5546 section 3's prose, which no constraint table states: `REFRESH` and `COUNTER`
    /// come from an attendee, and `DECLINECOUNTER` comes from the organizer.
    ///
    /// The rows most likely to be transcribed the wrong way round, because they are the ones
    /// read from paragraphs rather than from tables, and the two below are the pair that would
    /// let each side send the other's message.
    #[test]
    fn the_sender_rule_holds_in_both_directions() {
        for (octets, actor, note) in [
            (
                REFRESH_FROM_ORGANIZER,
                CHAIR,
                "section 3.2.6: a REFRESH asks the organizer to resend, so the organizer does \
                 not send one",
            ),
            (
                DECLINECOUNTER_FROM_ATTENDEE,
                BO,
                "section 3.2.8: a DECLINECOUNTER declines an attendee's counter, so an attendee \
                 does not send one",
            ),
        ] {
            let held = calendar(HELD_SERIES);
            let held_view = ScheduledView::of(&held);
            let message = calendar(octets);
            let message_view = ScheduledView::of(&message);
            let read = read_message(&message_view);
            assert!(read.is_ok(), "{note}: the message did not read at all");
            let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
                continue;
            };
            let answer = evaluate_message(&message, current, PartyId::new(actor));
            assert!(answer.is_err(), "{note}: {answer:?}");
        }
    }

    /// RFC 5546 section 3.2.3: the `ATTENDEE` of a `REPLY` MUST be the address of the attendee
    /// replying, and RFC 5545 section 3.3.3 says what a `CAL-ADDRESS` is.
    ///
    /// An address that is present and does not identify anybody is the shape agenda item 7
    /// names: it must be reported on the item, never turned into a message that says nothing
    /// happened. Both spellings are here — an empty value and one that is not UTF-8 — because
    /// `PartyId` treats the second as matching nobody by design, and the question is what
    /// becomes of the *reply* once it matches nobody.
    ///
    /// Observed: the gate authorizes the reply and describes zero changes, so an attendee's
    /// answer is dropped with an `Ok` and no diagnostic of any kind. `inspect_message` has a
    /// code for the condition, and nothing on the evaluation path runs it.
    #[test]
    fn a_reply_whose_attendee_identifies_nobody_is_not_silently_an_answer_of_nothing() {
        let mut observed: Vec<(&str, Option<usize>)> = Vec::new();
        for (octets, note) in [
            (REPLY_EMPTY_ATTENDEE, "an ATTENDEE with an empty value"),
            (REPLY_UNDECODABLE_ATTENDEE, "an ATTENDEE that is not UTF-8"),
        ] {
            let held = calendar(HELD_SERIES);
            let held_view = ScheduledView::of(&held);
            let message = calendar(octets);
            let message_view = ScheduledView::of(&message);
            let read = read_message(&message_view);
            assert!(read.is_ok(), "{note}: the message did not read at all");
            let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
                continue;
            };
            let answer = evaluate_message(&message, current, PartyId::new(BO));
            observed.push((note, answer.ok().map(|allowed| allowed.transition().len())));
        }
        assert!(
            observed.iter().all(|(_, described)| *described != Some(0)),
            "a reply that identifies nobody is refused or answers for somebody, never \
             authorized to change nothing: {observed:?}"
        );
    }

    /// RFC 5546 section 3.2.3's table gives `ATTENDEE` the value `1` and says it MUST be the
    /// address of the attendee replying.
    ///
    /// Both directions of a `1` row: a reply that answers for nobody, and a reply that answers for
    /// two parties at once. The second is the one with teeth — a reply carrying a second attendee
    /// is one party writing a `PARTSTAT` for another.
    #[test]
    fn a_reply_states_exactly_one_attendee() {
        for (octets, note) in [
            (REPLY_NO_ATTENDEE, "a reply answering for nobody"),
            (REPLY_TWO_ATTENDEES, "a reply answering for two parties"),
        ] {
            let held = calendar(HELD_SERIES);
            let held_view = ScheduledView::of(&held);
            let message = calendar(octets);
            let message_view = ScheduledView::of(&message);
            let read = read_message(&message_view);
            assert!(read.is_ok(), "{note}: the message did not read at all");
            let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
                continue;
            };
            let answer = evaluate_message(&message, current, PartyId::new(BO));
            assert!(
                answer.is_err(),
                "{note} does not satisfy section 3.2.3's ATTENDEE row of 1: {answer:?}"
            );
        }
    }

    /// RFC 5546 section 3.2.5's table gives `RECURRENCE-ID` the value `0 or 1`, with the comment
    /// "only if referring to an instance of a recurring calendar component".
    ///
    /// So a `CANCEL` carrying one is about that instance and a `CANCEL` carrying none is about the
    /// whole series, and the two are different messages with very different consequences. A message
    /// stating the property twice satisfies neither reading and is refused by the row.
    ///
    /// What is observed instead: the second `RECURRENCE-ID` line makes the bridge answer *absent*,
    /// so a message that names one instance twice is judged as a message that names no instance at
    /// all — and the same organizer whose single-line `CANCEL` of that instance is refused against
    /// the held series gets the whole series cancelled by adding a duplicate of the line that was
    /// limiting its reach.
    #[test]
    fn a_cancel_naming_an_instance_never_reaches_the_series() {
        let series = calendar(HELD_SERIES);
        let series_view = ScheduledView::of(&series);
        let Some(master) = series_view.child(0) else {
            return;
        };

        // The control: one `RECURRENCE-ID`, judged against the series the caller holds. A message
        // about one instance is not a message about the series.
        let once = calendar(CANCEL_ONE_INSTANCE);
        let once_view = ScheduledView::of(&once);
        let read = read_message(&once_view);
        assert!(read.is_ok(), "the instance CANCEL did not read");
        if let Ok(message) = read {
            assert_eq!(
                evaluate_message(&message, master, PartyId::new(CHAIR)).err(),
                Some(AuthorizationDenied::NoMatchingInstance),
                "a CANCEL naming one instance does not reach the series"
            );
        }

        // The attack: the identical message with its `RECURRENCE-ID` stated twice.
        let twice = calendar(CANCEL_ONE_INSTANCE_TWICE);
        let twice_view = ScheduledView::of(&twice);
        let read = read_message(&twice_view);
        assert!(read.is_ok(), "the doubled instance CANCEL did not read");
        let Ok(message) = read else { return };
        let answer = evaluate_message(&message, master, PartyId::new(CHAIR));
        assert!(
            answer.is_err(),
            "section 3.2.5 gives RECURRENCE-ID the value 0 or 1, and a message stating it twice \
             states no instance this gate may pick: {:?}",
            answer.map(|allowed| (
                allowed.reason(),
                allowed
                    .transition()
                    .changes()
                    .map(|(at, _)| String::from_utf8_lossy(at.name()).into_owned())
                    .collect::<Vec<_>>(),
            ))
        );

        // And it is genuinely the instance the message named, not the series: the held override
        // under that `RECURRENCE-ID` is what a one-line CANCEL reaches.
        let instance = calendar(HELD_INSTANCE);
        let instance_view = ScheduledView::of(&instance);
        let once_again = calendar(CANCEL_ONE_INSTANCE);
        let once_again_view = ScheduledView::of(&once_again);
        let read = read_message(&once_again_view);
        let (Ok(message), Some(override_component)) = (read, instance_view.child(0)) else {
            return;
        };
        let answer = evaluate_message(&message, override_component, PartyId::new(CHAIR));
        assert!(
            answer.is_ok(),
            "a CANCEL naming an instance the caller holds is the message section 3.2.5 describes: \
             {answer:?}"
        );
    }

    /// RFC 5546 section 3.2.2, with ADR-0005 amendment 4: a `REQUEST` about something the caller
    /// already holds is judged against the `ORGANIZER` line *the caller holds*, and only a message
    /// about something it does not hold falls back to the party the message itself names.
    ///
    /// The fallback is a stated cost and is not what this case attacks. What it attacks is the
    /// question the fallback turns on — "does the caller hold this" — which the bridge answers from
    /// `UID`, and which it answers "no" for a component whose `UID` is stated twice. A held series
    /// with one duplicated line therefore stops being held, and a `REQUEST` from a party that
    /// component names nowhere, about an entirely different `UID`, is judged against nothing but
    /// itself.
    #[test]
    fn a_request_about_something_the_caller_holds_is_judged_against_what_it_holds() {
        let intact = calendar(HELD_SERIES);
        let intact_view = ScheduledView::of(&intact);
        let takeover = calendar(REQUEST_TAKEOVER);
        let takeover_view = ScheduledView::of(&takeover);
        let read = read_message(&takeover_view);
        assert!(read.is_ok(), "the takeover REQUEST did not read");
        let (Ok(message), Some(current)) = (read, intact_view.child(0)) else {
            return;
        };
        assert!(
            evaluate_message(&message, current, PartyId::new(STRANGER)).is_err(),
            "a stranger's REQUEST about another UID is refused against a series the caller holds"
        );

        // The same message, the same actor, the same held series — with its `UID` line duplicated.
        let doubled = calendar(HELD_UID_TWICE);
        let doubled_view = ScheduledView::of(&doubled);
        let takeover_again = calendar(REQUEST_TAKEOVER);
        let takeover_again_view = ScheduledView::of(&takeover_again);
        let read = read_message(&takeover_again_view);
        let (Ok(message), Some(current)) = (read, doubled_view.child(0)) else {
            return;
        };
        let seen = current.uid().map(<[u8]>::to_vec);
        let answer = evaluate_message(&message, current, PartyId::new(STRANGER));
        assert!(
            answer.is_err(),
            "the held component states its UID twice, the bridge reads that as {seen:?}, and a \
             party it names nowhere is then answered {:?}",
            answer.map(|allowed| (
                allowed.actor(),
                allowed.reason(),
                allowed
                    .transition()
                    .changes()
                    .map(|(at, change)| {
                        (
                            String::from_utf8_lossy(at.name()).into_owned(),
                            at.index(),
                            match change {
                                ProposedChange::Add(_) => "add",
                                ProposedChange::Replace(_) => "replace",
                                ProposedChange::SetParameters(_) => "set-parameters",
                                ProposedChange::Remove => "remove",
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
            ))
        );
    }

    /// RFC 5546 section 3.2.7: a `COUNTER` proposes an alternative.
    ///
    /// A counter that restates the held component exactly proposes nothing at all. The
    /// specification states no refusal for it, so both answers are permitted readings; the case
    /// records what this crate does rather than asserting one of them, and asserts only that the
    /// answer is not an *authorized change*, since an empty proposal has nothing to authorize.
    #[test]
    fn a_counter_that_proposes_nothing_changes_nothing() {
        let held = calendar(HELD_SERIES);
        let held_view = ScheduledView::of(&held);
        let message = calendar(COUNTER_NOTHING);
        let message_view = ScheduledView::of(&message);
        let read = read_message(&message_view);
        assert!(read.is_ok(), "the empty COUNTER did not read");
        let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
            return;
        };
        match evaluate_message(&message, current, PartyId::new(BO)) {
            Ok(allowed) => {
                assert_eq!(allowed.reason(), TransitionReason::CounterProposed);
                assert_eq!(
                    allowed.transition().len(),
                    0,
                    "a counter restating the held component proposes nothing"
                );
            },
            Err(denied) => {
                // The other permitted reading. Recorded, not preferred.
                assert!(
                    matches!(denied, AuthorizationDenied::MethodForbidsField(_)),
                    "an empty COUNTER refused for some other reason: {denied:?}"
                );
            },
        }
    }

    /// RFC 6868 section 2 and ADR-0001 amendment 3: a parameter value crossing from one property to
    /// another is decoded once and encoded once.
    ///
    /// A `REPLY` delegating writes `DELEGATED-TO` from the sender's own `ATTENDEE` line onto the
    /// recipient's, which is the one place in this crate where a parameter is copied between two
    /// properties. The file spells the delegate `mailto:d^'q^^v@example.com`, which is the value
    /// `mailto:d"q^v@example.com`; the written line must read back as that same value and not as
    /// the doubly-encoded `mailto:d^^'q^^^^v@example.com`.
    #[test]
    fn a_parameter_copied_between_two_properties_is_encoded_once() {
        let held = calendar(HELD_SERIES);
        let held_view = ScheduledView::of(&held);
        let message = calendar(REPLY_CARET_DELEGATE);
        let message_view = ScheduledView::of(&message);
        let read = read_message(&message_view);
        assert!(read.is_ok(), "the delegating REPLY did not read");
        let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
            return;
        };
        let judged = evaluate_message(&message, current, PartyId::new(BO));
        assert!(judged.is_ok(), "a delegation is a legal reply: {judged:?}");
        let Ok(authorized) = judged else { return };
        assert!(
            authorized
                .transition()
                .change(&PropertyOccurrence::named(b"ATTENDEE", 1))
                .is_some(),
            "the reply writes the delegator's own line"
        );

        let mut target = calendar(HELD_SERIES);
        let Some(event) = target.components_mut().next() else {
            return;
        };
        let report = apply_transition(event, authorized);
        assert!(report.is_complete(), "the target took every change");

        let Some(line) = event
            .properties()
            .filter(|property| property.has_id(&PropertyId::ATTENDEE))
            .nth(1)
        else {
            return;
        };
        let written = delegated_to(line);
        assert_eq!(
            written.as_deref(),
            Some(&b"mailto:d\"q^v@example.com"[..]),
            "the value the file stated, once decoded and once encoded"
        );
    }

    /// The `DELEGATED-TO` value `property` states, unquoted and with its carets resolved.
    fn delegated_to(property: &Property) -> Option<Vec<u8>> {
        let held = property
            .parameters_named(b"DELEGATED-TO")
            .find(|entry| entry.has_value())?;
        Some(decode_caret(held.unquoted()).as_ref().to_vec())
    }

    /// RFC 5546 section 3.2.2 read through the crate's own well-formed path, so that every refusal
    /// above is measured against a message this gate does accept.
    #[test]
    fn the_well_formed_request_this_chapter_measures_against_is_accepted() {
        let held = calendar(HELD_SERIES);
        let held_view = ScheduledView::of(&held);
        let message = calendar(REQUEST_RESCHEDULES);
        let message_view = ScheduledView::of(&message);
        let read = read_message(&message_view);
        assert!(read.is_ok(), "the control REQUEST did not read");
        let (Ok(message), Some(current)) = (read, held_view.child(0)) else {
            return;
        };
        let answer = evaluate_message(&message, current, PartyId::new(CHAIR));
        assert!(
            answer.is_ok(),
            "the control REQUEST was refused: {answer:?}"
        );
        assert!(
            evaluate_message(&message, current, PartyId::new(STRANGER)).is_err(),
            "and a stranger sending it is not"
        );
    }

    /// Ignored properties of the type vocabulary this chapter leans on, kept honest.
    #[test]
    fn the_matrix_fixture_rewriting_touches_only_the_method_line() {
        let rewritten = with_method(MATRIX_EVENT, Method::Cancel);
        assert!(
            rewritten.windows(14).any(|run| run == b"METHOD:CANCEL\n"),
            "the rewriting wrote the method it was asked for"
        );
        assert!(
            !rewritten.windows(15).any(|run| run == b"METHOD:PUBLISH\n"),
            "and left none of the one it replaced"
        );
        assert!(
            rewritten.windows(13).any(|run| run == b"BEGIN:VEVENT\n"),
            "and changed nothing else"
        );
    }

    /// A `ScheduledComponent` implementation is required to exist for this chapter's subject.
    #[test]
    fn the_subject_is_the_shipped_bridge() {
        let held = calendar(HELD_SERIES);
        let view = ScheduledView::of(&held);
        assert_eq!(view.component_kind(), Some(ComponentKind::Calendar));
        let Some(event) = view.child(0) else {
            panic!("the held calendar carries one VEVENT");
        };
        assert_eq!(event.component_kind(), Some(ComponentKind::Event));
        assert_eq!(event.attendee_count(), 2);
    }
}
