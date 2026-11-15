// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;
use core::str;

use crate::internal::core::{
    Component, ContentLineReader, DateTimeValue, DecodeValue as _, Diagnostic, DiagnosticCode,
    Document, Duration, Item, Meter, Property, PropertyId, TextValue, ValueType,
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
        validate_alarms(calendar, issues);
        validate_time_relationships(calendar, issues);
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
        b"ACKNOWLEDGED".as_slice(),
        b"COMPLETED",
        b"CREATED",
        b"DTSTAMP",
        b"LAST-MODIFIED",
        b"TRIGGER",
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

fn first_property<'a>(component: &'a Component, name: &[u8]) -> Option<&'a Property> {
    component
        .properties()
        .find(|property| property.is_named(name))
}

fn property_count(component: &Component, name: &[u8]) -> usize {
    component
        .properties()
        .filter(|property| property.is_named(name))
        .count()
}

fn validate_alarms(component: &Component, issues: &mut Vec<Issue>) {
    if component.kind().is_none() {
        return;
    }
    for child in component.components() {
        if child.is_named(b"VALARM") && !alarm_is_valid(component, child) {
            issues.push(Issue::error("icalkit.validation.invalid-alarm"));
        }
        validate_alarms(child, issues);
    }
}

fn alarm_is_valid(parent: &Component, alarm: &Component) -> bool {
    if property_count(alarm, b"UID") > 1 || property_count(alarm, b"ACKNOWLEDGED") > 1 {
        return false;
    }
    if first_property(alarm, b"ACKNOWLEDGED")
        .is_some_and(|property| !date_time_property_is_valid(alarm, property))
    {
        return false;
    }
    let Some(action) = first_property(alarm, b"ACTION") else {
        return false;
    };
    if matches!(action.declared_value_type(), Some(Err(_))) {
        return true;
    }
    let action = action.value_text().as_bytes();
    if action.is_empty()
        || !action
            .iter()
            .all(|octet| octet.is_ascii_alphanumeric() || *octet == b'-')
    {
        return false;
    }
    let valid_properties = if action.eq_ignore_ascii_case(b"AUDIO") {
        !has_property(alarm, b"DESCRIPTION")
            && !has_property(alarm, b"SUMMARY")
            && !has_property(alarm, b"ATTENDEE")
            && property_count(alarm, b"ATTACH") <= 1
    } else if action.eq_ignore_ascii_case(b"DISPLAY") {
        has_property(alarm, b"DESCRIPTION")
            && !has_property(alarm, b"SUMMARY")
            && !has_property(alarm, b"ATTENDEE")
            && !has_property(alarm, b"ATTACH")
    } else if action.eq_ignore_ascii_case(b"EMAIL") {
        has_property(alarm, b"DESCRIPTION")
            && has_property(alarm, b"SUMMARY")
            && has_property(alarm, b"ATTENDEE")
    } else {
        return true;
    };
    if !valid_properties {
        return false;
    }
    let duration = first_property(alarm, b"DURATION");
    let repeat = first_property(alarm, b"REPEAT");
    if duration.is_some() != repeat.is_some()
        || duration.is_some_and(|property| !positive_duration(property))
    {
        return false;
    }
    if let Some(repeat) = repeat {
        match repeat.declared_value_type() {
            Some(Err(_)) => {},
            None | Some(Ok(ValueType::Integer)) => {
                if !i32::decode_property(repeat).is_ok_and(|count| count >= 0) {
                    return false;
                }
            },
            Some(Ok(_)) => return false,
        }
    }
    first_property(alarm, b"TRIGGER")
        .is_some_and(|trigger| trigger_is_valid(parent, alarm, trigger))
}

fn positive_duration(property: &Property) -> bool {
    match property.declared_value_type() {
        Some(Err(_)) => true,
        None | Some(Ok(ValueType::Duration)) => {
            Duration::decode_property(property).is_ok_and(|value| {
                value.days() >= 0
                    && value.seconds() >= 0
                    && (value.days() > 0 || value.seconds() > 0)
            })
        },
        Some(Ok(_)) => false,
    }
}

fn trigger_is_valid(parent: &Component, alarm: &Component, trigger: &Property) -> bool {
    if trigger.parameters_named(b"VALUE").nth(1).is_some()
        || trigger.parameters_named(b"RELATED").nth(1).is_some()
    {
        return false;
    }
    let declared = match trigger.declared_value_type() {
        Some(Err(_)) => return true,
        Some(Ok(value_type)) => value_type,
        None => ValueType::Duration,
    };
    if trigger.parameters_named(b"TZID").next().is_some() {
        return false;
    }
    match declared {
        ValueType::DateTime => {
            trigger.parameters_named(b"RELATED").next().is_none()
                && date_time_property_is_valid(alarm, trigger)
        },
        ValueType::Duration => {
            if Duration::decode_property(trigger).is_err() {
                return false;
            }
            let related = trigger
                .parameters_named(b"RELATED")
                .next()
                .map_or(b"START".as_slice(), |parameter| parameter.unquoted());
            if related.eq_ignore_ascii_case(b"START") {
                has_property(parent, b"DTSTART")
            } else if related.eq_ignore_ascii_case(b"END") {
                (parent.is_named(b"VEVENT") && has_property(parent, b"DTEND"))
                    || (parent.is_named(b"VTODO") && has_property(parent, b"DUE"))
                    || (has_property(parent, b"DTSTART") && has_property(parent, b"DURATION"))
            } else {
                false
            }
        },
        _ => false,
    }
}

fn read_date_time<'a>(component: &'a Component, name: &[u8]) -> Option<DateTimeValue<'a>> {
    DateTimeValue::decode_property(first_property(component, name)?).ok()
}

fn validate_time_relationships(component: &Component, issues: &mut Vec<Issue>) {
    if component.kind().is_none() {
        return;
    }
    let start = read_date_time(component, b"DTSTART");
    if component.is_named(b"VEVENT") || component.is_named(b"VTODO") {
        if let Some(duration) = first_property(component, b"DURATION") {
            let uses_clock_units = matches!(start, Some(DateTimeValue::Date(_)))
                && !matches!(duration.declared_value_type(), Some(Err(_)))
                && duration.value_text().as_bytes().contains(&b'T');
            if !positive_duration(duration) || uses_clock_units {
                issues.push(Issue::error("icalkit.validation.invalid-duration"));
            }
        }
    }
    if let Some(start) = start {
        for name in [b"DTEND".as_slice(), b"DUE"] {
            if read_date_time(component, name).is_some_and(|end| !endpoint_follows(start, end)) {
                issues.push(Issue::error("icalkit.validation.invalid-time-range"));
            }
        }
        if read_date_time(component, b"RECURRENCE-ID")
            .is_some_and(|id| !compatible_date_times(start, id))
        {
            issues.push(Issue::error("icalkit.validation.invalid-time-range"));
        }
    }
    for child in component.components() {
        validate_time_relationships(child, issues);
    }
}

fn compatible_date_times(left: DateTimeValue<'_>, right: DateTimeValue<'_>) -> bool {
    left.value_type() == right.value_type()
        && matches!(left, DateTimeValue::Local(_)) == matches!(right, DateTimeValue::Local(_))
}

fn endpoint_follows(start: DateTimeValue<'_>, end: DateTimeValue<'_>) -> bool {
    if !compatible_date_times(start, end) {
        return false;
    }
    match (start, end) {
        (DateTimeValue::Date(start), DateTimeValue::Date(end)) => end > start,
        (DateTimeValue::Local(start), DateTimeValue::Local(end))
        | (DateTimeValue::Utc(start), DateTimeValue::Utc(end)) => end > start,
        // Ordering zoned values requires the caller's zone source.
        _ => true,
    }
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
