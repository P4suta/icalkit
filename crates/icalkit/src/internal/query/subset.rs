// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 7 — `calendar-data` sub-selection. RFC 4791 sections 9.6, 9.6.1 through 9.6.4, 9.6.6
//! and 9.6.7.
//!
//! **This is the one place `docs/adr/0001`'s round trip is deliberately broken, and a caller
//! that does not know it will destroy somebody's data.** `CALDAV:comp`, `CALDAV:expand` and
//! `CALDAV:limit-recurrence-set` each answer with a calendar that is *not* what the server
//! stored. The octets are well-formed iCalendar and say nothing about being a reduction, so a
//! client that reads a `calendar-multiget` response and `PUT`s it back deletes every component,
//! property and override the selection left out — with a changed `ETag` as the only trace, which
//! is the exact failure this workspace exists to make structurally impossible.
//!
//! Everything this unit returns therefore travels as a [`Selection`]: the calendar and a
//! [`Reduction`] in one value, never a bare `Document`. A reduction reports itself to the
//! caller's diagnostic sink as `DiagnosticCode::QueryCalendarDataReduced`. Every public function
//! below repeats it, because the caller who needs to read it will not read this file.
//!
//! # The chain, and why every door takes a [`Selection`] rather than a document
//!
//! RFC 4791 section 9.6 writes the request as `calendar-data (comp?, (expand |
//! limit-recurrence-set)?, limit-freebusy-set?)`, so honoring one means applying up to three
//! reductions in that order. Each door here takes the answer the previous one gave and hands on
//! a witness carrying what *both* left out, so the fact that a calendar is a reduction cannot be
//! lost between two steps of the chain that produced it. A caller starts the chain with
//! `Selection::new(stored, Reduction::FAITHFUL)` and plugs `expand`'s answer in as the
//! [`Selection`] it already is.
//!
//! # What this unit owns
//!
//! - **The absent `comp` element (section 9.6).** "If the CALDAV:calendar-data XML element
//!   doesn't contain any CALDAV:comp element, calendar object resources will be returned in
//!   their entirety." That is [`select`] with no selection, and it is the only shape
//!   `docs/adr/0001`'s round trip survives.
//! - **`CALDAV:comp` (section 9.6.1).** A tree of component names, each carrying either
//!   `allprop` or a list of `prop` names, and either `allcomp` or a list of nested `comp`s. The
//!   grammar is `comp ((allprop | prop*), (allcomp | comp*))`, so the halves of each pair are
//!   alternatives: a value holding both is [`QueryError::SelectionContradiction`] and never
//!   silently reduced to one of them. `ical-dav` models the contradiction rather than refusing
//!   it, so this unit is where it is refused — and it is refused for the whole request tree
//!   before any calendar is read, because a query that contradicts itself does so for every
//!   resource and not only for the ones whose shape reaches the offending branch.
//! - **`CALDAV:allcomp` (section 9.6.2)** and **`CALDAV:allprop` (section 9.6.3)**.
//! - **`CALDAV:prop` (section 9.6.4)**, and its `novalue` attribute through [`without_values`]:
//!   "just the iCalendar property name and any iCalendar parameters and a trailing ':' without
//!   the subsequent value data".
//! - **`CALDAV:limit-recurrence-set` (section 9.6.6)**, which keeps the `RECURRENCE-ID`
//!   overrides that impact the window and drops the others while keeping the master component
//!   and its rule. Note the asymmetry with `expand`: this one keeps the rule.
//! - **`CALDAV:limit-freebusy-set` (section 9.6.7)**, the same for `VFREEBUSY` periods, decided
//!   by the `VFREEBUSY` row of section 9.9.
//! - `CALDAV:expand` is `expand`'s to generate and this unit's to place, which it does by taking
//!   the [`Selection`] that unit produced as the source of the next reduction.
//!
//! # What must survive a reduction, and where that departs from the specification
//!
//! A selected subtree still has to be a calendar somebody can read: `BEGIN:VCALENDAR`,
//! `VERSION`, `PRODID` and the `VTIMEZONE` components any surviving `TZID` refers to. Dropping a
//! `VTIMEZONE` whose `TZID` a kept `DTSTART` still names produces a calendar no reader can place
//! on a timeline, which is a worse answer than returning too much.
//!
//! Read strictly, section 9.6.1 does not agree: a `comp` element "defines which component types
//! to return", and section 9.6 states outright that what comes back "MAY be invalid per their
//! media type specification if the CALDAV:calendar-data XML element part of the calendaring
//! REPORT request did not specify required properties (e.g., UID, DTSTAMP, etc.)". The
//! specification's own worked examples then contradict each other about it: in section 7.8.1 one
//! request naming `<C:prop name="VERSION"/>` and no `PRODID` comes back for `abcd2.ics` without
//! a `PRODID` and for `abcd3.ics` with one. Both readings have a citation, only one of them
//! answers with a document a reader can parse, and this unit takes that one. It returns more
//! than the client asked for and never less, and every departure is in this paragraph.
//!
//! The same reading settles the empty `comp` element, which the grammar permits and section
//! 9.6.1 says nothing about: `<C:comp name="VTIMEZONE"/>` names no property and no
//! subcomponent, and section 7.8.1's response returns that `VTIMEZONE` whole. A component whose
//! selection names nothing at all is therefore returned entire rather than as an empty shell.
//!
//! # What it must not do
//!
//! Rebuild a property from its parsed form. `ical-core` preserves the text of every line it
//! read, and a selection that keeps a property keeps *that* text — anything else re-encodes a
//! value the server stored and turns a reduction into an edit of the parts that were kept. The
//! two doors that must author a line — a property returned without its value, and a `FREEBUSY`
//! whose periods were thinned — assemble it out of the octets the producer wrote, and are the
//! only places in this file that reach a constructor at all.
//!
//! # What it cannot do, because the request cannot say it
//!
//! `ical-dav`'s `CompSelection` carries a property's *name* and nothing else, so section
//! 9.6.4's `novalue` attribute has nowhere to live: a server that decoded `novalue="yes"` off
//! the wire has already lost it by the time the filter value reaches this crate. [`select`]
//! therefore cannot apply it, and [`without_values`] takes the names beside the selection
//! instead. That is a gap in `ical-dav` rather than a decision here, and the day `CompSelection`
//! carries the attribute this door reads it off the selection and loses its second argument.

use alloc::vec::{IntoIter, Vec};
use core::{mem, slice};

use ical_core::{
    CivilDateTime, Component, DateTimeValue, DecodeValue, Document, Duration, EncodeValue, Instant,
    Item, LimitExceeded, Limits, Meter, Period, Property, PropertyId, UtcOffset, ValueBuf,
};
use ical_dav::{CompSelection, TimeRange};

use crate::internal::query::expand::{
    component_start, expand_component, override_impacts, override_recurrence_id,
};
use crate::internal::query::{Budget, Match, QueryError, Reduction, Selection, Undecided, Zones};

/// The identity [`zone_id`] looks up.
///
/// `Component::properties_named` ties the identity's lifetime to the iterator it returns, so a
/// reference to the associated constant would be a temporary whose lifetime is too short for
/// the value borrowed from the matching property.
static TZID: PropertyId = PropertyId::TZID;

/// The identity [`component_uid`] looks up.
static UID: PropertyId = PropertyId::UID;

/// What calendar-data sub-selection is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
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

/// The component a `CALDAV:comp` tree is rooted at, and the one this unit keeps readable.
const CALENDAR: &[u8] = b"VCALENDAR";

/// The component a `TZID` names, RFC 5545 section 3.6.5.
const TIME_ZONE: &[u8] = b"VTIMEZONE";

/// The component whose `FREEBUSY` periods section 9.6.7 thins, RFC 5545 section 3.6.4.
const FREE_BUSY: &[u8] = b"VFREEBUSY";

/// Apply a `CALDAV:comp` tree to a calendar, RFC 4791 sections 9.6 and 9.6.1 through 9.6.4.
///
/// **What comes back is not what the server stored.** The octets are well-formed iCalendar and
/// say nothing about the components and properties that were left out, so a caller that writes
/// them back deletes every one of them, with a changed `ETag` as the only trace. That is why
/// this answers a [`Selection`] and never a bare document: the [`Reduction`] beside the calendar
/// is the only record that the round trip was broken, and a reduction is reported to a
/// diagnostic sink as `DiagnosticCode::QueryCalendarDataReduced`.
///
/// `wanted` is `None` for a request carrying no `CALDAV:comp` element, which section 9.6 returns
/// "in their entirety" — the witness `source` arrived with is handed back unchanged, because
/// this call left nothing out.
///
/// Otherwise the selection is walked beside the calendar: a component survives when the
/// selection at its level names it, its properties are the ones that level names, and both
/// halves of section 9.6.1's grammar behave as the alternatives it writes them as. What survives
/// beyond that is stated in this module's own documentation, which a reader comparing this
/// against RFC 4791 has to read: `VERSION` and `PRODID` on the calendar, every `VTIMEZONE` a
/// surviving `TZID` still names, and a component whose selection names nothing at all.
///
/// Section 9.6.4's `novalue` attribute is not applied here and cannot be; see
/// [`without_values`].
///
/// # Errors
///
/// [`QueryError::SelectionContradiction`] for a selection anywhere in the tree that states
/// `allprop` beside a `prop`, or `allcomp` beside a `comp`: section 9.6.1 writes the two halves
/// as alternatives, so a value holding both is one no request body expresses. The whole tree is
/// checked before the calendar is read, so one query is refused for every resource or for none.
/// [`QueryError::Limit`] where the calendar being built crosses the caller's octet budget, or
/// where it nests past `Limits::max_component_depth`.
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
        // Two entries are named by no `comp` element at any level and are left out together: a
        // content line outside every component, which is what a stream carries before its first
        // `BEGIN`, and a component the root of the selection does not name.
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

/// Return the named properties without their values, RFC 4791 section 9.6.4's `novalue="yes"`.
///
/// **What comes back is not what the server stored**, for the reason [`select`] states in full:
/// a property stripped of its value is a property whose value a client writing this calendar
/// back deletes. The [`Reduction`] beside the calendar is the only record of it.
///
/// Section 9.6.4: "the server will return just the iCalendar property name and any iCalendar
/// parameters and a trailing ':' without the subsequent value data". The name and the parameters
/// keep the octets their producer wrote; only the line's recorded fold layout goes, because the
/// folds were positions into text that no longer exists.
///
/// The names are an argument rather than a flag read off the selection because `ical-dav`'s
/// `CompSelection` cannot carry `novalue` — see this module's own documentation. A caller that
/// did not decode the attribute passes `&[]` and gets its source back unchanged.
///
/// # Errors
///
/// [`QueryError::Unrepresentable`] for a property whose name RFC 5545 section 3.1 cannot write
/// back as one content line — a line the reader stored because a file held it, and which this
/// crate declines to author a copy of. [`QueryError::Limit`] where the calendar being built
/// crosses the caller's octet budget or nests past `Limits::max_component_depth`.
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
    let mut copy = source.calendar().clone();
    let built = map_properties(
        mem::take(copy.items_mut()),
        limits,
        meter,
        &mut |_holder, line, ledger| {
            if names.iter().any(|name| line.is_named(name)) {
                stripped = true;
                return without_value(&line, ledger).map(Some);
            }
            charge_line(&line, ledger)?;
            Ok(Some(line))
        },
    )?;
    let reduction = widened(source.reduction(), stripped, false);
    Ok(Selection::new(Document::new(built), reduction))
}

/// Keep the master component and the overrides `decide` admits, RFC 4791 section 9.6.6.
///
/// **What comes back is not what the server stored.** A calendar missing the overrides that fell
/// outside the window is well-formed iCalendar that says nothing about them, so a client writing
/// it back deletes them. The [`Reduction`] beside it is the only record, and it reports itself
/// as `DiagnosticCode::QueryCalendarDataReduced`.
///
/// Section 9.6.6: the server "MUST return, in addition to the 'master component', only the
/// 'overridden components' that impact a specified time range". Which components those two names
/// pick out is structural and is this unit's: a component carrying a `RECURRENCE-ID` is an
/// overridden one, and everything else — the master, a non-recurring component, a `VTIMEZONE` —
/// is kept unconditionally. Whether one *impacts* the range is not structural and is not this
/// unit's: section 9.6.6 counts an override whose current start and end overlap the range, an
/// override whose original start and end would have, and an override whose `RANGE` parameter
/// makes it govern instances inside the range. All three need expansion and zone resolution, so
/// the answer arrives through `decide`, which is called once per overridden component.
///
/// An override `decide` cannot place is kept: only [`Match::Unmatched`] drops one, and
/// [`Match::Undecided`] returns it. A resource whose overlap could not be decided is one nothing
/// has established anything about, and dropping it would report an absence nobody established —
/// the same invention [`Match`]'s third value exists to refuse. The caller reads the reason off
/// the answer it produced.
///
/// Unlike `CALDAV:expand`, this keeps the recurrence rule: nothing here touches a property, so
/// the master's `RRULE`, `RDATE` and `EXDATE` come back as they were stored.
///
/// # Errors
///
/// [`QueryError::Limit`] where the calendar being built crosses the caller's octet budget.
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

/// Keep only overrides whose current or original occurrence impacts `window`.
///
/// This is the timeline-aware composition used by the facade. Decisions are made against the
/// complete stored calendar before the structural pass copies it, so a later component/property
/// selection cannot erase the UID, recurrence ID, or start needed to decide safely.
pub fn limit_recurrence_set_in_window(
    source: &Selection,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let mut decisions = Vec::new();
    for calendar in source.calendar().components() {
        let components: Vec<&Component> = calendar.components().collect();
        for candidate in components
            .iter()
            .copied()
            .filter(|component| is_override(component))
        {
            let answer = match components
                .iter()
                .copied()
                .find(|master| is_override_of(candidate, master))
            {
                Some(master) => override_impacts(master, candidate, window, zones, budget)?,
                None => Match::Undecided(Undecided::OverlapUndefined),
            };
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
            Item::Property(property) => items.push(Item::Property(property.clone())),
            Item::Component(component) if component.is_named(CALENDAR) => {
                items.push(Item::Component(expand_calendar_component(
                    component, window, zones, budget,
                )?));
            },
            Item::Component(component) => items.push(Item::Component(component.clone())),
        }
    }
    let calendar = Document::new(items);
    budget
        .meter
        .try_charge_bytes(u64::try_from(calendar.to_bytes().len()).unwrap_or(u64::MAX))
        .map_err(QueryError::Limit)?;
    Ok(Selection::new(
        calendar,
        Reduction {
            expanded: true,
            ..source.reduction()
        },
    ))
}

/// Expand the recurrence sets directly inside one `VCALENDAR`.
fn expand_calendar_component(
    calendar: &Component,
    window: TimeRange,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Component, QueryError> {
    let siblings = calendar.items();
    let components: Vec<&Component> = calendar.components().collect();
    let mut items = Vec::new();
    for entry in siblings {
        let Item::Component(master) = entry else {
            items.push(entry.clone());
            continue;
        };
        if is_override(master) {
            if components
                .iter()
                .copied()
                .any(|candidate| is_override_of(master, candidate))
            {
                continue;
            }
            items.push(entry.clone());
            continue;
        }
        let related: Vec<&Component> = components
            .iter()
            .copied()
            .filter(|candidate| is_override_of(candidate, master))
            .collect();
        let recurring = master.properties_named(&PropertyId::RRULE).next().is_some()
            || master.properties_named(&PropertyId::RDATE).next().is_some()
            || !related.is_empty();
        if !recurring {
            items.push(entry.clone());
            continue;
        }

        let expansion = expand_component(master, siblings, window, zones, budget)?;
        if expansion.incomplete().is_some() {
            return Err(QueryError::Unrepresentable);
        }
        let initial = component_start(master, zones).map_err(|_| QueryError::Unrepresentable)?;
        for instance in expansion.instances() {
            let template = effective_template(master, &related, *instance, zones)?;
            items.push(Item::Component(materialize_instance(
                &template, *instance, initial,
            )?));
        }
    }
    Ok(Component::new(
        calendar.begin().clone(),
        items,
        calendar.end().cloned(),
    ))
}

/// Compose every range anchor in force and then the exact override, if one exists.
fn effective_template(
    master: &Component,
    related: &[&Component],
    instance: crate::internal::query::Instance,
    zones: Zones<'_>,
) -> Result<Component, QueryError> {
    let mut anchors: Vec<(Instant, &Component)> = related
        .iter()
        .copied()
        .filter(|candidate| is_range_anchor(candidate))
        .map(|candidate| {
            override_recurrence_id(master, candidate, zones)
                .map(|recurrence_id| (recurrence_id, candidate))
                .map_err(|_| QueryError::Unrepresentable)
        })
        .collect::<Result<_, _>>()?;
    anchors.sort_by_key(|(recurrence_id, _)| *recurrence_id);

    let mut built = master.clone();
    for (_, anchor) in anchors
        .into_iter()
        .filter(|(recurrence_id, _)| *recurrence_id <= instance.recurrence_id())
    {
        overlay_properties(&mut built, anchor);
    }
    if let Some(exact) = related.iter().copied().find(|candidate| {
        override_recurrence_id(master, candidate, zones).ok() == Some(instance.recurrence_id())
    }) {
        overlay_properties(&mut built, exact);
    }
    Ok(built)
}

/// Apply the properties one override states, leaving omitted base properties alone.
fn overlay_properties(target: &mut Component, overlay: &Component) {
    let mut names: Vec<&[u8]> = Vec::new();
    for property in overlay.properties() {
        if is_expansion_control(property) || names.iter().any(|name| property.is_named(name)) {
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
            insert_property(target, property.clone());
        }
    }
}

/// Properties rebuilt from an [`Instance`](crate::internal::query::Instance), never copied as an override diff.
fn is_expansion_control(property: &Property) -> bool {
    [
        &PropertyId::RRULE,
        &PropertyId::RDATE,
        &PropertyId::EXDATE,
        &PropertyId::RECURRENCE_ID,
        &PropertyId::DTSTART,
        &PropertyId::DTEND,
        &PropertyId::DURATION,
    ]
    .iter()
    .any(|id| property.has_id(id))
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
    template: &Component,
    instance: crate::internal::query::Instance,
    initial: Instant,
) -> Result<Component, QueryError> {
    let mut built = template.clone();
    built.items_mut().retain(|entry| {
        entry
            .as_property()
            .is_none_or(|property| !is_expansion_control(property))
    });
    insert_property(&mut built, utc_property(b"DTSTART", instance.start())?);
    insert_property(&mut built, utc_property(b"DTEND", instance.end())?);
    if instance.recurrence_id() != initial {
        insert_property(
            &mut built,
            utc_property(b"RECURRENCE-ID", instance.recurrence_id())?,
        );
    }
    Ok(built)
}

/// One canonical UTC date-time property.
fn utc_property(name: &[u8], instant: Instant) -> Result<Property, QueryError> {
    let stamp =
        CivilDateTime::from_instant(instant, UtcOffset::UTC).ok_or(QueryError::Unrepresentable)?;
    let mut value = ValueBuf::new();
    DateTimeValue::Utc(stamp)
        .encode_value(&mut value)
        .map_err(|_| QueryError::Unrepresentable)?;
    Property::create(name, Vec::new(), value.as_bytes()).map_err(|_| QueryError::Unrepresentable)
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

/// Keep the `FREEBUSY` periods that intersect `window`, RFC 4791 section 9.6.7.
///
/// **What comes back is not what the server stored.** A `VFREEBUSY` whose periods were thinned
/// is well-formed iCalendar stating less busy time than the collection holds, and a client that
/// writes it back deletes the rest — while a client reading the gap as free double-books
/// somebody. The [`Reduction`] beside the calendar is the only record of either.
///
/// Section 9.6.7: the server "MUST only return the FREEBUSY property values of a VFREEBUSY
/// component that intersects a specified time range", using "the same logic as defined for
/// CALDAV:time-range". Section 9.9 writes that logic for a free-busy period as `(start <
/// freebusy-period-end) AND (end > freebusy-period-start)`, with a missing `start` read as minus
/// infinity and a missing `end` as plus infinity. It is a rule about the *values* of one
/// property rather than about the property, so a `FREEBUSY` listing three periods of which one
/// intersects comes back listing that one, in the octets its producer wrote, and a `FREEBUSY`
/// none of whose periods intersect is dropped.
///
/// A period this crate cannot place on the timeline is kept. RFC 5545 section 3.8.2.6 requires
/// the values of a `FREEBUSY` to be UTC date-times, so one that is floating, zoned or
/// undecodable violates the specification — and a comparison that never happened establishes
/// nothing, so dropping it would state that the time is free on a guess.
///
/// # Errors
///
/// [`QueryError::Unrepresentable`] where the thinned `FREEBUSY` cannot be written back as one
/// content line. [`QueryError::Limit`] where the calendar being built crosses the caller's octet
/// budget or nests past `Limits::max_component_depth`.
pub fn limit_freebusy_set(
    source: &Selection,
    window: TimeRange,
    budget: &mut Budget<'_>,
) -> Result<Selection, QueryError> {
    let limits = budget.limits;
    let meter = &mut *budget.meter;
    let mut trimmed = false;
    let mut copy = source.calendar().clone();
    let built = map_properties(
        mem::take(copy.items_mut()),
        limits,
        meter,
        &mut |holder, line, ledger| {
            if !holder.eq_ignore_ascii_case(FREE_BUSY) || !line.has_id(&PropertyId::FREEBUSY) {
                charge_line(&line, ledger)?;
                return Ok(Some(line));
            }
            let (kept, thinned) = kept_periods(line.value_text().as_bytes(), window);
            if !thinned {
                charge_line(&line, ledger)?;
                return Ok(Some(line));
            }
            trimmed = true;
            if kept.is_empty() {
                return Ok(None);
            }
            with_value(&line, &kept, ledger).map(Some)
        },
    )?;
    let reduction = widened(source.reduction(), false, trimmed);
    Ok(Selection::new(Document::new(built), reduction))
}

/// The witness `earlier` carried, widened by what this pass left out.
///
/// A reduction composes by union and never by replacement: the second step of a chain has no way
/// to know what the first one dropped, and a step answering only for itself would hand a caller
/// a faithful-looking witness over a calendar two reductions deep.
const fn widened(earlier: Reduction, components: bool, instances: bool) -> Reduction {
    Reduction {
        components_dropped: earlier.components_dropped || components,
        instances_dropped: earlier.instances_dropped || instances,
        expanded: earlier.expanded,
    }
}

/// The source calendar handed back as it stands, charged for the copy that is.
fn unchanged(source: &Selection, meter: &mut Meter) -> Result<Selection, QueryError> {
    charge_document(source.calendar(), meter)?;
    Ok(source.clone())
}

/// Refuse a selection tree that states "everything" and names things beside it.
///
/// RFC 4791 section 9.6.1 writes `comp ((allprop | prop*), (allcomp | comp*))`, so each pair is
/// an alternation and a value holding both halves is one no request body expresses. `ical-dav`
/// represents it and reports it through `CompSelection::is_contradictory`; refusing it is this
/// crate's, because reducing it to one half would answer a request nobody wrote.
///
/// The whole tree is checked, branches no calendar reaches included, because a contradictory
/// query is contradictory for every resource: a refusal that depended on the data would return
/// some resources and refuse others for one and the same `REPORT`.
///
/// Iterative, because nothing bounds the height of a `CompSelection` tree at construction and a
/// walk that recursed once per level would be an attack on the stack rather than a check.
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

/// One component being selected from, while the component it copies is being walked.
#[derive(Debug)]
struct SelectFrame<'a> {
    /// The component the entries are taken from, kept for its two boundary lines.
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

    /// The component this frame built, closed by the boundary lines its source was written with.
    fn finish(self) -> Component {
        Component::new(
            self.holder.begin().clone(),
            self.items,
            self.holder.end().cloned(),
        )
    }
}

/// What a selection says about one subcomponent of the component it is applied to.
#[derive(Debug)]
enum Chosen<'a> {
    /// Returned entire: `allcomp`, or a selection that names nothing at all.
    Whole,
    /// Returned as this nested selection says.
    Under(&'a CompSelection),
    /// Named by nothing at this level, so left out.
    Excluded,
}

/// What one entry of the component being selected from contributes to the answer.
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
/// Separate from the walk that applies it, so that the rules of section 9.6.1 read as rules
/// rather than as arms of the loop that carries the stack.
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

/// Build the component `wanted` selects out of `holder`, counting what was left out.
///
/// Iterative, over an explicit stack, for the reason `ical_core::Component` gives in full: the
/// nesting depth of a parsed calendar is a bound the caller raises through a public builder, so
/// a traversal recursing once per level takes the process down on a document the reader
/// accepted. A stack overflow is an abort rather than an unwind, so a server would lose the
/// process rather than the request.
fn select_component<'a>(
    holder: &'a Component,
    wanted: &'a CompSelection,
    limits: Limits,
    meter: &mut Meter,
    dropped: &mut usize,
) -> Result<Component, QueryError> {
    let mut open: Vec<SelectFrame<'a>> = Vec::new();
    let mut current = SelectFrame::of(holder, wanted);
    loop {
        let Some(entry) = current.rest.next() else {
            // This frame's entries are done: close it and carry on where its parent left off.
            let finished = current.finish();
            let Some(parent) = open.pop() else {
                return Ok(finished);
            };
            current = parent;
            current.items.push(Item::Component(finished));
            continue;
        };
        // Decided before the match, so the borrow the rules read the frame through has ended by
        // the time an arm writes to it. The entry borrows the calendar rather than the frame, so
        // it outlives both.
        let taken = step(&current, entry, meter)?;
        match taken {
            Step::Kept(kept) => current.items.push(kept),
            Step::Left => *dropped = dropped.saturating_add(1),
            Step::Enter(nested, inner) => {
                // The component about to be entered sits `open.len() + 2` deep, counting the
                // root this walk started at as depth one.
                if open.len().saturating_add(2) > usize::from(limits.max_component_depth()) {
                    return Err(QueryError::Limit(LimitExceeded::Depth));
                }
                open.push(current);
                current = SelectFrame::of(nested, inner);
            },
        }
    }
}

/// Whether `wanted` names nothing at all, which returns the component it names entire.
///
/// RFC 4791 section 9.6.1's grammar admits `<C:comp name="VTIMEZONE"/>`, and section 7.8.1 sends
/// exactly that and gets the whole `VTIMEZONE` back. The alternative reading answers with a
/// `BEGIN`/`END` pair holding nothing, which is not a component any reader can use.
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
    // What a calendar needs in order to be read back as one at all. Section 9.6 permits the
    // answer to be invalid instead; this module's own documentation states why that permission
    // is declined.
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

/// Put back every `VTIMEZONE` a surviving `TZID` still names, and say how many came back.
///
/// A calendar whose `DTSTART` reads `TZID=US/Eastern` and whose `VTIMEZONE` the selection
/// dropped cannot be placed on a timeline by any reader, which is a worse answer than one
/// component the request did not name. What comes back is the component the resource stored,
/// whole.
///
/// The count is what keeps the witness honest: a component dropped by the selection and put back
/// here was not left out, so the caller is not told about a reduction that did not happen.
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
    // Before the first component, which is where every worked example of RFC 4791 section 7.8
    // writes a `VTIMEZONE`: under the calendar's own properties and above what refers to it.
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

/// Whether any property anywhere inside `holder` reads its value under the zone `id`.
///
/// Compared octet for octet: RFC 5545 gives no case-folding for a `TZID`, and two identifiers
/// differing in case name two rows of a database this crate does not hold.
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

/// Build one calendar's entries, dropping the overridden components `decide` excludes.
///
/// Two levels and no recursion: RFC 4791 section 9.6.6's "overridden components" are the
/// components of the recurrence set, which sit directly inside the `VCALENDAR` beside the master
/// they override. A `RECURRENCE-ID` further down belongs to something else.
fn limited_calendar<D>(
    holder: &Component,
    decide: &mut D,
    meter: &mut Meter,
    trimmed: &mut bool,
) -> Result<Component, QueryError>
where
    D: FnMut(&Component) -> Match,
{
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

/// Whether `component` is an "overridden component" as RFC 4791 section 9.6.6 names one.
///
/// A `RECURRENCE-ID` is what makes a component one instance of a series rather than the master
/// of it, RFC 5545 section 3.8.4.4. Everything without one is returned unconditionally, which is
/// what section 9.6.6's "in addition to the 'master component'" requires.
fn is_override(component: &Component) -> bool {
    component
        .properties_named(&PropertyId::RECURRENCE_ID)
        .next()
        .is_some()
}

/// Whether `candidate` is an override belonging to `master`.
fn is_override_of(candidate: &Component, master: &Component) -> bool {
    if candidate.kind() != master.kind() || is_override(master) {
        return false;
    }
    component_uid(candidate).is_some() && component_uid(candidate) == component_uid(master)
}

/// The UID octets identifying a recurrence set.
fn component_uid(component: &Component) -> Option<&[u8]> {
    component
        .properties_named(&UID)
        .next()
        .map(|property| property.value_text().as_bytes())
}

/// One component being rebuilt, while the component it came from is taken apart.
#[derive(Debug)]
struct EditFrame {
    /// The component whose entries were taken out, kept for its two boundary lines.
    shell: Component,
    /// The entries of the level above, not yet visited.
    rest: IntoIter<Item>,
    /// The entries the level above had rebuilt when this one was entered.
    items: Vec<Item>,
}

/// Rebuild `entries`, replacing every property with what `edit` answers for it.
///
/// `edit` is handed the name of the component the property sits in — empty for a line outside
/// every component — the property itself, and the ledger, and answers with the property to keep
/// or `None` to leave it out. The property arrives *by value*, so one kept unchanged is moved
/// into the answer with its recorded line syntax intact rather than rebuilt from what it decodes
/// to. The entries arrive by value for the same reason, which is why this takes what a document
/// holds rather than the document.
///
/// Iterative, over an explicit stack, for the reason [`select_component`] gives.
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
            // This level is done: close the component it belonged to and carry on where the
            // level above left off.
            let Some(frame) = open.pop() else {
                return Ok(items);
            };
            let finished = Component::new(
                frame.shell.begin().clone(),
                items,
                frame.shell.end().cloned(),
            );
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
                // The component about to be entered sits `open.len() + 1` deep, the entries of
                // the document itself being depth zero.
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

/// One property with its name and its parameters and no value, RFC 4791 section 9.6.4.
///
/// Authored rather than edited, because `ical-core` keeps no public door that replaces a value
/// in place: what survives is the octets the producer wrote for the name and for every
/// parameter, and the line's recorded fold layout goes because the folds were positions into
/// text that no longer exists.
fn without_value(line: &Property, meter: &mut Meter) -> Result<Property, QueryError> {
    charge_line(line, meter)?;
    Property::create(line.name().as_bytes(), line.parameters().to_vec(), b"")
        .map_err(|_| QueryError::Unrepresentable)
}

/// One property with its name and its parameters and `value` in place of what it held.
fn with_value(line: &Property, value: &[u8], meter: &mut Meter) -> Result<Property, QueryError> {
    charge_line(line, meter)?;
    Property::create(line.name().as_bytes(), line.parameters().to_vec(), value)
        .map_err(|_| QueryError::Unrepresentable)
}

/// The periods of one `FREEBUSY` value that `window` keeps, and whether any were left out.
///
/// RFC 5545 section 3.8.2.6 writes the value as a comma-separated list of periods, so this
/// answers in the octets the producer wrote for the periods that stay: nothing is re-encoded,
/// and a period whose spelling this crate would not have chosen comes back in the spelling it
/// arrived in.
fn kept_periods(value: &[u8], window: TimeRange) -> (Vec<u8>, bool) {
    let mut kept: Vec<u8> = Vec::new();
    let mut thinned = false;
    for written in value.split(|octet| *octet == b',') {
        if period_bounds(written).is_some_and(|(from, until)| !intersects(window, from, until)) {
            thinned = true;
            continue;
        }
        if !kept.is_empty() {
            kept.push(b',');
        }
        kept.extend_from_slice(written);
    }
    (kept, thinned)
}

/// Where one written period begins and ends, or `None` for one this crate cannot place.
///
/// `None` covers a value that does not decode and a value that is not UTC. RFC 5545 section
/// 3.8.2.6 requires the periods of a `FREEBUSY` to be UTC date-times, so a floating or zoned one
/// violates it — and placing that one would need a zone `docs/adr/0003` forbids inventing.
fn period_bounds(written: &[u8]) -> Option<(Instant, Instant)> {
    let period = Period::decode_value(written).ok()?;
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

/// How many seconds `span` lasts, or `None` where that count is not representable.
fn span_seconds(span: Duration) -> Option<i64> {
    const SECONDS_PER_DAY: i64 = 86_400;
    span.days()
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(span.seconds())
}

/// Whether the period `from ..< until` intersects `window`, RFC 4791 section 9.9.
///
/// The `VFREEBUSY` row, transcribed: `(start < freebusy-period-end) AND (end >
/// freebusy-period-start)`, with a missing bound read as the infinity section 9.9 names for it.
/// Both comparisons are strict, unlike the `DTSTART`/`DTEND` row above them, and relaxing either
/// one to `<=` returns periods that touch the window at a point and occupy none of it.
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
    Ok(())
}

/// Charge the caller's ledger for every octet of `holder` and everything nested inside it.
///
/// Iterative, for the reason [`select_component`] gives, and over a worklist rather than a
/// nesting stack because charging has no order to respect.
fn charge_subtree(holder: &Component, meter: &mut Meter) -> Result<(), QueryError> {
    let mut pending: Vec<&Component> = alloc::vec![holder];
    while let Some(current) = pending.pop() {
        meter.try_charge_bytes(boundary_octets(current))?;
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

/// What one content line costs the calendar being built.
///
/// The name, the value and every parameter, which is what a copy of the line occupies.
/// Saturating throughout: at `u64::MAX` octets the answer is the same refusal either way, and a
/// wrap would report a clean ledger over a calendar that had exhausted it (`docs/adr/0007`).
fn line_octets(line: &Property) -> u64 {
    let mut total = octets(line.name().len()).saturating_add(octets(line.value_text().len()));
    for parameter in line.parameters() {
        total = total
            .saturating_add(octets(parameter.name().len()))
            .saturating_add(octets(parameter.value().len()));
    }
    total
}

/// What the two boundary lines of one component cost the calendar being built.
///
/// The name written twice, beside `BEGIN:`, `END:` and the two terminators, which is fourteen
/// octets. Charged rather than ignored because a calendar of ten thousand empty components has a
/// size that a ledger counting only property text would report as nothing at all.
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

    use ical_core::{
        DateTimeValue, DecodeValue, DiagnosticCode, Document, IgnoreDiagnostics, Instant,
        LimitExceeded, Limits, Meter, PropertyId, UtcOffset,
    };
    use ical_dav::{CompSelection, TimeRange};

    use super::{
        SUBSELECTION_SECTIONS, limit_freebusy_set, limit_recurrence_set, select, without_values,
    };
    use crate::internal::query::{Budget, Match, QueryError, Reduction, Selection, Undecided};

    /// RFC 4791 Appendix B, `abcd2.ics`, carrying the second overridden component that section
    /// 7.8.1's own response for that resource shows and section 7.8.2's narration counts.
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

    /// What RFC 4791 section 7.8.1's request returns for that resource, plus the `PRODID` this
    /// unit keeps. Its `VTIMEZONE` is whole, and no `VEVENT` carries the `DTSTAMP` the request
    /// did not name.
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

    /// A `VFREEBUSY` carrying the three-period `FREEBUSY` value of RFC 5545 section 3.8.2.6:
    /// 16:00 to 19:00, 20:00 to 21:00, and 23:00 to midnight, all on 8 March 1997 in UTC.
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

    /// One period, 16:00 to 19:00 on 8 March 1997 in UTC, for the edges of section 9.9's row.
    const ONE_PERIOD: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VFREEBUSY\r\n\
FREEBUSY:19970308T160000Z/PT3H\r\n\
END:VFREEBUSY\r\n\
END:VCALENDAR\r\n";

    /// A `FREEBUSY` whose first value is not a period at all, beside one that is.
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

    /// RFC 4791 section 9.6: "If the CALDAV:calendar-data XML element doesn't contain any
    /// CALDAV:comp element, calendar object resources will be returned in their entirety."
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

    /// RFC 4791 section 7.8.1's worked example, request and response.
    ///
    /// The request names `VERSION` on the calendar, ten properties on `VEVENT`, and an empty
    /// `<C:comp name="VTIMEZONE"/>`. The response returns that `VTIMEZONE` whole and each
    /// `VEVENT` without the `DTSTAMP` the request did not name, in the order the resource stored
    /// them.
    ///
    /// `PRODID` is the one octet-level departure from that response and it is deliberate: the
    /// specification's own two responses to this one request disagree — `abcd2.ics` comes back
    /// without a `PRODID` and `abcd3.ics` with one — and section 9.6 permits an invalid answer
    /// rather than requiring one. This module's documentation states why the readable reading
    /// wins.
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

    /// A selection naming a component type the resource does not carry returns none of that
    /// type: RFC 4791 section 9.6.1 defines which component types to return, and the calendar it
    /// is rooted at stays readable.
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

    /// RFC 4791 section 9.6.1: `comp ((allprop | prop*), (allcomp | comp*))`. Each pair is an
    /// alternation, so a value holding both halves is one no request body expresses — including
    /// in a branch no calendar this query runs against would ever reach.
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

    /// RFC 4791 sections 9.6.2 and 9.6.3: `allcomp` and `allprop` ask for everything, so the
    /// witness has to say that nothing was left out. This is the one answer a caller may write
    /// back.
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

    /// A `VTIMEZONE` no `comp` element names still comes back when a surviving `DTSTART` reads
    /// its value under that zone: the alternative is a calendar no reader can place on a
    /// timeline, which is a worse answer than one component too many.
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

    /// A zone nothing refers to any more stays out: the rescue is for a `TZID` still named, and
    /// never a floor under every reduction.
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

    /// RFC 4791 section 7.8.2's worked example: with a `limit-recurrence-set` over January 3 to
    /// January 5, 2006, "the first overridden component in the matching resource is returned,
    /// but the second one is not" — and the master comes back with its `RRULE` intact, which is
    /// the asymmetry with `CALDAV:expand`.
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

    /// An override whose overlap could not be decided is returned rather than dropped. Dropping
    /// it would report an absence nothing established, which is what the third value of a match
    /// exists to refuse.
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

    /// RFC 4791 section 9.6.7 keeps only the `FREEBUSY` property values that intersect the
    /// window, by the `VFREEBUSY` row of section 9.9. Of the three periods here only the second
    /// is inside 20:00 to 23:00: the first ends before it and the third starts at its
    /// non-inclusive end.
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

    /// RFC 4791 section 9.9, the `VFREEBUSY` row: `(start < freebusy-period-end) AND (end >
    /// freebusy-period-start)`. Both comparisons are strict, so a period touching the window at
    /// a point is outside it. The period under test runs from 16:00 to 19:00.
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

    /// A period this crate cannot place is kept: RFC 5545 section 3.8.2.6 requires UTC, and a
    /// value violating it is one no comparison was made on. Reporting that time as free because
    /// a value did not decode is the invention `docs/adr/0003` refuses.
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

    /// RFC 4791 section 9.6.4: with `novalue="yes"` "the server will return just the iCalendar
    /// property name and any iCalendar parameters and a trailing ':' without the subsequent
    /// value data".
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

    /// Naming no property asks for nothing: the calendar comes back as it stands, with the
    /// witness it arrived with.
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

    /// The witness is the union of every step, because a step has no way to know what the one
    /// before it left out — and a chain reporting only its last step would hand a caller a
    /// faithful-looking calendar two reductions deep.
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

    /// Every door is bounded, `docs/adr/0010`: the calendar being built is charged to the
    /// caller's ledger as it is assembled, and a ledger too small to hold it refuses.
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

    /// The nesting a selection may walk is the caller's `Limits::max_component_depth`, so a
    /// calendar nested past it is refused rather than walked. The walk holds no stack frames of
    /// its own, which is what makes the refusal a policy rather than a crash.
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
