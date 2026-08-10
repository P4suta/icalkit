// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Who a message is from, and who a component says its parties are.
//!
//! Specification: RFC 5545 section 3.3.3 (`CAL-ADDRESS`), section 3.8.4.1 (`ATTENDEE`),
//! section 3.8.4.3 (`ORGANIZER`), section 3.2.18 (`SENT-BY`), section 3.2.12 (`PARTSTAT`),
//! section 3.2.16 (`ROLE`), sections 3.2.4 and 3.2.5 (`DELEGATED-FROM`, `DELEGATED-TO`); RFC
//! 5546 section 1.3 for what the two roles mean in an exchange, and section 2.1.2 for
//! delegation.
//!
//! # Values, not spellings
//!
//! Every octet slice these types hand back is a **value**: RFC 6868's caret encoding is
//! already resolved, so `^'` has become `"` and `^^` has become `^`. That is the contract
//! `docs/adr/0001` amendment 3 states from the other end —
//! [`Parameter::create`](ical_core::Parameter::create) and
//! [`ParameterEdit`](ical_core::ParameterEdit) take a value and pick its spelling — and this
//! crate is the one that moves parameters between properties, so the two doors have to agree
//! about which side of the codec they are on. A reader building these types calls
//! [`decode_caret`](ical_core::decode_caret); a writer handing one of these values to
//! `ParameterEdit::set` does not encode it again. Encoding twice writes `^^'` where the file
//! had `^'`, and no gate in this workspace catches that.

use crate::method::ActorRole;

/// A calendar address, as an identity rather than as text.
///
/// Holds valid UTF-8 by construction. A `CAL-ADDRESS` that does not decode is not a
/// `PartyId`, and therefore matches nobody — the conservative direction, since the
/// alternative is an address that compares equal to something it is not. That state is
/// [`DiagnosticCode::SchedulingCalendarAddressUnreadable`], and it is a different fact from an
/// absent property.
///
/// [`DiagnosticCode::SchedulingCalendarAddressUnreadable`]: ical_core::DiagnosticCode::SchedulingCalendarAddressUnreadable
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartyId<'a>(&'a str);

impl<'a> PartyId<'a> {
    /// The identity `address` spells.
    #[must_use]
    pub const fn new(address: &'a str) -> Self {
        Self(address)
    }

    /// The identity `address` spells, or `None` when those octets are not UTF-8.
    #[must_use]
    pub fn from_bytes(address: &'a [u8]) -> Option<Self> {
        core::str::from_utf8(address).ok().map(Self)
    }

    /// The address as it was written.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.0
    }

    /// The address as octets.
    #[must_use]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }

    /// Whether this address and `other` name the same calendar user.
    ///
    /// For a `mailto:` URI: the scheme and the domain are folded, and the local part is
    /// compared exactly, per RFC 5321 section 2.4. The blanket ASCII fold every naive
    /// implementation reaches for would merge `J.Doe@example.com` with `j.doe@example.com`,
    /// which is the receiving host's decision to make and not this library's — and merging two
    /// identities is the direction that ends with a reply accepted from the wrong person.
    ///
    /// Every other scheme is compared octet for octet, because nothing here knows which parts
    /// of a `urn:` or an `http:` calendar address are case-insensitive.
    #[must_use]
    pub fn matches(self, other: PartyId<'_>) -> bool {
        match (mail_parts(self.0), mail_parts(other.0)) {
            (Some((mine, my_host)), Some((theirs, their_host))) => {
                mine == theirs && my_host.eq_ignore_ascii_case(their_host)
            },
            _ => self.0 == other.0,
        }
    }
}

/// The local part and the domain of a `mailto:` address, or `None` for anything else.
///
/// The last `@` separates them, because RFC 5321 section 4.1.2 lets a quoted local part carry
/// one and the domain may not.
fn mail_parts(address: &str) -> Option<(&str, &str)> {
    let rest = address
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))?;
    let body = address.get(rest.len()..)?;
    body.rsplit_once('@')
}

/// One `ORGANIZER` or `ATTENDEE` line, read as an identity.
///
/// `raw` is the value's octets as the file has them, kept so that an address that did not
/// decode is still something a caller can show a person. `address` is the identity, absent
/// exactly when the octets were not UTF-8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Party<'a> {
    /// The `CAL-ADDRESS` value, as written.
    raw: &'a [u8],
    /// The identity, absent when the value did not decode.
    address: Option<PartyId<'a>>,
    /// The `SENT-BY` parameter's identity, absent when there is none or it did not decode.
    sent_by: Option<PartyId<'a>>,
}

impl<'a> Party<'a> {
    /// The party `address` names, with `sent_by` from its `SENT-BY` parameter.
    ///
    /// Both are decoded here rather than by the caller, so that "did not decode" is one
    /// answer arrived at one way.
    #[must_use]
    pub fn read(address: &'a [u8], sent_by: Option<&'a [u8]>) -> Self {
        Self {
            raw: address,
            address: PartyId::from_bytes(address),
            sent_by: sent_by.and_then(PartyId::from_bytes),
        }
    }

    /// The `CAL-ADDRESS` value as written.
    #[must_use]
    pub const fn raw(self) -> &'a [u8] {
        self.raw
    }

    /// The identity, absent when the value did not decode.
    #[must_use]
    pub const fn address(self) -> Option<PartyId<'a>> {
        self.address
    }

    /// The `SENT-BY` identity, absent when there is none.
    #[must_use]
    pub const fn sent_by(self) -> Option<PartyId<'a>> {
        self.sent_by
    }

    /// Whether the value decoded at all.
    ///
    /// The present-and-unusable state `docs/adr/0009` amendment 1 names: RFC 5545 section
    /// 3.6's audit counts the property as present and this says it identifies nobody.
    #[must_use]
    pub const fn is_readable(self) -> bool {
        self.address.is_some()
    }

    /// Whether `who` is this party.
    #[must_use]
    pub fn is(self, who: PartyId<'_>) -> bool {
        self.address.is_some_and(|mine| mine.matches(who))
    }

    /// Whether `who` is sending on this party's behalf.
    ///
    /// Answered separately from [`Party::is`] so that "the assistant sent this" never becomes
    /// "the organizer sent this". A message may satisfy both, and the two facts stay apart.
    #[must_use]
    pub fn is_agent_of(self, who: PartyId<'_>) -> bool {
        self.sent_by.is_some_and(|agent| agent.matches(who))
    }

    /// What `who` is to this party, or `None` when they are neither it nor its agent.
    ///
    /// `principal` and `agent` are the two roles to answer with, so one function serves the
    /// organizer line and an attendee line without either caller re-deriving the pairing.
    #[must_use]
    pub fn role_of(
        self,
        who: PartyId<'_>,
        principal: ActorRole,
        agent: ActorRole,
    ) -> Option<ActorRole> {
        if self.is(who) {
            Some(principal)
        } else if self.is_agent_of(who) {
            Some(agent)
        } else {
            None
        }
    }
}

/// RFC 5545 section 3.2.12's participation status.
///
/// `Other` keeps a value this crate does not interpret reachable rather than losing it:
/// section 3.2.12 registers more values for `VTODO` than for `VEVENT` and lets an
/// implementation define its own, and [`Attendee::part_stat_text`] is what it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PartStat {
    /// `NEEDS-ACTION`, which is also the value an absent `PARTSTAT` means.
    #[default]
    NeedsAction,
    /// `ACCEPTED`.
    Accepted,
    /// `DECLINED`.
    Declined,
    /// `TENTATIVE`.
    Tentative,
    /// `DELEGATED`.
    Delegated,
    /// A value RFC 5545 section 3.2.12 registers for another component, or a private one.
    Other,
}

impl PartStat {
    /// The status `value` names, [`PartStat::Other`] for anything else.
    #[must_use]
    pub fn read(value: &[u8]) -> Self {
        for (spelling, status) in [
            (&b"NEEDS-ACTION"[..], Self::NeedsAction),
            (b"ACCEPTED", Self::Accepted),
            (b"DECLINED", Self::Declined),
            (b"TENTATIVE", Self::Tentative),
            (b"DELEGATED", Self::Delegated),
        ] {
            if spelling.eq_ignore_ascii_case(value) {
                return status;
            }
        }
        Self::Other
    }

    /// Whether this status answers an invitation at all.
    #[must_use]
    pub const fn is_answered(self) -> bool {
        !matches!(self, Self::NeedsAction)
    }
}

/// RFC 5545 section 3.2.16's participation role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Role {
    /// `CHAIR`.
    Chair,
    /// `REQ-PARTICIPANT`, which is also the value an absent `ROLE` means.
    #[default]
    RequiredParticipant,
    /// `OPT-PARTICIPANT`.
    OptionalParticipant,
    /// `NON-PARTICIPANT`.
    NonParticipant,
    /// A value section 3.2.16 does not register.
    Other,
}

impl Role {
    /// The role `value` names, [`Role::Other`] for anything else.
    #[must_use]
    pub fn read(value: &[u8]) -> Self {
        for (spelling, role) in [
            (&b"CHAIR"[..], Self::Chair),
            (b"REQ-PARTICIPANT", Self::RequiredParticipant),
            (b"OPT-PARTICIPANT", Self::OptionalParticipant),
            (b"NON-PARTICIPANT", Self::NonParticipant),
        ] {
            if spelling.eq_ignore_ascii_case(value) {
                return role;
            }
        }
        Self::Other
    }
}

/// One `ATTENDEE` line: who, how they answered, and who they delegated to or from.
///
/// Built by a builder rather than by one constructor of six arguments, because most lines
/// state none of the optional parameters and a positional `None` five times over is a call
/// nobody can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Attendee<'a> {
    /// The address and its agent.
    party: Party<'a>,
    /// The `PARTSTAT`, defaulted to `NEEDS-ACTION` as section 3.2.12 requires.
    part_stat: PartStat,
    /// The `PARTSTAT` value as written, for a status this crate does not interpret.
    part_stat_text: Option<&'a [u8]>,
    /// The `ROLE`, defaulted to `REQ-PARTICIPANT` as section 3.2.16 requires.
    role: Role,
    /// The `DELEGATED-FROM` identity, if there is one.
    delegated_from: Option<PartyId<'a>>,
    /// The `DELEGATED-TO` identity, if there is one.
    delegated_to: Option<PartyId<'a>>,
}

impl<'a> Attendee<'a> {
    /// An attendee who is `party` and states nothing else.
    #[must_use]
    pub const fn new(party: Party<'a>) -> Self {
        Self {
            party,
            part_stat: PartStat::NeedsAction,
            part_stat_text: None,
            role: Role::RequiredParticipant,
            delegated_from: None,
            delegated_to: None,
        }
    }

    /// The same attendee with the `PARTSTAT` `value` states.
    #[must_use]
    pub fn with_part_stat(self, value: &'a [u8]) -> Self {
        Self {
            part_stat: PartStat::read(value),
            part_stat_text: Some(value),
            ..self
        }
    }

    /// The same attendee with the `ROLE` `value` states.
    #[must_use]
    pub fn with_role(self, value: &'a [u8]) -> Self {
        Self {
            role: Role::read(value),
            ..self
        }
    }

    /// The same attendee with a `DELEGATED-FROM` identity.
    #[must_use]
    pub fn with_delegated_from(self, who: &'a [u8]) -> Self {
        Self {
            delegated_from: PartyId::from_bytes(who),
            ..self
        }
    }

    /// The same attendee with a `DELEGATED-TO` identity.
    #[must_use]
    pub fn with_delegated_to(self, who: &'a [u8]) -> Self {
        Self {
            delegated_to: PartyId::from_bytes(who),
            ..self
        }
    }

    /// The address and its agent.
    #[must_use]
    pub const fn party(self) -> Party<'a> {
        self.party
    }

    /// How this attendee answered.
    #[must_use]
    pub const fn part_stat(self) -> PartStat {
        self.part_stat
    }

    /// The `PARTSTAT` value as written, absent when the line stated none.
    ///
    /// A value, not a spelling: see this module's own documentation.
    #[must_use]
    pub const fn part_stat_text(self) -> Option<&'a [u8]> {
        self.part_stat_text
    }

    /// This attendee's role.
    #[must_use]
    pub const fn role(self) -> Role {
        self.role
    }

    /// Who delegated to this attendee, if anybody.
    #[must_use]
    pub const fn delegated_from(self) -> Option<PartyId<'a>> {
        self.delegated_from
    }

    /// Who this attendee delegated to, if anybody.
    #[must_use]
    pub const fn delegated_to(self) -> Option<PartyId<'a>> {
        self.delegated_to
    }

    /// Whether `who` is this attendee, their agent, or the party they delegated to.
    #[must_use]
    pub fn role_of(self, who: PartyId<'_>) -> Option<ActorRole> {
        self.party
            .role_of(who, ActorRole::Attendee, ActorRole::AttendeeAgent)
            .or_else(|| {
                self.delegated_to
                    .filter(|delegate| delegate.matches(who))
                    .map(|_| ActorRole::Delegate)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Attendee, PartStat, Party, PartyId, Role};
    use crate::method::ActorRole;

    /// RFC 5321 section 2.4, in both directions: the host is the receiver's to case-fold and
    /// the mailbox is not ours to.
    #[test]
    fn a_mail_address_folds_its_host_and_keeps_its_mailbox() {
        let doe = PartyId::new("mailto:J.Doe@Example.COM");
        assert!(doe.matches(PartyId::new("MAILTO:J.Doe@example.com")));
        assert!(
            !doe.matches(PartyId::new("mailto:j.doe@example.com")),
            "merging two mailboxes is how a reply is accepted from the wrong person"
        );
        assert!(!doe.matches(PartyId::new("mailto:J.Doe@example.org")));
    }

    /// Anything that is not a `mailto:` URI is compared as octets, because nothing here knows
    /// which halves of another scheme are case-insensitive.
    #[test]
    fn another_scheme_is_compared_exactly() {
        let urn = PartyId::new("urn:uuid:4f1b-9a");
        assert!(urn.matches(PartyId::new("urn:uuid:4f1b-9a")));
        assert!(!urn.matches(PartyId::new("URN:uuid:4f1b-9a")));
        assert_eq!(PartyId::from_bytes(&[0xff, 0xfe]), None);
    }

    /// An address that did not decode is present and identifies nobody, which is two facts
    /// rather than one.
    #[test]
    fn an_address_that_did_not_decode_matches_nobody_and_is_still_shown() {
        let broken = Party::read(&[0x6d, 0xff], None);
        assert!(!broken.is_readable());
        assert_eq!(broken.raw(), &[0x6d, 0xff]);
        assert!(!broken.is(PartyId::new("mailto:ann@example.com")));
        assert_eq!(
            broken.role_of(
                PartyId::new("mailto:ann@example.com"),
                ActorRole::Organizer,
                ActorRole::OrganizerAgent
            ),
            None
        );
    }

    /// `SENT-BY` is a second identity and never the first one.
    #[test]
    fn an_agent_is_reported_as_an_agent_and_not_as_the_party() {
        let organizer = Party::read(b"mailto:chair@example.com", Some(b"mailto:pa@example.com"));
        let assistant = PartyId::new("mailto:pa@example.com");
        assert!(!organizer.is(assistant));
        assert!(organizer.is_agent_of(assistant));
        assert_eq!(
            organizer.role_of(assistant, ActorRole::Organizer, ActorRole::OrganizerAgent),
            Some(ActorRole::OrganizerAgent)
        );
    }

    /// The defaults RFC 5545 sections 3.2.12 and 3.2.16 state, and a value we do not
    /// interpret surviving as the octets it was.
    #[test]
    fn an_attendee_defaults_the_way_the_specification_does_and_loses_no_value() {
        let plain = Attendee::new(Party::read(b"mailto:ann@example.com", None));
        assert_eq!(plain.part_stat(), PartStat::NeedsAction);
        assert_eq!(plain.role(), Role::RequiredParticipant);
        assert!(!plain.part_stat().is_answered());
        assert_eq!(plain.part_stat_text(), None);

        let odd = plain.with_part_stat(b"IN-PROCESS").with_role(b"CHAIR");
        assert_eq!(odd.part_stat(), PartStat::Other);
        assert_eq!(odd.part_stat_text(), Some(&b"IN-PROCESS"[..]));
        assert_eq!(odd.role(), Role::Chair);
        assert!(odd.part_stat().is_answered());
    }

    /// A delegate is on the list only as somebody else's `DELEGATED-TO` until the delegation
    /// has been applied, and RFC 5546 section 2.1.2's reply arrives in that window.
    #[test]
    fn a_delegate_is_reachable_through_the_attendee_who_delegated() {
        let attendee = Attendee::new(Party::read(b"mailto:bo@example.com", None))
            .with_part_stat(b"DELEGATED")
            .with_delegated_to(b"mailto:cy@example.com");
        assert_eq!(
            attendee.role_of(PartyId::new("mailto:cy@example.com")),
            Some(ActorRole::Delegate)
        );
        assert_eq!(
            attendee.role_of(PartyId::new("mailto:bo@example.com")),
            Some(ActorRole::Attendee)
        );
        assert_eq!(
            attendee.role_of(PartyId::new("mailto:zz@example.com")),
            None
        );
        assert_eq!(attendee.delegated_from(), None);
    }
}
