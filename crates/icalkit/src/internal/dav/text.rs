// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Character data, and the one place where XML's rules and iCalendar's rules disagree.
//!
//! XML 1.0 section 2.11 requires a processor to "behave as if it normalized all line breaks
//! in external parsed entities (including the document entity) on input, before parsing, by
//! translating both the two-character sequence #xD #xA and any #xD that is not followed by
//! #xA to a single #xA character". RFC 5545 section 3.1 terminates every content line with
//! `CRLF`. A CalDAV `<C:calendar-data>` element carries an iCalendar object verbatim inside
//! an XML document, and Radicale and `SabreDAV` both write those `CRLF` octets literally. A
//! conformant read of such a response therefore hands `ical-core` a calendar whose line
//! endings are not the ones the server stored, and writing it back changes the resource.
//!
//! RFC 4791 section 9.6 saw this coming and permits it: "Given that XML parsers normalize the
//! two-character sequence CRLF ... to a single LF character ..., the CR character ... MAY be
//! omitted in calendar object resources specified in the CALDAV:calendar-data XML element."
//! So a server that never sends a `CR` is conformant, and no reader can recover what such a
//! server never sent. What section 9.6 does *not* say is that a client may quietly rewrite
//! the line endings of a resource it received intact — and that is what a normalizing read
//! followed by a `PUT` does, to somebody else's data, with a changed `ETag` as the only trace.
//!
//! ## What this crate does about it
//!
//! Reading is verbatim for the elements [`ElementName::preserves_line_endings`] names and
//! section 2.11 normalization everywhere else. That set is three elements rather than one,
//! because RFC 4791 defines three places an iCalendar object travels through this envelope and
//! the argument does not weaken for the other two: `CALDAV:calendar-data` (section 9.6),
//! `CALDAV:calendar-timezone` (section 5.2.2, "a valid iCalendar object containing exactly one
//! VTIMEZONE component") and `CALDAV:timezone` (section 9.5). A client that reads a
//! collection's timezone under a folding read and `PROPPATCH`es it back rewrites the stored
//! object, which is the same harm one property over.
//!
//! Inside a verbatim element, references are still resolved, because a reference is markup
//! rather than a line break: `&#13;&#10;` from a conformant writer and a literal `CRLF` from
//! Radicale arrive at `ical-core` as the same two octets, which is the property that makes
//! this a scoped departure rather than a second dialect. Inside it, XML 1.0 section 2.2's
//! `Char` production is not enforced either, and that is one departure rather than two: an
//! element whose octets are handed back as they arrived is an element this reader has already
//! stopped being a conformant processor inside, and an `.ics` whose fold splits a codepoint is
//! a resource ADR 0001 guarantees this workspace round-trips.
//!
//! That departure is real, it is deliberate, and it is scoped: inside it this reader
//! is **not** a conformant XML 1.0 processor. Two documents that are equal as XML infosets
//! come out of it as different octets, so it must never be used to canonicalize or to verify
//! signed XML. A caller that wants strict conformance sets [`TextPolicy::Normalized`], gets
//! section 2.11 applied everywhere, and is told through
//! `DiagnosticCode::DavCalendarDataLineEndingsFolded` on every payload that lost a `CR` —
//! because the choice being available is worth nothing if taking it is silent.
//!
//! Writing is strictly conformant and needs no departure at all. A `CR` is written as the
//! character reference `&#13;`, which section 2.11 does not reach because a reference is
//! resolved after normalization, so any conformant processor — this one, `libxml2`, a peer's
//! — recovers the `CR`. No `CDATA` section is ever emitted: it cannot carry a `CR` past a
//! conformant reader, and it turns a literal `]]>` inside a `DESCRIPTION` into an escaping
//! bug. `>` is escaped unconditionally, which makes that sequence unwritable by accident.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::internal::core::{
    Diagnostic, DiagnosticCode, DiagnosticSink, LimitExceeded, Location, Meter, Severity,
    report_diagnostic,
};

use crate::internal::dav::element::ElementName;
use crate::internal::dav::failure::{DavError, SyntaxError};
use crate::internal::dav::sink::ByteSink;
// The character rules, the reference resolver and the escaping table are XML 1.0's rather than
// CalDAV's, so they live in the private layer `gates/xml-layering` compiles alone. What stays
// here is everything stated over an `ElementName` or a `ByteSink`, which is what that layer may
// not name (docs/adr/0012).
use crate::internal::dav::xml::chars::{
    check_chars as check_layer_chars, escape_for, normalize_attribute as normalize_layer, push,
    push_reference,
};
use crate::internal::dav::xml::scan::find;

/// The caller's policy for how character data is delivered.
///
/// A runtime policy rather than a feature flag on purpose. A feature is unified across a
/// dependency graph by the union rule, so one crate in a build could otherwise change how
/// another crate's calendars parse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextPolicy {
    /// Preserve the octets of the elements whose line endings are their content.
    ///
    /// The default, because it is the answer that cannot silently rewrite a resource.
    #[default]
    Verbatim,
    /// Apply XML 1.0 section 2.11 everywhere, as a conformant processor must.
    ///
    /// Costs the `CR` of every `CRLF` inside `calendar-data`, and says so through a
    /// diagnostic each time.
    Normalized,
}

/// How one element's character data is to be decoded.
///
/// Derived from the element and the caller's [`TextPolicy`] rather than chosen at a call site,
/// so the carve-out cannot spread to a second element by somebody passing the wrong value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextMode {
    /// Section 2.11 normalization, for an element whose line endings are its layout.
    Normalized,
    /// The same, for an element whose value this crate delivers as octets rather than as text.
    ///
    /// One element: `DAV:href`. `value.rs` states the decision it follows from — "a server is
    /// free to emit octets that are not UTF-8 in a path, and a type that cannot model a
    /// response one can read is the failure this workspace exists to prevent" — and a reader
    /// that held those octets to XML 1.0 section 2.2's `Char` production would refuse the
    /// response `Href` is byte-shaped in order to model. RFC 3986 requires a URI reference to
    /// be `US-ASCII` and stores emit paths that are not; both facts are true and this crate
    /// carries what arrived.
    NormalizedOctets,
    /// The octets as they arrived, for an element whose line endings are its content.
    Verbatim,
    /// Section 2.11 normalization of an element that would otherwise have been verbatim.
    ///
    /// Distinct from [`TextMode::Normalized`] because losing a `CR` from a `displayname` is
    /// XML working as specified and losing one from a `calendar-data` is a resource this
    /// caller can no longer write back unchanged. Only this mode reports it.
    NormalizedPayload,
}

impl TextMode {
    /// The mode an element's character data is read under.
    #[must_use]
    pub const fn of(element: Option<ElementName>, policy: TextPolicy) -> Self {
        let preserving = match element {
            Some(name) => name.preserves_line_endings(),
            None => false,
        };
        if !preserving {
            return match element {
                Some(ElementName::Href) => Self::NormalizedOctets,
                _ => Self::Normalized,
            };
        }
        match policy {
            TextPolicy::Verbatim => Self::Verbatim,
            TextPolicy::Normalized => Self::NormalizedPayload,
        }
    }

    /// Whether this mode folds line breaks.
    #[must_use]
    pub const fn normalizes(self) -> bool {
        matches!(
            self,
            Self::Normalized | Self::NormalizedOctets | Self::NormalizedPayload
        )
    }

    /// Whether this mode holds its run to XML 1.0 section 2.2's `Char` production.
    ///
    /// True everywhere the run is delivered as text, and false for the two places this crate
    /// delivers octets on purpose: inside the elements the line-ending carve-out names, and
    /// inside a `DAV:href`. Both departures are stated rather than discovered, and both are
    /// about a value a peer holds that would otherwise become unreadable rather than safe.
    #[must_use]
    pub const fn checks_characters(self) -> bool {
        matches!(self, Self::Normalized)
    }

    /// Whether this mode is inside the elements the line-ending carve-out names.
    ///
    /// The carve-out's boundary is also where XML 1.0 section 2.2's `Char` production stops
    /// being enforced, and that is one departure rather than two: an element whose octets are
    /// handed back as they arrived is an element this reader has already stopped being a
    /// conformant processor inside. An `.ics` a server stores may hold a fold that splits a
    /// codepoint — `docs/adr/0001` guarantees that file round-trips — and refusing to *read*
    /// it here would lose a resource the peer is holding rather than protect anybody from it.
    #[must_use]
    pub const fn preserves(self) -> bool {
        matches!(self, Self::Verbatim | Self::NormalizedPayload)
    }
}

/// What terminates the lines of a decoded run, as delivered.
///
/// Carried beside the octets rather than left for the caller to work out, because the question
/// it answers — "are these the octets the server stored?" — has no answer once the run is a
/// bare slice. A caller that means to write the payload back reads this first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineEndings {
    /// The run holds no line break at all.
    Absent,
    /// Every line break is `CRLF`, as RFC 5545 section 3.1 requires.
    Crlf,
    /// Every line break is a bare `LF`.
    ///
    /// Either the server omitted the `CR` as RFC 4791 section 9.6 permits, or something
    /// between it and here did. Nothing in the octets tells the two apart.
    Lf,
    /// Every line break is a bare `CR`.
    Cr,
    /// The run mixes terminators.
    Mixed,
    /// This reader folded the `CR`s away under [`TextPolicy::Normalized`].
    ///
    /// The one value that is a fact about the read rather than about the octets: whatever the
    /// server sent, what the caller holds cannot be written back as what it received.
    Folded,
}

impl LineEndings {
    /// Classify the terminators of a run this reader did not fold.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut seen = None;
        let mut at = 0;
        while let Some(&byte) = bytes.get(at) {
            let found = match (byte, bytes.get(at.saturating_add(1))) {
                (b'\r', Some(b'\n')) => Some(Self::Crlf),
                (b'\r', _) => Some(Self::Cr),
                (b'\n', _) => Some(Self::Lf),
                _ => None,
            };
            at = at.saturating_add(if found == Some(Self::Crlf) { 2 } else { 1 });
            seen = match (seen, found) {
                (kept, None) => kept,
                (None, one) => one,
                (kept, one) if kept == one => kept,
                _ => return Self::Mixed,
            };
        }
        seen.unwrap_or(Self::Absent)
    }

    /// Whether these octets are the ones the peer wrote.
    #[must_use]
    pub const fn is_as_sent(self) -> bool {
        !matches!(self, Self::Folded)
    }
}

/// A decoded run of character data.
///
/// `Wire` is a slice of the caller's own body and exists only when nothing had to be changed;
/// a reference, a `CDATA` boundary or a folded line break forces `Reassembled`. Which one a
/// caller got is visible rather than hidden, because the difference is an allocation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextRun<'a> {
    /// Octets borrowed from the body exactly as they lie in it.
    Wire(&'a [u8]),
    /// Octets that appear nowhere contiguously in the body and had to be copied.
    Reassembled(Box<[u8]>),
}

impl TextRun<'_> {
    /// The decoded octets.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Wire(borrowed) => borrowed,
            Self::Reassembled(owned) => owned,
        }
    }

    /// Whether the octets were copied out of the body.
    #[must_use]
    pub const fn is_reassembled(&self) -> bool {
        matches!(self, Self::Reassembled(_))
    }
}

/// A run of character data and the answer to whether it is what the peer sent.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecodedText<'a> {
    /// The octets.
    pub run: TextRun<'a>,
    /// What terminates their lines, and whether this reader changed that.
    pub line_endings: LineEndings,
}

/// Decode one element's character data span.
///
/// `raw` is the span between an element's start tag and its end tag, `CDATA` sections and
/// references included; finding that span is the tokenizer's job and resolving what is inside
/// it is this function's. `offset` is where the span begins in the body, so a diagnostic can
/// point at it.
///
/// The span is charged against the caller's per-element ceiling and its octet budget before
/// anything is copied, so an enormous `calendar-data` is refused at the bound rather than
/// after it is resident.
pub fn decode_text<'a>(
    raw: &'a [u8],
    mode: TextMode,
    offset: u64,
    meter: &mut Meter,
    sink: &mut dyn DiagnosticSink,
) -> Result<DecodedText<'a>, DavError> {
    let length = u32::try_from(raw.len()).map_err(|_| LimitExceeded::Text)?;
    meter.try_charge_text(length)?;

    // The span is held to XML 1.0 section 2.2's `Char` production and to section 4.3.3's
    // requirement that the document entity be well-formed UTF-8, before anything is decoded
    // and before the borrowed fast path below. `&#0;` was already refused under its own name
    // by `push_character_reference`; the literal octet is the same code point in the same
    // position, and accepting one spelling while refusing the other handed a caller a run
    // that is not text and re-emitted it into a document no peer can parse.
    if mode.checks_characters() {
        check_chars(raw)?;
    }

    if !needs_reassembly(raw, mode) {
        return Ok(DecodedText {
            run: TextRun::Wire(raw),
            line_endings: LineEndings::of(raw),
        });
    }

    let mut out = Vec::new();
    out.try_reserve(raw.len())
        .map_err(|_| LimitExceeded::Text)?;
    let folded = reassemble(raw, mode, &mut out)?;
    report_reassembly(mode, folded, offset, meter, sink);

    let line_endings = if folded {
        LineEndings::Folded
    } else {
        LineEndings::of(&out)
    };
    Ok(DecodedText {
        run: TextRun::Reassembled(out.into_boxed_slice()),
        line_endings,
    })
}

/// Whether the span holds anything that stops it from being handed back as a slice.
fn needs_reassembly(raw: &[u8], mode: TextMode) -> bool {
    raw.iter()
        .any(|byte| matches!(*byte, b'&' | b'<') || (mode.normalizes() && *byte == b'\r'))
}

/// Report what reassembly cost, on the channel a read continues past.
fn report_reassembly(
    mode: TextMode,
    folded: bool,
    offset: u64,
    meter: &mut Meter,
    sink: &mut dyn DiagnosticSink,
) {
    let location = Location::at_offset(offset);
    let code = match (mode, folded) {
        (TextMode::NormalizedPayload, true) => DiagnosticCode::DavCalendarDataLineEndingsFolded,
        (TextMode::Verbatim | TextMode::NormalizedPayload, _) => {
            DiagnosticCode::DavCalendarDataCopied
        },
        (TextMode::Normalized | TextMode::NormalizedOctets, _) => return,
    };
    report_diagnostic(sink, meter, Diagnostic::new(code, Severity::Note, location));
}

/// Copy the span into `out`, resolving references and `CDATA`. `true` if a `CR` was folded.
fn reassemble(raw: &[u8], mode: TextMode, out: &mut Vec<u8>) -> Result<bool, DavError> {
    let mut folded = false;
    let mut at = 0;
    while at < raw.len() {
        let rest = raw.get(at..).ok_or(SyntaxError::Malformed)?;
        let stop = rest
            .iter()
            .position(|byte| matches!(*byte, b'&' | b'<'))
            .unwrap_or(rest.len());
        let run = rest.get(..stop).ok_or(SyntaxError::Malformed)?;
        folded |= push_literal(run, mode, out)?;
        at = at.saturating_add(stop);
        let Some(&marker) = raw.get(at) else { break };
        at = if marker == b'&' {
            push_reference(raw, at, out)?
        } else {
            let (next, cut) = push_cdata(raw, at, mode, out)?;
            folded |= cut;
            next
        };
    }
    Ok(folded)
}

/// Copy octets that carry no markup, applying section 2.11 where the mode asks for it.
fn push_literal(run: &[u8], mode: TextMode, out: &mut Vec<u8>) -> Result<bool, DavError> {
    if !mode.normalizes() {
        return push(out, run).map(|()| false).map_err(DavError::from);
    }
    let mut folded = false;
    let mut at = 0;
    while let Some(&byte) = run.get(at) {
        let carriage = byte == b'\r';
        folded |= carriage;
        push(out, &[if carriage { b'\n' } else { byte }])?;
        // A `CRLF` is one break, so its `LF` is consumed with it rather than emitted twice.
        let paired = carriage && run.get(at.saturating_add(1)) == Some(&b'\n');
        at = at.saturating_add(if paired { 2 } else { 1 });
    }
    Ok(folded)
}

/// Refuse octets no conformant XML processor would deliver as characters.
///
/// The rule is XML 1.0 sections 2.2 and 4.3.3 and lives in the layer; this is the door the rest
/// of the crate reaches it through, so that a refusal arrives as a [`DavError`] like every other
/// one rather than as a second failure vocabulary spreading upward.
pub(crate) fn check_chars(bytes: &[u8]) -> Result<(), DavError> {
    check_layer_chars(bytes).map_err(DavError::from)
}

/// Normalize an attribute value the way XML 1.0 section 3.3.3 requires, into `out`.
///
/// The value between the quotes is not the value the attribute has; the layer states why and
/// does the work. This is the door, for the reason [`check_chars`] above is one.
pub(crate) fn normalize_attribute(raw: &[u8], out: &mut Vec<u8>) -> Result<(), DavError> {
    normalize_layer(raw, out).map_err(DavError::from)
}

/// Copy a `CDATA` section's content and answer where the octet after `]]>` is.
///
/// A `<` in character data is a `CDATA` section or it is not well-formed XML; there is no
/// third reading, and guessing at one is how a tokenizer becomes accidentally complete.
fn push_cdata(
    raw: &[u8],
    start: usize,
    mode: TextMode,
    out: &mut Vec<u8>,
) -> Result<(usize, bool), DavError> {
    let rest = raw.get(start..).ok_or(SyntaxError::Malformed)?;
    let body = rest
        .strip_prefix(b"<![CDATA[".as_slice())
        .ok_or(SyntaxError::Malformed)?;
    let end = find(body, b"]]>").ok_or(SyntaxError::Truncated)?;
    let content = body.get(..end).ok_or(SyntaxError::Malformed)?;
    // References are not markup inside a `CDATA` section, so only the line-break rule applies.
    let folded = push_literal(content, mode, out)?;
    let consumed = start
        .saturating_add(9)
        .saturating_add(end)
        .saturating_add(3)
        .min(raw.len());
    Ok((consumed, folded))
}

/// Write character data, escaped so that a conformant reader recovers every octet.
///
/// `CR` becomes `&#13;` because a literal one would be folded to `LF` by section 2.11 before
/// any reader saw it. `>` is escaped although XML does not require it, so that a `]]>` in a
/// `DESCRIPTION` cannot end a section this crate never opens.
pub fn write_escaped_text(out: &mut dyn ByteSink, bytes: &[u8]) -> Result<(), DavError> {
    write_escaped(out, bytes, false)
}

/// Write an attribute value, escaped for the rules that apply inside quotes.
///
/// Section 3.3.3 replaces a literal `TAB`, `LF` or `CR` in an attribute value with a space
/// during attribute-value normalization, and a character reference to one survives instead —
/// so the three are written as references and the quote that would end the value is escaped.
pub fn write_escaped_attribute(out: &mut dyn ByteSink, bytes: &[u8]) -> Result<(), DavError> {
    write_escaped(out, bytes, true)
}

/// The escaping both doors share.
fn write_escaped(out: &mut dyn ByteSink, bytes: &[u8], in_attribute: bool) -> Result<(), DavError> {
    let mut run_start = 0;
    for (at, byte) in bytes.iter().enumerate() {
        let Some(replacement) = escape_for(*byte, in_attribute) else {
            continue;
        };
        let plain = bytes.get(run_start..at).ok_or(SyntaxError::Malformed)?;
        out.write(plain)?;
        out.write(replacement)?;
        run_start = at.saturating_add(1);
    }
    let tail = bytes.get(run_start..).ok_or(SyntaxError::Malformed)?;
    out.write(tail)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{Diagnostic, DiagnosticCode, IgnoreDiagnostics, Limits, Meter};

    use super::{
        DecodedText, LineEndings, TextMode, TextPolicy, decode_text, write_escaped_attribute,
        write_escaped_text,
    };
    use crate::internal::dav::element::ElementName;
    use crate::internal::dav::failure::{DavError, SyntaxError};

    /// The `calendar-data` payload of a `SabreDAV`-shaped multistatus, with the `CRLF`
    /// terminators RFC 5545 section 3.1 requires and a content line folded at octet 75.
    const SABREDAV_PAYLOAD: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
PRODID:-//Example Corp.//CalDAV Client//EN\r\nBEGIN:VEVENT\r\nUID:1@example.invalid\r\n\
DTSTAMP:20260101T000000Z\r\nDTSTART:20260105T090000Z\r\n\
SUMMARY:Weekly sync with a summary long enough that the exporter folded i\r\n t here\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    fn decode<'a>(raw: &'a [u8], mode: TextMode, meter: &mut Meter) -> DecodedText<'a> {
        let mut sink = IgnoreDiagnostics;
        decode_text(raw, mode, 0, meter, &mut sink).unwrap()
    }

    #[test]
    fn the_carve_out_hands_ical_core_the_octets_the_server_sent() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let decoded = decode(SABREDAV_PAYLOAD, TextMode::Verbatim, &mut meter);
        // Borrowed, not copied: the payload is a slice of the caller's own body.
        assert!(!decoded.run.is_reassembled());
        assert_eq!(decoded.run.as_bytes(), SABREDAV_PAYLOAD);
        assert_eq!(decoded.line_endings, LineEndings::Crlf);
        assert!(decoded.line_endings.is_as_sent());
        // The fold survives as the `CRLF SPACE` that RFC 5545 section 3.1 unfolds.
        assert!(decoded.run.as_bytes().windows(3).any(|at| at == b"\r\n "));
    }

    #[test]
    fn a_conformant_writers_character_references_arrive_as_the_same_octets() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let escaped = b"BEGIN:VCALENDAR&#13;\nVERSION:2.0&#13;\nEND:VCALENDAR&#13;\n";
        let decoded = decode(escaped, TextMode::Verbatim, &mut meter);
        // A reference is markup, not a line break, so it is resolved in either mode and the
        // two server dialects converge on one answer.
        assert!(decoded.run.is_reassembled());
        assert_eq!(
            decoded.run.as_bytes(),
            b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n"
        );
        assert_eq!(decoded.line_endings, LineEndings::Crlf);
    }

    #[test]
    fn the_conformant_read_loses_the_carriage_returns_and_says_so() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let decoded = decode_text(
            SABREDAV_PAYLOAD,
            TextMode::NormalizedPayload,
            0,
            &mut meter,
            &mut reported,
        )
        .unwrap();
        assert!(!decoded.run.as_bytes().contains(&b'\r'));
        assert_eq!(decoded.line_endings, LineEndings::Folded);
        assert!(!decoded.line_endings.is_as_sent());
        assert_eq!(
            reported.first().copied().map(Diagnostic::code),
            Some(DiagnosticCode::DavCalendarDataLineEndingsFolded)
        );
    }

    #[test]
    fn an_ordinary_element_is_normalized_without_comment() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let decoded = decode_text(
            b"Ann\r\nWork",
            TextMode::Normalized,
            0,
            &mut meter,
            &mut reported,
        )
        .unwrap();
        assert_eq!(decoded.run.as_bytes(), b"Ann\nWork");
        assert!(reported.is_empty());
    }

    #[test]
    fn the_mode_is_derived_from_the_element_and_never_chosen() {
        assert_eq!(
            TextMode::of(Some(ElementName::CalendarData), TextPolicy::Verbatim),
            TextMode::Verbatim
        );
        assert_eq!(
            TextMode::of(Some(ElementName::CalendarData), TextPolicy::Normalized),
            TextMode::NormalizedPayload
        );
        assert_eq!(
            TextMode::of(Some(ElementName::Displayname), TextPolicy::Verbatim),
            TextMode::Normalized
        );
        assert_eq!(
            TextMode::of(None, TextPolicy::Verbatim),
            TextMode::Normalized
        );
    }

    #[test]
    fn nothing_beyond_the_five_predefined_entities_resolves() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let refused = decode_text(b"&xxe;", TextMode::Normalized, 0, &mut meter, &mut sink);
        assert_eq!(refused, Err(DavError::Syntax(SyntaxError::UndefinedEntity)));
    }

    #[test]
    fn a_reference_to_a_code_point_xml_excludes_is_refused() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let refused = decode_text(b"&#0;", TextMode::Normalized, 0, &mut meter, &mut sink);
        assert_eq!(
            refused,
            Err(DavError::Syntax(SyntaxError::ForbiddenCharacter))
        );
    }

    #[test]
    fn a_cdata_section_carries_its_content_and_its_line_endings() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let decoded = decode(b"<![CDATA[a]]>b\r\n", TextMode::Verbatim, &mut meter);
        assert_eq!(decoded.run.as_bytes(), b"ab\r\n");
    }

    #[test]
    fn an_unterminated_cdata_section_is_refused_rather_than_read_to_the_end() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut sink = IgnoreDiagnostics;
        let refused = decode_text(b"<![CDATA[a", TextMode::Verbatim, 0, &mut meter, &mut sink);
        assert_eq!(refused, Err(DavError::Syntax(SyntaxError::Truncated)));
    }

    #[test]
    fn what_is_written_is_what_a_conformant_reader_gets_back() {
        let mut out: Vec<u8> = Vec::new();
        write_escaped_text(&mut out, b"a\r\nb & c ]]> d").unwrap();
        assert_eq!(out, b"a&#13;\nb &amp; c ]]&gt; d".to_vec());

        let mut round: Vec<u8> = Vec::new();
        write_escaped_text(&mut round, SABREDAV_PAYLOAD).unwrap();
        let mut meter = Meter::new(Limits::DEFAULT);
        let decoded = decode(&round, TextMode::Verbatim, &mut meter);
        assert_eq!(decoded.run.as_bytes(), SABREDAV_PAYLOAD);
    }

    #[test]
    fn an_attribute_value_escapes_what_attribute_normalization_would_eat() {
        let mut out: Vec<u8> = Vec::new();
        write_escaped_attribute(&mut out, b"a\tb\r\n\"c\"").unwrap();
        assert_eq!(out, b"a&#9;b&#13;&#10;&quot;c&quot;".to_vec());
    }

    #[test]
    fn a_payload_past_the_per_element_ceiling_is_refused_before_it_is_copied() {
        let limits = Limits::DEFAULT.with_max_xml_text_bytes(8);
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let refused = decode_text(
            SABREDAV_PAYLOAD,
            TextMode::Verbatim,
            0,
            &mut meter,
            &mut sink,
        );
        assert!(matches!(refused, Err(DavError::Limit(_))));
    }
}
