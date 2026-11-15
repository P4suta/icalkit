// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;
use core::str;

use crate::internal::core::{
    Component, ContentLineReader, DateTimeValue, DecodeValue as _, Diagnostic, DiagnosticCode,
    Document, Item, Meter, Property, PropertyId, TextValue, ValueType,
};

use crate::ResourcePolicy;
use crate::Session;
use crate::failure::{Error, Issue};
use crate::model::EventRef;
use crate::recurrence::{Cursor, Occurrences, Window};

/// A strictly validated, losslessly stored calendar object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub(crate) document: Document,
    issues: Vec<Issue>,
    pub(crate) policy: ResourcePolicy,
}

impl Calendar {
    /// Strict shorthand using secure resource defaults.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        let mut meter = Meter::new(policy.limits);
        parse_calendar(bytes, policy, &mut meter)
    }

    /// Serialize the stored CST.
    /// Unedited lines retain their original octets.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.document.to_bytes()
    }

    /// Notes and warnings retained during strict validation.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// Validated events in document order.
    pub fn events(&self) -> impl Iterator<Item = EventRef<'_>> {
        self.document
            .components()
            .flat_map(|calendar| calendar.items().iter())
            .filter_map(Item::as_component)
            .filter(|component| component.is_named(b"VEVENT"))
            .map(EventRef::new)
    }

    /// Expand one stored component's complete recurrence set over a mandatory window.
    ///
    /// The master, `RRULE`, `RDATE`, `EXDATE`, and sibling `RECURRENCE-ID` overrides are composed internally.
    /// Results are ordered by effective start rather than cadence key.
    pub fn occurrences<'a>(
        &'a self,
        session: &'a mut Session<'_>,
        uid: &str,
        window: Window,
    ) -> Result<Occurrences<'a>, Error> {
        crate::recurrence::calendar_occurrences(self, session, uid, window)
    }

    /// Resume a stored recurrence set after an opaque cursor.
    pub fn resume_occurrences<'a>(
        &'a self,
        session: &'a mut Session<'_>,
        uid: &str,
        window: Window,
        cursor: Cursor,
    ) -> Result<Occurrences<'a>, Error> {
        crate::recurrence::resume_calendar_occurrences(self, session, uid, window, cursor)
    }

    /// Begin a transaction over a private copy of this calendar.
    pub fn edit(&mut self) -> Editor<'_> {
        let working = self.document.clone();
        let policy = self.policy;
        Editor {
            target: self,
            working,
            policy,
        }
    }
}

pub(crate) fn parse_calendar(
    bytes: &[u8],
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Calendar, Error> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut reader = ContentLineReader::new(bytes, policy.limits.grammar());
    let document = Document::from_tokens(&mut reader, meter, &mut diagnostics)
        .map_err(|_| Error::single("icalkit.parse.resource-limit"))?;
    audit_document(&document, meter, &mut diagnostics);
    let mut issues: Vec<Issue> = diagnostics
        .into_iter()
        .map(Issue::from_diagnostic)
        .collect();
    validate_document_shape(&document, &mut issues);
    if issues
        .iter()
        .any(|issue| issue.is_error() || issue.is_warning())
    {
        return Err(Error::new("icalkit.validation.failed", issues));
    }
    Ok(Calendar {
        document,
        issues,
        policy,
    })
}

fn audit_document(document: &Document, meter: &mut Meter, sink: &mut Vec<Diagnostic>) {
    let mut pending: Vec<&Component> = document.components().collect();
    while let Some(component) = pending.pop() {
        if component.kind().is_none() {
            continue;
        }
        component.audit(meter, sink);
        pending.extend(component.items().iter().filter_map(Item::as_component));
    }
}

fn validate_document_shape(document: &Document, issues: &mut Vec<Issue>) {
    let mut calendars = document.components();
    let first = calendars.next();
    if first.is_none_or(|component| !component.is_named(b"VCALENDAR"))
        || calendars.next().is_some()
        || document
            .items()
            .iter()
            .any(|item| item.as_property().is_some())
    {
        issues.push(Issue::error("icalkit.validation.calendar-shape"));
        return;
    }
    if first.is_some_and(|calendar| !known_text_is_utf8(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-text"));
    }
    if first.is_some_and(|calendar| !known_date_times_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-date-time"));
    }
    if let Some(calendar) = first {
        validate_required_starts(calendar, issues);
    }
}

fn validate_required_starts(calendar: &Component, issues: &mut Vec<Issue>) {
    let has_method = has_property(calendar, b"METHOD");
    for component in calendar.components() {
        let is_event = component.is_named(b"VEVENT");
        let is_todo = component.is_named(b"VTODO");
        let is_recurring_kind = is_event || is_todo || component.is_named(b"VJOURNAL");
        let requires_start = (is_event && !has_method)
            || (is_todo && has_property(component, b"DURATION"))
            || (is_recurring_kind && has_property(component, b"RRULE"));
        if requires_start && !has_property(component, b"DTSTART") {
            issues.push(Issue::error(
                DiagnosticCode::MissingRequiredProperty.as_str(),
            ));
        }
    }
}

fn has_property(component: &Component, name: &[u8]) -> bool {
    component
        .properties()
        .any(|property| property.is_named(name))
}

fn known_text_is_utf8(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    for property in component.properties() {
        if is_text_property(property) && str::from_utf8(property.value_text().as_bytes()).is_err() {
            return false;
        }
    }
    component
        .items()
        .iter()
        .filter_map(Item::as_component)
        .all(known_text_is_utf8)
}

fn is_text_property(property: &Property) -> bool {
    const TEXT_NAMES: &[&[u8]] = &[
        b"CATEGORIES",
        b"CLASS",
        b"COMMENT",
        b"CONTACT",
        b"DESCRIPTION",
        b"LOCATION",
        b"PRODID",
        b"RELATED-TO",
        b"RESOURCES",
        b"STATUS",
        b"SUMMARY",
        b"TRANSP",
        b"UID",
    ];
    TEXT_NAMES.iter().any(|name| property.is_named(name))
}

fn known_date_times_are_valid(component: &Component) -> bool {
    const DATE_TIME_NAMES: &[&[u8]] = &[
        b"COMPLETED",
        b"CREATED",
        b"DTEND",
        b"DTSTAMP",
        b"DTSTART",
        b"DUE",
        b"LAST-MODIFIED",
        b"RECURRENCE-ID",
    ];
    if component.kind().is_none() {
        return true;
    }
    for property in component.properties() {
        if DATE_TIME_NAMES.iter().any(|name| property.is_named(name))
            && !date_time_property_is_valid(component, property)
        {
            return false;
        }
    }
    component
        .items()
        .iter()
        .filter_map(Item::as_component)
        .all(known_date_times_are_valid)
}

fn date_time_property_is_valid(component: &Component, property: &Property) -> bool {
    if property.parameters_named(b"VALUE").nth(1).is_some()
        || property.parameters_named(b"TZID").nth(1).is_some()
    {
        return false;
    }
    let declared = match property.declared_value_type() {
        Some(Err(_)) => return true,
        Some(Ok(value_type)) => value_type,
        None => ValueType::DateTime,
    };
    let Ok(value) = DateTimeValue::decode_property(property) else {
        return false;
    };
    if value.value_type() != declared || crate::time::from_core_date_time(value).is_none() {
        return false;
    }
    if let Some(zone) = property.parameters_named(b"TZID").next() {
        if zone.unquoted().is_empty()
            || matches!(value, DateTimeValue::Date(_) | DateTimeValue::Utc(_))
        {
            return false;
        }
    }
    let requires_utc = [
        b"COMPLETED".as_slice(),
        b"CREATED",
        b"DTSTAMP",
        b"LAST-MODIFIED",
    ]
    .iter()
    .any(|name| property.is_named(name))
        || (component.is_named(b"VFREEBUSY")
            && (property.is_named(b"DTSTART") || property.is_named(b"DTEND")));
    if requires_utc && !matches!(value, DateTimeValue::Utc(_)) {
        return false;
    }
    let requires_local = property.is_named(b"DTSTART")
        && (component.is_named(b"STANDARD") || component.is_named(b"DAYLIGHT"));
    !requires_local || matches!(value, DateTimeValue::Local(_))
}

/// A transactional calendar editor.
/// Dropping it rolls every edit back.
#[derive(Debug)]
pub struct Editor<'a> {
    target: &'a mut Calendar,
    working: Document,
    policy: ResourcePolicy,
}

impl Editor<'_> {
    /// Replace one event's SUMMARY with a safely encoded TEXT value.
    pub fn set_summary(&mut self, uid: &str, summary: &str) -> Result<(), Error> {
        let encoded = encode_text(summary)?;
        let event = find_event_mut(&mut self.working, uid)
            .ok_or_else(|| Error::single("icalkit.edit.event-not-found"))?;
        if let Some(mut property) = event.get_mut::<TextValue<'_>>(&PropertyId::SUMMARY) {
            property
                .set(&TextValue::from_bytes(&encoded))
                .map_err(|_| Error::single("icalkit.edit.value-not-representable"))?;
            return Ok(());
        }
        let property = Property::create(b"SUMMARY", Vec::new(), &encoded)
            .map_err(|_| Error::single("icalkit.edit.value-not-representable"))?;
        insert_property(event, property);
        Ok(())
    }

    /// Set a vendor property on one event with an ASCII case-insensitive `X-` prefix.
    pub fn set_vendor_property(
        &mut self,
        uid: &str,
        name: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        if !name
            .as_bytes()
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"X-"))
        {
            return Err(Error::single("icalkit.edit.vendor-name-required"));
        }
        let event = find_event_mut(&mut self.working, uid)
            .ok_or_else(|| Error::single("icalkit.edit.event-not-found"))?;
        if crate::model::Name::new(name).is_none() {
            return Err(Error::single("icalkit.edit.name-not-representable"));
        }
        let id = PropertyId::from_name(name.as_bytes());
        if let Some(mut property) = event.get_mut::<TextValue<'_>>(&id) {
            property
                .set_raw(value)
                .map_err(|_| Error::single("icalkit.edit.value-not-representable"))?;
            return Ok(());
        }
        let property = Property::create(name.as_bytes(), Vec::new(), value)
            .map_err(|_| Error::single("icalkit.edit.value-not-representable"))?;
        insert_property(event, property);
        Ok(())
    }

    /// Validate the complete working copy and atomically replace the target.
    pub fn commit(self) -> Result<(), Error> {
        let bytes = self.working.to_bytes();
        let mut meter = Meter::new(self.policy.limits);
        let validated = parse_calendar(&bytes, self.policy, &mut meter)?;
        *self.target = validated;
        Ok(())
    }
}

pub(crate) fn find_event_mut<'a>(
    document: &'a mut Document,
    wanted: &str,
) -> Option<&'a mut Component> {
    for calendar in document.components_mut() {
        for item in calendar.items_mut() {
            let Some(component) = item.as_component_mut() else {
                continue;
            };
            if component.is_named(b"VEVENT") && event_uid(component) == Some(wanted.as_bytes()) {
                return Some(component);
            }
        }
    }
    None
}

fn event_uid(component: &Component) -> Option<&[u8]> {
    component
        .properties()
        .find(|property| property.is_named(b"UID"))
        .map(|property| property.value_text().as_bytes())
}

fn insert_property(component: &mut Component, property: Property) {
    let at = component
        .items()
        .iter()
        .position(|item| item.as_component().is_some())
        .unwrap_or(component.items().len());
    component.items_mut().insert(at, Item::Property(property));
}

fn encode_text(text: &str) -> Result<Vec<u8>, Error> {
    let mut encoded = Vec::new();
    for octet in text.bytes() {
        match octet {
            b'\\' | b';' | b',' => {
                encoded.push(b'\\');
                encoded.push(octet);
            },
            b'\n' => encoded.extend_from_slice(b"\\n"),
            b'\r' | 0..=8 | 11..=31 | 127 => {
                return Err(Error::single("icalkit.edit.control-character"));
            },
            _ => encoded.push(octet),
        }
    }
    Ok(encoded)
}
