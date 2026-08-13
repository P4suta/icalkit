// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The element writer, which is strictly conformant XML where the reader beside it is not.
//!
//! [`crate::internal::dav::text`] states the one place this crate's *reader* departs from XML 1.0: inside
//! `CALDAV:calendar-data` it hands back the octets as they arrived rather than applying section
//! 2.11 line-break normalization. Writing needs no such departure and takes none. A carriage
//! return leaves here as the character reference `&#13;`, which section 2.11 never reaches
//! because a reference is markup resolved *after* normalization, so any conformant processor —
//! this crate's, `libxml2`'s, a peer's — recovers the octet. Everything this file emits is
//! readable by a parser that has never heard of the carve-out.
//!
//! # What the type makes impossible
//!
//! **A foreign element.** [`XmlWriter::open`] takes an [`ElementName`], and the prefix comes
//! from [`Namespace::write_prefix`] rather than from anything a caller supplies. A name outside
//! the closed vocabulary has exactly one door, [`XmlWriter::open_extension`], which declares the
//! namespace on the element that uses it and refuses a local name it cannot verify.
//!
//! **An unbalanced document.** Every open element is on a stack. [`XmlWriter::close`] pops it
//! and writes the end tag the stack says, so an end tag naming the wrong element is not a
//! mistake that can be made; [`XmlWriter::finish`] closes whatever is still open, so a document
//! cannot end inside an element; and closing more than was opened is refused rather than
//! written. A start tag whose `>` is still unwritten becomes `/>` if it is closed with nothing
//! inside it, which is what `<D:displayname/>` in every real server's body is.
//!
//! **Two attributes of one name on one element,** which XML forbids and which a peer refuses
//! the whole body over. The names written on the tag still pending are held in a buffer this
//! writer reuses, so the check costs one allocation for the life of the writer and none per
//! element.
//!
//! **A namespace declaration a caller wrote.** [`XmlWriter::attribute`] refuses `xmlns` and
//! `xmlns:`-prefixed names. The prefixes this crate writes are its own fixed output choice, and
//! a caller able to rebind `D:` could make every element under it mean something else.
//!
//! **A `CDATA` section**, because there is no door that emits one. It cannot carry a `CR` past
//! a conformant reader, and it would turn a literal `]]>` inside a `DESCRIPTION` into an
//! escaping bug; `>` is escaped unconditionally by [`write_escaped_text`], so that sequence is
//! unwritable by accident.
//!
//! # What it does not check
//!
//! Whether an element is permitted where it was written. `<D:href>` inside `<D:prop>` is
//! well-formed XML and a nonsense `PROPFIND`, and the grammar of each body belongs to the unit
//! that encodes that body. This file guarantees well-formedness, escaping and bounding, and
//! makes no claim about RFC 4918's or RFC 4791's content models.
//!
//! # What it charges
//!
//! Every octet is charged against the caller's [`Meter`] before it reaches the sink, escaped
//! length rather than input length — charging what was handed in would undercount by exactly
//! the amount an attacker chooses to escape. Each element charges one element and one level of
//! depth as well, so a body this crate writes is bounded by the same `Limits` value that bounds
//! the one it reads, and many bodies under one ledger are bounded in aggregate
//! (`docs/adr/0010`).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

use ical_core::{LimitExceeded, Meter};

use crate::internal::dav::element::{ElementName, Namespace};
use crate::internal::dav::failure::{DavError, SinkFull, SyntaxError};
use crate::internal::dav::sink::ByteSink;
use crate::internal::dav::text::{write_escaped_attribute, write_escaped_text};
use crate::internal::dav::value::{ExtensionName, copy};

/// The declaration every document this crate writes begins with.
///
/// `UTF-8` is the only encoding this crate's reader accepts, and stating it is what lets a peer
/// that guessed otherwise be wrong loudly rather than quietly.
const DECLARATION: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// The namespaces every root element declares, in the order it declares them.
///
/// All three unconditionally, so the root of a `PROPFIND` and the root of a multistatus carry
/// the same preamble and no encoder has to work out which of them a body will end up needing.
const DECLARED: [Namespace<'static>; 3] =
    [Namespace::Dav, Namespace::CalDav, Namespace::CalendarServer];

/// The prefix a namespace outside the closed vocabulary is bound to.
///
/// Declared on the element that uses it rather than once on the root, because two extension
/// properties in one body routinely come from two different namespaces and a single root
/// declaration could serve only the first. A redeclaration is scoped to its own element, so the
/// binding a foreign element is read under is always the one written on it.
const EXTENSION_PREFIX: &[u8] = b"X";

/// One element opened and not yet closed.
///
/// A row of the table costs nothing to remember; a name outside it has to be copied, because
/// the end tag is written after the caller's own borrow of the name is long gone.
#[derive(Clone, Debug, PartialEq, Eq)]
enum OpenTag {
    /// A row of the closed vocabulary, whose prefix and local name are both `'static`.
    Known(ElementName),
    /// A name outside it, kept as octets so its end tag can be written.
    Foreign(Box<[u8]>),
}

impl OpenTag {
    /// The prefix this element's end tag is written with.
    fn prefix(&self) -> &[u8] {
        match *self {
            Self::Known(name) => name.namespace().write_prefix(),
            Self::Foreign(_) => EXTENSION_PREFIX,
        }
    }

    /// The element's local name.
    fn local_name(&self) -> &[u8] {
        match self {
            Self::Known(name) => name.local_name().as_bytes(),
            Self::Foreign(local) => local,
        }
    }
}

/// A writer over one document, holding the caller's sink and the caller's ledger.
///
/// Built once per body and driven by the unit that encodes that body. The lifetime is the
/// borrow of both, so a caller keeps its own `Vec` or its own buffer and gets it back.
pub struct XmlWriter<'a> {
    /// Where the octets go.
    out: &'a mut dyn ByteSink,
    /// The caller's ledger, charged before every octet reaches the sink.
    meter: &'a mut Meter,
    /// The elements opened and not yet closed, innermost last.
    open: Vec<OpenTag>,
    /// The attribute names on the tag still pending, each preceded by its own length.
    attributes: Vec<u8>,
    /// Whether a start tag's `>` is still unwritten, so attributes may still be added to it.
    pending: bool,
    /// Whether the declaration and the root element have been written.
    started: bool,
}

impl<'a> XmlWriter<'a> {
    /// A writer that will emit one document into `out`, charging `meter` for it.
    #[must_use]
    pub fn new(out: &'a mut dyn ByteSink, meter: &'a mut Meter) -> Self {
        Self {
            out,
            meter,
            open: Vec::new(),
            attributes: Vec::new(),
            pending: false,
            started: false,
        }
    }

    /// How many elements are open, with the root counting as one.
    #[must_use]
    pub fn depth(&self) -> u16 {
        u16::try_from(self.open.len()).unwrap_or(u16::MAX)
    }

    /// Open one element of the closed vocabulary.
    ///
    /// The first call also writes the XML declaration and the fixed namespace declarations, so
    /// a document without them is not a thing an encoder can forget to produce. Attributes may
    /// be added until anything else is written.
    pub fn open(&mut self, name: ElementName) -> Result<(), DavError> {
        if !name.is_supported() {
            // The table is unconditional and the build is not. Writing an element this build
            // cannot honor would produce a request it could not then read back.
            return Err(DavError::Unsupported(name));
        }
        let root = self.begin_element()?;
        self.emit(b"<")?;
        self.emit(name.namespace().write_prefix())?;
        self.emit(b":")?;
        self.emit(name.local_name().as_bytes())?;
        if root {
            self.declare_fixed()?;
        }
        self.open.push(OpenTag::Known(name));
        Ok(())
    }

    /// Open one element whose name is outside the closed vocabulary.
    ///
    /// The one door that writes a name this crate has no row for, and it exists because the
    /// crate is symmetric: a client that read a vendor property out of a `PROPFIND` response
    /// must be able to ask for it again, and an `ErrorBody` carries the names a server refused
    /// whatever namespace they came from. The namespace is declared on the element itself, so
    /// no binding of this crate's own is touched.
    ///
    /// The local name is refused unless it is an ASCII XML `Name`. XML admits far more than
    /// that, and every name this crate writes is ASCII, so a name it cannot verify is refused
    /// rather than emitted unchecked — at the stated cost that a server's non-ASCII property
    /// name is one this crate can read and not write back.
    pub fn open_extension(&mut self, name: &ExtensionName) -> Result<(), DavError> {
        let local = name.local_name();
        check_name(local)?;
        let root = self.begin_element()?;
        let kept = copy(local)?;
        self.emit(b"<")?;
        self.emit(EXTENSION_PREFIX)?;
        self.emit(b":")?;
        self.emit(local)?;
        if root {
            self.declare_fixed()?;
        }
        self.declare(EXTENSION_PREFIX, name.namespace())?;
        self.open.push(OpenTag::Foreign(kept));
        Ok(())
    }

    /// Add one attribute to the element that has just been opened.
    ///
    /// The value is escaped for the rules that apply inside quotes, so a literal tab, newline
    /// or carriage return survives attribute-value normalization as a character reference
    /// rather than being replaced by a space.
    pub fn attribute(&mut self, name: &[u8], value: &[u8]) -> Result<(), DavError> {
        if !self.pending {
            // An attribute belongs to a start tag, and there is no start tag still open.
            return Err(SyntaxError::Malformed.into());
        }
        if name == b"xmlns".as_slice() || name.starts_with(b"xmlns:".as_slice()) {
            // The prefixes this crate writes are its own; a caller that could rebind one could
            // change what every element under it means.
            return Err(SyntaxError::Malformed.into());
        }
        check_name(name)?;
        self.note_attribute(name)?;
        self.emit(b" ")?;
        self.emit(name)?;
        self.emit(b"=\"")?;
        self.emit_escaped(value, true)?;
        self.emit(b"\"")
    }

    /// Write character data inside the element that is open.
    ///
    /// Escaped so that a conformant reader recovers every octet handed in, which is what makes
    /// the write half of the line-ending resolution cost nothing: `CR` leaves as `&#13;`.
    pub fn text(&mut self, bytes: &[u8]) -> Result<(), DavError> {
        if self.open.is_empty() {
            // Character data outside the root element is not a document.
            return Err(SyntaxError::Malformed.into());
        }
        self.seal()?;
        self.emit_escaped(bytes, false)
    }

    /// Close the innermost open element.
    ///
    /// An element closed with nothing written inside it becomes `<D:displayname/>` rather than
    /// a pair of tags, which is the same infoset and the shape every deployed server writes.
    pub fn close(&mut self) -> Result<(), DavError> {
        let tag = self.open.pop().ok_or(SyntaxError::Malformed)?;
        self.meter.leave_element();
        if self.pending {
            self.pending = false;
            return self.emit(b"/>");
        }
        self.emit(b"</")?;
        self.emit(tag.prefix())?;
        self.emit(b":")?;
        self.emit(tag.local_name())?;
        self.emit(b">")
    }

    /// Write one element with nothing inside it.
    pub fn empty(&mut self, name: ElementName) -> Result<(), DavError> {
        self.open(name)?;
        self.close()
    }

    /// Write one element outside the closed vocabulary with nothing inside it.
    ///
    /// The shape a property *name* takes: a `DAV:prop` request and a `DAV:error` body both
    /// carry names without values.
    pub fn empty_extension(&mut self, name: &ExtensionName) -> Result<(), DavError> {
        self.open_extension(name)?;
        self.close()
    }

    /// Write one element carrying `bytes` as its character data.
    ///
    /// Empty content writes `<D:displayname/>`, because an element with no character data and
    /// one with an empty run are the same element and the shorter spelling is what servers
    /// send.
    pub fn element_text(&mut self, name: ElementName, bytes: &[u8]) -> Result<(), DavError> {
        self.open(name)?;
        if !bytes.is_empty() {
            self.text(bytes)?;
        }
        self.close()
    }

    /// Close every element still open, ending the document.
    ///
    /// Idempotent: a second call has nothing left to close. Calling it is what makes a
    /// truncated document unrepresentable rather than merely unlikely — a caller that forgot a
    /// [`XmlWriter::close`] still hands its peer well-formed XML.
    pub fn finish(&mut self) -> Result<(), DavError> {
        while !self.open.is_empty() {
            self.close()?;
        }
        Ok(())
    }

    /// Everything an element does before its own name is written.
    ///
    /// Answers whether this element is the document's root, which is the one that carries the
    /// fixed namespace declarations.
    fn begin_element(&mut self) -> Result<bool, DavError> {
        self.seal()?;
        let root = self.open.is_empty();
        if root && self.started {
            // An XML document has exactly one root element, and a second would be octets no
            // parser accepts.
            return Err(SyntaxError::Malformed.into());
        }
        self.meter.try_charge_element()?;
        self.meter.try_enter_element()?;
        // Reserved here so the push at the end of an element cannot be the step that fails
        // after the start tag is already on the wire.
        self.open
            .try_reserve(1)
            .map_err(|_| DavError::Limit(LimitExceeded::Budget))?;
        if root {
            self.emit(DECLARATION)?;
            self.started = true;
        }
        self.attributes.clear();
        self.pending = true;
        Ok(root)
    }

    /// Write the `>` that ends a start tag, if one is still waiting for it.
    fn seal(&mut self) -> Result<(), DavError> {
        if self.pending {
            self.pending = false;
            self.emit(b">")?;
        }
        Ok(())
    }

    /// Declare the three namespaces this crate writes elements in.
    fn declare_fixed(&mut self) -> Result<(), DavError> {
        for namespace in DECLARED {
            self.declare(namespace.write_prefix(), namespace.uri())?;
        }
        Ok(())
    }

    /// Declare one prefix on the element currently pending.
    fn declare(&mut self, prefix: &[u8], uri: &[u8]) -> Result<(), DavError> {
        self.emit(b" xmlns:")?;
        self.emit(prefix)?;
        self.emit(b"=\"")?;
        self.emit_escaped(uri, true)?;
        self.emit(b"\"")
    }

    /// Record an attribute name, refusing one this element already carries.
    fn note_attribute(&mut self, name: &[u8]) -> Result<(), DavError> {
        let length = u8::try_from(name.len()).map_err(|_| SyntaxError::Malformed)?;
        if holds_attribute(&self.attributes, name) {
            return Err(SyntaxError::DuplicateAttribute.into());
        }
        self.attributes
            .try_reserve(name.len().saturating_add(1))
            .map_err(|_| DavError::Limit(LimitExceeded::Budget))?;
        self.attributes.push(length);
        self.attributes.extend_from_slice(name);
        Ok(())
    }

    /// Charge `bytes` and hand them to the sink, in that order.
    ///
    /// Charging first is what makes the bound bind: a body past the caller's budget is refused
    /// before its octets reach a sink that may already have written them somewhere.
    fn emit(&mut self, bytes: &[u8]) -> Result<(), DavError> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.meter.try_charge_bytes(length)?;
        self.out.write(bytes)?;
        Ok(())
    }

    /// Escape `bytes` into the sink, charging what the escaping actually costs.
    fn emit_escaped(&mut self, bytes: &[u8], quoted: bool) -> Result<(), DavError> {
        let mut charged = Charged {
            out: &mut *self.out,
            meter: &mut *self.meter,
            refused: None,
        };
        let outcome = if quoted {
            write_escaped_attribute(&mut charged, bytes)
        } else {
            write_escaped_text(&mut charged, bytes)
        };
        // A refused charge is a bound the caller stated, not a sink with no room, and the two
        // are different things for a caller deciding whether to retry with a larger buffer.
        match charged.refused {
            Some(exceeded) => Err(DavError::Limit(exceeded)),
            None => outcome,
        }
    }
}

impl Debug for XmlWriter<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        // Neither the sink nor the ledger's borrow is something a caller debugging an encoder
        // wants printed; how deep the writer sits and whether a tag is still open is.
        formatter
            .debug_struct("XmlWriter")
            .field("depth", &self.depth())
            .field("pending", &self.pending)
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

/// A sink that charges the caller's ledger for every octet passing through it.
///
/// [`write_escaped_text`] writes into a [`ByteSink`] directly, and an escaped run is longer
/// than the octets handed in — charging the input's length would undercount by exactly what an
/// attacker chooses to escape. The dimension a refusal named is kept here rather than reported
/// as [`SinkFull`], because a caller told its buffer was full would enlarge a buffer that was
/// never the problem.
struct Charged<'a> {
    /// Where the octets go once they are paid for.
    out: &'a mut dyn ByteSink,
    /// The caller's ledger.
    meter: &'a mut Meter,
    /// The dimension a charge was refused on, if one was.
    refused: Option<LimitExceeded>,
}

impl ByteSink for Charged<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if let Err(exceeded) = self.meter.try_charge_bytes(length) {
            self.refused = Some(exceeded);
            return Err(SinkFull);
        }
        self.out.write(bytes)
    }
}

/// Whether a length-prefixed buffer of attribute names already holds `name`.
fn holds_attribute(buffer: &[u8], name: &[u8]) -> bool {
    let mut at = 0;
    while let Some(&length) = buffer.get(at) {
        let start = at.saturating_add(1);
        let end = start.saturating_add(usize::from(length));
        if buffer.get(start..end) == Some(name) {
            return true;
        }
        at = end;
    }
    false
}

/// Refuse a name this writer will not emit.
///
/// An XML `Name` admits most of Unicode. This accepts the ASCII subset every name in RFC 4918,
/// RFC 4791 and RFC 6578 is drawn from, and refuses everything else rather than emitting octets
/// it has not checked into a position where a stray `<`, space or quote would end the tag.
fn check_name(name: &[u8]) -> Result<(), DavError> {
    let first = *name.first().ok_or(SyntaxError::Malformed)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return Err(SyntaxError::Malformed.into());
    }
    let ordinary = name
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-' | b'.'));
    if ordinary {
        Ok(())
    } else {
        Err(SyntaxError::Malformed.into())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{IgnoreDiagnostics, LimitExceeded, Limits, Meter};

    use super::XmlWriter;
    use crate::internal::dav::element::{ElementName, Namespace};
    use crate::internal::dav::failure::{DavError, SinkFull, SyntaxError};
    use crate::internal::dav::sink::SliceSink;
    use crate::internal::dav::text::{TextMode, decode_text};
    use crate::internal::dav::value::ExtensionName;

    /// What every document this writer produces begins with.
    const PRELUDE: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

    /// The declarations every root carries, whichever element the root is.
    const DECLARATIONS: &[u8] = b" xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\"";

    /// A `calendar-multiget` shaped like RFC 4791 section 7.9's own example.
    const MULTIGET: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<C:calendar-multiget xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<D:prop><D:getetag/><C:calendar-data/></D:prop>\
<D:href>/calendars/ann/work/20260105T090000Z-1.ics</D:href>\
<D:href>/calendars/ann/work/gone.ics</D:href>\
</C:calendar-multiget>";

    /// One resource whose two properties disagree about their status, as RFC 4918 section 9.1
    /// writes one and as the `SabreDAV` fixture beside this crate carries one.
    const MULTISTATUS: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">\
<D:response><D:href>/calendars/ann/work/20260105T090000Z-1.ics</D:href>\
<D:propstat><D:prop><D:getetag>\"5f2b8c1e9a04\"</D:getetag></D:prop>\
<D:status>HTTP/1.1 200 OK</D:status></D:propstat>\
<D:propstat><D:prop><D:displayname/></D:prop>\
<D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>\
</D:response></D:multistatus>";

    /// An `.ics` with the `CRLF` terminators RFC 5545 section 3.1 requires, a content line
    /// folded at octet 75, and the sequence that would end a `CDATA` section this crate must
    /// therefore never open.
    const PAYLOAD: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:20260105T090000Z-1@example.invalid\r\nDESCRIPTION:a literal ]]> & a < inside a note\r\n\
SUMMARY:Weekly sync with a summary long enough that the exporter folded i\r\n t here\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    /// One recorded spelling of a name: the prefix the named server writes, the namespace URI
    /// it resolves to, the local name, and the tag this writer emits — `None` where the URI
    /// makes it a different element entirely.
    type Spelling = (
        &'static [u8],
        &'static [u8],
        &'static [u8],
        Option<&'static [u8]>,
    );

    /// One `time-range`: its start, its end, and the element those two bounds produce.
    type Window = (Option<&'static [u8]>, Option<&'static [u8]>, &'static [u8]);

    /// Build one document under default bounds and answer its octets.
    fn write_document(
        build: impl FnOnce(&mut XmlWriter<'_>) -> Result<(), DavError>,
    ) -> Result<Vec<u8>, DavError> {
        let mut body: Vec<u8> = Vec::new();
        let mut meter = Meter::new(Limits::DEFAULT);
        {
            let mut writer = XmlWriter::new(&mut body, &mut meter);
            build(&mut writer)?;
            writer.finish()?;
        }
        Ok(body)
    }

    /// Whether `body` carries `wanted` somewhere in it.
    fn carries(body: &[u8], wanted: &[u8]) -> bool {
        body.windows(wanted.len()).any(|window| window == wanted)
    }

    /// A prefix is decided by the namespace and never by the local name.
    ///
    /// `DAV:prop` and `CALDAV:prop` are the row this test exists for: one local name, two
    /// elements, two prefixes. A writer keyed on local names would emit the same tag for both
    /// and a server would answer the wrong question.
    #[test]
    fn the_namespace_decides_the_prefix_and_the_local_name_never_does() {
        let cases: [(ElementName, &[u8]); 6] = [
            (ElementName::Prop, b"<D:prop/>"),
            (ElementName::CalendarDataProp, b"<C:prop/>"),
            (ElementName::CalendarData, b"<C:calendar-data/>"),
            (ElementName::Getctag, b"<CS:getctag/>"),
            (ElementName::PrincipalUrl, b"<D:principal-URL/>"),
            (ElementName::Multistatus, b"<D:multistatus/>"),
        ];
        for (name, wanted) in cases {
            let body = write_document(|writer| {
                writer.open(ElementName::Propfind)?;
                writer.empty(name)
            })
            .unwrap();
            assert!(carries(&body, wanted), "{name:?} wrote {body:?}");
        }
    }

    /// A name is resolved before it is written, so every prefix the deployed world sends
    /// converges on the one this crate emits.
    #[test]
    fn any_servers_spelling_of_a_name_writes_back_as_this_crates_own() {
        // The first column is the prefix the named server actually writes and is read by
        // nothing: the lookup takes the resolved URI and the local name. The last row is the
        // one a table keyed on prefix strings gets wrong, and it is a different element.
        let cases: [Spelling; 6] = [
            (b"d:", Namespace::DAV_URI, b"href", Some(b"<D:href/>")),
            (b"", Namespace::DAV_URI, b"href", Some(b"<D:href/>")),
            (
                b"ns0:",
                Namespace::DAV_URI,
                b"response",
                Some(b"<D:response/>"),
            ),
            (
                b"cal:",
                Namespace::CALDAV_URI,
                b"calendar-data",
                Some(b"<C:calendar-data/>"),
            ),
            (
                b"cs:",
                Namespace::CALENDARSERVER_URI,
                b"getctag",
                Some(b"<CS:getctag/>"),
            ),
            (b"D:", b"http://evil.example/not-dav", b"href", None),
        ];
        for (spelled, uri, local, wanted) in cases {
            let found = ElementName::resolve(Namespace::from_uri(uri), local);
            assert_eq!(found.is_some(), wanted.is_some(), "{spelled:?} {local:?}");
            let (Some(name), Some(tag)) = (found, wanted) else {
                continue;
            };
            let body = write_document(|writer| {
                writer.open(ElementName::Multistatus)?;
                writer.empty(name)
            })
            .unwrap();
            assert!(carries(&body, tag), "{spelled:?} {local:?} wrote {body:?}");
        }
    }

    /// The root carries the three declarations the table itself states, and nothing else.
    #[test]
    fn the_root_declares_the_three_namespaces_this_crate_writes_in() {
        let mut composed: Vec<u8> = Vec::new();
        for namespace in [Namespace::Dav, Namespace::CalDav, Namespace::CalendarServer] {
            composed.extend_from_slice(b" xmlns:");
            composed.extend_from_slice(namespace.write_prefix());
            composed.extend_from_slice(b"=\"");
            composed.extend_from_slice(namespace.uri());
            composed.extend_from_slice(b"\"");
        }
        assert_eq!(
            composed, DECLARATIONS,
            "the constant the tables are written against"
        );

        let body = write_document(|writer| writer.open(ElementName::Propname)).unwrap();
        let mut wanted = Vec::from(PRELUDE);
        wanted.extend_from_slice(b"<D:propname");
        wanted.extend_from_slice(DECLARATIONS);
        wanted.extend_from_slice(b"/>");
        assert_eq!(body, wanted);
    }

    /// The client direction: a `calendar-multiget` built the way RFC 4791 section 7.9 writes
    /// one, out of the same writer the server direction below uses.
    #[test]
    fn a_client_builds_a_multiget_and_the_octets_are_the_ones_a_server_reads() {
        let body = write_document(|writer| {
            writer.open(ElementName::CalendarMultiget)?;
            writer.open(ElementName::Prop)?;
            writer.empty(ElementName::Getetag)?;
            writer.empty(ElementName::CalendarData)?;
            writer.close()?;
            writer.element_text(
                ElementName::Href,
                b"/calendars/ann/work/20260105T090000Z-1.ics",
            )?;
            writer.element_text(ElementName::Href, b"/calendars/ann/work/gone.ics")
        })
        .unwrap();
        assert_eq!(body, MULTIGET);
    }

    /// The server direction: one `href` reporting `getetag` at 200 beside `displayname` at 404,
    /// which is what makes a per-property status structural rather than decorative.
    ///
    /// `SabreDAV` writes the `ETag` as `&quot;5f2b8c1e9a04&quot;`; this writer leaves the quote
    /// alone, because a quote in character data needs no escaping and both spellings are the
    /// same three-and-a-bit characters. That difference is exactly why a reader must compare
    /// infosets and not wire strings.
    #[test]
    fn a_server_builds_one_response_whose_properties_disagree_about_their_status() {
        let body = write_document(|writer| {
            writer.open(ElementName::Multistatus)?;
            writer.open(ElementName::Response)?;
            writer.element_text(
                ElementName::Href,
                b"/calendars/ann/work/20260105T090000Z-1.ics",
            )?;
            writer.open(ElementName::Propstat)?;
            writer.open(ElementName::Prop)?;
            writer.element_text(ElementName::Getetag, b"\"5f2b8c1e9a04\"")?;
            writer.close()?;
            writer.element_text(ElementName::Status, b"HTTP/1.1 200 OK")?;
            writer.close()?;
            writer.open(ElementName::Propstat)?;
            writer.open(ElementName::Prop)?;
            writer.empty(ElementName::Displayname)?;
            writer.close()?;
            writer.element_text(ElementName::Status, b"HTTP/1.1 404 Not Found")
            // The propstat, the response and the multistatus are left open on purpose: what
            // closes them is `finish`, which is what makes a truncated body unrepresentable.
        })
        .unwrap();
        assert_eq!(body, MULTISTATUS);
    }

    /// Both `time-range` bounds are independently optional, and an absent one is absent from
    /// the wire rather than written as a placeholder.
    #[test]
    fn a_time_range_writes_the_bounds_it_has_and_no_others() {
        let cases: [Window; 3] = [
            (
                Some(b"20260105T000000Z"),
                None,
                b"<C:time-range start=\"20260105T000000Z\"/>",
            ),
            (
                None,
                Some(b"20260112T000000Z"),
                b"<C:time-range end=\"20260112T000000Z\"/>",
            ),
            (
                Some(b"20260105T000000Z"),
                Some(b"20260112T000000Z"),
                b"<C:time-range start=\"20260105T000000Z\" end=\"20260112T000000Z\"/>",
            ),
        ];
        for (from, until, fragment) in cases {
            let body = write_document(|writer| {
                writer.open(ElementName::CalendarQuery)?;
                writer.open(ElementName::Filter)?;
                writer.open(ElementName::CompFilter)?;
                writer.attribute(b"name", b"VEVENT")?;
                writer.open(ElementName::TimeRange)?;
                if let Some(start) = from {
                    writer.attribute(b"start", start)?;
                }
                match until {
                    Some(end) => writer.attribute(b"end", end),
                    None => Ok(()),
                }
            })
            .unwrap();

            let mut wanted = Vec::from(PRELUDE);
            wanted.extend_from_slice(b"<C:calendar-query");
            wanted.extend_from_slice(DECLARATIONS);
            wanted.extend_from_slice(b"><C:filter><C:comp-filter name=\"VEVENT\">");
            wanted.extend_from_slice(fragment);
            wanted.extend_from_slice(b"</C:comp-filter></C:filter></C:calendar-query>");
            assert_eq!(body, wanted, "{fragment:?}");
        }
    }

    /// The write half of the line-ending resolution: conformant octets that this crate's own
    /// reader hands back unchanged.
    #[test]
    fn a_payload_written_here_is_the_payload_read_back_out_of_it() {
        let body = write_document(|writer| {
            writer.open(ElementName::CalendarData)?;
            writer.text(PAYLOAD)
        })
        .unwrap();

        // No departure from XML 1.0 is needed on this side. Nothing carries a raw `CR`, no
        // `CDATA` section is opened, and the `]]>` a `DESCRIPTION` may hold is escaped where it
        // stands rather than being an escaping bug waiting for one.
        assert!(!body.contains(&b'\r'));
        assert!(!carries(&body, b"<![CDATA["));
        assert!(carries(&body, b"&#13;\n"));
        assert!(carries(&body, b"]]&gt; &amp; a &lt; inside"));

        let open = b"<C:calendar-data>".len();
        let start = PRELUDE
            .len()
            .saturating_add(open)
            .saturating_add(DECLARATIONS.len());
        let end = body.len().saturating_sub(b"</C:calendar-data>".len());
        let span = body.get(start..end).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let read = decode_text(span, TextMode::Verbatim, 0, &mut meter, &mut sink).unwrap();
        assert_eq!(read.run.as_bytes(), PAYLOAD);
    }

    /// A property name outside the vocabulary is written with its own declaration, so no
    /// prefix of this crate's is bent to carry it.
    #[test]
    fn a_name_outside_the_vocabulary_declares_its_own_namespace_on_itself() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let vendor =
            ExtensionName::new(b"http://apple.com/ns/ical/", b"calendar-color", &mut meter)
                .unwrap();
        let refused =
            ExtensionName::new(b"http://x.invalid/ns", b"not a name", &mut meter).unwrap();

        let body = write_document(|writer| {
            writer.open(ElementName::Propfind)?;
            writer.open(ElementName::Prop)?;
            writer.empty(ElementName::Getetag)?;
            writer.empty_extension(&vendor)
        })
        .unwrap();
        assert!(carries(
            &body,
            b"<X:calendar-color xmlns:X=\"http://apple.com/ns/ical/\"/>"
        ));

        let stopped = write_document(|writer| {
            writer.open(ElementName::Propfind)?;
            writer.empty_extension(&refused)
        });
        assert_eq!(stopped, Err(DavError::Syntax(SyntaxError::Malformed)));
    }

    /// A document a peer refuses is not one this writer can be made to produce.
    #[test]
    fn the_documents_a_peer_would_refuse_are_the_ones_that_cannot_be_written() {
        // Closing more than was opened.
        let unopened = write_document(|writer| {
            writer.open(ElementName::Propname)?;
            writer.close()?;
            writer.close()
        });
        assert_eq!(unopened, Err(DavError::Syntax(SyntaxError::Malformed)));

        // A second root element.
        let twice = write_document(|writer| {
            writer.empty(ElementName::Propname)?;
            writer.open(ElementName::Multistatus)
        });
        assert_eq!(twice, Err(DavError::Syntax(SyntaxError::Malformed)));

        // Character data outside the root.
        let loose = write_document(|writer| writer.text(b"stray"));
        assert_eq!(loose, Err(DavError::Syntax(SyntaxError::Malformed)));

        // The same attribute twice on one element.
        let repeated = write_document(|writer| {
            writer.open(ElementName::CompFilter)?;
            writer.attribute(b"name", b"VEVENT")?;
            writer.attribute(b"name", b"VTODO")
        });
        assert_eq!(
            repeated,
            Err(DavError::Syntax(SyntaxError::DuplicateAttribute))
        );

        // A namespace declaration a caller wrote, and an attribute name that would end the tag.
        for name in [
            b"xmlns:D".as_slice(),
            b"xmlns".as_slice(),
            b"a b".as_slice(),
        ] {
            let injected = write_document(|writer| {
                writer.open(ElementName::CompFilter)?;
                writer.attribute(name, b"http://evil.example/not-dav")
            });
            assert_eq!(injected, Err(DavError::Syntax(SyntaxError::Malformed)));
        }

        // An attribute after the start tag has been sealed by the element inside it.
        let late = write_document(|writer| {
            writer.open(ElementName::CompFilter)?;
            writer.empty(ElementName::TimeRange)?;
            writer.attribute(b"name", b"VEVENT")
        });
        assert_eq!(late, Err(DavError::Syntax(SyntaxError::Malformed)));
    }

    /// Every octet emitted is charged, and the two ways of running out are told apart.
    #[test]
    fn what_reaches_the_sink_is_what_the_ledger_was_charged_for() {
        let mut body: Vec<u8> = Vec::new();
        let mut meter = Meter::new(Limits::DEFAULT);
        {
            let mut writer = XmlWriter::new(&mut body, &mut meter);
            writer.open(ElementName::Multistatus).unwrap();
            // Escaped content costs more octets than it was handed, which is the amount a
            // charge over the input's length would have missed.
            writer
                .element_text(ElementName::Displayname, b"Ann & <Work>\r\n")
                .unwrap();
            writer.finish().unwrap();
        }
        assert!(carries(&body, b"Ann &amp; &lt;Work&gt;&#13;\n"));
        assert_eq!(meter.spent(), u64::try_from(body.len()).unwrap());
    }

    /// A caller's buffer running out and a caller's budget running out are different answers.
    #[test]
    fn a_full_sink_and_a_spent_budget_are_reported_apart() {
        let mut buffer = [0_u8; 16];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut room = SliceSink::new(&mut buffer);
        let filled = {
            let mut writer = XmlWriter::new(&mut room, &mut meter);
            writer.open(ElementName::Multistatus)
        };
        assert_eq!(filled, Err(DavError::Output(SinkFull)));

        let mut body: Vec<u8> = Vec::new();
        let mut tight = Meter::with_budget(Limits::DEFAULT, 8);
        let spent = {
            let mut writer = XmlWriter::new(&mut body, &mut tight);
            writer.open(ElementName::Multistatus)
        };
        assert_eq!(spent, Err(DavError::Limit(LimitExceeded::Budget)));
        assert!(body.is_empty(), "a refused charge writes nothing");
    }

    /// Nesting stops where the caller said it stops, on the way out as on the way in.
    #[test]
    fn a_document_is_no_deeper_than_the_caller_admits() {
        let limits = Limits::DEFAULT.with_max_xml_depth(3);
        let mut body: Vec<u8> = Vec::new();
        let mut meter = Meter::new(limits);
        let refused = {
            let mut writer = XmlWriter::new(&mut body, &mut meter);
            writer.open(ElementName::CalendarQuery).unwrap();
            writer.open(ElementName::Filter).unwrap();
            writer.open(ElementName::CompFilter).unwrap();
            assert_eq!(writer.depth(), 3);
            writer.open(ElementName::CompFilter)
        };
        assert_eq!(refused, Err(DavError::Limit(LimitExceeded::Depth)));
    }

    /// An element this build cannot honor is refused rather than written into a request that
    /// could not then be read back.
    #[test]
    fn a_build_without_the_feature_will_not_write_the_report_it_cannot_read() {
        if crate::internal::dav::SYNC_COLLECTION_ENABLED {
            return;
        }
        let refused = write_document(|writer| writer.open(ElementName::SyncCollection));
        assert_eq!(
            refused,
            Err(DavError::Unsupported(ElementName::SyncCollection))
        );
    }
}
