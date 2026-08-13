// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Owned iTIP messages and a borrowed read-review-authorize-apply workflow.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter, Write as _};

use ical_core::{
    CivilDateTime, Component, ContentLineReader, Diagnostic, Document, Instant, Item, Meter,
    Property, PropertyId, TextValue, UtcOffset,
};
use ical_itip::{
    ComponentTarget, ItipMessage, Method, PartyId, ScheduleTarget, ScheduledView, Transition,
    evaluate_message, inspect_message,
};

use crate::calendar::{find_event_mut, parse_calendar};
use crate::failure::{Issue, IssueCode};
use crate::time::Timestamp;
use crate::{Calendar, Error, ResourcePolicy};

/// An owned scheduling message that has passed strict calendar and iTIP validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    calendar: Calendar,
    method: Method,
}

impl Message {
    /// Strictly read a complete inbound iTIP message.
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let calendar = Calendar::parse(bytes)?;
        Self::from_calendar(calendar)
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
        let message_component = root_component(&self.calendar)
            .ok_or_else(|| Rejection::new("icalkit.scheduling.message-invalid"))?;
        let message_view = ScheduledView::of(message_component);
        let mut meter = Meter::new(self.calendar.policy.limits);
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let message = ItipMessage::read(
            &message_view,
            self.calendar.policy.limits,
            &mut meter,
            &mut diagnostics,
        )
        .map_err(|_| Rejection::new("icalkit.scheduling.message-invalid"))?;
        let uid = message.uid().as_bytes();
        let current_component = event_component(current, uid)
            .ok_or_else(|| Rejection::new("icalkit.scheduling.no-matching-current"))?;
        let current_view = ScheduledView::of(current_component);
        let authorization = evaluate_message(&message, &current_view, PartyId::new(actor.as_str()))
            .map_err(|_| Rejection::new("icalkit.scheduling.authorization-denied"))?;
        let transition = authorization.into_transition();
        let uid = core::str::from_utf8(uid)
            .map_err(|_| Rejection::new("icalkit.scheduling.message-invalid"))?
            .to_string();
        Ok(Review {
            message: self,
            current,
            uid,
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
        let method = {
            let component = root_component(&calendar)
                .ok_or_else(|| Error::single("icalkit.scheduling.message-invalid"))?;
            let view = ScheduledView::of(component);
            let mut meter = Meter::new(calendar.policy.limits);
            let mut diagnostics: Vec<Diagnostic> = Vec::new();
            let message =
                ItipMessage::read(&view, calendar.policy.limits, &mut meter, &mut diagnostics)
                    .map_err(|_| Error::single("icalkit.scheduling.message-invalid"))?;
            inspect_message(&message, None, &mut meter, &mut diagnostics);
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
    uid: String,
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
            uid: self.uid,
            transition: self.transition,
        }
    }
}

/// A single-use authorized transition borrowing the exact inputs that were reviewed.
#[derive(Debug)]
pub struct AuthorizedChange<'a> {
    message: &'a Message,
    current: &'a Calendar,
    uid: String,
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
            let event = find_event_mut(&mut updated.document, &self.uid)
                .ok_or_else(|| Error::single("icalkit.scheduling.target-moved"))?;
            let mut target = ComponentTarget::new(event, policy.limits);
            for (at, change) in self.transition.changes() {
                target
                    .write_change(at, change)
                    .map_err(|_| Error::single("icalkit.scheduling.apply-refused"))?;
            }
        }
        let bytes = updated.document.to_bytes();
        let mut meter = Meter::new(policy.limits);
        let validated = parse_calendar(&bytes, policy, &mut meter)?;
        Ok(validated)
    }
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

fn event_component<'a>(calendar: &'a Calendar, uid: &[u8]) -> Option<&'a Component> {
    root_component(calendar)?
        .items()
        .iter()
        .filter_map(Item::as_component)
        .find(|component| {
            component.is_named(b"VEVENT")
                && component
                    .properties()
                    .find(|property| property.is_named(b"UID"))
                    .is_some_and(|property| property.value_text().as_bytes() == uid)
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
