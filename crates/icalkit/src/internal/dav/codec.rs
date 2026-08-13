// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The four traits every body in this crate is read and written through.
//!
//! The asymmetry worth noticing is that [`ReadXml`] carries a diagnostic sink and [`WriteXml`]
//! does not. That line runs along decode versus encode — both of which a client and a server
//! each do — and never along client versus server. A decoder is handed octets somebody else
//! chose and has things to report about them; an encoder is handed a value this crate's own
//! constructors already refused the bad shapes of.
//!
//! [`ResponseSource`] carries no lifetime parameter and is object-safe, so a caller can hold
//! `&mut dyn ResponseSource` without a generic spreading through its own types. It is
//! deliberately not an `Iterator`: `Iterator::next` takes nothing but `&mut self`, and every
//! read here carries the caller's policy, ledger and sink.

use ical_core::{Limits, Meter};

use crate::internal::dav::element::{ElementName, Namespace, QName};
use crate::internal::dav::failure::DavError;
use crate::internal::dav::policy::DecodeContext;
use crate::internal::dav::response::DavResponse;
use crate::internal::dav::sink::ByteSink;
use crate::internal::dav::text::DecodedText;

/// One step of a document, as the tokenizer yields it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum XmlEvent<'a> {
    /// An element opened.
    Start {
        /// Its resolved name.
        name: QName<'a>,
        /// The row of the closed vocabulary it lands on, if any.
        known: Option<ElementName>,
        /// How deep it sits, with the root element at one.
        depth: u16,
    },
    /// An element closed.
    End {
        /// Its resolved name.
        name: QName<'a>,
        /// The row of the closed vocabulary it lands on, if any.
        known: Option<ElementName>,
        /// How deep it sat.
        depth: u16,
    },
    /// Character data, decoded under the mode its element earns.
    Text(DecodedText<'a>),
}

/// A pull tokenizer over one body.
///
/// The lifetime is the body's, so [`crate::internal::dav::TextRun::Wire`] can borrow from it and a
/// `calendar-data` payload reaches `ical-core` without being copied.
pub trait XmlPull<'a> {
    /// The next event, or `None` at the end of the document.
    fn next_event(
        &mut self,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<XmlEvent<'a>>, DavError>;

    /// Consume the element that has just started, and everything inside it.
    fn skip_element(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError>;

    /// How deep the reader currently sits.
    fn depth(&self) -> u16;

    /// How far into the body the reader currently sits, in octets.
    fn offset(&self) -> u64;

    /// The namespace a prefix is bound to at the current position, if any.
    ///
    /// Present so that a caller reading an attribute value that names an element — a
    /// `supported-report` name, for instance — resolves it the same way the reader does.
    /// It takes a prefix because a document's own octets are the only place prefixes exist;
    /// nothing in this crate's vocabulary is keyed on what it returns except through
    /// [`ElementName::resolve`].
    fn resolve_prefix(&self, prefix: &[u8]) -> Option<Namespace<'a>>;

    /// An attribute of the element that has just started, by resolved name.
    ///
    /// An unprefixed attribute is in no namespace at all — XML Namespaces 1.0 section 6.2 is
    /// explicit that a default declaration does not apply to attributes — so `start` and `end`
    /// on a `time-range` are looked up with [`Namespace::Other`] over an empty URI.
    ///
    /// The value is the one XML 1.0 section 3.3.3 defines and not the octets between the
    /// quotes: references are resolved and a literal tab, line feed or carriage return has
    /// become a space. It therefore borrows the tokenizer rather than the body, and it is only
    /// about the element whose `Start` was handed back last.
    fn attribute(&self, name: QName<'_>) -> Option<&[u8]>;

    /// How many attributes that element carries, namespace declarations excluded.
    ///
    /// Present with [`XmlPull::attribute_at`] so that a reader keeping a foreign subtree can
    /// keep what was written *on* its elements too. Looking a name up requires knowing it, and
    /// the whole point of a foreign element is that this crate does not.
    fn attribute_count(&self) -> usize;

    /// One of those attributes, by index, resolved and normalized like [`XmlPull::attribute`].
    ///
    /// The order is the tokenizer's own and is not the document's: XML gives attribute order
    /// no meaning, and this reader sorts them so that a repeated name is found by a walk
    /// rather than by comparing every pair with every other.
    fn attribute_at(&self, index: usize) -> Option<(QName<'a>, &[u8])>;
}

/// A value that can be read out of a document.
pub trait ReadXml: Sized {
    /// Read one value, starting at the element that has just opened.
    fn read_xml(
        events: &mut dyn XmlPull<'_>,
        context: &mut DecodeContext<'_>,
    ) -> Result<Self, DavError>;
}

/// A value that can be written as a document or as a fragment of one.
pub trait WriteXml {
    /// Write the value into `out`, charging what it costs.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError>;
}

/// A multistatus, delivered one response at a time.
///
/// This is the ingestion primitive and the owned [`crate::internal::dav::MultiStatus`] is one consumer of it.
/// No response-count bound both defends a client with tens of kilobytes against a forged flood
/// and lets a server enumerate a real forty-thousand-resource collection, because a count
/// cannot tell a truthful entry from a forged one. A reader that never builds the collection
/// does not need that bound to be right.
pub trait ResponseSource {
    /// The next response, or `None` when the multistatus is finished.
    fn next_response(
        &mut self,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<DavResponse>, DavError>;

    /// The `DAV:sync-token` the body carried, once it has been reached.
    ///
    /// RFC 6578 section 3 puts the token after the responses, so this answers `None` until the
    /// source has been drained. Reading it early is not an error and not a promise.
    fn sync_token(&self) -> Option<&[u8]>;

    /// Whether this source stopped short of the body's own end.
    ///
    /// A source that cut the stream at a bound has delivered part of an answer, and RFC 6578
    /// section 3.4 makes `sync_token` a statement about the whole of one. A consumer needs the
    /// two facts together or it cannot tell "the server sent no token" from "the token this
    /// source is holding covers changes it never handed over".
    ///
    /// The default is `false`, which is the honest answer for a source that has no bound of
    /// its own to stop at.
    fn was_truncated(&self) -> bool {
        false
    }
}
