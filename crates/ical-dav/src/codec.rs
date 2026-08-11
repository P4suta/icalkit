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

use crate::element::{ElementName, Namespace, QName};
use crate::failure::DavError;
use crate::policy::DecodeContext;
use crate::response::DavResponse;
use crate::sink::ByteSink;
use crate::text::DecodedText;

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
/// The lifetime is the body's, so [`crate::TextRun::Wire`] can borrow from it and a
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
    fn attribute(&self, name: QName<'_>) -> Option<&'a [u8]>;
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
/// This is the ingestion primitive and the owned [`crate::MultiStatus`] is one consumer of it.
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
}
