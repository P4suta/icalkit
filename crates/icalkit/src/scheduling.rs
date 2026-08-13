// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Owned iTIP messages and a borrowed read-review-authorize-apply workflow.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter, Write as _};

use crate::internal::core::{
    CivilDateTime, Component, ContentLineReader, Diagnostic, Document, Instant, Item, Meter,
    Property, PropertyId, TextValue, UtcOffset,
};
use crate::internal::itip::{
    ComponentTarget, ItipMessage, MediaTypeParams, Method, PartyId, ScheduleTarget,
    ScheduledComponent, ScheduledView, Transition, evaluate_message, inspect_message,
};

use crate::calendar::parse_calendar;
use crate::failure::{Issue, IssueCode};
use crate::internal::query::{Budget, Match as KernelMatch, Zones, recurrence_set_contains};
use crate::time::{Timestamp, ZoneAdapter};
use crate::{Calendar, Engine, Error, ResourcePolicy, Session};

/// An owned scheduling message that has passed strict calendar and iTIP validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    calendar: Calendar,
    method: Method,
}

impl Message {
    /// Strictly read a complete inbound iTIP message.
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let engine = Engine::default();
        let mut session = engine.session();
        Self::read_in(&mut session, bytes)
    }

    /// Strictly read an iTIP message under an engine session's aggregate resource budget.
    pub fn read_in(session: &mut Session<'_>, bytes: &[u8]) -> Result<Self, Error> {
        let calendar = session.parse(bytes)?;
        Self::from_calendar_in(calendar, session.engine.policy, &mut session.meter)
    }

    /// Read a decoded RFC 6047 section 2.4 `text/calendar` MIME part.
    ///
    /// `content_type` is the unfolded field value after `Content-Type:`. `decoded_body` must
    /// already have had its Content-Transfer-Encoding removed by the mail implementation. The
    /// email envelope's `From` identity is deliberately not accepted here: pass the separately
    /// authenticated envelope sender as [`Actor`] when reviewing the returned message.
    pub fn read_imip(content_type: &[u8], decoded_body: &[u8]) -> Result<Self, Error> {
        let engine = Engine::default();
        let mut session = engine.session();
        Self::read_imip_in(&mut session, content_type, decoded_body)
    }

    /// Read a decoded RFC 6047 MIME part under one aggregate session budget.
    ///
    /// The header and decoded calendar body both charge the supplied session.
    pub fn read_imip_in(
        session: &mut Session<'_>,
        content_type: &[u8],
        decoded_body: &[u8],
    ) -> Result<Self, Error> {
        let declared = MediaTypeParams::read(
            content_type,
            session.engine.policy.limits,
            &mut session.meter,
        )
        .map_err(|_| Error::single("icalkit.scheduling.imip-content-type-invalid"))?;
        if !declared.is_calendar() {
            return Err(Error::single("icalkit.scheduling.imip-media-type"));
        }
        if !declared.charset_agrees_with(decoded_body) {
            return Err(Error::single("icalkit.scheduling.imip-charset-mismatch"));
        }
        let message = Self::read_in(session, decoded_body)?;
        if declared.declared_method() != Some(message.method) {
            return Err(Error::single("icalkit.scheduling.imip-method-mismatch"));
        }
        Ok(message)
    }

    /// Build a PUBLISH message, using the caller-supplied DTSTAMP.
    pub fn publish(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Publish, dtstamp)
    }

    /// Build a REQUEST message, using the caller-supplied DTSTAMP.
    pub fn request(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Request, dtstamp)
    }

    /// Build a REPLY message, using the caller-supplied DTSTAMP.
    pub fn reply(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Reply, dtstamp)
    }

    /// Build an ADD message, using the caller-supplied DTSTAMP.
    pub fn add(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Add, dtstamp)
    }

    /// Build a CANCEL message, using the caller-supplied DTSTAMP.
    pub fn cancel(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Cancel, dtstamp)
    }

    /// Build a REFRESH message, using the caller-supplied DTSTAMP.
    pub fn refresh(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Refresh, dtstamp)
    }

    /// Build a COUNTER message, using the caller-supplied DTSTAMP.
    pub fn counter(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Counter, dtstamp)
    }

    /// Build a DECLINECOUNTER message, using the caller-supplied DTSTAMP.
    pub fn decline_counter(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::DeclineCounter, dtstamp)
    }

    /// The RFC 5546 method spelling.
    #[must_use]
    pub fn method(&self) -> &'static str {
        core::str::from_utf8(self.method.as_bytes()).unwrap_or("")
    }

    /// A canonical RFC 6047 `Content-Type` value for this message.
    ///
    /// UTF-8 is stated even for an ASCII-only body, and `method` is derived from the validated
    /// iTIP object so the emitted envelope cannot disagree with it.
    #[must_use]
    pub fn imip_content_type(&self) -> String {
        let mut value = String::from("text/calendar; charset=UTF-8; method=");
        value.push_str(self.method());
        value
    }

    /// The validated calendar carrying this message.
    #[must_use]
    pub const fn as_calendar(&self) -> &Calendar {
        &self.calendar
    }

    /// Serialize the losslessly stored message.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.calendar.to_bytes()
    }

    /// Review this message against a held calendar on behalf of one actor.
    pub fn review<'a>(
        &'a self,
        current: &'a Calendar,
        actor: &Actor,
    ) -> Result<Review<'a>, Rejection> {
        let engine = Engine::builder()
            .resource_policy(self.calendar.policy)
            .build();
        let mut session = engine.session();
        self.review_in(&mut session, current, actor)
    }

    /// Review under an engine session, sharing its aggregate budget and zone database.
    pub fn review_in<'a>(
        &'a self,
        session: &mut Session<'_>,
        current: &'a Calendar,
        actor: &Actor,
    ) -> Result<Review<'a>, Rejection> {
        let message_component = root_component(&self.calendar)
            .ok_or_else(|| Rejection::new("icalkit.scheduling.message-invalid"))?;
        let message_view = ScheduledView::of(message_component);
        session.recurrence_diagnostics.clear();
        let message = ItipMessage::read(
            &message_view,
            session.engine.policy.limits,
            &mut session.meter,
            &mut session.recurrence_diagnostics,
        )
        .map_err(|_| Rejection::new("icalkit.scheduling.message-invalid"))?;
        let uid = message.uid().as_bytes();
        let (item, current_component) = event_component_for_message(current, uid, &message)
            .ok_or_else(|| Rejection::new("icalkit.scheduling.no-matching-current"))?;
        let current_view = ScheduledView::of(current_component);
        let authorization = evaluate_message(&message, &current_view, PartyId::new(actor.as_str()))
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
        let split = current_view.recurrence_id().is_none()
            && authorization
                .identity()
                .instance()
                .is_some_and(crate::internal::itip::InstanceRef::is_this_and_future);
        let target = if split {
            let anchor = range_anchor_component(&self.calendar, uid)
                .ok_or_else(|| Rejection::new("icalkit.scheduling.authorization-denied"))?;
            let source = ZoneAdapter::new(session.engine.zone_database());
            let zones = Zones::new(&source);
            let mut budget = Budget::new(session.engine.policy.limits, &mut session.meter);
            let membership = recurrence_set_contains(
                current_component,
                anchor,
                zones,
                &mut budget,
                &mut session.recurrence_diagnostics,
            )
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
            if membership != KernelMatch::Matched {
                return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
            }
            ChangeTarget::SplitAfter { master: item }
        } else {
            ChangeTarget::Existing { item }
        };
        let transition = authorization.into_transition();
        Ok(Review {
            message: self,
            current,
            target,
            transition,
        })
    }

    fn outbound(payload: &[u8], method: Method, dtstamp: Timestamp) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        let mut meter = Meter::new(policy.limits);
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut reader = ContentLineReader::new(payload, policy.limits.grammar());
        let mut document = Document::from_tokens(&mut reader, &mut meter, &mut diagnostics)
            .map_err(|_| Error::single("icalkit.scheduling.payload-resource-limit"))?;
        let stamp = format_dtstamp(dtstamp)?;
        let calendar = document
            .components_mut()
            .next()
            .ok_or_else(|| Error::single("icalkit.scheduling.payload-shape"))?;
        set_property(calendar, &PropertyId::METHOD, b"METHOD", method.as_bytes())?;
        let mut payloads = 0usize;
        for item in calendar.items_mut() {
            let Some(component) = item.as_component_mut() else {
                continue;
            };
            if !is_scheduling_payload(component) {
                continue;
            }
            set_property(component, &PropertyId::DTSTAMP, b"DTSTAMP", &stamp)?;
            payloads = payloads.saturating_add(1);
        }
        if payloads == 0 {
            return Err(Error::single("icalkit.scheduling.payload-shape"));
        }
        let bytes = document.to_bytes();
        let mut validation_meter = Meter::new(policy.limits);
        let calendar = parse_calendar(&bytes, policy, &mut validation_meter)?;
        Self::from_calendar(calendar)
    }

    fn from_calendar(calendar: Calendar) -> Result<Self, Error> {
        let policy = calendar.policy;
        let mut meter = Meter::new(policy.limits);
        Self::from_calendar_in(calendar, policy, &mut meter)
    }

    fn from_calendar_in(
        calendar: Calendar,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<Self, Error> {
        let method = {
            let component = root_component(&calendar)
                .ok_or_else(|| Error::single("icalkit.scheduling.message-invalid"))?;
            let view = ScheduledView::of(component);
            let mut diagnostics: Vec<Diagnostic> = Vec::new();
            let message = ItipMessage::read(&view, policy.limits, meter, &mut diagnostics)
                .map_err(|_| Error::single("icalkit.scheduling.message-invalid"))?;
            inspect_message(&message, None, meter, &mut diagnostics);
            let issues: Vec<Issue> = diagnostics
                .into_iter()
                .map(Issue::from_diagnostic)
                .collect();
            if issues
                .iter()
                .any(|issue| issue.is_error() || issue.is_warning())
            {
                return Err(Error::new("icalkit.scheduling.message-invalid", issues));
            }
            message.method()
        };
        Ok(Self { calendar, method })
    }
}

/// A scheduling identity supplied by the application.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Actor {
    address: String,
}

impl Actor {
    /// Construct a non-empty CAL-ADDRESS without control or whitespace characters.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Option<Self> {
        let address = address.into();
        (!address.is_empty()
            && !address
                .bytes()
                .any(|octet| octet.is_ascii_control() || octet.is_ascii_whitespace()))
        .then_some(Self { address })
    }

    /// The CAL-ADDRESS spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.address
    }
}

/// An inert review that still borrows the message and state it describes.
#[derive(Debug)]
pub struct Review<'a> {
    message: &'a Message,
    current: &'a Calendar,
    target: ChangeTarget,
    transition: Transition,
}

impl<'a> Review<'a> {
    /// How many property occurrences would change.
    #[must_use]
    pub fn change_count(&self) -> usize {
        self.transition.len()
    }

    /// The message this review describes.
    #[must_use]
    pub const fn message(&self) -> &Message {
        self.message
    }

    /// Turn this review into a single-use authorization capability.
    #[must_use]
    pub fn authorize(self) -> AuthorizedChange<'a> {
        AuthorizedChange {
            message: self.message,
            current: self.current,
            target: self.target,
            transition: self.transition,
        }
    }
}

/// A single-use authorized transition borrowing the exact inputs that were reviewed.
#[derive(Debug)]
pub struct AuthorizedChange<'a> {
    message: &'a Message,
    current: &'a Calendar,
    target: ChangeTarget,
    transition: Transition,
}

impl AuthorizedChange<'_> {
    /// The message whose authorization this capability carries.
    #[must_use]
    pub const fn message(&self) -> &Message {
        self.message
    }

    /// Apply to a private clone of the reviewed state, consuming this authorization.
    pub fn apply(self) -> Result<Calendar, Error> {
        let mut updated = self.current.clone();
        let policy = updated.policy;
        {
            let calendar = updated
                .document
                .components_mut()
                .next()
                .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
            match self.target {
                ChangeTarget::Existing { item } => {
                    let event = calendar
                        .items_mut()
                        .get_mut(item)
                        .and_then(Item::as_component_mut)
                        .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
                    apply_to(event, &self.transition, policy)?;
                },
                ChangeTarget::SplitAfter { master } => {
                    let mut detached = calendar
                        .items()
                        .get(master)
                        .and_then(Item::as_component)
                        .cloned()
                        .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
                    apply_to(&mut detached, &self.transition, policy)?;
                    calendar
                        .items_mut()
                        .insert(master.saturating_add(1), Item::Component(detached));
                },
            }
        }
        let bytes = updated.document.to_bytes();
        let mut meter = Meter::new(policy.limits);
        let validated = parse_calendar(&bytes, policy, &mut meter)?;
        Ok(validated)
    }
}

/// Which component an authorized transition writes, including a not-yet-materialized range
/// anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangeTarget {
    /// Mutate the component that was reviewed.
    Existing { item: usize },
    /// Clone the reviewed master, apply the property diff, and insert the detached anchor.
    SplitAfter { master: usize },
}

/// Apply every occurrence-addressed change to one private component clone.
fn apply_to(
    event: &mut Component,
    transition: &Transition,
    policy: ResourcePolicy,
) -> Result<(), Error> {
    let mut target = ComponentTarget::new(event, policy.limits);
    for (at, change) in transition.changes() {
        target
            .write_change(at, change)
            .map_err(|_| Error::single("icalkit.scheduling.apply-refused"))?;
    }
    Ok(())
}

/// A scheduling message that failed review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    code: IssueCode,
}

impl Rejection {
    const fn new(code: &'static str) -> Self {
        Self {
            code: IssueCode::new(code),
        }
    }

    /// Stable machine-readable rejection code.
    #[must_use]
    pub const fn code(&self) -> IssueCode {
        self.code
    }
}

impl Display for Rejection {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.code.fmt(formatter)
    }
}

impl core::error::Error for Rejection {}

fn root_component(calendar: &Calendar) -> Option<&Component> {
    calendar.document.components().next()
}

fn event_component_for_message<'a>(
    calendar: &'a Calendar,
    uid: &[u8],
    message: &ItipMessage<'_>,
) -> Option<(usize, &'a Component)> {
    let events = root_component(calendar)?
        .items()
        .iter()
        .enumerate()
        .filter_map(|(item, entry)| entry.as_component().map(|component| (item, component)))
        .filter(|(_, component)| {
            component.is_named(b"VEVENT")
                && component
                    .properties()
                    .find(|property| property.is_named(b"UID"))
                    .is_some_and(|property| property.value_text().as_bytes() == uid)
        });
    let mut master = None;
    for (item, component) in events {
        let view = ScheduledView::of(component);
        if message.payload_for(&view).is_some() {
            return Some((item, component));
        }
        if view.recurrence_id().is_none() {
            master = Some((item, component));
        }
    }
    master
}

fn range_anchor_component<'a>(calendar: &'a Calendar, uid: &[u8]) -> Option<&'a Component> {
    root_component(calendar)?
        .items()
        .iter()
        .filter_map(Item::as_component)
        .filter(|component| {
            component.is_named(b"VEVENT")
                && component
                    .properties()
                    .find(|property| property.is_named(b"UID"))
                    .is_some_and(|property| property.value_text().as_bytes() == uid)
        })
        .find(|component| {
            ScheduledView::of(component)
                .recurrence_id()
                .is_some_and(crate::internal::itip::InstanceRef::is_this_and_future)
        })
}

fn is_scheduling_payload(component: &Component) -> bool {
    [b"VEVENT".as_slice(), b"VTODO", b"VJOURNAL", b"VFREEBUSY"]
        .iter()
        .any(|name| component.is_named(name))
}

fn set_property(
    component: &mut Component,
    id: &PropertyId,
    name: &[u8],
    value: &[u8],
) -> Result<(), Error> {
    if let Some(mut property) = component.get_mut::<TextValue<'_>>(id) {
        property
            .set_raw(value)
            .map_err(|_| Error::single("icalkit.scheduling.value-not-representable"))?;
        return Ok(());
    }
    let property = Property::create(name, Vec::new(), value)
        .map_err(|_| Error::single("icalkit.scheduling.value-not-representable"))?;
    let at = component
        .items()
        .iter()
        .position(|item| item.as_component().is_some())
        .unwrap_or(component.items().len());
    component.items_mut().insert(at, Item::Property(property));
    Ok(())
}

fn format_dtstamp(timestamp: Timestamp) -> Result<Vec<u8>, Error> {
    if timestamp.subsec_nanosecond() != 0 {
        return Err(Error::single("icalkit.scheduling.fractional-dtstamp"));
    }
    let instant = Instant::from_unix_seconds(timestamp.as_second());
    let value = CivilDateTime::from_instant(instant, UtcOffset::UTC)
        .ok_or_else(|| Error::single("icalkit.scheduling.dtstamp-out-of-range"))?;
    let date = value.date();
    let time = value.time();
    let mut written = String::new();
    write!(
        written,
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
    .map_err(|_| Error::single("icalkit.scheduling.dtstamp-out-of-range"))?;
    Ok(written.into_bytes())
}
