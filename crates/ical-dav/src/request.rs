// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The request bodies, which a client writes and a server reads out of the same types.
//!
//! Nothing here has a field that is meaningful in only one direction. A `calendar-query` is
//! the same value whether it was built by a client about to `REPORT` or read by a server about
//! to answer one; the direction shows up in which of [`crate::WriteXml`] and
//! [`crate::ReadXml`] is called and in the `Limits` the caller passes, never in which fields
//! exist. That is DP-15's structural test and it is the reason this module and
//! [`crate::response`] are the whole of the protocol's vocabulary.
//!
//! A `time-range` is carried here and evaluated nowhere. Deciding which instances of a
//! recurring event fall inside one is `ical-recur`'s work and this crate does not depend on
//! it; a server composes the two.

use alloc::boxed::Box;

use ical_core::{Instant, LimitExceeded, Limits, Meter};

use crate::bound::Bounded;
use crate::element::ElementName;
use crate::failure::{DavError, ValueError};
use crate::response::CalendarPayload;
use crate::value::{ExtensionName, Href, bounded_cap, copy};

/// The name of a property, whether or not this crate has a row for it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PropName {
    /// A property of the closed vocabulary.
    Known(ElementName),
    /// A property outside it, kept by name rather than dropped.
    Extension(ExtensionName),
}

/// What a request asks for, and what shape a `calendar-data` payload should come back in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropRequest {
    /// The names. Private behind a charged push, like every other collection here.
    names: Bounded<PropName>,
    /// The `CALDAV:calendar-data` request, when the payload itself is wanted.
    pub calendar_data: Option<CalendarDataRequest>,
}

impl PropRequest {
    /// An empty request under the caller's bounds.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            names: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
            calendar_data: None,
        }
    }

    /// Ask for one more property.
    pub fn push(&mut self, name: PropName, meter: &mut Meter) -> Result<(), DavError> {
        self.names.push(name, meter)
    }

    /// The names asked for.
    #[must_use]
    pub fn names(&self) -> &[PropName] {
        self.names.as_slice()
    }
}

/// A `PROPFIND` body, RFC 4918 section 14.20.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PropFind {
    /// `DAV:allprop`, with the RFC 4918 section 14.8 `include` list beside it.
    ///
    /// The list is not redundant: `allprop` deliberately omits the expensive properties, and
    /// `include` is the only way to ask for one of those without naming every cheap one too.
    AllProp(PropRequest),
    /// `DAV:propname` — the names a resource carries, with no values.
    Names,
    /// `DAV:prop` — exactly these.
    Props(PropRequest),
}

/// Which of RFC 4791 section 9.5's three property shapes a `calendar-query` asks with.
///
/// The production is
/// `calendar-query ((DAV:allprop | DAV:propname | DAV:prop)?, filter, timezone?)`, so all three
/// are bodies a conformant client sends and a server has to read. A field that was a property
/// list and nothing else could express one of them: a client here could not send `allprop` and
/// a server here refused the body outright with `DavError::Unexpected`, which is a conformant
/// request rejected rather than a shape nobody uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueryShape {
    /// `DAV:prop` — exactly the properties named beside it.
    #[default]
    Named,
    /// `DAV:allprop` — every property the server volunteers.
    AllProp,
    /// `DAV:propname` — the names a resource carries, with no values.
    Names,
}

/// A `CALDAV:calendar-query` body, RFC 4791 section 9.5.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarQuery {
    /// Which of the three property shapes section 9.5 defines this query asks with.
    pub shape: QueryShape,
    /// What to return for each matching resource, when the shape names properties.
    pub props: PropRequest,
    /// Which resources match. Absent asks for all of them.
    pub filter: Option<CompFilter>,
    /// The zone a floating `time-range` in the filter is resolved in, section 9.9.
    ///
    /// Carried because the alternative is not "ignored" but "answered differently": a server
    /// that resolves a floating window in a zone of its own choosing answers a question the
    /// client did not ask, and a proxy that re-encodes the query without the zone changes what
    /// the query means. The value is an iCalendar object, so it travels as a payload with its
    /// line-ending witness beside it like every other one.
    pub timezone: Option<CalendarPayload>,
}

impl CalendarQuery {
    /// A query that asks for nothing and matches everything.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            shape: QueryShape::Named,
            props: PropRequest::new(limits),
            filter: None,
            timezone: None,
        }
    }
}

/// A `CALDAV:calendar-multiget` body, RFC 4791 section 9.10.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarMultiget {
    /// What to return for each named resource.
    pub props: PropRequest,
    /// Which resources to return. Capped at the same number as the responses that answer them.
    hrefs: Bounded<Href>,
}

impl CalendarMultiget {
    /// A multiget that names nothing yet.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            props: PropRequest::new(limits),
            hrefs: Bounded::with_cap(
                bounded_cap(limits.max_responses()),
                LimitExceeded::Responses,
            ),
        }
    }

    /// Name one more resource.
    pub fn push_href(&mut self, href: Href, meter: &mut Meter) -> Result<(), DavError> {
        self.hrefs.push(href, meter)
    }

    /// The resources named.
    #[must_use]
    pub fn hrefs(&self) -> &[Href] {
        self.hrefs.as_slice()
    }
}

/// A `CALDAV:free-busy-query` body, RFC 4791 section 9.11.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FreeBusyQuery {
    /// The window to report busy time over.
    pub range: TimeRange,
}

/// How far below a collection an RFC 6578 synchronization reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncLevel {
    /// The collection's own members.
    #[default]
    One,
    /// Every member below it.
    Infinite,
}

impl SyncLevel {
    /// The element content RFC 6578 section 6.3 writes.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::One => b"1",
            Self::Infinite => b"infinite",
        }
    }

    /// Read a `DAV:sync-level`.
    pub fn parse(value: &[u8]) -> Result<Self, DavError> {
        match value {
            b"1" => Ok(Self::One),
            b"infinite" => Ok(Self::Infinite),
            _ => Err(DavError::Invalid(ValueError::SyncLevel)),
        }
    }
}

/// A `DAV:sync-collection` body, RFC 6578 section 3.
#[cfg(feature = "sync-collection")]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncCollection {
    /// The token the last synchronization ended at. Absent asks for an initial enumeration.
    pub token: Option<crate::value::SyncToken>,
    /// How far below the collection to reach.
    pub level: SyncLevel,
    /// The most responses the client is willing to receive, RFC 5323 section 5.17.
    pub limit: Option<u32>,
    /// What to return for each changed resource.
    pub props: PropRequest,
}

#[cfg(feature = "sync-collection")]
impl SyncCollection {
    /// An initial synchronization of a collection's own members.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            token: None,
            level: SyncLevel::One,
            limit: None,
            props: PropRequest::new(limits),
        }
    }
}

/// A window, with two independently optional bounds.
///
/// RFC 4791 section 9.9 permits an open start and an open end and requires at least one bound,
/// which an `Option<(Instant, Instant)>` cannot express: it can say "no window" and "both
/// ends" and neither of the two shapes a real query uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeRange {
    /// The first instant inside the window, if it has one.
    start: Option<Instant>,
    /// The first instant after it, if it has one.
    end: Option<Instant>,
}

impl TimeRange {
    /// A window with the bounds it was given, which must not be neither and must not invert.
    pub const fn new(start: Option<Instant>, end: Option<Instant>) -> Result<Self, DavError> {
        match (start, end) {
            (None, None) => Err(DavError::Invalid(ValueError::TimeRangeUnbounded)),
            (Some(from), Some(until)) if until.unix_seconds() <= from.unix_seconds() => {
                Err(DavError::Invalid(ValueError::TimeRangeInverted))
            },
            _ => Ok(Self { start, end }),
        }
    }

    /// A window open at its end.
    #[must_use]
    pub const fn starting_at(start: Instant) -> Self {
        Self {
            start: Some(start),
            end: None,
        }
    }

    /// A window open at its start.
    #[must_use]
    pub const fn ending_before(end: Instant) -> Self {
        Self {
            start: None,
            end: Some(end),
        }
    }

    /// The first instant inside the window, if it has one.
    #[must_use]
    pub const fn start(self) -> Option<Instant> {
        self.start
    }

    /// The first instant after the window, if it has one.
    #[must_use]
    pub const fn end(self) -> Option<Instant> {
        self.end
    }
}

/// How a `CALDAV:calendar-data` payload should come back, RFC 4791 section 9.6.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarDataRequest {
    /// `CALDAV:expand` — return instances rather than the recurrence rule, over this window.
    pub expand: Option<TimeRange>,
    /// `CALDAV:limit-recurrence-set` — keep only the overrides inside this window.
    pub limit_recurrence_set: Option<TimeRange>,
    /// `CALDAV:limit-freebusy-set` — keep only the `VFREEBUSY` periods inside this window.
    pub limit_freebusy_set: Option<TimeRange>,
    /// The component subtree to return. Absent asks for the whole object.
    pub comp: Option<CompSelection>,
}

impl CalendarDataRequest {
    /// Whether honoring this request answers with less than the resource holds.
    ///
    /// `CALDAV:expand`, `limit-recurrence-set`, `limit-freebusy-set` and `comp` each return a
    /// calendar that is *not* what the server stored, and the octets that come back say nothing
    /// about it — they are well-formed iCalendar, so a caller that writes them back deletes what
    /// was left out. This is the one question a caller has to ask before doing that, and asking
    /// it used to mean inspecting four fields and knowing which of them reduce.
    ///
    /// `false` for a request that asks for the whole object, which is the only shape
    /// `docs/adr/0001`'s round trip survives.
    #[must_use]
    pub const fn is_reducing(&self) -> bool {
        self.expand.is_some()
            || self.limit_recurrence_set.is_some()
            || self.limit_freebusy_set.is_some()
            || self.comp.is_some()
    }
}

/// Which components and properties of a calendar object to return, RFC 4791 section 9.6.1.
///
/// A tree, because the request is one: `VCALENDAR` containing `VEVENT` containing named
/// properties is the shape every client that asks for less than everything sends.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompSelection {
    /// The component name this selection is about, `VCALENDAR` at the root.
    name: Box<[u8]>,
    /// `CALDAV:allcomp` — return every subcomponent.
    pub all_comps: bool,
    /// `CALDAV:allprop` — return every property.
    pub all_props: bool,
    /// The named properties to return.
    props: Bounded<Box<[u8]>>,
    /// The named subcomponents to return.
    comps: Bounded<CompSelection>,
}

impl CompSelection {
    /// A selection naming one component and nothing inside it.
    pub fn new(name: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        meter.try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        Ok(Self {
            name: copy(name)?,
            all_comps: false,
            all_props: false,
            props: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
            comps: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
        })
    }

    /// The component name.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Ask for one named property of this component.
    pub fn push_prop(&mut self, name: &[u8], meter: &mut Meter) -> Result<(), DavError> {
        meter.try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        self.props.push(copy(name)?, meter)
    }

    /// Ask for one named subcomponent.
    pub fn push_comp(&mut self, child: Self, meter: &mut Meter) -> Result<(), DavError> {
        self.comps.push(child, meter)
    }

    /// The named properties asked for.
    #[must_use]
    pub fn props(&self) -> &[Box<[u8]>] {
        self.props.as_slice()
    }

    /// The named subcomponents asked for.
    #[must_use]
    pub fn comps(&self) -> &[Self] {
        self.comps.as_slice()
    }

    /// Whether the selection states "everything" and names things beside it.
    ///
    /// RFC 4791 section 9.6.1 writes `comp ((allprop | prop*), (allcomp | comp*))`, so the two
    /// halves of each pair are alternatives: a value holding both is one no body can express,
    /// and reducing it to one of them silently would answer a request nobody wrote.
    /// [`ValueError::SelectionContradiction`] is the refusal, and this is the predicate that
    /// finds it — the sibling [`CompFilter::is_contradictory`] and
    /// [`PropFilter::is_contradictory`] have always had.
    ///
    /// It was missing until an evaluator outside this crate needed it, which is the evidence
    /// `docs/adr/0012` predicted: a filter representation is complete exactly when the thing
    /// that consumes it can be written without reaching past the accessors.
    #[must_use]
    pub fn is_contradictory(&self) -> bool {
        (self.all_props && !self.props.is_empty()) || (self.all_comps && !self.comps.is_empty())
    }
}

/// Which components a `calendar-query` matches, RFC 4791 section 9.7.1.
///
/// The tree bounds its own nesting at construction: [`CompFilter::push_comp`] refuses a child
/// that would put the subtree past `Limits::max_xml_depth`, so a tree that exists is a tree an
/// encoder may recurse over without overflowing a stack — and a server decoding an untrusted
/// `REPORT` gets the same bound from the same field, on the way in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompFilter {
    /// The component name, `VCALENDAR` at the root.
    name: Box<[u8]>,
    /// `CALDAV:is-not-defined` — match resources that do *not* carry this component.
    pub is_not_defined: bool,
    /// The window this component must overlap.
    pub time_range: Option<TimeRange>,
    /// The subcomponent filters.
    comps: Bounded<CompFilter>,
    /// The property filters.
    props: Bounded<PropFilter>,
    /// The height of this subtree, one for a leaf.
    height: u16,
}

impl CompFilter {
    /// A filter naming one component.
    pub fn new(name: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        meter.try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        Ok(Self {
            name: copy(name)?,
            is_not_defined: false,
            time_range: None,
            comps: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
            props: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
            height: 1,
        })
    }

    /// The component name.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// The height of this subtree, one for a leaf.
    #[must_use]
    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Nest one filter inside this one, refusing a tree deeper than the caller admits.
    pub fn push_comp(
        &mut self,
        child: Self,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let grown = child
            .height
            .checked_add(1)
            .ok_or(DavError::Limit(LimitExceeded::Depth))?
            .max(self.height);
        if grown > limits.max_xml_depth() {
            return Err(DavError::Limit(LimitExceeded::Depth));
        }
        self.comps.push(child, meter)?;
        self.height = grown;
        Ok(())
    }

    /// Add one property filter.
    pub fn push_prop(&mut self, filter: PropFilter, meter: &mut Meter) -> Result<(), DavError> {
        self.props.push(filter, meter)
    }

    /// The subcomponent filters.
    #[must_use]
    pub fn comps(&self) -> &[Self] {
        self.comps.as_slice()
    }

    /// The property filters.
    #[must_use]
    pub fn props(&self) -> &[PropFilter] {
        self.props.as_slice()
    }

    /// Whether the filter states a condition and its own negation.
    ///
    /// RFC 4791 section 9.7.1 makes `is-not-defined` exclusive with every other test in the
    /// same filter, because a component that is not there has no time range and no properties.
    #[must_use]
    pub fn is_contradictory(&self) -> bool {
        self.is_not_defined
            && (self.time_range.is_some() || !self.comps.is_empty() || !self.props.is_empty())
    }
}

/// Which properties a `calendar-query` matches, RFC 4791 section 9.7.2.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropFilter {
    /// The property name.
    name: Box<[u8]>,
    /// `CALDAV:is-not-defined` — match components that do *not* carry this property.
    pub is_not_defined: bool,
    /// The window this property's value must overlap.
    pub time_range: Option<TimeRange>,
    /// The substring test on its value.
    pub text_match: Option<TextMatch>,
    /// The parameter filters.
    params: Bounded<ParamFilter>,
}

impl PropFilter {
    /// A filter naming one property.
    pub fn new(name: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        meter.try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        Ok(Self {
            name: copy(name)?,
            is_not_defined: false,
            time_range: None,
            text_match: None,
            params: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
        })
    }

    /// The property name.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Add one parameter filter.
    pub fn push_param(&mut self, filter: ParamFilter, meter: &mut Meter) -> Result<(), DavError> {
        self.params.push(filter, meter)
    }

    /// The parameter filters.
    #[must_use]
    pub fn params(&self) -> &[ParamFilter] {
        self.params.as_slice()
    }

    /// Whether the filter states a condition and its own negation.
    #[must_use]
    pub fn is_contradictory(&self) -> bool {
        self.is_not_defined
            && (self.time_range.is_some() || self.text_match.is_some() || !self.params.is_empty())
    }
}

/// Which parameters a `calendar-query` matches, RFC 4791 section 9.7.3.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamFilter {
    /// The parameter name.
    name: Box<[u8]>,
    /// `CALDAV:is-not-defined` — match properties that do *not* carry this parameter.
    pub is_not_defined: bool,
    /// The substring test on its value.
    pub text_match: Option<TextMatch>,
}

impl ParamFilter {
    /// A filter naming one parameter.
    pub fn new(name: &[u8], meter: &mut Meter) -> Result<Self, DavError> {
        meter.try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        Ok(Self {
            name: copy(name)?,
            is_not_defined: false,
            text_match: None,
        })
    }

    /// The parameter name.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Whether the filter states a condition and its own negation.
    #[must_use]
    pub const fn is_contradictory(&self) -> bool {
        self.is_not_defined && self.text_match.is_some()
    }
}

/// A substring test, RFC 4791 section 9.7.5.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextMatch {
    /// The text to look for.
    value: Box<[u8]>,
    /// How to compare, RFC 4791 section 7.5.
    pub collation: Collation,
    /// Whether a *non*-match is what matches.
    pub negate: bool,
}

impl TextMatch {
    /// A test for `value` under the default collation.
    pub fn new(value: &[u8], meter: &mut Meter) -> Result<Self, DavError> {
        meter.try_charge_bytes(u64::try_from(value.len()).unwrap_or(u64::MAX))?;
        Ok(Self {
            value: copy(value)?,
            collation: Collation::AsciiCasemap,
            negate: false,
        })
    }

    /// The text to look for.
    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

/// How a [`TextMatch`] compares, RFC 4791 section 7.5.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Collation {
    /// `i;ascii-casemap`, the default: case-insensitive over ASCII and octet-exact elsewhere.
    #[default]
    AsciiCasemap,
    /// `i;octet`.
    Octet,
    /// A collation this crate has no name for, kept as the peer wrote it.
    Other(Box<[u8]>),
}

impl Collation {
    /// The attribute value this collation is written as.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::AsciiCasemap => b"i;ascii-casemap",
            Self::Octet => b"i;octet",
            Self::Other(name) => name,
        }
    }

    /// Classify a `collation` attribute value.
    pub fn parse(value: &[u8]) -> Result<Self, DavError> {
        match value {
            b"i;ascii-casemap" => Ok(Self::AsciiCasemap),
            b"i;octet" => Ok(Self::Octet),
            other => Ok(Self::Other(copy(other)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use ical_core::{Instant, LimitExceeded, Limits, Meter};

    use super::{CompFilter, PropFilter, PropName, PropRequest, TimeRange};
    use crate::element::ElementName;
    use crate::failure::{DavError, ValueError};

    #[test]
    fn both_time_range_bounds_are_independently_optional() {
        let early = Instant::from_unix_seconds(1_767_225_600);
        let late = Instant::from_unix_seconds(1_767_830_400);
        assert!(TimeRange::new(Some(early), None).is_ok());
        assert!(TimeRange::new(None, Some(late)).is_ok());
        assert!(TimeRange::new(Some(early), Some(late)).is_ok());
        assert_eq!(
            TimeRange::new(None, None),
            Err(DavError::Invalid(ValueError::TimeRangeUnbounded))
        );
        assert_eq!(
            TimeRange::new(Some(late), Some(early)),
            Err(DavError::Invalid(ValueError::TimeRangeInverted))
        );
    }

    #[test]
    fn a_filter_tree_is_bounded_at_the_depth_the_caller_stated() {
        let limits = Limits::DEFAULT.with_max_xml_depth(3);
        let mut meter = Meter::new(limits);
        let mut nested = CompFilter::new(b"VALARM", limits, &mut meter).unwrap();
        for _ in 0..2 {
            let mut parent = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
            parent.push_comp(nested, limits, &mut meter).unwrap();
            nested = parent;
        }
        let mut root = CompFilter::new(b"VCALENDAR", limits, &mut meter).unwrap();
        assert_eq!(
            root.push_comp(nested, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Depth))
        );
    }

    #[test]
    fn a_filter_that_states_a_condition_and_its_negation_says_so() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut filter = PropFilter::new(b"SUMMARY", limits, &mut meter).unwrap();
        assert!(!filter.is_contradictory());
        filter.is_not_defined = true;
        assert!(!filter.is_contradictory());
        filter.time_range = Some(TimeRange::starting_at(Instant::EPOCH));
        assert!(filter.is_contradictory());
    }

    #[test]
    fn a_property_request_is_capped_where_the_caller_capped_it() {
        let limits = Limits::DEFAULT.with_max_props_per_response(1);
        let mut meter = Meter::new(limits);
        let mut wanted = PropRequest::new(limits);
        wanted
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        assert_eq!(
            wanted.push(PropName::Known(ElementName::CalendarData), &mut meter),
            Err(DavError::Limit(LimitExceeded::Properties))
        );
    }
}
