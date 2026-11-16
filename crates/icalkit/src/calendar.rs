// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;
use core::str;

use crate::internal::core::codec::{RequestStatus, TextSyntax, is_text_value};
use crate::internal::core::mailto::is_mailto_uri;
use crate::internal::core::{
    BinaryValue, Component, ComponentKind, ContentLineReader, DateTimeValue, DecodeValue as _,
    Diagnostic, DiagnosticCode, Document, Duration, Geo, Item, Meter, Period, Property, PropertyId,
    TextValue, UriValue, UtcOffset, ValueType, is_iana_token,
};

use crate::ResourcePolicy;
use crate::Session;
use crate::failure::{Error, Issue};
use crate::internal::recur::{
    RecurrenceRule, RuleLimit, RulePart, UntilClock, ValueKind, parse_recur,
};
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
    let recurrence_issues = audit_document(&document, meter, &mut diagnostics);
    let mut issues: Vec<Issue> = diagnostics
        .into_iter()
        .map(Issue::from_diagnostic)
        .collect();
    issues.extend(recurrence_issues);
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

fn audit_document(
    document: &Document,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut pending: Vec<&Component> = document.components().collect();
    while let Some(component) = pending.pop() {
        if component.kind().is_none() {
            continue;
        }
        component.audit(meter, sink);
        if !recurrence_rules_are_valid(component, meter, sink) {
            issues.push(Issue::error("icalkit.validation.invalid-recurrence"));
        }
        pending.extend(component.items().iter().filter_map(Item::as_component));
    }
    issues
}

fn recurrence_rules_are_valid(
    component: &Component,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> bool {
    let mut valid = true;
    for property in component
        .properties()
        .filter(|property| property.is_named(b"RRULE"))
    {
        match property.declared_value_type() {
            Some(Err(_)) => continue,
            None | Some(Ok(ValueType::Recur)) => {},
            Some(Ok(_)) => {
                valid = false;
                continue;
            },
        }
        let before = sink.len();
        let parsed = parse_recur(property.value_text().as_bytes(), meter, sink);
        if sink.iter().skip(before).any(|diagnostic| {
            !matches!(diagnostic.severity(), crate::internal::core::Severity::Note)
        }) {
            valid = false;
        }
        match parsed {
            Ok(rule) => valid &= recurrence_matches_start(component, &rule),
            Err(_) => valid = false,
        }
    }
    valid
}

fn recurrence_matches_start(component: &Component, rule: &RecurrenceRule) -> bool {
    let Some(start) = read_date_time(component, b"DTSTART") else {
        return true;
    };
    if matches!(start, DateTimeValue::Date(_))
        && [RulePart::Hour, RulePart::Minute, RulePart::Second]
            .into_iter()
            .any(|part| rule.has_part(part))
    {
        return false;
    }
    let RuleLimit::Until {
        value_kind, clock, ..
    } = rule.limit()
    else {
        return true;
    };
    let observance = component.is_named(b"STANDARD") || component.is_named(b"DAYLIGHT");
    match start {
        DateTimeValue::Date(_) => value_kind == ValueKind::Date,
        DateTimeValue::Local(_) if !observance => {
            value_kind == ValueKind::DateTime && clock == UntilClock::Floating
        },
        DateTimeValue::Local(_) | DateTimeValue::Utc(_) | DateTimeValue::Zoned { .. } => {
            value_kind == ValueKind::DateTime && clock == UntilClock::Utc
        },
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
    if first.is_some_and(|calendar| !names_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-name"));
    }
    if first.is_some_and(|calendar| !known_text_properties_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-text"));
    }
    if first.is_some_and(|calendar| !known_enumerations_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-enumeration"));
    }
    if first.is_some_and(|calendar| !known_linked_values_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-uri-or-binary"));
    }
    if first.is_some_and(|calendar| !known_attendee_parameters_are_valid(calendar)) {
        issues.push(Issue::error(
            "icalkit.validation.invalid-attendee-parameter",
        ));
    }
    if first.is_some_and(|calendar| !known_address_parameters_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-address-parameter"));
    }
    if first.is_some_and(|calendar| !known_integer_properties_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-integer"));
    }
    if first.is_some_and(|calendar| !known_geographic_properties_are_valid(calendar)) {
        issues.push(Issue::error(
            "icalkit.validation.invalid-geographic-position",
        ));
    }
    if first.is_some_and(|calendar| !known_utc_offsets_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-utc-offset"));
    }
    if first.is_some_and(|calendar| !known_date_times_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-date-time"));
    }
    if first.is_some_and(|calendar| !known_temporal_lists_are_valid(calendar)) {
        issues.push(Issue::error("icalkit.validation.invalid-date-time-list"));
    }
    if let Some(calendar) = first {
        validate_component_structure(calendar, issues);
        validate_required_starts(calendar, issues);
        validate_alarms(calendar, issues);
        validate_time_relationships(calendar, issues);
    }
}

fn names_are_valid(component: &Component) -> bool {
    is_iana_token(component.name().as_bytes())
        && component.properties().all(|property| {
            is_iana_token(property.name().as_bytes())
                && property
                    .parameters()
                    .iter()
                    .all(|parameter| is_iana_token(parameter.name().as_bytes()))
        })
        && component.components().all(names_are_valid)
}

fn validate_component_structure(component: &Component, issues: &mut Vec<Issue>) {
    let Some(kind) = component.kind() else {
        return;
    };
    let missing_child = match kind {
        ComponentKind::Calendar => component.components().next().is_none(),
        ComponentKind::TimeZone => !component.components().any(|child| {
            matches!(
                child.kind(),
                Some(ComponentKind::Standard | ComponentKind::Daylight)
            )
        }),
        ComponentKind::Event
        | ComponentKind::Todo
        | ComponentKind::Journal
        | ComponentKind::FreeBusy
        | ComponentKind::Standard
        | ComponentKind::Daylight
        | ComponentKind::Alarm => false,
    };
    if missing_child {
        issues.push(Issue::error(
            "icalkit.validation.invalid-component-structure",
        ));
    }
    for child in component.components() {
        let Some(child_kind) = child.kind() else {
            continue;
        };
        let allowed = match kind {
            ComponentKind::Calendar => matches!(
                child_kind,
                ComponentKind::Event
                    | ComponentKind::Todo
                    | ComponentKind::Journal
                    | ComponentKind::FreeBusy
                    | ComponentKind::TimeZone
            ),
            ComponentKind::Event | ComponentKind::Todo => child_kind == ComponentKind::Alarm,
            ComponentKind::TimeZone => matches!(
                child_kind,
                ComponentKind::Standard | ComponentKind::Daylight
            ),
            ComponentKind::Journal
            | ComponentKind::FreeBusy
            | ComponentKind::Standard
            | ComponentKind::Daylight
            | ComponentKind::Alarm => false,
        };
        if !allowed {
            issues.push(Issue::error(
                "icalkit.validation.invalid-component-structure",
            ));
        }
        validate_component_structure(child, issues);
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

fn known_text_properties_are_valid(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    for property in component.properties() {
        if is_text_property(property) && !text_property_is_valid(property) {
            return false;
        }
    }
    component
        .items()
        .iter()
        .filter_map(Item::as_component)
        .all(known_text_properties_are_valid)
}

fn known_enumerations_are_valid(component: &Component) -> bool {
    let Some(kind) = component.kind() else {
        return true;
    };
    component
        .properties()
        .all(|property| enumerated_property_is_valid(kind, property))
        && component.components().all(known_enumerations_are_valid)
}

fn enumerated_property_is_valid(kind: ComponentKind, property: &Property) -> bool {
    const NAMES: &[&[u8]] = &[b"STATUS", b"TRANSP", b"CALSCALE", b"CLASS", b"METHOD"];
    if !NAMES.iter().any(|name| property.is_named(name)) {
        return true;
    }
    match property.declared_value_type() {
        Some(Err(_)) => return true,
        None | Some(Ok(ValueType::Text)) => {},
        Some(Ok(_)) => return false,
    }
    let value = property.value_text().as_bytes();
    if property.is_named(b"CLASS") || property.is_named(b"METHOD") {
        return is_iana_token(value);
    }
    let allowed: &[&[u8]] = if property.is_named(b"STATUS") {
        match kind {
            ComponentKind::Event => &[b"TENTATIVE", b"CONFIRMED", b"CANCELLED"],
            ComponentKind::Todo => &[
                b"NEEDS-ACTION",
                b"COMPLETED",
                b"IN-PROCESS",
                b"CANCELLED",
                b"PENDING",
                b"FAILED",
            ],
            ComponentKind::Journal => &[b"DRAFT", b"FINAL", b"CANCELLED"],
            _ => return true,
        }
    } else if property.is_named(b"TRANSP") {
        &[b"OPAQUE", b"TRANSPARENT"]
    } else {
        &[b"GREGORIAN"]
    };
    allowed
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn text_property_is_valid(property: &Property) -> bool {
    if property.parameters_named(b"VALUE").nth(1).is_some() {
        return false;
    }
    match property.declared_value_type() {
        Some(Err(_)) => return true,
        None | Some(Ok(ValueType::Text)) => {},
        Some(Ok(_)) => return false,
    }
    let bytes = property.value_text().as_bytes();
    if property.is_named(b"REQUEST-STATUS") {
        return property.parameters_named(b"LANGUAGE").nth(1).is_none()
            && RequestStatus::parse(bytes).is_some();
    }
    let syntax = if property.is_named(b"CATEGORIES") || property.is_named(b"RESOURCES") {
        TextSyntax::CommaList
    } else {
        TextSyntax::Scalar
    };
    is_text_value(bytes, syntax)
}

#[derive(Clone, Copy)]
enum LinkedProperty {
    Uri,
    CalendarAddress,
    Attachment,
}

#[derive(Clone, Copy)]
enum AddressParameter {
    Uri,
    Mailto,
    UriList,
}

fn known_address_parameters_are_valid(component: &Component) -> bool {
    let Some(kind) = component.kind() else {
        return true;
    };
    component
        .properties()
        .filter(|property| property.is_named(b"ATTENDEE") || property.is_named(b"ORGANIZER"))
        .all(|property| address_parameters_are_valid(property, kind))
        && component
            .components()
            .all(known_address_parameters_are_valid)
}

fn address_parameters_are_valid(property: &Property, kind: ComponentKind) -> bool {
    const PARAMETERS: &[(&[u8], AddressParameter)] = &[
        (b"MEMBER", AddressParameter::UriList),
        (b"DELEGATED-FROM", AddressParameter::UriList),
        (b"DELEGATED-TO", AddressParameter::UriList),
        (b"DIR", AddressParameter::Uri),
        (b"SENT-BY", AddressParameter::Mailto),
    ];
    if !matches!(
        property.declared_value_type(),
        None | Some(Ok(ValueType::CalAddress))
    ) {
        return true;
    }
    PARAMETERS.iter().all(|(name, syntax)| {
        let mut parameters = property.parameters_named(name);
        let Some(parameter) = parameters.next() else {
            return true;
        };
        let allowed = (property.is_named(b"ATTENDEE")
            && matches!(
                kind,
                ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal
            ))
            || (property.is_named(b"ORGANIZER")
                && !matches!(syntax, AddressParameter::UriList)
                && matches!(
                    kind,
                    ComponentKind::Event
                        | ComponentKind::Todo
                        | ComponentKind::Journal
                        | ComponentKind::FreeBusy
                ));
        parameters.next().is_none()
            && allowed
            && quoted_address_parameter_is_valid(parameter.value().as_bytes(), *syntax)
    })
}

fn quoted_address_parameter_is_valid(mut bytes: &[u8], syntax: AddressParameter) -> bool {
    loop {
        let Some(quoted) = bytes.strip_prefix(b"\"") else {
            return false;
        };
        let Some(ending) = quoted.iter().position(|octet| *octet == b'"') else {
            return false;
        };
        let (uri, after) = quoted.split_at(ending);
        let Some(tail) = after.strip_prefix(b"\"") else {
            return false;
        };
        let valid = match syntax {
            AddressParameter::Uri | AddressParameter::UriList => {
                UriValue::decode_value(uri).is_ok()
            },
            AddressParameter::Mailto => is_mailto_uri(uri),
        };
        if !valid {
            return false;
        }
        if tail.is_empty() {
            return true;
        }
        if !matches!(syntax, AddressParameter::UriList) {
            return false;
        }
        let Some(rest) = tail.strip_prefix(b",") else {
            return false;
        };
        bytes = rest;
    }
}

fn known_attendee_parameters_are_valid(component: &Component) -> bool {
    let Some(kind) = component.kind() else {
        return true;
    };
    component
        .properties()
        .filter(|property| property.is_named(b"ATTENDEE"))
        .all(|property| attendee_parameters_are_valid(property, kind))
        && component
            .components()
            .all(known_attendee_parameters_are_valid)
}

fn attendee_parameters_are_valid(property: &Property, kind: ComponentKind) -> bool {
    const CONTROLS: &[&[u8]] = &[b"CUTYPE", b"PARTSTAT", b"ROLE", b"RSVP"];
    if !matches!(
        property.declared_value_type(),
        None | Some(Ok(ValueType::CalAddress))
    ) {
        return true;
    }
    CONTROLS.iter().all(|name| {
        let mut parameters = property.parameters_named(name);
        let Some(parameter) = parameters.next() else {
            return true;
        };
        let value = parameter.value().as_bytes();
        if parameters.next().is_some()
            || !matches!(
                kind,
                ComponentKind::Event | ComponentKind::Todo | ComponentKind::Journal
            )
            || !is_iana_token(value)
        {
            return false;
        }
        if parameter.is_named(b"RSVP") {
            value.eq_ignore_ascii_case(b"TRUE") || value.eq_ignore_ascii_case(b"FALSE")
        } else {
            !parameter.is_named(b"PARTSTAT") || participation_status_is_valid(value, kind)
        }
    })
}

fn participation_status_is_valid(value: &[u8], kind: ComponentKind) -> bool {
    if [b"COMPLETED".as_slice(), b"IN-PROCESS", b"FAILED"]
        .iter()
        .any(|status| value.eq_ignore_ascii_case(status))
    {
        return kind == ComponentKind::Todo;
    }
    if [b"TENTATIVE".as_slice(), b"DELEGATED"]
        .iter()
        .any(|status| value.eq_ignore_ascii_case(status))
    {
        return kind != ComponentKind::Journal;
    }
    true
}

fn known_linked_values_are_valid(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    component.properties().all(|property| {
        let kind = if property.is_named(b"ATTACH") {
            LinkedProperty::Attachment
        } else if property.is_named(b"ATTENDEE") || property.is_named(b"ORGANIZER") {
            LinkedProperty::CalendarAddress
        } else if property.is_named(b"URL") || property.is_named(b"TZURL") {
            LinkedProperty::Uri
        } else {
            return true;
        };
        linked_property_is_valid(property, kind)
    }) && component.components().all(known_linked_values_are_valid)
}

fn linked_property_is_valid(property: &Property, kind: LinkedProperty) -> bool {
    if property.parameters_named(b"VALUE").nth(1).is_some() {
        return false;
    }
    if matches!(kind, LinkedProperty::Attachment)
        && (property.parameters_named(b"ENCODING").nth(1).is_some()
            || property.parameters_named(b"FMTTYPE").nth(1).is_some())
    {
        return false;
    }
    let declared = match property.declared_value_type() {
        Some(Err(_)) => return true,
        Some(Ok(value_type)) => value_type,
        None => match kind {
            LinkedProperty::CalendarAddress => ValueType::CalAddress,
            LinkedProperty::Uri | LinkedProperty::Attachment => ValueType::Uri,
        },
    };
    let encoding = property.parameters_named(b"ENCODING").next();
    match (kind, declared) {
        (LinkedProperty::Uri, ValueType::Uri)
        | (LinkedProperty::CalendarAddress, ValueType::CalAddress) => {
            UriValue::decode_property(property).is_ok()
        },
        (LinkedProperty::Attachment, ValueType::Uri) => {
            encoding.is_none_or(|parameter| parameter.unquoted().eq_ignore_ascii_case(b"8BIT"))
                && UriValue::decode_property(property).is_ok()
        },
        (LinkedProperty::Attachment, ValueType::Binary) => {
            encoding.is_some_and(|parameter| parameter.unquoted().eq_ignore_ascii_case(b"BASE64"))
                && BinaryValue::decode_property(property).is_ok()
        },
        _ => false,
    }
}

fn known_integer_properties_are_valid(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    component.properties().all(|property| {
        let bounds = if property.is_named(b"PRIORITY") {
            0..=9
        } else if property.is_named(b"PERCENT-COMPLETE") {
            0..=100
        } else if property.is_named(b"SEQUENCE") {
            0..=i32::MAX
        } else {
            return true;
        };
        if property.parameters_named(b"VALUE").nth(1).is_some() {
            return false;
        }
        match property.declared_value_type() {
            Some(Err(_)) => true,
            None | Some(Ok(ValueType::Integer)) => {
                i32::decode_property(property).is_ok_and(|value| bounds.contains(&value))
            },
            Some(Ok(_)) => false,
        }
    }) && component
        .components()
        .all(known_integer_properties_are_valid)
}

fn known_utc_offsets_are_valid(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    component.properties().all(|property| {
        if !property.is_named(b"TZOFFSETFROM") && !property.is_named(b"TZOFFSETTO") {
            return true;
        }
        if property.parameters_named(b"VALUE").nth(1).is_some() {
            return false;
        }
        match property.declared_value_type() {
            Some(Err(_)) => true,
            None | Some(Ok(ValueType::UtcOffset)) => UtcOffset::decode_property(property).is_ok(),
            Some(Ok(_)) => false,
        }
    }) && component.components().all(known_utc_offsets_are_valid)
}

fn known_geographic_properties_are_valid(component: &Component) -> bool {
    if component.kind().is_none() {
        return true;
    }
    component
        .properties()
        .all(|property| !property.is_named(b"GEO") || geographic_property_is_valid(property))
        && component
            .components()
            .all(known_geographic_properties_are_valid)
}

fn geographic_property_is_valid(property: &Property) -> bool {
    if property.parameters_named(b"VALUE").nth(1).is_some() {
        return false;
    }
    match property.declared_value_type() {
        Some(Err(_)) => return true,
        None | Some(Ok(ValueType::Float)) => {},
        Some(Ok(_)) => return false,
    }
    if Geo::decode_property(property).is_err() {
        return false;
    }
    let mut coordinates = property
        .value_text()
        .as_bytes()
        .split(|octet| *octet == b';');
    coordinates
        .next()
        .is_some_and(|latitude| coordinate_is_within(latitude, 90))
        && coordinates
            .next()
            .is_some_and(|longitude| coordinate_is_within(longitude, 180))
}

fn coordinate_is_within(bytes: &[u8], bound: u16) -> bool {
    let unsigned = bytes
        .strip_prefix(b"+")
        .or_else(|| bytes.strip_prefix(b"-"))
        .unwrap_or(bytes);
    let mut fields = unsigned.split(|octet| *octet == b'.');
    let Some(degrees) = fields
        .next()
        .and_then(|whole| str::from_utf8(whole).ok()?.parse::<u16>().ok())
    else {
        return false;
    };
    degrees < bound
        || (degrees == bound
            && fields
                .next()
                .is_none_or(|fraction| fraction.iter().all(|octet| *octet == b'0')))
}

fn is_text_property(property: &Property) -> bool {
    const TEXT_NAMES: &[&[u8]] = &[
        b"ACTION",
        b"CALSCALE",
        b"CATEGORIES",
        b"CLASS",
        b"COMMENT",
        b"CONTACT",
        b"DESCRIPTION",
        b"LOCATION",
        b"METHOD",
        b"PRODID",
        b"RELATED-TO",
        b"REQUEST-STATUS",
        b"RESOURCES",
        b"STATUS",
        b"SUMMARY",
        b"TRANSP",
        b"TZID",
        b"TZNAME",
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
    if !date_time_value_is_valid(property, value, declared) {
        return false;
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

fn date_time_value_is_valid(
    property: &Property,
    value: DateTimeValue<'_>,
    declared: ValueType,
) -> bool {
    value.value_type() == declared
        && crate::time::from_core_date_time(value).is_some()
        && property
            .parameters_named(b"TZID")
            .next()
            .is_none_or(|zone| {
                !zone.unquoted().is_empty()
                    && !matches!(value, DateTimeValue::Date(_) | DateTimeValue::Utc(_))
            })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemporalListContext {
    Recurrence,
    Observance,
    FreeBusy,
}

fn known_temporal_lists_are_valid(component: &Component) -> bool {
    let Some(kind) = component.kind() else {
        return true;
    };
    let context = if matches!(kind, ComponentKind::Standard | ComponentKind::Daylight) {
        TemporalListContext::Observance
    } else {
        TemporalListContext::Recurrence
    };
    component.properties().all(|property| {
        if property.is_named(b"RDATE") || property.is_named(b"EXDATE") {
            temporal_list_is_valid(property, context)
        } else if property.is_named(b"FREEBUSY") {
            temporal_list_is_valid(property, TemporalListContext::FreeBusy)
        } else {
            true
        }
    }) && component.components().all(known_temporal_lists_are_valid)
}

fn temporal_list_is_valid(property: &Property, context: TemporalListContext) -> bool {
    let freebusy = context == TemporalListContext::FreeBusy;
    if property.parameters_named(b"VALUE").nth(1).is_some()
        || property.parameters_named(b"TZID").nth(1).is_some()
        || (freebusy && property.parameters_named(b"FBTYPE").nth(1).is_some())
    {
        return false;
    }
    let declared = match property.declared_value_type() {
        Some(Err(_)) => return true,
        Some(Ok(value_type)) => value_type,
        None if freebusy => ValueType::Period,
        None => ValueType::DateTime,
    };
    if freebusy && declared != ValueType::Period {
        return false;
    }
    if context == TemporalListContext::Observance && declared != ValueType::DateTime {
        return false;
    }
    property
        .value_text()
        .as_bytes()
        .split(|octet| *octet == b',')
        .all(|field| match declared {
            ValueType::Date | ValueType::DateTime => {
                DateTimeValue::decode_property_field(property, field).is_ok_and(|value| {
                    let form_valid = match context {
                        TemporalListContext::Observance => matches!(value, DateTimeValue::Local(_)),
                        TemporalListContext::Recurrence | TemporalListContext::FreeBusy => true,
                    };
                    form_valid && date_time_value_is_valid(property, value, declared)
                })
            },
            ValueType::Period if !property.is_named(b"EXDATE") => {
                period_field_is_valid(property, field, freebusy)
            },
            _ => false,
        })
}

fn period_field_is_valid(property: &Property, field: &[u8], utc_only: bool) -> bool {
    let Ok(period) = Period::decode_property_field(property, field) else {
        return false;
    };
    if property
        .parameters_named(b"TZID")
        .next()
        .is_some_and(|zone| zone.unquoted().is_empty())
    {
        return false;
    }
    let valid_bound = |bound| {
        crate::time::from_core_date_time(bound).is_some()
            && (!utc_only || matches!(bound, DateTimeValue::Utc(_)))
    };
    if !valid_bound(period.start()) {
        return false;
    }
    match period {
        Period::Explicit { start, end } => {
            valid_bound(end)
                && match (start, end) {
                    (DateTimeValue::Local(start), DateTimeValue::Local(end))
                    | (DateTimeValue::Utc(start), DateTimeValue::Utc(end)) => end > start,
                    _ => true,
                }
        },
        Period::Starting { .. } => true,
    }
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
    if !is_iana_token(action) {
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
