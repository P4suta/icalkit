// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The tokenizer: one contiguous body in, resolved events out, and a stated list of refusals.
//!
//! [`XmlReader`] is the only implementation of [`XmlPull`] this crate ships, and it is the
//! attack surface `SECURITY.md` names. Everything below is written from that posture: a
//! hand-rolled reader that is merely *incomplete* is safer than one that is accidentally
//! *complete*, so a construct no `DAV:` or CalDAV body needs is refused under its own name
//! rather than guessed at, dropped, or passed through.
//!
//! # It iterates, and the stack it uses is one it owns
//!
//! There is no recursion anywhere in this file. Open elements live in a `Vec` this type holds,
//! so a body nested a million deep costs a million entries of a heap vector — each one charged
//! through [`ical_core::Meter::try_enter_element`] and refused at
//! [`ical_core::Limits::max_xml_depth`] — and never a frame of the caller's stack. That is the
//! claim `docs/adr/0004` (DP-14) made in prose and nothing backed until this file existed.
//!
//! # What it refuses
//!
//! - Any `DOCTYPE`, in any casing, anywhere: [`SyntaxError::Doctype`]. Not a bounded expansion
//!   budget. The billion laughs, an internal or external parameter entity, and a general entity
//!   pointing at `/etc/passwd` all need a `DOCTYPE` to declare what they expand, so refusing the
//!   declaration closes the class instead of racing it.
//! - Any processing instruction beyond the XML declaration: [`SyntaxError::ProcessingInstruction`].
//! - An encoding declaration naming anything but UTF-8: [`SyntaxError::Encoding`].
//! - A prefix with no declaration in scope: [`SyntaxError::UnboundPrefix`].
//! - An end tag naming another element: [`SyntaxError::MismatchedTag`].
//! - One attribute name twice on one element, *after* namespace resolution, so that two prefixes
//!   bound to one URI are the collision XML Namespaces 1.0 section 6.3 says they are:
//!   [`SyntaxError::DuplicateAttribute`].
//! - A tag, a comment, a `CDATA` section or an attribute value the body ends inside:
//!   [`SyntaxError::Truncated`].
//! - A character XML 1.0 section 2.2's `Char` production excludes, or octets that are not the
//!   UTF-8 section 4.3.3 requires of the document entity: [`SyntaxError::ForbiddenCharacter`].
//!   In an element name, in an attribute value, and in character data alike, and whether the
//!   character is spelled `&#0;` or written as the octet — one spelling refused under its own
//!   name while the other went by is how a caller ends up holding a run that is not text. The
//!   two exceptions are stated rather than accidental: the elements
//!   [`ElementName::preserves_line_endings`] names, where this reader is already not a
//!   conformant processor, and `DAV:href`, whose value `value.rs` deliberately models as
//!   octets because a store may hold a path that is not UTF-8.
//! - Everything else that is not the XML this crate reads — a `<` inside an attribute value, a
//!   second root element, a name carrying two colons, `xmlns:p=""`, which XML Namespaces 1.0
//!   forbids while permitting `xmlns=""`: [`SyntaxError::Malformed`].
//!
//! Entity references and numeric character references are [`decode_text`]'s to refuse, and it
//! does: nothing outside the five XML 1.0 predefines resolves, because with no `DOCTYPE`
//! accepted nothing can ever have been declared.
//!
//! # Prefixes are the document's, namespaces are the vocabulary's
//!
//! `DAV:` may arrive as `D:`, `d:`, `ns0:` or as a default declaration with no prefix at all,
//! and the three fixtures under `tests/fixtures/` are three real servers doing three of those.
//! So a prefix is resolved through the scoped binding stack this reader maintains and is then
//! thrown away: every event carries a [`QName`] of a *namespace* and a local name, and the row
//! of the closed vocabulary comes from [`ElementName::resolve`]. `xml` is bound to its reserved
//! URI without a declaration, as XML Namespaces 1.0 section 3 requires, because RFC 4918 bodies
//! carry `xml:lang`.
//!
//! # Two things this reader hands on rather than deciding
//!
//! **Character data is emitted as it lies, whitespace included.** Indentation between elements
//! is character data, and a tokenizer that dropped it would be deciding significance on its
//! caller's behalf; readers above it ignore a run they did not ask for. Every span goes to
//! [`decode_text`] under the mode [`TextMode::of`] derives from the enclosing element and the
//! caller's [`crate::TextPolicy`] — this file never chooses that mode, which is what keeps the
//! `calendar-data` carve-out one element wide.
//!
//! **An attribute value is the value XML 1.0 section 3.3.3 defines, not the octets between its
//! quotes.** References are resolved and every literal tab, line feed and carriage return is
//! replaced by one space, before [`XmlPull::attribute`] answers. This file used to hand back
//! the raw span on the reasoning that the attributes this crate's vocabulary defines — a
//! `time-range`'s `start` and `end`, a `comp-filter`'s `name`, a `text-match`'s `collation` —
//! are `US-ASCII` values with nothing to escape. That is an assumption about a cooperative
//! peer, and the peer is the attacker: a `comp-filter name="VE&#78;T"` selected `VE&#78;T` here
//! and `VENT` in every conformant processor, so two implementations disagreed about which
//! components a hostile `calendar-query` matches. The normalized value borrows the reader
//! rather than the body, because it appears nowhere in the body contiguously.
//!
//! # The refusal a rejecting policy owes a foreign element
//!
//! Under [`UnknownPolicy::Reject`], [`XmlPull::skip_element`] must refuse a foreign element, and
//! neither [`DavError::Unsupported`] nor [`DavError::Unexpected`] can say so: both name an
//! [`ElementName`], which a foreign element by definition does not have. [`DavError::Foreign`]
//! is that refusal, and it carries nothing — the element's own spelling lives in the body, which
//! the error outlives. A caller that wants the name has it on the [`XmlEvent::Start`] it was
//! handed one call earlier.

use alloc::vec::Vec;

use ical_core::{DiagnosticCode, LimitExceeded, Meter, Severity};

use crate::codec::{XmlEvent, XmlPull};
use crate::element::{ElementName, Namespace, QName};
use crate::failure::{DavError, SyntaxError};
use crate::policy::{DecodeContext, UnknownPolicy};
use crate::text::{TextMode, check_chars, decode_text, normalize_attribute};
// The lexical layer and the namespace binding stack are XML's rather than CalDAV's, so they live
// in the private module `gates/xml-layering` compiles alone (docs/adr/0012). What stays in this
// file is the state machine, which is stated over `ElementName` and `Namespace` and is therefore
// the half of the tokenizer that layer may not name.
use crate::xml::bind::PrefixStack;
use crate::xml::scan::{
    BYTE_ORDER_MARK, CDATA_CLOSE, CDATA_OPEN, COMMENT_CLOSE, COMMENT_OPEN, DECLARATION_OPEN,
    NO_NAMESPACE, check_encoding, declared_prefix, find, is_attribute_name_end, is_name_end,
    is_name_forbidden, is_space, space_end, split_name,
};

/// The URI XML Namespaces 1.0 section 3 binds the `xml` prefix to, declared or not.
///
/// Re-exported from the private XML layer, which is where the constant now lives: RFC 4918
/// section 14 writes `xml:lang` on `DAV:displayname` and on `responsedescription`, and reading
/// that name back is this crate's business rather than the layer's.
pub(crate) use crate::xml::scan::XML_URI;

/// Where in the document the reader sits, which decides what is legal next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Before the root element: a declaration, comments, and nothing else.
    Prolog,
    /// Inside the root element.
    Content,
    /// After the root element closed: comments and whitespace, and then the end.
    Epilog,
}

/// One element the reader is inside.
#[derive(Clone, Copy, Debug)]
struct Open<'a> {
    /// Its resolved name, kept so the end tag can be compared against it.
    name: QName<'a>,
    /// The name exactly as the start tag spelled it, prefix included.
    ///
    /// Kept beside the resolved name because XML 1.0 section 3 matches an end tag against the
    /// *spelling* of its start tag, not against what that spelling resolved to. Two prefixes
    /// bound to one URI make `<d:x …></e:x>` a document whose tags resolve alike and that no
    /// conformant processor accepts, and this reader does not accept it either.
    spelled: &'a [u8],
    /// The row of the closed vocabulary it landed on, if any.
    known: Option<ElementName>,
    /// How many prefix bindings this element declared, and therefore takes away when it closes.
    bindings: u16,
}

/// One attribute as it lies in the body, before any prefix has been resolved.
#[derive(Clone, Copy, Debug)]
struct RawAttribute<'a> {
    /// The name, prefix included.
    name: &'a [u8],
    /// The value, quotes excluded and nothing resolved.
    value: &'a [u8],
}

/// One attribute of the element that has just started, resolved and normalized.
///
/// The value is a range into [`XmlReader::values`] rather than a slice of the body, because
/// XML 1.0 section 3.3.3's normalization resolves references and replaces literal whitespace —
/// so the value an attribute *has* appears nowhere contiguously in the octets it arrived in.
#[derive(Clone, Copy, Debug)]
struct Attribute<'a> {
    /// The resolved name: a namespace and a local name, never a prefix.
    name: QName<'a>,
    /// Where the normalized value starts.
    from: usize,
    /// Where it ends.
    to: usize,
}

/// What one turn of the state machine produced.
enum Step<'a> {
    /// The document is finished.
    Done,
    /// Something with no event of its own was consumed — a comment, say.
    Skipped,
    /// An event for the caller.
    Event(XmlEvent<'a>),
}

/// A pull tokenizer over one contiguous body.
///
/// The lifetime is the body's, so a `calendar-data` payload reaches `ical-core` as
/// [`crate::TextRun::Wire`] — a borrowed slice, no copy — which is the property
/// `docs/adr/0001`'s round trip needs on the way through the XML envelope.
///
/// Every reading door takes the caller's [`DecodeContext`], and this type holds no policy and
/// no ledger of its own: the bounds that apply to a body are the ones its reader passes, one
/// call at a time, so many bodies under one [`Meter`] are bounded in aggregate.
#[derive(Debug)]
pub struct XmlReader<'a> {
    /// The whole body.
    body: &'a [u8],
    /// How far into it the reader sits.
    at: usize,
    /// Which part of the document is being read.
    stage: Stage,
    /// The elements currently open, outermost first. The explicit stack DP-14 promised.
    open: Vec<Open<'a>>,
    /// Every live binding. The layer holds the stack; this file only asks it questions.
    bindings: PrefixStack<'a>,
    /// The attributes of the element that has just started, as they lie in the body.
    raw: Vec<RawAttribute<'a>>,
    /// The same attributes with their prefixes resolved, sorted by name.
    resolved: Vec<Attribute<'a>>,
    /// The normalized values those attributes point into, one element's worth at a time.
    values: Vec<u8>,
    /// Whether an empty-element tag still owes its `End`.
    pending_end: bool,
}

impl<'a> XmlReader<'a> {
    /// A reader over one whole body.
    ///
    /// Contiguous by construction: a chunked transport gives up the borrow that makes
    /// [`crate::TextRun::Wire`] possible, and this type would rather say so in its signature
    /// than copy on the caller's behalf.
    ///
    /// Nothing is read here and nothing is charged here. The body's own length is checked
    /// against [`ical_core::Limits::max_response_bytes`] on the first call that carries a
    /// policy, because a constructor with no [`DecodeContext`] has none to check against.
    #[must_use]
    pub const fn new(body: &'a [u8]) -> Self {
        Self {
            body,
            at: 0,
            stage: Stage::Prolog,
            open: Vec::new(),
            bindings: PrefixStack::new(),
            raw: Vec::new(),
            resolved: Vec::new(),
            values: Vec::new(),
            pending_end: false,
        }
    }

    /// The octets from the cursor to the end of the body.
    fn rest(&self) -> &'a [u8] {
        self.body.get(self.at..).unwrap_or(&[])
    }

    /// Whether the body continues with `marker` at the cursor.
    fn starts_with(&self, marker: &[u8]) -> bool {
        self.rest().starts_with(marker)
    }

    /// Move the cursor past any whitespace.
    fn eat_space(&mut self) {
        self.at = space_end(self.body, self.at);
    }

    /// Move the cursor past any whitespace, charging the octets it walked over.
    ///
    /// Whitespace outside the root element is octets a peer chose and a reader scanned, which
    /// is what `docs/adr/0010` means by work. Free scanning is free scanning whether the
    /// octets are indentation or a comment.
    fn eat_space_charged(&mut self, meter: &mut Meter) -> Result<(), DavError> {
        let from = self.at;
        self.eat_space();
        charge_span(meter, from, self.at)
    }

    /// How deep the reader sits, with the root element at one.
    fn level(&self) -> u16 {
        // The depth is bounded by `max_xml_depth`, which is itself a `u16`, so the
        // saturation below is unreachable rather than lossy — and it is written anyway,
        // because a cast that cannot be wrong today is a cast nobody checks tomorrow.
        u16::try_from(self.open.len()).unwrap_or(u16::MAX)
    }

    /// Where the cursor sits, as a diagnostic location.
    fn position(&self) -> u64 {
        u64::try_from(self.at).unwrap_or(u64::MAX)
    }

    /// The next event, or `None` at the end of the document.
    fn pull(&mut self, context: &mut DecodeContext<'_>) -> Result<Option<XmlEvent<'a>>, DavError> {
        if self.stage == Stage::Prolog {
            self.read_prolog(context)?;
        }
        if self.pending_end {
            self.pending_end = false;
            if let Some(event) = self.end_event(context.meter) {
                return Ok(Some(event));
            }
        }
        loop {
            match self.step(context)? {
                Step::Done => return Ok(None),
                Step::Skipped => {},
                Step::Event(event) => return Ok(Some(event)),
            }
        }
    }

    /// One turn of the machine: look at the cursor, decide what is there, consume it.
    fn step(&mut self, context: &mut DecodeContext<'_>) -> Result<Step<'a>, DavError> {
        if self.stage == Stage::Epilog {
            return self.step_epilog(context.meter);
        }
        let Some(&byte) = self.body.get(self.at) else {
            // A body that ends with elements still open ended inside them.
            return if self.open.is_empty() {
                Ok(Step::Done)
            } else {
                Err(SyntaxError::Truncated.into())
            };
        };
        if byte != b'<' || self.starts_with(CDATA_OPEN) {
            return self.read_text(context).map(Step::Event);
        }
        if self.starts_with(b"</") {
            return self.read_end_tag(context).map(Step::Event);
        }
        if self.starts_with(COMMENT_OPEN) {
            self.skip_comment(context.meter)?;
            return Ok(Step::Skipped);
        }
        if self.starts_with(b"<!") {
            return Err(self.refuse_bang());
        }
        if self.starts_with(b"<?") {
            return Err(SyntaxError::ProcessingInstruction.into());
        }
        self.read_start_tag(context).map(Step::Event)
    }

    /// After the root element: whitespace and comments, and then nothing.
    fn step_epilog(&mut self, meter: &mut Meter) -> Result<Step<'a>, DavError> {
        loop {
            self.eat_space_charged(meter)?;
            if self.at >= self.body.len() {
                return Ok(Step::Done);
            }
            if self.starts_with(COMMENT_OPEN) {
                self.skip_comment(meter)?;
                continue;
            }
            // A second root element, or text outside the one there was. Either way the octets
            // are not one XML document, and guessing which one was meant is not this reader's.
            return Err(if self.starts_with(b"<?") {
                SyntaxError::ProcessingInstruction.into()
            } else {
                SyntaxError::Malformed.into()
            });
        }
    }

    /// Consume everything before the root element's `<`, refusing what this crate does not read.
    fn read_prolog(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
        let length = u64::try_from(self.body.len()).unwrap_or(u64::MAX);
        if length > context.limits.max_response_bytes() {
            return Err(DavError::Limit(LimitExceeded::Budget));
        }
        if self.starts_with(BYTE_ORDER_MARK) {
            self.at = BYTE_ORDER_MARK.len();
            charge_span(context.meter, 0, self.at)?;
        }
        self.eat_space_charged(context.meter)?;
        if self.declaration_here() {
            let opened = self.at;
            self.read_declaration()?;
            charge_span(context.meter, opened, self.at)?;
        }
        loop {
            self.eat_space_charged(context.meter)?;
            if !self.starts_with(COMMENT_OPEN) {
                break;
            }
            self.skip_comment(context.meter)?;
        }
        self.check_root_follows()?;
        self.stage = Stage::Content;
        Ok(())
    }

    /// Whether an XML declaration begins at the cursor.
    ///
    /// The whitespace after `<?xml` is load-bearing: `<?xml-stylesheet ... ?>` also starts with
    /// those five octets and is a processing instruction, which this crate refuses.
    fn declaration_here(&self) -> bool {
        self.starts_with(DECLARATION_OPEN)
            && self
                .body
                .get(self.at.saturating_add(DECLARATION_OPEN.len()))
                .is_some_and(|byte| is_space(*byte))
    }

    /// Read the XML declaration, which this crate reads for its encoding and nothing else.
    fn read_declaration(&mut self) -> Result<(), DavError> {
        let rest = self.rest();
        let end = find(rest, b"?>").ok_or(SyntaxError::Truncated)?;
        let inside = rest
            .get(DECLARATION_OPEN.len()..end)
            .ok_or(SyntaxError::Malformed)?;
        check_encoding(inside)?;
        self.at = self.at.saturating_add(end).saturating_add(2);
        Ok(())
    }

    /// Refuse the `<!` construct at the cursor, naming a `DOCTYPE` as one wherever it can.
    ///
    /// The casing is compared loosely only so that the refusal *names* what was attempted; a
    /// lowercase `<!doctype` is refused either way, since every other `<!` here is malformed.
    fn refuse_bang(&self) -> DavError {
        let head = self.rest().get(..b"<!DOCTYPE".len()).unwrap_or(&[]);
        if head.eq_ignore_ascii_case(b"<!DOCTYPE") {
            SyntaxError::Doctype.into()
        } else {
            SyntaxError::Malformed.into()
        }
    }

    /// What must follow the prolog is the root element and nothing else.
    fn check_root_follows(&self) -> Result<(), DavError> {
        match self.body.get(self.at) {
            None => Err(SyntaxError::Truncated.into()),
            Some(b'<') if self.starts_with(b"<!") => Err(self.refuse_bang()),
            Some(b'<') if self.starts_with(b"<?") => Err(SyntaxError::ProcessingInstruction.into()),
            Some(b'<') => Ok(()),
            Some(_) => Err(SyntaxError::Malformed.into()),
        }
    }

    /// Consume a comment, charging its octets, and carry no event.
    ///
    /// The charge is the whole comment including its delimiters. A comment is markup this
    /// reader scans octet by octet and hands nobody, and charging nothing for it made the
    /// aggregate ledger this type documents — "many bodies under one `Meter` are bounded in
    /// aggregate" — false for exactly the shape a peer would choose: eight mebibytes refused
    /// as character data cost nothing at all as `<!-- ... -->`, so a peer bought unmetered
    /// scanning at `max_response_bytes` a request, forever. The scan happens either way; what
    /// changes is that the ledger now sees it.
    fn skip_comment(&mut self, meter: &mut Meter) -> Result<(), DavError> {
        let opened = self.at;
        let from = self.at.saturating_add(COMMENT_OPEN.len());
        let inside = self.body.get(from..).ok_or(SyntaxError::Truncated)?;
        let end = find(inside, COMMENT_CLOSE).ok_or(SyntaxError::Truncated)?;
        self.at = from
            .saturating_add(end)
            .saturating_add(COMMENT_CLOSE.len())
            .min(self.body.len());
        charge_span(meter, opened, self.at)
    }

    /// Read one run of character data, `CDATA` sections and references included.
    fn read_text(&mut self, context: &mut DecodeContext<'_>) -> Result<XmlEvent<'a>, DavError> {
        let from = self.at;
        let end = self.scan_text()?;
        let span = self.body.get(from..end).ok_or(SyntaxError::Malformed)?;
        // The mode is derived from the element the data sits in and the caller's policy. This
        // file never picks one, which is what stops the `calendar-data` carve-out spreading.
        let known = self.open.last().and_then(|open| open.known);
        let mode = TextMode::of(known, context.text);
        let offset = u64::try_from(from).unwrap_or(u64::MAX);
        let decoded = decode_text(span, mode, offset, context.meter, context.sink)?;
        self.at = end;
        Ok(XmlEvent::Text(decoded))
    }

    /// Find where the character data at the cursor ends.
    ///
    /// A `CDATA` section does not end it: `a<![CDATA[b]]>c` is one run of three octets, and
    /// handing it over as one span is what lets [`decode_text`] answer with the octets rather
    /// than with the markup around them.
    fn scan_text(&self) -> Result<usize, DavError> {
        let mut at = self.at;
        loop {
            let rest = self.body.get(at..).unwrap_or(&[]);
            // Character data running to the end of the body is an element that never closed.
            let mark = rest
                .iter()
                .position(|byte| *byte == b'<')
                .ok_or(SyntaxError::Truncated)?;
            let found = at.saturating_add(mark);
            let opens_cdata = self
                .body
                .get(found..)
                .is_some_and(|tail| tail.starts_with(CDATA_OPEN));
            if !opens_cdata {
                return Ok(found);
            }
            let inside = found.saturating_add(CDATA_OPEN.len());
            let tail = self.body.get(inside..).unwrap_or(&[]);
            let end = find(tail, CDATA_CLOSE).ok_or(SyntaxError::Truncated)?;
            at = inside
                .saturating_add(end)
                .saturating_add(CDATA_CLOSE.len())
                .min(self.body.len());
        }
    }

    /// Read a start tag, bind what it declares, and open it.
    fn read_start_tag(
        &mut self,
        context: &mut DecodeContext<'_>,
    ) -> Result<XmlEvent<'a>, DavError> {
        let from = self.at;
        let (spelled, after) = self.scan_name(from.saturating_add(1), is_name_end)?;
        let (end, empty) = self.scan_attributes(after, context.meter)?;
        charge_span(context.meter, from, end)?;
        // The declarations on this element are in scope for this element, so they are bound
        // before its own name is resolved: `<d:multistatus xmlns:d="DAV:">` binds `d` for the
        // tag that carries it, which is how every server in `tests/fixtures/` writes its root.
        let declared = self.bind_declarations(context.meter)?;
        self.resolve_attributes(context.meter)?;
        let name = self.resolve_name(spelled)?;
        let known = name.known();
        context.meter.try_charge_element()?;
        context.meter.try_enter_element()?;
        self.open
            .try_reserve(1)
            .map_err(|_| LimitExceeded::Budget)?;
        self.open.push(Open {
            name,
            spelled,
            known,
            bindings: declared,
        });
        self.at = end;
        self.pending_end = empty;
        Ok(XmlEvent::Start {
            name,
            known,
            depth: self.level(),
        })
    }

    /// Read an end tag and close what it names, or refuse it for naming something else.
    fn read_end_tag(&mut self, context: &mut DecodeContext<'_>) -> Result<XmlEvent<'a>, DavError> {
        let from = self.at;
        let (spelled, after) = self.scan_name(from.saturating_add(2), is_name_end)?;
        let at = space_end(self.body, after);
        let end = match self.body.get(at) {
            Some(b'>') => at.saturating_add(1),
            Some(_) => return Err(SyntaxError::Malformed.into()),
            None => return Err(SyntaxError::Truncated.into()),
        };
        // Compared as spelled, which is what XML 1.0 section 3 requires and is stricter than
        // comparing what the two names resolve to; the resolution is then implied, because an
        // end tag sits inside the scope its start tag opened.
        let open = self
            .open
            .last()
            .copied()
            .ok_or(SyntaxError::MismatchedTag)?;
        if open.spelled != spelled {
            return Err(SyntaxError::MismatchedTag.into());
        }
        charge_span(context.meter, from, end)?;
        self.at = end;
        // The element the tag named is the one on top of the stack, which was just compared
        // against it, so the event is the one closing that element produces.
        self.end_event(context.meter)
            .ok_or_else(|| DavError::from(SyntaxError::MismatchedTag))
    }

    /// Close the innermost open element and answer the `End` event that closing it produces.
    fn end_event(&mut self, meter: &mut Meter) -> Option<XmlEvent<'a>> {
        let depth = self.level();
        let open = self.close_open(meter)?;
        Some(XmlEvent::End {
            name: open.name,
            known: open.known,
            depth,
        })
    }

    /// Close the innermost open element, releasing every binding it declared.
    fn close_open(&mut self, meter: &mut Meter) -> Option<Open<'a>> {
        let open = self.open.pop()?;
        self.bindings.unbind(open.bindings, meter);
        meter.leave_element();
        if self.open.is_empty() {
            self.stage = Stage::Epilog;
        }
        Some(open)
    }

    /// Read a name at `from`, stopping at the first octet `ends` accepts.
    fn scan_name(&self, from: usize, ends: fn(u8) -> bool) -> Result<(&'a [u8], usize), DavError> {
        let rest = self.body.get(from..).ok_or(SyntaxError::Truncated)?;
        let length = rest
            .iter()
            .position(|byte| ends(*byte))
            .ok_or(SyntaxError::Truncated)?;
        let name = rest.get(..length).ok_or(SyntaxError::Malformed)?;
        if name.is_empty() || name.iter().any(|byte| is_name_forbidden(*byte)) {
            return Err(SyntaxError::Malformed.into());
        }
        // A name is characters too. Without this a `NUL` or an octet sequence that is not
        // UTF-8 could sit inside an element or attribute name, where `is_name_forbidden` only
        // ever looked for the five octets that would let a name smuggle markup past the scan.
        check_chars(name)?;
        Ok((name, from.saturating_add(length)))
    }

    /// Read every attribute of the tag whose name ended at `from`.
    ///
    /// Answers where the tag ends and whether it closed itself. The attributes are kept as they
    /// lie in the body; resolving them needs the declarations among them to be bound first.
    fn scan_attributes(
        &mut self,
        from: usize,
        meter: &mut Meter,
    ) -> Result<(usize, bool), DavError> {
        self.raw.clear();
        let mut at = from;
        loop {
            at = space_end(self.body, at);
            match self.body.get(at).copied() {
                None => return Err(SyntaxError::Truncated.into()),
                Some(b'>') => return Ok((at.saturating_add(1), false)),
                Some(b'/') => {
                    return if self.body.get(at.saturating_add(1)) == Some(&b'>') {
                        Ok((at.saturating_add(2), true))
                    } else {
                        Err(SyntaxError::Malformed.into())
                    };
                },
                Some(_) => at = self.scan_attribute(at, meter)?,
            }
        }
    }

    /// Read one `name="value"` pair and answer where the octet after it is.
    fn scan_attribute(&mut self, from: usize, meter: &mut Meter) -> Result<usize, DavError> {
        let (name, after) = self.scan_name(from, is_attribute_name_end)?;
        let at = space_end(self.body, after);
        if self.body.get(at) != Some(&b'=') {
            return Err(SyntaxError::Malformed.into());
        }
        let at = space_end(self.body, at.saturating_add(1));
        let quote = *self.body.get(at).ok_or(SyntaxError::Truncated)?;
        if quote != b'"' && quote != b'\'' {
            return Err(SyntaxError::Malformed.into());
        }
        let opens = at.saturating_add(1);
        let tail = self.body.get(opens..).ok_or(SyntaxError::Truncated)?;
        let length = tail
            .iter()
            .position(|byte| *byte == quote)
            .ok_or(SyntaxError::Truncated)?;
        let value = tail.get(..length).ok_or(SyntaxError::Malformed)?;
        // XML 1.0 section 3.1 forbids a literal `<` in an attribute value, and a reader that
        // took one would be reading a tag somebody hid inside a value.
        if value.contains(&b'<') {
            return Err(SyntaxError::Malformed.into());
        }
        self.push_raw(RawAttribute { name, value }, meter)?;
        Ok(opens.saturating_add(length).saturating_add(1))
    }

    /// Keep one attribute, charging what holding it costs.
    ///
    /// The charge is the item's own footprint, which is what `Bounded::push` charges for the
    /// same reason. The vector is reused across elements, so the memory is the widest tag's
    /// rather than the sum of every tag's, and this charge is conservative by that much.
    fn push_raw(&mut self, attribute: RawAttribute<'a>, meter: &mut Meter) -> Result<(), DavError> {
        let footprint = u64::try_from(size_of::<RawAttribute<'a>>()).unwrap_or(u64::MAX);
        meter.try_charge_bytes(footprint)?;
        self.raw.try_reserve(1).map_err(|_| LimitExceeded::Budget)?;
        self.raw.push(attribute);
        Ok(())
    }

    /// Bind every `xmlns` declaration the tag carried, and answer how many that was.
    fn bind_declarations(&mut self, meter: &mut Meter) -> Result<u16, DavError> {
        let mut declared: u16 = 0;
        for index in 0..self.raw.len() {
            let Some(attribute) = self.raw.get(index).copied() else {
                break;
            };
            let Some(prefix) = declared_prefix(attribute.name) else {
                continue;
            };
            // XML Namespaces 1.0 section 6.2 permits `xmlns=""`, which returns unprefixed
            // names to no namespace, and forbids `xmlns:p=""` outright. Binding a prefix to
            // nothing is refused rather than treated as either.
            if !prefix.is_empty() && attribute.value.is_empty() {
                return Err(SyntaxError::Malformed.into());
            }
            if self.bindings.declared_here(declared, prefix) {
                return Err(SyntaxError::DuplicateAttribute.into());
            }
            self.bindings.bind(prefix, attribute.value, meter)?;
            declared = declared.saturating_add(1);
        }
        Ok(declared)
    }

    /// Resolve and normalize every attribute that is not a declaration, refusing a repeat.
    fn resolve_attributes(&mut self, meter: &mut Meter) -> Result<(), DavError> {
        self.resolved.clear();
        self.values.clear();
        for index in 0..self.raw.len() {
            let Some(attribute) = self.raw.get(index).copied() else {
                break;
            };
            if declared_prefix(attribute.name).is_some() {
                continue;
            }
            let (prefix, local) = split_name(attribute.name)?;
            // XML Namespaces 1.0 section 6.2: a default declaration never applies to an
            // attribute, so an unprefixed one is in no namespace whatever `xmlns=` says.
            let namespace = if prefix.is_empty() {
                Namespace::Other(NO_NAMESPACE)
            } else {
                self.binding_for(prefix).ok_or(SyntaxError::UnboundPrefix)?
            };
            let footprint = u64::try_from(size_of::<Attribute<'a>>()).unwrap_or(u64::MAX);
            meter.try_charge_bytes(footprint)?;
            let from = self.values.len();
            normalize_attribute(attribute.value, &mut self.values)?;
            let to = self.values.len();
            self.resolved
                .try_reserve(1)
                .map_err(|_| LimitExceeded::Budget)?;
            self.resolved.push(Attribute {
                name: QName::new(namespace, local),
                from,
                to,
            });
        }
        // Sorted so the duplicate check is a walk rather than a comparison of every pair with
        // every other, which is work an attacker would otherwise size. Attribute order is not
        // significant in XML, so nothing above this can tell that it was reordered.
        self.resolved
            .sort_unstable_by(|left, right| sort_key(left.name).cmp(&sort_key(right.name)));
        for pair in self.resolved.windows(2) {
            let [left, right] = pair else { continue };
            if same_name(left.name, right.name) {
                return Err(SyntaxError::DuplicateAttribute.into());
            }
        }
        Ok(())
    }

    /// The normalized value one held attribute carries.
    fn value_of(&self, held: Attribute<'a>) -> Option<&[u8]> {
        self.values.get(held.from..held.to)
    }

    /// Resolve a name as the document spelled it into a namespace and a local name.
    fn resolve_name(&self, spelled: &'a [u8]) -> Result<QName<'a>, DavError> {
        let (prefix, local) = split_name(spelled)?;
        let namespace = if prefix.is_empty() {
            // An unprefixed element takes the default declaration if there is one, and is in
            // no namespace if there is not. Neither is an error.
            self.binding_for(b"")
                .unwrap_or(Namespace::Other(NO_NAMESPACE))
        } else {
            self.binding_for(prefix).ok_or(SyntaxError::UnboundPrefix)?
        };
        Ok(QName::new(namespace, local))
    }

    /// The namespace a prefix is bound to at the cursor, if any.
    ///
    /// Named apart from the trait method it answers so that no call inside this file depends on
    /// inherent methods winning over trait methods of the same name.
    fn binding_for(&self, prefix: &[u8]) -> Option<Namespace<'a>> {
        // The layer answers with the octets the document bound; classifying those into the
        // closed vocabulary is this crate's and not the layer's, which is the seam in one line.
        self.bindings.uri_for(prefix).map(Namespace::from_uri)
    }

    /// Consume the element that has just started, and everything inside it.
    fn skip(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
        let Some(open) = self.open.last().copied() else {
            // Nothing is open, so there is nothing to consume. A caller that reached here
            // called this before any `Start`, and skipping the rest of the body would be a
            // worse answer than doing nothing.
            return Ok(());
        };
        // The policy is asked about a foreign element and about nothing else. Skipping a
        // modeled subtree is how a reader passes over what it has no use for, and a
        // diagnostic about that would report something that did not happen.
        if open.known.is_none() {
            if context.unknown == UnknownPolicy::Reject {
                return Err(DavError::Foreign);
            }
            context.report(
                DiagnosticCode::DavForeignElementSkipped,
                Severity::Note,
                self.position(),
            );
        }
        let target = self.level();
        loop {
            match self.pull(context)? {
                Some(XmlEvent::End { depth, .. }) if depth == target => return Ok(()),
                Some(_) => {},
                None => return Err(SyntaxError::Truncated.into()),
            }
        }
    }
}

impl<'a> XmlPull<'a> for XmlReader<'a> {
    fn next_event(
        &mut self,
        context: &mut DecodeContext<'_>,
    ) -> Result<Option<XmlEvent<'a>>, DavError> {
        self.pull(context)
    }

    fn skip_element(&mut self, context: &mut DecodeContext<'_>) -> Result<(), DavError> {
        self.skip(context)
    }

    fn depth(&self) -> u16 {
        self.level()
    }

    fn offset(&self) -> u64 {
        self.position()
    }

    fn resolve_prefix(&self, prefix: &[u8]) -> Option<Namespace<'a>> {
        self.binding_for(prefix)
    }

    fn attribute(&self, name: QName<'_>) -> Option<&[u8]> {
        let held = self
            .resolved
            .iter()
            .copied()
            .find(|held| same_name(held.name, name))?;
        self.value_of(held)
    }

    fn attribute_count(&self) -> usize {
        self.resolved.len()
    }

    fn attribute_at(&self, index: usize) -> Option<(QName<'a>, &[u8])> {
        let held = self.resolved.get(index).copied()?;
        Some((held.name, self.value_of(held)?))
    }
}

/// Charge the octets between two positions against the caller's ledger.
///
/// Markup is charged here and character data is charged by [`decode_text`], so the body is
/// charged once over rather than twice or not at all.
fn charge_span(meter: &mut Meter, from: usize, to: usize) -> Result<(), DavError> {
    let octets = u64::try_from(to.saturating_sub(from)).unwrap_or(u64::MAX);
    meter.try_charge_bytes(octets)?;
    Ok(())
}

/// Whether two resolved names are the same name.
///
/// Compared over the namespace's URI rather than over the enum, so a name from the table and a
/// name borrowed from a body agree without either lifetime having to become the other.
fn same_name(left: QName<'_>, right: QName<'_>) -> bool {
    left.namespace.is(right.namespace) && left.local_name == right.local_name
}

/// The key a name sorts under, which orders the namespace before the local name.
fn sort_key(name: QName<'_>) -> (&[u8], &[u8]) {
    (name.namespace.uri(), name.local_name)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        Diagnostic, DiagnosticCode, IgnoreDiagnostics, LimitExceeded, Limits, Meter, Severity,
    };

    use super::{XML_URI, XmlReader};
    use crate::codec::{XmlEvent, XmlPull};
    use crate::element::{ElementName, Namespace, QName};
    use crate::failure::{DavError, SyntaxError};
    use crate::policy::{DecodeContext, UnknownPolicy};
    use crate::text::{LineEndings, TextPolicy, write_escaped_text};

    /// The `.ics` all three fixtures carry, byte for byte.
    const PAYLOAD: &[u8] = include_bytes!("../tests/fixtures/calendar-data-payload.ics");

    /// `SabreDAV`: `d:` and `cal:` prefixes, literal `CRLF`, two responses, four statuses.
    const SABREDAV: &[u8] = include_bytes!("../tests/fixtures/sabredav-calendar-multiget.xml");

    /// Radicale: `ns0:` and `ns1:` from `ElementTree`, an apostrophe-quoted declaration.
    const RADICALE: &[u8] = include_bytes!("../tests/fixtures/radicale-calendar-multiget.xml");

    /// Calendar Server: a default `DAV:` declaration, `C:` beside it, `CR` as `&#13;`.
    const CALENDAR_SERVER: &[u8] =
        include_bytes!("../tests/fixtures/calendarserver-calendar-multiget.xml");

    /// One event, as a table writes it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Seen {
        /// An element opened, and the row it landed on.
        Start(Option<ElementName>),
        /// An element closed.
        End(Option<ElementName>),
        /// Character data that is not only the whitespace a server indents with.
        Text(Vec<u8>),
    }

    /// Read a whole body under `limits` and `policy`, keeping what a structural table asserts.
    ///
    /// The whitespace between elements is character data and is emitted; a table about element
    /// structure does not want it, so it is dropped here rather than in the reader.
    fn read(body: &[u8], limits: Limits, policy: TextPolicy) -> Result<Vec<Seen>, DavError> {
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink).with_text(policy);
        let mut reader = XmlReader::new(body);
        let mut seen = Vec::new();
        while let Some(event) = reader.next_event(&mut context)? {
            match event {
                XmlEvent::Start { known, .. } => seen.push(Seen::Start(known)),
                XmlEvent::End { known, .. } => seen.push(Seen::End(known)),
                XmlEvent::Text(text) => {
                    let octets = text.run.as_bytes();
                    if !octets.iter().all(u8::is_ascii_whitespace) {
                        seen.push(Seen::Text(octets.to_vec()));
                    }
                },
            }
        }
        Ok(seen)
    }

    /// The same read under the defaults, which is what a caller saying nothing gets.
    fn read_default(body: &[u8]) -> Result<Vec<Seen>, DavError> {
        read(body, Limits::DEFAULT, TextPolicy::Verbatim)
    }

    /// Every element that opened, in order.
    fn opened(body: &[u8]) -> Vec<Option<ElementName>> {
        read_default(body)
            .unwrap()
            .into_iter()
            .filter_map(|seen| {
                if let Seen::Start(known) = seen {
                    Some(known)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Every run of character data that is not indentation.
    fn spoken(body: &[u8]) -> Vec<Vec<u8>> {
        read_default(body)
            .unwrap()
            .into_iter()
            .filter_map(|seen| {
                if let Seen::Text(octets) = seen {
                    Some(octets)
                } else {
                    None
                }
            })
            .collect()
    }

    /// The `calendar-data` payload of `body`, with the witness the read produced.
    fn payload_of(body: &[u8], policy: TextPolicy) -> (Vec<u8>, LineEndings, bool) {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink).with_text(policy);
        let mut reader = XmlReader::new(body);
        let mut inside = false;
        while let Some(event) = reader.next_event(&mut context).unwrap() {
            match event {
                XmlEvent::Start { known, .. } => {
                    inside = known == Some(ElementName::CalendarData);
                },
                XmlEvent::Text(text) if inside => {
                    return (
                        text.run.as_bytes().to_vec(),
                        text.line_endings,
                        text.run.is_reassembled(),
                    );
                },
                _ => {},
            }
        }
        panic!("the fixture carries no calendar-data element");
    }

    /// A body nested `levels` deep inside one `DAV:` root.
    fn nested(levels: usize) -> Vec<u8> {
        let mut body = Vec::from(&br#"<D:multistatus xmlns:D="DAV:">"#[..]);
        for _ in 0..levels {
            body.extend_from_slice(b"<D:response>");
        }
        for _ in 0..levels {
            body.extend_from_slice(b"</D:response>");
        }
        body.extend_from_slice(b"</D:multistatus>");
        body
    }

    /// One element carrying `count` namespace declarations, which no depth or element count sees.
    fn declaring(count: usize) -> Vec<u8> {
        let mut body = Vec::from(&br#"<D:multistatus xmlns:D="DAV:""#[..]);
        for index in 0..count {
            let high = u8::try_from(index.div_euclid(26)).unwrap();
            let low = u8::try_from(index.rem_euclid(26)).unwrap();
            body.extend_from_slice(b" xmlns:");
            body.push(b'a'.saturating_add(high));
            body.push(b'a'.saturating_add(low));
            body.extend_from_slice(br#"="urn:example:filler""#);
        }
        body.extend_from_slice(b"/>");
        body
    }

    /// Three servers, three prefix habits, one document.
    ///
    /// This is the claim the whole element table rests on: a reader keyed on the literal string
    /// `D:href` reads one of these three and silently skips the other two, against the most
    /// widely deployed CalDAV software there is.
    #[test]
    fn a_prefix_is_the_document_s_choice_and_never_the_vocabulary_s() {
        let cases = [
            ("SabreDAV, d: and cal:", SABREDAV),
            ("Radicale, ns0: and ns1:", RADICALE),
            (
                "Calendar Server, a default DAV: declaration",
                CALENDAR_SERVER,
            ),
        ];
        let expected = [
            Some(ElementName::Multistatus),
            Some(ElementName::Response),
            Some(ElementName::Href),
            Some(ElementName::Propstat),
            Some(ElementName::Prop),
            Some(ElementName::Getetag),
            Some(ElementName::CalendarData),
        ];
        for (shape, body) in cases {
            let starts = opened(body);
            assert_eq!(starts.get(..expected.len()), Some(&expected[..]), "{shape}");
        }
    }

    /// The payload arrives as the server wrote it, through the tokenizer rather than beside it.
    ///
    /// `tests/calendar_data_collision.rs` proves this of the character-data rules with the span
    /// cut out by hand. Here the span is the one this reader found, which is the half that was
    /// prose until this file existed.
    #[test]
    fn the_calendar_data_payload_survives_the_envelope() {
        let cases = [
            ("SabreDAV writes the CRLF literally", SABREDAV, false),
            ("Radicale writes it literally too", RADICALE, false),
            ("Calendar Server writes it as &#13;", CALENDAR_SERVER, true),
        ];
        for (shape, body, copied) in cases {
            let (octets, endings, reassembled) = payload_of(body, TextPolicy::Verbatim);
            assert_eq!(octets, PAYLOAD, "{shape}");
            assert_eq!(endings, LineEndings::Crlf, "{shape}");
            assert!(endings.is_as_sent(), "{shape}");
            assert_eq!(reassembled, copied, "{shape}");
            // The fold RFC 5545 section 3.1 wrote is still a fold.
            assert!(octets.windows(3).any(|at| at == b"\r\n "), "{shape}");
        }
    }

    /// The conformant read is available through the tokenizer, and it still costs the `CR`s.
    #[test]
    fn the_normalized_policy_reaches_the_payload_through_the_reader() {
        let (octets, endings, _) = payload_of(SABREDAV, TextPolicy::Normalized);
        assert!(!octets.contains(&b'\r'));
        assert_eq!(endings, LineEndings::Folded);
        assert!(!endings.is_as_sent());
    }

    /// One `href` reporting two statuses, and another reporting one, out of real octets.
    #[test]
    fn per_property_status_reaches_the_reader_as_two_propstats() {
        let said = spoken(SABREDAV);
        let statuses: Vec<&[u8]> = said
            .iter()
            .map(Vec::as_slice)
            .filter(|run| run.starts_with(b"HTTP/1.1"))
            .collect();
        assert_eq!(
            statuses,
            [
                b"HTTP/1.1 200 OK".as_slice(),
                b"HTTP/1.1 403 Forbidden".as_slice(),
                b"HTTP/1.1 404 Not Found".as_slice(),
            ]
        );
    }

    /// A property at 404 beside one at 200, in the shape RFC 4918 section 14.24 writes.
    #[test]
    fn one_response_carries_a_found_property_and_a_missing_one() {
        let body = br#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:response>
    <D:href>/calendars/ann/work/1.ics</D:href>
    <D:propstat>
      <D:prop><D:getetag>"33441-34321"</D:getetag></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
    <D:propstat>
      <D:prop><C:calendar-data/></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>
"#;
        let starts = opened(body);
        assert_eq!(
            starts,
            [
                Some(ElementName::Multistatus),
                Some(ElementName::Response),
                Some(ElementName::Href),
                Some(ElementName::Propstat),
                Some(ElementName::Prop),
                Some(ElementName::Getetag),
                Some(ElementName::Status),
                Some(ElementName::Propstat),
                Some(ElementName::Prop),
                Some(ElementName::CalendarData),
                Some(ElementName::Status),
            ]
        );
        let said = spoken(body);
        assert_eq!(
            said.first().map(Vec::as_slice),
            Some(b"/calendars/ann/work/1.ics".as_slice())
        );
        assert_eq!(
            said.last().map(Vec::as_slice),
            Some(b"HTTP/1.1 404 Not Found".as_slice())
        );
    }

    /// A `time-range` with one bound absent, which RFC 4791 section 9.9 permits.
    #[test]
    fn a_time_range_bound_that_is_absent_is_absent_rather_than_defaulted() {
        let body = br#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/></D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20060104T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut reader = XmlReader::new(body);
        let mut found = false;
        while let Some(event) = reader.next_event(&mut context).unwrap() {
            let XmlEvent::Start { known, .. } = event else {
                continue;
            };
            if known != Some(ElementName::TimeRange) {
                continue;
            }
            found = true;
            // An unprefixed attribute is in no namespace at all, whatever the element's own
            // default declaration says; XML Namespaces 1.0 section 6.2 is explicit about it.
            let start = QName::new(Namespace::Other(b""), b"start");
            let end = QName::new(Namespace::Other(b""), b"end");
            let caldav = QName::new(Namespace::CalDav, b"start");
            assert_eq!(
                reader.attribute(start),
                Some(b"20060104T000000Z".as_slice())
            );
            assert_eq!(reader.attribute(end), None);
            assert_eq!(reader.attribute(caldav), None);
        }
        assert!(found, "the query carries a time-range");
    }

    /// A body this crate's own writing helpers produced is a body this reader reads back.
    ///
    /// The client direction and the server direction over one payload: the octets are escaped
    /// the way `write_escaped_text` escapes them, with the fixed output prefixes
    /// `Namespace::write_prefix` chose, and what comes back is the `.ics` that went in.
    #[test]
    fn what_this_crate_writes_is_what_this_crate_reads() {
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(br#"<?xml version="1.0" encoding="UTF-8"?>"#);
        body.extend_from_slice(b"<");
        body.extend_from_slice(Namespace::Dav.write_prefix());
        body.extend_from_slice(br#":multistatus xmlns:D="DAV:" "#);
        body.extend_from_slice(br#"xmlns:C="urn:ietf:params:xml:ns:caldav">"#);
        body.extend_from_slice(b"<D:response><D:href>/ann/1.ics</D:href><D:propstat><D:prop>");
        body.extend_from_slice(b"<");
        body.extend_from_slice(Namespace::CalDav.write_prefix());
        body.extend_from_slice(b":calendar-data>");
        write_escaped_text(&mut body, PAYLOAD).unwrap();
        body.extend_from_slice(b"</C:calendar-data></D:prop>");
        body.extend_from_slice(b"<D:status>HTTP/1.1 200 OK</D:status>");
        body.extend_from_slice(b"</D:propstat></D:response></D:multistatus>");

        assert_eq!(
            opened(&body).get(..7),
            Some(
                &[
                    Some(ElementName::Multistatus),
                    Some(ElementName::Response),
                    Some(ElementName::Href),
                    Some(ElementName::Propstat),
                    Some(ElementName::Prop),
                    Some(ElementName::CalendarData),
                    Some(ElementName::Status),
                ][..]
            )
        );
        let (octets, endings, reassembled) = payload_of(&body, TextPolicy::Verbatim);
        assert_eq!(octets, PAYLOAD);
        assert_eq!(endings, LineEndings::Crlf);
        // `CR` is written as `&#13;`, which is markup, so the read reassembles rather than
        // borrows — and recovers every octet, which is the whole point of writing it that way.
        assert!(reassembled);
    }

    /// A prefix rebound mid-document means what the innermost declaration says, and only there.
    #[test]
    fn a_prefix_rebound_mid_document_shadows_and_then_stops_shadowing() {
        // `a:response` is `DAV:response` outside and a CalDAV element with no row inside, and
        // the sibling after it is `DAV:response` again because the inner scope ended.
        let body = br#"<a:multistatus xmlns:a="DAV:">
          <a:response xmlns:a="urn:ietf:params:xml:ns:caldav"/>
          <a:response/>
        </a:multistatus>"#;
        assert_eq!(
            opened(body),
            [
                Some(ElementName::Multistatus),
                None,
                Some(ElementName::Response),
            ]
        );
    }

    /// A default declaration changes what an unprefixed name means, and the table says so.
    #[test]
    fn a_default_declaration_decides_what_an_unprefixed_element_is() {
        // `DAV:prop` and `CALDAV:prop` are different rows on purpose, and this is the body that
        // tells them apart with no prefix written anywhere.
        let body = br#"<multistatus xmlns="DAV:">
          <prop/>
          <prop xmlns="urn:ietf:params:xml:ns:caldav"/>
        </multistatus>"#;
        assert_eq!(
            opened(body),
            [
                Some(ElementName::Multistatus),
                Some(ElementName::Prop),
                Some(ElementName::CalendarDataProp),
            ]
        );
    }

    /// A familiar prefix bound to somewhere else is somewhere else.
    #[test]
    fn a_dav_looking_prefix_bound_elsewhere_is_a_foreign_element() {
        let body = br#"<D:multistatus xmlns:D="http://evil.example/not-dav">
          <D:response/>
        </D:multistatus>"#;
        assert_eq!(opened(body), [None, None]);
    }

    /// The reserved `xml` prefix is bound without a declaration, as RFC 4918 bodies assume.
    #[test]
    fn the_xml_prefix_is_bound_with_no_declaration() {
        let body = br#"<D:displayname xmlns:D="DAV:" xml:lang="en">Work</D:displayname>"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut reader = XmlReader::new(body);
        let event = reader.next_event(&mut context).unwrap();
        assert!(matches!(
            event,
            Some(XmlEvent::Start {
                known: Some(ElementName::Displayname),
                depth: 1,
                ..
            })
        ));
        let lang = QName::new(Namespace::from_uri(XML_URI), b"lang");
        assert_eq!(reader.attribute(lang), Some(b"en".as_slice()));
    }

    /// A `CDATA` section is character data, because real servers emit one.
    #[test]
    fn a_cdata_section_is_read_and_is_not_a_boundary() {
        let body = br#"<D:displayname xmlns:D="DAV:">Ann <![CDATA[& <Bob>]]> too</D:displayname>"#;
        assert_eq!(
            spoken(body).first().map(Vec::as_slice),
            Some(b"Ann & <Bob> too".as_slice())
        );
    }

    /// Entity expansion is refused at the declaration, which closes the class.
    ///
    /// This is `SECURITY.md`'s list. Every one of these attacks needs a `DOCTYPE` to declare
    /// what it expands, so none of them is bounded here — they are all refused at the same
    /// octet, before any expansion budget could be spent racing them.
    #[test]
    fn entity_expansion_is_refused_where_it_would_be_declared() {
        let cases: [(&str, &[u8], SyntaxError); 6] = [
            (
                "a bare DOCTYPE",
                b"<!DOCTYPE multistatus><D:multistatus xmlns:D=\"DAV:\"/>",
                SyntaxError::Doctype,
            ),
            (
                "the billion laughs, which needs a DOCTYPE to declare its entities",
                b"<!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;\">]><lolz/>",
                SyntaxError::Doctype,
            ),
            (
                "an external entity pointing at a local file",
                b"<!DOCTYPE x [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><x>&xxe;</x>",
                SyntaxError::Doctype,
            ),
            (
                "an external parameter entity, which is the same declaration",
                b"<!DOCTYPE x [<!ENTITY % p SYSTEM \"http://evil.example/e\"> %p;]><x/>",
                SyntaxError::Doctype,
            ),
            (
                "a DOCTYPE in a casing that is not the specification's",
                b"<!doctype html><html/>",
                SyntaxError::Doctype,
            ),
            (
                "an entity reference naming something no DOCTYPE could have declared",
                b"<D:href xmlns:D=\"DAV:\">&xxe;</D:href>",
                SyntaxError::UndefinedEntity,
            ),
        ];
        for (shape, body, expected) in cases {
            assert_eq!(
                read_default(body),
                Err(DavError::Syntax(expected)),
                "{shape}"
            );
        }
    }

    /// The constructs this crate declines to implement, refused rather than guessed at.
    #[test]
    fn what_this_reader_does_not_implement_is_refused_under_its_own_name() {
        let cases: [(&str, &[u8], SyntaxError); 5] = [
            (
                "a processing instruction",
                b"<?xml-stylesheet href=\"x.xsl\"?><D:multistatus xmlns:D=\"DAV:\"/>",
                SyntaxError::ProcessingInstruction,
            ),
            (
                "a processing instruction after the root element",
                b"<D:multistatus xmlns:D=\"DAV:\"/><?php echo 1; ?>",
                SyntaxError::ProcessingInstruction,
            ),
            (
                "an encoding this crate does not read",
                b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><D:x xmlns:D=\"DAV:\"/>",
                SyntaxError::Encoding,
            ),
            (
                "a prefix with no declaration in scope",
                b"<D:multistatus><D:response/></D:multistatus>",
                SyntaxError::UnboundPrefix,
            ),
            (
                "an attribute prefix with no declaration in scope",
                b"<D:x xmlns:D=\"DAV:\" q:name=\"VEVENT\"/>",
                SyntaxError::UnboundPrefix,
            ),
        ];
        for (shape, body, expected) in cases {
            assert_eq!(
                read_default(body),
                Err(DavError::Syntax(expected)),
                "{shape}"
            );
        }
    }

    /// Octets that are not one well-formed document, each refused under its own name.
    #[test]
    fn a_body_that_is_not_well_formed_is_refused_rather_than_repaired() {
        let cases: [(&str, &[u8], SyntaxError); 10] = [
            (
                "an end tag naming another element",
                b"<D:multistatus xmlns:D=\"DAV:\"><D:response></D:propstat></D:multistatus>",
                SyntaxError::MismatchedTag,
            ),
            (
                "an end tag whose prefix resolves alike and is spelled otherwise",
                b"<d:x xmlns:d=\"DAV:\" xmlns:e=\"DAV:\">t</e:x>",
                SyntaxError::MismatchedTag,
            ),
            (
                "one attribute name twice",
                b"<C:comp xmlns:C=\"urn:ietf:params:xml:ns:caldav\" name=\"A\" name=\"B\"/>",
                SyntaxError::DuplicateAttribute,
            ),
            (
                "one attribute name twice under two prefixes bound to one URI",
                b"<D:x xmlns:D=\"DAV:\" xmlns:d=\"DAV:\" D:a=\"1\" d:a=\"2\"/>",
                SyntaxError::DuplicateAttribute,
            ),
            (
                "one prefix declared twice on one element",
                b"<D:x xmlns:D=\"DAV:\" xmlns:D=\"urn:ietf:params:xml:ns:caldav\"/>",
                SyntaxError::DuplicateAttribute,
            ),
            (
                "a start tag the body ends inside",
                b"<D:multistatus xmlns:D=\"DAV:\"><D:response",
                SyntaxError::Truncated,
            ),
            (
                "a comment the body ends inside",
                b"<D:x xmlns:D=\"DAV:\"><!-- and then</D:x>",
                SyntaxError::Truncated,
            ),
            (
                "a CDATA section the body ends inside",
                b"<D:x xmlns:D=\"DAV:\"><![CDATA[BEGIN:VCALENDAR",
                SyntaxError::Truncated,
            ),
            (
                "a tag hidden inside an attribute value",
                b"<D:x xmlns:D=\"DAV:\" name=\"<D:y/>\"/>",
                SyntaxError::Malformed,
            ),
            (
                "undeclaring a prefix, which XML Namespaces 1.0 permits only for the default",
                b"<D:x xmlns:D=\"DAV:\" xmlns:p=\"\"><p:y/></D:x>",
                SyntaxError::Malformed,
            ),
        ];
        for (shape, body, expected) in cases {
            assert_eq!(
                read_default(body),
                Err(DavError::Syntax(expected)),
                "{shape}"
            );
        }
    }

    /// A second root element, and text where no element is open, are not one document.
    #[test]
    fn there_is_one_root_element_and_nothing_beside_it() {
        let two = br#"<D:x xmlns:D="DAV:"/><D:y xmlns:D="DAV:"/>"#;
        assert_eq!(
            read_default(two),
            Err(DavError::Syntax(SyntaxError::Malformed))
        );
        let loose = b"not xml at all";
        assert_eq!(
            read_default(loose),
            Err(DavError::Syntax(SyntaxError::Malformed))
        );
        assert_eq!(
            read_default(b""),
            Err(DavError::Syntax(SyntaxError::Truncated))
        );
    }

    /// Every dimension a body's size does not reach, refused under the name a caller can raise.
    #[test]
    fn the_bounds_a_body_s_length_never_reaches_are_charged() {
        let deep = nested(8);
        let wide = declaring(8);
        let cases: [(&str, Vec<u8>, Limits, LimitExceeded); 5] = [
            (
                "nesting past the depth bound, which no recursion here pays for in stack",
                deep.clone(),
                Limits::DEFAULT.with_max_xml_depth(4),
                LimitExceeded::Depth,
            ),
            (
                "more elements than the policy admits",
                deep.clone(),
                Limits::DEFAULT.with_max_xml_elements(4),
                LimitExceeded::Elements,
            ),
            (
                "one element declaring more namespaces than may be live at once",
                wide,
                Limits::DEFAULT.with_max_prefix_bindings(4),
                LimitExceeded::PrefixBindings,
            ),
            (
                "a body longer than one response may be",
                deep,
                Limits::DEFAULT.with_max_response_bytes(16),
                LimitExceeded::Budget,
            ),
            (
                "one element's character data past its own ceiling",
                Vec::from(&br#"<D:x xmlns:D="DAV:">0123456789</D:x>"#[..]),
                Limits::DEFAULT.with_max_xml_text_bytes(4),
                LimitExceeded::Text,
            ),
        ];
        for (shape, body, limits, expected) in cases {
            let refused = read(&body, limits, TextPolicy::Verbatim);
            assert_eq!(refused, Err(DavError::Limit(expected)), "{shape}");
        }
    }

    /// The bindings a scope opened are gone when it closes, and the ledger says so.
    #[test]
    fn a_closing_element_gives_back_the_bindings_it_took() {
        let body = br#"<D:multistatus xmlns:D="DAV:">
          <D:response xmlns:C="urn:ietf:params:xml:ns:caldav"><C:calendar-data/></D:response>
        </D:multistatus>"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut live = Vec::new();
        {
            let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
            let mut reader = XmlReader::new(body);
            while let Some(event) = reader.next_event(&mut context).unwrap() {
                if let XmlEvent::End { depth, .. } = event {
                    live.push((depth, context.meter.live_prefix_bindings()));
                }
            }
        }
        // The innermost element declared nothing; the response gives back the one it declared,
        // and the root the one it declared, which leaves the ledger where it started.
        assert_eq!(live, [(3, 2), (2, 1), (1, 0)]);
        assert_eq!(meter.live_prefix_bindings(), 0);
    }

    /// A foreign subtree is skipped whole, with the diagnostic RFC 4918 section 17 costs.
    #[test]
    fn a_foreign_subtree_is_consumed_and_reported_rather_than_half_read() {
        let body = br#"<D:multistatus xmlns:D="DAV:" xmlns:s="http://sabredav.org/ns">
          <s:exception><s:message>Sabre\DAV\Exception</s:message></s:exception>
          <D:response/>
        </D:multistatus>"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut reader = XmlReader::new(body);
        let mut after = Vec::new();
        while let Some(event) = reader.next_event(&mut context).unwrap() {
            let XmlEvent::Start { known, .. } = event else {
                continue;
            };
            if known.is_none() {
                reader.skip_element(&mut context).unwrap();
                continue;
            }
            after.push(known);
        }
        // Nothing inside the foreign element reached the caller, and the sibling after it did.
        assert_eq!(
            after,
            [Some(ElementName::Multistatus), Some(ElementName::Response)]
        );
        assert_eq!(
            reported.first().copied().map(Diagnostic::code),
            Some(DiagnosticCode::DavForeignElementSkipped)
        );
        assert_eq!(
            reported.first().copied().map(Diagnostic::severity),
            Some(Severity::Note)
        );
    }

    /// A rejecting caller refuses the foreign element, and skips a modeled one in silence.
    #[test]
    fn a_rejecting_policy_refuses_the_foreign_element_and_only_that() {
        let body = br#"<D:multistatus xmlns:D="DAV:" xmlns:s="http://sabredav.org/ns">
          <D:response><D:href>/1.ics</D:href></D:response>
          <s:exception/>
        </D:multistatus>"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported)
            .with_unknown(UnknownPolicy::Reject);
        let mut reader = XmlReader::new(body);
        let mut refusal = None;
        while let Some(event) = reader.next_event(&mut context).unwrap() {
            let XmlEvent::Start { known, .. } = event else {
                continue;
            };
            // A known element is skipped in silence: this door is how a reader passes over a
            // subtree it has no use for, and a diagnostic about a modeled element would be a
            // report of something that did not happen.
            if known == Some(ElementName::Response) {
                reader.skip_element(&mut context).unwrap();
                continue;
            }
            if known.is_none() {
                refusal = Some(reader.skip_element(&mut context));
                break;
            }
        }
        assert!(
            reported.is_empty(),
            "skipping a modeled element says nothing"
        );
        assert_eq!(refusal, Some(Err(DavError::Foreign)));
    }

    /// Where the reader sits is answerable at every step, and the depth is the event's own.
    #[test]
    fn the_reader_answers_where_it_is() {
        let body = br#"<D:x xmlns:D="DAV:"><D:y/></D:x>"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut reader = XmlReader::new(body);
        let mut depths = Vec::new();
        while let Some(event) = reader.next_event(&mut context).unwrap() {
            match event {
                XmlEvent::Start { depth, .. } | XmlEvent::End { depth, .. } => depths.push(depth),
                XmlEvent::Text(_) => {},
            }
        }
        // An empty-element tag is one start and one end at the same depth, which is what a
        // caller counting `<D:href/>` against `<D:href></D:href>` needs it to be.
        assert_eq!(depths, [1, 2, 2, 1]);
        assert_eq!(reader.depth(), 0);
        assert_eq!(reader.offset(), u64::try_from(body.len()).unwrap());
    }

    /// A comment is octets a peer chose and a reader scanned, so it costs what it costs.
    ///
    /// The claim on this type — that many bodies under one `Meter` are bounded in aggregate —
    /// was false while a comment charged nothing: eight mebibytes refused as character data
    /// were free as `<!-- ... -->`, and `max_response_bytes` is per body rather than across
    /// them, so a peer bought unmetered scanning at whatever rate it liked, forever.
    #[test]
    fn a_comment_costs_the_octets_it_is_made_of() {
        let mut body = Vec::from(&br#"<D:multistatus xmlns:D="DAV:"><!--"#[..]);
        body.extend(core::iter::repeat_n(b'a', 4096));
        body.extend_from_slice(b"--></D:multistatus>");
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        assert_eq!(
            read(&body, limits, TextPolicy::Verbatim).map(|seen| seen.len()),
            Ok(2)
        );
        let mut sink = IgnoreDiagnostics;
        {
            let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
            let mut reader = XmlReader::new(&body);
            while reader.next_event(&mut context).unwrap().is_some() {}
        }
        assert!(meter.spent() >= 4096, "a comment cost {}", meter.spent());
    }

    /// An attribute value is the value XML 1.0 section 3.3.3 defines, not the octets in it.
    ///
    /// Section 3.3.3 resolves references and replaces every literal tab, line feed and
    /// carriage return with a space before the value is delivered. Handing the raw octets back
    /// made a `comp-filter name="VE&#78;T"` select `VE&#78;T` here and `VENT` in every
    /// conformant processor, which is two implementations disagreeing about which components a
    /// hostile `calendar-query` matches.
    #[test]
    fn an_attribute_value_is_resolved_and_normalized_before_it_is_delivered() {
        let body = br#"<C:comp-filter xmlns:C="urn:ietf:params:xml:ns:caldav" name="VE&#78;T"
 x="a&amp;b" y="p	q" />"#;
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut reader = XmlReader::new(body);
        let first = reader.next_event(&mut context).unwrap();
        assert!(matches!(first, Some(XmlEvent::Start { .. })));
        let named = |local: &'static [u8]| QName::new(Namespace::Other(b""), local);
        assert_eq!(reader.attribute(named(b"name")), Some(b"VENT".as_slice()));
        assert_eq!(reader.attribute(named(b"x")), Some(b"a&b".as_slice()));
        assert_eq!(reader.attribute(named(b"y")), Some(b"p q".as_slice()));
        // Every attribute is reachable without knowing its name, which is what a reader
        // keeping a foreign subtree needs in order to keep what was written on it.
        assert_eq!(reader.attribute_count(), 3);
    }

    /// A character the `Char` production excludes is refused however it is spelled.
    ///
    /// `&#0;` was refused under its own name and the literal octet was not, so one spelling
    /// was a violation and the other was invisible — and the run handed to the caller was not
    /// text. The exception is the elements the line-ending carve-out names and `DAV:href`,
    /// which this crate delivers as octets on purpose and states that it does.
    #[test]
    fn a_character_xml_excludes_is_refused_as_an_octet_too() {
        let cases: [&[u8]; 3] = [
            b"<D:displayname xmlns:D=\"DAV:\">a\x00b</D:displayname>",
            b"<D:displayname xmlns:D=\"DAV:\">a\x08b</D:displayname>",
            b"<D:displayname xmlns:D=\"DAV:\">\xc3\x28</D:displayname>",
        ];
        for body in cases {
            assert_eq!(
                read_default(body).err(),
                Some(DavError::Syntax(SyntaxError::ForbiddenCharacter)),
                "{body:?}"
            );
        }
        // A path a store holds that is not UTF-8 is the one value byte-shaped `Href` exists
        // for, and refusing it would mean this crate could not model a response it can read.
        let latin1 = b"<D:href xmlns:D=\"DAV:\">/calendars/ann/\xe9t\xe9.ics</D:href>";
        assert_eq!(read_default(latin1).map(|seen| seen.len()), Ok(3));
    }
}
