// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The message, once it is known to be one.
//!
//! Specification: RFC 5546 section 3.1.1 (one `UID` across a message's components), section 3
//! for what a `METHOD` may carry.
//!
//! [`ItipMessage`] means *already checked and already charged*: the `METHOD` is one RFC 5546
//! defines, RFC 5546 defines that method for the component type present, at least one
//! scheduling payload exists, every payload shares one `UID`, every attendee list and every
//! nesting is inside the caller's [`Limits`], and the work of establishing all of that was
//! debited from the caller's [`Meter`]. Nothing downstream re-checks any of it, which is why
//! [`ItipMessage::read`] is the only constructor.
//!
//! # Why a limit breach is an error here and a diagnostic everywhere else
//!
//! ADR-0009 routes a limit breach on an otherwise parseable value to the diagnostic channel,
//! and this module does the opposite for every one of them, deliberately. A truncated attendee
//! list is not a degraded answer, it is a *different authorization answer*: dropping the 513th
//! attendee turns "this party may reply" into "this party is unknown", and an attacker who can
//! pad a list past the threshold picks which of the two the server believes. Truncate-and-flag
//! is a safe policy for a 40 KB `DESCRIPTION` and an unsafe one for anything the authorization
//! decision reads.

use alloc::vec::Vec;

use ical_core::{
    ComponentKind, Diagnostic, DiagnosticCode, DiagnosticSink, Limits, Meter, Severity,
    report_diagnostic,
};

use crate::identity::{MessageIdentity, Uid};
use crate::method::Method;
use crate::state::ScheduledComponent;
use crate::table::MethodRule;

/// Why a stream of components is not a scheduling message.
///
/// Every variant is a refusal of the whole message. There is no partially-read `ItipMessage`,
/// because every downstream decision reads the parts a partial read would be missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MessageError {
    /// No `METHOD` property. An `.ics` without one is an ordinary calendar, not a message.
    MissingMethod,
    /// A `METHOD` naming no method RFC 5546 defines.
    UnknownMethod,
    /// No component RFC 5546 states scheduling semantics for.
    NoPayload,
    /// A payload with no `UID`.
    MissingUid,
    /// Two payloads with different `UID`s, which RFC 5546 section 3.1.1 forbids.
    MixedUids,
    /// A component this build will not reason about as a scheduling payload.
    ///
    /// A `VFREEBUSY` without the `freebusy` feature is this, refused rather than ignored: a
    /// scheduling message a build cannot reason about is not one it may accept.
    UnsupportedPayload(ComponentKind),
    /// Two payloads of different component types in one message.
    MixedPayloadKinds,
    /// RFC 5546 states no table for this method applied to this component type.
    UndefinedForComponent(ComponentKind),
    /// A payload carried more attendees than the caller's policy admits.
    TooManyAttendees,
    /// The message carried more components than the caller's policy admits.
    TooManyComponents,
    /// The message nested components deeper than the caller's policy admits.
    TooDeep,
    /// The caller's shared ledger ran out.
    BudgetExhausted,
}

/// One iTIP message: a method, one identity, and the payloads it is about.
#[derive(Debug)]
pub struct ItipMessage<'a> {
    /// The calendar the payloads were found in.
    calendar: &'a dyn ScheduledComponent,
    /// The method, already known to be one RFC 5546 defines.
    method: Method,
    /// The rules RFC 5546 section 3 states for this method and this component type.
    rule: MethodRule,
    /// The `UID` every payload agrees on.
    uid: Uid,
    /// Which children of the calendar are payloads, in document order.
    payloads: Vec<usize>,
}

impl<'a> ItipMessage<'a> {
    /// Read `calendar` as a scheduling message, or say why it is not one.
    ///
    /// The only constructor. `limits` and `meter` are ADR-0010's pair: the policy is the
    /// caller's and the ledger outlives this call, so a thousand messages read under one meter
    /// are bounded in aggregate rather than a thousand times individually.
    ///
    /// # Errors
    ///
    /// [`MessageError`], every variant of which refuses the whole message.
    pub fn read<S: DiagnosticSink + ?Sized>(
        calendar: &'a dyn ScheduledComponent,
        limits: Limits,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Result<Self, MessageError> {
        let method = read_method(calendar, meter, sink)?;
        let payloads = collect_payloads(calendar, limits, meter)?;
        let kind = payload_kind(calendar, &payloads).ok_or(MessageError::NoPayload)?;
        let rule =
            MethodRule::lookup(method, kind).ok_or(MessageError::UndefinedForComponent(kind))?;
        let uid = agreed_uid(calendar, &payloads)?;
        charge_attendees(calendar, &payloads, limits, meter)?;
        Ok(Self {
            calendar,
            method,
            rule,
            uid,
            payloads,
        })
    }

    /// The method, already known to be one RFC 5546 defines.
    #[must_use]
    pub const fn method(&self) -> Method {
        self.method
    }

    /// What RFC 5546 section 3 states for this method and this component type.
    #[must_use]
    pub const fn rule(&self) -> MethodRule {
        self.rule
    }

    /// The component type every payload is.
    #[must_use]
    pub const fn kind(&self) -> ComponentKind {
        self.rule.kind()
    }

    /// The calendar the payloads were found in.
    #[must_use]
    pub const fn calendar(&self) -> &'a dyn ScheduledComponent {
        self.calendar
    }

    /// The `UID` every payload agrees on.
    #[must_use]
    pub const fn uid(&self) -> &Uid {
        &self.uid
    }

    /// How many scheduling payloads this message carries.
    #[must_use]
    pub fn payload_count(&self) -> usize {
        self.payloads.len()
    }

    /// The `index`th payload, in document order.
    #[must_use]
    pub fn payload(&self, index: usize) -> Option<&'a dyn ScheduledComponent> {
        self.payloads
            .get(index)
            .and_then(|at| self.calendar.child(*at))
    }

    /// What the `index`th payload is about.
    #[must_use]
    pub fn payload_identity(&self, index: usize) -> Option<MessageIdentity> {
        self.payload(index)
            .map(|payload| MessageIdentity::new(self.uid.clone(), payload.recurrence_id()))
    }

    /// The payload addressing the same thing `current` does, if this message carries one.
    ///
    /// The match is [`crate::InstanceMatch::Same`] and nothing weaker: an ambiguous instance —
    /// two halves of a repeated hour that nothing told apart — answers `None` here rather than
    /// picking one, because picking is how a message about one meeting reaches another.
    #[must_use]
    pub fn payload_for(
        &self,
        current: &dyn ScheduledComponent,
    ) -> Option<&'a dyn ScheduledComponent> {
        let target = MessageIdentity::new(
            Uid::new(current.uid().unwrap_or(&[])),
            current.recurrence_id(),
        );
        (0..self.payload_count()).find_map(|index| {
            let identity = self.payload_identity(index)?;
            identity
                .matches(&target)
                .is_same()
                .then(|| self.payload(index))?
        })
    }
}

/// The method `calendar` states, reported when it states one nothing here knows.
fn read_method<S: DiagnosticSink + ?Sized>(
    calendar: &dyn ScheduledComponent,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<Method, MessageError> {
    let value = calendar.method().ok_or(MessageError::MissingMethod)?;
    Method::read(value).ok_or_else(|| {
        report_diagnostic(
            sink,
            meter,
            Diagnostic::new(
                DiagnosticCode::SchedulingMethodUnknown,
                Severity::Violation,
                ical_core::Location::NOWHERE,
            ),
        );
        MessageError::UnknownMethod
    })
}

/// Which children of `calendar` are scheduling payloads, charged and bounded.
///
/// Every child is charged, payload or not, because a message of a hundred thousand
/// `VTIMEZONE` components costs the reader the same walk either way.
fn collect_payloads(
    calendar: &dyn ScheduledComponent,
    limits: Limits,
    meter: &mut Meter,
) -> Result<Vec<usize>, MessageError> {
    let ceiling = usize::try_from(limits.max_payload_components()).unwrap_or(usize::MAX);
    if calendar.child_count() > ceiling {
        return Err(MessageError::TooManyComponents);
    }
    let mut payloads = Vec::new();
    let mut seen: Option<ComponentKind> = None;
    for index in 0..calendar.child_count() {
        meter
            .try_charge(1)
            .map_err(|_exhausted| MessageError::BudgetExhausted)?;
        let Some(child) = calendar.child(index) else {
            continue;
        };
        charge_nesting(child, limits, meter)?;
        let Some(kind) = child.component_kind() else {
            continue;
        };
        match classify(kind) {
            Payload::Ignored => continue,
            Payload::Refused => return Err(MessageError::UnsupportedPayload(kind)),
            Payload::Scheduling => {},
        }
        if seen.is_some_and(|first| first != kind) {
            return Err(MessageError::MixedPayloadKinds);
        }
        seen = Some(kind);
        payloads.push(index);
    }
    Ok(payloads)
}

/// What one top-level component of a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Payload {
    /// A component RFC 5546 states scheduling semantics for.
    Scheduling,
    /// A component that is legally there and is not a payload, such as a `VTIMEZONE`.
    Ignored,
    /// A component that has no business at the top level of a message, or that this build
    /// will not reason about.
    Refused,
}

/// What `kind` is at the top level of a message.
///
/// A `VFREEBUSY` is refused rather than ignored without the `freebusy` feature, because a
/// scheduling message a build cannot reason about is not a message it may accept.
const fn classify(kind: ComponentKind) -> Payload {
    match kind {
        ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal => Payload::Scheduling,
        #[cfg(feature = "freebusy")]
        ComponentKind::FreeBusy => Payload::Scheduling,
        #[cfg(not(feature = "freebusy"))]
        ComponentKind::FreeBusy => Payload::Refused,
        ComponentKind::TimeZone => Payload::Ignored,
        // `ComponentKind` is `#[non_exhaustive]`, so this arm covers `VCALENDAR`, the two
        // observances, `VALARM`, and any kind a later RFC adds. All of them are refused
        // rather than ignored: a component nesting where the tables say it may not is a
        // message shaped to be read two ways.
        _ => Payload::Refused,
    }
}

/// Charge every component nested under `root`, refusing a tree deeper or wider than policy.
///
/// An explicit stack rather than recursion, for the reason `ical-core`'s own emitter uses one:
/// nesting is attacker-chosen and a stack that grows with it is a crash the caller cannot
/// catch.
fn charge_nesting(
    root: &dyn ScheduledComponent,
    limits: Limits,
    meter: &mut Meter,
) -> Result<(), MessageError> {
    let deepest = usize::from(limits.max_component_depth());
    let widest = usize::try_from(limits.max_payload_components()).unwrap_or(usize::MAX);
    let mut pending: Vec<(&dyn ScheduledComponent, usize)> = alloc::vec![(root, 1)];
    let mut seen = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        if depth > deepest {
            return Err(MessageError::TooDeep);
        }
        seen = seen.saturating_add(1);
        if seen > widest {
            return Err(MessageError::TooManyComponents);
        }
        meter
            .try_charge(1)
            .map_err(|_exhausted| MessageError::BudgetExhausted)?;
        for index in 0..node.child_count() {
            if let Some(child) = node.child(index) {
                pending.push((child, depth.saturating_add(1)));
            }
        }
    }
    Ok(())
}

/// The component type every payload is, or `None` when there are none.
fn payload_kind(calendar: &dyn ScheduledComponent, payloads: &[usize]) -> Option<ComponentKind> {
    payloads
        .first()
        .and_then(|at| calendar.child(*at))
        .and_then(ScheduledComponent::component_kind)
}

/// The one `UID` every payload states, per RFC 5546 section 3.1.1.
fn agreed_uid(calendar: &dyn ScheduledComponent, payloads: &[usize]) -> Result<Uid, MessageError> {
    let mut agreed: Option<Uid> = None;
    for at in payloads {
        let payload = calendar.child(*at).ok_or(MessageError::NoPayload)?;
        let uid = Uid::new(payload.uid().ok_or(MessageError::MissingUid)?);
        match &agreed {
            Some(first) if !first.matches(&uid) => return Err(MessageError::MixedUids),
            Some(_) => {},
            None => agreed = Some(uid),
        }
    }
    agreed.ok_or(MessageError::NoPayload)
}

/// Charge every attendee of every payload, refusing a list longer than policy.
fn charge_attendees(
    calendar: &dyn ScheduledComponent,
    payloads: &[usize],
    limits: Limits,
    meter: &mut Meter,
) -> Result<(), MessageError> {
    let ceiling = usize::try_from(limits.max_attendees()).unwrap_or(usize::MAX);
    for at in payloads {
        let payload = calendar.child(*at).ok_or(MessageError::NoPayload)?;
        let count = payload.attendee_count();
        if count > ceiling {
            return Err(MessageError::TooManyAttendees);
        }
        let units = u64::try_from(count).unwrap_or(u64::MAX);
        meter
            .try_charge(units)
            .map_err(|_exhausted| MessageError::BudgetExhausted)?;
    }
    Ok(())
}
