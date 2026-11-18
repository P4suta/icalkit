// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Owned iTIP messages and single-use authorization for reviewed changes.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter, Write as _};
use core::str;

use crate::internal::core::{
    CivilDateTime, Component, ContentLineReader, DateTimeValue, DecodeValue as _, Diagnostic,
    Document, Instant, Item, LimitExceeded, Meter, Property, PropertyId, TextValue, UriValue,
    UtcOffset,
};
use crate::internal::itip::{
    ComponentTarget, InstanceMatch, InstanceRef, ItipMessage, MediaTypeParams, Method, PartyId,
    ScheduleTarget, ScheduledComponent, ScheduledView, Transition, evaluate_initial_payload,
    evaluate_message, evaluate_new_instance_payload, inspect_message,
};

use crate::calendar::parse_calendar;
use crate::failure::{Issue, IssueCode};
use crate::internal::core::codec::decode_single_scalar_text;
use crate::internal::query::{
    Budget, Match as KernelMatch, ReplyOccurrence, Zones, event_for_instance_reply,
    recurrence_set_contains,
};
use crate::internal::tz::FixedOffsetSource;
use crate::time::{ResourceZones, Timestamp};
use crate::{Calendar, Engine, Error, ResourcePolicy, Session};

/// An owned scheduling message that has passed strict calendar and iTIP validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    calendar: Calendar,
    method: Method,
}

#[derive(Debug)]
pub(crate) struct BusyTimeEnvelope<'a> {
    pub(crate) uid: Cow<'a, str>,
    pub(crate) start: CivilDateTime,
    pub(crate) end: CivilDateTime,
    pub(crate) originator: &'a str,
    pub(crate) recipients: Vec<&'a str>,
}

fn calendar_address(property: &Property) -> Option<&str> {
    let uri = property.value::<UriValue<'_>>().value()?;
    str::from_utf8(uri.as_bytes()).ok()
}

impl Message {
    pub(crate) fn busy_time_envelope(&self) -> Option<BusyTimeEnvelope<'_>> {
        self.busy_time_envelope_for(Method::Request)
    }

    pub(crate) fn busy_time_reply_envelope(&self) -> Option<BusyTimeEnvelope<'_>> {
        self.busy_time_envelope_for(Method::Reply)
    }

    fn busy_time_envelope_for(&self, method: Method) -> Option<BusyTimeEnvelope<'_>> {
        if self.method != method {
            return None;
        }
        let mut payloads = root_component(&self.calendar)?
            .components()
            .filter(|component| is_scheduling_payload(component));
        let payload = payloads.next()?;
        if !payload.is_named(b"VFREEBUSY") || payloads.next().is_some() {
            return None;
        }
        let uid = decode_single_scalar_text(payload, &PropertyId::UID)?;
        let utc = |id: &PropertyId| -> Option<CivilDateTime> {
            let property = payload.properties().find(|property| property.has_id(id))?;
            match property.value::<DateTimeValue<'_>>().value()? {
                DateTimeValue::Utc(stamp) => Some(stamp),
                _ => None,
            }
        };
        utc(&PropertyId::DTSTAMP)?;
        let start = utc(&PropertyId::DTSTART)?;
        let end = utc(&PropertyId::DTEND)?;
        if end <= start {
            return None;
        }
        let organizer = payload
            .properties()
            .find(|property| property.has_id(&PropertyId::ORGANIZER))?;
        let originator = calendar_address(organizer)?;
        let mut recipients = Vec::new();
        for property in payload
            .properties()
            .filter(|property| property.has_id(&PropertyId::ATTENDEE))
        {
            let address = calendar_address(property)?;
            if !recipients.contains(&address) {
                recipients.push(address);
            }
        }
        (!recipients.is_empty()).then_some(BusyTimeEnvelope {
            uid,
            start,
            end,
            originator,
            recipients,
        })
    }

    /// Read and validate a complete inbound iTIP message.
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let engine = Engine::default();
        let mut session = engine.session();
        Self::read_in(&mut session, bytes)
    }

    /// Read and validate an iTIP message using the session's shared budget.
    pub fn read_in(session: &mut Session<'_>, bytes: &[u8]) -> Result<Self, Error> {
        Self::read_with_policy(bytes, session.engine.policy, &mut session.meter)
    }

    pub(crate) fn read_with_policy(
        bytes: &[u8],
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<Self, Error> {
        let calendar = parse_calendar(bytes, policy, meter)?;
        Self::from_calendar_in(calendar, policy, meter)
    }

    /// Read a decoded `text/calendar` body part under RFC 6047 section 2.4.
    ///
    /// `content_type` is the unfolded field value after `Content-Type:`.
    /// Decode the `Content-Transfer-Encoding` before passing `decoded_body`.
    /// Authenticate the envelope sender and pass its identity as [`Actor`] during review.
    pub fn read_imip(content_type: &[u8], decoded_body: &[u8]) -> Result<Self, Error> {
        let engine = Engine::default();
        let mut session = engine.session();
        Self::read_imip_in(&mut session, content_type, decoded_body)
    }

    /// Read an RFC 6047 body part and charge its header and decoded body to one session.
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

    /// Build a `PUBLISH` with the caller's `DTSTAMP`.
    pub fn publish(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Publish, dtstamp)
    }

    /// Build a `REQUEST` with the caller's `DTSTAMP`.
    pub fn request(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Request, dtstamp)
    }

    /// Build a `REPLY` with the caller's `DTSTAMP`.
    pub fn reply(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Reply, dtstamp)
    }

    /// Build an `ADD` with the caller's `DTSTAMP`.
    pub fn add(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Add, dtstamp)
    }

    /// Build a `CANCEL` with the caller's `DTSTAMP`.
    pub fn cancel(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Cancel, dtstamp)
    }

    /// Build a `REFRESH` with the caller's `DTSTAMP`.
    pub fn refresh(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Refresh, dtstamp)
    }

    /// Build a `COUNTER` with the caller's `DTSTAMP`.
    pub fn counter(payload: &[u8], dtstamp: Timestamp) -> Result<Self, Error> {
        Self::outbound(payload, Method::Counter, dtstamp)
    }

    /// Build a `DECLINECOUNTER` with the caller's `DTSTAMP`.
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
    /// The header declares `UTF-8` for every body and takes `method` from the validated message.
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

    /// Review the message for one actor using the stored calendar.
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

    /// Review the message using the session's shared budget and zone database.
    pub fn review_in<'a>(
        &'a self,
        session: &mut Session<'_>,
        current: &'a Calendar,
        actor: &Actor,
    ) -> Result<Review<'a>, Rejection> {
        self.review_target_in(session, ReviewTarget::Held(current), actor)
    }

    /// Review creation for an absent destination on behalf of one actor.
    /// The caller must enforce absence when storing the result.
    pub fn review_creation(&self, actor: &Actor) -> Result<Review<'_>, Rejection> {
        let engine = Engine::builder()
            .resource_policy(self.calendar.policy)
            .build();
        let mut session = engine.session();
        self.review_creation_in(&mut session, actor)
    }

    /// Review creation using the session's shared budget and zone database.
    pub fn review_creation_in(
        &self,
        session: &mut Session<'_>,
        actor: &Actor,
    ) -> Result<Review<'_>, Rejection> {
        self.review_target_in(session, ReviewTarget::Absent, actor)
    }

    fn review_target_in<'a>(
        &'a self,
        session: &mut Session<'_>,
        target: ReviewTarget<'a>,
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
        match target {
            ReviewTarget::Held(current) => {
                let uid = message.uid().as_bytes();
                let current_components =
                    scheduled_components(current, uid, &message, &mut session.meter)?;
                if current_components.is_empty() {
                    self.review_initial(target, actor, &message)
                } else {
                    self.review_existing(session, current, actor, &message, &current_components)
                }
            },
            ReviewTarget::Absent => self.review_initial(target, actor, &message),
        }
    }

    /// Map every payload onto an exact component or one recurrence-proven master.
    fn review_existing<'a>(
        &'a self,
        session: &mut Session<'_>,
        current: &'a Calendar,
        actor: &Actor,
        message: &ItipMessage<'_>,
        current_components: &[(usize, &'a Component)],
    ) -> Result<Review<'a>, Rejection> {
        let payloads = message_payloads(&self.calendar, message);
        if payloads.len() != message.payload_count() {
            return Err(Rejection::new("icalkit.scheduling.message-invalid"));
        }
        let mut masters = current_components
            .iter()
            .copied()
            .filter(|(_, component)| ScheduledView::of(component).recurrence_id().is_none());
        let master = masters.next();
        if masters.next().is_some() {
            return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
        }
        let mut used = Vec::new();
        let mut answered = BTreeSet::new();
        let mut changes = Vec::with_capacity(payloads.len());
        for (payload_index, (source, payload)) in payloads.into_iter().enumerate() {
            let payload_view = ScheduledView::of(payload);
            if self.method == Method::Reply
                && message.kind() == crate::internal::core::ComponentKind::Event
                && payload_view.recurrence_id().is_some()
                && let Some((_, base)) = master
            {
                let occurrence = reply_occurrence(session, current, &self.calendar, base, payload)?;
                if !answered.insert(occurrence.key) {
                    return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
                }
                if let Some(change) = review_occurrence(
                    occurrence,
                    message,
                    payload_index,
                    source,
                    actor,
                    &mut session.meter,
                )? {
                    changes.push(change);
                }
                continue;
            }
            if let Some((item, current_component)) =
                stored_instance(payload_view.recurrence_id(), current_components)?
            {
                if used.contains(&item) {
                    return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
                }
                used.push(item);
                let current_view = ScheduledView::of(current_component);
                let authorization =
                    evaluate_message(message, &current_view, PartyId::new(actor.as_str()))
                        .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
                changes.push(ReviewedChange {
                    target: ChangeTarget::existing(item),
                    transition: authorization.into_transition(),
                });
                continue;
            }

            let (master_item, master_component) =
                master.ok_or_else(|| Rejection::new("icalkit.scheduling.no-matching-current"))?;
            let master_view = ScheduledView::of(master_component);
            let authorization = evaluate_new_instance_payload(
                message,
                &master_view,
                payload_index,
                PartyId::new(actor.as_str()),
            )
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
            let limits = session.engine.policy.limits;
            let membership = session.with_zone_source(|source, meter, diagnostics| {
                let zones = Zones::new(source);
                let mut budget = Budget::new(limits, meter);
                recurrence_set_contains(master_component, payload, zones, &mut budget, diagnostics)
                    .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))
            })?;
            if membership != KernelMatch::Matched {
                return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
            }
            let no_change = authorization.transition().is_empty();
            changes.push(ReviewedChange {
                target: if no_change {
                    ChangeTarget::existing(master_item)
                } else {
                    ChangeTarget::Detach {
                        master: master_item,
                        source,
                    }
                },
                transition: authorization.into_transition(),
            });
        }
        Ok(Review {
            message: self,
            target: ReviewTarget::Held(current),
            changes,
        })
    }

    fn review_initial<'a>(
        &'a self,
        target: ReviewTarget<'a>,
        actor: &Actor,
        message: &ItipMessage<'_>,
    ) -> Result<Review<'a>, Rejection> {
        let payloads = message_payloads(&self.calendar, message);
        if payloads.len() != message.payload_count() {
            return Err(Rejection::new("icalkit.scheduling.message-invalid"));
        }
        let mut changes = Vec::with_capacity(payloads.len());
        for (payload_index, (source, payload)) in payloads.into_iter().enumerate() {
            let empty = Component::create(payload.name().as_bytes(), Vec::new())
                .map_err(|_| Rejection::new("icalkit.scheduling.message-invalid"))?;
            let empty_view = ScheduledView::of(&empty);
            let authorization = evaluate_initial_payload(
                message,
                &empty_view,
                payload_index,
                PartyId::new(actor.as_str()),
            )
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
            changes.push(ReviewedChange {
                target: ChangeTarget::Create { source },
                transition: authorization.into_transition(),
            });
        }
        Ok(Review {
            message: self,
            target,
            changes,
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
            if meter.is_exhausted()
                || issues
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

/// The scheduling identity the caller supplies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Actor {
    address: String,
}

impl Actor {
    /// Construct a calendar user address with valid URI syntax.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Option<Self> {
        let address = address.into();
        UriValue::decode_value(address.as_bytes())
            .is_ok()
            .then_some(Self { address })
    }

    /// The `CAL-ADDRESS` spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.address
    }
}

/// An inert review that still borrows the message and state it describes.
#[derive(Debug)]
pub struct Review<'a> {
    message: &'a Message,
    target: ReviewTarget<'a>,
    changes: Vec<ReviewedChange>,
}

impl<'a> Review<'a> {
    /// How many property occurrences would change.
    #[must_use]
    pub fn change_count(&self) -> usize {
        self.changes.iter().fold(0, |count, change| {
            count.saturating_add(change.transition.len())
        })
    }

    /// The message this review describes.
    #[must_use]
    pub const fn message(&self) -> &Message {
        self.message
    }

    /// Turn this review into a single-use authorization capability.
    /// For a `COUNTER`, the organizer chooses whether to accept the proposed changes.
    #[must_use]
    pub fn authorize(self) -> AuthorizedChange<'a> {
        AuthorizedChange {
            message: self.message,
            target: self.target,
            changes: self.changes,
        }
    }
}

/// A single-use authorization that borrows the inputs from its review.
#[derive(Debug)]
pub struct AuthorizedChange<'a> {
    message: &'a Message,
    target: ReviewTarget<'a>,
    changes: Vec<ReviewedChange>,
}

impl AuthorizedChange<'_> {
    /// The message whose authorization this capability carries.
    #[must_use]
    pub const fn message(&self) -> &Message {
        self.message
    }

    /// Apply to a private clone of the reviewed state, consuming this authorization.
    /// After accepting a `COUNTER`, the organizer reschedules by sending a `REQUEST`.
    pub fn apply(mut self) -> Result<Calendar, Error> {
        let (mut document, policy) = match self.target {
            ReviewTarget::Held(current) => (current.document.clone(), current.policy),
            ReviewTarget::Absent => {
                let document = creation_document(&self.message.calendar)?;
                let policy = self.message.calendar.policy;
                return parse_calendar(
                    &document.to_bytes(),
                    policy,
                    &mut Meter::new(policy.limits),
                );
            },
        };
        self.changes.sort_by_key(|change| change.target.phase());
        {
            let calendar = document
                .components_mut()
                .next()
                .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
            for change in self.changes {
                match change.target {
                    ChangeTarget::Create { source } => {
                        let payload = root_component(&self.message.calendar)
                            .and_then(|calendar| calendar.items().get(source))
                            .and_then(Item::as_component)
                            .cloned()
                            .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
                        calendar.items_mut().push(Item::Component(payload));
                    },
                    ChangeTarget::Existing {
                        item,
                        inherited_attendees,
                    } => {
                        let component = calendar
                            .items_mut()
                            .get_mut(item)
                            .and_then(Item::as_component_mut)
                            .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
                        if !inherited_attendees.is_empty() {
                            component
                                .items_mut()
                                .try_reserve_exact(inherited_attendees.len())
                                .map_err(|_| Error::single("icalkit.scheduling.apply-refused"))?;
                            let at = component
                                .items()
                                .iter()
                                .position(|item| item.as_component().is_some())
                                .unwrap_or(component.items().len());
                            component.items_mut().splice(
                                at..at,
                                inherited_attendees.into_iter().map(Item::Property),
                            );
                        }
                        apply_to(component, &change.transition, policy)?;
                        if self.message.method == Method::Cancel {
                            mark_cancelled(component)?;
                        }
                    },
                    ChangeTarget::ReplyToOccurrence {
                        mut occurrence,
                        source,
                    } => {
                        apply_to(&mut occurrence, &change.transition, policy)?;
                        copy_recurrence_id(&mut occurrence, &self.message.calendar, source)?;
                        calendar.items_mut().push(Item::Component(*occurrence));
                    },
                    ChangeTarget::Detach { master, source } => {
                        let mut detached = calendar
                            .items()
                            .get(master)
                            .and_then(Item::as_component)
                            .cloned()
                            .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
                        apply_to(&mut detached, &change.transition, policy)?;
                        copy_recurrence_id(&mut detached, &self.message.calendar, source)?;
                        if self.message.method == Method::Cancel {
                            mark_cancelled(&mut detached)?;
                        }
                        calendar.items_mut().push(Item::Component(detached));
                    },
                }
            }
        }
        let bytes = document.to_bytes();
        let mut meter = Meter::new(policy.limits);
        let validated = parse_calendar(&bytes, policy, &mut meter)?;
        Ok(validated)
    }
}

#[derive(Debug)]
enum ReviewTarget<'a> {
    Held(&'a Calendar),
    Absent,
}

/// Which component an authorized transition writes, including a not-yet-materialized range anchor.
#[derive(Debug)]
enum ChangeTarget {
    /// Insert a payload whose identity is new to the calendar.
    Create { source: usize },
    /// Update the component at its reviewed position.
    Existing {
        item: usize,
        inherited_attendees: Vec<Property>,
    },
    /// Clone the reviewed master, apply the property diff, and append a detached instance.
    Detach { master: usize, source: usize },
    /// Store the reviewed occurrence with the attendee's answer.
    ReplyToOccurrence {
        occurrence: Box<Component>,
        source: usize,
    },
}

impl ChangeTarget {
    fn existing(item: usize) -> Self {
        Self::Existing {
            item,
            inherited_attendees: Vec::new(),
        }
    }

    /// Apply indexed mutations before appending components, keeping every reviewed index stable.
    const fn phase(&self) -> u8 {
        match self {
            Self::Existing { .. } => 0,
            Self::Create { .. } | Self::Detach { .. } | Self::ReplyToOccurrence { .. } => 1,
        }
    }
}

#[derive(Debug)]
struct ReviewedChange {
    target: ChangeTarget,
    transition: Transition,
}

/// Apply every occurrence-addressed change to one private component clone.
fn apply_to(
    event: &mut Component,
    transition: &Transition,
    policy: ResourcePolicy,
) -> Result<(), Error> {
    let mut target = ComponentTarget::new(event, policy.limits);
    for (at, change) in transition.writes() {
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

fn stored_instance<'a>(
    identity: Option<InstanceRef>,
    components: &[(usize, &'a Component)],
) -> Result<Option<(usize, &'a Component)>, Rejection> {
    let mut exact = None;
    for &(item, component) in components {
        match compare_instances(identity, ScheduledView::of(component).recurrence_id()) {
            InstanceMatch::Same if exact.is_none() => exact = Some((item, component)),
            InstanceMatch::Same | InstanceMatch::Ambiguous => {
                return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
            },
            InstanceMatch::Different => {},
        }
    }
    Ok(exact)
}

fn review_occurrence(
    mut occurrence: ReplyOccurrence<'_>,
    message: &ItipMessage<'_>,
    payload_index: usize,
    source: usize,
    actor: &Actor,
    meter: &mut Meter,
) -> Result<Option<ReviewedChange>, Rejection> {
    let transition = evaluate_new_instance_payload(
        message,
        &ScheduledView::of(&occurrence.event),
        payload_index,
        PartyId::new(actor.as_str()),
    )
    .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?
    .into_transition();
    if transition.is_empty() {
        return Ok(None);
    }
    let target = if let Some((item, stored)) = occurrence.stored_item {
        ChangeTarget::Existing {
            item,
            inherited_attendees: take_inherited_attendees(&mut occurrence.event, stored, meter)
                .map_err(|_| Rejection::new("icalkit.scheduling.review-resource-limit"))?,
        }
    } else {
        ChangeTarget::ReplyToOccurrence {
            occurrence: Box::new(occurrence.event),
            source,
        }
    };
    Ok(Some(ReviewedChange { target, transition }))
}

fn take_inherited_attendees(
    event: &mut Component,
    stored: &Component,
    meter: &mut Meter,
) -> Result<Vec<Property>, LimitExceeded> {
    let mut attendees = Vec::new();
    if stored
        .properties_named(&PropertyId::ATTENDEE)
        .next()
        .is_some()
    {
        return Ok(attendees);
    }
    let count = event.properties_named(&PropertyId::ATTENDEE).count();
    charge_attendee_slots(meter, count, stored.items().len())?;
    meter.admit(|| {
        attendees
            .try_reserve_exact(count)
            .map_err(|_| LimitExceeded::Budget)
    })?;
    attendees.extend(event.items_mut().drain(..).filter_map(|entry| match entry {
        Item::Property(property) if property.has_id(&PropertyId::ATTENDEE) => Some(property),
        _ => None,
    }));
    Ok(attendees)
}

/// Charge the review's attendee slots and the full item buffer the stored occurrence may need.
fn charge_attendee_slots(
    meter: &mut Meter,
    count: usize,
    stored: usize,
) -> Result<(), LimitExceeded> {
    let transfer = count.checked_mul(size_of::<Property>());
    let destination = stored
        .checked_add(count)
        .and_then(|slots| slots.checked_mul(size_of::<Item>()));
    let bytes = transfer
        .zip(destination)
        .and_then(|(left, right)| left.checked_add(right))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| meter.refuse_limit(LimitExceeded::Budget))?;
    meter.try_charge_bytes(bytes)
}

fn reply_occurrence<'a>(
    session: &mut Session<'_>,
    current: &'a Calendar,
    received: &Calendar,
    master: &Component,
    payload: &Component,
) -> Result<ReplyOccurrence<'a>, Rejection> {
    let siblings = root_component(current)
        .ok_or_else(|| Rejection::new("icalkit.scheduling.no-matching-current"))?
        .items();
    let limits = session.engine.policy.limits;
    session.with_zone_source(|external, meter, notes| {
        let source = ResourceZones::read(Some(current), external, meter, notes)
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
        let incoming = ResourceZones::read(Some(received), &source, meter, notes)
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
        // A floating occurrence keeps its civil time.
        let civil = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let zones = Zones::new(&source).with_query_zone_source(civil.tzid(), &civil);
        let mut budget = Budget::new(limits, meter);
        let received_zones = Zones::new(&incoming).with_query_zone_source(civil.tzid(), &civil);
        event_for_instance_reply(
            master,
            siblings,
            payload,
            zones,
            received_zones,
            &mut budget,
            notes,
        )
        .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))
    })
}

fn root_component(calendar: &Calendar) -> Option<&Component> {
    calendar.document.components().next()
}

fn creation_document(message: &Calendar) -> Result<Document, Error> {
    let mut document = message.document.clone();
    let calendar = document
        .components_mut()
        .next()
        .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
    calendar.items_mut().retain(|item| {
        !item
            .as_property()
            .is_some_and(|property| property.has_id(&PropertyId::METHOD))
    });
    Ok(document)
}

fn scheduled_components<'a>(
    calendar: &'a Calendar,
    uid: &[u8],
    message: &ItipMessage<'_>,
    meter: &mut Meter,
) -> Result<Vec<(usize, &'a Component)>, Rejection> {
    let mut matches = Vec::new();
    let mut charge = |units: usize| {
        meter
            .try_charge(u64::try_from(units).unwrap_or(u64::MAX))
            .map_err(|_| Rejection::new("icalkit.scheduling.review-resource-limit"))
    };
    let items = root_component(calendar).map_or(&[][..], Component::items);
    charge(items.len())?;
    for (item, component) in items
        .iter()
        .enumerate()
        .filter_map(|(item, entry)| entry.as_component().map(|component| (item, component)))
        .filter(|(_, component)| is_scheduling_payload(component))
    {
        charge(component.items().len())?;
        for property in component.properties_named(&PropertyId::UID) {
            charge(property.value_text().len())?;
            for parameter in property.parameters() {
                charge(parameter.name().len())?;
                charge(parameter.value().len())?;
            }
        }
        let known_uid = decode_single_scalar_text(component, &PropertyId::UID)
            .ok_or_else(|| Rejection::new("icalkit.scheduling.current-identity-unreadable"))?;
        if known_uid.as_bytes() == uid {
            if component.kind() != Some(message.kind()) {
                return Err(Rejection::new("icalkit.scheduling.authorization-denied"));
            }
            matches.push((item, component));
        }
    }
    Ok(matches)
}

fn message_payloads<'a>(
    calendar: &'a Calendar,
    message: &ItipMessage<'_>,
) -> Vec<(usize, &'a Component)> {
    root_component(calendar)
        .into_iter()
        .flat_map(|calendar| calendar.items().iter())
        .enumerate()
        .filter_map(|(item, entry)| entry.as_component().map(|component| (item, component)))
        .filter(|(_, component)| component.kind() == Some(message.kind()))
        .collect()
}

fn compare_instances(left: Option<InstanceRef>, right: Option<InstanceRef>) -> InstanceMatch {
    match (left, right) {
        (None, None) => InstanceMatch::Same,
        (Some(left), Some(right)) => left.compare(right),
        _ => InstanceMatch::Different,
    }
}

fn copy_recurrence_id(
    target: &mut Component,
    message: &Calendar,
    source: usize,
) -> Result<(), Error> {
    if target
        .properties()
        .any(|property| property.has_id(&PropertyId::RECURRENCE_ID))
    {
        return Ok(());
    }
    let property = root_component(message)
        .and_then(|calendar| calendar.items().get(source))
        .and_then(Item::as_component)
        .and_then(|component| {
            component
                .properties()
                .find(|property| property.has_id(&PropertyId::RECURRENCE_ID))
        })
        .cloned()
        .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
    let at = target
        .items()
        .iter()
        .position(|item| item.as_component().is_some())
        .unwrap_or(target.items().len());
    target.items_mut().insert(at, Item::Property(property));
    Ok(())
}

fn mark_cancelled(component: &mut Component) -> Result<(), Error> {
    set_property(component, &PropertyId::STATUS, b"STATUS", b"CANCELLED")
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

#[cfg(test)]
mod tests {
    use super::{Actor, Message};
    use crate::{Calendar, Engine, Meter, ResourcePolicy};
    use alloc::{format, string::String, vec::Vec};

    #[test]
    fn inherited_attendees_require_space_for_review_and_storage() {
        use crate::internal::core::{Item, Property};
        use core::fmt::Write as _;

        let calendar = |method: &str, events: &str| {
            format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\n{method}{events}END:VCALENDAR\r\n"
            )
        };
        let event_start = "BEGIN:VEVENT\r\nUID:s\r\nDTSTAMP:20260301T120000Z\r\n";
        let organizer = "ORGANIZER:mailto:chair@x\r\n";
        let count = 128;
        let mut master = format!(
            "{event_start}DTSTART:20260310T140000Z\r\nSUMMARY:S\r\n{organizer}RRULE:FREQ=WEEKLY;COUNT=2\r\n"
        );
        for index in 0..count {
            write!(master, "ATTENDEE:mailto:p{index}@x\r\n").unwrap();
        }
        master.push_str("END:VEVENT\r\n");
        let key = "RECURRENCE-ID:20260317T140000Z\r\n";
        let stored = format!("{event_start}{key}DTSTART:20260317T160000Z\r\nEND:VEVENT\r\n");
        let held = calendar("", &format!("{master}{stored}"));
        let current = Calendar::parse(held.as_bytes()).unwrap();
        let incoming = format!(
            "{event_start}{key}{organizer}ATTENDEE;PARTSTAT=ACCEPTED:mailto:p1@x\r\nEND:VEVENT\r\n"
        );
        let message = Message::read(calendar("METHOD:REPLY\r\n", &incoming).as_bytes()).unwrap();
        let actor = Actor::new("mailto:p1@x").unwrap();
        let engine = Engine::default();
        let mut session = engine.session();
        message.review_in(&mut session, &current, &actor).unwrap();
        let total = session.meter.spent();
        let rows = count * (size_of::<Property>() + size_of::<Item>());
        for budget in [u64::try_from(rows - 1).unwrap(), total - 1] {
            let mut session = engine.session();
            session.meter = Meter::with_budget(ResourcePolicy::secure().limits, budget);
            let Some(error) = message.review_in(&mut session, &current, &actor).err() else {
                panic!("review accepted {budget} bytes");
            };
            assert_eq!(
                error.code().as_str(),
                "icalkit.scheduling.review-resource-limit"
            );
            assert!(session.meter.is_exhausted());
            assert!(message.review_in(&mut session, &current, &actor).is_err());
            assert_eq!(current.to_bytes(), held.as_bytes());
        }
    }

    #[test]
    fn an_unrepresentable_attendee_allocation_exhausts_the_session() {
        use crate::internal::core::{Item, LimitExceeded};

        for (count, stored) in [
            (usize::MAX, 0),
            (1, usize::MAX),
            (1, usize::MAX / size_of::<Item>()),
        ] {
            let mut meter = Meter::new(ResourcePolicy::secure().limits);
            assert_eq!(
                super::charge_attendee_slots(&mut meter, count, stored),
                Err(LimitExceeded::Budget)
            );
            assert!(meter.is_exhausted());
        }
    }

    #[test]
    fn held_identity_review_charges_the_session_for_each_uid() {
        let event = "BEGIN:VEVENT\r\nUID:event@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nSUMMARY:Planning\r\nORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\nSEQUENCE:1\r\nEND:VEVENT\r\n";
        let calendar = |method: &str, components: &str| {
            format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit meter//EN\r\n{method}{components}END:VCALENDAR\r\n"
            )
        };
        let incoming = event.replace("SEQUENCE:1", "SEQUENCE:2");
        let message = Message::read(calendar("METHOD:REQUEST\r\n", &incoming).as_bytes()).unwrap();
        let actor = Actor::new("mailto:alice@example.test").unwrap();
        let engine = Engine::default();
        let mut costs = Vec::new();
        for extra in [String::new(), event.replace("UID:event", "UID:other")] {
            let held =
                Calendar::parse(calendar("", &format!("{event}{extra}")).as_bytes()).unwrap();
            let mut session = engine.session();
            message.review_in(&mut session, &held, &actor).unwrap();
            costs.push(session.meter.spent());
        }
        assert!(costs[1] >= costs[0] + u64::try_from("other@example.test".len()).unwrap());
        let body = format!("{event}{}", event.replace("UID:event", "UID:other"));
        let held = Calendar::parse(calendar("", &body).as_bytes()).unwrap();
        let mut session = engine.session();
        session.meter = Meter::with_budget(ResourcePolicy::secure().limits, costs[0]);
        let refusal = message.review_in(&mut session, &held, &actor).unwrap_err();
        assert_eq!(
            refusal.code().as_str(),
            "icalkit.scheduling.review-resource-limit"
        );
    }

    #[test]
    fn message_promotion_requires_a_complete_budgeted_inspection() {
        let bytes = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n",
            "METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:event@example.test\r\n",
            "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nSUMMARY:Planning\r\n",
            "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let calendar = Calendar::parse(bytes.as_bytes()).unwrap();
        let policy = ResourcePolicy::secure();
        let mut admitted = 0;
        for budget in 0..=1024 {
            let mut meter = Meter::with_budget(policy.limits, budget);
            if let Ok(message) = Message::from_calendar_in(calendar.clone(), policy, &mut meter) {
                assert!(
                    !meter.is_exhausted(),
                    "promoted an incomplete inspection with budget {budget}"
                );
                assert_eq!(message.to_bytes(), bytes.as_bytes());
                admitted += 1;
            }
        }
        assert!(admitted > 0);
    }
}
