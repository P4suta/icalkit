// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ical-itip` attacked on version ordering and identity.
//!
//! RFC 5546 section 2.1.4 orders two versions of one component by `SEQUENCE`, and section 2.1.5
//! breaks a tie between two equal `SEQUENCE`s with `DTSTAMP`. That pair is the whole of iTIP's
//! replay defense, `SECURITY.md` names "a stale `SEQUENCE` overwriting a newer version" as a
//! security failure rather than a correctness one, and `ical-itip`'s own `identity` module says
//! the tie is broken "towards refusal" because the alternative "lets a message with no
//! `DTSTAMP` at all overwrite one that has one".
//!
//! Every expectation below is read from those two sections and from the crate's own stated
//! invariants, never off an answer this workspace gave. The state each case is judged against
//! is read through `ScheduledView`, the bridge `ical-itip` ships, so that where a case fails it
//! fails against the crate's own reading of a file and not against a harness written here.
//!
//! Where a case needs a *store* rather than one snapshot — two messages arriving in either
//! order, a message replayed, a second reply from one attendee — it applies through
//! `apply_transition` into an owned `Component` and re-reads it, which is the only way to ask
//! whether the defense that refused the second message was one the first message installed.

/// The cases, in one module so that the helpers they share are test code like the cases
/// themselves: `unwrap` inside a free function is production code to Clippy unless the
/// module says otherwise, and a corpus helper that returned `Option` to please a lint would
/// read as though a missing fixture were an expected answer.
#[cfg(test)]
mod cases {
    use icalkit_conformance::internal::core::{
        Component, Diagnostic, Document, IgnoreDiagnostics, Instant, Limits, Meter, ProposedChange,
    };
    use icalkit_conformance::internal::itip::{
        AuthorizationDenied, ItipMessage, MessageError, PartStat, PartyId, PropertyOccurrence,
        ScheduledComponent, ScheduledView, SequenceRead, Transition, TransitionReason,
        apply_transition, evaluate_message,
    };

    /// The series the caller holds: `SEQUENCE:2`, `DTSTAMP:20260301T120000Z`, two attendees.
    const HELD_SERIES: &[u8] = include_bytes!("fixtures/break_itip_sequence/held_series.ics");
    /// The same series stored at `SEQUENCE:0`.
    const HELD_SEQUENCE_ZERO: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_sequence_zero.ics");
    /// The same series stored at `SEQUENCE:5`.
    const HELD_SEQUENCE_FIVE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_sequence_five.ics");
    /// The same series as an imported copy that states its `UID` twice.
    const HELD_UID_TWICE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_uid_stated_twice.ics");
    /// One override of the series, addressed by a `RECURRENCE-ID` naming one instant.
    const HELD_INSTANCE: &[u8] = include_bytes!("fixtures/break_itip_sequence/held_instance.ics");
    /// The same override stored with `RANGE=THISANDFUTURE`.
    const HELD_INSTANCE_ONWARDS: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_instance_onwards.ics");
    /// The earlier half of the hour `America/New_York` repeats, addressed by a wall clock.
    const HELD_FOLD_EARLIER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_folded_earlier_half.ics");
    /// The later half of the same hour, addressed by the same wall clock.
    const HELD_FOLD_LATER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_folded_later_half.ics");
    /// The earlier half again, addressed by the same wall clock under a `TZID`.
    const HELD_FOLD_ZONED: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/held_folded_zoned_earlier.ics");

    /// A `REQUEST` at the held `SEQUENCE` carrying an older `DTSTAMP`: the control.
    const REQUEST_OLDER_DTSTAMP: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_older_dtstamp.ics");
    /// The same message with its `DTSTAMP` written as a `DATE`.
    const REQUEST_DTSTAMP_DATE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_dtstamp_as_a_date.ics");
    /// The same message with its `DTSTAMP` written under a `TZID`.
    const REQUEST_DTSTAMP_ZONED: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_dtstamp_under_a_tzid.ics");
    /// The same message carrying exactly the `DTSTAMP` the caller already holds.
    const REQUEST_SAME_DTSTAMP: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_identical_dtstamp.ics");
    /// A `REQUEST` with no `SEQUENCE` at all, which RFC 5546 section 3.2 reads as zero.
    const REQUEST_NO_SEQUENCE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_without_sequence.ics");
    /// A `REQUEST` whose `SEQUENCE` is `0000001`.
    const REQUEST_LEADING_ZEROS: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_leading_zeros.ics");
    /// A `REQUEST` whose `SEQUENCE` is `+9`.
    const REQUEST_SIGNED: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_signed.ics");
    /// A `REQUEST` whose `SEQUENCE` is `-1`.
    const REQUEST_NEGATIVE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_negative.ics");
    /// A `REQUEST` whose `SEQUENCE` is one past `u32::MAX`.
    const REQUEST_PAST_U32: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_past_u32.ics");
    /// A `REQUEST` whose `SEQUENCE` is twenty digits long.
    const REQUEST_ABSURD: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_absurd.ics");
    /// A `REQUEST` at `u32::MAX`, which is past what RFC 5545 section 3.3.8 calls an integer.
    const REQUEST_CEILING: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_ceiling.ics");
    /// A `REQUEST` at `2147483647`, the highest integer RFC 5545 section 3.3.8 admits.
    const REQUEST_INT_MAX: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_sequence_int_max.ics");
    /// A `REQUEST` at `SEQUENCE:3` moving the series.
    const REQUEST_THREE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_at_sequence_three.ics");
    /// A `REQUEST` at `SEQUENCE:5` moving it further.
    const REQUEST_FIVE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_at_sequence_five.ics");
    /// A `REQUEST` about the held identity naming a party nobody invited as its `ORGANIZER`.
    const REQUEST_STRANGER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/request_from_a_stranger.ics");
    /// A `REPLY` whose `UID` differs from the held one only by case.
    const REPLY_UID_UPPER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_uid_upper_cased.ics");
    /// A `REPLY` whose `UID` differs from the held one only by a trailing space.
    const REPLY_UID_SPACED: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_uid_trailing_space.ics");
    /// A `REPLY` whose `UID` differs from the held one only by a trailing NUL.
    const REPLY_UID_NUL: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_uid_trailing_nul.ics");
    /// A `REPLY` declining, at `DTSTAMP:20260301T140000Z`.
    const REPLY_DECLINED_LATER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_cy_declined_later.ics");
    /// The same attendee's earlier answer, accepting, at `DTSTAMP:20260301T130000Z`.
    const REPLY_ACCEPTED_EARLIER: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_cy_accepted_earlier.ics");
    /// A `REPLY` addressed to a repeated hour by a wall clock nobody placed.
    const REPLY_FLOATING_FOLD: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_to_the_floating_fold.ics");
    /// The same reply with the wall clock placed under a `TZID`.
    const REPLY_ZONED_FOLD: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_to_the_zoned_fold.ics");
    /// A `REPLY` naming 02:30 on 2026-03-08, an hour `America/New_York` does not have.
    const REPLY_IN_THE_GAP: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/reply_naming_a_gap.ics");
    /// An `ADD` carrying a `RECURRENCE-ID`, which section 3.2.4's table gives `0`.
    const ADD_NAMING_AN_INSTANCE: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/add_naming_an_instance.ics");
    /// A `COUNTER` whose `RECURRENCE-ID` reaches every later instance.
    const COUNTER_ONWARDS: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/counter_this_and_future.ics");
    /// A `CANCEL` whose `RECURRENCE-ID` reaches every later instance.
    const CANCEL_ONWARDS: &[u8] =
        include_bytes!("fixtures/break_itip_sequence/cancel_this_and_future.ics");

    /// The organizer every fixture names, except where a case says otherwise.
    const CHAIR: &str = "mailto:chair@example.com";
    /// The attendee the held list carries second.
    const BO: &str = "mailto:bo@example.com";
    /// The attendee it carries first.
    const CY: &str = "mailto:cy@example.com";
    /// A party on neither the organizer line nor the attendee list.
    const STRANGER: &str = "mailto:zz@example.com";

    /// What one judgment answered, in a form that outlives the borrows it was made through.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Answer {
        /// The gate authorized it: the kind of change, and every occurrence it would touch.
        Allowed(TransitionReason, Vec<(Vec<u8>, usize)>),
        /// The gate refused it, naming this reason.
        Refused(AuthorizationDenied),
        /// It was not a scheduling message at all.
        NotAMessage(MessageError),
    }

    impl Answer {
        /// Whether the gate let this one through.
        fn is_allowed(&self) -> bool {
            matches!(*self, Self::Allowed(..))
        }

        /// Whether it describes a change to the occurrence `name` states first.
        fn touches(&self, name: &[u8]) -> bool {
            match *self {
                Self::Allowed(_, ref changes) => changes.iter().any(|(at, _)| at == name),
                _ => false,
            }
        }
    }

    /// The document `source` spells, read under the default policy with nothing reported.
    fn read(source: &[u8]) -> Document {
        Document::parse(source, Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// The one `VCALENDAR` a fixture holds.
    fn calendar_of(document: &Document) -> &Component {
        document.components().next().unwrap()
    }

    /// The one component inside it, which is the state a caller holds.
    fn event_of(document: &Document) -> &Component {
        calendar_of(document).components().next().unwrap()
    }

    /// Every occurrence a transition would touch, as a name and an index.
    fn occurrences(transition: &Transition) -> Vec<(Vec<u8>, usize)> {
        transition
            .changes()
            .map(|(at, _)| (at.name().to_vec(), at.index()))
            .collect()
    }

    /// Judge the message `calendar` carries against `held`, on behalf of `actor`.
    ///
    /// Both sides are read through `ScheduledView`, so the answer is the crate's own reading of
    /// two files rather than a harness's.
    fn judge(held: &Component, calendar: &Component, actor: &str) -> Answer {
        let current = ScheduledView::of(held);
        let offered = ScheduledView::of(calendar);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let message = match ItipMessage::read(&offered, limits, &mut meter, &mut sink) {
            Ok(message) => message,
            Err(error) => return Answer::NotAMessage(error),
        };
        match evaluate_message(&message, &current, PartyId::new(actor)) {
            Ok(authorized) => {
                Answer::Allowed(authorized.reason(), occurrences(authorized.transition()))
            },
            Err(denied) => Answer::Refused(denied),
        }
    }

    /// Judge `message` against `held`, both as octets, on behalf of `actor`.
    fn judge_files(held: &[u8], message: &[u8], actor: &str) -> Answer {
        let state = read(held);
        let calendar = read(message);
        judge(event_of(&state), calendar_of(&calendar), actor)
    }

    /// Apply `message` into `store` on behalf of `actor`, answering what the gate said.
    ///
    /// The snapshot the decision is made against is a clone, because `Authorization` borrows the
    /// state it judged and `apply_transition` writes into a separate value — which is the shape the
    /// design document says the borrow checker forces on every caller.
    fn apply_into(store: &mut Component, message: &[u8], actor: &str) -> Answer {
        let snapshot = store.clone();
        let calendar = read(message);
        let answered = judge(&snapshot, calendar_of(&calendar), actor);
        if !answered.is_allowed() {
            return answered;
        }
        let current = ScheduledView::of(&snapshot);
        let offered = ScheduledView::of(calendar_of(&calendar));
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let read_message = ItipMessage::read(&offered, limits, &mut meter, &mut sink).unwrap();
        let authorized = evaluate_message(&read_message, &current, PartyId::new(actor)).unwrap();
        let report = apply_transition(store, authorized);
        assert!(
            report.is_complete(),
            "the store refused part of an authorized transition: {report:?}"
        );
        answered
    }

    /// The `SEQUENCE` a stored component now states.
    fn stored_sequence(store: &Component) -> SequenceRead {
        ScheduledView::of(store).sequence()
    }

    /// The `PARTSTAT` the stored copy now records for its `index`th attendee.
    fn stored_part_stat(store: &Component, index: usize) -> PartStat {
        ScheduledView::of(store)
            .attendee(index)
            .unwrap()
            .part_stat()
    }

    /// The `DTSTART` line the stored copy now carries.
    fn stored_dtstart(store: &Component) -> Vec<u8> {
        let view = ScheduledView::of(store);
        for index in 0..view.property_count() {
            if view.property_name(index) == Some(&b"DTSTART"[..]) {
                return view.property_line(index).unwrap_or_default().to_vec();
            }
        }
        Vec::new()
    }

    /// The instant `20260301T120000Z`, which is the `DTSTAMP` the held series carries.
    fn held_dtstamp() -> Instant {
        let document = read(HELD_SERIES);
        ScheduledView::of(event_of(&document)).dtstamp().unwrap()
    }

    /// RFC 5546 section 2.1.5: two equal `SEQUENCE`s are ordered by `DTSTAMP`, and the older one
    /// does not overwrite the newer.
    ///
    /// The control is the message whose `DTSTAMP` is an older UTC instant, and it is refused. The
    /// other three are the same message, at the same `SEQUENCE`, carrying the same older claim in a
    /// spelling `ical-itip`'s own reader answers `None` for — a `DATE`, and a value under a `TZID`,
    /// are both documented as "answer `None`", and section 2.1.5 breaks a tie with this number. A
    /// message whose tie-break cannot be read has not won the tie: `identity.rs` says in its own
    /// words that the refusing direction exists because the alternative "lets a message with no
    /// `DTSTAMP` at all overwrite one that has one".
    #[test]
    fn a_message_whose_dtstamp_cannot_be_read_does_not_win_the_tie_it_cannot_break() {
        let stale = AuthorizationDenied::DtstampStale {
            have: held_dtstamp(),
        };
        // Every row is judged before anything is asserted, so one failure reports all three
        // answers rather than only the first one that differs.
        let observed: Vec<(&str, Answer)> = [
            ("dtstamp-as-an-older-utc-instant", REQUEST_OLDER_DTSTAMP),
            ("dtstamp-written-as-a-date", REQUEST_DTSTAMP_DATE),
            ("dtstamp-written-under-a-tzid", REQUEST_DTSTAMP_ZONED),
        ]
        .into_iter()
        .map(|(id, message)| (id, judge_files(HELD_SERIES, message, CHAIR)))
        .collect();
        let moved: Vec<&str> = observed
            .iter()
            .filter(|(_, answered)| answered.touches(b"DTSTART"))
            .map(|(id, _)| *id)
            .collect();
        assert!(
            moved.is_empty(),
            "these messages moved the meeting at the revision already held: {moved:?}, from \
             {observed:?}"
        );
        for (id, answered) in observed {
            assert_eq!(answered, Answer::Refused(stale.clone()), "{id}");
        }
    }

    /// What a message whose `DTSTAMP` does not read leaves behind, once it has been applied.
    ///
    /// The tie-break RFC 5546 section 2.1.5 orders equal revisions with is a property of the stored
    /// component, so a message that writes an unreadable one into the caller's copy disarms the
    /// comparison for every later message at that `SEQUENCE`. The control here is the same stale
    /// message the case above refuses against the pristine store.
    #[test]
    fn a_message_does_not_disarm_the_tie_break_the_next_message_is_ordered_by() {
        let held = read(HELD_SERIES);
        let mut store = event_of(&held).clone();
        assert_eq!(
            judge(
                &store.clone(),
                calendar_of(&read(REQUEST_OLDER_DTSTAMP)),
                CHAIR
            ),
            Answer::Refused(AuthorizationDenied::DtstampStale {
                have: held_dtstamp()
            }),
            "the pristine store orders this message and refuses it"
        );

        apply_into(&mut store, REQUEST_DTSTAMP_DATE, CHAIR);
        let after = judge(
            &store.clone(),
            calendar_of(&read(REQUEST_OLDER_DTSTAMP)),
            CHAIR,
        );
        assert!(
            !after.is_allowed(),
            "one applied message left the store with nothing to order the next one by: {after:?}"
        );
    }

    /// RFC 5546 section 3.2: an absent `SEQUENCE` is zero, which is a revision and not an unknown.
    ///
    /// Against a stored zero it is the same revision and its `DTSTAMP` decides; against a stored
    /// five it is four revisions behind and is refused whatever its `DTSTAMP` says.
    #[test]
    fn an_absent_sequence_is_zero_against_a_stored_zero_and_against_a_stored_five() {
        assert_eq!(
            judge_files(HELD_SEQUENCE_FIVE, REQUEST_NO_SEQUENCE, CHAIR),
            Answer::Refused(AuthorizationDenied::SequenceStale { have: 5 }),
            "no SEQUENCE is revision zero, which is four behind a stored five"
        );
        assert_eq!(
            judge_files(HELD_SERIES, REQUEST_NO_SEQUENCE, CHAIR),
            Answer::Refused(AuthorizationDenied::SequenceStale { have: 2 })
        );
        let against_zero = judge_files(HELD_SEQUENCE_ZERO, REQUEST_NO_SEQUENCE, CHAIR);
        assert!(
            against_zero.is_allowed(),
            "an equal revision with a later DTSTAMP supersedes: {against_zero:?}"
        );
    }

    /// A `SEQUENCE` that is not a revision is refused rather than read as one.
    ///
    /// RFC 5545 section 3.8.7.4 makes `SEQUENCE` a non-negative integer, and `ical-itip` states
    /// that a present-and-unreadable one is "no revision at all". The rows below are the spellings
    /// an attacker reaches for: a value below the range, a value above it, and a value long enough
    /// that a reader accumulating digits would wrap.
    #[test]
    fn a_sequence_outside_the_range_is_no_revision_rather_than_a_large_one() {
        for (id, message) in [
            ("a-negative-sequence", REQUEST_NEGATIVE),
            ("one-past-u32", REQUEST_PAST_U32),
            ("twenty-digits", REQUEST_ABSURD),
            // RFC 5545 section 3.3.8 stops an integer at 2147483647, so this is not a revision
            // either — it is above the range the value type states, not merely above `i32`.
            ("u32-max", REQUEST_CEILING),
        ] {
            assert_eq!(
                judge_files(HELD_SERIES, message, CHAIR),
                Answer::Refused(AuthorizationDenied::SequenceUnreadable),
                "{id}"
            );
        }
        // Leading zeros are an integer's spelling and not a second value: `0000001` is revision
        // one, which is behind the stored two.
        assert_eq!(
            judge_files(HELD_SERIES, REQUEST_LEADING_ZEROS, CHAIR),
            Answer::Refused(AuthorizationDenied::SequenceStale { have: 2 }),
            "leading zeros"
        );
        // The top of RFC 5545 section 3.3.8's range is a revision like any other, and it is ahead
        // of the stored two.
        assert!(
            judge_files(HELD_SERIES, REQUEST_INT_MAX, CHAIR).is_allowed(),
            "the highest revision an integer can state is still a revision"
        );
        // A signed integer is what RFC 5545 section 3.3.8 writes, so `+9` is nine. Either reading
        // lands in the refusing direction, so this row records which one and refuses neither.
        let signed = judge_files(HELD_SERIES, REQUEST_SIGNED, CHAIR);
        assert!(
            signed.is_allowed()
                || signed == Answer::Refused(AuthorizationDenied::SequenceUnreadable),
            "a signed SEQUENCE is either nine or nothing: {signed:?}"
        );
    }

    /// Two updates about one identity, arriving in either order, leave the same state behind.
    ///
    /// The second half is the one that matters: the defense that refuses the older message is one
    /// the newer message had to install, so this asks whether applying a transition actually moves
    /// the stored revision.
    #[test]
    fn two_updates_about_one_identity_land_the_same_way_in_either_order() {
        let held = read(HELD_SERIES);

        let mut ascending = event_of(&held).clone();
        assert!(apply_into(&mut ascending, REQUEST_THREE, CHAIR).is_allowed());
        assert_eq!(stored_sequence(&ascending), SequenceRead::Value(3));
        assert!(apply_into(&mut ascending, REQUEST_FIVE, CHAIR).is_allowed());
        assert_eq!(stored_sequence(&ascending), SequenceRead::Value(5));

        let mut descending = event_of(&held).clone();
        assert!(apply_into(&mut descending, REQUEST_FIVE, CHAIR).is_allowed());
        assert_eq!(stored_sequence(&descending), SequenceRead::Value(5));
        assert_eq!(
            apply_into(&mut descending, REQUEST_THREE, CHAIR),
            Answer::Refused(AuthorizationDenied::SequenceStale { have: 5 }),
            "an older revision arriving second is refused"
        );
        assert_eq!(
            stored_dtstart(&ascending),
            stored_dtstart(&descending),
            "the same two messages left two different meetings behind"
        );
    }

    /// The same message applied twice is idempotent rather than additive.
    #[test]
    fn replaying_one_message_changes_nothing_the_second_time() {
        let held = read(HELD_SERIES);
        let mut store = event_of(&held).clone();
        assert!(apply_into(&mut store, REQUEST_FIVE, CHAIR).is_allowed());
        let once = stored_dtstart(&store);
        let count = ScheduledView::of(&store).property_count();

        let replayed = apply_into(&mut store, REQUEST_FIVE, CHAIR);
        assert_eq!(
            replayed,
            Answer::Allowed(TransitionReason::Updated, Vec::new()),
            "a message already applied describes nothing further"
        );
        assert_eq!(stored_dtstart(&store), once);
        assert_eq!(
            ScheduledView::of(&store).property_count(),
            count,
            "the replay added properties to the stored copy"
        );
    }

    /// RFC 5546 section 2.1.5, on the channel it was written for: two answers from one attendee.
    ///
    /// An attendee declines at 14:00Z and the earlier acceptance at 13:00Z is replayed afterwards.
    /// The later answer is the current one, and the earlier one must not overwrite it — this is the
    /// same sentence that refuses a stale `REQUEST`, applied to the message type that carries an
    /// attendee's participation.
    #[test]
    fn an_earlier_reply_does_not_overwrite_the_later_one_from_the_same_attendee() {
        let held = read(HELD_SERIES);
        let mut store = event_of(&held).clone();

        assert!(apply_into(&mut store, REPLY_DECLINED_LATER, CY).is_allowed());
        assert_eq!(stored_part_stat(&store, 0), PartStat::Declined);

        let replayed = apply_into(&mut store, REPLY_ACCEPTED_EARLIER, CY);
        assert_eq!(
            stored_part_stat(&store, 0),
            PartStat::Declined,
            "an answer from 13:00Z overwrote the one from 14:00Z: {replayed:?}"
        );
    }

    /// Agenda item 1: two instances on one cadence key are two meetings, and a reply to one is not
    /// a reply to the other.
    ///
    /// `America/New_York` repeats 01:30 on 2026-11-01, so a `RECURRENCE-ID` naming that wall clock
    /// names two real instants — 05:30Z and 06:30Z — and nothing in the file says which. `ical-itip`
    /// states the rule for exactly this: "a message whose instance cannot be told from its neighbor
    /// is denied rather than applied to a guess, because a guess cancels somebody else's meeting."
    /// The same reply is offered here against each half in turn, written once as a bare wall clock
    /// and once under the `TZID` that repeats it.
    #[test]
    fn a_reply_to_one_half_of_a_repeated_hour_is_not_a_reply_to_the_other() {
        // The same wall clock written under the zone that repeats it, which is the shape the
        // gate already refuses. Asserted first so that the row below reports on its own.
        assert_eq!(
            judge_files(HELD_FOLD_ZONED, REPLY_ZONED_FOLD, BO),
            Answer::Refused(AuthorizationDenied::AmbiguousInstance),
            "a wall clock under the zone that repeats it names two instants and picks neither"
        );
        // The other side of the same transition: 02:30 on 2026-03-08 is an hour
        // `America/New_York` skips, so no instance of the series is there to answer for.
        assert_eq!(
            judge_files(HELD_SERIES, REPLY_IN_THE_GAP, BO),
            Answer::Refused(AuthorizationDenied::NoMatchingInstance),
            "a reply naming an hour the zone does not have answers for no instance"
        );

        let earlier = judge_files(HELD_FOLD_EARLIER, REPLY_FLOATING_FOLD, BO);
        let later = judge_files(HELD_FOLD_LATER, REPLY_FLOATING_FOLD, BO);
        assert!(
            !(earlier.is_allowed() && later.is_allowed()),
            "one reply answered both halves of the repeated hour: earlier {earlier:?}, later \
             {later:?}"
        );
    }

    /// A `UID` is octets: RFC 5545 section 3.8.4.7 gives it no case folding and no whitespace
    /// stripping, and the direction that merges two identifiers is how a message about one meeting
    /// reaches another.
    #[test]
    fn an_identifier_that_differs_by_case_or_by_space_is_a_different_identifier() {
        for (id, message) in [
            ("uid-upper-cased", REPLY_UID_UPPER),
            ("uid-with-a-trailing-space", REPLY_UID_SPACED),
            // A NUL is a control character RFC 5545 section 3.1 excludes from a value, so a
            // reader may refuse the line, keep it, or drop the octet — but no reading of it
            // may end with the message being *about* the identifier the caller holds.
            ("uid-with-a-trailing-nul", REPLY_UID_NUL),
        ] {
            assert_eq!(
                judge_files(HELD_SERIES, message, BO),
                Answer::Refused(AuthorizationDenied::UidMismatch),
                "{id}"
            );
        }
    }

    /// A stored copy whose own `UID` does not read is still a component the caller holds.
    ///
    /// `ScheduledView` answers `None` for a `UID` stated twice, which its own documentation calls
    /// the reading that "refuses the message rather than picking a winner out of two". The gate
    /// reads that same `None` as `PriorState::Absent` — the caller holds nothing — and RFC 5546
    /// section 3.2.2 lets a `REQUEST` act on nothing and looks its sending party up in the message.
    /// So this case offers a `REQUEST` about the held identity, from a party the held copy names
    /// nowhere, naming itself as the `ORGANIZER`, and asks who the gate says may apply it. The
    /// held copy carries `chair@example.com` on its own `ORGANIZER` line and neither `zz` nor any
    /// message may make that line say something else.
    #[test]
    fn a_stranger_does_not_become_the_organizer_of_a_component_whose_uid_did_not_read() {
        let answered = judge_files(HELD_UID_TWICE, REQUEST_STRANGER, STRANGER);
        assert!(
            !answered.is_allowed(),
            "a party nobody invited was authorized against a component the caller holds: {answered:?}"
        );
        assert_eq!(
            answered,
            Answer::Refused(AuthorizationDenied::OrganizerMismatch)
        );
    }

    /// RFC 5546 section 3.2.4's table gives `ADD` a `RECURRENCE-ID` of `0`, and a reference reaching
    /// every later instance is a different claim from one naming a single instance.
    #[test]
    fn a_reference_reaching_further_than_the_method_admits_is_refused() {
        // Refused, though on the identity gate rather than on the table: an `ADD` naming an
        // instance the caller does not hold never reaches the row that forbids the reference. RFC
        // 5546 states no order between its own refusals, so the row records which one is named
        // first rather than asserting the other.
        let added = judge_files(HELD_SERIES, ADD_NAMING_AN_INSTANCE, CHAIR);
        assert!(!added.is_allowed(), "an ADD states no instance at all");
        assert_eq!(
            added,
            Answer::Refused(AuthorizationDenied::NoMatchingInstance),
            "the identity gate runs before section 3.2.4's table"
        );
        let countered = judge_files(HELD_INSTANCE, COUNTER_ONWARDS, BO);
        assert!(
            !countered.is_allowed(),
            "a THISANDFUTURE reference does not address an override stored for one instance: \
             {countered:?}"
        );
        // A `CANCEL` may carry `RANGE=THISANDFUTURE`, and what this crate describes for one is a
        // change to the single component it was handed. The row records the extent, because
        // "represented and not implemented" is a claim about the transition and not about the gate.
        let cancelled = judge_files(HELD_INSTANCE_ONWARDS, CANCEL_ONWARDS, CHAIR);
        assert!(
            matches!(cancelled, Answer::Allowed(TransitionReason::Cancelled, _)),
            "a cancellation of an override stored with the same range: {cancelled:?}"
        );
    }

    /// A message at the revision the caller already holds, restating a different time.
    ///
    /// `SEQUENCE` and `DTSTAMP` are equal, so neither version supersedes the other and RFC 5546
    /// section 2.1.4 has nothing to order: the two are the same revision of one component, and one
    /// of them is not the one the organizer sent. `identity.rs` breaks such a tie "towards
    /// refusal", so a message that cannot show it is newer does not get to move the meeting.
    #[test]
    fn a_message_at_the_revision_already_held_does_not_move_the_meeting() {
        let answered = judge_files(HELD_SERIES, REQUEST_SAME_DTSTAMP, CHAIR);
        assert!(
            !answered.touches(b"DTSTART"),
            "an equal revision restating a different time moved it: {answered:?}"
        );
    }

    /// A `ProposedChange` is the vocabulary a transition is written in, and this file reads one
    /// nowhere except through the occurrences above — the row is here so that the import which
    /// makes the crate's change vocabulary visible is exercised rather than merely named.
    #[test]
    fn a_transition_states_its_changes_in_the_shared_vocabulary() {
        let held = read(HELD_SERIES);
        let calendar = read(REQUEST_THREE);
        let current = ScheduledView::of(event_of(&held));
        let offered = ScheduledView::of(calendar_of(&calendar));
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink: Vec<Diagnostic> = Vec::new();
        let message = ItipMessage::read(&offered, limits, &mut meter, &mut sink).unwrap();
        let authorized = evaluate_message(&message, &current, PartyId::new(CHAIR)).unwrap();
        let dtstart = PropertyOccurrence::named(b"DTSTART", 0);
        assert!(matches!(
            authorized.transition().change(&dtstart),
            Some(&ProposedChange::Replace(_))
        ));
    }
}
