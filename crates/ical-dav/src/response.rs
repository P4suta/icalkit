// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The multistatus, which a server writes and a client reads out of the same types.
//!
//! One `href` carries divergent per-property statuses — `getetag` at `200` beside
//! `calendar-data` at `403` — which is ordinary rather than exotic, so a response is either
//! one status for the whole resource or a list of [`PropStat`]s and never a flat `status: u16`
//! (RFC 4918 section 14.24). [`DavResponse::successful_value`] reads across every propstat
//! whose status is a success, which is the only correct way to ask "did I get the object"
//! once statuses can diverge.
//!
//! [`MultiStatus`] is the owned form and one consumer of [`ResponseSource`], never a second
//! path into the same octets: [`MultiStatus::read`] drives the public streaming interface and
//! there is no private fast path for the two to drift apart along.

use alloc::boxed::Box;

use ical_core::{DiagnosticCode, LimitExceeded, Limits, Meter, Severity};

use crate::bound::Bounded;
use crate::codec::ResponseSource;
use crate::failure::DavError;
use crate::policy::DecodeContext;
use crate::request::PropName;
use crate::text::{DecodedText, LineEndings};
use crate::value::{ETag, Href, ResourceType, Status, SyncToken, bounded_cap, copy};

/// An iCalendar object as it traveled inside `CALDAV:calendar-data`, with its provenance.
///
/// The octets and the answer to "are these the octets the server stored" are one value,
/// because they are one question. A caller that means to `PUT` this payload back reads
/// [`CalendarPayload::is_as_sent`] first: `false` means this reader folded the line endings
/// under [`crate::TextPolicy::Normalized`], so writing them back would change the resource and
/// its `ETag` without anybody having edited it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarPayload {
    /// The octets, opaque to this crate and parsed by `ical-core`.
    octets: Box<[u8]>,
    /// What terminates their lines, and whether this reader changed that.
    line_endings: LineEndings,
}

impl CalendarPayload {
    /// A payload from a decoded run of character data.
    pub fn from_text(
        text: &DecodedText<'_>,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<Self, DavError> {
        let octets = text.run.as_bytes();
        charge_payload(octets.len(), limits, meter)?;
        Ok(Self {
            octets: copy(octets)?,
            line_endings: text.line_endings,
        })
    }

    /// A payload from an `.ics` a server already holds.
    ///
    /// The witness is read off the octets, because a server writing a payload it stored is
    /// sending exactly what it stored — there is no read to have changed anything.
    pub fn from_octets(octets: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        charge_payload(octets.len(), limits, meter)?;
        Ok(Self {
            octets: copy(octets)?,
            line_endings: LineEndings::of(octets),
        })
    }

    /// The same payload, with the witness that this read cost it its carriage returns.
    ///
    /// A payload assembled out of several runs cannot take its witness from the octets it ends
    /// up with — folding is a fact about the *read*, and once the `CR`s are gone the octets no
    /// longer say that they were ever there.
    #[must_use]
    pub fn into_folded(mut self) -> Self {
        self.line_endings = LineEndings::Folded;
        self
    }

    /// The octets.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.octets
    }

    /// What terminates the payload's lines.
    #[must_use]
    pub const fn line_endings(&self) -> LineEndings {
        self.line_endings
    }

    /// Whether these are the octets the peer wrote.
    #[must_use]
    pub const fn is_as_sent(&self) -> bool {
        self.line_endings.is_as_sent()
    }
}

/// Charge a payload against the per-element ceiling and the shared budget.
fn charge_payload(length: usize, limits: Limits, meter: &mut Meter) -> Result<(), DavError> {
    let length = u32::try_from(length).map_err(|_| LimitExceeded::Text)?;
    if length > limits.max_xml_text_bytes() {
        return Err(DavError::Limit(LimitExceeded::Text));
    }
    meter.try_charge_bytes(u64::from(length))?;
    Ok(())
}

/// A property's value, in whichever of the shapes the vocabulary gives it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PropValue {
    /// The element was present and empty, which is what a `propname` answer carries.
    Empty,
    /// Character data.
    Text(Box<[u8]>),
    /// A `DAV:href` inside the property, as `calendar-home-set` and `owner` carry.
    Reference(Href),
    /// A `DAV:resourcetype`.
    Resource(ResourceType),
    /// A `DAV:getetag` or a `CALDAV:schedule-tag`.
    Entity(ETag),
    /// A `CALDAV:calendar-data` payload.
    CalendarData(CalendarPayload),
    /// A property this crate has no model for, kept as the *character data* inside it.
    ///
    /// `docs/adr/0001`'s rule one layer up: what is not understood is preserved rather than
    /// dropped, because dropping it is how one client destroys another's data.
    ///
    /// These octets are text and are written as text — escaped, so that what a peer sent as
    /// character data leaves this crate as character data. That is not a detail of the
    /// encoder. A peer writing `&lt;D:href&gt;/calendars/ann/private/secret.ics&lt;/D:href&gt;`
    /// inside its own extension property is writing a string; a proxying server that copied
    /// the decoded octets out unescaped would put a `DAV:href` element the peer never sent
    /// into its own multistatus, and RFC 4918 gives that element meaning. Markup a peer really
    /// did send is [`PropValue::Markup`], which is a different field with a different rule.
    Unmodeled(Box<[u8]>),
    /// A property whose value is elements this crate has no model for, kept as a fragment.
    ///
    /// RFC 4918 section 14.18 makes a property's value an arbitrary XML fragment and section
    /// 9.1.3's own example puts a peer's structure — `<R:bigbox><R:BoxType>…</R:BoxType>` —
    /// inside one. Flattening that to its concatenated text is a loss a downstream client
    /// cannot detect, so the elements are kept.
    ///
    /// What is kept is a *re-serialization* and not the octets off the wire: this crate's own
    /// prefixes, each element declaring the namespace it resolved to, every text run escaped
    /// by the same door every other value goes through. That is what makes the fragment
    /// self-contained — a peer's prefix bindings do not survive into this crate's document —
    /// and what makes copying it to the sink safe, since nothing inside it is octets a peer
    /// chose the *syntax* of. A caller building one by hand is choosing markup, and the
    /// encoder refuses a fragment whose tags do not balance, that declares or instructs, or
    /// that carries a reference no reader would resolve.
    Markup(Box<[u8]>),
}

/// A property name and what came back for it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DavProperty {
    /// The name.
    pub name: PropName,
    /// The value.
    pub value: PropValue,
}

/// One status and the properties it applies to, RFC 4918 section 14.22.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropStat {
    /// The status these properties came back under.
    pub status: Status,
    /// The precondition that explains *this group's* refusal, if the server named one.
    ///
    /// RFC 4918 section 14.22's grammar is `propstat (prop, status, error?,
    /// responsedescription?)`, so an error inside a group explains the group it sits in. A
    /// response naming `CALDAV:supported-calendar-data` under its `403` and
    /// `CALDAV:supported-filter` under its `404` has said two different things about two
    /// different properties, and a client asking "why was `calendar-data` refused" has to be
    /// able to read the one that belongs to that group. Hoisting both into
    /// [`DavResponse::error`] merged them into a bag with no record of which explained what,
    /// and left the writing direction unable to put either back where it was.
    pub error: Option<ErrorBody>,
    /// The properties. Private behind a charged push.
    props: Bounded<DavProperty>,
}

impl PropStat {
    /// An empty group under one status.
    #[must_use]
    pub fn new(status: Status, limits: Limits) -> Self {
        Self {
            status,
            error: None,
            props: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
        }
    }

    /// Add one property to the group.
    pub fn push(&mut self, property: DavProperty, meter: &mut Meter) -> Result<(), DavError> {
        self.props.push(property, meter)
    }

    /// The properties in the group.
    #[must_use]
    pub fn props(&self) -> &[DavProperty] {
        self.props.as_slice()
    }
}

/// What a response says about its resource, RFC 4918 section 14.24.
///
/// Two-valued because the specification is: a response carries either one status for the whole
/// resource or per-property statuses, and there is no third shape.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResponseBody {
    /// One status for the resource, which is what a bare `404` in a multiget looks like.
    Status(Status),
    /// Per-property statuses.
    PropStats(Bounded<PropStat>),
}

/// The preconditions an error body names, RFC 4918 section 16.
///
/// A list of element names rather than prose: RFC 4791 section 5.3.2.1 and RFC 6638 section
/// 3.2.1 both define their refusals as empty elements, and
/// `CALDAV:allowed-organizer-scheduling-object-change` is how a server says that a stored
/// copy's `ORGANIZER` moved — the one defense a file-level scheduling gate cannot supply,
/// because it needs the copy that was there before the write.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorBody {
    /// The named conditions.
    conditions: Bounded<PropName>,
}

impl ErrorBody {
    /// An error body naming nothing yet.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            conditions: Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
        }
    }

    /// Name one more condition.
    pub fn push(&mut self, condition: PropName, meter: &mut Meter) -> Result<(), DavError> {
        self.conditions.push(condition, meter)
    }

    /// The named conditions.
    #[must_use]
    pub fn conditions(&self) -> &[PropName] {
        self.conditions.as_slice()
    }
}

/// One resource's answer inside a multistatus.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DavResponse {
    /// Which resource this is about.
    pub href: Href,
    /// What it says about it.
    pub body: ResponseBody,
    /// The precondition or postcondition that explains a refusal, if there is one.
    pub error: Option<ErrorBody>,
}

impl DavResponse {
    /// A response carrying one status for the whole resource.
    #[must_use]
    pub const fn with_status(href: Href, status: Status) -> Self {
        Self {
            href,
            body: ResponseBody::Status(status),
            error: None,
        }
    }

    /// A response that will carry per-property statuses.
    #[must_use]
    pub fn with_propstats(href: Href, limits: Limits) -> Self {
        Self {
            href,
            body: ResponseBody::PropStats(Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            )),
            error: None,
        }
    }

    /// Add one property group, if this response carries groups.
    pub fn push_propstat(&mut self, propstat: PropStat, meter: &mut Meter) -> Result<(), DavError> {
        match &mut self.body {
            ResponseBody::PropStats(groups) => groups.push(propstat, meter),
            ResponseBody::Status(_) => {
                Err(DavError::Unexpected(crate::element::ElementName::Propstat))
            },
        }
    }

    /// The property groups, or nothing when the response carries a single status.
    #[must_use]
    pub fn propstats(&self) -> &[PropStat] {
        match &self.body {
            ResponseBody::PropStats(groups) => groups.as_slice(),
            ResponseBody::Status(_) => &[],
        }
    }

    /// The value a named property came back with, across every successful group.
    ///
    /// The only correct way to ask "did I get the object" once statuses can diverge: a name
    /// found under `403` is a name the server refused, and reading it as though it had arrived
    /// is how a client shows an empty calendar and calls it synchronized.
    #[must_use]
    pub fn successful_value(&self, name: &PropName) -> Option<&PropValue> {
        self.propstats()
            .iter()
            .filter(|group| group.status.is_success())
            .flat_map(PropStat::props)
            .find(|property| &property.name == name)
            .map(|property| &property.value)
    }
}

/// A whole multistatus, held at once.
///
/// One consumer of [`ResponseSource`] and not a second reader. A caller that cannot hold a
/// collection drains the source instead and never builds this, which is the defense that works
/// when the entries may be forgeries — no count distinguishes a real forty-thousand-resource
/// collection from a fabricated one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultiStatus {
    /// The responses. Private behind a charged push.
    responses: Bounded<DavResponse>,
    /// The RFC 6578 token this body ended with, if it carried one.
    pub sync_token: Option<SyncToken>,
}

impl MultiStatus {
    /// An empty multistatus under the caller's bounds.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            responses: Bounded::with_cap(
                bounded_cap(limits.max_responses()),
                LimitExceeded::Responses,
            ),
            sync_token: None,
        }
    }

    /// Add one response.
    pub fn push(&mut self, response: DavResponse, meter: &mut Meter) -> Result<(), DavError> {
        self.responses.push(response, meter)
    }

    /// The responses.
    #[must_use]
    pub fn responses(&self) -> &[DavResponse] {
        self.responses.as_slice()
    }

    /// Drain a source into an owned multistatus.
    ///
    /// The cap is the caller's `Limits::max_responses`, and crossing it is reported as
    /// `DiagnosticCode::DavResponsesTruncated` on the caller's sink rather than as an error,
    /// because a partial collection plus a report of its partiality is more useful than
    /// nothing — and `Severity::LimitReached` is exactly the channel `docs/adr/0009` gives to
    /// work cut short at a bound with what was already read intact.
    /// A token is a statement about the whole answer, so a partial answer states none.
    ///
    /// RFC 6578 section 3.4 makes `DAV:sync-token` the state a *complete* report brings the
    /// client up to. A caller that stored the token of a report this reader cut short would
    /// never be told about the changes it did not receive — no later `sync-collection`
    /// mentions them, because as far as the server is concerned they were delivered. The
    /// guard has to be the fact of truncation and not the position of the element: a server
    /// writing the token before its responses (which this reader accepts) would otherwise
    /// hand back a full token for sixteen of forty thousand changes, while the ordinary
    /// spelling was safe only because the reader never reached the end of the body.
    pub fn read(
        source: &mut dyn ResponseSource,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError> {
        let mut collected = Self::new(context.limits);
        let mut truncated = false;
        while let Some(response) = source.next_response(context)? {
            if collected.responses.is_full() {
                context.report(
                    DiagnosticCode::DavResponsesTruncated,
                    Severity::LimitReached,
                    0,
                );
                truncated = true;
                break;
            }
            collected.push(response, context.meter)?;
        }
        truncated |= source.was_truncated();
        if truncated {
            if source.sync_token().is_some() {
                context.report(
                    DiagnosticCode::DavSyncTokenWithheld,
                    Severity::LimitReached,
                    0,
                );
            }
            return Ok(collected);
        }
        if let Some(token) = source.sync_token() {
            collected.sync_token = Some(SyncToken::new(token, context.limits, context.meter)?);
        }
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{Diagnostic, DiagnosticCode, Limits, Meter};

    use super::{
        CalendarPayload, DavProperty, DavResponse, MultiStatus, PropStat, PropValue, ResponseBody,
    };
    use crate::codec::ResponseSource;
    use crate::element::ElementName;
    use crate::failure::DavError;
    use crate::policy::DecodeContext;
    use crate::request::PropName;
    use crate::text::LineEndings;
    use crate::value::{Href, Status};

    /// A source that yields responses a test built, which is what a recorded exchange is once
    /// the tokenizer has run: `docs/adr/0004`'s "an interoperability case is a value, not a
    /// live connection" with the connection already removed.
    #[derive(Debug)]
    struct Canned {
        remaining: Vec<DavResponse>,
        token: Option<Vec<u8>>,
    }

    impl ResponseSource for Canned {
        fn next_response(
            &mut self,
            _context: &mut DecodeContext<'_>,
        ) -> Result<Option<DavResponse>, DavError> {
            Ok(if self.remaining.is_empty() {
                None
            } else {
                Some(self.remaining.remove(0))
            })
        }

        fn sync_token(&self) -> Option<&[u8]> {
            self.token.as_deref()
        }
    }

    fn payload(octets: &[u8], meter: &mut Meter) -> PropValue {
        PropValue::CalendarData(
            CalendarPayload::from_octets(octets, Limits::DEFAULT, meter).unwrap(),
        )
    }

    #[test]
    fn one_href_reports_divergent_statuses_for_two_of_its_properties() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let href = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter).unwrap();
        let mut response = DavResponse::with_propstats(href, limits);

        let mut readable = PropStat::new(Status::OK, limits);
        readable
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: payload(b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", &mut meter),
                },
                &mut meter,
            )
            .unwrap();
        let mut refused = PropStat::new(Status::FORBIDDEN, limits);
        refused
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Displayname),
                    value: PropValue::Empty,
                },
                &mut meter,
            )
            .unwrap();
        response.push_propstat(readable, &mut meter).unwrap();
        response.push_propstat(refused, &mut meter).unwrap();

        let wanted = PropName::Known(ElementName::CalendarData);
        assert!(response.successful_value(&wanted).is_some());
        // Found under 403, which is not the same as returned.
        let refused_name = PropName::Known(ElementName::Displayname);
        assert!(response.successful_value(&refused_name).is_none());
    }

    #[test]
    fn a_payload_carries_whether_it_is_what_the_server_stored() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let held = CalendarPayload::from_octets(
            b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
            Limits::DEFAULT,
            &mut meter,
        )
        .unwrap();
        assert_eq!(held.line_endings(), LineEndings::Crlf);
        assert!(held.is_as_sent());
    }

    #[test]
    fn the_owned_multistatus_is_one_consumer_of_the_streaming_one() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let first = Href::new(b"/c/1.ics", limits, &mut meter).unwrap();
        let second = Href::new(b"/c/2.ics", limits, &mut meter).unwrap();
        let mut source = Canned {
            remaining: alloc::vec![
                DavResponse::with_propstats(first, limits),
                DavResponse::with_status(second, Status::NOT_FOUND),
            ],
            token: Some(b"http://example.invalid/ns/sync/42".to_vec()),
        };
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let body = MultiStatus::read(&mut source, &mut context).unwrap();
        assert_eq!(body.responses().len(), 2);
        assert!(matches!(
            body.responses().get(1).map(|one| &one.body),
            Some(&ResponseBody::Status(Status::NOT_FOUND))
        ));
        assert_eq!(
            body.sync_token.as_ref().map(super::SyncToken::as_bytes),
            Some(b"http://example.invalid/ns/sync/42".as_slice())
        );
    }

    #[test]
    fn a_collection_past_the_cap_is_truncated_and_reported_rather_than_refused() {
        let limits = Limits::DEFAULT.with_max_responses(1);
        let mut meter = Meter::new(limits);
        let first = Href::new(b"/c/1.ics", limits, &mut meter).unwrap();
        let second = Href::new(b"/c/2.ics", limits, &mut meter).unwrap();
        let mut source = Canned {
            remaining: alloc::vec![
                DavResponse::with_status(first, Status::OK),
                DavResponse::with_status(second, Status::OK),
            ],
            token: None,
        };
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let body = MultiStatus::read(&mut source, &mut context).unwrap();
        assert_eq!(body.responses().len(), 1);
        assert_eq!(
            reported.first().copied().map(Diagnostic::code),
            Some(DiagnosticCode::DavResponsesTruncated)
        );
    }
}
