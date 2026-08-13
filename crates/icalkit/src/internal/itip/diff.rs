// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a message would change, worked out by comparing octets.
//!
//! ADR-0005 leaves octet-versus-typed comparison open and warns about the hole under it. This
//! is the octet answer, chosen because its failure direction is the safe one: byte-identical
//! means untouched, so no organizer-only field an attendee edited can report "unchanged". The
//! cost lands on the other side, where a re-fold or a reordered parameter reports a change a
//! semantic diff would not, and a caller diffing for display will see it.
//!
//! A `REPLY` is the exception. It states one `ATTENDEE` and RFC 5546 section 3.2.3 says its
//! other properties MUST NOT differ from the request's, so diffing the whole component would
//! describe every property the replying client happened to echo. It is matched to the local
//! attendee list by `CAL-ADDRESS` and expressed as
//! [`ProposedChange::SetParameters`], so the
//! recipient's own `X-` parameters on that `ATTENDEE` line survive an answer to it.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::internal::core::{
    DateTimeValue, EncodeValue, Instant, ParameterEdit, PropertyId, ProposedChange, RawText,
    ValueBuf,
};
use crate::internal::tz::wall_clock;

use crate::internal::itip::message::ItipMessage;
use crate::internal::itip::method::Method;
use crate::internal::itip::party::{ANSWERED_AT, PartyId};
use crate::internal::itip::state::{PropertyOccurrence, ScheduledComponent};
use crate::internal::itip::transition::{Transition, TransitionReason, is_time_property};

/// What `message` would change about `current`, described and not made.
///
/// Hands a caller what a *denied* message tried to do without handing it the ability to do it,
/// which is ADR-0005's recommendation that a rejected reply stay inspectable. The result is
/// inert: applying it needs a [`crate::internal::itip::Authorization`], and no route leads from here to one.
#[must_use]
pub fn describe_message(message: &ItipMessage<'_>, current: &dyn ScheduledComponent) -> Transition {
    let Some(payload) = message.payload_for(current).or_else(|| message.payload(0)) else {
        return Transition::new(reason_for(message.method(), current, false));
    };
    describe_payload(message.method(), payload, current)
}

/// What one payload would change about `current`.
#[must_use]
pub fn describe_payload(
    method: Method,
    payload: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
) -> Transition {
    if method == Method::Reply {
        return describe_reply(payload, current);
    }
    // RFC 5546 section 3.2.6: a `REFRESH` asks the organizer to resend and changes nothing in
    // the recipient's copy. Diffing it against the whole held component would state the removal
    // of every property the four-line request does not echo — the organizer's own `DTSTART`,
    // `RRULE` and attendee list — and the field rule would then refuse the attendee for
    // removals this function invented rather than for anything the attendee wrote.
    if method == Method::Refresh {
        return Transition::new(TransitionReason::RefreshRequested);
    }
    let offered = lines(payload);
    let held = lines(current);
    let moved = offered.iter().any(|(at, line)| {
        is_time_property(at.name()) && held.get(at).is_none_or(|kept| kept != line)
    });
    let mut transition = Transition::new(reason_for(method, current, moved));
    for (at, line) in &offered {
        match held.get(at) {
            Some(kept) if kept == line => {},
            Some(_) => {
                transition.record(
                    at.clone(),
                    ProposedChange::Replace(RawText::from_bytes(line)),
                );
            },
            None => {
                transition.record(at.clone(), ProposedChange::Add(RawText::from_bytes(line)));
            },
        }
    }
    for at in held.keys() {
        if !offered.contains_key(at) {
            transition.record(at.clone(), ProposedChange::Remove);
        }
    }
    transition
}

/// What a `REPLY` would change: one attendee's answer, and nothing else.
///
/// The reply's own `ATTENDEE` line is matched into the local list by `CAL-ADDRESS`, so the
/// occurrence named is the recipient's own numbering rather than the sender's. A reply from an
/// address the local copy does not carry describes nothing at all — saying so is
/// [`crate::internal::itip::evaluate_message`]'s job, and describing an addition here would be this function
/// inventing the participant that the gate exists to refuse.
fn describe_reply(
    payload: &dyn ScheduledComponent,
    current: &dyn ScheduledComponent,
) -> Transition {
    let mut transition = Transition::new(TransitionReason::ParticipationChanged);
    let Some(answer) = payload.attendee(0) else {
        return transition;
    };
    let Some(who) = answer.party().address() else {
        return transition;
    };
    let Some(at) = attendee_occurrence_of(current, who) else {
        return transition;
    };
    let mut edits = Vec::new();
    if let Some(status) = answer.part_stat_text() {
        edits.push(ParameterEdit::set(b"PARTSTAT", status));
    }
    match answer.delegated_to() {
        Some(delegate) => edits.push(ParameterEdit::set(b"DELEGATED-TO", delegate.as_bytes())),
        None => edits.push(ParameterEdit::remove(b"DELEGATED-TO")),
    }
    if let Some(delegator) = answer.delegated_from() {
        edits.push(ParameterEdit::set(b"DELEGATED-FROM", delegator.as_bytes()));
    }
    // When the answer was written, recorded on the line it answers for. Without it the next
    // reply from this attendee is one RFC 5546 section 2.1.5 has nothing to order, and the
    // attendee's own earlier answer replayed afterwards silently reverts this one.
    let stamped = payload.dtstamp().and_then(utc_text);
    match stamped.as_ref() {
        Some(written) => edits.push(ParameterEdit::set(ANSWERED_AT, written.as_bytes())),
        // A reply whose own `DTSTAMP` did not read records no time rather than a wrong one, and
        // the parameter already on the line is cleared: keeping an older answer's time beside a
        // newer answer's status would order the next reply against the wrong one.
        None => edits.push(ParameterEdit::remove(ANSWERED_AT)),
    }
    transition.record(at, ProposedChange::SetParameters(edits));
    transition
}

/// `instant` written as the UTC `DATE-TIME` a `DTSTAMP` is, or `None` when it is off the
/// representable calendar.
///
/// Through [`crate::internal::tz::wall_clock`], because that is the inverse of the projection every
/// timestamp reaching this crate came in on: a `DTSTAMP` is read onto the nominal timeline, and
/// writing one back through any other offset would move it.
fn utc_text(instant: Instant) -> Option<RawText> {
    let mut written = ValueBuf::new();
    DateTimeValue::Utc(wall_clock(instant)?)
        .encode_value(&mut written)
        .ok()?;
    Some(written.into_raw_text())
}

/// Which `ATTENDEE` occurrence of `component` is `who`.
#[must_use]
pub fn attendee_occurrence_of(
    component: &dyn ScheduledComponent,
    who: PartyId<'_>,
) -> Option<PropertyOccurrence> {
    (0..component.attendee_count()).find_map(|index| {
        let attendee = component.attendee(index)?;
        attendee
            .party()
            .is(who)
            .then(|| component.attendee_occurrence(index))?
    })
}

/// Every property of `component`, keyed by the occurrence it is.
///
/// Counted per normalized identity rather than per name as written, because that is the key the
/// answer is filed under: an implementation reporting `DTSTART` on one line and `dtstart` on
/// another would otherwise count two first occurrences and file both under one key, and the
/// second would silently replace the first.
fn lines(component: &dyn ScheduledComponent) -> BTreeMap<PropertyOccurrence, &[u8]> {
    let mut seen: BTreeMap<PropertyId, usize> = BTreeMap::new();
    let mut found = BTreeMap::new();
    for index in 0..component.property_count() {
        let (Some(name), Some(line)) = (
            component.property_name(index),
            component.property_line(index),
        ) else {
            continue;
        };
        let id = PropertyId::from_name(name);
        let at = seen.entry(id.clone()).or_insert(0);
        found.insert(PropertyOccurrence::new(id, *at), line);
        *at = at.saturating_add(1);
    }
    found
}

/// What kind of change a method describes, given what the caller already holds.
pub(crate) fn reason_for(
    method: Method,
    current: &dyn ScheduledComponent,
    moved: bool,
) -> TransitionReason {
    match method {
        Method::Publish => TransitionReason::Published,
        Method::Request if current.uid().is_none() => TransitionReason::Created,
        Method::Request if moved => TransitionReason::Rescheduled,
        Method::Request => TransitionReason::Updated,
        Method::Reply => TransitionReason::ParticipationChanged,
        Method::Add => TransitionReason::InstancesAdded,
        Method::Cancel => TransitionReason::Cancelled,
        Method::Refresh => TransitionReason::RefreshRequested,
        Method::Counter => TransitionReason::CounterProposed,
        Method::DeclineCounter => TransitionReason::CounterDeclined,
    }
}
