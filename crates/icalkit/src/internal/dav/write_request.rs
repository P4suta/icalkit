// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The client direction: every request body of [`crate::internal::dav::request`], as octets.
//!
//! Nothing here is a client type. These are [`crate::internal::dav::WriteXml`] implementations on the values
//! `read_request.rs` reads back, which is DP-15's structural test made mechanical: a field
//! meaningful in only one direction would have to appear on one side of this seam and not the
//! other, and there is no such field. A client encodes a `calendar-query` and a server decodes
//! it; a server encodes the multistatus that answers it and a client decodes that. The
//! direction shows up in which trait is called and in the `Limits` the caller passes.
//!
//! # What a body looks like
//!
//! Compact: the XML declaration, then elements with no insignificant whitespace between them.
//! Servers do not read indentation and a client that sends it pays for it on every request; a
//! deterministic byte string is also what makes the wire column of a test table an assertion
//! rather than an approximation.
//!
//! The prefixes are this crate's own fixed `D:`, `C:` and `CS:`, all declared on every root
//! element by the shared stack-balanced writer. That
//! they are an output choice and never an input assumption is `element.rs`'s subject: a peer
//! writing `d:`, or a default declaration, or a different prefix per element, is read
//! correctly by this crate's own reader. A fixed preamble makes request and response output
//! canonical and keeps namespace selection out of the individual body encoders.
//!
//! A property outside the closed vocabulary is written under the writer's scoped `X:` prefix,
//! declared on that element alone. Two adjacent extension properties may therefore use
//! unrelated namespaces without changing any fixed binding.
//!
//! # What this unit owns that nothing else does
//!
//! The `YYYYMMDDTHHMMSSZ` an RFC 4791 section 9.9 `time-range` attribute carries.
//! [`crate::internal::core::Instant`] is a bare timeline point with no civil spelling of its own, so
//! [`write_utc_date_time`] asks `ical-core` for the arithmetic and reports an instant that has
//! no such spelling as [`ValueError::TimeUnrepresentable`]. It never clamps to one that does:
//! a `time-range` silently moved to a year that fits is a query returning the wrong events and
//! saying nothing about it.
//!
//! # What is charged
//!
//! One element for every element opened, and the octets of every variable-length payload — a
//! component name, an `href`, a `text-match` value, a sync token — against the shared ledger.
//! Nesting is bounded against `Limits::max_xml_depth` by the encoder itself rather than by the
//! value: [`CompFilter::push_comp`] refuses a tree past that bound at construction and
//! [`CompSelection::push_comp`] takes no bounds at all, so this recursion is the only thing
//! between a caller's own deeply nested selection and a blown stack.

use crate::internal::core::{
    CivilDateTime, DateTimeValue, EncodeValue, Instant, Limits, Meter, UtcOffset, ValueBuf,
};

use crate::internal::dav::codec::WriteXml;
use crate::internal::dav::element::ElementName;
use crate::internal::dav::failure::{DavError, ValueError};
use crate::internal::dav::request::{
    CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, QueryShape, TextMatch,
    TimeRange,
};
use crate::internal::dav::sink::ByteSink;
use crate::internal::dav::value::ExtensionName;
use crate::internal::dav::writer::XmlWriter;

/// Write the `YYYYMMDDTHHMMSSZ` an RFC 4791 section 9.9 `time-range` attribute carries.
///
/// The one civil rendering this crate performs. An [`Instant`] is a point on the timeline with
/// no spelling of its own, and the years RFC 5545 section 3.3.4 can write are four digits, so
/// an instant outside them is refused as [`ValueError::TimeUnrepresentable`] rather than moved
/// to one that fits. Nothing is written on that path: the octets are rendered before the sink
/// is touched, so a refusal leaves the caller's buffer as it was.
pub fn write_utc_date_time(out: &mut dyn ByteSink, at: Instant) -> Result<(), DavError> {
    let rendered = utc_date_time_bytes(at)?;
    out.write(rendered.as_bytes())?;
    Ok(())
}

/// The `YYYYMMDDTHHMMSSZ` octets of an instant, or the refusal that it has none.
fn utc_date_time_bytes(at: Instant) -> Result<ValueBuf, DavError> {
    // UTC rather than a caller-chosen offset: section 9.9 admits only the `Z` form, and an
    // offset would be a second way to write one instant that a server would normalize back.
    let stamp =
        CivilDateTime::from_instant(at, UtcOffset::UTC).ok_or(ValueError::TimeUnrepresentable)?;
    let mut rendered = ValueBuf::new();
    DateTimeValue::Utc(stamp)
        .encode_value(&mut rendered)
        .map_err(|_refused| ValueError::TimeUnrepresentable)?;
    Ok(rendered)
}

/// Whether a request asks for any property at all.
fn asks_for_a_property(wanted: &PropRequest) -> bool {
    !wanted.names().is_empty() || wanted.calendar_data.is_some()
}

/// Whether a `calendar-data` request asks for a shape rather than the whole object.
fn asks_for_a_shape(request: &CalendarDataRequest) -> bool {
    request.comp.is_some()
        || request.expand.is_some()
        || request.limit_recurrence_set.is_some()
        || request.limit_freebusy_set.is_some()
}

/// Whether a component selection names anything inside the component.
fn selects_inside(selection: &CompSelection) -> bool {
    selection.all_props
        || selection.all_comps
        || !selection.props().is_empty()
        || !selection.comps().is_empty()
}

/// The octets an encoder writes into, and the bounds it writes under.
///
/// Two lifetimes rather than one because the sink and the ledger belong to the caller
/// separately: draining many bodies into one buffer under one meter, which is the aggregate
/// shape `docs/adr/0010` exists for, does not make them agree.
struct Encoder<'out, 'ledger> {
    /// The one stack-balanced writer used by every DAV body encoder.
    writer: XmlWriter<'out>,
    /// The caller's running ledger.
    meter: &'ledger mut Meter,
}

impl<'out, 'ledger> Encoder<'out, 'ledger> {
    /// An encoder for an element fragment below a root owned by its caller.
    fn new(out: &'out mut dyn ByteSink, _limits: Limits, meter: &'ledger mut Meter) -> Self {
        Self {
            writer: XmlWriter::fragment(out),
            meter,
        }
    }

    /// An encoder for a complete XML request document.
    fn document(out: &'out mut dyn ByteSink, meter: &'ledger mut Meter) -> Self {
        Self {
            writer: XmlWriter::new(out),
            meter,
        }
    }
}

impl Encoder<'_, '_> {
    /// Write `<P:local`, leaving the start tag open for attributes.
    fn open(&mut self, name: ElementName) -> Result<(), DavError> {
        self.writer.open(name, self.meter)
    }

    /// End a start tag whose element has children or character data.
    fn begin_content(&mut self) -> Result<(), DavError> {
        self.writer.begin_content(self.meter)
    }

    /// End a start tag whose element has neither.
    fn close_empty(&mut self) -> Result<(), DavError> {
        self.writer.close(self.meter)
    }

    /// Write `</P:local>`.
    fn close(&mut self, _name: ElementName) -> Result<(), DavError> {
        self.writer.close(self.meter)
    }

    /// Write one attribute of the start tag that is open.
    ///
    /// The value goes through the shared writer's escaping, which preserves what XML 1.0
    /// section 3.3.3's attribute-value normalization would otherwise replace with a space.
    fn attribute(&mut self, name: &[u8], value: &[u8]) -> Result<(), DavError> {
        self.writer.attribute(name, value, self.meter)
    }

    /// Write the declaration and the root element's start tag.
    fn open_root(&mut self, name: ElementName) -> Result<(), DavError> {
        self.open(name)?;
        self.begin_content()
    }

    /// An element with no attributes and no content.
    fn empty_element(&mut self, name: ElementName) -> Result<(), DavError> {
        self.writer.empty(name, self.meter)
    }

    /// An element whose content is character data.
    fn text_element(&mut self, name: ElementName, bytes: &[u8]) -> Result<(), DavError> {
        self.writer.element_text(name, bytes, self.meter)
    }

    /// An element whose content is a decimal count.
    ///
    /// A fixed buffer rather than an allocation: ten digits hold every `u32`, and an encoder
    /// that allocated to write a number would allocate on the path a caller chose a
    /// [`crate::internal::dav::SliceSink`] to avoid.
    ///
    /// Compiled with the feature that needs it. `DAV:nresults` is the only count any request
    /// body of this crate carries, and an uncalled helper is a warning under `-D warnings`.
    fn count_element(&mut self, name: ElementName, count: u32) -> Result<(), DavError> {
        let mut buffer = [b'0'; 10];
        let mut at = buffer.len();
        let mut left = count;
        loop {
            at = at.saturating_sub(1);
            let digit = u8::try_from(left.checked_rem(10).unwrap_or(0)).unwrap_or(0);
            if let Some(slot) = buffer.get_mut(at) {
                *slot = b'0'.saturating_add(digit);
            }
            left = left.checked_div(10).unwrap_or(0);
            if left == 0 {
                break;
            }
        }
        let rendered = buffer.get(at..).unwrap_or(b"0");
        self.writer.element_text(name, rendered, self.meter)
    }

    /// A property name outside the closed vocabulary, in its own namespace.
    fn extension_element(&mut self, name: &ExtensionName) -> Result<(), DavError> {
        self.writer.empty_extension(name, self.meter)
    }

    /// One requested property name, however this crate knows it.
    fn prop_name(&mut self, name: &PropName) -> Result<(), DavError> {
        match name {
            PropName::Known(known) => self.empty_element(*known),
            PropName::Extension(foreign) => self.extension_element(foreign),
        }
    }

    /// A property-name list: `DAV:prop` in a request, `DAV:include` beside an `allprop`.
    fn prop_list(&mut self, name: ElementName, wanted: &PropRequest) -> Result<(), DavError> {
        self.open(name)?;
        self.begin_content()?;
        for asked in wanted.names() {
            // The structured request supersedes the bare name. A caller that both pushed
            // `CALDAV:calendar-data` and set `calendar_data` asked for one element, and
            // RFC 4791 section 9.6's grammar admits it once.
            let superseded = wanted.calendar_data.is_some()
                && *asked == PropName::Known(ElementName::CalendarData);
            if !superseded {
                self.prop_name(asked)?;
            }
        }
        if let Some(request) = wanted.calendar_data.as_ref() {
            self.calendar_data(request)?;
        }
        self.close(name)
    }

    /// A window, as the empty element and two optional attributes RFC 4791 section 9.9 gives.
    ///
    /// The element name is a parameter because four elements carry this shape: `time-range`
    /// itself, and `expand`, `limit-recurrence-set` and `limit-freebusy-set` beside a
    /// `calendar-data` request.
    fn range(&mut self, name: ElementName, window: TimeRange) -> Result<(), DavError> {
        self.open(name)?;
        // Independently optional, because section 9.9 permits an open start and an open end.
        // The constructor already refused the range that states neither, so an absent bound
        // here is a bound the caller meant to leave open rather than one nobody supplied.
        if let Some(from) = window.start() {
            let rendered = utc_date_time_bytes(from)?;
            self.attribute(b"start", rendered.as_bytes())?;
        }
        if let Some(until) = window.end() {
            let rendered = utc_date_time_bytes(until)?;
            self.attribute(b"end", rendered.as_bytes())?;
        }
        self.close_empty()
    }

    /// A substring test, RFC 4791 section 9.7.5.
    fn text_match(&mut self, test: &TextMatch) -> Result<(), DavError> {
        self.open(ElementName::TextMatch)?;
        // RFC 4791 section 7.5 states `i;ascii-casemap` as the default, so writing it tells a
        // server nothing it did not already assume and costs every request the octets.
        if !matches!(test.collation, Collation::AsciiCasemap) {
            self.attribute(b"collation", test.collation.as_bytes())?;
        }
        if test.negate {
            self.attribute(b"negate-condition", b"yes")?;
        }
        self.begin_content()?;
        self.writer.text(test.value(), self.meter)?;
        self.close(ElementName::TextMatch)
    }

    /// A parameter filter, RFC 4791 section 9.7.3.
    fn param_filter(&mut self, filter: &ParamFilter) -> Result<(), DavError> {
        if filter.is_contradictory() {
            return Err(ValueError::FilterContradiction.into());
        }
        self.open(ElementName::ParamFilter)?;
        self.attribute(b"name", filter.name())?;
        if filter.is_not_defined {
            self.begin_content()?;
            self.empty_element(ElementName::IsNotDefined)?;
            return self.close(ElementName::ParamFilter);
        }
        let Some(test) = filter.text_match.as_ref() else {
            return self.close_empty();
        };
        self.begin_content()?;
        self.text_match(test)?;
        self.close(ElementName::ParamFilter)
    }

    /// A property filter, RFC 4791 section 9.7.2.
    fn prop_filter(&mut self, filter: &PropFilter) -> Result<(), DavError> {
        if filter.is_contradictory() {
            return Err(ValueError::FilterContradiction.into());
        }
        self.open(ElementName::PropFilter)?;
        self.attribute(b"name", filter.name())?;
        if filter.is_not_defined {
            self.begin_content()?;
            self.empty_element(ElementName::IsNotDefined)?;
            return self.close(ElementName::PropFilter);
        }
        let bare = filter.time_range.is_none()
            && filter.text_match.is_none()
            && filter.params().is_empty();
        if bare {
            return self.close_empty();
        }
        self.begin_content()?;
        if let Some(window) = filter.time_range {
            self.range(ElementName::TimeRange, window)?;
        }
        if let Some(test) = filter.text_match.as_ref() {
            self.text_match(test)?;
        }
        for constraint in filter.params() {
            self.param_filter(constraint)?;
        }
        self.close(ElementName::PropFilter)
    }

    /// A component filter and everything under it, RFC 4791 section 9.7.1.
    ///
    /// Recursive over a tree [`CompFilter::push_comp`] already bounded at construction, and
    /// bounded again here, because the bound a value carries and the bound an encoder observes
    /// are the caller's one number read from two ends.
    fn comp_filter(&mut self, filter: &CompFilter) -> Result<(), DavError> {
        if filter.is_contradictory() {
            return Err(ValueError::FilterContradiction.into());
        }
        self.open(ElementName::CompFilter)?;
        self.attribute(b"name", filter.name())?;
        if filter.is_not_defined {
            self.begin_content()?;
            self.empty_element(ElementName::IsNotDefined)?;
            return self.close(ElementName::CompFilter);
        }
        let bare =
            filter.time_range.is_none() && filter.props().is_empty() && filter.comps().is_empty();
        if bare {
            return self.close_empty();
        }
        self.begin_content()?;
        if let Some(window) = filter.time_range {
            self.range(ElementName::TimeRange, window)?;
        }
        for constraint in filter.props() {
            self.prop_filter(constraint)?;
        }
        for nested in filter.comps() {
            self.comp_filter(nested)?;
        }
        self.close(ElementName::CompFilter)
    }

    /// A component selection and everything under it, RFC 4791 section 9.6.1.
    ///
    /// This is the recursion with no bound in the value: [`CompSelection::push_comp`] takes no
    /// `Limits` and refuses no depth, so [`Encoder::enter`] is what stands between a caller's
    /// own selection tree and a blown stack.
    fn comp_selection(&mut self, selection: &CompSelection) -> Result<(), DavError> {
        // RFC 4791 section 9.6.1's grammar is `comp ((allprop | prop*), (allcomp | comp*))`, so
        // "every property" and "these properties" are alternatives and a value stating both is
        // one no body can express. The crate's own precedent for an inexpressible value is a
        // refusal — a `CompFilter` that states a condition and its own negation is
        // `ValueError::FilterContradiction` — and the third answer, writing `allprop` and
        // dropping the named properties, sends a request the client never wrote and never
        // tells it so.
        if (selection.all_props && !selection.props().is_empty())
            || (selection.all_comps && !selection.comps().is_empty())
        {
            return Err(DavError::Invalid(ValueError::SelectionContradiction));
        }
        self.open(ElementName::CalendarDataComp)?;
        self.attribute(b"name", selection.name())?;
        if !selects_inside(selection) {
            return self.close_empty();
        }
        self.begin_content()?;
        if selection.all_props {
            self.empty_element(ElementName::CalendarDataAllprop)?;
        } else {
            for wanted in selection.props() {
                self.open(ElementName::CalendarDataProp)?;
                self.attribute(b"name", wanted)?;
                self.close_empty()?;
            }
        }
        if selection.all_comps {
            self.empty_element(ElementName::CalendarDataAllcomp)?;
        } else {
            for nested in selection.comps() {
                self.comp_selection(nested)?;
            }
        }
        self.close(ElementName::CalendarDataComp)
    }

    /// What shape a `calendar-data` payload should come back in, RFC 4791 section 9.6.
    fn calendar_data(&mut self, request: &CalendarDataRequest) -> Result<(), DavError> {
        self.open(ElementName::CalendarData)?;
        if !asks_for_a_shape(request) {
            return self.close_empty();
        }
        self.begin_content()?;
        if let Some(selection) = request.comp.as_ref() {
            self.comp_selection(selection)?;
        }
        // Section 9.6's grammar reads `(comp?, (expand | limit-recurrence-set)?,
        // limit-freebusy-set?)`, so the middle two are alternatives there and independent
        // fields here. A value carrying both is written as it stands rather than quietly
        // reduced to one: a caller that set two windows asked two questions, and choosing one
        // on their behalf would send a query nobody wrote.
        if let Some(window) = request.expand {
            self.range(ElementName::Expand, window)?;
        }
        if let Some(window) = request.limit_recurrence_set {
            self.range(ElementName::LimitRecurrenceSet, window)?;
        }
        if let Some(window) = request.limit_freebusy_set {
            self.range(ElementName::LimitFreebusySet, window)?;
        }
        self.close(ElementName::CalendarData)
    }
}

impl WriteXml for PropFind {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        _limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut encoder = Encoder::document(out, meter);
        encoder.open_root(ElementName::Propfind)?;
        match self {
            Self::AllProp(wanted) => {
                encoder.empty_element(ElementName::Allprop)?;
                // RFC 4918 section 14.8 makes `include` optional, and one naming nothing asks
                // for nothing — so an empty request writes no element rather than an empty one.
                if asks_for_a_property(wanted) {
                    encoder.prop_list(ElementName::Include, wanted)?;
                }
            },
            Self::Names => encoder.empty_element(ElementName::Propname)?,
            Self::Props(wanted) => encoder.prop_list(ElementName::Prop, wanted)?,
        }
        encoder.close(ElementName::Propfind)
    }
}

impl WriteXml for CalendarQuery {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        _limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut encoder = Encoder::document(out, meter);
        encoder.open_root(ElementName::CalendarQuery)?;
        match self.shape {
            QueryShape::AllProp => encoder.empty_element(ElementName::Allprop)?,
            QueryShape::Names => encoder.empty_element(ElementName::Propname)?,
            QueryShape::Named => {
                if asks_for_a_property(&self.props) {
                    encoder.prop_list(ElementName::Prop, &self.props)?;
                }
            },
        }
        // An absent filter asks for every resource, which is what the field says it does.
        // RFC 4791 section 9.5's grammar requires the element, so a strict server refuses a
        // query built that way — and writing an empty `filter` instead would be this crate
        // answering a question the caller did not, with a body that reads back as a value
        // nobody wrote.
        if let Some(filter) = self.filter.as_ref() {
            encoder.open(ElementName::Filter)?;
            encoder.begin_content()?;
            encoder.comp_filter(filter)?;
            encoder.close(ElementName::Filter)?;
        }
        // Section 9.5 puts the zone after the filter, which is also where a reader of this
        // body needs it: it says what a floating window in that filter is resolved against.
        if let Some(zone) = self.timezone.as_ref() {
            encoder.text_element(ElementName::Timezone, zone.as_bytes())?;
        }
        encoder.close(ElementName::CalendarQuery)
    }
}

impl WriteXml for CalendarMultiget {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        _limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut encoder = Encoder::document(out, meter);
        encoder.open_root(ElementName::CalendarMultiget)?;
        if asks_for_a_property(&self.props) {
            encoder.prop_list(ElementName::Prop, &self.props)?;
        }
        for href in self.hrefs() {
            encoder.text_element(ElementName::Href, href.as_bytes())?;
        }
        encoder.close(ElementName::CalendarMultiget)
    }
}

impl WriteXml for FreeBusyQuery {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        _limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut encoder = Encoder::document(out, meter);
        encoder.open_root(ElementName::FreeBusyQuery)?;
        encoder.range(ElementName::TimeRange, self.range)?;
        encoder.close(ElementName::FreeBusyQuery)
    }
}

impl WriteXml for crate::internal::dav::request::SyncCollection {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        _limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut encoder = Encoder::document(out, meter);
        encoder.open_root(ElementName::SyncCollection)?;
        match self.token.as_ref() {
            Some(token) => encoder.text_element(ElementName::SyncToken, token.as_bytes())?,
            // RFC 6578 section 3 requires the element and makes an empty one the request for
            // an initial enumeration, so an absent token is written rather than omitted. The
            // octets are the server's own and travel back through here uninterpreted.
            None => encoder.empty_element(ElementName::SyncToken)?,
        }
        encoder.text_element(ElementName::SyncLevel, self.level.as_bytes())?;
        if let Some(most) = self.limit {
            encoder.open(ElementName::Limit)?;
            encoder.begin_content()?;
            encoder.count_element(ElementName::Nresults, most)?;
            encoder.close(ElementName::Limit)?;
        }
        encoder.prop_list(ElementName::Prop, &self.props)?;
        encoder.close(ElementName::SyncCollection)
    }
}

impl WriteXml for PropRequest {
    /// Written as `DAV:prop`, which is where a request's property list lives everywhere except
    /// beside an `allprop`; that one place writes `DAV:include` through the same code.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).prop_list(ElementName::Prop, self)
    }
}

impl WriteXml for CompFilter {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).comp_filter(self)
    }
}

impl WriteXml for PropFilter {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).prop_filter(self)
    }
}

impl WriteXml for ParamFilter {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).param_filter(self)
    }
}

impl WriteXml for TextMatch {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).text_match(self)
    }
}

impl WriteXml for TimeRange {
    /// Written as `CALDAV:time-range`. The three other elements carrying this shape —
    /// `expand`, `limit-recurrence-set` and `limit-freebusy-set` — belong to the
    /// `calendar-data` request that holds them and are written there.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).range(ElementName::TimeRange, *self)
    }
}

impl WriteXml for CompSelection {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).comp_selection(self)
    }
}

impl WriteXml for CalendarDataRequest {
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        Encoder::new(out, limits, meter).calendar_data(self)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{Instant, LimitExceeded, Limits, Meter};

    use super::write_utc_date_time;
    use crate::internal::dav::codec::WriteXml;
    use crate::internal::dav::element::ElementName;
    use crate::internal::dav::failure::{DavError, SinkFull, SyntaxError, ValueError};
    use crate::internal::dav::request::{
        CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
        FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, TextMatch,
        TimeRange,
    };
    use crate::internal::dav::sink::SliceSink;
    use crate::internal::dav::value::{ExtensionName, Href};

    /// `20060104T000000Z`, the start of RFC 4791 section 7.8.1's own `time-range`.
    const JANUARY_4: i64 = 1_136_332_800;
    /// `20060105T000000Z`, its end.
    const JANUARY_5: i64 = 1_136_419_200;

    /// The root a `PROPFIND` opens with, which three of the cases below share.
    const PROPFIND_ROOT: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">";

    /// The window RFC 4791 section 7.8.1's example asks over.
    fn january() -> TimeRange {
        let from = Instant::from_unix_seconds(JANUARY_4);
        let until = Instant::from_unix_seconds(JANUARY_5);
        TimeRange::new(Some(from), Some(until)).unwrap()
    }

    /// Encode a body under the caller's bounds, or report why it could not be encoded.
    fn encode(
        value: &dyn WriteXml,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<Vec<u8>, DavError> {
        let mut out: Vec<u8> = Vec::new();
        value.write_xml(&mut out, limits, meter)?;
        Ok(out)
    }

    /// Encode under `Limits::DEFAULT` and a fresh ledger, for a case that is not about bounds.
    fn wire(value: &dyn WriteXml) -> Vec<u8> {
        let mut meter = Meter::new(Limits::DEFAULT);
        encode(value, Limits::DEFAULT, &mut meter).unwrap()
    }

    /// `prefix` followed by `suffix`, for a case that shares a root with its neighbors.
    fn joined(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
        let mut whole = prefix.to_vec();
        whole.extend_from_slice(suffix);
        whole
    }

    #[test]
    fn request_roots_use_the_shared_canonical_preamble() {
        let body = wire(&PropFind::Names);
        assert!(body.starts_with(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">"
        ));
    }

    #[test]
    fn a_calendar_query_is_the_body_rfc_4791_section_7_8_1_shows() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut query = CalendarQuery::new(limits);
        query
            .props
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        query.props.calendar_data = Some(CalendarDataRequest::default());
        let mut events = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
        events.time_range = Some(january());
        let mut calendar = CompFilter::new(b"VCALENDAR", limits, &mut meter).unwrap();
        calendar.push_comp(events, limits, &mut meter).unwrap();
        query.filter = Some(calendar);

        // The example of RFC 4791 section 7.8.1 with its indentation removed: the element
        // names, the attribute names, the nesting and the `YYYYMMDDTHHMMSSZ` spelling are the
        // RFC's own, not this writer's habits written down twice.
        let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<C:calendar-query xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<D:prop><D:getetag/><C:calendar-data/></D:prop>\
<C:filter><C:comp-filter name=\"VCALENDAR\"><C:comp-filter name=\"VEVENT\">\
<C:time-range start=\"20060104T000000Z\" end=\"20060105T000000Z\"/>\
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>";
        assert_eq!(encode(&query, limits, &mut meter).unwrap(), expected);
    }

    #[test]
    fn a_multiget_names_every_resource_it_was_given() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut multiget = CalendarMultiget::new(limits);
        multiget
            .props
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        multiget.props.calendar_data = Some(CalendarDataRequest::default());
        for path in [
            b"/calendars/ann/work/1.ics".as_slice(),
            b"/calendars/ann/work/2.ics".as_slice(),
        ] {
            let href = Href::new(path, limits, &mut meter).unwrap();
            multiget.push_href(href, &mut meter).unwrap();
        }

        // RFC 4791 section 7.9.1's shape: the property list once, then one `href` per resource.
        let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<C:calendar-multiget xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<D:prop><D:getetag/><C:calendar-data/></D:prop>\
<D:href>/calendars/ann/work/1.ics</D:href>\
<D:href>/calendars/ann/work/2.ics</D:href></C:calendar-multiget>";
        assert_eq!(encode(&multiget, limits, &mut meter).unwrap(), expected);
    }

    #[test]
    fn a_free_busy_query_carries_one_window_and_nothing_else() {
        let query = FreeBusyQuery { range: january() };
        let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<C:free-busy-query xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<C:time-range start=\"20060104T000000Z\" end=\"20060105T000000Z\"/></C:free-busy-query>";
        assert_eq!(wire(&query), expected);
    }

    #[test]
    fn the_three_propfind_shapes_are_the_three_rfc_4918_section_9_1_defines() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut named = PropRequest::new(limits);
        named
            .push(PropName::Known(ElementName::Resourcetype), &mut meter)
            .unwrap();
        named
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        assert_eq!(
            wire(&PropFind::Props(named)),
            joined(
                PROPFIND_ROOT,
                b"<D:prop><D:resourcetype/><D:getetag/></D:prop></D:propfind>"
            )
        );

        assert_eq!(
            wire(&PropFind::Names),
            joined(PROPFIND_ROOT, b"<D:propname/></D:propfind>")
        );

        // An `allprop` that includes nothing writes no `include`: RFC 4918 section 14.8 makes
        // the element optional and one naming nothing asks for nothing.
        assert_eq!(
            wire(&PropFind::AllProp(PropRequest::new(limits))),
            joined(PROPFIND_ROOT, b"<D:allprop/></D:propfind>")
        );
    }

    #[test]
    fn the_fixed_namespaces_are_declared_on_every_request_root() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut wanted = PropRequest::new(limits);
        wanted
            .push(PropName::Known(ElementName::Getctag), &mut meter)
            .unwrap();

        // `getctag` is the property every widely deployed client polls a collection with, and
        // it lives in `http://calendarserver.org/ns/` rather than in `DAV:`.
        let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\"><D:allprop/>\
<D:include><CS:getctag/></D:include></D:propfind>";
        assert_eq!(wire(&PropFind::AllProp(wanted)), expected);

        let plain = wire(&PropFind::Names);
        assert!(plain.windows(8).any(|at| at == b"xmlns:CS"));
    }

    #[test]
    fn a_property_outside_the_vocabulary_is_written_in_its_own_namespace() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let vendor = b"http://apple.com/ns/ical/".as_slice();
        let color = ExtensionName::new(vendor, b"calendar-color", &mut meter).unwrap();
        let mut wanted = PropRequest::new(limits);
        wanted.push(PropName::Extension(color), &mut meter).unwrap();

        // The shared writer's scoped extension prefix is rebound on the element itself.
        assert_eq!(
            wire(&PropFind::Props(wanted)),
            joined(
                PROPFIND_ROOT,
                b"<D:prop><X:calendar-color xmlns:X=\"http://apple.com/ns/ical/\"/></D:prop>\
</D:propfind>"
            )
        );
    }

    #[test]
    fn a_name_that_is_not_an_xml_name_is_refused_rather_than_written() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let foreign = b"http://x.invalid/ns/".as_slice();
        let broken = ExtensionName::new(foreign, b"has space", &mut meter).unwrap();
        let mut wanted = PropRequest::new(limits);
        wanted
            .push(PropName::Extension(broken), &mut meter)
            .unwrap();
        assert_eq!(
            encode(&PropFind::Props(wanted), limits, &mut meter),
            Err(DavError::Syntax(SyntaxError::Malformed))
        );
    }

    #[test]
    fn both_time_range_bounds_are_independently_optional_on_the_way_out() {
        let early = Instant::from_unix_seconds(JANUARY_4);
        let late = Instant::from_unix_seconds(JANUARY_5);
        // RFC 4791 section 9.9 permits an open start and an open end. The attribute a bound
        // does not have is absent rather than written as an extreme that means something else.
        assert_eq!(
            wire(&TimeRange::starting_at(early)),
            b"<C:time-range start=\"20060104T000000Z\"/>"
        );
        assert_eq!(
            wire(&TimeRange::ending_before(late)),
            b"<C:time-range end=\"20060105T000000Z\"/>"
        );
        assert_eq!(
            wire(&january()),
            b"<C:time-range start=\"20060104T000000Z\" end=\"20060105T000000Z\"/>"
        );
    }

    #[test]
    fn an_instant_with_no_utc_spelling_is_reported_rather_than_clamped() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let beyond = Instant::from_unix_seconds(i64::MAX);
        assert_eq!(
            encode(&TimeRange::starting_at(beyond), limits, &mut meter),
            Err(DavError::Invalid(ValueError::TimeUnrepresentable))
        );

        // The same refusal through the door a caller rendering the value itself would use,
        // and the spelling for an instant that has one.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            write_utc_date_time(&mut out, beyond),
            Err(DavError::Invalid(ValueError::TimeUnrepresentable))
        );
        assert!(out.is_empty());
        write_utc_date_time(&mut out, Instant::EPOCH).unwrap();
        assert_eq!(out, b"19700101T000000Z");
    }

    #[test]
    fn the_default_collation_is_left_unwritten_and_any_other_is_stated() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut test = TextMatch::new(b"Ann", &mut meter).unwrap();
        // RFC 4791 section 7.5 states `i;ascii-casemap` as the default, so writing it says
        // nothing a server did not already assume and costs every request the octets.
        assert_eq!(wire(&test), b"<C:text-match>Ann</C:text-match>");

        test.collation = Collation::Octet;
        assert_eq!(
            wire(&test),
            b"<C:text-match collation=\"i;octet\">Ann</C:text-match>"
        );

        test.negate = true;
        assert_eq!(
            wire(&test),
            b"<C:text-match collation=\"i;octet\" negate-condition=\"yes\">Ann</C:text-match>"
        );
    }

    #[test]
    fn text_that_would_otherwise_be_markup_is_escaped_on_the_way_out() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let test = TextMatch::new(b"a<b&c\r\n]]>", &mut meter).unwrap();
        // `>` is escaped unconditionally, so a `]]>` a caller took out of a `DESCRIPTION`
        // cannot close a section this crate never opens; a `CR` becomes `&#13;` because a
        // literal one is folded away by XML 1.0 section 2.11 before any reader sees it.
        assert_eq!(
            wire(&test),
            b"<C:text-match>a&lt;b&amp;c&#13;\n]]&gt;</C:text-match>"
        );
    }

    #[test]
    fn a_filter_stating_a_condition_and_its_negation_is_refused_rather_than_written() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut events = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
        events.is_not_defined = true;
        events.time_range = Some(TimeRange::starting_at(Instant::EPOCH));
        assert_eq!(
            encode(&events, limits, &mut meter),
            Err(DavError::Invalid(ValueError::FilterContradiction))
        );

        // The same rule one layer down, where RFC 4791 section 9.7.2's grammar makes
        // `is-not-defined` exclusive with everything beside it.
        let mut summary = PropFilter::new(b"SUMMARY", limits, &mut meter).unwrap();
        summary.is_not_defined = true;
        summary.text_match = Some(TextMatch::new(b"Ann", &mut meter).unwrap());
        assert_eq!(
            encode(&summary, limits, &mut meter),
            Err(DavError::Invalid(ValueError::FilterContradiction))
        );
    }

    #[test]
    fn a_negated_filter_writes_the_one_element_its_grammar_admits() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut absent = ParamFilter::new(b"PARTSTAT", &mut meter).unwrap();
        absent.is_not_defined = true;
        assert_eq!(
            wire(&absent),
            b"<C:param-filter name=\"PARTSTAT\"><C:is-not-defined/></C:param-filter>"
        );
    }

    #[test]
    fn a_property_filter_writes_its_test_and_its_parameters_in_the_grammars_order() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut attendee = PropFilter::new(b"ATTENDEE", limits, &mut meter).unwrap();
        let address = TextMatch::new(b"mailto:ann@example.invalid", &mut meter).unwrap();
        attendee.text_match = Some(address);
        let mut partstat = ParamFilter::new(b"PARTSTAT", &mut meter).unwrap();
        partstat.text_match = Some(TextMatch::new(b"NEEDS-ACTION", &mut meter).unwrap());
        attendee.push_param(partstat, &mut meter).unwrap();

        // The nesting of RFC 4791 section 9.7.2's own example: an `ATTENDEE` whose value
        // matches, with a `PARTSTAT` test under it. The RFC's example spells the default
        // collation out on both `text-match` elements and this writer leaves it unwritten,
        // which section 7.5 permits and which is the one deliberate difference.
        let expected: &[u8] = b"<C:prop-filter name=\"ATTENDEE\">\
<C:text-match>mailto:ann@example.invalid</C:text-match>\
<C:param-filter name=\"PARTSTAT\"><C:text-match>NEEDS-ACTION</C:text-match>\
</C:param-filter></C:prop-filter>";
        assert_eq!(wire(&attendee), expected);
    }

    #[test]
    fn a_calendar_data_request_writes_the_subtree_rfc_4791_section_9_6_1_shows() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut event = CompSelection::new(b"VEVENT", limits, &mut meter).unwrap();
        event.push_prop(b"SUMMARY", &mut meter).unwrap();
        event.push_prop(b"UID", &mut meter).unwrap();
        let mut calendar = CompSelection::new(b"VCALENDAR", limits, &mut meter).unwrap();
        calendar.push_comp(event, &mut meter).unwrap();
        let request = CalendarDataRequest {
            expand: Some(january()),
            limit_recurrence_set: None,
            limit_freebusy_set: None,
            comp: Some(calendar),
        };

        let expected: &[u8] = b"<C:calendar-data>\
<C:comp name=\"VCALENDAR\"><C:comp name=\"VEVENT\">\
<C:prop name=\"SUMMARY\"/><C:prop name=\"UID\"/></C:comp></C:comp>\
<C:expand start=\"20060104T000000Z\" end=\"20060105T000000Z\"/></C:calendar-data>";
        assert_eq!(wire(&request), expected);

        // A request that asks for no shape is the empty element every client sends.
        assert_eq!(wire(&CalendarDataRequest::default()), b"<C:calendar-data/>");
    }

    #[test]
    fn the_structured_calendar_data_request_supersedes_the_bare_name() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);

        // The bare name alone is how a caller asks for the whole payload.
        let mut named = PropRequest::new(limits);
        named
            .push(PropName::Known(ElementName::CalendarData), &mut meter)
            .unwrap();
        assert_eq!(wire(&named), b"<D:prop><C:calendar-data/></D:prop>");

        // Both together are one request and not two: RFC 4791 section 9.6's grammar admits
        // the element once, and a body naming it twice is one a server may refuse outright.
        let mut layered = PropRequest::new(limits);
        layered
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        layered
            .push(PropName::Known(ElementName::CalendarData), &mut meter)
            .unwrap();
        layered.calendar_data = Some(CalendarDataRequest::default());
        assert_eq!(
            wire(&layered),
            b"<D:prop><D:getetag/><C:calendar-data/></D:prop>"
        );
    }

    #[test]
    fn a_sync_collection_is_the_body_rfc_6578_section_3_defines() {
        use crate::internal::dav::request::{SyncCollection, SyncLevel};
        use crate::internal::dav::value::SyncToken;

        if !crate::internal::dav::SYNC_COLLECTION_ENABLED {
            return;
        }
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let issued = b"http://example.invalid/ns/sync/1234".as_slice();
        let mut request = SyncCollection::new(limits);
        request.token = Some(SyncToken::new(issued, limits, &mut meter).unwrap());
        request.level = SyncLevel::One;
        request.limit = Some(100);
        request
            .props
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();

        let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:sync-collection xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<D:sync-token>http://example.invalid/ns/sync/1234</D:sync-token>\
<D:sync-level>1</D:sync-level><D:limit><D:nresults>100</D:nresults></D:limit>\
<D:prop><D:getetag/></D:prop></D:sync-collection>";
        assert_eq!(encode(&request, limits, &mut meter).unwrap(), expected);

        // No token is the initial enumeration, and RFC 6578 section 3 spells that as the empty
        // element rather than as an absent one.
        let initial = SyncCollection::new(limits);
        let written = encode(&initial, limits, &mut meter).unwrap();
        assert!(
            written.windows(15).any(|at| at == b"<D:sync-token/>"),
            "an initial synchronization writes the element empty"
        );
    }

    #[test]
    fn a_selection_deeper_than_the_caller_admits_is_refused_rather_than_recursed_over() {
        // `CompSelection::push_comp` takes no bounds, so a tree can be built to whatever depth
        // the heap allows. The encoder is the only thing that refuses to walk it.
        let limits = Limits::DEFAULT.with_max_xml_depth(3);
        let mut meter = Meter::new(limits);
        let mut nested = CompSelection::new(b"VALARM", limits, &mut meter).unwrap();
        for _ in 0..3 {
            let mut parent = CompSelection::new(b"VEVENT", limits, &mut meter).unwrap();
            parent.push_comp(nested, &mut meter).unwrap();
            nested = parent;
        }
        assert_eq!(
            encode(&nested, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Depth))
        );
    }

    #[test]
    fn the_encoder_charges_the_ledger_it_was_handed() {
        let limits = Limits::DEFAULT.with_max_xml_elements(2);
        let mut meter = Meter::new(limits);
        let mut wanted = PropRequest::new(limits);
        wanted
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .unwrap();
        // `propfind` and `prop` fit; `getetag` is the third element and is refused.
        assert_eq!(
            encode(&PropFind::Props(wanted), limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Elements))
        );
    }

    #[test]
    fn many_bodies_under_one_ledger_are_bounded_in_aggregate() {
        let limits = Limits::DEFAULT;
        let mut building = Meter::new(limits);
        let mut multiget = CalendarMultiget::new(limits);
        let href = Href::new(b"/calendars/ann/work/1.ics", limits, &mut building).unwrap();
        multiget.push_href(href, &mut building).unwrap();

        // Four octets of budget will not carry a twenty-five octet `href`, whatever the caps
        // on the collections holding it say. One ledger across many bodies is the whole of
        // `docs/adr/0010`'s aggregate argument.
        let mut writing = Meter::with_budget(limits, 4);
        assert_eq!(
            encode(&multiget, limits, &mut writing),
            Err(DavError::Limit(LimitExceeded::Budget))
        );
    }

    #[test]
    fn a_sink_with_no_room_refuses_rather_than_emitting_a_prefix() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut buffer = [0_u8; 16];
        let mut sink = SliceSink::new(&mut buffer);
        let query = CalendarQuery::new(limits);
        assert_eq!(
            query.write_xml(&mut sink, limits, &mut meter),
            Err(DavError::Output(SinkFull))
        );
        // The declaration alone is thirty-eight octets, so nothing at all fitted and the sink
        // still holds nothing rather than half a document.
        assert!(sink.is_empty());
    }
}
