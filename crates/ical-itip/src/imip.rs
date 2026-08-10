// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The MIME envelope an iTIP object arrives in, and the claims it makes about the object.
//!
//! Specification: RFC 6047, "iCalendar Message-Based Interoperability Protocol (iMIP)"
//! <https://www.rfc-editor.org/rfc/rfc6047>, section 2.4 for the `Content-Type` parameters and
//! section 2.5 for what the mail addresses do and do not mean; RFC 2045 section 5.1 for the
//! header grammar those parameters are written in.
//!
//! # The envelope is not the object
//!
//! An iMIP message is an iTIP object carried in an email, and the email says things about the
//! object that the object also says about itself. Those are **two statements from two parties**,
//! and this module exists so that a caller has somewhere to compare them rather than a reason to
//! conflate them:
//!
//! - The envelope's `From` is **not** the `ORGANIZER` and **not** the `ATTENDEE` the iCalendar
//!   object names. It is the address that submitted the mail, which anybody may write.
//!   [`sender_is_named`] answers exactly one question about it — whether that address appears on
//!   the component at all — and answers nothing about whether its owner may do anything.
//!   Treating "the mail came from this address" as "this address is the organizer" is how a
//!   forged invitation is accepted, and it is a conflation no later check can undo, because by
//!   then the identity the gate is judging is the attacker's choice.
//! - The `Content-Type` header's `method` parameter is the envelope's claim about the object's
//!   `METHOD`. [`MediaTypeParams::agrees_with`] compares the two. RFC 6047 section 2.4 requires
//!   the parameter and requires it to match, so a message where it is absent or different is one
//!   whose two halves disagree — and a reader that trusts either half over the other is picking
//!   which of two messages an attacker gets to send.
//!
//! # Thinness is the security claim
//!
//! Nothing here evaluates anything. This module reads no clock, opens nothing, adds no
//! dependency, and **changes no authorization answer**: a caller composes it *in front of*
//! [`crate::evaluate_message`], which is judged against the address the caller supplies and
//! against the object itself. A version of this module that returned a verdict would be a second
//! gate, and two gates that can disagree is one gate too many.
//!
//! [`sender_is_named`] is therefore deliberately **permissive**: it is a filter a caller may run
//! before spending the gate's work, and it admits every party the gate would recognize plus the
//! addresses a delegation names. A filter that refused something the gate would have allowed
//! would silently change the answer, which is the one thing this module may not do.
//!
//! # Bounds
//!
//! [`MediaTypeParams::read`] is bounded by [`Limits`] and charges the caller's [`Meter`], for
//! ADR-0010's reason: a header arrives from the same stranger the calendar did, and five
//! thousand of them are bounded in aggregate only against a shared ledger. Every refusal is an
//! `Err` over the whole header, never a truncation — a quoted parameter value that never closes
//! is [`MediaTypeError::UnterminatedValue`] and not a value ending wherever the octets did,
//! because a header that two readers finish in two places is a header that says two things.

use alloc::vec::Vec;

use ical_core::{Limits, Meter};

use crate::message::ItipMessage;
use crate::method::Method;
use crate::party::{Attendee, PartyId};
use crate::state::ScheduledComponent;

/// Why a `Content-Type` header value was not read.
///
/// Every variant refuses the whole header. There is no partially-read [`MediaTypeParams`],
/// because a half-read header is a claim about the object that nobody made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MediaTypeError {
    /// The header was longer than the caller's policy admits.
    TooLong,
    /// The caller's shared ledger ran out.
    BudgetExhausted,
    /// The header carried an octet no unfolded header field may contain.
    ///
    /// A bare `CR` or `LF` means the caller handed over more than one header field, or a folded
    /// one it did not unfold. Either way the value being read is not the value that was sent.
    ControlOctet,
    /// The header did not begin with a `type/subtype`.
    MalformedMediaType,
    /// A parameter was not `attribute=value`.
    MalformedParameter,
    /// A quoted parameter value never closed.
    ///
    /// The refusal RFC 6047's thinness rests on: truncating here would let an attacker choose
    /// where a value ends, and therefore choose which method the envelope appears to declare.
    UnterminatedValue,
    /// One of the parameters RFC 6047 section 2.4 names was written twice.
    ///
    /// Refused rather than resolved, because implementations disagree about whether the first
    /// or the last wins, and a header shaped to be read two ways is one an attacker wrote in
    /// order to be read two ways.
    RepeatedParameter,
}

/// What an RFC 6047 section 2.4 `Content-Type` header claims about the object it carries.
///
/// The three parameters RFC 5545 section 8.1 registers for `text/calendar` and RFC 6047 uses:
/// `method`, `component` and `charset`. [`MediaTypeParams::read`] is the only constructor, so a
/// value of this type is one that came from a header rather than from a caller's assumption.
///
/// Values are held unescaped — RFC 2045's quoted-string escapes are resolved — which is why
/// they are owned rather than borrowed from the header: `"V\"EVENT"` names octets that appear
/// nowhere in the input contiguously.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaTypeParams {
    /// The `type/subtype`, as the header spelled it.
    media_type: Vec<u8>,
    /// The `method` parameter's value, unescaped.
    method: Option<Vec<u8>>,
    /// The `component` parameter's value, unescaped.
    component: Option<Vec<u8>>,
    /// The `charset` parameter's value, unescaped.
    charset: Option<Vec<u8>>,
}

impl MediaTypeParams {
    /// Read `header` as a `Content-Type` header value.
    ///
    /// `header` is the field body alone — everything after `Content-Type:` — unfolded and with
    /// no terminator, which is the same unit
    /// [`ScheduledComponent::property_line`] deals in. `limits` and `meter` are ADR-0010's pair:
    /// the policy is the caller's and the ledger outlives this call.
    ///
    /// An RFC 5322 comment is **not** stripped. `text/calendar; method=REQUEST (an invitation)`
    /// is [`MediaTypeError::MalformedParameter`], because a comment-aware reader and a
    /// comment-blind one disagree about the value, and refusing is the direction that costs
    /// interoperability rather than security.
    ///
    /// # Errors
    ///
    /// [`MediaTypeError`], every variant of which refuses the whole header.
    pub fn read(header: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, MediaTypeError> {
        charge(header, limits, meter)?;
        if header.iter().copied().any(is_forbidden_octet) {
            return Err(MediaTypeError::ControlOctet);
        }
        let mut cursor = Reader::new(header);
        let mut params = Self {
            media_type: cursor.media_type()?,
            method: None,
            component: None,
            charset: None,
        };
        while let Some((name, value)) = cursor.next_parameter()? {
            params.store(name, value)?;
        }
        Ok(params)
    }

    /// The `type/subtype`, as the header spelled it.
    #[must_use]
    pub fn media_type(&self) -> &[u8] {
        &self.media_type
    }

    /// Whether the envelope declared `text/calendar`.
    ///
    /// A separate question from [`MediaTypeParams::agrees_with`], and RFC 6047 section 2.4
    /// requires both: a mislabeled media type is a fact about the envelope, and the method
    /// parameter is a claim about the body. A caller that wants the whole of section 2.4 asks
    /// this and then that.
    #[must_use]
    pub fn is_calendar(&self) -> bool {
        self.media_type.eq_ignore_ascii_case(&b"text/calendar"[..])
    }

    /// The `method` parameter's value as written, absent when the header stated none.
    #[must_use]
    pub fn method(&self) -> Option<&[u8]> {
        self.method.as_deref()
    }

    /// The method the envelope declared.
    ///
    /// `None` when the header stated no `method` parameter and also when it stated one RFC 5546
    /// does not define — two different facts, kept apart by [`MediaTypeParams::method`], and one
    /// answer here because neither is a method this library can agree with.
    #[must_use]
    pub fn declared_method(&self) -> Option<Method> {
        self.method().and_then(Method::read)
    }

    /// The `component` parameter's value, absent when the header stated none.
    #[must_use]
    pub fn component(&self) -> Option<&[u8]> {
        self.component.as_deref()
    }

    /// The `charset` parameter's value, absent when the header stated none.
    #[must_use]
    pub fn charset(&self) -> Option<&[u8]> {
        self.charset.as_deref()
    }

    /// Whether the envelope's `method` parameter names `message`'s own `METHOD`.
    ///
    /// RFC 6047 section 2.4 requires the parameter and requires it to agree, so an absent one is
    /// `false` here rather than "no claim was made": a message whose envelope declines to say
    /// what it is has not agreed with anything. A value RFC 5546 does not define is `false` for
    /// the same reason.
    ///
    /// The comparison is made through [`Method::read`], so `method=reply` and `METHOD:REPLY`
    /// agree, as RFC 5545 section 3.1 compares every value drawn from an enumerated set.
    #[must_use]
    pub fn agrees_with(&self, message: &ItipMessage<'_>) -> bool {
        self.declared_method() == Some(message.method())
    }

    /// Record one parameter, refusing a second spelling of a name already stated.
    ///
    /// A name this does not know is skipped rather than refused: RFC 5545 section 8.1 registers
    /// `optinfo` as well, and a header carrying one is still a header this module can read.
    fn store(&mut self, name: &[u8], value: Vec<u8>) -> Result<(), MediaTypeError> {
        for (spelling, slot) in [
            (&b"method"[..], &mut self.method),
            (b"component", &mut self.component),
            (b"charset", &mut self.charset),
        ] {
            if !spelling.eq_ignore_ascii_case(name) {
                continue;
            }
            if slot.is_some() {
                return Err(MediaTypeError::RepeatedParameter);
            }
            *slot = Some(value);
            return Ok(());
        }
        Ok(())
    }
}

/// Whether `sender` appears anywhere on `component` as a party to it.
///
/// **This answers presence and never permission.** RFC 6047 section 2.5 puts the iTIP object in
/// an email whose `From` is an ordinary mail address: it is not the `ORGANIZER`, it is not the
/// `ATTENDEE`, and nothing in the transport makes it either. What it is worth is a cheap
/// negative — a message from an address the component has never heard of is one a caller may
/// drop before spending anything on it — and that is the whole of this function's contract.
///
/// Whether the party may act is [`crate::evaluate_message`]'s question, judged against the same
/// address, and this function must never be composed as if it answered that: `true` here is
/// consistent with every denial [`crate::AuthorizationDenied`] states.
///
/// The `ORGANIZER` and `ATTENDEE` lines directly inside `component` are consulted, together with
/// their `SENT-BY` agents and the addresses a delegation names on either side. Lines inside a
/// nested component are not: an `ATTENDEE` on a `VALARM` is who the alarm mails, not a party to
/// the meeting, and admitting it would let a message be accepted on the strength of somebody
/// else's reminder.
#[must_use]
pub fn sender_is_named(sender: PartyId<'_>, component: &dyn ScheduledComponent) -> bool {
    let organizes = component
        .organizer()
        .is_some_and(|party| party.is(sender) || party.is_agent_of(sender));
    organizes
        || (0..component.attendee_count()).any(|index| {
            component
                .attendee(index)
                .is_some_and(|who| attends(who, sender))
        })
}

/// Whether `sender` is `who`, their agent, or a party to their delegation.
fn attends(who: Attendee<'_>, sender: PartyId<'_>) -> bool {
    who.party().is(sender)
        || who.party().is_agent_of(sender)
        || who
            .delegated_to()
            .is_some_and(|reached| reached.matches(sender))
        || who
            .delegated_from()
            .is_some_and(|source| source.matches(sender))
}

/// Refuse a header past the policy's ceiling, then debit the ledger for the one admitted.
///
/// The ceiling is checked first so that an oversized header costs a length comparison rather
/// than its own octets: a bound that charges for what it refuses is a bound an attacker spends.
fn charge(header: &[u8], limits: Limits, meter: &mut Meter) -> Result<(), MediaTypeError> {
    let octets = u64::try_from(header.len()).unwrap_or(u64::MAX);
    if octets > u64::from(limits.grammar().max_header_bytes()) {
        return Err(MediaTypeError::TooLong);
    }
    meter
        .try_charge(octets)
        .map_err(|_exhausted| MediaTypeError::BudgetExhausted)
}

/// Whether `octet` has no business in an unfolded header field body.
///
/// Control characters other than the horizontal tab, and `DEL`. A bare `CR` or `LF` is the one
/// that matters: it means the value being read spans a field boundary, so what this module
/// reports is not what the mail declared.
const fn is_forbidden_octet(octet: u8) -> bool {
    (octet < b' ' && octet != b'\t') || octet == 0x7f
}

/// Whether `octet` may appear in an RFC 2045 section 5.1 token.
///
/// Anything US-ASCII that is not `SPACE`, not a control character, and not one of the
/// `tspecials` that structure the header.
const fn is_token_octet(octet: u8) -> bool {
    octet > b' ' && octet < 0x7f && !is_tspecial(octet)
}

/// RFC 2045 section 5.1's `tspecials`, written out rather than looked up in a slice.
const fn is_tspecial(octet: u8) -> bool {
    matches!(
        octet,
        b'(' | b')'
            | b'<'
            | b'>'
            | b'@'
            | b','
            | b';'
            | b':'
            | b'\\'
            | b'"'
            | b'/'
            | b'['
            | b']'
            | b'?'
            | b'='
    )
}

/// One `attribute=value` off the header: the attribute borrowed as written, the value owned.
///
/// The value cannot borrow because a quoted string is unescaped as it is read, so the octets a
/// caller gets back are not always octets the header contains.
type HeaderParameter<'a> = (&'a [u8], Vec<u8>);

/// A cursor over one header field body.
///
/// A cursor rather than a split on `;`, because a quoted parameter value may carry a semicolon
/// and a reader that splits first sees a parameter list nobody wrote.
#[derive(Debug)]
struct Reader<'a> {
    /// The octets being read.
    input: &'a [u8],
    /// How many of them have been consumed.
    at: usize,
}

impl<'a> Reader<'a> {
    /// A cursor at the start of `input`.
    const fn new(input: &'a [u8]) -> Self {
        Self { input, at: 0 }
    }

    /// The octet under the cursor, or `None` at the end.
    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    /// Advance one octet, saturating rather than wrapping at the end of the address space.
    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// Whether everything has been consumed.
    fn is_done(&self) -> bool {
        self.at >= self.input.len()
    }

    /// Consume the linear whitespace RFC 5322 permits between the header's parts.
    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.bump();
        }
    }

    /// Consume `wanted` if it is under the cursor, reporting whether it was.
    fn eat(&mut self, wanted: u8) -> bool {
        if self.peek() == Some(wanted) {
            self.bump();
            return true;
        }
        false
    }

    /// Consume a run of token octets, which may be empty.
    fn token(&mut self) -> &'a [u8] {
        let from = self.at;
        while self.peek().is_some_and(is_token_octet) {
            self.bump();
        }
        self.input.get(from..self.at).unwrap_or(&[])
    }

    /// Consume the leading `type/subtype`.
    fn media_type(&mut self) -> Result<Vec<u8>, MediaTypeError> {
        self.skip_space();
        let top = self.token();
        if top.is_empty() || !self.eat(b'/') {
            return Err(MediaTypeError::MalformedMediaType);
        }
        let sub = self.token();
        if sub.is_empty() {
            return Err(MediaTypeError::MalformedMediaType);
        }
        let width = top.len().saturating_add(sub.len()).saturating_add(1);
        let mut spelling = Vec::with_capacity(width);
        spelling.extend_from_slice(top);
        spelling.push(b'/');
        spelling.extend_from_slice(sub);
        Ok(spelling)
    }

    /// Consume the next `; attribute=value`, or `None` once the header is spent.
    ///
    /// A trailing `;` with nothing after it is the end of the header rather than an error:
    /// producers emit one and it declares nothing, so nothing is decided by admitting it.
    fn next_parameter(&mut self) -> Result<Option<HeaderParameter<'a>>, MediaTypeError> {
        self.skip_space();
        if self.is_done() {
            return Ok(None);
        }
        if !self.eat(b';') {
            return Err(MediaTypeError::MalformedParameter);
        }
        self.skip_space();
        if self.is_done() {
            return Ok(None);
        }
        let name = self.token();
        if name.is_empty() {
            return Err(MediaTypeError::MalformedParameter);
        }
        self.skip_space();
        if !self.eat(b'=') {
            return Err(MediaTypeError::MalformedParameter);
        }
        self.skip_space();
        Ok(Some((name, self.value()?)))
    }

    /// Consume a parameter value: a token, or a quoted string.
    fn value(&mut self) -> Result<Vec<u8>, MediaTypeError> {
        if self.eat(b'"') {
            return self.quoted();
        }
        let token = self.token();
        if token.is_empty() {
            return Err(MediaTypeError::MalformedParameter);
        }
        Ok(Vec::from(token))
    }

    /// Consume the body of a quoted string, resolving RFC 2045's backslash escapes.
    ///
    /// A string that never closes is a refusal and never a value ending where the octets did.
    fn quoted(&mut self) -> Result<Vec<u8>, MediaTypeError> {
        let mut value = Vec::new();
        while let Some(octet) = self.peek() {
            self.bump();
            match octet {
                b'"' => return Ok(value),
                b'\\' => {
                    let escaped = self.peek().ok_or(MediaTypeError::UnterminatedValue)?;
                    self.bump();
                    value.push(escaped);
                },
                _ => value.push(octet),
            }
        }
        Err(MediaTypeError::UnterminatedValue)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{ComponentKind, GrammarLimits, IgnoreDiagnostics, Instant, Limits, Meter};
    use ical_recur::OverrideRange;

    use super::{MediaTypeError, MediaTypeParams, sender_is_named};
    use crate::authorize::{AuthorizationDenied, evaluate_message};
    use crate::identity::{InstanceClock, InstanceRef, SequenceRead};
    use crate::message::ItipMessage;
    use crate::party::{Attendee, Party, PartyId};
    use crate::state::{PropertyOccurrence, ScheduledComponent};
    use crate::transition::TransitionReason;

    /// The organizer of every fixture here.
    const CHAIR: &[u8] = b"mailto:chair@example.com";

    /// The organizer's assistant, who appears only as a `SENT-BY`.
    const ASSISTANT: &[u8] = b"mailto:pa@example.com";

    /// The attendee whose reply the gate is asked about.
    const ANN: &[u8] = b"mailto:ann@example.com";

    /// The party Ann delegated to, who appears only as a `DELEGATED-TO`.
    const DELEGATE: &[u8] = b"mailto:bo@example.com";

    /// An address the invitation names nowhere.
    const STRANGER: &[u8] = b"mailto:zoe@example.net";

    /// The `UID` every fixture shares.
    const UID: &[u8] = b"4f1b-9a@example.com";

    /// The header a conforming iMIP `REPLY` arrives under.
    const REPLY_HEADER: &[u8] = b"text/calendar; method=REPLY; charset=UTF-8; component=VEVENT";

    /// One `ATTENDEE` line of a fixture, as an identity rather than as text.
    #[derive(Clone, Copy, Debug)]
    struct Person {
        /// The `CAL-ADDRESS` value.
        address: &'static [u8],
        /// The `PARTSTAT` value, absent when the line states none.
        part_stat: Option<&'static [u8]>,
        /// The `SENT-BY` parameter's value, absent when the line states none.
        sent_by: Option<&'static [u8]>,
        /// The `DELEGATED-TO` parameter's value, absent when the line states none.
        delegated_to: Option<&'static [u8]>,
    }

    impl Person {
        /// An attendee who is `address` and states nothing else.
        const fn new(address: &'static [u8]) -> Self {
            Self {
                address,
                part_stat: None,
                sent_by: None,
                delegated_to: None,
            }
        }

        /// The same attendee, answering `status`.
        const fn answering(self, status: &'static [u8]) -> Self {
            Self {
                part_stat: Some(status),
                ..self
            }
        }

        /// The same attendee, having delegated to `who`.
        const fn delegating_to(self, who: &'static [u8]) -> Self {
            Self {
                delegated_to: Some(who),
                ..self
            }
        }
    }

    /// One row of the gate table: what it is called, the message, who applies it, the answer.
    type GateCase = (
        &'static str,
        Fixture,
        &'static [u8],
        Result<TransitionReason, AuthorizationDenied>,
    );

    /// A component a test hands to the gate, standing in for an `ical_core::Component`.
    ///
    /// The structured accessors are what the gate reads; `properties` exists so that the
    /// section 3 conformance table has names to count and the diff has lines to compare. A
    /// fixture keeps the two consistent by building both from the same parts.
    #[derive(Clone, Debug, Default)]
    struct Fixture {
        /// What kind of component this is.
        kind: Option<ComponentKind>,
        /// The `METHOD` value, on the calendar rather than on a payload.
        method: Option<&'static [u8]>,
        /// The `UID` value.
        uid: Option<&'static [u8]>,
        /// What the `SEQUENCE` property was.
        sequence: SequenceRead,
        /// The `DTSTAMP`.
        dtstamp: Option<Instant>,
        /// The `RECURRENCE-ID`, absent when this is about the whole series.
        instance: Option<InstanceRef>,
        /// The `ORGANIZER`'s address.
        organizer: Option<&'static [u8]>,
        /// The `ORGANIZER`'s `SENT-BY` address.
        organizer_agent: Option<&'static [u8]>,
        /// The `ATTENDEE` list, in document order.
        attendees: Vec<Person>,
        /// Whole content lines, in document order.
        properties: Vec<Vec<u8>>,
        /// Nested components, in document order.
        children: Vec<Fixture>,
    }

    /// The property name a content line begins with.
    fn name_of(line: &[u8]) -> &[u8] {
        line.iter()
            .position(|octet| matches!(*octet, b':' | b';'))
            .and_then(|at| line.get(..at))
            .unwrap_or(line)
    }

    impl ScheduledComponent for Fixture {
        fn component_kind(&self) -> Option<ComponentKind> {
            self.kind
        }

        fn method(&self) -> Option<&[u8]> {
            self.method
        }

        fn uid(&self) -> Option<&[u8]> {
            self.uid
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
                .map(|address| Party::read(address, self.organizer_agent))
        }

        fn attendee_count(&self) -> usize {
            self.attendees.len()
        }

        fn attendee(&self, index: usize) -> Option<Attendee<'_>> {
            let person = self.attendees.get(index)?;
            let mut who = Attendee::new(Party::read(person.address, person.sent_by));
            if let Some(status) = person.part_stat {
                who = who.with_part_stat(status);
            }
            if let Some(delegate) = person.delegated_to {
                who = who.with_delegated_to(delegate);
            }
            Some(who)
        }

        fn attendee_occurrence(&self, index: usize) -> Option<PropertyOccurrence> {
            (index < self.attendees.len()).then(|| PropertyOccurrence::named(b"ATTENDEE", index))
        }

        fn property_count(&self) -> usize {
            self.properties.len()
        }

        fn property_name(&self, index: usize) -> Option<&[u8]> {
            self.property_line(index).map(name_of)
        }

        fn property_line(&self, index: usize) -> Option<&[u8]> {
            self.properties.get(index).map(Vec::as_slice)
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

    /// One content line, built from its parts so a fixture's text and its identities agree.
    fn line(parts: &[&[u8]]) -> Vec<u8> {
        let mut built = Vec::new();
        for part in parts {
            built.extend_from_slice(part);
        }
        built
    }

    /// What the recipient already holds: an invitation Ann has not answered, at `SEQUENCE:2`.
    fn held() -> Fixture {
        Fixture {
            kind: Some(ComponentKind::Event),
            uid: Some(UID),
            sequence: SequenceRead::Value(2),
            dtstamp: Some(Instant::from_unix_seconds(1_000)),
            organizer: Some(CHAIR),
            attendees: alloc::vec![Person::new(ANN)],
            properties: alloc::vec![
                line(&[b"UID:", UID]),
                line(&[b"DTSTAMP:20260810T090000Z"]),
                line(&[b"DTSTART:20260812T090000Z"]),
                line(&[b"SUMMARY:Budget review"]),
                line(&[b"ORGANIZER:", CHAIR]),
                line(&[b"ATTENDEE;PARTSTAT=NEEDS-ACTION:", ANN]),
            ],
            ..Fixture::default()
        }
    }

    /// A `REPLY` from `who`, at `sequence` and `stamp`, answering `status`.
    ///
    /// The properties are exactly RFC 5546 section 3.2.3's required rows, so a row of the table
    /// below fails on the thing it is about rather than on conformance.
    fn reply(
        who: &'static [u8],
        sequence: u32,
        stamp: i64,
        status: &'static [u8],
        instance: Option<InstanceRef>,
    ) -> Fixture {
        let mut properties = alloc::vec![
            line(&[b"UID:", UID]),
            line(&[b"DTSTAMP:20260810T120000Z"]),
            line(&[b"ORGANIZER:", CHAIR]),
            line(&[b"ATTENDEE;PARTSTAT=", status, b":", who]),
        ];
        if instance.is_some() {
            properties.push(line(&[b"RECURRENCE-ID:20260819T090000Z"]));
        }
        let payload = Fixture {
            kind: Some(ComponentKind::Event),
            uid: Some(UID),
            sequence: SequenceRead::Value(sequence),
            dtstamp: Some(Instant::from_unix_seconds(stamp)),
            instance,
            organizer: Some(CHAIR),
            attendees: alloc::vec![Person::new(who).answering(status)],
            properties,
            ..Fixture::default()
        };
        Fixture {
            kind: Some(ComponentKind::Calendar),
            method: Some(b"REPLY"),
            children: alloc::vec![payload],
            ..Fixture::default()
        }
    }

    /// An instance identity the series in [`held`] does not carry.
    fn absent_instance() -> InstanceRef {
        InstanceRef::new(
            Instant::from_unix_seconds(1_787_000_000),
            InstanceClock::Utc,
            OverrideRange::ThisOnly,
        )
    }

    /// The message `calendar` is, read under a fresh ledger.
    fn message_of(calendar: &Fixture) -> ItipMessage<'_> {
        let mut meter = Meter::new(Limits::DEFAULT);
        ItipMessage::read(
            calendar,
            Limits::DEFAULT,
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap()
    }

    /// The header `bytes` parse to, under the default policy and a fresh ledger.
    fn parse(bytes: &[u8]) -> Result<MediaTypeParams, MediaTypeError> {
        let mut meter = Meter::new(Limits::DEFAULT);
        MediaTypeParams::read(bytes, Limits::DEFAULT, &mut meter)
    }

    /// The four things RFC 6047 section 2.4 names, as one comparable value.
    type Parsed<'a> = (
        &'a [u8],
        Option<&'a [u8]>,
        Option<&'a [u8]>,
        Option<&'a [u8]>,
    );

    /// What a header was read as.
    fn summarize(params: &MediaTypeParams) -> Parsed<'_> {
        (
            params.media_type(),
            params.method(),
            params.component(),
            params.charset(),
        )
    }

    /// Headers RFC 2045 section 5.1's grammar admits, and what RFC 6047 section 2.4 reads.
    #[test]
    fn a_content_type_header_is_read_as_the_three_parameters_the_specification_names() {
        let cases: [(&str, &[u8], Parsed<'_>); 8] = [
            (
                "the canonical iMIP header",
                b"text/calendar; method=REQUEST; charset=UTF-8; component=VEVENT",
                (
                    b"text/calendar",
                    Some(b"REQUEST"),
                    Some(b"VEVENT"),
                    Some(b"UTF-8"),
                ),
            ),
            (
                "parameter names are case-insensitive and their order is free",
                b"Text/Calendar;CHARSET=utf-8;Method=reply",
                (b"Text/Calendar", Some(b"reply"), None, Some(b"utf-8")),
            ),
            (
                "a quoted value is a value",
                b"text/calendar; method=\"REQUEST\"",
                (b"text/calendar", Some(b"REQUEST"), None, None),
            ),
            (
                "and its escapes are resolved rather than kept",
                b"text/calendar; method=\"REQ\\UEST\"",
                (b"text/calendar", Some(b"REQUEST"), None, None),
            ),
            (
                "a semicolon inside a quoted value is not a parameter boundary",
                b"text/calendar; method=\"REQUEST;CANCEL\"",
                (b"text/calendar", Some(b"REQUEST;CANCEL"), None, None),
            ),
            (
                "a trailing semicolon declares nothing and decides nothing",
                b"text/calendar; method=REQUEST;",
                (b"text/calendar", Some(b"REQUEST"), None, None),
            ),
            (
                "a parameter this module does not know is skipped, not refused",
                b"text/calendar; optinfo=x; method=CANCEL",
                (b"text/calendar", Some(b"CANCEL"), None, None),
            ),
            (
                "an RFC 2231 continuation names no parameter here, so none is declared",
                b"text/calendar; method*0=RE; method*1=QUEST",
                (b"text/calendar", None, None, None),
            ),
        ];

        for (shape, header, expected) in cases {
            let observed = parse(header);
            let summary = observed.as_ref().ok().map(summarize);
            assert_eq!(summary, Some(expected), "{shape}");
        }
    }

    /// Every shape this module refuses whole, and the reason a caller is given.
    #[test]
    fn a_header_that_two_readers_would_finish_differently_is_refused_whole() {
        let cases: [(&str, &[u8], MediaTypeError); 7] = [
            (
                "a quoted value that never closes is a refusal and never a truncation",
                b"text/calendar; method=\"REQUEST",
                MediaTypeError::UnterminatedValue,
            ),
            (
                "a trailing backslash leaves the same string open",
                b"text/calendar; method=\"REQUEST\\",
                MediaTypeError::UnterminatedValue,
            ),
            (
                "two spellings of one parameter let an attacker pick which reader believes it",
                b"text/calendar; method=REQUEST; method=CANCEL",
                MediaTypeError::RepeatedParameter,
            ),
            (
                "no subtype is no media type",
                b"text; method=REQUEST",
                MediaTypeError::MalformedMediaType,
            ),
            (
                "a parameter without a value",
                b"text/calendar; method",
                MediaTypeError::MalformedParameter,
            ),
            (
                "an RFC 5322 comment is not stripped, so it is not silently read past",
                b"text/calendar; method=REQUEST (an invitation)",
                MediaTypeError::MalformedParameter,
            ),
            (
                "a bare CRLF means the caller handed over more than one field",
                b"text/calendar;\r\n method=REQUEST",
                MediaTypeError::ControlOctet,
            ),
        ];

        for (shape, header, expected) in cases {
            assert_eq!(parse(header).err(), Some(expected), "{shape}");
        }
    }

    /// ADR-0010: the header is the stranger's too, so it is bounded and it is charged.
    #[test]
    fn a_header_is_bounded_by_policy_and_charged_to_the_shared_ledger() {
        let tight = Limits::DEFAULT.with_grammar(GrammarLimits::DEFAULT.with_max_header_bytes(16));
        let header: &[u8] = b"text/calendar; method=REQUEST";
        let mut refused = Meter::new(tight);
        assert_eq!(
            MediaTypeParams::read(header, tight, &mut refused).err(),
            Some(MediaTypeError::TooLong)
        );
        assert_eq!(
            refused.spent(),
            0,
            "a bound that charges for what it refuses is one an attacker spends"
        );

        let mut ledger = Meter::new(Limits::DEFAULT);
        assert!(MediaTypeParams::read(header, Limits::DEFAULT, &mut ledger).is_ok());
        assert_eq!(ledger.spent(), 29);

        let mut spent = Meter::with_budget(Limits::DEFAULT, 4);
        assert_eq!(
            MediaTypeParams::read(header, Limits::DEFAULT, &mut spent).err(),
            Some(MediaTypeError::BudgetExhausted)
        );
    }

    /// RFC 6047 section 2.4: the envelope's claim is checked against the body and never for it.
    #[test]
    fn the_declared_method_is_compared_with_the_object_and_never_trusted_over_it() {
        let calendar = reply(ANN, 2, 2_000, b"ACCEPTED", None);
        let message = message_of(&calendar);
        let cases: [(&str, &[u8], bool); 6] = [
            (
                "the envelope declares what the body carries",
                REPLY_HEADER,
                true,
            ),
            (
                "compared as RFC 5545 section 3.1 compares an enumerated value",
                b"text/calendar; method=reply",
                true,
            ),
            (
                "quoting the value changes nothing about what it names",
                b"text/calendar; method=\"REPLY\"",
                true,
            ),
            (
                "an envelope claiming another method disagrees with the body",
                b"text/calendar; method=REQUEST",
                false,
            ),
            (
                "section 2.4 requires the parameter, so declining to state one is not agreement",
                b"text/calendar; charset=UTF-8",
                false,
            ),
            (
                "a method RFC 5546 does not define agrees with nothing",
                b"text/calendar; method=INVITE",
                false,
            ),
        ];

        for (shape, header, expected) in cases {
            let declared = parse(header).unwrap();
            assert_eq!(declared.agrees_with(&message), expected, "{shape}");
        }
        assert!(parse(REPLY_HEADER).unwrap().is_calendar());
        assert!(!parse(b"text/plain; method=REPLY").unwrap().is_calendar());
    }

    /// Presence, and nothing else. Every row is an address and whether the component names it.
    #[test]
    fn the_envelope_sender_is_only_looked_up_and_never_believed() {
        let mut current = held();
        current.organizer_agent = Some(ASSISTANT);
        current.attendees = alloc::vec![Person::new(ANN).delegating_to(DELEGATE)];
        let cases: [(&str, &[u8], bool); 5] = [
            ("the organizer", CHAIR, true),
            ("an agent the organizer's SENT-BY names", ASSISTANT, true),
            ("an attendee", ANN, true),
            (
                "a delegate the attendee list reaches only through DELEGATED-TO",
                DELEGATE,
                true,
            ),
            (
                "an address the component has never heard of",
                STRANGER,
                false,
            ),
        ];

        for (shape, address, expected) in cases {
            let who = PartyId::from_bytes(address).unwrap();
            assert_eq!(sender_is_named(who, &current), expected, "{shape}");
        }
    }

    /// The gate's own table, run with the envelope check composed in front of it.
    ///
    /// Each row is (prior state, incoming message, applying party) and the answer RFC 5546's own
    /// text gives: section 3.2.3 for what a `REPLY` is, section 2.1.4 for `SEQUENCE` ordering,
    /// section 2.1.5 for the `DTSTAMP` tie-break, section 3.7.1 for naming an instance. The
    /// envelope agrees in every row, and the answer is the one `evaluate_message` gives on its
    /// own — which is the whole of what this module claims about itself.
    #[test]
    fn composing_the_envelope_check_in_front_of_the_gate_changes_no_answer() {
        let current = held();
        let cases: [GateCase; 5] = [
            (
                "Ann accepts the invitation she was sent",
                reply(ANN, 2, 2_000, b"ACCEPTED", None),
                ANN,
                Ok(TransitionReason::ParticipationChanged),
            ),
            (
                "a reply from an address the invitation never names",
                reply(STRANGER, 2, 2_000, b"ACCEPTED", None),
                STRANGER,
                Err(AuthorizationDenied::UnknownAttendee),
            ),
            (
                "a reply answering an older invitation",
                reply(ANN, 1, 9_000, b"ACCEPTED", None),
                ANN,
                Err(AuthorizationDenied::SequenceStale { have: 2 }),
            ),
            (
                "the same revision, stamped earlier than the one already held",
                reply(ANN, 2, 900, b"ACCEPTED", None),
                ANN,
                Err(AuthorizationDenied::DtstampStale {
                    have: Instant::from_unix_seconds(1_000),
                }),
            ),
            (
                "a reply naming an instance the series does not have",
                reply(ANN, 2, 2_000, b"ACCEPTED", Some(absent_instance())),
                ANN,
                Err(AuthorizationDenied::NoMatchingInstance),
            ),
        ];

        for (shape, calendar, sender, expected) in cases {
            let message = message_of(&calendar);
            let declared = parse(REPLY_HEADER).unwrap();
            assert!(declared.agrees_with(&message), "{shape}");
            let who = PartyId::from_bytes(sender).unwrap();
            let observed =
                evaluate_message(&message, &current, who).map(|granted| granted.reason());
            assert_eq!(observed, expected, "{shape}");
        }
    }

    /// The attack this module exists to keep out of reach.
    ///
    /// The object says Ann accepted. The mail carrying it came from somebody else. Nothing in
    /// the body distinguishes the two cases below — the only difference is which address the
    /// caller supplies — so a reader that took the object's word for who sent it would accept
    /// both.
    #[test]
    fn a_body_naming_an_attendee_does_not_make_its_sender_that_attendee() {
        let current = held();
        let calendar = reply(ANN, 2, 2_000, b"ACCEPTED", None);
        let message = message_of(&calendar);
        let stranger = PartyId::from_bytes(STRANGER).unwrap();
        let ann = PartyId::from_bytes(ANN).unwrap();

        assert!(!sender_is_named(stranger, &current));
        assert_eq!(
            evaluate_message(&message, &current, stranger).map(|granted| granted.reason()),
            Err(AuthorizationDenied::UnknownAttendee)
        );

        assert!(sender_is_named(ann, &current));
        assert_eq!(
            evaluate_message(&message, &current, ann).map(|granted| granted.reason()),
            Ok(TransitionReason::ParticipationChanged),
            "and the same object from the party it names is the reply it claims to be"
        );
    }
}
