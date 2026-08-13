// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The server direction: a request body read out of the octets some client sent.
//!
//! Nothing here has a type of its own beyond [`RequestBody`], which names which of the five
//! bodies a document carried. Every value this module produces is the one a client builds in
//! [`crate::internal::dav::request`], through the same constructors, so a server decoding an untrusted
//! `REPORT` meets exactly the refusals a client building the same request meets: a `time-range`
//! with neither bound is [`ValueError::TimeRangeUnbounded`] from [`TimeRange::new`], a filter
//! tree past `Limits::max_xml_depth` is [`LimitExceeded::Depth`], and a property list past
//! `Limits::max_props_per_response` is [`LimitExceeded::Properties`] from the charged push. That
//! is DP-15's symmetry claim with the direction removed from the types and left in which trait
//! is called.
//!
//! # What a hostile body costs before it costs a stack
//!
//! [`CompFilter::push_comp`] bounds the tree it is handed, and it is handed one only after the
//! recursion that read it has already returned. A body nesting `comp-filter` a hundred thousand
//! deep would therefore have spent the stack before any bound was consulted, which is the debt
//! `docs/adr/0004` left open under this unit's name. So the recursion carries its own nesting
//! count and refuses `LimitExceeded::Depth` on the way *down*, at the same number the push
//! refuses on the way up: a tree this module reads is a tree that constructor would have
//! accepted, and a tree it would not is refused before the frame is pushed rather than after.
//!
//! # Where an unknown element is a name and where it is a question
//!
//! Inside `DAV:prop` and `DAV:include` an element outside the closed vocabulary is a *property
//! name*, which RFC 4918 section 14.18 admits without limit, so it is kept as
//! [`PropName::Extension`] and never skipped: a server answers a name it does not know with a
//! `404` in the multistatus, and dropping the name would answer a different request. Everywhere
//! else — under `filter`, under `calendar-data`, at the root — a foreign element changes what
//! the request means, so [`UnknownPolicy`] decides between skipping it with a diagnostic and
//! refusing the body.

use alloc::vec::Vec;

use crate::internal::core::{
    DateTimeValue, DecodeValue, DiagnosticCode, Instant, LimitExceeded, Severity, UtcOffset,
};

use crate::internal::dav::codec::{ReadXml, XmlEvent, XmlPull};
use crate::internal::dav::element::{ElementName, Namespace, QName};
use crate::internal::dav::failure::{DavError, SyntaxError, ValueError};
use crate::internal::dav::policy::{DecodeContext, UnknownPolicy};
use crate::internal::dav::request::{
    CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, QueryShape, TextMatch,
    TimeRange,
};
use crate::internal::dav::response::CalendarPayload;
use crate::internal::dav::text::LineEndings;
use crate::internal::dav::value::{ExtensionName, Href};

/// The namespace an unprefixed attribute is in, which is none at all.
///
/// XML Namespaces 1.0 section 6.2 is explicit that a default declaration does not apply to
/// attributes, so `name`, `start`, `end`, `collation` and `negate-condition` are looked up in no
/// namespace rather than in the one their element resolved into.
const NO_NAMESPACE: Namespace<'static> = Namespace::Other(b"");

/// Which request body a document carried, read from its root element.
///
/// The door a server actually needs: an HTTP layer hands over octets and a method, and which of
/// the five bodies arrived is a fact about the octets rather than about the method — `REPORT`
/// carries three of them. Reading it here is also what makes
/// [`DavError::Unsupported`](crate::internal::dav::DavError::Unsupported) reachable for a whole body, so a
/// build without `sync-collection` refuses an RFC 6578 `REPORT` instead of answering it with a
/// full enumeration and letting the client believe it had synchronized.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RequestBody {
    /// A `DAV:propfind`, RFC 4918 section 14.20.
    PropFind(PropFind),
    /// A `CALDAV:calendar-query`, RFC 4791 section 9.5.
    CalendarQuery(CalendarQuery),
    /// A `CALDAV:calendar-multiget`, RFC 4791 section 9.10.
    CalendarMultiget(CalendarMultiget),
    /// A `CALDAV:free-busy-query`, RFC 4791 section 9.11.
    FreeBusyQuery(FreeBusyQuery),
    /// A `DAV:sync-collection`, RFC 6578 section 3.
    SyncCollection(crate::internal::dav::request::SyncCollection),
}

impl RequestBody {
    /// Read whichever body a document carries, from before its root element.
    ///
    /// Unlike [`ReadXml::read_xml`], which starts at an element the caller has already opened,
    /// this opens the root itself — because which body arrived is exactly what the root element
    /// says, and a caller that had to know before reading could not have found out.
    pub fn read(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        loop {
            match events.next_event(context)? {
                None => return Err(SyntaxError::Truncated.into()),
                // Character data before the root element is layout: an XML declaration, a
                // comment and the line breaks between them are not events this reader sees.
                Some(XmlEvent::Text(_)) => {},
                Some(XmlEvent::End { .. }) => return Err(SyntaxError::Malformed.into()),
                Some(XmlEvent::Start { known, .. }) => {
                    return Self::of_root(known, events, context);
                },
            }
        }
    }

    /// Dispatch on the root element, refusing what this build or this crate cannot honor.
    fn of_root(
        known: Option<ElementName>,
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        // A foreign root is refused whatever `UnknownPolicy` says, because skipping it would
        // leave no body at all and a request nobody read is not a request anybody answered.
        let row = known.ok_or(DavError::Foreign)?;
        supported(row)?;
        match row {
            ElementName::Propfind => PropFind::read_xml(events, context).map(Self::PropFind),
            ElementName::CalendarQuery => {
                CalendarQuery::read_xml(events, context).map(Self::CalendarQuery)
            },
            ElementName::CalendarMultiget => {
                CalendarMultiget::read_xml(events, context).map(Self::CalendarMultiget)
            },
            ElementName::FreeBusyQuery => {
                FreeBusyQuery::read_xml(events, context).map(Self::FreeBusyQuery)
            },
            ElementName::SyncCollection => {
                crate::internal::dav::request::SyncCollection::read_xml(events, context)
                    .map(Self::SyncCollection)
            },
            other => Err(DavError::Unexpected(other)),
        }
    }
}

impl ReadXml for PropFind {
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut chosen: Option<Self> = None;
        while let Some((_, known)) = next_child(events, context)? {
            read_propfind_child(events, context, &mut chosen, known)?;
        }
        // RFC 4918 section 14.20 requires one of the three. An empty `<propfind/>` is not the
        // absent body section 9.1 reads as `allprop`; guessing which of the two a peer meant is
        // the kind of completion this crate refuses.
        chosen.ok_or(DavError::Unexpected(ElementName::Propfind))
    }
}

/// One child of a `DAV:propfind`.
fn read_propfind_child(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    chosen: &mut Option<PropFind>,
    known: Option<ElementName>,
) -> Result<(), DavError> {
    match known {
        Some(ElementName::Prop) => {
            let mut wanted = PropRequest::new(context.limits);
            read_prop_request(events, context, &mut wanted)?;
            accept(chosen, PropFind::Props(wanted), ElementName::Prop)
        },
        Some(ElementName::Propname) => {
            events.skip_element(context)?;
            accept(chosen, PropFind::Names, ElementName::Propname)
        },
        Some(ElementName::Allprop) => {
            events.skip_element(context)?;
            let empty = PropFind::AllProp(PropRequest::new(context.limits));
            accept(chosen, empty, ElementName::Allprop)
        },
        // RFC 4918 section 14.8 puts `include` beside `allprop` and nowhere else: it names the
        // expensive properties `allprop` deliberately omits, so it has nothing to add to a
        // request that already named what it wanted.
        Some(ElementName::Include) => {
            if let Some(PropFind::AllProp(wanted)) = chosen {
                read_prop_request(events, context, wanted)
            } else {
                Err(DavError::Unexpected(ElementName::Include))
            }
        },
        Some(other) => Err(DavError::Unexpected(other)),
        None => skip_foreign(events, context),
    }
}

/// Record which of `allprop`, `propname` and `prop` a `propfind` chose, refusing a second.
fn accept(
    chosen: &mut Option<PropFind>,
    found: PropFind,
    name: ElementName,
) -> Result<(), DavError> {
    if chosen.is_some() {
        return Err(DavError::Unexpected(name));
    }
    *chosen = Some(found);
    Ok(())
}

impl ReadXml for CalendarQuery {
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut query = Self::new(context.limits);
        while let Some((_, known)) = next_child(events, context)? {
            match known {
                Some(ElementName::Prop) => read_prop_request(events, context, &mut query.props)?,
                // RFC 4791 section 9.5's own production is
                // `((DAV:allprop | DAV:propname | DAV:prop)?, filter, timezone?)`. Two of the
                // three used to be refused outright, which is a conformant request this crate
                // answered `DavError::Unexpected` to.
                Some(ElementName::Allprop) => {
                    events.skip_element(context)?;
                    query.shape = QueryShape::AllProp;
                },
                Some(ElementName::Propname) => {
                    events.skip_element(context)?;
                    query.shape = QueryShape::Names;
                },
                Some(ElementName::Filter) => query.filter = Some(read_filter(events, context)?),
                Some(ElementName::Timezone) => {
                    query.timezone = Some(read_payload(events, context)?);
                },
                Some(other) => return Err(DavError::Unexpected(other)),
                None => skip_foreign(events, context)?,
            }
        }
        Ok(query)
    }
}

impl ReadXml for CalendarMultiget {
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut multiget = Self::new(context.limits);
        while let Some((_, known)) = next_child(events, context)? {
            match known {
                Some(ElementName::Prop) => read_prop_request(events, context, &mut multiget.props)?,
                Some(ElementName::Href) => {
                    let octets = read_text(events, context, Whitespace::Layout)?;
                    let href = Href::new(&octets, context.limits, context.meter)?;
                    multiget.push_href(href, context.meter)?;
                },
                Some(other) => return Err(DavError::Unexpected(other)),
                None => skip_foreign(events, context)?,
            }
        }
        Ok(multiget)
    }
}

impl ReadXml for FreeBusyQuery {
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut found: Option<TimeRange> = None;
        while let Some((_, known)) = next_child(events, context)? {
            match known {
                Some(ElementName::TimeRange) => found = Some(read_time_range(events, context)?),
                Some(other) => return Err(DavError::Unexpected(other)),
                None => skip_foreign(events, context)?,
            }
        }
        // An absent `time-range` and one carrying neither bound state the same thing — no
        // window — so they are refused by the same constructor rather than by two rules.
        let range = match found {
            Some(window) => window,
            None => TimeRange::new(None, None)?,
        };
        Ok(Self { range })
    }
}

impl ReadXml for crate::internal::dav::request::SyncCollection {
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut request = Self::new(context.limits);
        while let Some((_, known)) = next_child(events, context)? {
            read_sync_child(events, context, &mut request, known)?;
        }
        Ok(request)
    }
}

/// One child of a `DAV:sync-collection`.
fn read_sync_child(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    request: &mut crate::internal::dav::request::SyncCollection,
    known: Option<ElementName>,
) -> Result<(), DavError> {
    match known {
        Some(ElementName::SyncToken) => {
            let octets = read_text(events, context, Whitespace::Layout)?;
            // RFC 6578 section 3.2 writes an empty `sync-token` for an initial synchronization,
            // which is the absence of a token rather than a token of no octets.
            request.token = if octets.is_empty() {
                None
            } else {
                Some(crate::internal::dav::value::SyncToken::new(
                    &octets,
                    context.limits,
                    context.meter,
                )?)
            };
            Ok(())
        },
        Some(ElementName::SyncLevel) => {
            let octets = read_text(events, context, Whitespace::Layout)?;
            request.level = crate::internal::dav::request::SyncLevel::parse(&octets)?;
            Ok(())
        },
        Some(ElementName::Limit) => {
            request.limit = Some(read_nresults(events, context)?);
            Ok(())
        },
        Some(ElementName::Prop) => read_prop_request(events, context, &mut request.props),
        Some(other) => Err(DavError::Unexpected(other)),
        None => skip_foreign(events, context),
    }
}

/// Read the `DAV:nresults` inside a `DAV:limit`, RFC 5323 section 5.17.
fn read_nresults(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<u32, DavError> {
    let mut found: Option<u32> = None;
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::Nresults) => {
                let octets = read_text(events, context, Whitespace::Layout)?;
                found = Some(parse_count(&octets)?);
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    found.ok_or_else(refused)
}

/// Read an unsigned decimal count, refusing what is not one rather than reading a prefix.
fn parse_count(octets: &[u8]) -> Result<u32, DavError> {
    if octets.is_empty() {
        return Err(refused());
    }
    let mut count: u32 = 0;
    for byte in octets {
        let digit = char::from(*byte).to_digit(10).ok_or_else(refused)?;
        count = count
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(digit))
            .ok_or_else(refused)?;
    }
    Ok(count)
}

/// Read the property names a request asks for, out of a `DAV:prop` or a `DAV:include`.
fn read_prop_request(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    wanted: &mut PropRequest,
) -> Result<(), DavError> {
    while let Some((name, known)) = next_child(events, context)? {
        match known {
            // The payload request is a field rather than a name in the list, which is the shape
            // a client builds: `props.calendar_data = Some(..)` beside `props.push(..)`.
            Some(ElementName::CalendarData) => {
                wanted.calendar_data = Some(read_calendar_data(events, context)?);
            },
            Some(row) => {
                supported(row)?;
                wanted.push(PropName::Known(row), context.meter)?;
                events.skip_element(context)?;
            },
            None => {
                let extension =
                    ExtensionName::new(name.namespace.uri(), name.local_name, context.meter)?;
                wanted.push(PropName::Extension(extension), context.meter)?;
                events.skip_element(context)?;
            },
        }
    }
    Ok(())
}

/// Read a `CALDAV:calendar-data` request, RFC 4791 section 9.6.
///
/// One row read two ways: inside a request this element carries elements, and inside a response
/// it carries the payload whose line endings `docs/adr/0004` Amendment 1 is about. Character
/// data here is layout, which is why nothing in this function looks at any.
fn read_calendar_data(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<CalendarDataRequest, DavError> {
    let mut wanted = CalendarDataRequest::default();
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::CalendarDataComp) => {
                wanted.comp = Some(read_comp_selection(events, context, 0)?);
            },
            // Section 9.6.5 requires both bounds of an `expand`; `TimeRange::new` requires one.
            // The stricter cardinality is a server's to enforce against its own recurrence
            // layer, which this crate does not depend on and cannot expand for.
            Some(ElementName::Expand) => wanted.expand = Some(read_time_range(events, context)?),
            Some(ElementName::LimitRecurrenceSet) => {
                wanted.limit_recurrence_set = Some(read_time_range(events, context)?);
            },
            Some(ElementName::LimitFreebusySet) => {
                wanted.limit_freebusy_set = Some(read_time_range(events, context)?);
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    Ok(wanted)
}

/// Read a `CALDAV:comp` selection, RFC 4791 section 9.6.1.
///
/// `depth` is this recursion's own nesting, refused at the same number `Limits::max_xml_depth`
/// gives the filter tree and for the same reason: the stack this function spends is the request
/// body's to choose otherwise.
fn read_comp_selection(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    depth: u16,
) -> Result<CompSelection, DavError> {
    if depth >= context.limits.max_xml_depth() {
        return Err(DavError::Limit(LimitExceeded::Depth));
    }
    let named = required_name(events)?;
    let mut selection = CompSelection::new(named, context.limits, context.meter)?;
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::CalendarDataAllcomp) => {
                selection.all_comps = true;
                events.skip_element(context)?;
            },
            Some(ElementName::CalendarDataAllprop) => {
                selection.all_props = true;
                events.skip_element(context)?;
            },
            Some(ElementName::CalendarDataProp) => {
                let wanted = required_name(events)?;
                selection.push_prop(wanted, context.meter)?;
                events.skip_element(context)?;
            },
            Some(ElementName::CalendarDataComp) => {
                let child = read_comp_selection(events, context, depth.saturating_add(1))?;
                selection.push_comp(child, context.meter)?;
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    Ok(selection)
}

/// Read a `CALDAV:filter`, which carries exactly one root `comp-filter`.
fn read_filter(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<CompFilter, DavError> {
    let mut found: Option<CompFilter> = None;
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::CompFilter) => {
                if found.is_some() {
                    return Err(DavError::Unexpected(ElementName::CompFilter));
                }
                found = Some(read_comp_filter(events, context, 0)?);
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    found.ok_or(DavError::Unexpected(ElementName::Filter))
}

/// Read a `CALDAV:comp-filter`, RFC 4791 section 9.7.1.
///
/// The nesting count is checked before the frame is pushed rather than after the subtree is
/// built, which is the whole of this unit's answer to the recursion debt: `push_comp` refuses a
/// tree it is handed, and a body that would overflow the stack is never handed to it.
fn read_comp_filter(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    depth: u16,
) -> Result<CompFilter, DavError> {
    if depth >= context.limits.max_xml_depth() {
        return Err(DavError::Limit(LimitExceeded::Depth));
    }
    let named = required_name(events)?;
    let mut filter = CompFilter::new(named, context.limits, context.meter)?;
    while let Some((_, known)) = next_child(events, context)? {
        read_comp_filter_child(events, context, &mut filter, (known, depth))?;
    }
    if filter.is_contradictory() {
        return Err(ValueError::FilterContradiction.into());
    }
    Ok(filter)
}

/// One child of a `CALDAV:comp-filter`.
fn read_comp_filter_child(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    filter: &mut CompFilter,
    position: (Option<ElementName>, u16),
) -> Result<(), DavError> {
    let (known, depth) = position;
    match known {
        Some(ElementName::IsNotDefined) => {
            filter.is_not_defined = true;
            events.skip_element(context)
        },
        Some(ElementName::TimeRange) => {
            filter.time_range = Some(read_time_range(events, context)?);
            Ok(())
        },
        Some(ElementName::PropFilter) => {
            let child = read_prop_filter(events, context)?;
            filter.push_prop(child, context.meter)
        },
        Some(ElementName::CompFilter) => {
            let child = read_comp_filter(events, context, depth.saturating_add(1))?;
            filter.push_comp(child, context.limits, context.meter)
        },
        Some(other) => Err(DavError::Unexpected(other)),
        None => skip_foreign(events, context),
    }
}

/// Read a `CALDAV:prop-filter`, RFC 4791 section 9.7.2.
fn read_prop_filter(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<PropFilter, DavError> {
    let named = required_name(events)?;
    let mut filter = PropFilter::new(named, context.limits, context.meter)?;
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::IsNotDefined) => {
                filter.is_not_defined = true;
                events.skip_element(context)?;
            },
            Some(ElementName::TimeRange) => {
                filter.time_range = Some(read_time_range(events, context)?);
            },
            Some(ElementName::TextMatch) => {
                filter.text_match = Some(read_text_match(events, context)?);
            },
            Some(ElementName::ParamFilter) => {
                let child = read_param_filter(events, context)?;
                filter.push_param(child, context.meter)?;
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    if filter.is_contradictory() {
        return Err(ValueError::FilterContradiction.into());
    }
    Ok(filter)
}

/// Read a `CALDAV:param-filter`, RFC 4791 section 9.7.3.
fn read_param_filter(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<ParamFilter, DavError> {
    let named = required_name(events)?;
    let mut filter = ParamFilter::new(named, context.meter)?;
    while let Some((_, known)) = next_child(events, context)? {
        match known {
            Some(ElementName::IsNotDefined) => {
                filter.is_not_defined = true;
                events.skip_element(context)?;
            },
            Some(ElementName::TextMatch) => {
                filter.text_match = Some(read_text_match(events, context)?);
            },
            Some(other) => return Err(DavError::Unexpected(other)),
            None => skip_foreign(events, context)?,
        }
    }
    if filter.is_contradictory() {
        return Err(ValueError::FilterContradiction.into());
    }
    Ok(filter)
}

/// Read a `CALDAV:text-match`, RFC 4791 section 9.7.5.
fn read_text_match(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<TextMatch, DavError> {
    let collation = match events.attribute(QName::new(NO_NAMESPACE, b"collation")) {
        Some(named) => Collation::parse(named)?,
        None => Collation::AsciiCasemap,
    };
    let negate = read_negate(events)?;
    // The one text this module keeps whole: a search string is somebody's data, and a trailing
    // space in it is a character they typed rather than a line the exporter broke.
    let value = read_text(events, context, Whitespace::Content)?;
    let mut matcher = TextMatch::new(&value, context.meter)?;
    matcher.collation = collation;
    matcher.negate = negate;
    Ok(matcher)
}

/// Read `negate-condition`, which RFC 4791 section 9.7.5 declares as `(yes | no) "no"`.
fn read_negate(events: &dyn XmlPull<'_>) -> Result<bool, DavError> {
    match events.attribute(QName::new(NO_NAMESPACE, b"negate-condition")) {
        None => Ok(false),
        Some(value) => match value {
            b"no" => Ok(false),
            b"yes" => Ok(true),
            _ => Err(ValueError::AttributeValue.into()),
        },
    }
}

/// Read a `CALDAV:time-range`, RFC 4791 section 9.9.
///
/// Both bounds are read independently and handed to [`TimeRange::new`], so an open start and an
/// open end are the two shapes section 9.9 permits and neither is a special case here.
fn read_time_range(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<TimeRange, DavError> {
    let start = attribute_instant(events, b"start")?;
    let end = attribute_instant(events, b"end")?;
    let range = TimeRange::new(start, end)?;
    events.skip_element(context)?;
    Ok(range)
}

/// One bound of a window, absent when the attribute is.
fn attribute_instant(
    events: &dyn XmlPull<'_>,
    local_name: &[u8],
) -> Result<Option<Instant>, DavError> {
    match events.attribute(QName::new(NO_NAMESPACE, local_name)) {
        None => Ok(None),
        Some(written) => parse_utc_date_time(written).map(Some),
    }
}

/// Read the `YYYYMMDDTHHMMSSZ` a `time-range` attribute carries.
///
/// The `Z` is required rather than assumed: RFC 4791 section 9.9 writes a UTC `DATE-TIME`, and a
/// floating value read as though it were UTC would move the window by the reader's own zone.
fn parse_utc_date_time(written: &[u8]) -> Result<Instant, DavError> {
    let value =
        DateTimeValue::decode_value(written).map_err(|_| ValueError::TimeUnrepresentable)?;
    let DateTimeValue::Utc(stamp) = value else {
        return Err(ValueError::TimeUnrepresentable.into());
    };
    stamp
        .at_offset(UtcOffset::UTC)
        .ok_or_else(|| DavError::from(ValueError::TimeUnrepresentable))
}

/// The `name` attribute a filter or a selection element must carry.
///
/// The borrow is the tokenizer's rather than the body's, because the value XML 1.0 section
/// 3.3.3 defines is the one with its references resolved and its literal whitespace replaced —
/// which is not a run of octets the body holds anywhere.
fn required_name<'a>(events: &'a dyn XmlPull<'_>) -> Result<&'a [u8], DavError> {
    events
        .attribute(QName::new(NO_NAMESPACE, b"name"))
        .ok_or_else(|| DavError::from(ValueError::AttributeMissing))
}

/// Whether the whitespace around an element's character data is layout or content.
///
/// An `href`, a sync token and a `sync-level` are grammars with no whitespace in them — RFC 3986
/// section 4.1 admits none in a URI reference — so what surrounds one is the producer's
/// indentation. A `text-match` value is not a grammar at all, and trimming it would silently
/// change which events a query matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whitespace {
    /// Drop what surrounds the value.
    Layout,
    /// Keep every octet.
    Content,
}

/// Collect an element's character data, and consume the element.
fn read_text(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
    edges: Whitespace,
) -> Result<Vec<u8>, DavError> {
    let mut collected = Vec::new();
    loop {
        match events.next_event(context)? {
            None => return Err(SyntaxError::Truncated.into()),
            Some(XmlEvent::End { .. }) => break,
            Some(XmlEvent::Text(decoded)) => append(&mut collected, decoded.run.as_bytes())?,
            // An element inside one whose content is text is not a shape these grammars have;
            // a foreign one is still the caller's policy to decide about.
            Some(XmlEvent::Start { known, .. }) => match known {
                Some(row) => return Err(DavError::Unexpected(row)),
                None => skip_foreign(events, context)?,
            },
        }
    }
    if edges == Whitespace::Layout {
        trim(&mut collected);
    }
    Ok(collected)
}

/// Collect an iCalendar object a request element carries, with its line-ending witness.
///
/// The same shape a `calendar-data` payload arrives in on the response side, because it is the
/// same kind of value: RFC 4791 section 9.5's `CALDAV:timezone` is "a valid iCalendar object
/// containing exactly one VTIMEZONE component", and its `CRLF` terminators are RFC 5545 section
/// 3.1 syntax rather than layout. No trimming, for the same reason.
fn read_payload(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<CalendarPayload, DavError> {
    let mut collected = Vec::new();
    let mut endings: Option<LineEndings> = None;
    loop {
        match events.next_event(context)? {
            None => return Err(SyntaxError::Truncated.into()),
            Some(XmlEvent::End { .. }) => break,
            Some(XmlEvent::Text(decoded)) => {
                // The witness of the first run is the read's own answer about folding, and a
                // fold is a fact about the read rather than about one run of it.
                if endings.is_none() || endings == Some(LineEndings::Absent) {
                    endings = Some(decoded.line_endings);
                }
                append(&mut collected, decoded.run.as_bytes())?;
            },
            Some(XmlEvent::Start { known, .. }) => match known {
                Some(row) => return Err(DavError::Unexpected(row)),
                None => skip_foreign(events, context)?,
            },
        }
    }
    let payload = CalendarPayload::from_octets(&collected, context.limits, context.meter)?;
    match endings {
        Some(LineEndings::Folded) => Ok(payload.into_folded()),
        _ => Ok(payload),
    }
}

/// Drop the whitespace a producer indented a value with, in place.
fn trim(value: &mut Vec<u8>) {
    let last = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |at| at.saturating_add(1));
    value.truncate(last);
    let first = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    value.drain(..first);
}

/// Append to a buffer that reports a refusing allocator rather than aborting on one.
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), DavError> {
    out.try_reserve(bytes.len())
        .map_err(|_| LimitExceeded::Budget)?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// The next child element of the element being read, or `None` at its end.
///
/// Every caller consumes each child whole — by recursing through its end tag or by
/// [`XmlPull::skip_element`] — so the first `End` this sees is the reader's own, and there is no
/// depth arithmetic in any of the bodies above.
fn next_child<'a>(
    events: &mut dyn XmlPull<'a>,
    context: &mut DecodeContext<'_>,
) -> Result<Option<(QName<'a>, Option<ElementName>)>, DavError> {
    loop {
        match events.next_event(context)? {
            None => return Err(SyntaxError::Truncated.into()),
            Some(XmlEvent::End { .. }) => return Ok(None),
            // Character data between child elements is the producer's indentation. None of
            // these grammars is mixed content, so there is nothing here to lose.
            Some(XmlEvent::Text(_)) => {},
            Some(XmlEvent::Start { name, known, .. }) => return Ok(Some((name, known))),
        }
    }
}

/// What an element outside the closed vocabulary costs, under the caller's policy.
fn skip_foreign(
    events: &mut dyn XmlPull<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    if context.unknown == UnknownPolicy::Reject {
        return Err(DavError::Foreign);
    }
    let offset = events.offset();
    context.report(
        DiagnosticCode::DavForeignElementSkipped,
        Severity::Note,
        offset,
    );
    events.skip_element(context)
}

/// Whether this build can honor the element, as a refusal rather than as a question.
const fn supported(name: ElementName) -> Result<(), DavError> {
    if name.is_supported() {
        Ok(())
    } else {
        Err(DavError::Unsupported(name))
    }
}

/// The refusal for a body that is well-formed XML and states something this reader will not
/// guess at.
///
/// Two cases share it, both under the `sync-collection` feature: a `DAV:limit` carrying no
/// `DAV:nresults` at all, and an `nresults` whose character data is not an unsigned decimal.
/// Neither is a well-formedness error and neither has a variant of its own — [`DavError::Unexpected`]
/// can only name an element that *is* there, and [`ValueError`] names values by the element that
/// carries them, with no row for a count. `Malformed` is the closest the vocabulary comes: a body
/// this reader will not read. The three cases that used to share it are now named — a foreign
/// element is [`DavError::Foreign`], a missing attribute is [`ValueError::AttributeMissing`], and
/// an attribute outside its enumeration is [`ValueError::AttributeValue`].
const fn refused() -> DavError {
    DavError::Syntax(SyntaxError::Malformed)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{
        Diagnostic, DiagnosticCode, Instant, LimitExceeded, Limits, Meter,
    };

    use super::{RequestBody, Whitespace, read_text};
    use crate::internal::dav::codec::{XmlEvent, XmlPull};
    use crate::internal::dav::element::{ElementName, Namespace, QName};
    use crate::internal::dav::failure::{DavError, SyntaxError, ValueError};
    use crate::internal::dav::policy::{DecodeContext, UnknownPolicy};
    use crate::internal::dav::request::{
        CalendarDataRequest, CalendarQuery, Collation, CompFilter, PropFind, PropName, TimeRange,
    };
    use crate::internal::dav::response::{DavProperty, DavResponse, PropStat, PropValue};
    use crate::internal::dav::text::{TextMode, decode_text};
    use crate::internal::dav::value::{Href, Status};

    /// 2006-01-04T00:00:00Z, the start of RFC 4791 section 7.8.1's own window.
    const RFC_WINDOW_START: i64 = 1_136_332_800;

    /// 2006-01-05T00:00:00Z, its end.
    const RFC_WINDOW_END: i64 = 1_136_419_200;

    /// 2026-01-01T00:00:00Z, the instant `request.rs`'s own tests are written around.
    const YEAR_START: i64 = 1_767_225_600;

    /// 2026-01-08T00:00:00Z.
    const WEEK_LATER: i64 = 1_767_830_400;

    // ---------------------------------------------------------------------------------------
    // A tokenizer, standing in for unit 1 so that this unit's table is wire bytes.
    // ---------------------------------------------------------------------------------------

    /// One open element: what it resolved to, and where its bindings begin.
    #[derive(Debug)]
    struct Frame<'a> {
        /// The name as the document spelled it, for the end tag to be checked against.
        written: &'a [u8],
        /// The resolved name.
        name: QName<'a>,
        /// The row it lands on, if any.
        known: Option<ElementName>,
        /// How many bindings were live before this element declared its own.
        bindings: usize,
    }

    /// A pull tokenizer over one body.
    ///
    /// Deliberately small, and deliberately not unit 1: it resolves prefixes and default
    /// declarations and hands character data to this crate's own `decode_text`, because those
    /// are the parts these tables depend on. It refuses almost nothing — the refusal list is
    /// unit 1's, and asserting it here would be asserting a second reader.
    #[derive(Debug)]
    struct Tokenizer<'a> {
        /// The body.
        body: &'a [u8],
        /// How far into it the reader sits.
        at: usize,
        /// How deep it sits, the root element being one.
        depth: u16,
        /// Prefix bindings, innermost last; the default declaration binds the empty prefix.
        bindings: Vec<(&'a [u8], &'a [u8])>,
        /// The open elements.
        frames: Vec<Frame<'a>>,
        /// The attributes of the element that has just started.
        attributes: Vec<(&'a [u8], &'a [u8])>,
        /// Whether a self-closing tag still owes an end event.
        closing: bool,
    }

    impl<'a> Tokenizer<'a> {
        /// A reader over `body`, positioned before its first event.
        fn new(body: &'a [u8]) -> Self {
            Self {
                body,
                at: 0,
                depth: 0,
                bindings: Vec::new(),
                frames: Vec::new(),
                attributes: Vec::new(),
                closing: false,
            }
        }

        /// The namespace a prefix is bound to, innermost declaration first.
        fn lookup(&self, prefix: &[u8]) -> Option<&'a [u8]> {
            self.bindings
                .iter()
                .rev()
                .find(|(bound, _)| *bound == prefix)
                .map(|(_, uri)| *uri)
        }

        /// Resolve a written name against the bindings in scope.
        fn resolve(&self, written: &'a [u8]) -> Result<QName<'a>, DavError> {
            let (prefix, local_name) = split_prefix(written);
            let uri = match self.lookup(prefix) {
                Some(found) => found,
                // An unprefixed name with no default declaration is in no namespace at all.
                None if prefix.is_empty() => b"".as_slice(),
                None => return Err(SyntaxError::UnboundPrefix.into()),
            };
            Ok(QName::new(Namespace::from_uri(uri), local_name))
        }

        /// Record the `xmlns` declarations the element that has just started carries.
        fn declare(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
            for &(written, uri) in &self.attributes {
                let prefix = if written == b"xmlns".as_slice() {
                    Some(b"".as_slice())
                } else {
                    written.strip_prefix(b"xmlns:".as_slice())
                };
                if let Some(prefix) = prefix {
                    context.meter.try_bind_prefix()?;
                    self.bindings.push((prefix, uri));
                }
            }
            Ok(())
        }

        /// Read a start tag, and the attributes and declarations on it.
        fn read_start(
            &mut self,
            context: &mut DecodeContext<'_>,
        ) -> Result<XmlEvent<'a>, DavError> {
            let close = position_of(self.body, self.at, b'>').ok_or(SyntaxError::Truncated)?;
            let inner = slice(self.body, self.at.saturating_add(1), close)?;
            let empty = inner.ends_with(b"/");
            let inner = if empty {
                slice(inner, 0, inner.len().saturating_sub(1))?
            } else {
                inner
            };
            self.at = close.saturating_add(1);
            let (written, rest) = split_written_name(inner);
            self.attributes.clear();
            parse_attributes(rest, &mut self.attributes)?;
            let bindings = self.bindings.len();
            self.declare(context)?;
            let name = self.resolve(written)?;
            let known = name.known();
            self.frames.push(Frame {
                written,
                name,
                known,
                bindings,
            });
            self.depth = self.depth.saturating_add(1);
            self.closing = empty;
            Ok(XmlEvent::Start {
                name,
                known,
                depth: self.depth,
            })
        }

        /// Close the innermost open element and release the bindings it declared.
        fn pop_frame(&mut self, context: &mut DecodeContext<'_>) -> Result<XmlEvent<'a>, DavError> {
            let frame = self.frames.pop().ok_or(SyntaxError::MismatchedTag)?;
            while self.bindings.len() > frame.bindings {
                self.bindings.pop();
                context.meter.unbind_prefix();
            }
            let depth = self.depth;
            self.depth = self.depth.saturating_sub(1);
            Ok(XmlEvent::End {
                name: frame.name,
                known: frame.known,
                depth,
            })
        }

        /// Read an end tag, which must name the element it closes.
        fn read_end(&mut self, context: &mut DecodeContext<'_>) -> Result<XmlEvent<'a>, DavError> {
            let close = position_of(self.body, self.at, b'>').ok_or(SyntaxError::Truncated)?;
            let written = slice(self.body, self.at.saturating_add(2), close)?.trim_ascii();
            self.at = close.saturating_add(1);
            let open = self.frames.last().ok_or(SyntaxError::MismatchedTag)?;
            if open.written != written {
                return Err(SyntaxError::MismatchedTag.into());
            }
            self.pop_frame(context)
        }

        /// Read a run of character data, through this crate's own decoder.
        fn read_characters(
            &mut self,
            context: &mut DecodeContext<'_>,
        ) -> Result<XmlEvent<'a>, DavError> {
            let start = self.at;
            let stop = position_of(self.body, start, b'<').unwrap_or(self.body.len());
            let span = slice(self.body, start, stop)?;
            self.at = stop;
            let known = self.frames.last().and_then(|frame| frame.known);
            let mode = TextMode::of(known, context.text);
            let offset = u64::try_from(start).unwrap_or(u64::MAX);
            let decoded = decode_text(span, mode, offset, context.meter, context.sink)?;
            Ok(XmlEvent::Text(decoded))
        }
    }

    impl<'a> XmlPull<'a> for Tokenizer<'a> {
        fn next_event(
            &mut self,
            context: &mut DecodeContext<'_>,
        ) -> Result<Option<XmlEvent<'a>>, DavError> {
            if self.closing {
                self.closing = false;
                return self.pop_frame(context).map(Some);
            }
            loop {
                let Some(&byte) = self.body.get(self.at) else {
                    return Ok(None);
                };
                if byte != b'<' {
                    return self.read_characters(context).map(Some);
                }
                let rest = self.body.get(self.at..).unwrap_or(&[]);
                if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
                    let close =
                        position_of(self.body, self.at, b'>').ok_or(SyntaxError::Truncated)?;
                    self.at = close.saturating_add(1);
                    continue;
                }
                if rest.starts_with(b"</") {
                    return self.read_end(context).map(Some);
                }
                return self.read_start(context).map(Some);
            }
        }

        fn skip_element(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
            let target = self.depth;
            loop {
                match self.next_event(context)? {
                    None => return Err(SyntaxError::Truncated.into()),
                    Some(XmlEvent::End { depth, .. }) if depth == target => return Ok(()),
                    Some(_) => {},
                }
            }
        }

        fn depth(&self) -> u16 {
            self.depth
        }

        fn offset(&self) -> u64 {
            u64::try_from(self.at).unwrap_or(u64::MAX)
        }

        fn resolve_prefix(&self, prefix: &[u8]) -> Option<Namespace<'a>> {
            self.lookup(prefix).map(Namespace::from_uri)
        }

        fn attribute(&self, wanted: QName<'_>) -> Option<&[u8]> {
            (0..self.attribute_count())
                .filter_map(|index| self.attribute_at(index))
                .find(|(held, _)| {
                    held.namespace.is(wanted.namespace) && held.local_name == wanted.local_name
                })
                .map(|(_, value)| value)
        }

        fn attribute_count(&self) -> usize {
            self.attributes
                .iter()
                .filter(|(written, _)| !written.starts_with(b"xmlns"))
                .count()
        }

        fn attribute_at(&self, index: usize) -> Option<(QName<'a>, &[u8])> {
            let (written, value) = self
                .attributes
                .iter()
                .copied()
                .filter(|(written, _)| !written.starts_with(b"xmlns"))
                .nth(index)?;
            let (prefix, local_name) = split_prefix(written);
            // An unprefixed attribute is in no namespace: XML Namespaces 1.0 section 6.2.
            let uri = if prefix.is_empty() {
                b"".as_slice()
            } else {
                self.lookup(prefix)?
            };
            Some((QName::new(Namespace::from_uri(uri), local_name), value))
        }
    }

    /// The first position of `needle` at or after `from`.
    fn position_of(haystack: &[u8], from: usize, needle: u8) -> Option<usize> {
        haystack
            .get(from..)?
            .iter()
            .position(|byte| *byte == needle)
            .map(|at| from.saturating_add(at))
    }

    /// A subslice, or a malformed body.
    fn slice(bytes: &[u8], from: usize, to: usize) -> Result<&[u8], DavError> {
        bytes
            .get(from..to)
            .ok_or(DavError::Syntax(SyntaxError::Malformed))
    }

    /// Split a written name into its prefix and its local name.
    fn split_prefix(written: &[u8]) -> (&[u8], &[u8]) {
        match written.iter().position(|byte| *byte == b':') {
            Some(colon) => (
                written.get(..colon).unwrap_or(&[]),
                written.get(colon.saturating_add(1)..).unwrap_or(&[]),
            ),
            None => (&[], written),
        }
    }

    /// Split a tag's interior into its name and everything after it.
    fn split_written_name(tag: &[u8]) -> (&[u8], &[u8]) {
        let end = tag
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(tag.len());
        (tag.get(..end).unwrap_or(&[]), tag.get(end..).unwrap_or(&[]))
    }

    /// Read `name="value"` pairs until the tag runs out.
    fn parse_attributes<'a>(
        mut rest: &'a [u8],
        into: &mut Vec<(&'a [u8], &'a [u8])>,
    ) -> Result<(), DavError> {
        loop {
            rest = rest.trim_ascii_start();
            if rest.is_empty() {
                return Ok(());
            }
            let equals = position_of(rest, 0, b'=').ok_or(SyntaxError::Malformed)?;
            let written = slice(rest, 0, equals)?.trim_ascii();
            let after = rest
                .get(equals.saturating_add(2)..)
                .ok_or(SyntaxError::Malformed)?;
            let close = position_of(after, 0, b'"').ok_or(SyntaxError::Truncated)?;
            into.push((written, slice(after, 0, close)?));
            rest = after
                .get(close.saturating_add(1)..)
                .ok_or(SyntaxError::Malformed)?;
        }
    }

    // ---------------------------------------------------------------------------------------
    // The table.
    // ---------------------------------------------------------------------------------------

    /// Read one body under the caller's policy, keeping whatever reached the sink.
    fn read_under(
        wire: &[u8],
        limits: Limits,
        unknown: UnknownPolicy,
    ) -> (Result<RequestBody, DavError>, Vec<Diagnostic>) {
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let outcome = {
            let mut context =
                DecodeContext::new(limits, &mut meter, &mut reported).with_unknown(unknown);
            let mut events = Tokenizer::new(wire);
            RequestBody::read(&mut events, &mut context)
        };
        (outcome, reported)
    }

    /// Read one body under the default policy.
    fn read(wire: &[u8]) -> Result<RequestBody, DavError> {
        read_under(wire, Limits::DEFAULT, UnknownPolicy::Skip).0
    }

    /// The query every prefix habit below spells, as a client builds it.
    fn client_query(limits: Limits, meter: &mut Meter) -> CalendarQuery {
        let mut query = CalendarQuery::new(limits);
        query
            .props
            .push(PropName::Known(ElementName::Getetag), meter)
            .unwrap();
        query.props.calendar_data = Some(CalendarDataRequest::default());
        let mut wanted = CompFilter::new(b"VEVENT", limits, meter).unwrap();
        wanted.time_range = Some(
            TimeRange::new(
                Some(Instant::from_unix_seconds(RFC_WINDOW_START)),
                Some(Instant::from_unix_seconds(RFC_WINDOW_END)),
            )
            .unwrap(),
        );
        let mut calendar = CompFilter::new(b"VCALENDAR", limits, meter).unwrap();
        calendar.push_comp(wanted, limits, meter).unwrap();
        query.filter = Some(calendar);
        query
    }

    /// RFC 4791 section 7.8.1's own body, with the RFC's own prefixes.
    const AS_THE_RFC_WRITES_IT: &[u8] = br#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20060104T000000Z" end="20060105T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
"#;

    /// The same request as a `SabreDAV`-facing client writes it: lowercase, one line.
    const AS_A_LOWERCASE_CLIENT_WRITES_IT: &[u8] = br#"<?xml version="1.0"?><cal:calendar-query xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><cal:calendar-data/></d:prop><cal:filter><cal:comp-filter name="VCALENDAR"><cal:comp-filter name="VEVENT"><cal:time-range start="20060104T000000Z" end="20060105T000000Z"/></cal:comp-filter></cal:comp-filter></cal:filter></cal:calendar-query>"#;

    /// The same request with the generated prefixes Apple's client emits.
    const AS_APPLE_WRITES_IT: &[u8] =
        br#"<x1:calendar-query xmlns:x0="DAV:" xmlns:x1="urn:ietf:params:xml:ns:caldav">
 <x0:prop><x0:getetag/><x1:calendar-data/></x0:prop>
 <x1:filter><x1:comp-filter name="VCALENDAR"><x1:comp-filter name="VEVENT">
 <x1:time-range start="20060104T000000Z" end="20060105T000000Z"/>
 </x1:comp-filter></x1:comp-filter></x1:filter></x1:calendar-query>"#;

    /// The same request with `DAV:` as the default declaration, as `dav4jvm` writes it.
    const AS_A_DEFAULT_DECLARATION_WRITES_IT: &[u8] =
        br#"<CAL:calendar-query xmlns="DAV:" xmlns:CAL="urn:ietf:params:xml:ns:caldav">
  <prop><getetag/><CAL:calendar-data/></prop>
  <CAL:filter><CAL:comp-filter name="VCALENDAR"><CAL:comp-filter name="VEVENT">
  <CAL:time-range start="20060104T000000Z" end="20060105T000000Z"/>
  </CAL:comp-filter></CAL:comp-filter></CAL:filter>
</CAL:calendar-query>"#;

    /// Four clients, four prefix habits, one value — and the value a client builds.
    ///
    /// This is DP-15's symmetry from the server end: the query a client assembles through the
    /// constructors and the query a server reads out of the octets are the same value, and none
    /// of the four spellings of it is privileged by a literal string anywhere.
    #[test]
    fn four_prefix_habits_are_one_request() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let built = client_query(limits, &mut meter);
        let cases = [
            ("RFC 4791 section 7.8.1", AS_THE_RFC_WRITES_IT),
            ("a lowercase d: and cal:", AS_A_LOWERCASE_CLIENT_WRITES_IT),
            ("Apple's generated x0: and x1:", AS_APPLE_WRITES_IT),
            ("DAV: as the default", AS_A_DEFAULT_DECLARATION_WRITES_IT),
        ];
        for (habit, wire) in cases {
            let read_back = read(wire);
            assert_eq!(
                read_back,
                Ok(RequestBody::CalendarQuery(built.clone())),
                "{habit}"
            );
        }
    }

    /// A prefix that looks familiar and is bound elsewhere is a different element.
    #[test]
    fn a_familiar_prefix_bound_to_another_namespace_is_not_this_vocabulary() {
        let hostile = br#"<D:calendar-query xmlns:D="http://evil.example/not-caldav">
  <D:prop/></D:calendar-query>"#;
        assert_eq!(read(hostile), Err(DavError::Foreign));
    }

    /// Thunderbird declares CalDAV as the default, so an unprefixed `filter` is CalDAV's.
    #[test]
    fn a_default_declaration_decides_what_an_unprefixed_element_means() {
        let wire = br#"<calendar-query xmlns="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:">
  <D:prop><D:getetag/></D:prop>
  <filter><comp-filter name="VCALENDAR"><comp-filter name="VTODO"/></comp-filter></filter>
</calendar-query>"#;
        let Ok(RequestBody::CalendarQuery(query)) = read(wire) else {
            panic!("a calendar-query is what this body is");
        };
        assert_eq!(query.props.names(), [PropName::Known(ElementName::Getetag)]);
        let root = query.filter.unwrap();
        assert_eq!(root.name(), b"VCALENDAR");
        assert_eq!(
            root.comps().first().map(CompFilter::name),
            Some(&b"VTODO"[..])
        );
    }

    /// Either bound may be absent, on the way in as on the way out.
    #[test]
    fn one_bound_of_a_time_range_may_be_absent() {
        let open_ended = br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:time-range start="20260101T000000Z"/></C:free-busy-query>"#;
        let open_started = br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:time-range end="20260108T000000Z"/></C:free-busy-query>"#;
        let start = Instant::from_unix_seconds(YEAR_START);
        let end = Instant::from_unix_seconds(WEEK_LATER);
        assert_eq!(
            read(open_ended),
            Ok(RequestBody::FreeBusyQuery(
                crate::internal::dav::request::FreeBusyQuery {
                    range: TimeRange::starting_at(start)
                }
            ))
        );
        assert_eq!(
            read(open_started),
            Ok(RequestBody::FreeBusyQuery(
                crate::internal::dav::request::FreeBusyQuery {
                    range: TimeRange::ending_before(end)
                }
            ))
        );
    }

    /// The refusals a client building a request meets are the refusals a server reading one
    /// meets, because both go through the same constructors.
    #[test]
    fn a_server_meets_the_refusals_a_client_would_have() {
        let cases: [(&str, &[u8], DavError); 5] = [
            (
                "a time-range with neither bound states no interval",
                br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
                    <C:time-range/></C:free-busy-query>"#,
                DavError::Invalid(ValueError::TimeRangeUnbounded),
            ),
            (
                "an absent time-range states no interval either",
                br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav"/>"#,
                DavError::Invalid(ValueError::TimeRangeUnbounded),
            ),
            (
                "a window that ends where it started",
                br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
                    <C:time-range start="20260108T000000Z" end="20260101T000000Z"/>
                    </C:free-busy-query>"#,
                DavError::Invalid(ValueError::TimeRangeInverted),
            ),
            (
                "a component that is not defined has no window to overlap",
                br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:">
                    <D:prop/><C:filter><C:comp-filter name="VEVENT"><C:is-not-defined/>
                    <C:time-range start="20260101T000000Z"/></C:comp-filter></C:filter>
                    </C:calendar-query>"#,
                DavError::Invalid(ValueError::FilterContradiction),
            ),
            (
                "a time-range attribute that is not a UTC date-time",
                br#"<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
                    <C:time-range start="20260101T000000"/></C:free-busy-query>"#,
                DavError::Invalid(ValueError::TimeUnrepresentable),
            ),
        ];
        for (shape, wire, expected) in cases {
            assert_eq!(read(wire), Err(expected), "{shape}");
        }
    }

    /// A filter tree deeper than the caller admits is refused on the way down.
    ///
    /// `CompFilter::push_comp` refuses the same tree, and it refuses it after the recursion that
    /// read the body has already returned — which on a hostile body is after the stack is gone.
    /// The number is the caller's either way; what this asserts is where it binds.
    #[test]
    fn a_filter_tree_past_the_bound_is_refused_before_the_stack_is_spent() {
        let limits = Limits::DEFAULT.with_max_xml_depth(4);
        let mut wire: Vec<u8> = Vec::new();
        wire.extend_from_slice(
            br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:">
            <D:prop/><C:filter>"#,
        );
        for _ in 0..2048_u32 {
            wire.extend_from_slice(br#"<C:comp-filter name="VEVENT">"#);
        }
        for _ in 0..2048_u32 {
            wire.extend_from_slice(b"</C:comp-filter>");
        }
        wire.extend_from_slice(b"</C:filter></C:calendar-query>");
        let (outcome, _) = read_under(&wire, limits, UnknownPolicy::Skip);
        assert_eq!(outcome, Err(DavError::Limit(LimitExceeded::Depth)));
    }

    /// A property list past the caller's bound is refused by the charged push.
    #[test]
    fn a_property_request_past_the_cap_is_refused() {
        let limits = Limits::DEFAULT.with_max_props_per_response(1);
        let wire = br#"<D:propfind xmlns:D="DAV:"><D:prop>
            <D:getetag/><D:displayname/><D:getcontenttype/></D:prop></D:propfind>"#;
        let (outcome, _) = read_under(wire, limits, UnknownPolicy::Skip);
        assert_eq!(outcome, Err(DavError::Limit(LimitExceeded::Properties)));
    }

    /// A foreign element is skipped with a diagnostic, or refuses the body, as the caller says.
    #[test]
    fn a_foreign_element_is_the_caller_s_policy_and_never_this_crate_s() {
        let wire = br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"
            xmlns:D="DAV:" xmlns:V="http://vendor.example/ns">
            <D:prop><D:getetag/></D:prop>
            <C:filter><V:magic-filter><V:inside/></V:magic-filter>
            <C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#;
        let (skipped, reported) = read_under(wire, Limits::DEFAULT, UnknownPolicy::Skip);
        let Ok(RequestBody::CalendarQuery(query)) = skipped else {
            panic!("the vendor element is skipped and the request survives");
        };
        assert_eq!(
            query.filter.as_ref().map(CompFilter::name),
            Some(&b"VCALENDAR"[..])
        );
        assert_eq!(
            reported.first().copied().map(Diagnostic::code),
            Some(DiagnosticCode::DavForeignElementSkipped)
        );
        let (refused, _) = read_under(wire, Limits::DEFAULT, UnknownPolicy::Reject);
        assert_eq!(refused, Err(DavError::Foreign));
    }

    /// A property with no row is a name, not an element to skip.
    #[test]
    fn a_property_this_crate_has_no_row_for_is_kept_by_name() {
        let wire = br#"<D:propfind xmlns:D="DAV:" xmlns:CS="http://calendarserver.org/ns/">
            <D:prop><D:getetag/><CS:getctag/><CS:pushkey/></D:prop></D:propfind>"#;
        let Ok(RequestBody::PropFind(PropFind::Props(wanted))) = read(wire) else {
            panic!("a propfind naming three properties");
        };
        let names = wanted.names();
        assert_eq!(names.len(), 3);
        assert_eq!(names.first(), Some(&PropName::Known(ElementName::Getetag)));
        assert_eq!(names.get(1), Some(&PropName::Known(ElementName::Getctag)));
        let PropName::Extension(unknown) = names.get(2).unwrap() else {
            panic!("a property with no row is kept by name rather than dropped");
        };
        assert_eq!(unknown.local_name(), b"pushkey");
        assert_eq!(unknown.namespace(), Namespace::CALENDARSERVER_URI);
    }

    /// A `propfind` names exactly one of the three, and `include` belongs to `allprop`.
    #[test]
    fn a_propfind_names_one_of_allprop_propname_and_prop() {
        let with_include = br#"<propfind xmlns="DAV:">
            <allprop/><include><getcontentlength/></include></propfind>"#;
        let Ok(RequestBody::PropFind(PropFind::AllProp(wanted))) = read(with_include) else {
            panic!("allprop with the expensive property it omits named beside it");
        };
        assert_eq!(
            wanted.names(),
            [PropName::Known(ElementName::Getcontentlength)]
        );
        assert_eq!(
            read(br#"<propfind xmlns="DAV:"><propname/></propfind>"#),
            Ok(RequestBody::PropFind(PropFind::Names))
        );
        assert_eq!(
            read(br#"<propfind xmlns="DAV:"><propname/><allprop/></propfind>"#),
            Err(DavError::Unexpected(ElementName::Allprop))
        );
        assert_eq!(
            read(br#"<propfind xmlns="DAV:"/>"#),
            Err(DavError::Unexpected(ElementName::Propfind))
        );
    }

    /// A multiget names resources, and the answer a server builds reports them per property.
    ///
    /// The second half is the same shared shape read from the other end: one `href` carrying
    /// `calendar-data` at `200` beside a name at `404` is what RFC 4918 section 14.24 permits,
    /// and `successful_value` is the only correct way to ask what actually arrived.
    #[test]
    fn a_multiget_names_resources_a_server_answers_per_property() {
        let wire = br#"<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><d:getetag/><c:calendar-data/></d:prop>
  <d:href>/calendars/ann/work/1.ics</d:href>
  <d:href>/calendars/ann/work/2.ics</d:href>
</c:calendar-multiget>"#;
        let Ok(RequestBody::CalendarMultiget(multiget)) = read(wire) else {
            panic!("a multiget naming two resources");
        };
        assert_eq!(
            multiget.props.names(),
            [PropName::Known(ElementName::Getetag)]
        );
        assert!(multiget.props.calendar_data.is_some());
        let asked: Vec<&[u8]> = multiget.hrefs().iter().map(Href::as_bytes).collect();
        assert_eq!(
            asked,
            [
                b"/calendars/ann/work/1.ics".as_slice(),
                b"/calendars/ann/work/2.ics".as_slice()
            ]
        );

        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let first = multiget.hrefs().first().unwrap().clone();
        let mut answer = DavResponse::with_propstats(first, limits);
        let mut returned = PropStat::new(Status::OK, limits);
        returned
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Getetag),
                    value: PropValue::Empty,
                },
                &mut meter,
            )
            .unwrap();
        let mut absent = PropStat::new(Status::NOT_FOUND, limits);
        absent
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::Empty,
                },
                &mut meter,
            )
            .unwrap();
        answer.push_propstat(returned, &mut meter).unwrap();
        answer.push_propstat(absent, &mut meter).unwrap();
        let etag = PropName::Known(ElementName::Getetag);
        let payload = PropName::Known(ElementName::CalendarData);
        assert!(answer.successful_value(&etag).is_some());
        assert!(answer.successful_value(&payload).is_none());
    }

    /// A `calendar-data` request selects components and properties, and bounds its own nesting.
    #[test]
    fn a_calendar_data_request_selects_what_comes_back() {
        let wire = br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><C:calendar-data>
    <C:comp name="VCALENDAR"><C:prop name="VERSION"/>
      <C:comp name="VEVENT"><C:allprop/></C:comp></C:comp>
    <C:limit-recurrence-set start="20260101T000000Z" end="20260108T000000Z"/>
  </C:calendar-data></D:prop>
  <C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;
        let Ok(RequestBody::CalendarQuery(query)) = read(wire) else {
            panic!("a query asking for part of each object");
        };
        let wanted = query.props.calendar_data.unwrap();
        assert_eq!(
            wanted.limit_recurrence_set,
            Some(
                TimeRange::new(
                    Some(Instant::from_unix_seconds(YEAR_START)),
                    Some(Instant::from_unix_seconds(WEEK_LATER))
                )
                .unwrap()
            )
        );
        let root = wanted.comp.unwrap();
        assert_eq!(root.name(), b"VCALENDAR");
        let named = root.props().first().unwrap();
        assert_eq!(&**named, b"VERSION");
        let inner = root.comps().first().unwrap();
        assert_eq!(inner.name(), b"VEVENT");
        assert!(inner.all_props);
    }

    /// A `text-match` carries its collation, its negation, and every octet of its value.
    #[test]
    fn a_text_match_keeps_its_collation_its_negation_and_its_spaces() {
        let wire = br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/></D:prop>
  <C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
    <C:prop-filter name="ATTENDEE">
      <C:text-match collation="i;octet"
        negate-condition="yes">mailto:ann@example.invalid</C:text-match>
      <C:param-filter name="PARTSTAT">
        <C:text-match collation="i;ascii-casemap">NEEDS-ACTION </C:text-match>
      </C:param-filter>
    </C:prop-filter>
  </C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#;
        let Ok(RequestBody::CalendarQuery(query)) = read(wire) else {
            panic!("a query filtering on an attendee");
        };
        let occasions = query.filter.unwrap().comps().first().unwrap().clone();
        let attendee = occasions.props().first().unwrap();
        assert_eq!(attendee.name(), b"ATTENDEE");
        let matcher = attendee.text_match.as_ref().unwrap();
        assert_eq!(matcher.collation, Collation::Octet);
        assert!(matcher.negate);
        assert_eq!(matcher.value(), b"mailto:ann@example.invalid");
        let parameter = attendee.params().first().unwrap();
        assert_eq!(parameter.name(), b"PARTSTAT");
        let inner = parameter.text_match.as_ref().unwrap();
        assert_eq!(inner.collation, Collation::AsciiCasemap);
        // A trailing space in a search string is a character somebody typed.
        assert_eq!(inner.value(), b"NEEDS-ACTION ");
    }

    /// A `negate-condition` outside its enumeration is refused rather than read as `no`.
    #[test]
    fn an_attribute_value_outside_its_enumeration_is_refused() {
        let wire = br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop/><C:filter><C:comp-filter name="VEVENT"><C:prop-filter name="SUMMARY">
  <C:text-match negate-condition="perhaps">x</C:text-match>
  </C:prop-filter></C:comp-filter></C:filter></C:calendar-query>"#;
        assert_eq!(
            read(wire),
            Err(DavError::Invalid(ValueError::AttributeValue))
        );
    }

    /// A `PROPFIND` body is one shape a `REPORT` body is not, and the root says which.
    #[test]
    fn the_root_element_says_which_body_arrived() {
        assert!(matches!(
            read(br#"<propfind xmlns="DAV:"><propname/></propfind>"#),
            Ok(RequestBody::PropFind(_))
        ));
        assert_eq!(
            read(br#"<multistatus xmlns="DAV:"/>"#),
            Err(DavError::Unexpected(ElementName::Multistatus))
        );
    }

    /// A value's references are resolved and the indentation around it is not part of it.
    #[test]
    fn character_data_is_decoded_and_its_layout_dropped() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let wire = br#"<D:href xmlns:D="DAV:">  /calendars/ann/a&amp;b.ics
        </D:href>"#;
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut events = Tokenizer::new(wire);
        // Open the element the way `RequestBody::read` would before handing over.
        let opened = events.next_event(&mut context).unwrap();
        assert!(matches!(opened, Some(XmlEvent::Start { .. })));
        let collected = read_text(&mut events, &mut context, Whitespace::Layout).unwrap();
        assert_eq!(collected, b"/calendars/ann/a&b.ics".to_vec());
    }

    /// Without the feature, an RFC 6578 `REPORT` is refused rather than answered differently.
    #[test]
    fn a_build_without_sync_collection_refuses_the_report_it_cannot_honor() {
        if crate::internal::dav::SYNC_COLLECTION_ENABLED {
            return;
        }
        let wire = br#"<D:sync-collection xmlns:D="DAV:">
            <D:sync-token>http://example.invalid/ns/sync/42</D:sync-token>
            <D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></D:sync-collection>"#;
        assert_eq!(
            read(wire),
            Err(DavError::Unsupported(ElementName::SyncCollection))
        );
    }

    /// With the feature, the token round-trips through the client without being interpreted.
    #[test]
    fn a_sync_collection_carries_an_opaque_token_a_level_and_a_limit() {
        if !crate::internal::dav::SYNC_COLLECTION_ENABLED {
            return;
        }
        let wire = br#"<D:sync-collection xmlns:D="DAV:">
  <D:sync-token>http://example.invalid/ns/sync/42</D:sync-token>
  <D:sync-level>infinite</D:sync-level>
  <D:limit><D:nresults>100</D:nresults></D:limit>
  <D:prop><D:getetag/></D:prop>
</D:sync-collection>"#;
        let Ok(RequestBody::SyncCollection(request)) = read(wire) else {
            panic!("a sync-collection REPORT");
        };
        assert_eq!(
            request
                .token
                .as_ref()
                .map(crate::internal::dav::value::SyncToken::as_bytes),
            Some(b"http://example.invalid/ns/sync/42".as_slice())
        );
        assert_eq!(
            request.level,
            crate::internal::dav::request::SyncLevel::Infinite
        );
        assert_eq!(request.limit, Some(100));
        assert_eq!(
            request.props.names(),
            [PropName::Known(ElementName::Getetag)]
        );
    }

    /// An empty `sync-token` is the absence of one, which is what asks for a full enumeration.
    #[test]
    fn an_empty_sync_token_asks_for_an_initial_synchronization() {
        if !crate::internal::dav::SYNC_COLLECTION_ENABLED {
            return;
        }
        let wire = br#"<D:sync-collection xmlns:D="DAV:"><D:sync-token/>
            <D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></D:sync-collection>"#;
        let Ok(RequestBody::SyncCollection(request)) = read(wire) else {
            panic!("a sync-collection REPORT");
        };
        assert_eq!(request.token, None);
        assert_eq!(request.level, crate::internal::dav::request::SyncLevel::One);
        assert_eq!(request.limit, None);
    }
}
