// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading a multistatus, one [`DavResponse`] at a time.
//!
//! Peak memory is one response. Nothing here builds a collection, and
//! [`MultiStatus::read`](crate::MultiStatus::read) — which does — drives this same public
//! [`ResponseSource`], so the owned form and the streaming form cannot drift apart along a
//! private fast path. A client with sixty-four kilobytes drains the source and keeps what it
//! needs; a client with memory materializes the collection; both read the same octets through
//! the same code.
//!
//! # What this reader will not conclude
//!
//! **A status it cannot read is never a success.** A [`PropStat`] is built holding
//! [`UNREADABLE_STATUS`] and a parsed status replaces it, so a `DAV:status` that is missing or
//! unreadable leaves the group under a code
//! [`Status::is_success`](crate::Status::is_success) answers `false` to. The properties are
//! kept — they arrived, and `docs/adr/0001`'s rule is that what is not understood is preserved
//! — but [`DavResponse::successful_value`] will not hand one back as though the server had
//! returned it. Reading an unstated outcome as `200` is how a client shows an empty calendar
//! and calls it synchronized.
//!
//! **A property whose value has no modeled shape is kept and reported**, in one of the two
//! shapes a property's value can be. `PropValue` has no list variant and no tree, so a property
//! carrying several `href`s or an extension this crate has no row for is kept whole and
//! reported with `DiagnosticCode::DavPropertyUnmodeled`.
//!
//! Which shape depends on what was inside the element, because a property's value is character
//! data or it is elements. Content that is elements with nothing but layout between them is the
//! peer's own structure and becomes [`PropValue::Markup`]: the subtree re-serialized in this
//! crate's prefixes, each element declaring the namespace it resolved to, every text run
//! escaped. Content that is character data becomes [`PropValue::Unmodeled`], the octets
//! concatenated in document order, and it is written back *escaped* — text a peer escaped stays
//! text, or a proxy would promote a peer's string into markup of its own.
//!
//! A property that carries both keeps its character data and reports
//! `DiagnosticCode::DavPropertyMarkupDropped` at `Severity::Violation`. One `Box<[u8]>` cannot
//! say where a peer's markup sat among a peer's text without inventing an order between them,
//! and saying so is better than inventing one. No mainstream server writes that shape.
//!
//! **A response naming no resource is not delivered.** RFC 4918 section 14.24 requires an
//! `href`, [`DavResponse::href`] is not an `Option`, and a response that names nothing cannot
//! be matched to the request that asked for it — which is how one resource's `ETag` gets
//! attributed to another. It is reported as `DiagnosticCode::DavResponseWithoutHref` and the
//! read continues at the next response.
//!
//! **The grouped `href` form is refused rather than guessed at.** RFC 4918 section 14.24's
//! grammar admits `(href*, status)` — several resources under one status — and a reader that
//! holds one response at a time cannot deliver the second without buffering the first
//! response's worth of `href`s before their shared status arrives. Taking the first and
//! dropping the rest would be a silent discard, so a second `href` inside one response is
//! [`DavError::Unexpected`]. No CalDAV `REPORT` emits that form: a `PROPFIND` and a
//! `calendar-multiget` answer with `propstat`s, which the same grammar allows exactly one
//! `href` in front of.

use alloc::boxed::Box;
use alloc::vec::Vec;

use ical_core::{DiagnosticCode, LimitExceeded, Limits, Meter, Severity};

use crate::codec::{ResponseSource, XmlEvent, XmlPull};
use crate::element::{ElementName, Namespace, QName};
use crate::failure::DavError;
use crate::policy::{DecodeContext, UnknownPolicy};
use crate::reader::XML_URI;
use crate::request::PropName;
use crate::response::{CalendarPayload, DavProperty, DavResponse, ErrorBody, PropStat, PropValue};
use crate::text::{DecodedText, LineEndings, TextRun, write_escaped_attribute, write_escaped_text};
use crate::value::{ETag, ExtensionName, Href, ResourceType, Status, copy};

/// The status a group of properties carries when the server stated none this reader can read.
///
/// Any non-success code would do and `200` is the one answer that would be a lie, so the choice
/// is between wrong claims. `500` is the least wrong: an unreadable status line is a fault in
/// the server's own answer rather than a refusal it expressed, which is what `403` would say
/// and what `404` would say. What matters to a caller is that
/// [`DavResponse::successful_value`] does not hand back a property whose outcome nobody stated.
pub const UNREADABLE_STATUS: Status = match Status::new(500) {
    Ok(known) => known,
    // Unreachable: 500 is inside the range `Status::new` admits. Written as a match rather
    // than an unwrap because this crate's profile forbids one and a panic is not a fallback.
    Err(_) => Status::INSUFFICIENT_STORAGE,
};

/// A multistatus body, delivered one response at a time.
///
/// Built over a tokenizer rather than over octets: the body is the tokenizer's to hold, this
/// reader's subject is the `DAV:` grammar above it, and separating the two is what lets one
/// grammar be driven by a tokenizer over a contiguous body and by one that is not.
pub struct MultiStatusReader<'body, 'pull> {
    /// The tokenizer this reader pulls from.
    events: &'pull mut dyn XmlPull<'body>,
    /// How far through the document this reader has got.
    stage: Stage,
    /// The RFC 6578 token the body ended with, once it has been reached.
    token: Option<Box<[u8]>>,
    /// Whether a bound ended the stream before the body did.
    truncated: bool,
}

/// How far through the document a reader has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The `DAV:multistatus` element has not opened yet.
    Unopened,
    /// Inside the multistatus, between responses.
    Open,
    /// The multistatus closed, the body ended, or a bound ended the stream.
    Finished,
}

impl<'body, 'pull> MultiStatusReader<'body, 'pull> {
    /// A reader over the events of one multistatus body.
    ///
    /// The caller's bounds arrive with each call inside its `DecodeContext` rather than here: a
    /// source that captured a policy at construction would read its second body under the first
    /// body's ledger, which is the aggregate mistake `docs/adr/0010` exists to prevent.
    #[must_use]
    pub fn new(events: &'pull mut dyn XmlPull<'body>) -> Self {
        Self {
            events,
            stage: Stage::Unopened,
            token: None,
            truncated: false,
        }
    }

    /// Walk the document until a response is complete or the multistatus ends.
    fn walk(&mut self, context: &mut DecodeContext<'_>) -> Result<Option<DavResponse>, DavError> {
        loop {
            if self.stage == Stage::Finished {
                return Ok(None);
            }
            let Some(event) = self.events.next_event(context)? else {
                self.stage = Stage::Finished;
                return Ok(None);
            };
            match event {
                XmlEvent::Start { name, known, depth } => {
                    let opened = Opened { name, known, depth };
                    if let Some(found) = self.opened(opened, context)? {
                        return Ok(Some(found));
                    }
                },
                XmlEvent::End {
                    known: Some(ElementName::Multistatus),
                    ..
                } => {
                    self.stage = Stage::Finished;
                    return Ok(None);
                },
                XmlEvent::End { .. } | XmlEvent::Text(_) => {},
            }
        }
    }

    /// Act on an element that opened at the top level of the document.
    fn opened(
        &mut self,
        child: Opened<'body>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<DavResponse>, DavError> {
        match (self.stage, child.known) {
            (Stage::Unopened, Some(ElementName::Multistatus)) => {
                self.stage = Stage::Open;
                Ok(None)
            },
            // A document whose root is something else is not a multistatus, whatever else it
            // may be. Naming the element that arrived is the useful half of the refusal, so a
            // root with a row is `Unexpected` and a foreign one — which has no row, that being
            // what makes it foreign — is `Foreign`.
            (Stage::Unopened, Some(found)) => Err(DavError::Unexpected(found)),
            (Stage::Unopened, None) => Err(DavError::Foreign),
            (Stage::Open, Some(ElementName::Response)) => self.response(child.depth, context),
            (Stage::Open, Some(ElementName::SyncToken)) => {
                self.sync_token_of(child.depth, context)?;
                Ok(None)
            },
            (Stage::Open, Some(_)) => {
                self.events.skip_element(context)?;
                Ok(None)
            },
            (Stage::Open, None) => {
                self.foreign(context)?;
                Ok(None)
            },
            (Stage::Finished, _) => Ok(None),
        }
    }

    /// Read one `DAV:response` whole, or answer `None` for one that names no resource.
    fn response(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<DavResponse>, DavError> {
        if !self.charge(context)? {
            return Ok(None);
        }
        let mut parts = ResponseParts::new();
        while let Some(child) = self.next_child(depth, context)? {
            self.in_response(&mut parts, child, context)?;
        }
        let at = self.events.offset();
        Ok(parts.finish(at, context))
    }

    /// Charge one response against the shared ledger, and say whether to read it.
    ///
    /// Crossing `Limits::max_responses` ends the stream rather than refusing the body:
    /// `MultiStatus::read` truncates and reports at its own cap, and a source that raised an
    /// error at the same number would turn that documented behavior into a failure the moment
    /// the two caps were reached in the other order. `Severity::LimitReached` is
    /// `docs/adr/0009`'s channel for work cut short with what was already read intact.
    fn charge(&mut self, context: &mut DecodeContext<'_>) -> Result<bool, DavError> {
        match context.meter.try_charge_response() {
            Ok(()) => Ok(true),
            Err(LimitExceeded::Responses) => {
                let at = self.events.offset();
                context.report(
                    DiagnosticCode::DavResponsesTruncated,
                    Severity::LimitReached,
                    at,
                );
                self.stage = Stage::Finished;
                self.truncated = true;
                Ok(false)
            },
            Err(other) => Err(DavError::Limit(other)),
        }
    }

    /// Act on one child of a `DAV:response`.
    fn in_response(
        &mut self,
        parts: &mut ResponseParts,
        child: Opened<'body>,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        match child.known {
            Some(ElementName::Href) => {
                let named = self.href_of(child.depth, context)?;
                parts.absorb_href(named)
            },
            Some(ElementName::Propstat) => self.propstat(parts, child.depth, context),
            Some(ElementName::Status) => {
                parts.status = Some(self.status_of(child.depth, context)?);
                Ok(())
            },
            Some(ElementName::Error) => {
                let named = self.error_body(child.depth, context)?;
                parts.absorb_error(named, context.meter)
            },
            // `responsedescription` is prose for a human and `location` is a redirect target;
            // `DavResponse` carries neither, and a known element this crate models nowhere is
            // not a foreign one, so it is passed over without a diagnostic.
            Some(_) => self.events.skip_element(context),
            None => self.foreign(context),
        }
    }

    /// Read one `DAV:propstat` into the response being built.
    fn propstat(
        &mut self,
        parts: &mut ResponseParts,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        let mut group = PropStat::new(UNREADABLE_STATUS, context.limits);
        let mut stated = false;
        while let Some(child) = self.next_child(depth, context)? {
            match child.known {
                Some(ElementName::Prop) => self.prop_bag(&mut group, child.depth, context)?,
                Some(ElementName::Status) => {
                    group.status = self.status_of(child.depth, context)?;
                    stated = true;
                },
                // RFC 4918 section 14.22's grammar is `propstat (prop, status, error?,
                // responsedescription?)`, so an error inside a group explains *that group*.
                // It stays on the group: two propstats naming two different preconditions
                // have said two different things, and merging them into the response's own
                // bag left a client unable to read the condition that belongs to the
                // property it asked about — and left the writing direction with nowhere to
                // put it back.
                Some(ElementName::Error) => {
                    let named = self.error_body(child.depth, context)?;
                    absorb_into(&mut group.error, named, context.meter)?;
                },
                Some(_) => self.events.skip_element(context)?,
                None => self.foreign(context)?,
            }
        }
        if !stated {
            let at = self.events.offset();
            context.report(DiagnosticCode::DavStatusUnreadable, Severity::Violation, at);
        }
        parts.absorb_group(group, context)
    }

    /// Read the properties of one `DAV:prop` into a group.
    fn prop_bag(
        &mut self,
        group: &mut PropStat,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        while let Some(child) = self.next_child(depth, context)? {
            let property = self.property(child, context)?;
            group.push(property, context.meter)?;
        }
        Ok(())
    }

    /// Read one property: its name, and the value its element earns.
    ///
    /// A property outside the vocabulary is kept under [`PropName::Extension`] rather than
    /// subjected to [`UnknownPolicy`]. Inside `DAV:prop` the vocabulary is open by design —
    /// RFC 4918 section 17 — so a name with no row is data rather than an element this reader
    /// failed to understand, and nothing about it is skipped.
    fn property(
        &mut self,
        child: Opened<'body>,
        context: &mut DecodeContext<'_>,
    ) -> Result<DavProperty, DavError> {
        let name = name_of(child, context.meter)?;
        let value = self.property_value(child, context)?;
        Ok(DavProperty { name, value })
    }

    /// Walk one property element and decide what its content amounted to.
    fn property_value(
        &mut self,
        child: Opened<'body>,
        context: &mut DecodeContext<'_>,
    ) -> Result<PropValue, DavError> {
        let mut parts = ValueParts::new(Shape::of(child.known), context.limits);
        while let Some(event) = self.events.next_event(context)? {
            match event {
                XmlEvent::End { depth: at, .. } if at <= child.depth => break,
                // A run belongs to the property itself only when nothing is open inside it.
                // Text under a child element is that child's content, not character data
                // beside it, so it does not make the property mixed.
                XmlEvent::Text(run) => {
                    let own = self.events.depth() <= child.depth;
                    parts.absorb(run, own, context)?;
                },
                XmlEvent::Start { name, known, depth } => {
                    let inner = Opened { name, known, depth };
                    // The start tag is recorded before the child is acted on, because acting
                    // on it may consume the whole subtree and the fragment has to stay
                    // balanced whichever branch was taken.
                    if parts.markup {
                        open_in_fragment(&*self.events, &mut parts, name, context)?;
                    }
                    self.in_property(&mut parts, inner, context)?;
                },
                XmlEvent::End { name, .. } => {
                    if parts.markup {
                        close_in_fragment(&mut parts, name, context)?;
                    }
                },
            }
        }
        let at = self.events.offset();
        parts.finish(at, context)
    }

    /// Act on one child element of a property.
    fn in_property(
        &mut self,
        parts: &mut ValueParts<'body>,
        child: Opened<'body>,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        parts.child_count = parts.child_count.saturating_add(1);
        if let Some(holder) = parts.resource.as_mut() {
            claim(holder, child, context.meter)?;
            // A `resourcetype` answers with its claims and never with a fragment, so the one
            // being built cannot be its value and is dropped rather than left half-written by
            // the subtree this skips.
            parts.drop_fragment();
            return self.events.skip_element(context);
        }
        if parts.shape == Shape::Reference && child.known == Some(ElementName::Href) {
            parts.href_count = parts.href_count.saturating_add(1);
            let found = self.href_of(child.depth, context)?;
            // `href_of` consumed the element's own end tag, so the fragment is closed here
            // with the value that came out of it — a reference-shaped property whose children
            // turn out not to be the one `href` this shape admits still has to be keepable.
            if parts.markup {
                let octets = found.as_ref().map(Href::as_bytes).unwrap_or_default();
                parts.push_escaped_text(octets, context)?;
                close_in_fragment(parts, child.name, context)?;
            }
            if parts.first_href.is_none() {
                parts.first_href = found;
            }
            return Ok(());
        }
        // Deliberately not skipped: the loop that called this walks into the element instead,
        // so the character data inside a property with no modeled shape is collected rather
        // than passed over. `Unmodeled` promises the octets, and a subtree whose text was
        // skipped on the way past would leave it with none to promise.
        Ok(())
    }

    /// Read the preconditions a `DAV:error` names.
    fn error_body(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<ErrorBody, DavError> {
        let mut named = ErrorBody::new(context.limits);
        while let Some(child) = self.next_child(depth, context)? {
            named.push(name_of(child, context.meter)?, context.meter)?;
            self.events.skip_element(context)?;
        }
        Ok(named)
    }

    /// Read a `DAV:href`'s content as one.
    fn href_of(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<Href>, DavError> {
        let Some(text) = self.text_of(depth, context)? else {
            return Ok(None);
        };
        // A URI carries no whitespace, so what a pretty-printer put around one is layout.
        let trimmed = trim_xml(text.run.as_bytes());
        if trimmed.is_empty() {
            return Ok(None);
        }
        Href::new(trimmed, context.limits, context.meter).map(Some)
    }

    /// Read a `DAV:status`, reporting one nothing can read rather than guessing at it.
    fn status_of(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<Status, DavError> {
        let text = self.text_of(depth, context)?;
        let read = text
            .as_ref()
            .and_then(|held| Status::parse_status_line(trim_xml(held.run.as_bytes())).ok());
        let Some(found) = read else {
            let at = self.events.offset();
            context.report(DiagnosticCode::DavStatusUnreadable, Severity::Violation, at);
            return Ok(UNREADABLE_STATUS);
        };
        Ok(found)
    }

    /// Read a `DAV:sync-token` and keep its octets without looking inside them.
    ///
    /// Bounded by `Limits::max_href_bytes` for the reason `SyncToken::new` is: a token is a URI
    /// in every implementation that writes one, and no separate dimension is worth inventing.
    /// Charged here rather than only where an owned `SyncToken` is built, because a caller that
    /// drains the stream and never builds a collection would otherwise hold uncharged octets.
    fn sync_token_of(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        let Some(text) = self.text_of(depth, context)? else {
            return Ok(());
        };
        let trimmed = trim_xml(text.run.as_bytes());
        let length = u32::try_from(trimmed.len()).map_err(|_| LimitExceeded::Href)?;
        if length > context.limits.max_href_bytes() {
            return Err(DavError::Limit(LimitExceeded::Href));
        }
        context.meter.try_charge_bytes(u64::from(length))?;
        self.token = Some(copy(trimmed)?);
        Ok(())
    }

    /// Read one element's character data whole, skipping any markup inside it.
    ///
    /// Every run, not the first one. An XML comment carries no event and therefore splits a
    /// text node in two, so `<D:href>/c/1.ics<!-- --># /2.ics</D:href>` reaches this loop as
    /// two runs of one value. Keeping the first and discarding the rest with nothing reported
    /// made an `href` and a `DAV:sync-token` into values the peer never sent — and a token is
    /// the one value a client hands straight back to the server, so an earlier-looking one is
    /// a resynchronization that silently covers changes it never received.
    fn text_of(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<DecodedText<'body>>, DavError> {
        let mut held: Option<DecodedText<'body>> = None;
        let mut joined: Vec<u8> = Vec::new();
        while let Some(event) = self.events.next_event(context)? {
            match event {
                XmlEvent::End { depth: at, .. } if at <= depth => break,
                XmlEvent::Text(run) => {
                    let Some(first) = held.as_ref() else {
                        held = Some(run);
                        continue;
                    };
                    if joined.is_empty() {
                        append(&mut joined, first.run.as_bytes(), context)?;
                    }
                    append(&mut joined, run.run.as_bytes(), context)?;
                },
                XmlEvent::Start { .. } => self.events.skip_element(context)?,
                XmlEvent::End { .. } => {},
            }
        }
        if joined.is_empty() {
            return Ok(held);
        }
        // The witness belongs to the octets it travels with, so a value assembled out of
        // several runs is classified over the assembly rather than over the first of them.
        let line_endings = LineEndings::of(&joined);
        Ok(Some(DecodedText {
            run: TextRun::Reassembled(joined.into_boxed_slice()),
            line_endings,
        }))
    }

    /// The next child of the element that opened at `depth`, or `None` at its end.
    ///
    /// Every caller must consume the child it is handed — by reading it to its end or by
    /// skipping it — because this reader's position is the tokenizer's, and a child left open
    /// would make the next call descend into it.
    fn next_child(
        &mut self,
        depth: u16,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<Opened<'body>>, DavError> {
        loop {
            let Some(event) = self.events.next_event(context)? else {
                return Ok(None);
            };
            match event {
                XmlEvent::Start {
                    name,
                    known,
                    depth: at,
                } => {
                    return Ok(Some(Opened {
                        name,
                        known,
                        depth: at,
                    }));
                },
                XmlEvent::End { depth: at, .. } if at <= depth => return Ok(None),
                XmlEvent::End { .. } | XmlEvent::Text(_) => {},
            }
        }
    }

    /// Skip or refuse an element outside the vocabulary, as the caller's policy says.
    ///
    /// The diagnostic is reported either way: a caller that refuses the body still wants the
    /// offset of what it refused, and the refusal itself carries no name because the element
    /// has no row to name it by.
    fn foreign(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
        let at = self.events.offset();
        context.report(DiagnosticCode::DavForeignElementSkipped, Severity::Note, at);
        match context.unknown {
            UnknownPolicy::Skip => self.events.skip_element(context),
            UnknownPolicy::Reject => Err(DavError::Foreign),
        }
    }
}

impl ResponseSource for MultiStatusReader<'_, '_> {
    fn next_response(
        &mut self,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<DavResponse>, DavError> {
        self.walk(context)
    }

    fn sync_token(&self) -> Option<&[u8]> {
        self.token.as_deref()
    }

    fn was_truncated(&self) -> bool {
        self.truncated
    }
}

impl core::fmt::Debug for MultiStatusReader<'_, '_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The tokenizer is a trait object with no `Debug` bound, and adding one would forbid a
        // caller's own from being anything it likes. The token is shown as read or not rather
        // than as octets, because it is opaque and printing it invites reading it.
        formatter
            .debug_struct("MultiStatusReader")
            .field("stage", &self.stage)
            .field("sync_token_read", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

/// One element that has just opened.
#[derive(Clone, Copy, Debug)]
struct Opened<'body> {
    /// Its resolved name, which is a namespace and a local name and never a prefix.
    name: QName<'body>,
    /// The row of the closed vocabulary it lands on, if any.
    known: Option<ElementName>,
    /// How deep it sits.
    depth: u16,
}

/// What a `DAV:response` has yielded so far.
#[derive(Debug)]
struct ResponseParts {
    /// The resource this response is about, until a propstat takes it.
    href: Option<Href>,
    /// The one status a resource-wide answer carries.
    status: Option<Status>,
    /// The response being built, once a propstat has claimed the `href`.
    built: Option<DavResponse>,
    /// The preconditions any `DAV:error` in the response named.
    error: Option<ErrorBody>,
}

impl ResponseParts {
    /// A response that has yielded nothing yet.
    const fn new() -> Self {
        Self {
            href: None,
            status: None,
            built: None,
            error: None,
        }
    }

    /// Record the resource this response is about.
    fn absorb_href(&mut self, found: Option<Href>) -> Result<(), DavError> {
        if found.is_none() {
            return Ok(());
        }
        if self.href.is_some() || self.built.is_some() {
            return Err(DavError::Unexpected(ElementName::Href));
        }
        self.href = found;
        Ok(())
    }

    /// Add one property group, building the response around the `href` if it is the first.
    fn absorb_group(
        &mut self,
        group: PropStat,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        if self.built.is_none() {
            // No `href` means this response names no resource and will be dropped whole, so
            // the group goes with it and `finish` makes the one report about it.
            let Some(named) = self.href.take() else {
                return Ok(());
            };
            self.built = Some(DavResponse::with_propstats(named, context.limits));
        }
        match self.built.as_mut() {
            Some(holding) => holding.push_propstat(group, context.meter),
            None => Ok(()),
        }
    }

    /// Add the conditions a response-level error body named to the ones already collected.
    fn absorb_error(&mut self, named: ErrorBody, meter: &mut Meter) -> Result<(), DavError> {
        absorb_into(&mut self.error, named, meter)
    }

    /// The response these parts amount to, or `None` for one that names no resource.
    fn finish(self, at: u64, context: &mut DecodeContext<'_>) -> Option<DavResponse> {
        // Property groups outrank a resource-wide status when a response carries both, which
        // RFC 4918 section 14.24's grammar forbids anyway: the groups are the more specific
        // answer, and reading the status instead would flatten statuses that had diverged.
        if let Some(mut holding) = self.built {
            holding.error = self.error;
            return Some(holding);
        }
        let Some(named) = self.href else {
            context.report(
                DiagnosticCode::DavResponseWithoutHref,
                Severity::Violation,
                at,
            );
            return None;
        };
        // RFC 4918 section 14.24 requires a status or a propstat and neither arrived, so a
        // response reaching here states no outcome for the resource it names.
        let outcome = self.status.unwrap_or_else(|| {
            context.report(DiagnosticCode::DavStatusUnreadable, Severity::Violation, at);
            UNREADABLE_STATUS
        });
        let mut answer = DavResponse::with_status(named, outcome);
        answer.error = self.error;
        Some(answer)
    }
}

/// The shape of value a property's name earns.
///
/// Derived from the element rather than guessed from the content, so that a `getetag` holding
/// something that is not a quoted string is a property this crate could not model and not a
/// property that turned out to be text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// `CALDAV:calendar-data`, whose octets travel with the witness the decode produced.
    Payload,
    /// An entity tag: `DAV:getetag`, `CALDAV:schedule-tag`.
    Entity,
    /// `DAV:resourcetype`, whose children are the value.
    Resource,
    /// A property whose value is one `DAV:href`.
    Reference,
    /// A property of the closed vocabulary whose value is its character data.
    Plain,
    /// A property outside the vocabulary, whose value this crate never interpreted.
    ///
    /// Apart from [`Shape::Plain`] because the two are different claims and one direction has
    /// to be able to state the one the other reads. `PropValue::Text` says "this crate read
    /// this property's value as text"; `PropValue::Unmodeled` says "this crate has no model
    /// for this property and kept what was inside it". A reader that answered `Text` for an
    /// extension property made the second unstatable, so a server proxying one wrote a value
    /// its own client could not read back as the value it wrote.
    Foreign,
}

impl Shape {
    /// The shape an element's name earns.
    ///
    /// `CALDAV:calendar-user-address-set` is deliberately absent: RFC 6638 section 2.4.1 makes
    /// it a *set* of addresses, `PropValue::Reference` holds one, and a shape that took the
    /// first would drop the `mailto:` beside the `urn:uuid:` without saying so. It reaches
    /// `Unmodeled` with a diagnostic instead, which is a report rather than a silent loss.
    const fn of(known: Option<ElementName>) -> Self {
        match known {
            Some(ElementName::CalendarData) => Self::Payload,
            Some(ElementName::Getetag | ElementName::ScheduleTag) => Self::Entity,
            Some(ElementName::Resourcetype) => Self::Resource,
            Some(
                ElementName::CurrentUserPrincipal
                | ElementName::PrincipalUrl
                | ElementName::Owner
                | ElementName::CalendarHomeSet
                | ElementName::ScheduleInboxUrl
                | ElementName::ScheduleOutboxUrl,
            ) => Self::Reference,
            Some(_) => Self::Plain,
            None => Self::Foreign,
        }
    }
}

/// What one property element's content has yielded so far.
#[derive(Debug)]
struct ValueParts<'body> {
    /// The shape this property's name earns.
    shape: Shape,
    /// The first run of character data that was not layout.
    text: Option<DecodedText<'body>>,
    /// Every run of character data, once a second one has proved there is more than one.
    joined: Vec<u8>,
    /// The first `DAV:href` child, for a reference-shaped property.
    first_href: Option<Href>,
    /// A `DAV:resourcetype`'s claims, built only for that shape.
    resource: Option<ResourceType>,
    /// The peer's own elements, re-serialized in this crate's spelling.
    fragment: Vec<u8>,
    /// Whether that fragment is still a value this property could have.
    ///
    /// It starts true and is put out for one reason: a run of character data that is more
    /// than layout. A property's value is text or it is elements, and one that carries both
    /// keeps its text — so the fragment stops being built at the first non-blank run rather
    /// than being grown to no purpose behind every `calendar-data` payload in a body.
    markup: bool,
    /// Elements seen inside this property, at any depth.
    child_count: u32,
    /// `DAV:href` children seen.
    href_count: u32,
}

impl<'body> ValueParts<'body> {
    /// An empty accumulator for a property of this shape.
    fn new(shape: Shape, limits: Limits) -> Self {
        Self {
            shape,
            text: None,
            joined: Vec::new(),
            first_href: None,
            resource: match shape {
                Shape::Resource => Some(ResourceType::new(limits)),
                _ => None,
            },
            fragment: Vec::new(),
            markup: true,
            child_count: 0,
            href_count: 0,
        }
    }

    /// Stop building a fragment, and release what has been built.
    fn drop_fragment(&mut self) {
        self.markup = false;
        self.fragment = Vec::new();
    }

    /// Append literal octets to the fragment, bounded and charged like any other retention.
    fn push_fragment(
        &mut self,
        bytes: &[u8],
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        append(&mut self.fragment, bytes, context)
    }

    /// Append character data to the fragment, escaped as this crate escapes every other run.
    ///
    /// The escaping is what keeps the security property: a fragment holds markup a peer really
    /// sent, and text a peer sent stays text inside it rather than becoming a second element.
    fn push_escaped_text(
        &mut self,
        bytes: &[u8],
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        let mut escaped: Vec<u8> = Vec::new();
        write_escaped_text(&mut escaped, bytes)?;
        self.push_fragment(&escaped, context)
    }

    /// The character data collected so far.
    fn bytes(&self) -> &[u8] {
        if self.joined.is_empty() {
            match &self.text {
                Some(held) => held.run.as_bytes(),
                None => &[],
            }
        } else {
            &self.joined
        }
    }

    /// Record one run of character data.
    ///
    /// Every run, whatever is in it. A blank run used to be dropped as layout, on the
    /// reasoning that the newline a pretty-printer puts inside `<D:resourcetype>` is not a
    /// value — which is true of `resourcetype` and of nothing else. Applied unconditionally it
    /// deleted the `CRLF` that terminates a content line whenever two comments left it alone
    /// in a run inside a `calendar-data`, welding two iCalendar properties into one and
    /// changing the object's `UID`; and it read a `DAV:displayname` whose value is a space as
    /// a property that arrived empty, which is a different fact. The shapes that really do
    /// have layout around them — `resourcetype`, a reference-shaped property — answer out of
    /// their children and never out of this text at all.
    fn absorb(
        &mut self,
        decoded: DecodedText<'body>,
        own: bool,
        context: &mut DecodeContext<'_>,
    ) -> Result<(), DavError> {
        if own && !is_blank(decoded.run.as_bytes()) {
            self.drop_fragment();
        } else if self.markup {
            self.push_escaped_text(decoded.run.as_bytes(), context)?;
        }
        let Some(held) = self.text.take() else {
            self.text = Some(decoded);
            return Ok(());
        };
        if self.joined.is_empty() {
            append(&mut self.joined, held.run.as_bytes(), context)?;
        }
        append(&mut self.joined, decoded.run.as_bytes(), context)?;
        self.text = Some(held);
        Ok(())
    }

    /// The value this content amounts to.
    fn finish(mut self, at: u64, context: &mut DecodeContext<'_>) -> Result<PropValue, DavError> {
        // A `resourcetype` answers with its claims even when it carries none: an empty one is
        // a plain resource, which is a different fact from a property that arrived empty.
        if let Some(claimed) = self.resource.take() {
            return Ok(PropValue::Resource(claimed));
        }
        if self.child_count == 0 && self.bytes().is_empty() {
            return Ok(PropValue::Empty);
        }
        match self.shape {
            Shape::Payload => self.payload(at, context),
            Shape::Entity => self.entity(at, context),
            Shape::Reference => self.reference(at, context),
            Shape::Resource | Shape::Plain => self.plain(at, context),
            Shape::Foreign => self.unmodeled(at, context),
        }
    }

    /// A `calendar-data` payload, with the line-ending witness the decode produced.
    fn payload(mut self, at: u64, context: &mut DecodeContext<'_>) -> Result<PropValue, DavError> {
        // The witness exists only on the run the decode produced, so a payload broken across
        // runs by markup is one this reader cannot hand on as the octets the server stored.
        if self.child_count == 0 && self.joined.is_empty() {
            if let Some(held) = self.text.take() {
                let carried = CalendarPayload::from_text(&held, context.limits, context.meter)?;
                return Ok(PropValue::CalendarData(carried));
            }
        }
        self.unmodeled(at, context)
    }

    /// An entity tag, if the content is the quoted string RFC 9110 section 8.8.3 requires.
    fn entity(self, at: u64, context: &mut DecodeContext<'_>) -> Result<PropValue, DavError> {
        if self.child_count == 0 && self.joined.is_empty() {
            let parsed = self
                .text
                .as_ref()
                .and_then(|held| ETag::parse(trim_xml(held.run.as_bytes())).ok());
            if let Some(tag) = parsed {
                return Ok(PropValue::Entity(tag));
            }
        }
        self.unmodeled(at, context)
    }

    /// The one `href` a reference-shaped property carries.
    fn reference(
        mut self,
        at: u64,
        context: &mut DecodeContext<'_>,
    ) -> Result<PropValue, DavError> {
        if self.child_count == 1 && self.href_count == 1 {
            if let Some(target) = self.first_href.take() {
                return Ok(PropValue::Reference(target));
            }
        }
        self.unmodeled(at, context)
    }

    /// Character data, when that is all the element held.
    fn plain(self, at: u64, context: &mut DecodeContext<'_>) -> Result<PropValue, DavError> {
        if self.child_count == 0 {
            let kept = retain(self.bytes(), context)?;
            return Ok(PropValue::Text(kept));
        }
        self.unmodeled(at, context)
    }

    /// Keep what the property held and say that nothing interpreted it.
    ///
    /// Two answers, because a property's value is two different kinds of thing. Elements with
    /// nothing but layout between them are the peer's own structure and are kept as a
    /// fragment — RFC 4918 section 9.1.3's own example is `<R:bigbox><R:BoxType>…`, and
    /// flattening that to `Box type A` is a loss the next reader cannot detect. Character data
    /// is kept as character data and written back escaped, so that a string a peer wrote
    /// cannot become an element this crate did not receive.
    ///
    /// A property carrying both keeps the text and reports the elements dropped. One
    /// `Box<[u8]>` cannot say where a peer's markup sat among a peer's text without inventing
    /// an order between them, and inventing one is worse than saying so.
    fn unmodeled(self, at: u64, context: &mut DecodeContext<'_>) -> Result<PropValue, DavError> {
        context.report(DiagnosticCode::DavPropertyUnmodeled, Severity::Note, at);
        if self.child_count > 0 && self.markup {
            return Ok(PropValue::Markup(self.fragment.into_boxed_slice()));
        }
        if self.child_count > 0 {
            context.report(
                DiagnosticCode::DavPropertyMarkupDropped,
                Severity::Violation,
                at,
            );
        }
        let kept = retain(self.bytes(), context)?;
        Ok(PropValue::Unmodeled(kept))
    }
}

/// The prefix a kept fragment writes one namespace under.
///
/// This crate's own output prefixes for the three namespaces it has a table for, and one
/// spelling for everything else. A prefix is the document's choice and never the vocabulary's,
/// so re-spelling a peer's is not a loss — what would be a loss is writing the peer's prefix
/// into a document that never bound it.
fn fragment_prefix(namespace: Namespace<'_>) -> &'static [u8] {
    match namespace {
        Namespace::Dav => b"D",
        Namespace::CalDav => b"C",
        Namespace::CalendarServer => b"CS",
        // XML Namespaces 1.0 section 3 binds this URI to `xml` and to nothing else, so a
        // generated prefix for it would be a declaration the specification forbids.
        Namespace::Other(uri) if uri == XML_URI => b"xml",
        Namespace::Other([]) => b"",
        Namespace::Other(_) => b"X",
    }
}

/// Write one element's start tag into the fragment a property is keeping.
///
/// Every element declares the namespace it resolved to, so the fragment is self-contained: the
/// peer's own bindings were scoped to the peer's document and do not travel with the octets.
fn open_in_fragment(
    events: &dyn XmlPull<'_>,
    parts: &mut ValueParts<'_>,
    name: QName<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    parts.push_fragment(b"<", context)?;
    push_qualified_name(parts, name, context)?;
    push_declaration(
        parts,
        fragment_prefix(name.namespace),
        name.namespace,
        context,
    )?;
    for index in 0..events.attribute_count() {
        let Some((held, value)) = events.attribute_at(index) else {
            continue;
        };
        push_fragment_attribute(parts, index, held, value, context)?;
    }
    parts.push_fragment(b">", context)
}

/// Write one element's end tag into the fragment a property is keeping.
fn close_in_fragment(
    parts: &mut ValueParts<'_>,
    name: QName<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    parts.push_fragment(b"</", context)?;
    push_qualified_name(parts, name, context)?;
    parts.push_fragment(b">", context)
}

/// Write a name under the prefix this crate spells its namespace with.
fn push_qualified_name(
    parts: &mut ValueParts<'_>,
    name: QName<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    let prefix = fragment_prefix(name.namespace);
    if !prefix.is_empty() {
        parts.push_fragment(prefix, context)?;
        parts.push_fragment(b":", context)?;
    }
    parts.push_fragment(name.local_name, context)
}

/// Declare one prefix on the element being written, unless the prefix declares itself.
fn push_declaration(
    parts: &mut ValueParts<'_>,
    prefix: &[u8],
    namespace: Namespace<'_>,
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    if prefix == b"xml" {
        return Ok(());
    }
    parts.push_fragment(b" xmlns", context)?;
    if !prefix.is_empty() {
        parts.push_fragment(b":", context)?;
        parts.push_fragment(prefix, context)?;
    }
    parts.push_fragment(b"=\"", context)?;
    let mut escaped: Vec<u8> = Vec::new();
    write_escaped_attribute(&mut escaped, namespace.uri())?;
    parts.push_fragment(&escaped, context)?;
    parts.push_fragment(b"\"", context)
}

/// Write one attribute of a kept element, with whatever prefix its own namespace needs.
///
/// An unprefixed attribute is in no namespace (XML Namespaces 1.0 section 6.2) and is written
/// back unprefixed. One that is in a namespace gets a prefix of its own — `A0`, `A1` — rather
/// than the element's, because the two namespaces need not be the same and a shared prefix
/// would silently move the attribute into the element's.
fn push_fragment_attribute(
    parts: &mut ValueParts<'_>,
    index: usize,
    name: QName<'_>,
    value: &[u8],
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    parts.push_fragment(b" ", context)?;
    if name.namespace.uri().is_empty() {
        parts.push_fragment(name.local_name, context)?;
    } else {
        let mut prefix: Vec<u8> = Vec::new();
        if name.namespace.uri() == XML_URI {
            prefix.extend_from_slice(b"xml");
        } else {
            prefix.push(b'A');
            push_decimal(&mut prefix, index);
        }
        parts.push_fragment(&prefix, context)?;
        parts.push_fragment(b":", context)?;
        parts.push_fragment(name.local_name, context)?;
        push_declaration(parts, &prefix, name.namespace, context)?;
    }
    parts.push_fragment(b"=\"", context)?;
    let mut escaped: Vec<u8> = Vec::new();
    write_escaped_attribute(&mut escaped, value)?;
    parts.push_fragment(&escaped, context)?;
    parts.push_fragment(b"\"", context)
}

/// Append a number's decimal digits, which a generated prefix needs and nothing else does.
fn push_decimal(out: &mut Vec<u8>, value: usize) {
    let mut digits: Vec<u8> = Vec::new();
    let mut left = value;
    loop {
        let digit = u8::try_from(left.checked_rem(10).unwrap_or(0)).unwrap_or(0);
        digits.push(b'0'.saturating_add(digit));
        left = left.checked_div(10).unwrap_or(0);
        if left == 0 {
            break;
        }
    }
    digits.reverse();
    out.extend_from_slice(&digits);
}

/// Record one child of a `DAV:resourcetype`.
///
/// A row this crate has no field for is kept by name rather than dropped, including one that is
/// in the vocabulary: `ResourceType` has three fields and the table has seventy-seven rows, so
/// "known" and "modeled as a field" are different questions.
fn claim(holder: &mut ResourceType, child: Opened<'_>, meter: &mut Meter) -> Result<(), DavError> {
    match child.known {
        Some(ElementName::Collection) => holder.collection = true,
        Some(ElementName::Calendar) => holder.calendar = true,
        Some(ElementName::Principal) => holder.principal = true,
        _ => {
            let named = extension_of(child, meter)?;
            holder.push_other(named, meter)?;
        },
    }
    Ok(())
}

/// The name of an element, whether or not the vocabulary has a row for it.
fn name_of(child: Opened<'_>, meter: &mut Meter) -> Result<PropName, DavError> {
    match child.known {
        Some(known) => Ok(PropName::Known(known)),
        None => Ok(PropName::Extension(extension_of(child, meter)?)),
    }
}

/// The name of an element outside the vocabulary, kept as a namespace and a local name.
fn extension_of(child: Opened<'_>, meter: &mut Meter) -> Result<ExtensionName, DavError> {
    ExtensionName::new(child.name.namespace.uri(), child.name.local_name, meter)
}

/// Merge the conditions one `DAV:error` named into whatever is already held at that position.
///
/// RFC 4918 permits more than one `error` in the same position, and two in one position are
/// two statements about one thing. Two in *different* positions are not, which is why this
/// takes the slot it is merging into rather than always reaching for the response's.
fn absorb_into(
    holder: &mut Option<ErrorBody>,
    named: ErrorBody,
    meter: &mut Meter,
) -> Result<(), DavError> {
    let Some(holding) = holder.as_mut() else {
        *holder = Some(named);
        return Ok(());
    };
    for condition in named.conditions() {
        holding.push(condition.clone(), meter)?;
    }
    Ok(())
}

/// Whether a run of character data is layout rather than a value.
///
/// XML 1.0 section 2.3's `S` production, which is what a pretty-printer puts between elements.
/// A value that is only whitespace is not one a caller can tell apart from the layout around
/// it, so this reader reads it as an absence rather than as a value nobody can act on.
fn is_blank(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
}

/// Drop the layout either side of a value that cannot contain any.
fn trim_xml(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(start, |last| last.saturating_add(1));
    bytes.get(start..end).unwrap_or(&[])
}

/// Append one run to the octets a property is accumulating, bounded and charged.
fn append(
    target: &mut Vec<u8>,
    bytes: &[u8],
    context: &mut DecodeContext<'_>,
) -> Result<(), DavError> {
    let grown = target.len().saturating_add(bytes.len());
    let length = u32::try_from(grown).map_err(|_| LimitExceeded::Text)?;
    if length > context.limits.max_xml_text_bytes() {
        return Err(DavError::Limit(LimitExceeded::Text));
    }
    context
        .meter
        .try_charge_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
    target
        .try_reserve(bytes.len())
        .map_err(|_| LimitExceeded::Text)?;
    target.extend_from_slice(bytes);
    Ok(())
}

/// Copy octets a property will hold, against the per-element ceiling and the shared budget.
///
/// The same two bounds `CalendarPayload` crosses, for the same reason: what is copied out of
/// the body is held a second time, and `Limits::max_response_bytes` bounds only the first.
fn retain(bytes: &[u8], context: &mut DecodeContext<'_>) -> Result<Box<[u8]>, DavError> {
    let length = u32::try_from(bytes.len()).map_err(|_| LimitExceeded::Text)?;
    if length > context.limits.max_xml_text_bytes() {
        return Err(DavError::Limit(LimitExceeded::Text));
    }
    context.meter.try_charge_bytes(u64::from(length))?;
    copy(bytes)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{Diagnostic, DiagnosticCode, Instant, LimitExceeded, Limits, Meter};

    use super::{MultiStatusReader, UNREADABLE_STATUS};
    use crate::codec::{ResponseSource, XmlEvent, XmlPull};
    use crate::element::{ElementName, Namespace, QName};
    use crate::failure::DavError;
    use crate::policy::{DecodeContext, UnknownPolicy};
    use crate::request::{CalendarQuery, CompFilter, PropName, TimeRange};
    use crate::response::{
        DavProperty, DavResponse, MultiStatus, PropStat, PropValue, ResponseBody,
    };
    use crate::text::{LineEndings, TextMode};
    use crate::value::{Href, Status};

    /// The `.ics` the three recorded exchanges are carrying, byte for byte.
    const PAYLOAD: &[u8] = include_bytes!("../tests/fixtures/calendar-data-payload.ics");

    /// `SabreDAV`: `d:`/`cal:` prefixes, a `&quot;`-escaped `ETag`, per-property status, and a
    /// second response that is a bare `404`.
    const SABREDAV: &[u8] = include_bytes!("../tests/fixtures/sabredav-calendar-multiget.xml");

    /// Radicale: `ns0:`/`ns1:`, the prefixes Python's `ElementTree` invents, and no layout.
    const RADICALE: &[u8] = include_bytes!("../tests/fixtures/radicale-calendar-multiget.xml");

    /// Calendar Server: a default `xmlns="DAV:"`, so its `DAV:` elements carry no prefix at all.
    const CALENDAR_SERVER: &[u8] =
        include_bytes!("../tests/fixtures/calendarserver-calendar-multiget.xml");

    /// Every response a body yields, with whatever it reported on the way.
    fn drain(body: &[u8], limits: Limits) -> (Vec<DavResponse>, Vec<Diagnostic>) {
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let read = {
            let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
            collect(body, &mut context).unwrap()
        };
        (read, reported)
    }

    /// Every response a body yields under a context the caller built.
    fn collect(body: &[u8], context: &mut DecodeContext<'_>) -> Result<Vec<DavResponse>, DavError> {
        let mut pull = Pull::new(body);
        let mut reader = MultiStatusReader::new(&mut pull);
        let mut read = Vec::new();
        while let Some(found) = reader.next_response(context)? {
            read.push(found);
        }
        Ok(read)
    }

    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().copied().map(Diagnostic::code).collect()
    }

    fn prop_name(name: ElementName) -> PropName {
        PropName::Known(name)
    }

    /// The payload a response came back with, and the witness that traveled beside it.
    fn payload_of(response: &DavResponse) -> Option<(&[u8], LineEndings)> {
        match response.successful_value(&prop_name(ElementName::CalendarData))? {
            PropValue::CalendarData(carried) => Some((carried.as_bytes(), carried.line_endings())),
            _ => None,
        }
    }

    /// The whole of `docs/adr/0004`'s recorded `SabreDAV` exchange, read as values.
    ///
    /// One `href` reporting `calendar-data` at `200` beside `displayname` at `403`, and a
    /// second `href` at a bare `404` — the two shapes `ResponseBody` exists to keep apart.
    #[test]
    fn a_sabredav_multistatus_reads_as_the_two_responses_it_carries() {
        let (read, reported) = drain(SABREDAV, Limits::DEFAULT);
        assert_eq!(read.len(), 2);
        assert!(reported.is_empty(), "{reported:?}");

        let first = read.first().unwrap();
        assert_eq!(
            first.href.as_bytes(),
            b"/calendars/ann/work/20260105T090000Z-1.ics"
        );
        // Two groups, and the statuses did not collapse into one.
        let statuses: Vec<u16> = first
            .propstats()
            .iter()
            .map(|group| group.status.code())
            .collect();
        assert_eq!(statuses, [200, 403]);
        assert_eq!(payload_of(first), Some((PAYLOAD, LineEndings::Crlf)));
        // Found under 403 is not the same as returned, which is all `successful_value` is for.
        assert!(
            first
                .successful_value(&prop_name(ElementName::Displayname))
                .is_none()
        );
        assert!(matches!(
            first.successful_value(&prop_name(ElementName::Getetag)),
            Some(PropValue::Entity(_))
        ));

        let second = read.get(1).unwrap();
        assert_eq!(second.href.as_bytes(), b"/calendars/ann/work/gone.ics");
        assert!(matches!(
            second.body,
            ResponseBody::Status(Status::NOT_FOUND)
        ));
        assert!(second.propstats().is_empty());
    }

    /// The same exchange in the three spellings the deployed world sends it in.
    ///
    /// `d:`/`cal:`, `ns0:`/`ns1:`, and a default `xmlns="DAV:"` where the `DAV:` elements carry
    /// no prefix at all. A reader keyed on the literal string `D:href` gets all three wrong.
    #[test]
    fn one_payload_arrives_from_three_servers_that_agree_on_no_prefix() {
        let cases = [
            ("SabreDAV, d: and cal:", SABREDAV),
            ("Radicale, ns0: and ns1:", RADICALE),
            (
                "Calendar Server, a default DAV: declaration",
                CALENDAR_SERVER,
            ),
        ];
        for (server, body) in cases {
            let (read, reported) = drain(body, Limits::DEFAULT);
            let first = read.first().unwrap();
            assert_eq!(
                payload_of(first),
                Some((PAYLOAD, LineEndings::Crlf)),
                "{server}"
            );
            let tag = match first.successful_value(&prop_name(ElementName::Getetag)) {
                Some(PropValue::Entity(found)) => found.clone(),
                other => panic!("{server}: {other:?}"),
            };
            // `&quot;` from SabreDAV and a literal quote from Radicale are one `ETag`.
            assert_eq!(tag.as_bytes(), b"5f2b8c1e9a04", "{server}");
            assert!(!tag.is_weak(), "{server}");
            // The only thing any of the three costs is Calendar Server's `&#13;`, which has
            // to be reassembled out of the body and says so. Nothing was skipped, nothing
            // went uninterpreted, and no status went unread.
            assert!(
                codes(&reported)
                    .iter()
                    .all(|code| *code == DiagnosticCode::DavCalendarDataCopied),
                "{server}: {reported:?}"
            );
        }
    }

    /// The payload the reader hands on is the payload the caller could `PUT` back.
    ///
    /// Under the strict policy it is not, and the witness on the value says so rather than
    /// leaving the caller to compare it against octets it no longer holds.
    #[test]
    fn the_witness_reaches_the_caller_through_the_whole_read() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let read = {
            let mut context = DecodeContext::new(limits, &mut meter, &mut reported)
                .with_text(crate::text::TextPolicy::Normalized);
            collect(SABREDAV, &mut context).unwrap()
        };
        let first = read.first().unwrap();
        let (octets, endings) = payload_of(first).unwrap();
        assert_eq!(endings, LineEndings::Folded);
        assert!(!endings.is_as_sent());
        assert!(!octets.contains(&b'\r'));
        assert!(codes(&reported).contains(&DiagnosticCode::DavCalendarDataLineEndingsFolded));
    }

    /// `docs/adr/0004`'s untested claim: skip-unknown keeps a `SabreDAV` response readable.
    ///
    /// The foreign elements are the ones that server actually emits — its own `s:` namespace
    /// around an exception and a version — where it puts them: inside the response and inside a
    /// `propstat`. A foreign element inside `DAV:prop` is a *property* rather than an element
    /// this reader failed to understand, so it is kept by name and never skipped.
    #[test]
    fn a_sabredav_response_stays_readable_when_its_extensions_are_skipped() {
        let (read, reported) = drain(EXTENDED, Limits::DEFAULT);
        let first = read.first().unwrap();
        assert!(matches!(
            first.successful_value(&prop_name(ElementName::Getetag)),
            Some(PropValue::Entity(_))
        ));
        assert!(codes(&reported).contains(&DiagnosticCode::DavForeignElementSkipped));

        // The vendor property is kept under its own namespace and local name.
        let vendor = first
            .propstats()
            .iter()
            .flat_map(PropStat::props)
            .find(|property| matches!(property.name, PropName::Extension(_)))
            .unwrap();
        match &vendor.name {
            PropName::Extension(named) => {
                assert_eq!(named.namespace(), b"http://sabredav.org/ns");
                assert_eq!(named.local_name(), b"sync-token-length");
            },
            PropName::Known(known) => panic!("{known:?}"),
        }
        // `Unmodeled` rather than `Text`, and the difference is a claim rather than a shade of
        // one. `Text` says this crate read the value; `Unmodeled` says it has no model for
        // this property and kept what was inside it. A property outside the vocabulary is
        // always the second, and a reader that answered the first left the writing direction
        // unable to state a value its own reading direction would give back.
        assert_eq!(vendor.value, PropValue::Unmodeled(b"36".to_vec().into()));
        assert!(codes(&reported).contains(&DiagnosticCode::DavPropertyUnmodeled));
    }

    /// The other half of the same policy: a caller that will not tolerate an extension.
    #[test]
    fn the_same_body_is_refused_when_the_caller_asked_for_that() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let refused = {
            let mut context = DecodeContext::new(limits, &mut meter, &mut reported)
                .with_unknown(UnknownPolicy::Reject);
            collect(EXTENDED, &mut context)
        };
        assert_eq!(refused, Err(DavError::Foreign));
    }

    /// A `PROPFIND` on a calendar collection, in the shape Apple's and Google's servers answer
    /// one: `D:`, `CS:` for the vendor `getctag`, a `resourcetype` with two claims, and one
    /// property the resource does not carry reported at `404` beside the ones at `200`.
    #[test]
    fn a_propfind_answer_keeps_the_two_statuses_of_one_resource_apart() {
        let (read, reported) = drain(PROPFIND, Limits::DEFAULT);
        assert_eq!(read.len(), 1);
        let first = read.first().unwrap();
        assert_eq!(first.propstats().len(), 2);

        let claims = match first.successful_value(&prop_name(ElementName::Resourcetype)) {
            Some(PropValue::Resource(found)) => found.clone(),
            other => panic!("{other:?}"),
        };
        assert!(claims.collection);
        assert!(claims.calendar);
        assert!(!claims.principal);

        assert_eq!(
            first.successful_value(&prop_name(ElementName::Displayname)),
            Some(&PropValue::Text(b"Work".to_vec().into()))
        );
        assert!(matches!(
            first.successful_value(&prop_name(ElementName::Getctag)),
            Some(PropValue::Text(_))
        ));
        // Asked for, answered at 404, and therefore not something the caller received.
        assert!(
            first
                .successful_value(&prop_name(ElementName::CalendarDescription))
                .is_none()
        );
        let refused = first
            .propstats()
            .iter()
            .find(|group| !group.status.is_success())
            .unwrap();
        assert_eq!(refused.status.code(), 404);
        assert_eq!(refused.props().len(), 1);
        assert!(reported.is_empty(), "{reported:?}");
    }

    /// One value, built by a server and read by a client, asserted to be the same value.
    ///
    /// The direction shows up only in which door is called: the server calls the constructors,
    /// the client reads the octets, and what they hold afterwards compares equal. A field that
    /// meant something in one direction and not in the other would not survive this.
    #[test]
    fn a_server_builds_and_a_client_reads_one_multistatus_to_the_same_value() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);

        let href = Href::new(b"/calendars/ann/work/gone.ics", limits, &mut meter).unwrap();
        let built = DavResponse::with_status(href, Status::NOT_FOUND);

        let (read, _) = drain(MINIMAL, limits);
        assert_eq!(read.len(), 1);
        assert_eq!(read.first(), Some(&built));
    }

    /// The cap is one number and both directions meet it: a server building a group and a
    /// client reading the same group are refused at the same bound, under the same name.
    #[test]
    fn one_property_cap_binds_the_read_and_the_build_alike() {
        let limits = Limits::DEFAULT.with_max_props_per_response(1);
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();

        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: prop_name(ElementName::Getetag),
                    value: PropValue::Empty,
                },
                &mut meter,
            )
            .unwrap();
        let building = group.push(
            DavProperty {
                name: prop_name(ElementName::Displayname),
                value: PropValue::Empty,
            },
            &mut meter,
        );

        let reading = {
            let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
            collect(PROPFIND, &mut context)
        };
        assert_eq!(building, Err(DavError::Limit(LimitExceeded::Properties)));
        assert_eq!(reading.map(|_| ()), building);
    }

    /// The response cap ends the stream with what was read intact, for both consumers.
    ///
    /// A streaming caller sees the stream end and the note beside it; `MultiStatus::read` keeps
    /// the responses it got, which is the behavior that file documents and which an error at
    /// the same number would have taken away.
    #[test]
    fn the_response_cap_truncates_the_stream_rather_than_refusing_the_body() {
        let limits = Limits::DEFAULT.with_max_responses(1);
        let (read, reported) = drain(SABREDAV, limits);
        assert_eq!(read.len(), 1);
        assert_eq!(codes(&reported), [DiagnosticCode::DavResponsesTruncated]);

        let mut meter = Meter::new(limits);
        let mut second: Vec<Diagnostic> = Vec::new();
        let mut pull = Pull::new(SABREDAV);
        let mut reader = MultiStatusReader::new(&mut pull);
        let mut context = DecodeContext::new(limits, &mut meter, &mut second);
        let owned = MultiStatus::read(&mut reader, &mut context).unwrap();
        assert_eq!(owned.responses().len(), 1);
    }

    /// RFC 6578's token round-trips through the client without being interpreted.
    #[test]
    fn a_sync_token_arrives_opaque_and_only_once_the_body_has_been_drained() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut pull = Pull::new(SYNC);
        let mut reader = MultiStatusReader::new(&mut pull);
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);

        assert!(reader.next_response(&mut context).unwrap().is_some());
        // RFC 6578 section 3 puts the token after the responses, so reading it early is not a
        // promise. Draining is what makes it available.
        while reader.next_response(&mut context).unwrap().is_some() {}
        assert_eq!(
            reader.sync_token(),
            Some(b"http://radicale.example/ns/sync-token/1234".as_slice())
        );
    }

    /// What a body cannot state is reported, and the read carries on to the next response.
    #[test]
    fn a_response_that_states_nothing_is_reported_and_never_read_as_success() {
        let cases = [
            (
                "a status line nothing can read",
                UNREADABLE,
                DiagnosticCode::DavStatusUnreadable,
                1,
            ),
            (
                "a response naming no resource",
                HREFLESS,
                DiagnosticCode::DavResponseWithoutHref,
                1,
            ),
        ];
        for (shape, body, expected, survivors) in cases {
            let (read, reported) = drain(body, Limits::DEFAULT);
            assert_eq!(read.len(), survivors, "{shape}");
            assert!(codes(&reported).contains(&expected), "{shape}");
        }

        // The properties of an unreadable group are kept — they arrived — and no caller is
        // told they came back.
        let (read, _) = drain(UNREADABLE, Limits::DEFAULT);
        let first = read.first().unwrap();
        let group = first.propstats().first().unwrap();
        assert_eq!(group.props().len(), 1);
        assert_eq!(group.status, UNREADABLE_STATUS);
        assert!(
            first
                .successful_value(&prop_name(ElementName::Getetag))
                .is_none()
        );
    }

    /// A property this crate has no model for is kept whole and reported, never discarded.
    #[test]
    fn a_value_with_no_modeled_shape_is_kept_and_said_to_be_uninterpreted() {
        let (read, reported) = drain(UNMODELED, Limits::DEFAULT);
        let first = read.first().unwrap();
        // RFC 6638 section 2.4.1 makes this a set, and `PropValue` holds one reference — so
        // the value has no modeled shape and is kept as the peer's own elements. Both
        // addresses are there, and so is the structure that says they are two of them: a
        // caller re-encoding this property emits a set and not a concatenation.
        let wanted = prop_name(ElementName::CalendarUserAddressSet);
        let addresses = match first.successful_value(&wanted) {
            Some(PropValue::Markup(kept)) => kept.clone(),
            other => panic!("{other:?}"),
        };
        assert!(
            addresses
                .windows(26)
                .any(|at| at == b"mailto:ann@example.invalid")
        );
        assert!(addresses.windows(17).any(|at| at == b"urn:uuid:0f5c1e2a"));
        let tags = addresses.iter().fold(0_usize, |seen, byte| match *byte {
            b'<' => seen.saturating_add(1),
            _ => seen,
        });
        assert_eq!(tags, 4, "two `href` elements, opened and closed");
        assert!(codes(&reported).contains(&DiagnosticCode::DavPropertyUnmodeled));

        // One `href` under a name modeled as a reference is a reference.
        assert!(matches!(
            first.successful_value(&prop_name(ElementName::CurrentUserPrincipal)),
            Some(PropValue::Reference(_))
        ));
        // An unquoted `ETag` is a value this crate refuses to guess at rather than a tag.
        assert!(matches!(
            first.successful_value(&prop_name(ElementName::Getetag)),
            Some(PropValue::Unmodeled(_))
        ));
    }

    /// A `DAV:error` reaches the caller as the conditions it named.
    #[test]
    fn a_precondition_travels_as_the_element_that_names_it() {
        let (read, _) = drain(REFUSED, Limits::DEFAULT);
        let first = read.first().unwrap();
        let named = first.error.as_ref().unwrap();
        assert_eq!(
            named.conditions(),
            [prop_name(
                ElementName::AllowedOrganizerSchedulingObjectChange
            )]
            .as_slice()
        );
    }

    /// The grouped `href` form is refused rather than read as one resource with the rest lost.
    #[test]
    fn a_second_href_in_one_response_is_refused_rather_than_dropped() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let refused = {
            let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
            collect(GROUPED, &mut context)
        };
        assert_eq!(refused, Err(DavError::Unexpected(ElementName::Href)));
    }

    /// The client asks with one bound of its window open, and reads what came back.
    ///
    /// RFC 4791 section 9.9 permits an open end, and the request half of this exchange is a
    /// value whose `end()` is absent. The response half is what this unit reads, and the two
    /// are asserted together because a query whose answer nobody can read is not an exchange.
    #[test]
    fn a_half_open_window_is_asked_for_and_its_answer_is_read() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);

        let mut asked = CalendarQuery::new(limits);
        asked
            .props
            .push(prop_name(ElementName::Getetag), &mut meter)
            .unwrap();
        asked
            .props
            .push(prop_name(ElementName::CalendarData), &mut meter)
            .unwrap();
        let mut events = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
        events.time_range = Some(TimeRange::starting_at(Instant::from_unix_seconds(
            1_767_225_600,
        )));
        let mut calendar = CompFilter::new(b"VCALENDAR", limits, &mut meter).unwrap();
        calendar.push_comp(events, limits, &mut meter).unwrap();
        asked.filter = Some(calendar);

        let window = asked
            .filter
            .as_ref()
            .and_then(|tree| tree.comps().first())
            .and_then(|child| child.time_range)
            .unwrap();
        assert!(window.start().is_some());
        assert!(
            window.end().is_none(),
            "an open end is one RFC 4791 permits"
        );

        let (read, _) = drain(SABREDAV, limits);
        let first = read.first().unwrap();
        // Everything the query asked for came back for the resource that matched the window.
        for wanted in asked.props.names() {
            assert!(first.successful_value(wanted).is_some(), "{wanted:?}");
        }
    }

    /// The stand-in tokenizer resolves names the way the landed vocabulary does.
    ///
    /// Asserted directly rather than trusted, because every case above reads through it: if it
    /// resolved on prefixes, the three-server case would pass for the wrong reason.
    #[test]
    fn the_stand_in_tokenizer_resolves_a_name_and_never_a_prefix() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut pull = Pull::new(b"<x:multistatus xmlns:x=\"DAV:\"/>");
        let opened = pull.next_event(&mut context).unwrap().unwrap();
        match opened {
            XmlEvent::Start { name, known, depth } => {
                assert_eq!(name, QName::new(Namespace::Dav, b"multistatus"));
                assert_eq!(known, Some(ElementName::Multistatus));
                assert_eq!(depth, 1);
            },
            other => panic!("{other:?}"),
        }
        // A familiar prefix over another namespace is a different element.
        let mut hostile = Pull::new(b"<D:multistatus xmlns:D=\"http://evil.example/\"/>");
        match hostile.next_event(&mut context).unwrap().unwrap() {
            XmlEvent::Start { known, .. } => assert_eq!(known, None),
            other => panic!("{other:?}"),
        }
    }

    /// A `SabreDAV` body carrying the vendor elements that server actually emits.
    const EXTENDED: &[u8] = b"<?xml version=\"1.0\"?>\n\
<d:multistatus xmlns:d=\"DAV:\" xmlns:s=\"http://sabredav.org/ns\">\n\
 <d:response>\n\
  <d:href>/calendars/ann/work/1.ics</d:href>\n\
  <s:exception>Sabre\\DAV\\Exception\\NotFound</s:exception>\n\
  <d:propstat>\n\
   <d:prop>\n\
    <d:getetag>\"5f2b8c1e9a04\"</d:getetag>\n\
    <s:sync-token-length>36</s:sync-token-length>\n\
   </d:prop>\n\
   <s:sabredav-version>4.4.0</s:sabredav-version>\n\
   <d:status>HTTP/1.1 200 OK</d:status>\n\
  </d:propstat>\n\
 </d:response>\n\
</d:multistatus>\n";

    /// A `PROPFIND` on a calendar collection, answered the way a deployed server answers one.
    const PROPFIND: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/</D:href>\n\
    <D:propstat>\n\
      <D:prop>\n\
        <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\n\
        <D:displayname>Work</D:displayname>\n\
        <CS:getctag>http://example.invalid/ns/sync/42</CS:getctag>\n\
      </D:prop>\n\
      <D:status>HTTP/1.1 200 OK</D:status>\n\
    </D:propstat>\n\
    <D:propstat>\n\
      <D:prop>\n\
        <C:calendar-description/>\n\
      </D:prop>\n\
      <D:status>HTTP/1.1 404 Not Found</D:status>\n\
    </D:propstat>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// One resource, one status, no properties: the smallest multistatus a server sends.
    const MINIMAL: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\">\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/gone.ics</D:href>\n\
    <D:status>HTTP/1.1 404 Not Found</D:status>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// A `propstat` whose status line is not one RFC 4918 section 14.28 can be read from.
    const UNREADABLE: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\">\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/1.ics</D:href>\n\
    <D:propstat>\n\
      <D:prop><D:getetag>\"5f2b8c1e9a04\"</D:getetag></D:prop>\n\
      <D:status>OK</D:status>\n\
    </D:propstat>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// A response naming no resource, followed by one that does.
    const HREFLESS: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\">\n\
  <D:response>\n\
    <D:status>HTTP/1.1 507 Insufficient Storage</D:status>\n\
  </D:response>\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/1.ics</D:href>\n\
    <D:status>HTTP/1.1 200 OK</D:status>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// Three properties whose values sit on either side of what this crate models.
    const UNMODELED: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\n\
  <D:response>\n\
    <D:href>/principals/ann/</D:href>\n\
    <D:propstat>\n\
      <D:prop>\n\
        <C:calendar-user-address-set>\n\
          <D:href>mailto:ann@example.invalid</D:href>\n\
          <D:href>urn:uuid:0f5c1e2a</D:href>\n\
        </C:calendar-user-address-set>\n\
        <D:current-user-principal><D:href>/principals/ann/</D:href></D:current-user-principal>\n\
        <D:getetag>5f2b8c1e9a04</D:getetag>\n\
      </D:prop>\n\
      <D:status>HTTP/1.1 200 OK</D:status>\n\
    </D:propstat>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// A refused write, with the RFC 6638 precondition that explains it.
    const REFUSED: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/1.ics</D:href>\n\
    <D:status>HTTP/1.1 403 Forbidden</D:status>\n\
    <D:error><C:allowed-organizer-scheduling-object-change/></D:error>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// RFC 4918 section 14.24's grouped form, which this reader refuses.
    const GROUPED: &[u8] = b"<?xml version=\"1.0\"?>\n\
<D:multistatus xmlns:D=\"DAV:\">\n\
  <D:response>\n\
    <D:href>/calendars/ann/work/1.ics</D:href>\n\
    <D:href>/calendars/ann/work/2.ics</D:href>\n\
    <D:status>HTTP/1.1 424 Failed Dependency</D:status>\n\
  </D:response>\n\
</D:multistatus>\n";

    /// An RFC 6578 answer: the changed resources, then the token to come back with.
    const SYNC: &[u8] = b"<?xml version='1.0' encoding='utf-8'?>\n\
<multistatus xmlns=\"DAV:\">\n\
  <response>\n\
    <href>/ann/work/1.ics</href>\n\
    <propstat>\n\
      <prop><getetag>\"5f2b8c1e9a04\"</getetag></prop>\n\
      <status>HTTP/1.1 200 OK</status>\n\
    </propstat>\n\
  </response>\n\
  <sync-token>http://radicale.example/ns/sync-token/1234</sync-token>\n\
</multistatus>\n";

    /// A tokenizer over one body, standing in for the one unit 1 is landing beside this file.
    ///
    /// It exists so that this unit's cases are wire bytes rather than hand-built events, and it
    /// is deliberately the smallest thing that reads the bodies above: it resolves prefixes
    /// through a scoped binding stack and hands character data to the landed `decode_text`, and
    /// it refuses nothing, because refusing is the shipped tokenizer's subject and not this
    /// unit's. Attribute values are read as far as the first whitespace, which every body here
    /// and every fixture stays inside.
    struct Pull<'body> {
        /// The octets being read.
        body: &'body [u8],
        /// How far into them the reader sits.
        at: usize,
        /// The elements currently open, outermost first.
        open: Vec<Frame<'body>>,
        /// The prefix bindings currently live.
        bound: Vec<Binding<'body>>,
        /// The attributes of the element that opened last.
        attributes: Vec<Binding<'body>>,
        /// Whether that element was empty and still owes its end event.
        closing: bool,
    }

    /// One open element.
    #[derive(Clone, Copy, Debug)]
    struct Frame<'body> {
        /// Its resolved name.
        name: QName<'body>,
        /// The row of the vocabulary it lands on, if any.
        known: Option<ElementName>,
        /// How deep it sits.
        depth: u16,
    }

    /// One namespace binding, or one attribute, as the document spelled it.
    #[derive(Clone, Copy, Debug)]
    struct Binding<'body> {
        /// The prefix bound, or the attribute's name as written.
        prefix: &'body [u8],
        /// The URI it is bound to, or the attribute's value.
        value: &'body [u8],
        /// The depth at which it was declared.
        depth: u16,
    }

    impl<'body> Pull<'body> {
        fn new(body: &'body [u8]) -> Self {
            Self {
                body,
                at: 0,
                open: Vec::new(),
                bound: Vec::new(),
                attributes: Vec::new(),
                closing: false,
            }
        }

        fn starts_with(&self, needle: &[u8]) -> bool {
            self.body
                .get(self.at..)
                .is_some_and(|rest| rest.starts_with(needle))
        }

        fn find_at(&self, needle: &[u8], from: usize) -> Option<usize> {
            let rest = self.body.get(from..)?;
            rest.windows(needle.len())
                .position(|window| window == needle)
                .map(|found| from.saturating_add(found))
        }

        /// Step over the XML declaration and any comment, neither of which is an event.
        fn skip_prologue(&mut self) {
            while self.starts_with(b"<?") || self.starts_with(b"<!--") {
                let needle: &[u8] = if self.starts_with(b"<?") {
                    b"?>"
                } else {
                    b"-->"
                };
                self.at = match self.find_at(needle, self.at.saturating_add(2)) {
                    Some(found) => found.saturating_add(needle.len()),
                    None => self.body.len(),
                };
            }
        }

        /// Where the tag open at `self.at` ends, quotes respected.
        fn tag_end(&self) -> usize {
            let mut at = self.at;
            let mut quote: Option<u8> = None;
            while let Some(&byte) = self.body.get(at) {
                match (quote, byte) {
                    (None, b'"' | b'\'') => quote = Some(byte),
                    (Some(open), found) if open == found => quote = None,
                    (None, b'>') => return at,
                    _ => {},
                }
                at = at.saturating_add(1);
            }
            self.body.len()
        }

        /// A raw name split into its prefix and its local part.
        fn split_name(raw: &'body [u8]) -> (&'body [u8], &'body [u8]) {
            match raw.iter().position(|byte| *byte == b':') {
                Some(colon) => (
                    raw.get(..colon).unwrap_or(&[]),
                    raw.get(colon.saturating_add(1)..).unwrap_or(&[]),
                ),
                None => (&[], raw),
            }
        }

        fn lookup(&self, prefix: &[u8]) -> Namespace<'body> {
            self.bound
                .iter()
                .rev()
                .find(|held| held.prefix == prefix)
                .map_or(Namespace::Other(&[]), |held| {
                    Namespace::from_uri(held.value)
                })
        }

        /// Record one attribute, keeping a namespace declaration as a binding.
        fn declare(&mut self, piece: &'body [u8], depth: u16) {
            let Some(equals) = piece.iter().position(|byte| *byte == b'=') else {
                return;
            };
            let name = piece.get(..equals).unwrap_or(&[]);
            let quoted = piece.get(equals.saturating_add(1)..).unwrap_or(&[]);
            let value = quoted
                .strip_prefix(b"\"")
                .and_then(|open| open.strip_suffix(b"\""))
                .or_else(|| {
                    quoted
                        .strip_prefix(b"'")
                        .and_then(|open| open.strip_suffix(b"'"))
                })
                .unwrap_or(quoted);
            let held = Binding {
                prefix: name,
                value,
                depth,
            };
            if name == b"xmlns" {
                self.bound.push(Binding {
                    prefix: &[],
                    ..held
                });
            } else if let Some(prefix) = name.strip_prefix(b"xmlns:".as_slice()) {
                self.bound.push(Binding { prefix, ..held });
            } else {
                self.attributes.push(held);
            }
        }

        fn start_tag(&mut self) -> XmlEvent<'body> {
            let body = self.body;
            let close = self.tag_end();
            let raw = body.get(self.at.saturating_add(1)..close).unwrap_or(&[]);
            let empty = raw.ends_with(b"/");
            let inner = if empty {
                raw.get(..raw.len().saturating_sub(1)).unwrap_or(&[])
            } else {
                raw
            };
            self.at = close.saturating_add(1);
            let depth = u16::try_from(self.open.len().saturating_add(1)).unwrap_or(u16::MAX);
            self.attributes.clear();
            let mut pieces = inner
                .split(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
                .filter(|piece| !piece.is_empty());
            let spelled = pieces.next().unwrap_or(&[]);
            for piece in pieces {
                self.declare(piece, depth);
            }
            let (prefix, local) = Self::split_name(spelled);
            let name = QName::new(self.lookup(prefix), local);
            let known = name.known();
            self.open.push(Frame { name, known, depth });
            self.closing = empty;
            XmlEvent::Start { name, known, depth }
        }

        /// Close the element on top of the stack and drop the bindings it declared.
        fn leave(&mut self) -> XmlEvent<'body> {
            let frame = self.open.pop().unwrap();
            self.bound.retain(|held| held.depth < frame.depth);
            XmlEvent::End {
                name: frame.name,
                known: frame.known,
                depth: frame.depth,
            }
        }

        fn end_tag(&mut self) -> XmlEvent<'body> {
            let close = self.tag_end();
            self.at = close.saturating_add(1);
            self.leave()
        }

        /// The character data up to the next tag, `CDATA` sections carried inside it.
        fn text(&mut self, context: &mut DecodeContext<'_>) -> Result<XmlEvent<'body>, DavError> {
            let body = self.body;
            let start = self.at;
            let mut at = self.at;
            while let Some(&byte) = body.get(at) {
                if byte != b'<' {
                    at = at.saturating_add(1);
                    continue;
                }
                if !body
                    .get(at..)
                    .is_some_and(|rest| rest.starts_with(b"<![CDATA["))
                {
                    break;
                }
                at = match self.find_at(b"]]>", at) {
                    Some(found) => found.saturating_add(3),
                    None => body.len(),
                };
            }
            let span = body.get(start..at).unwrap_or(&[]);
            self.at = at;
            let inside = self.open.last().and_then(|frame| frame.known);
            let offset = u64::try_from(start).unwrap_or(u64::MAX);
            let mode = TextMode::of(inside, context.text);
            let decoded =
                crate::text::decode_text(span, mode, offset, context.meter, context.sink)?;
            Ok(XmlEvent::Text(decoded))
        }
    }

    impl<'body> XmlPull<'body> for Pull<'body> {
        fn next_event(
            &mut self,
            context: &mut DecodeContext<'_>,
        ) -> Result<Option<XmlEvent<'body>>, DavError> {
            if self.closing {
                self.closing = false;
                return Ok(Some(self.leave()));
            }
            self.skip_prologue();
            let Some(&byte) = self.body.get(self.at) else {
                return Ok(None);
            };
            if byte != b'<' {
                return self.text(context).map(Some);
            }
            if self.starts_with(b"</") {
                return Ok(Some(self.end_tag()));
            }
            Ok(Some(self.start_tag()))
        }

        fn skip_element(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
            let target = self.open.len().saturating_sub(1);
            while self.open.len() > target {
                if self.next_event(context)?.is_none() {
                    break;
                }
            }
            Ok(())
        }

        fn depth(&self) -> u16 {
            u16::try_from(self.open.len()).unwrap_or(u16::MAX)
        }

        fn offset(&self) -> u64 {
            u64::try_from(self.at).unwrap_or(u64::MAX)
        }

        fn resolve_prefix(&self, prefix: &[u8]) -> Option<Namespace<'body>> {
            self.bound
                .iter()
                .rev()
                .find(|held| held.prefix == prefix)
                .map(|held| Namespace::from_uri(held.value))
        }

        fn attribute(&self, name: QName<'_>) -> Option<&[u8]> {
            (0..self.attribute_count())
                .filter_map(|index| self.attribute_at(index))
                .find(|(held, _)| {
                    held.namespace.is(name.namespace) && held.local_name == name.local_name
                })
                .map(|(_, value)| value)
        }

        fn attribute_count(&self) -> usize {
            self.attributes.len()
        }

        fn attribute_at(&self, index: usize) -> Option<(QName<'body>, &[u8])> {
            let held = self.attributes.get(index)?;
            let (prefix, local) = Self::split_name(held.prefix);
            // XML Namespaces 1.0 section 6.2: a default declaration never reaches an
            // attribute, so an unprefixed one is in no namespace at all.
            let space = if prefix.is_empty() {
                Namespace::Other(&[])
            } else {
                self.lookup(prefix)
            };
            Some((QName::new(space, local), held.value))
        }
    }
}
