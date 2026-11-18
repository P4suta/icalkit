// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Select and limit iCalendar data under RFC 4791 sections 9.6 through 9.6.7.
//!
//! A [`Selection`] pairs a calendar with a [`Reduction`] that describes any omissions.
//! Writing a partial response back to the server can delete properties, components, overrides, or busy periods.
//! A chain of selections combines all its reduction flags.
//!
//! Without a `CALDAV:comp` element, [`select`] returns the complete source.
//! A component selection names individual properties and children or requests every member of either kind.
//! A component selector with no nested choices returns the entire component, following RFC 4791 section 7.8.1.
//! The selection keeps `VERSION`, `PRODID`, and the definitions of referenced zones.
//! RFC 4791 section 9.6 permits partial responses that omit required properties.
//!
//! [`without_values`] accepts the property names for `novalue="yes"`.
//! It keeps the original name and parameters.
//! Limiting recurrence keeps the master and the overrides that affect the time range.
//! Expansion materializes instances instead.
//! Limiting busy time tests individual `FREEBUSY` periods and keeps the bytes of each surviving field.

use alloc::vec::{IntoIter, Vec};
use core::{mem, slice};

use crate::internal::core::{
    CivilDate, CivilDateTime, Component, DateTimeValue, DecodeValue, Document, Duration,
    EncodeValue, Instant, Item, LimitExceeded, Limits, LineLayout, Meter, Parameter, Period,
    Property, PropertyId, RawText, UtcOffset, ValueBuf, ValueType, View, Writer,
};
use crate::internal::dav::{CompSelection, TimeRange};

use crate::internal::query::expand::{
    EndKind, InstanceShape, component_overlaps, component_start, event_instance_at_key,
    expand_related, has_recurrence, occupancy_of, override_impacts, override_recurrence_id,
    placed_value, unique_reply_key,
};
use crate::internal::query::series_index::{Member, SeriesIndex, SiblingScope, is_override};
use crate::internal::query::{Budget, Match, QueryError, Reduction, Selection, Undecided, Zones};

/// Stable identity for borrowing the matching `TZID` property.
static TZID: PropertyId = PropertyId::TZID;

/// Specification sections for component selection, value omission, and time-range limits.
pub const SUBSELECTION_SECTIONS: &[&str] = &[
    "RFC 4791 section 9.6, CALDAV:calendar-data, and the resource returned in its entirety",
    "RFC 4791 section 9.6.1, CALDAV:comp",
    "RFC 4791 section 9.6.2, CALDAV:allcomp",
    "RFC 4791 section 9.6.3, CALDAV:allprop",
    "RFC 4791 section 9.6.4, CALDAV:prop and novalue",
    "RFC 4791 section 9.6.6, CALDAV:limit-recurrence-set",
    "RFC 4791 section 9.6.7, CALDAV:limit-freebusy-set",
    "RFC 4791 section 9.9, the VFREEBUSY row a FREEBUSY period is kept by",
    "RFC 4791 section 7.8.1, the worked example of a comp selection",
    "RFC 4791 section 7.8.2, the worked example of a limited recurrence set",
    "RFC 5545 section 3.8.2.6, FREEBUSY and the periods one value lists",
];

/// The root component for a calendar selection.
const CALENDAR: &[u8] = b"VCALENDAR";

/// The component a `TZID` names, RFC 5545 section 3.6.5.
const TIME_ZONE: &[u8] = b"VTIMEZONE";

/// The component whose `FREEBUSY` periods section 9.6.7 thins, RFC 5545 section 3.6.4.
const FREE_BUSY: &[u8] = b"VFREEBUSY";

/// Apply a `CALDAV:comp` tree under RFC 4791 sections 9.6.1 through 9.6.4.
///
/// Pass `None` to keep the entire source and its existing reduction flags.
/// A selector with no nested choices returns the whole component.
/// The output keeps calendar metadata and referenced zone definitions.
/// Use [`without_values`] for `novalue="yes"`.
///
/// The [`Reduction`] identifies omitted content.
/// Writing a reduced result back can delete content from the stored resource.
///
/// # Errors
///
/// Returns [`QueryError::SelectionContradiction`] if a selector requests all properties or children and also names individual members of the same kind.
/// Validation checks the entire selection tree before reading the resource.
/// Returns [`QueryError::Limit`] if copying exceeds the byte or nesting limits.
pub fn select(
    source: &Selection,
    wanted: Option<&CompSelection>,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let limits = budget.limits;
    let meter = &mut *budget.meter;
    let Some(wanted) = wanted else {
        return unchanged(source, meter);
    };
    refuse_contradiction(wanted)?;
    let mut dropped: usize = 0;
    let mut items = Vec::new();
    for entry in source.calendar().items() {
        // A component selector excludes top-level properties and components outside its root.
        let named = entry
            .as_component()
            .filter(|holder| holder.is_named(wanted.name()));
        let Some(holder) = named else {
            dropped = dropped.saturating_add(1);
            continue;
        };
        let mut built = select_component(holder, wanted, limits, meter, &mut dropped)?;
        let restored = restore_zones(holder, &mut built, meter)?;
        dropped = dropped.saturating_sub(restored);
        items.push(Item::Component(built));
    }
    let reduction = widened(source.reduction(), dropped > 0, false);
    Ok(Selection::new(Document::new(items), reduction))
}

/// Apply `novalue="yes"` to the named properties under RFC 4791 section 9.6.4.
///
/// The result keeps each property name, its parameters, and the trailing colon.
/// Removing the value discards the original fold positions.
/// An empty name list keeps the source unchanged.
/// The [`Reduction`] records the omission because writing the result back would erase the values.
///
/// # Errors
///
/// Returns [`QueryError::Unrepresentable`] for a property the writer can't express as one content line.
/// Returns [`QueryError::Limit`] if projection exceeds the byte budget or component depth limit.
pub fn without_values(
    source: &Selection,
    names: &[&[u8]],
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let limits = budget.limits;
    let meter = &mut *budget.meter;
    if names.is_empty() {
        return unchanged(source, meter);
    }
    let mut stripped = false;
    let built = map_properties(
        copy_items(source, meter)?,
        limits,
        meter,
        &mut |_holder, line, ledger| {
            if names.iter().any(|name| line.is_named(name)) {
                stripped = true;
                return with_value(line, b"", ledger).map(Some);
            }
            Ok(Some(line))
        },
    )?;
    let reduction = widened(source.reduction(), stripped, false);
    Ok(Selection::new(Document::new(built), reduction))
}

/// Keep the master and the overrides that affect the requested range under RFC 4791 section 9.6.6.
///
/// The caller supplies `decide` for each component with a `RECURRENCE-ID`.
/// Check the current occurrence, its original interval, and later instances affected by `RANGE`.
/// Only [`Match::Unmatched`] drops an override.
/// Unknown overlap leaves the override in the response.
/// The master keeps its `RRULE`, `RDATE`, and `EXDATE` properties.
/// The [`Reduction`] records omitted overrides because a replacement write could delete them.
///
/// # Errors
///
/// Returns [`QueryError::Limit`] if copying exceeds the byte budget.
pub fn limit_recurrence_set<D>(
    source: &Selection,
    mut decide: D,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError>
where
    D: FnMut(&Component) -> Match,
{
    let meter = &mut *budget.meter;
    let mut trimmed = false;
    let mut items = Vec::new();
    for entry in source.calendar().items() {
        match entry {
            Item::Property(line) => {
                charge_line(line, meter)?;
                items.push(Item::Property(line.clone()));
            },
            Item::Component(holder) => {
                let kept = limited_calendar(holder, &mut decide, meter, &mut trimmed)?;
                items.push(Item::Component(kept));
            },
        }
    }
    let reduction = widened(source.reduction(), false, trimmed);
    Ok(Selection::new(Document::new(items), reduction))
}

/// Use the complete calendar to decide whether current or original occurrences affect the window.
/// Run the overlap checks before a structural selection can remove required properties.
pub fn limit_recurrence_set_in_window(
    source: &Selection,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let mut decisions = Vec::new();
    for calendar in source.calendar().components() {
        let mut scope = SiblingScope::new(calendar.items());
        for candidate in calendar
            .components()
            .filter(|component| is_override(component))
        {
            let index = scope.index(budget.meter)?;
            let answer = match index.master(candidate, budget.meter)? {
                Ok(Some(master)) => match index.related(master, budget.meter)? {
                    Ok(related) => {
                        override_impacts(master, candidate, related, window, zones, budget)?
                    },
                    Err(reason) => Match::Undecided(reason),
                },
                Ok(None) => Match::Undecided(Undecided::OverlapUndefined),
                Err(reason) => Match::Undecided(reason),
            };
            budget
                .meter
                .admit(|| decisions.try_reserve(1).map_err(|_| LimitExceeded::Budget))?;
            decisions.push(answer);
        }
    }
    let mut decisions = decisions.into_iter();
    limit_recurrence_set(
        source,
        |_candidate| {
            decisions
                .next()
                .unwrap_or(Match::Undecided(Undecided::OverlapUndefined))
        },
        budget,
    )
}

/// Replace recurring components with the UTC instances inside `window`.
pub fn expand_calendar(
    source: &Selection,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let mut items = Vec::new();
    for entry in source.calendar().items() {
        match entry {
            Item::Property(property) => {
                charge_line(property, budget.meter)?;
                items.push(Item::Property(property.clone()));
            },
            Item::Component(component) if component.is_named(CALENDAR) => {
                items.push(Item::Component(expand_calendar_component(
                    component, window, zones, budget,
                )?));
            },
            Item::Component(component) => {
                charge_subtree(component, budget.meter)?;
                items.push(Item::Component(component.clone()));
            },
        }
    }
    let items = map_properties(
        items,
        budget.limits,
        budget.meter,
        &mut |_, property, meter| normalize_expanded_property(property, zones, meter),
    )?;
    let calendar = Document::new(items);
    calendar.serialize(&mut OutputBudget(budget.meter))?;
    Ok(Selection::new(
        calendar,
        Reduction {
            expanded: true,
            ..source.reduction()
        },
    ))
}

struct OutputBudget<'a>(&'a mut Meter);

impl Writer for OutputBudget<'_> {
    type Error = LimitExceeded;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.try_charge_bytes(octets(bytes.len()))
    }
}

/// Expand the recurrence sets directly inside one `VCALENDAR`.
fn expand_calendar_component(
    calendar: &Component,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Component, QueryError> {
    charge_boundaries(calendar, budget.meter)?;
    let siblings = calendar.items();
    let index = SeriesIndex::new(siblings, budget.meter)?;
    let mut items = Vec::new();
    for entry in siblings {
        let master = match entry {
            Item::Component(component) => component,
            Item::Property(property) => {
                charge_line(property, budget.meter)?;
                items.push(Item::Property(property.clone()));
                continue;
            },
        };
        if master.is_named(b"VTIMEZONE") {
            continue;
        }
        if is_override(master) {
            if index
                .master(master, budget.meter)?
                .map_err(|_| QueryError::Unrepresentable)?
                .is_some()
            {
                continue;
            }
            if let Some(instance) = single_in_window(master, &index, window, zones, budget)? {
                items.push(Item::Component(instance));
            }
            continue;
        }
        let related = index
            .related(master, budget.meter)?
            .map_err(|_| QueryError::Unrepresentable)?;
        let recurring = has_recurrence(master) || !related.is_empty();
        if !recurring {
            if let Some(instance) = single_in_window(master, &index, window, zones, budget)? {
                items.push(Item::Component(instance));
            }
            continue;
        }

        let expansion = expand_related(master, related, window, zones, budget)?;
        if expansion.incomplete().is_some() {
            return Err(QueryError::Unrepresentable);
        }
        let initial = component_start(master, zones).map_err(|_| QueryError::Unrepresentable)?;
        for instance in expansion.instances() {
            let template = effective_template(master, related, *instance, zones, budget.meter)?;
            items.push(Item::Component(materialize_instance(
                template,
                *instance,
                initial,
                budget.meter,
            )?));
        }
    }
    Ok(Component::new(
        calendar.begin().clone(),
        items,
        calendar.end().cloned(),
    ))
}

fn single_in_window(
    component: &Component,
    index: &SeriesIndex<'_>,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Option<Component>, QueryError> {
    match component_overlaps(component, index, window, zones, budget, &mut Vec::new())? {
        Match::Matched => Ok(Some(single_template(component, zones, budget.meter)?)),
        Match::Unmatched => Ok(None),
        Match::Undecided(_) => Err(QueryError::Unrepresentable),
    }
}

fn single_template(
    component: &Component,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<Component, QueryError> {
    charge_subtree(component, meter)?;
    let mut built = component.clone();
    let requires_elapsed_duration = matches!(
        component.dtstart(),
        View::Valid {
            value: DateTimeValue::Zoned { .. } | DateTimeValue::Local(_),
            ..
        }
    );
    let duration = built.items_mut().iter_mut().find_map(|item| match item {
        Item::Property(property) if property.is_named(b"DURATION") => Some(property),
        _ => None,
    });
    if let Some(duration) = duration.filter(|_| requires_elapsed_duration) {
        let held = occupancy_of(component, zones, &[]).map_err(|_| QueryError::Unrepresentable)?;
        let seconds = held
            .start
            .zip(held.duration_end)
            .and_then(|(start, end)| start.checked_seconds_until(end))
            .filter(|seconds| *seconds > 0)
            .ok_or(QueryError::Unrepresentable)?;
        charge_line(duration, meter)?;
        let mut value = ValueBuf::new();
        Duration::new(0, seconds)
            .encode_value(&mut value)
            .map_err(|_| QueryError::Unrepresentable)?;
        duration.set_value_text(RawText::from_bytes(value.as_bytes()));
    }
    Ok(built)
}

fn normalize_expanded_property(
    mut property: Property,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<Option<Property>, QueryError> {
    if is_recurrence_property(&property) {
        return Ok(None);
    }
    let explicit_zone = property.parameters_named(b"TZID").next().is_some();
    match property.declared_value_type() {
        Some(Ok(ValueType::DateTime)) => {},
        None if [
            &PropertyId::DTSTART,
            &PropertyId::DTEND,
            &PropertyId::from_static(b"DUE"),
            &PropertyId::RECURRENCE_ID,
        ]
        .iter()
        .any(|id| property.has_id(id)) => {},
        Some(_) | None if !explicit_zone => return Ok(Some(property)),
        Some(_) | None => return Err(QueryError::Unrepresentable),
    }
    let value =
        DateTimeValue::decode_property(&property).map_err(|_| QueryError::Unrepresentable)?;
    match value {
        DateTimeValue::Zoned { .. } => {},
        DateTimeValue::Local(_) if zones.query_zone().is_some() => {},
        _ if !explicit_zone => return Ok(Some(property)),
        _ => return Err(QueryError::Unrepresentable),
    }
    let instant = placed_value(value, zones).map_err(|_| QueryError::Unrepresentable)?;
    let stamp =
        CivilDateTime::from_instant(instant, UtcOffset::UTC).ok_or(QueryError::Unrepresentable)?;
    charge_line(&property, meter)?;
    let mut value = ValueBuf::new();
    DateTimeValue::Utc(stamp)
        .encode_value(&mut value)
        .map_err(|_| QueryError::Unrepresentable)?;
    property.set_value_text(RawText::from_bytes(value.as_bytes()));
    property
        .edit_parameters()
        .retain(|parameter| !parameter.is_named(b"TZID"));
    Ok(Some(property))
}

pub(crate) struct ReplyOccurrence {
    pub(crate) event: Component,
    pub(crate) key: Instant,
    pub(crate) stored_item: Option<usize>,
}

/// Build the stored occurrence whose participation an attendee answers.
pub(crate) fn event_for_instance_reply(
    master: &Component,
    siblings: &[Item],
    reply: &Component,
    zones: Zones<'_>,
    received_zones: Zones<'_>,
    budget: &mut Budget<'_>,
    notes: &mut Vec<crate::internal::core::Diagnostic>,
) -> Result<ReplyOccurrence, QueryError> {
    let index = SeriesIndex::new(siblings, budget.meter)?;
    let related = index
        .related(master, budget.meter)?
        .map_err(|_| QueryError::Unrepresentable)?;
    let identity = reply
        .properties()
        .find(|property| property.has_id(&PropertyId::RECURRENCE_ID))
        .ok_or(QueryError::Unrepresentable)?;
    let addressed = identity
        .value::<DateTimeValue<'_>>()
        .value()
        .ok_or(QueryError::Unrepresentable)?;
    let key = unique_reply_key(addressed, zones)?;
    if unique_reply_key(addressed, received_zones)? != key {
        return Err(QueryError::Unrepresentable);
    }
    let instance = event_instance_at_key(master, related, addressed, key, zones, budget, notes)?;
    let mut stored_item = None;
    for member in related {
        let at = override_recurrence_id(master, member.component, zones)
            .map_err(|_| QueryError::Unrepresentable)?;
        let range = is_range_anchor(member.component);
        if at == key {
            let stored = member
                .component
                .get::<DateTimeValue<'_>>(&PropertyId::RECURRENCE_ID)
                .value()
                .ok_or(QueryError::Unrepresentable)?;
            if range
                || stored_item.replace(member.position).is_some()
                || unique_reply_key(stored, zones)? != key
                || matches!((stored, addressed), (DateTimeValue::Zoned {tzid: first, ..}, DateTimeValue::Zoned {tzid: second, ..}) if first != second)
            {
                return Err(QueryError::Unrepresentable);
            }
        }
    }
    charge_line(identity, budget.meter)?;
    let mut event = effective_template(master, related, instance, zones, budget.meter)?;
    set_reply_times(&mut event, instance, zones, budget.meter)?;
    Ok(ReplyOccurrence {
        event,
        key,
        stored_item,
    })
}

fn set_reply_times(
    event: &mut Component,
    instance: crate::internal::query::Instance,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let InstanceShape::Event { end, .. } = instance.shape() else {
        return Err(QueryError::Unrepresentable);
    };
    event.items_mut().retain(|entry| {
        entry.as_property().is_none_or(|property| {
            !is_recurrence_property(property)
                && !property.has_id(&PropertyId::RECURRENCE_ID)
                && (end == EndKind::Duration || !property.has_id(&PropertyId::DURATION))
                && (end == EndKind::Explicit || !property.has_id(&PropertyId::DTEND))
        })
    });
    for property in event
        .items_mut()
        .iter_mut()
        .filter_map(Item::as_property_mut)
    {
        if property.has_id(&PropertyId::DTSTART) {
            write_instance_time(property, instance.start(), zones, meter)?;
        } else if property.has_id(&PropertyId::DTEND) {
            write_instance_time(property, instance.end(), zones, meter)?;
        }
    }
    if end == EndKind::Explicit && event.properties_named(&PropertyId::DTEND).next().is_none() {
        let start = event
            .properties()
            .find(|property| property.has_id(&PropertyId::DTSTART))
            .ok_or(QueryError::Unrepresentable)?;
        charge_line(start, meter)?;
        let parameters = start
            .parameters()
            .iter()
            .filter(|p| p.is_named(b"TZID") || p.is_named(b"VALUE"))
            .cloned()
            .collect();
        let mut finish = Property::create(b"DTEND", parameters, start.value_text().as_bytes())
            .map_err(|_| QueryError::Unrepresentable)?;
        write_instance_time(&mut finish, instance.end(), zones, meter)?;
        insert_property(event, finish);
    }
    if end == EndKind::Duration
        && occupancy_of(event, zones, &[])
            .map_err(|_| QueryError::Unrepresentable)?
            .duration_end
            != Some(instance.end())
    {
        let seconds = instance
            .start()
            .checked_seconds_until(instance.end())
            .ok_or(QueryError::Unrepresentable)?;
        let mut value = ValueBuf::new();
        Duration::new(0, seconds)
            .encode_value(&mut value)
            .map_err(|_| QueryError::Unrepresentable)?;
        let duration = event
            .items_mut()
            .iter_mut()
            .filter_map(Item::as_property_mut)
            .find(|property| property.has_id(&PropertyId::DURATION))
            .ok_or(QueryError::Unrepresentable)?;
        meter.try_charge_bytes(octets(value.as_bytes().len()))?;
        duration.set_value_text(value.into_raw_text());
    }
    Ok(())
}

fn write_instance_time(
    property: &mut Property,
    instant: Instant,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let original = property
        .value::<DateTimeValue<'_>>()
        .value()
        .ok_or(QueryError::Unrepresentable)?;
    let offset = if let DateTimeValue::Zoned { tzid, .. } = original {
        let name = core::str::from_utf8(tzid).map_err(|_| QueryError::Unrepresentable)?;
        zones
            .offset_at(Some(name), instant)
            .map_err(|_| QueryError::Unrepresentable)?
            .offset
    } else {
        UtcOffset::UTC
    };
    let local = CivilDateTime::from_instant(instant, offset).ok_or(QueryError::Unrepresentable)?;
    let projected = match original {
        DateTimeValue::Date(_) => DateTimeValue::Date(local.date()),
        DateTimeValue::Local(_) => DateTimeValue::Local(local),
        DateTimeValue::Utc(_) => DateTimeValue::Utc(local),
        DateTimeValue::Zoned { tzid, .. } => DateTimeValue::Zoned { stamp: local, tzid },
    };
    if placed_value(projected, zones).map_err(|_| QueryError::Unrepresentable)? != instant {
        return Err(QueryError::Unrepresentable);
    }
    let mut value = ValueBuf::new();
    projected
        .encode_value(&mut value)
        .map_err(|_| QueryError::Unrepresentable)?;
    meter.try_charge_bytes(octets(value.as_bytes().len()))?;
    property.set_value_text(value.into_raw_text());
    Ok(())
}

/// Use the exact component when present, or compose the range anchors in force.
pub(super) fn effective_template(
    master: &Component,
    related: &[Member<'_>],
    instance: crate::internal::query::Instance,
    zones: Zones<'_>,
    meter: &mut Meter,
) -> Result<Component, QueryError> {
    charge_subtree(master, meter)?;
    let mut built = master.clone();
    if let Some(exact) = related
        .iter()
        .map(|member| member.component)
        .find(|candidate| {
            override_recurrence_id(master, candidate, zones).ok() == Some(instance.recurrence_id())
        })
    {
        overlay_properties(&mut built, exact, meter)?;
        return Ok(built);
    }
    let mut anchors: Vec<(Instant, &Component)> = related
        .iter()
        .map(|member| member.component)
        .filter(|candidate| is_range_anchor(candidate))
        .map(|candidate| {
            override_recurrence_id(master, candidate, zones)
                .map(|recurrence_id| (recurrence_id, candidate))
                .map_err(|_| QueryError::Unrepresentable)
        })
        .collect::<Result<_, _>>()?;
    anchors.sort_by_key(|(recurrence_id, _)| *recurrence_id);

    for (_, anchor) in anchors
        .into_iter()
        .filter(|(recurrence_id, _)| *recurrence_id <= instance.recurrence_id())
    {
        overlay_properties(&mut built, anchor, meter)?;
    }
    Ok(built)
}

/// Apply the properties one override states, leaving omitted base properties alone.
fn overlay_properties(
    target: &mut Component,
    overlay: &Component,
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let mut names: Vec<&[u8]> = Vec::new();
    for property in overlay.properties() {
        if is_recurrence_property(property) || names.iter().any(|name| property.is_named(name)) {
            continue;
        }
        names.push(property.name().as_bytes());
    }
    for name in names {
        target.items_mut().retain(|entry| {
            entry
                .as_property()
                .is_none_or(|property| !property.is_named(name))
        });
        for property in overlay
            .properties()
            .filter(|property| property.is_named(name))
        {
            charge_line(property, meter)?;
            insert_property(target, property.clone());
        }
    }
    Ok(())
}

/// Properties rebuilt from an [`Instance`](crate::internal::query::Instance), never copied as an override diff.
fn is_expansion_control(property: &Property) -> bool {
    is_recurrence_property(property)
        || [
            &PropertyId::RECURRENCE_ID,
            &PropertyId::DTSTART,
            &PropertyId::DTEND,
            &PropertyId::from_static(b"DUE"),
            &PropertyId::DURATION,
        ]
        .iter()
        .any(|id| property.has_id(id))
}

fn is_recurrence_property(property: &Property) -> bool {
    [b"RRULE".as_slice(), b"RDATE", b"EXDATE", b"EXRULE"]
        .iter()
        .any(|name| property.is_named(name))
}

/// Whether this override is a `RANGE=THISANDFUTURE` anchor.
fn is_range_anchor(component: &Component) -> bool {
    component
        .properties_named(&PropertyId::RECURRENCE_ID)
        .next()
        .is_some_and(|property| {
            property
                .parameters_named(b"RANGE")
                .any(|parameter| parameter.unquoted().eq_ignore_ascii_case(b"THISANDFUTURE"))
        })
}

/// Build one UTC instance while retaining the selected template's non-recurrence properties.
fn materialize_instance(
    mut template: Component,
    instance: crate::internal::query::Instance,
    initial: Instant,
    meter: &mut Meter,
) -> Result<Component, QueryError> {
    let controls = instance_controls(&mut template, instance, initial, meter)?;
    template.items_mut().retain(|entry| {
        entry
            .as_property()
            .is_none_or(|property| !is_expansion_control(property))
    });
    for property in controls {
        insert_property(&mut template, property);
    }
    Ok(template)
}

fn instance_controls(
    template: &mut Component,
    instance: crate::internal::query::Instance,
    initial: Instant,
    meter: &mut Meter,
) -> Result<Vec<Property>, QueryError> {
    let mut controls = Vec::new();
    if let InstanceShape::Journal {
        dates: Some((key, start)),
    } = instance.shape()
    {
        controls.push(date_property(template, b"DTSTART", start, meter)?);
        if instance.recurrence_id() != initial {
            controls.push(date_property(template, b"RECURRENCE-ID", key, meter)?);
        }
        return Ok(controls);
    }
    controls.push(utc_property(template, b"DTSTART", instance.start(), meter)?);
    match instance.shape() {
        InstanceShape::Event { end, .. } => {
            if instance.end() > instance.start() {
                controls.push(utc_property(template, b"DTEND", instance.end(), meter)?);
            } else if end != EndKind::Implied {
                return Err(QueryError::Unrepresentable);
            }
        },
        InstanceShape::Todo {
            end: EndKind::Duration,
        } => {
            let seconds = instance
                .start()
                .checked_seconds_until(instance.end())
                .filter(|seconds| *seconds > 0)
                .ok_or(QueryError::Unrepresentable)?;
            let mut value = ValueBuf::new();
            Duration::new(0, seconds)
                .encode_value(&mut value)
                .map_err(|_| QueryError::Unrepresentable)?;
            let property = replacement_property(template, b"DURATION", value.as_bytes(), meter)?;
            controls.push(property);
        },
        InstanceShape::Todo {
            end: EndKind::Explicit,
        } => {
            if instance.end() <= instance.start() {
                return Err(QueryError::Unrepresentable);
            }
            controls.push(utc_property(template, b"DUE", instance.end(), meter)?);
        },
        InstanceShape::Todo {
            end: EndKind::Implied,
        }
        | InstanceShape::Journal { .. } => {},
    }
    if instance.recurrence_id() != initial {
        controls.push(utc_property(
            template,
            b"RECURRENCE-ID",
            instance.recurrence_id(),
            meter,
        )?);
    }
    Ok(controls)
}

fn date_property(
    template: &mut Component,
    name: &[u8],
    date: CivilDate,
    meter: &mut Meter,
) -> Result<Property, QueryError> {
    let mut value = ValueBuf::new();
    DateTimeValue::Date(date)
        .encode_value(&mut value)
        .map_err(|_| QueryError::Unrepresentable)?;
    let mut property = replacement_property(template, name, value.as_bytes(), meter)?;
    keep_temporal_parameters(&mut property, name, b"DATE");
    if property.parameters_named(b"VALUE").next().is_none() {
        meter.try_charge_bytes(octets(b"VALUE".len().saturating_add(b"DATE".len())))?;
        property.edit_parameters().push(Parameter::new(
            RawText::from_bytes(b"VALUE"),
            RawText::from_bytes(b"DATE"),
        ));
    }
    Ok(property)
}

/// One canonical UTC date-time property.
fn utc_property(
    template: &mut Component,
    name: &[u8],
    instant: Instant,
    meter: &mut Meter,
) -> Result<Property, QueryError> {
    let stamp =
        CivilDateTime::from_instant(instant, UtcOffset::UTC).ok_or(QueryError::Unrepresentable)?;
    let mut value = ValueBuf::new();
    DateTimeValue::Utc(stamp)
        .encode_value(&mut value)
        .map_err(|_| QueryError::Unrepresentable)?;
    let mut property = replacement_property(template, name, value.as_bytes(), meter)?;
    keep_temporal_parameters(&mut property, name, b"DATE-TIME");
    Ok(property)
}

fn keep_temporal_parameters(property: &mut Property, name: &[u8], value_type: &[u8]) {
    property.edit_parameters().retain(|parameter| {
        if parameter.is_named(b"TZID") {
            false
        } else if parameter.is_named(b"RANGE") {
            !name.eq_ignore_ascii_case(b"RECURRENCE-ID")
        } else if parameter.is_named(b"VALUE") {
            parameter.unquoted().eq_ignore_ascii_case(value_type)
        } else {
            true
        }
    });
}

fn replacement_property(
    template: &mut Component,
    name: &[u8],
    value: &[u8],
    meter: &mut Meter,
) -> Result<Property, QueryError> {
    meter.try_charge_bytes(octets(value.len()))?;
    let position = template.items().iter().position(|item| {
        item.as_property()
            .is_some_and(|property| property.is_named(name))
    });
    if let Some(Item::Property(mut property)) =
        position.map(|index| template.items_mut().remove(index))
    {
        property.set_value_text(RawText::from_bytes(value));
        Ok(property)
    } else {
        meter.try_charge_bytes(octets(name.len()))?;
        Property::create(name, Vec::new(), value).map_err(|_| QueryError::Unrepresentable)
    }
}

/// Insert a property before any nested component.
fn insert_property(component: &mut Component, property: Property) {
    let at = component
        .items()
        .iter()
        .position(|entry| entry.as_component().is_some())
        .unwrap_or(component.items().len());
    component.items_mut().insert(at, Item::Property(property));
}

/// Keep overlapping `FREEBUSY` fields under RFC 4791 sections 9.6.7 and 9.9.
///
/// Keep readable UTC periods that intersect the half-open window.
/// Missing bounds leave the corresponding side open.
/// Keep each surviving field's original octets.
/// Keep unplaceable fields because their overlap remains unknown.
/// If all fields fall outside the range, drop the property.
/// The [`Reduction`] records omissions because writing the result back could delete busy periods.
///
/// # Errors
///
/// Returns [`QueryError::Unrepresentable`] if the writer can't express the remaining periods in one property.
/// Returns [`QueryError::Limit`] if projection exceeds the byte budget or component depth limit.
pub fn limit_freebusy_set(
    source: &Selection,
    window: TimeRange,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let limits = budget.limits;
    let meter = &mut *budget.meter;
    let mut trimmed = false;
    let built = map_properties(
        copy_items(source, meter)?,
        limits,
        meter,
        &mut |holder, line, ledger| {
            if !holder.eq_ignore_ascii_case(FREE_BUSY) || !line.has_id(&PropertyId::FREEBUSY) {
                return Ok(Some(line));
            }
            let (kept, thinned) = kept_periods(&line, window, ledger)?;
            if !thinned {
                return Ok(Some(line));
            }
            trimmed = true;
            kept.map(|value| with_value(line, &value, ledger))
                .transpose()
        },
    )?;
    let reduction = widened(source.reduction(), false, trimmed);
    Ok(Selection::new(Document::new(built), reduction))
}

/// Combine earlier and current reduction flags.
const fn widened(earlier: Reduction, components: bool, instances: bool) -> Reduction {
    Reduction {
        components_dropped: earlier.components_dropped || components,
        instances_dropped: earlier.instances_dropped || instances,
        expanded: earlier.expanded,
    }
}

/// Copy the source and charge the copy to the meter.
fn unchanged(source: &Selection, meter: &mut Meter) -> Result<Selection, QueryError> {
    charge_document(source.calendar(), meter)?;
    Ok(source.clone())
}

/// Reject `allprop` with an individual `prop` or `allcomp` with an individual `comp` anywhere in the tree.
/// RFC 4791 section 9.6.1 makes each pair mutually exclusive.
fn refuse_contradiction(root: &CompSelection) -> Result<(), QueryError> {
    let mut pending: Vec<&CompSelection> = alloc::vec![root];
    while let Some(node) = pending.pop() {
        if node.is_contradictory() {
            return Err(QueryError::SelectionContradiction);
        }
        pending.extend(node.comps());
    }
    Ok(())
}

/// Selection state for one open component.
#[derive(Debug)]
struct SelectFrame<'a> {
    /// The source of the component's opening and closing lines.
    holder: &'a Component,
    /// The entries of that component not yet examined.
    rest: slice::Iter<'a, Item>,
    /// What this level of the request asked for.
    wanted: &'a CompSelection,
    /// The entries kept so far.
    items: Vec<Item>,
}

impl<'a> SelectFrame<'a> {
    /// A frame that selects out of `holder` what `wanted` names, with nothing kept yet.
    fn of(holder: &'a Component, wanted: &'a CompSelection) -> Self {
        Self {
            holder,
            rest: holder.items().iter(),
            wanted,
            items: Vec::new(),
        }
    }

    /// Restore the original boundary lines around the selected items.
    fn finish(self) -> Component {
        Component::new(
            self.holder.begin().clone(),
            self.items,
            self.holder.end().cloned(),
        )
    }
}

/// The selection for one child component.
#[derive(Debug)]
enum Chosen<'a> {
    /// Returned entire: `allcomp`, or a selection that names nothing at all.
    Whole,
    /// Returned as this nested selection says.
    Under(&'a CompSelection),
    /// Named by nothing at this level, so left out.
    Excluded,
}

/// The contribution of a source entry to the selection.
#[derive(Debug)]
enum Step<'a> {
    /// Kept as it stands, and already charged.
    Kept(Item),
    /// Named by nothing, and counted as left out.
    Left,
    /// Kept as a nested selection says, which the walk descends into.
    Enter(&'a Component, &'a CompSelection),
}

/// What the selection at `frame`'s level does with `entry`.
///
/// Separate from the walk that applies it, so that the rules of section 9.6.1 read as rules rather than as arms of the loop that carries the stack.
fn step<'a>(
    frame: &SelectFrame<'a>,
    entry: &'a Item,
    meter: &mut Meter,
) -> Result<Step<'a>, QueryError> {
    match entry {
        Item::Property(line) => {
            if keeps_property(frame.wanted, frame.holder, line) {
                charge_line(line, meter)?;
                Ok(Step::Kept(Item::Property(line.clone())))
            } else {
                Ok(Step::Left)
            }
        },
        Item::Component(nested) => match chosen(frame.wanted, nested) {
            Chosen::Whole => {
                charge_subtree(nested, meter)?;
                Ok(Step::Kept(Item::Component(nested.clone())))
            },
            Chosen::Under(inner) => Ok(Step::Enter(nested, inner)),
            Chosen::Excluded => Ok(Step::Left),
        },
    }
}

/// Build the selected component and count omissions.
/// An explicit stack bounds traversal by the configured component depth.
fn select_component<'a>(
    holder: &'a Component,
    wanted: &'a CompSelection,
    limits: Limits,
    meter: &mut Meter,
    dropped: &mut usize,
) -> Result<Component, QueryError> {
    charge_boundaries(holder, meter)?;
    let mut open: Vec<SelectFrame<'a>> = Vec::new();
    let mut current = SelectFrame::of(holder, wanted);
    loop {
        let Some(entry) = current.rest.next() else {
            let finished = current.finish();
            let Some(parent) = open.pop() else {
                return Ok(finished);
            };
            current = parent;
            current.items.push(Item::Component(finished));
            continue;
        };
        // Decided before the match, so the borrow the rules read the frame through has ended by the time an arm writes to it.
        // The entry borrows the calendar rather than the frame, so it outlives both.
        let taken = step(&current, entry, meter)?;
        match taken {
            Step::Kept(kept) => current.items.push(kept),
            Step::Left => *dropped = dropped.saturating_add(1),
            Step::Enter(nested, inner) => {
                // The root occupies depth one.
                if open.len().saturating_add(2) > usize::from(limits.max_component_depth()) {
                    return Err(QueryError::Limit(LimitExceeded::Depth));
                }
                charge_boundaries(nested, meter)?;
                open.push(current);
                current = SelectFrame::of(nested, inner);
            },
        }
    }
}

/// An empty component selector returns the whole named component under RFC 4791 section 7.8.1.
fn keeps_whole(wanted: &CompSelection) -> bool {
    !wanted.all_props && !wanted.all_comps && wanted.props().is_empty() && wanted.comps().is_empty()
}

/// Whether the property `line` of `holder` survives the selection `wanted`.
fn keeps_property(wanted: &CompSelection, holder: &Component, line: &Property) -> bool {
    if keeps_whole(wanted) || wanted.all_props {
        return true;
    }
    if wanted.props().iter().any(|name| line.is_named(name)) {
        return true;
    }
    // Keep the metadata needed to read the selected calendar.
    holder.is_named(CALENDAR)
        && (line.has_id(&PropertyId::VERSION) || line.has_id(&PropertyId::PRODID))
}

/// What the selection `wanted` says about the subcomponent `nested`.
fn chosen<'a>(wanted: &'a CompSelection, nested: &Component) -> Chosen<'a> {
    if keeps_whole(wanted) || wanted.all_comps {
        return Chosen::Whole;
    }
    match wanted
        .comps()
        .iter()
        .find(|inner| nested.is_named(inner.name()))
    {
        Some(inner) => Chosen::Under(inner),
        None => Chosen::Excluded,
    }
}

/// Restore entire zone definitions referenced by surviving properties.
/// Return the number restored so the reduction count includes only actual omissions.
fn restore_zones(
    source: &Component,
    built: &mut Component,
    meter: &mut Meter,
) -> Result<usize, QueryError> {
    let stranded: Vec<&Component> = {
        let kept: &Component = built;
        source
            .components()
            .filter(|zone| zone.is_named(TIME_ZONE))
            .filter(|zone| {
                zone_id(zone).is_some_and(|id| !holds_zone(kept, id) && names_zone(kept, id))
            })
            .collect()
    };
    let restored = stranded.len();
    // Place restored definitions after the calendar properties and before the first component.
    let mut at = built
        .items()
        .iter()
        .position(|entry| entry.as_component().is_some())
        .unwrap_or_else(|| built.items().len());
    for zone in stranded {
        charge_subtree(zone, meter)?;
        built.items_mut().insert(at, Item::Component(zone.clone()));
        at = at.saturating_add(1);
    }
    Ok(restored)
}

/// The `TZID` one `VTIMEZONE` declares, RFC 5545 section 3.8.3.1.
fn zone_id(zone: &Component) -> Option<&[u8]> {
    zone.properties_named(&TZID)
        .next()
        .map(|line| line.value_text().as_bytes())
}

/// Whether `holder` already carries the `VTIMEZONE` declaring `id`.
fn holds_zone(holder: &Component, id: &[u8]) -> bool {
    holder
        .components()
        .filter(|zone| zone.is_named(TIME_ZONE))
        .any(|zone| zone_id(zone) == Some(id))
}

/// Compare each referenced `TZID` to `id` by exact bytes throughout the component tree.
fn names_zone(holder: &Component, id: &[u8]) -> bool {
    let mut pending: Vec<&Component> = alloc::vec![holder];
    while let Some(current) = pending.pop() {
        for entry in current.items() {
            match entry {
                Item::Property(line) => {
                    if line
                        .parameters_named(b"TZID")
                        .any(|parameter| parameter.unquoted() == id)
                    {
                        return true;
                    }
                },
                Item::Component(nested) => pending.push(nested),
            }
        }
    }
    false
}

/// Drop only direct children of `VCALENDAR` that represent overrides outside the range.
/// The master and unrelated components remain.
fn limited_calendar<D>(
    holder: &Component,
    decide: &mut D,
    meter: &mut Meter,
    trimmed: &mut bool,
) -> Result<Component, QueryError>
where
    D: FnMut(&Component) -> Match,
{
    charge_boundaries(holder, meter)?;
    let mut items = Vec::new();
    for entry in holder.items() {
        match entry {
            Item::Property(line) => {
                charge_line(line, meter)?;
                items.push(Item::Property(line.clone()));
            },
            Item::Component(nested) => {
                if is_override(nested) && decide(nested) == Match::Unmatched {
                    *trimmed = true;
                } else {
                    charge_subtree(nested, meter)?;
                    items.push(Item::Component(nested.clone()));
                }
            },
        }
    }
    Ok(Component::new(
        holder.begin().clone(),
        items,
        holder.end().cloned(),
    ))
}

fn copy_items(source: &Selection, meter: &mut Meter) -> Result<Vec<Item>, QueryError> {
    charge_document(source.calendar(), meter)?;
    Ok(source.calendar().items().to_vec())
}

/// State for rebuilding one component's contents.
#[derive(Debug)]
struct EditFrame {
    /// The source component with its boundary lines.
    shell: Component,
    /// The parent's remaining entries.
    rest: IntoIter<Item>,
    /// The parent's rebuilt entries preceding this child.
    items: Vec<Item>,
}

/// Rebuild entries with the result of `edit` for each property.
/// The callback receives the enclosing component name, the property, and the meter.
/// A top-level property has an empty component name.
/// Returning `None` drops the property.
/// The explicit traversal stack observes the component depth limit.
fn map_properties<F>(
    entries: Vec<Item>,
    limits: Limits,
    meter: &mut Meter,
    edit: &mut F,
) -> Result<Vec<Item>, QueryError>
where
    F: FnMut(&[u8], Property, &mut Meter) -> Result<Option<Property>, QueryError>,
{
    let mut rest = entries.into_iter();
    let mut items: Vec<Item> = Vec::new();
    let mut open: Vec<EditFrame> = Vec::new();
    loop {
        let Some(entry) = rest.next() else {
            let Some(frame) = open.pop() else {
                return Ok(items);
            };
            let mut finished = frame.shell;
            *finished.items_mut() = items;
            rest = frame.rest;
            items = frame.items;
            items.push(Item::Component(finished));
            continue;
        };
        match entry {
            Item::Property(line) => {
                let holder = open
                    .last()
                    .map_or(&b""[..], |frame| frame.shell.name().as_bytes());
                if let Some(kept) = edit(holder, line, meter)? {
                    items.push(Item::Property(kept));
                }
            },
            Item::Component(mut nested) => {
                // Document entries start at depth zero.
                if open.len().saturating_add(1) > usize::from(limits.max_component_depth()) {
                    return Err(QueryError::Limit(LimitExceeded::Depth));
                }
                let inner = mem::take(nested.items_mut());
                open.push(EditFrame {
                    shell: nested,
                    rest,
                    items,
                });
                rest = inner.into_iter();
                items = Vec::new();
            },
        }
    }
}

fn with_value(mut line: Property, value: &[u8], meter: &mut Meter) -> Result<Property, QueryError> {
    meter.try_charge_bytes(octets(line.name().len()).saturating_add(octets(value.len())))?;
    let parameters = mem::take(line.edit_parameters());
    Property::create(line.name().as_bytes(), parameters, value)
        .map_err(|_| QueryError::Unrepresentable)
}

/// Return the surviving `FREEBUSY` fields and whether any field fell outside the window.
/// Preserve the original octets even for empty or unreadable fields.
fn kept_periods(
    property: &Property,
    window: TimeRange,
    meter: &mut Meter,
) -> Result<(Option<Vec<u8>>, bool), QueryError> {
    meter.try_charge_bytes(line_octets(property))?;
    let original = property.value_text().as_bytes();
    let mut kept = None;
    let mut thinned = false;
    let mut start = 0usize;
    for (written, period) in
        super::freebusy::freebusy_fields(original, property.parameters().iter())
    {
        if period
            .and_then(period_bounds)
            .is_some_and(|(from, until)| !intersects(window, from, until))
        {
            if !thinned && start != 0 {
                append_field(&mut kept, &original[..start.saturating_sub(1)], meter)?;
            }
            thinned = true;
        } else if thinned {
            append_field(&mut kept, written, meter)?;
        }
        start = start.saturating_add(written.len()).saturating_add(1);
    }
    Ok((kept, thinned))
}

fn append_field(
    kept: &mut Option<Vec<u8>>,
    field: &[u8],
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let separator = kept.is_some();
    let size = field.len().saturating_add(usize::from(separator));
    meter.try_charge_bytes(octets(size))?;
    let value = kept.get_or_insert_with(Vec::new);
    meter.admit(|| value.try_reserve(size).map_err(|_| LimitExceeded::Budget))?;
    if separator {
        value.push(b',');
    }
    value.extend_from_slice(field);
    Ok(())
}

/// Find the UTC bounds of a period, or return `None` for a bound outside UTC or an unrepresentable result.
/// RFC 5545 section 3.8.2.6 requires UTC periods.
fn period_bounds(period: Period<'_>) -> Option<(Instant, Instant)> {
    let from = utc_instant(period.start())?;
    let until = match period {
        Period::Explicit { end, .. } => utc_instant(end)?,
        Period::Starting { duration, .. } => from.checked_add_seconds(span_seconds(duration)?)?,
    };
    Some((from, until))
}

/// The instant a value names when it names one in UTC, and `None` when it names one elsewhere.
fn utc_instant(value: DateTimeValue<'_>) -> Option<Instant> {
    match value {
        DateTimeValue::Utc(stamp) => stamp.at_offset(UtcOffset::UTC),
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Zoned { .. } => None,
    }
}

/// Convert the duration to seconds if the count fits.
fn span_seconds(span: Duration) -> Option<i64> {
    const SECONDS_PER_DAY: i64 = 86_400;
    span.days()
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(span.seconds())
}

/// Test the half-open overlap in the `VFREEBUSY` row of RFC 4791 section 9.9.
/// A period that just touches a window boundary has no overlap.
fn intersects(window: TimeRange, from: Instant, until: Instant) -> bool {
    window
        .start()
        .is_none_or(|edge| edge.unix_seconds() < until.unix_seconds())
        && window
            .end()
            .is_none_or(|edge| edge.unix_seconds() > from.unix_seconds())
}

/// Charge the caller's ledger for the octets of one content line.
fn charge_line(line: &Property, meter: &mut Meter) -> Result<(), QueryError> {
    meter.try_charge_bytes(line_octets(line))?;
    charge_folds(line.layout(), meter)
}

fn charge_folds(layout: &LineLayout, meter: &mut Meter) -> Result<(), QueryError> {
    for fold in layout.folds() {
        meter.try_charge_bytes(octets(fold.newline.written_len()).saturating_add(1))?;
    }
    Ok(())
}

/// Charge the caller's ledger for every octet of `holder` and everything nested inside it.
///
/// Iterative, for the reason [`select_component`] gives, and over a worklist rather than a nesting stack because charging has no order to respect.
fn charge_subtree(holder: &Component, meter: &mut Meter) -> Result<(), QueryError> {
    let mut pending: Vec<&Component> = alloc::vec![holder];
    while let Some(current) = pending.pop() {
        charge_boundaries(current, meter)?;
        for entry in current.items() {
            match entry {
                Item::Property(line) => charge_line(line, meter)?,
                Item::Component(nested) => pending.push(nested),
            }
        }
    }
    Ok(())
}

/// Charge the caller's ledger for every octet of one document.
fn charge_document(calendar: &Document, meter: &mut Meter) -> Result<(), QueryError> {
    for entry in calendar.items() {
        match entry {
            Item::Property(line) => charge_line(line, meter)?,
            Item::Component(holder) => charge_subtree(holder, meter)?,
        }
    }
    Ok(())
}

/// Charge the name, value, and parameters of a copied content line.
/// Saturating arithmetic preserves the upper bound on overflow.
fn line_octets(line: &Property) -> u64 {
    let mut total = octets(line.name().len()).saturating_add(octets(line.value_text().len()));
    for parameter in line.parameters() {
        total = total
            .saturating_add(octets(parameter.name().len()))
            .saturating_add(octets(parameter.value_storage_len()));
    }
    total
}

fn charge_boundaries(holder: &Component, meter: &mut Meter) -> Result<(), QueryError> {
    meter.try_charge_bytes(boundary_octets(holder))?;
    charge_folds(holder.begin().layout(), meter)?;
    if let Some(end) = holder.end() {
        charge_folds(end.layout(), meter)?;
    }
    Ok(())
}

/// Charge both copies of the component name and 14 octets for `BEGIN:`, `END:`, and the terminators.
fn boundary_octets(holder: &Component) -> u64 {
    const FIXED: u64 = 14;
    octets(holder.name().len())
        .saturating_mul(2)
        .saturating_add(FIXED)
}

/// A length as the ledger counts one, saturating where `usize` is wider than the count.
fn octets(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::internal::core::{
        DateTimeValue, DecodeValue, DiagnosticCode, Document, IgnoreDiagnostics, Instant,
        LimitExceeded, Limits, Meter, PropertyId, UtcOffset,
    };
    use crate::internal::dav::{CompSelection, TimeRange};

    use super::{
        SUBSELECTION_SECTIONS, limit_freebusy_set, limit_recurrence_set, select, without_values,
    };
    use crate::internal::query::{Budget, Match, QueryError, Reduction, Selection, Undecided};

    #[test]
    fn copying_an_encoded_time_zone_counts_its_decoded_storage() {
        use crate::internal::core::{Parameter, Property};

        for (name, encoded, decoded) in [
            (
                b"Zone^Name".as_slice(),
                b"Zone^^Name".len(),
                b"Zone^Name".len(),
            ),
            (b"Zone/Name".as_slice(), b"Zone/Name".len(), 0),
        ] {
            let zone = Parameter::create(b"TZID", name).unwrap();
            let stamp = b"20260317T100000";
            let line = Property::create(b"DTSTART", vec![zone], stamp).unwrap();
            let bytes =
                u64::try_from(b"DTSTART".len() + b"TZID".len() + encoded + decoded + stamp.len())
                    .unwrap();
            let mut short = Meter::with_budget(Limits::DEFAULT, bytes - 1);
            assert_eq!(
                super::charge_line(&line, &mut short),
                Err(QueryError::Limit(LimitExceeded::Budget))
            );
            let mut exact = Meter::with_budget(Limits::DEFAULT, bytes);
            assert!(super::charge_line(&line, &mut exact).is_ok());
            assert_eq!(exact.spent(), bytes);
        }
    }

    /// RFC 4791 Appendix B, `abcd2.ics`, carrying the second overridden component that section 7.8.1's own response for that resource shows and section 7.8.2's narration counts.
    const RECURRING: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Corp.//CalDAV Client//EN\r\n\
BEGIN:VTIMEZONE\r\n\
LAST-MODIFIED:20040110T032845Z\r\n\
TZID:US/Eastern\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20000404T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4\r\n\
TZNAME:EDT\r\n\
TZOFFSETFROM:-0500\r\n\
TZOFFSETTO:-0400\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20001026T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
TZNAME:EST\r\n\
TZOFFSETFROM:-0400\r\n\
TZOFFSETTO:-0500\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART;TZID=US/Eastern:20060102T120000\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
SUMMARY:Event #2\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART;TZID=US/Eastern:20060104T140000\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;TZID=US/Eastern:20060104T120000\r\n\
SUMMARY:Event #2 bis\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
DTSTAMP:20060206T001121Z\r\n\
DTSTART;TZID=US/Eastern:20060106T140000\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;TZID=US/Eastern:20060106T120000\r\n\
SUMMARY:Event #2 bis bis\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// RFC 4791 section 7.8.1 supplies the expected selection.
    /// The result also keeps `PRODID`.
    const SELECTED: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Corp.//CalDAV Client//EN\r\n\
BEGIN:VTIMEZONE\r\n\
LAST-MODIFIED:20040110T032845Z\r\n\
TZID:US/Eastern\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20000404T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4\r\n\
TZNAME:EDT\r\n\
TZOFFSETFROM:-0500\r\n\
TZOFFSETTO:-0400\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20001026T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
TZNAME:EST\r\n\
TZOFFSETFROM:-0400\r\n\
TZOFFSETTO:-0500\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
DTSTART;TZID=US/Eastern:20060102T120000\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\n\
SUMMARY:Event #2\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
DTSTART;TZID=US/Eastern:20060104T140000\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;TZID=US/Eastern:20060104T120000\r\n\
SUMMARY:Event #2 bis\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
DTSTART;TZID=US/Eastern:20060106T140000\r\n\
DURATION:PT1H\r\n\
RECURRENCE-ID;TZID=US/Eastern:20060106T120000\r\n\
SUMMARY:Event #2 bis bis\r\n\
UID:00959BC664CA650E933C892C@example.com\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// Three periods from RFC 5545 section 3.8.2.6 on `1997-03-08` in UTC.
    const BUSY: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Corp.//CalDAV Client//EN\r\n\
BEGIN:VFREEBUSY\r\n\
UID:19970901T115957Z-76A912@example.com\r\n\
DTSTAMP:19970901T120000Z\r\n\
FREEBUSY:19970308T160000Z/PT3H,19970308T200000Z/PT1H,19970308T230000Z/1997\r\n\
\x200309T000000Z\r\n\
END:VFREEBUSY\r\n\
END:VCALENDAR\r\n";

    /// The calendar that survives a selection naming a component type the resource has none of:
    /// the two properties that keep it readable, and nothing else.
    const EMPTIED: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Corp.//CalDAV Client//EN\r\n\
END:VCALENDAR\r\n";

    /// One UTC period from `16:00` to `19:00` on `1997-03-08` for testing boundaries.
    const ONE_PERIOD: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VFREEBUSY\r\n\
FREEBUSY:19970308T160000Z/PT3H\r\n\
END:VFREEBUSY\r\n\
END:VCALENDAR\r\n";

    /// A valid period shares a property with an unreadable field.
    const UNPLACEABLE: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VFREEBUSY\r\n\
FREEBUSY:not-a-period,19970308T160000Z/PT3H\r\n\
END:VFREEBUSY\r\n\
END:VCALENDAR\r\n";

    /// The document those octets parse to.
    fn parsed(octets: &[u8]) -> Document {
        Document::parse(octets, Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// A selection over a stored calendar that has had nothing taken out of it yet.
    fn stored(octets: &[u8]) -> Selection {
        Selection::new(parsed(octets), Reduction::FAITHFUL)
    }

    /// A ledger generous enough that nothing but the test about ledgers reaches it.
    fn ledger() -> Meter {
        Meter::new(Limits::DEFAULT)
    }

    /// Whether `octets` carries `run` anywhere inside it.
    fn holds(octets: &[u8], run: &[u8]) -> bool {
        octets.windows(run.len()).any(|window| window == run)
    }

    /// A `CALDAV:comp` element naming `name`, asking for `props` and holding `children`.
    fn comp(name: &[u8], props: &[&[u8]], children: Vec<CompSelection>) -> CompSelection {
        let mut meter = ledger();
        let mut built = CompSelection::new(name, Limits::DEFAULT, &mut meter).unwrap();
        for wanted in props {
            built.push_prop(wanted, &mut meter).unwrap();
        }
        for child in children {
            built.push_comp(child, &mut meter).unwrap();
        }
        built
    }

    /// The instant a "date with UTC time" names, which is how RFC 4791 section 9.9 writes one.
    fn utc(written: &[u8]) -> Instant {
        match DateTimeValue::decode_value(written).unwrap() {
            DateTimeValue::Utc(stamp) => stamp.at_offset(UtcOffset::UTC).unwrap(),
            other => panic!("{written:?} is not a date with UTC time: {other:?}"),
        }
    }

    /// The window `from ..< until`, both bounds stated.
    fn window(from: &[u8], until: &[u8]) -> TimeRange {
        TimeRange::new(Some(utc(from)), Some(utc(until))).unwrap()
    }

    /// Return complete resources when the request omits the component selector under [RFC 4791 section 9.6](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6).
    #[test]
    fn an_absent_comp_element_returns_the_resource_in_its_entirety() {
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            None,
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), RECURRING);
        assert!(answer.reduction().is_faithful());
        assert_eq!(answer.reduction().code(), None);
    }

    /// The RFC 4791 section 7.8.1 selector requests ten event properties and a whole `VTIMEZONE`.
    /// The response omits `DTSTAMP` and keeps `PRODID` under the local selection policy.
    #[test]
    fn a_comp_tree_keeps_what_it_names_and_leaves_the_calendar_readable() {
        let event = comp(
            b"VEVENT",
            &[
                b"SUMMARY",
                b"UID",
                b"DTSTART",
                b"DTEND",
                b"DURATION",
                b"RRULE",
                b"RDATE",
                b"EXRULE",
                b"EXDATE",
                b"RECURRENCE-ID",
            ],
            Vec::new(),
        );
        let wanted = comp(
            b"VCALENDAR",
            &[b"VERSION"],
            vec![event, comp(b"VTIMEZONE", &[], Vec::new())],
        );
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), SELECTED);
        assert!(answer.reduction().components_dropped, "the DTSTAMPs went");
        assert_eq!(
            answer.reduction().code(),
            Some(DiagnosticCode::QueryCalendarDataReduced)
        );
    }

    #[test]
    fn expansion_admits_instance_copies_before_normalizing_their_payload() {
        let payload = "x".repeat(1024);
        let wire = alloc::format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20240101T090000Z\r\n\
             RRULE:FREQ=DAILY;COUNT=3\r\nDESCRIPTION:{payload}\r\nX-CHECK;TZID=Unused:value\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let limits = Limits::DEFAULT;
        let doc = Document::parse(wire.as_bytes(), limits, &mut IgnoreDiagnostics).unwrap();
        let source = Selection::new(doc, Reduction::FAITHFUL);
        let zone = crate::internal::tz::FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let range = window(b"20240101T000000Z", b"20240104T000000Z");
        for (units, expected) in [
            (2048, QueryError::Limit(LimitExceeded::Budget)),
            (100_000, QueryError::Unrepresentable),
        ] {
            let mut meter = Meter::with_budget(limits, units);
            let result = super::expand_calendar(
                &source,
                range,
                crate::internal::query::Zones::new(&zone),
                &mut Budget::new(limits, &mut meter),
            );
            assert_eq!(result.unwrap_err(), expected);
        }
    }

    /// A selector for an absent component type keeps the calendar envelope.
    #[test]
    fn a_selection_naming_nothing_the_resource_carries_returns_an_empty_calendar() {
        let wanted = comp(
            b"VCALENDAR",
            &[],
            vec![comp(b"VTODO", &[b"SUMMARY"], Vec::new())],
        );
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), EMPTIED);
        assert!(answer.reduction().components_dropped);
    }

    /// RFC 4791 section 9.6.1 requires alternatives even on branches the calendar never reaches.
    #[test]
    fn a_selection_that_states_everything_and_names_things_beside_it_is_refused() {
        let mut both_props = comp(b"VCALENDAR", &[b"VERSION"], Vec::new());
        both_props.all_props = true;

        let mut both_comps = comp(b"VCALENDAR", &[], vec![comp(b"VEVENT", &[], Vec::new())]);
        both_comps.all_comps = true;

        let mut buried_alarm = comp(b"VALARM", &[b"ACTION"], Vec::new());
        buried_alarm.all_props = true;
        let buried = comp(
            b"VCALENDAR",
            &[],
            vec![comp(b"VTODO", &[], vec![buried_alarm])],
        );

        for wanted in [both_props, both_comps, buried] {
            let mut meter = ledger();
            assert_eq!(
                select(
                    &stored(RECURRING),
                    Some(&wanted),
                    &mut Budget::new(Limits::DEFAULT, &mut meter)
                ),
                Err(QueryError::SelectionContradiction)
            );
        }
    }

    /// Requesting all properties and all components can preserve the complete resource under RFC 4791 sections 9.6.2 and 9.6.3.
    #[test]
    fn a_selection_asking_for_everything_leaves_nothing_out() {
        let mut wanted = comp(b"VCALENDAR", &[], Vec::new());
        wanted.all_props = true;
        wanted.all_comps = true;
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), RECURRING);
        assert!(answer.reduction().is_faithful());
    }

    /// A surviving `TZID` keeps its referenced definition even when the selector omits `VTIMEZONE`.
    #[test]
    fn a_time_zone_a_surviving_tzid_names_comes_back_unasked() {
        let wanted = comp(
            b"VCALENDAR",
            &[],
            vec![comp(b"VEVENT", &[b"DTSTART", b"UID"], Vec::new())],
        );
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(
            holds(&octets, b"TZID:US/Eastern"),
            "the zone the kept DTSTART names is in the answer"
        );
        assert!(
            holds(&octets, b"BEGIN:DAYLIGHT"),
            "and it is the whole VTIMEZONE rather than a shell of one"
        );
        assert!(
            !holds(&octets, b"DURATION"),
            "while what the selection did not name is gone"
        );
    }

    /// A zone nothing refers to any more stays out: the rescue is for a `TZID` still named, and never a floor under every reduction.
    #[test]
    fn a_time_zone_nothing_refers_to_stays_out() {
        let wanted = comp(
            b"VCALENDAR",
            &[],
            vec![comp(b"VEVENT", &[b"UID"], Vec::new())],
        );
        let mut meter = ledger();
        let answer = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(!holds(&octets, b"BEGIN:VTIMEZONE"));
        assert!(answer.reduction().components_dropped);
    }

    /// The RFC 4791 section 7.8.2 range keeps the first override and the master's rule.
    /// The second override falls outside the range.
    #[test]
    fn limiting_a_recurrence_set_keeps_the_master_its_rule_and_what_impacts_the_range() {
        let mut meter = ledger();
        let answer = limit_recurrence_set(
            &stored(RECURRING),
            |overridden| {
                Match::of(
                    overridden
                        .properties_named(&PropertyId::RECURRENCE_ID)
                        .any(|line| line.value_text().as_bytes().starts_with(b"20060104")),
                )
            },
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(
            holds(&octets, b"RRULE:FREQ=DAILY;COUNT=5"),
            "the master keeps the rule this reduction does not expand"
        );
        assert!(holds(&octets, b"SUMMARY:Event #2 bis\r\n"));
        assert!(
            !holds(&octets, b"SUMMARY:Event #2 bis bis"),
            "the second overridden component is not returned"
        );
        assert!(answer.reduction().instances_dropped);
        assert!(
            !answer.reduction().components_dropped,
            "and no property was touched"
        );
    }

    /// Unknown overlap keeps the override in the result.
    #[test]
    fn an_undecidable_override_is_returned_rather_than_left_out() {
        let mut meter = ledger();
        let answer = limit_recurrence_set(
            &stored(RECURRING),
            |_overridden| Match::Undecided(Undecided::ZoneUnstated),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), RECURRING);
        assert!(
            answer.reduction().is_faithful(),
            "nothing was left out, so nothing is claimed to have been"
        );
    }

    /// Only the middle fixture period intersects the half-open `20:00` to `23:00` window.
    #[test]
    fn limiting_a_freebusy_set_keeps_only_the_periods_that_intersect() {
        let mut meter = ledger();
        let answer = limit_freebusy_set(
            &stored(BUSY),
            window(b"19970308T200000Z", b"19970308T230000Z"),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(
            holds(&octets, b"FREEBUSY:19970308T200000Z/PT1H\r\n"),
            "what stays is the octets the producer wrote for it: {octets:?}"
        );
        assert!(!holds(&octets, b"19970308T160000Z"));
        assert!(!holds(&octets, b"19970308T230000Z"));
        assert!(answer.reduction().instances_dropped);
    }

    /// RFC 4791 section 9.9, the `VFREEBUSY` row: `(start < freebusy-period-end) AND (end > freebusy-period-start)`.
    /// Both comparisons are strict, so a period touching the window at a point is outside it.
    /// The period under test runs from 16:00 to 19:00.
    #[test]
    fn the_edges_of_the_window_are_the_ones_section_nine_nine_writes() {
        // The window, and whether the period survives it.
        let cases: [(&[u8], &[u8], bool); 5] = [
            // The window ends where the period starts: `end > freebusy-period-start` fails.
            (b"19970308T130000Z", b"19970308T160000Z", false),
            // The window starts where the period ends: `start < freebusy-period-end` fails.
            (b"19970308T190000Z", b"19970308T200000Z", false),
            // Starting together, which both comparisons admit.
            (b"19970308T160000Z", b"19970308T170000Z", true),
            // Ending together, which both comparisons admit.
            (b"19970308T180000Z", b"19970308T190000Z", true),
            // Nowhere near it.
            (b"19970308T120000Z", b"19970308T130000Z", false),
        ];

        for (from, until, kept) in cases {
            let mut meter = ledger();
            let answer = limit_freebusy_set(
                &stored(ONE_PERIOD),
                window(from, until),
                &mut Budget::new(Limits::DEFAULT, &mut meter),
            )
            .unwrap();
            let rendered = answer.calendar().to_bytes();
            let held = holds(&rendered, b"FREEBUSY:");
            assert_eq!(held, kept, "the window {from:?} to {until:?}: {rendered:?}");
            assert_eq!(answer.reduction().instances_dropped, !kept);
        }
    }

    /// An unreadable period has unknown overlap.
    #[test]
    fn a_freebusy_period_that_cannot_be_placed_is_kept() {
        let mut meter = ledger();
        let answer = limit_freebusy_set(
            &stored(UNPLACEABLE),
            window(b"19970308T190000Z", b"19970308T200000Z"),
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(
            holds(&octets, b"FREEBUSY:not-a-period\r\n"),
            "what could not be read stays, and what could be read and fell outside goes"
        );
        assert!(answer.reduction().instances_dropped);
    }

    /// For `novalue="yes"`, keep the name, parameters, and trailing colon under [RFC 4791 section 9.6.4](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.4).
    #[test]
    fn a_property_returned_without_its_value_keeps_its_name_and_its_parameters() {
        let mut meter = ledger();
        let answer = without_values(
            &stored(RECURRING),
            &[b"DTSTART", b"SUMMARY"],
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        let octets = answer.calendar().to_bytes();
        assert!(
            holds(&octets, b"DTSTART;TZID=US/Eastern:\r\nDURATION:PT1H"),
            "the parameters stay, the value goes, and the line is still one line"
        );
        assert!(holds(&octets, b"SUMMARY:\r\nUID:"));
        assert!(!holds(&octets, b"20060102T120000"), "no value came back");
        assert!(
            holds(&octets, b"DTSTART:\r\nRRULE:FREQ=YEARLY"),
            "a DTSTART inside a VTIMEZONE is named too: the request names properties and not \
             positions"
        );
        assert!(answer.reduction().components_dropped);
    }

    /// Naming no property asks for nothing: the calendar comes back as it stands, with the witness it arrived with.
    #[test]
    fn naming_no_property_leaves_every_value_in_place() {
        let mut meter = ledger();
        let answer = without_values(
            &stored(BUSY),
            &[],
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        )
        .unwrap();
        assert_eq!(answer.calendar().to_bytes(), BUSY);
        assert!(answer.reduction().is_faithful());
    }

    /// Each selection step preserves earlier reduction flags.
    #[test]
    fn the_witness_carries_what_every_step_of_the_chain_left_out() {
        let mut meter = ledger();
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let expanded = Selection::new(
            parsed(BUSY),
            Reduction {
                components_dropped: false,
                instances_dropped: false,
                expanded: true,
            },
        );
        let thinned = limit_freebusy_set(
            &expanded,
            window(b"19970308T200000Z", b"19970308T230000Z"),
            &mut budget,
        )
        .unwrap();
        let answer = without_values(&thinned, &[b"UID"], &mut budget).unwrap();
        assert_eq!(
            answer.reduction(),
            Reduction {
                components_dropped: true,
                instances_dropped: true,
                expanded: true,
            }
        );
        assert!(!answer.reduction().is_faithful());
    }

    /// The caller's byte budget must cover the selected calendar.
    #[test]
    fn a_ledger_too_small_to_hold_the_answer_refuses() {
        let mut meter = Meter::with_budget(Limits::DEFAULT, 64);
        let outcome = select(
            &stored(RECURRING),
            None,
            &mut Budget::new(Limits::DEFAULT, &mut meter),
        );
        assert_eq!(outcome, Err(QueryError::Limit(LimitExceeded::Budget)));
        assert!(meter.is_exhausted(), "and the refusal latches");
    }

    /// An explicit traversal stack enforces the caller's component depth limit.
    #[test]
    fn a_calendar_nested_past_the_callers_bound_is_refused() {
        let limits = Limits::DEFAULT.with_max_component_depth(1);
        let mut wanted = comp(b"VCALENDAR", &[], vec![comp(b"VEVENT", &[], Vec::new())]);
        wanted.all_props = true;
        let mut meter = ledger();
        let outcome = select(
            &stored(RECURRING),
            Some(&wanted),
            &mut Budget::new(limits, &mut meter),
        );
        assert_eq!(outcome, Err(QueryError::Limit(LimitExceeded::Depth)));
    }

    /// The manifest is the review order, so it names every passage this file transcribes.
    #[test]
    fn the_manifest_names_the_passages_this_file_was_written_from() {
        assert_eq!(SUBSELECTION_SECTIONS.len(), 11);
        assert!(
            SUBSELECTION_SECTIONS
                .iter()
                .all(|row| row.starts_with("RFC 4791 section ") || row.starts_with("RFC 5545 ")),
            "every row names the document and the section it was read from"
        );
        assert!(
            SUBSELECTION_SECTIONS
                .iter()
                .any(|row| row.contains("section 9.6.4, CALDAV:prop and novalue")),
            "novalue is section 9.6.4 and allprop is 9.6.3, whatever this file's seed said"
        );
    }
}
