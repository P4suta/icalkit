// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5545 section 3.3.11 `TEXT` escaping, and section 3.2 parameter quoting.
//!
//! Specification: <https://www.rfc-editor.org/rfc/rfc5545#section-3.3.11>.
//!
//! Both directions are octet operations. Every substitution the specification defines is
//! ASCII, which is what makes validate-then-unescape sound: no substitution can satisfy a
//! UTF-8 continuation requirement, so an orphaned lead byte fails validation deterministically
//! instead of being completed by an escape that happened to follow it.
//!
//! Nothing here changes what is stored. Resolving escapes is a view over octets the tree
//! still holds escaped, so a producer's spelling of a line feed — `\n` or `\N`, both legal —
//! survives whether or not anything ever resolves it, and this module rewrites neither into
//! the other. Escaping is the other direction and a narrower one: it is for text this crate
//! authors itself, where there is no producer whose spelling to preserve, which is why the
//! two tables below are separate rather than one read backwards.
//!
//! A parameter whose closing `DQUOTE` never arrived comes back as a value that says so. It is
//! not an error, because the octets are all still there and section 3.2 being violated has
//! never been a reason to lose them; it is not a diagnostic either, because a diagnostic
//! needs an offset and a severity that only the caller holding the input can supply
//! (`docs/adr/0009`).
//!
//! Allocation is the caller's as well. Each direction has a form returning [`Cow`] that
//! borrows when there is nothing to do, and an `_into` form that appends to a buffer the
//! caller has already charged against its meter (`docs/adr/0007`).

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::report::DiagnosticCode;

/// The section 3.3.11 substitutions, as `(octet after the backslash, octet it stands for)`.
///
/// A table rather than a match arm per case, because the same rows drive both directions and
/// two hand-written directions are two places for them to disagree. `N` and `n` both stand
/// for a line feed: the specification permits either spelling, and a producer that wrote the
/// uppercase one gets it back.
pub const TEXT_ESCAPES: [(u8, u8); 5] = [
    (b'\\', b'\\'),
    (b';', b';'),
    (b',', b','),
    (b'n', b'\n'),
    (b'N', b'\n'),
];

/// The octets that must be escaped when this crate writes a `TEXT` value.
///
/// A line feed is written as the lowercase `\n` of the two spellings the specification
/// allows, because a value this crate authored has no producer whose spelling to preserve.
pub const TEXT_MUST_ESCAPE: [u8; 4] = *b"\\;,\n";

/// The octets that force a section 3.2 parameter value to be wrapped in `DQUOTE`.
///
/// Section 3.2's `SAFE-CHAR` excludes exactly these three from an unquoted value, because
/// each one ends something: `;` a parameter, `:` the header, `,` a value in a list. A value
/// carrying none of them is written bare, since quoting one that did not need it would put
/// octets on the wire that the caller never asked for.
pub const PARAMETER_MUST_QUOTE: [u8; 3] = *b";:,";

/// The octet `spelling` stands for when it follows a backslash, or `None` if it stands for
/// nothing.
///
/// Both spellings of a line feed answer here. Reading is where a producer's choice has to be
/// understood; only writing has to pick one.
#[must_use]
pub fn text_escape_meaning(spelling: u8) -> Option<u8> {
    TEXT_ESCAPES
        .into_iter()
        .find(|(written, _)| *written == spelling)
        .map(|(_, stands_for)| stands_for)
}

/// How this crate spells `octet` when it authors a `TEXT` value, or `None` for an octet it
/// writes as itself.
///
/// Gated on [`TEXT_MUST_ESCAPE`] and not on [`TEXT_ESCAPES`] alone, because the first table
/// is a policy about what this crate emits and the second is a mapping both directions share:
/// a spelling that is legal to read back is not automatically one to write. Where the mapping
/// offers two spellings for one octet the earlier row wins, which is what makes an authored
/// line feed come out lowercase.
#[must_use]
pub fn text_escape_spelling(octet: u8) -> Option<u8> {
    if !TEXT_MUST_ESCAPE.contains(&octet) {
        return None;
    }
    TEXT_ESCAPES
        .into_iter()
        .find(|(_, stands_for)| *stands_for == octet)
        .map(|(written, _)| written)
}

/// The substitution at the front of `rest`, and what follows it.
///
/// Split out so the loop below stays one decision deep. A backslash that begins nothing the
/// table defines is not an escape at all, and neither is one with nothing after it.
fn take_escape(rest: &[u8]) -> Option<(u8, &[u8])> {
    let (&head, tail) = rest.split_first()?;
    if head != b'\\' {
        return None;
    }
    let (&spelling, after) = tail.split_first()?;
    Some((text_escape_meaning(spelling)?, after))
}

/// Whether resolving the escapes in `bytes` would change anything.
///
/// False for a backslash that begins no defined substitution, which is why a value carrying
/// only such a backslash is borrowed rather than copied.
#[must_use]
pub fn text_needs_unescaping(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .any(|pair| matches!(pair, [b'\\', spelling] if text_escape_meaning(*spelling).is_some()))
}

/// Resolve the escapes in a `TEXT` value, borrowing when there are none.
///
/// A backslash that begins no defined substitution — including one at the very end of the
/// value, with nothing left for it to escape — is passed through as itself. That is not
/// leniency dressed up as a rule: the stored octets are what get written back either way, so
/// a repair invented here would reach a caller's display and nothing else, while making the
/// text it showed disagree with the file.
///
/// ```
/// use ical_grammar::unescape_text;
///
/// assert_eq!(unescape_text(br"a\nb").as_ref(), b"a\nb");
/// assert_eq!(unescape_text(br"ends with \").as_ref(), br"ends with \");
/// ```
#[must_use]
pub fn unescape_text(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !text_needs_unescaping(bytes) {
        return Cow::Borrowed(bytes);
    }
    // Resolving only ever shortens, so this capacity is never grown past.
    let mut resolved = Vec::with_capacity(bytes.len());
    unescape_text_into(bytes, &mut resolved);
    Cow::Owned(resolved)
}

/// Resolve the escapes in a `TEXT` value, appending to `out`.
///
/// The form for a caller that owns a buffer and has already charged the octets it is about to
/// append; [`unescape_text`] is the form for one that has not.
pub fn unescape_text_into(bytes: &[u8], out: &mut Vec<u8>) {
    let mut rest = bytes;
    while let Some((&head, tail)) = rest.split_first() {
        if let Some((stands_for, after)) = take_escape(rest) {
            out.push(stands_for);
            rest = after;
        } else {
            out.push(head);
            rest = tail;
        }
    }
}

/// Whether writing `bytes` as a `TEXT` value would escape anything.
#[must_use]
pub fn text_needs_escaping(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|octet| text_escape_spelling(*octet).is_some())
}

/// Escape a `TEXT` value for writing, borrowing when nothing needs it.
///
/// This is not the inverse of [`unescape_text`] over arbitrary input and is not meant to be.
/// Reading accepts both spellings of a line feed and writing emits one, so text that arrived
/// as `\N` and came back through here would come out as `\n` — which is why nothing on the
/// round-trip path passes preserved octets through this function. It is for values this crate
/// authors, where no spelling is being overwritten.
///
/// ```
/// use ical_grammar::escape_text;
///
/// assert_eq!(escape_text(b"a;b\nc").as_ref(), br"a\;b\nc");
/// assert_eq!(escape_text(b"nothing to do").as_ref(), b"nothing to do");
/// ```
#[must_use]
pub fn escape_text(bytes: &[u8]) -> Cow<'_, [u8]> {
    // Counted rather than estimated: each escaped octet costs exactly one more, so one extra
    // pass buys an exact capacity and no reallocation on the way out.
    let escape_count = bytes
        .iter()
        .filter(|octet| text_escape_spelling(**octet).is_some())
        .count();
    if escape_count == 0 {
        return Cow::Borrowed(bytes);
    }
    // Saturating because a length plus a count of a subset of the same slice cannot overflow
    // a `usize` for any slice that exists; the saturation is unreachable, not a truncation.
    let mut written = Vec::with_capacity(bytes.len().saturating_add(escape_count));
    escape_text_into(bytes, &mut written);
    Cow::Owned(written)
}

/// Escape a `TEXT` value for writing, appending to `out`.
///
/// The form for a caller that owns a buffer and has already charged the octets it is about to
/// append; [`escape_text`] is the form for one that has not.
pub fn escape_text_into(bytes: &[u8], out: &mut Vec<u8>) {
    for &octet in bytes {
        match text_escape_spelling(octet) {
            Some(spelling) => {
                out.push(b'\\');
                out.push(spelling);
            },
            None => out.push(octet),
        }
    }
}

/// What surrounded a section 3.2 parameter value, once the quotes were taken off.
///
/// An outcome rather than a `Result`, because none of the three is a failure to produce a
/// value: the unterminated case has octets too, and dropping them to report a violation is
/// the data loss `docs/adr/0001` exists to prevent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParameterQuoting {
    /// No `DQUOTE` was present, and the value is exactly the octets that were there.
    Bare,
    /// An opening and a closing `DQUOTE` were present, and both were removed.
    Quoted,
    /// An opening `DQUOTE` was present and no closing one arrived before the value ended.
    Unterminated,
}

impl ParameterQuoting {
    /// Whether a `DQUOTE` was opened and never closed.
    #[must_use]
    pub const fn is_unterminated(self) -> bool {
        matches!(self, Self::Unterminated)
    }

    /// The code a caller reports for this outcome, or `None` when there is nothing to report.
    ///
    /// The mapping lives here so that every caller that notices an unterminated quote names
    /// it the same way; the location and the severity stay with the caller, which is the half
    /// of a diagnostic this module cannot know.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Bare | Self::Quoted => None,
            Self::Unterminated => Some(DiagnosticCode::UnterminatedQuotedParameter),
        }
    }
}

/// A section 3.2 parameter value with its `DQUOTE` pair taken off, and what was found.
///
/// Borrowed, because unquoting removes octets from the ends and never rewrites the middle:
/// there is nothing here to own. The quotes stay in storage regardless — section 3.2 lets a
/// producer quote a value that did not need it, and writing back the unquoted form would send
/// a line the producer did not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnquotedParameter<'a> {
    /// The octets between the quotes, or all of them when there were none.
    value: &'a [u8],
    /// What the quotes were doing.
    quoting: ParameterQuoting,
}

impl<'a> UnquotedParameter<'a> {
    /// The octets between the quotes, or all of them when there were none.
    #[must_use]
    pub const fn value(self) -> &'a [u8] {
        self.value
    }

    /// What the quotes were doing.
    #[must_use]
    pub const fn quoting(self) -> ParameterQuoting {
        self.quoting
    }

    /// Whether a `DQUOTE` was opened and never closed.
    #[must_use]
    pub const fn is_unterminated(self) -> bool {
        self.quoting.is_unterminated()
    }

    /// The code a caller reports for this value, or `None` when there is nothing to report.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        self.quoting.diagnostic_code()
    }
}

/// Take the `DQUOTE` pair off a section 3.2 parameter value, saying what was found.
///
/// The closing quote is looked for in what is left *after* the opening one is removed, which
/// is the whole reason a lone `DQUOTE` comes back as unterminated: tested against the original
/// octets it both starts and ends with a quote, and calling that a well-formed empty value
/// would silently accept a truncated line.
///
/// ```
/// use ical_grammar::{unquote_parameter, ParameterQuoting};
///
/// let closed = unquote_parameter(b"\"Europe/Paris\"");
/// assert_eq!(closed.value(), b"Europe/Paris");
/// assert_eq!(closed.quoting(), ParameterQuoting::Quoted);
///
/// let cut_short = unquote_parameter(b"\"Europe/Paris");
/// assert_eq!(cut_short.value(), b"Europe/Paris");
/// assert!(cut_short.is_unterminated());
/// ```
#[must_use]
pub fn unquote_parameter(bytes: &[u8]) -> UnquotedParameter<'_> {
    let Some(after_opening) = bytes.strip_prefix(b"\"") else {
        return UnquotedParameter {
            value: bytes,
            quoting: ParameterQuoting::Bare,
        };
    };
    match after_opening.strip_suffix(b"\"") {
        Some(between) => UnquotedParameter {
            value: between,
            quoting: ParameterQuoting::Quoted,
        },
        None => UnquotedParameter {
            value: after_opening,
            quoting: ParameterQuoting::Unterminated,
        },
    }
}

/// Whether section 3.2 requires `bytes` to be written inside a `DQUOTE` pair.
#[must_use]
pub fn parameter_needs_quoting(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|octet| PARAMETER_MUST_QUOTE.contains(octet))
}

/// Whether section 3.2 can write `bytes` as a parameter value at all.
///
/// A `DQUOTE` inside a value has no spelling: `QSAFE-CHAR` excludes it and section 3.2 defines
/// no escape that would bring it back, so quoting such a value produces a line that reads back
/// as something else. Control characters other than `HTAB` are excluded by the same grammar
/// and would end the line outright. This is a question and not a refusal — what a caller does
/// with the answer is the caller's, and this module never rejects octets it was handed.
#[must_use]
pub fn parameter_is_representable(bytes: &[u8]) -> bool {
    !bytes
        .iter()
        .any(|octet| *octet == b'"' || is_control_octet(*octet))
}

/// The octets that end a section 3.2 parameter name, whatever else surrounds them.
///
/// Each one hands the rest of the name to something else: `=` starts the value, `;` starts the
/// next parameter, `:` ends the header, `,` separates the values of a multi-valued parameter,
/// and a `DQUOTE` opens a quoted string. A name carrying any of them is a name the reader
/// would give back in pieces.
pub const PARAMETER_NAME_DELIMITERS: [u8; 5] = *b"=;:,\"";

/// Whether section 3.2 can write `bytes` as a parameter name at all.
///
/// Narrower than section 3.2's `param-name`, which is `iana-token / x-name` and admits only
/// `ALPHA`, `DIGIT` and `-`. Producers write `_` and `.` in vendor parameter names and this
/// crate reads them back unchanged, so refusing them on the write side would refuse a name
/// that survives a round trip. What is refused instead is the narrower and mechanical claim:
/// a name that would not be read back as the same name. The empty name is one of those — it
/// reads back as a parameter that is not there.
#[must_use]
pub fn parameter_name_is_representable(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && !bytes
            .iter()
            .any(|octet| PARAMETER_NAME_DELIMITERS.contains(octet) || is_control_octet(*octet))
}

/// Whether RFC 5545 section 3.1 can write `bytes` as a property or a component name.
///
/// Narrower than section 3.1's `name`, which is `iana-token / x-name` and admits only `ALPHA`,
/// `DIGIT` and `-`. Producers write `_` and `.` in vendor names and this crate reads them back
/// unchanged, so refusing them on the write side would refuse a name that survives a round
/// trip. What is refused instead is the mechanical claim: a name the reader would not give back
/// whole. `;` opens a parameter and `:` ends the header, so a name carrying either comes back
/// shorter with the rest of the line attached; a control character ends the physical line
/// outright, which is how one fabricated property becomes two. The empty name is refused as
/// well, because it reads back as the blank line it looks like.
///
/// A component name is held to the same rule although it is written as a `BEGIN` line's
/// *value*, where `;` and `:` would survive. One predicate is worth more than the two octets it
/// costs, and neither is a name anybody means to author.
#[must_use]
pub fn property_name_is_representable(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && !bytes
            .iter()
            .any(|octet| matches!(*octet, b';' | b':') || is_control_octet(*octet))
}

/// Whether `octet` is one of section 3.1's `CONTROL` octets.
///
/// `HTAB` is deliberately outside the set: section 3.1 counts it as whitespace, and a
/// parameter value carrying one is legal however unusual it looks.
#[must_use]
pub const fn is_control_octet(octet: u8) -> bool {
    matches!(octet, 0x00..=0x08 | 0x0A..=0x1F | 0x7F)
}

/// Write a section 3.2 parameter value, adding the `DQUOTE` pair only when it is required.
///
/// Octets that [`parameter_is_representable`] rejects are written through unchanged. Refusing
/// here would make this module the place a value is rejected, and the caller holding the
/// property is the only one that can say what a refusal costs it.
#[must_use]
pub fn quote_parameter(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !parameter_needs_quoting(bytes) {
        return Cow::Borrowed(bytes);
    }
    // Two quotes, and saturating for the same unreachable reason as in `escape_text`.
    let mut written = Vec::with_capacity(bytes.len().saturating_add(2));
    quote_parameter_into(bytes, &mut written);
    Cow::Owned(written)
}

/// Write a section 3.2 parameter value into `out`, quoting it only when it is required.
pub fn quote_parameter_into(bytes: &[u8], out: &mut Vec<u8>) {
    if !parameter_needs_quoting(bytes) {
        out.extend_from_slice(bytes);
        return;
    }
    out.push(b'"');
    out.extend_from_slice(bytes);
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        PARAMETER_MUST_QUOTE, PARAMETER_NAME_DELIMITERS, ParameterQuoting, TEXT_ESCAPES,
        TEXT_MUST_ESCAPE, escape_text, escape_text_into, parameter_is_representable,
        parameter_name_is_representable, parameter_needs_quoting, property_name_is_representable,
        quote_parameter, quote_parameter_into, text_escape_meaning, text_escape_spelling,
        text_needs_escaping, text_needs_unescaping, unescape_text, unescape_text_into,
        unquote_parameter,
    };
    use crate::report::DiagnosticCode;

    /// The empty value is legal everywhere and is the shape a fold or a bare `NAME:` produces.
    #[test]
    fn an_empty_value_costs_nothing_in_any_direction() {
        assert!(matches!(unescape_text(b""), Cow::Borrowed(_)));
        assert!(matches!(escape_text(b""), Cow::Borrowed(_)));
        assert!(matches!(quote_parameter(b""), Cow::Borrowed(_)));
        assert!(unescape_text(b"").is_empty());
        assert!(escape_text(b"").is_empty());
        assert!(quote_parameter(b"").is_empty());
        assert!(!text_needs_unescaping(b""));
        assert!(!text_needs_escaping(b""));

        let empty = unquote_parameter(b"");
        assert_eq!(empty.value(), b"");
        assert_eq!(empty.quoting(), ParameterQuoting::Bare);
        assert_eq!(empty.diagnostic_code(), None);
    }

    /// The policy table and the mapping table are separate, so their agreement is a claim that
    /// has to be checked rather than one the types make.
    #[test]
    fn every_octet_this_crate_escapes_has_a_spelling_that_reads_back() {
        for octet in TEXT_MUST_ESCAPE {
            let spelling = text_escape_spelling(octet).unwrap();
            assert_eq!(text_escape_meaning(spelling), Some(octet));
        }
        assert_eq!(
            text_escape_spelling(b'\n'),
            Some(b'n'),
            "an authored line feed takes the lowercase spelling"
        );
        for (written, stands_for) in TEXT_ESCAPES {
            assert_eq!(text_escape_meaning(written), Some(stands_for));
        }
        assert_eq!(text_escape_spelling(b'x'), None);
        assert_eq!(text_escape_meaning(b'x'), None);
    }

    /// Both spellings of a line feed resolve, and neither is rewritten into the other: what
    /// distinguishes them is the stored octets, which this module does not touch.
    #[test]
    fn every_defined_substitution_resolves_and_neither_line_feed_spelling_is_preferred() {
        let cases: [(&[u8], &[u8]); 7] = [
            (br"\n", b"\n"),
            (br"\N", b"\n"),
            (br"\,", b","),
            (br"\;", b";"),
            (br"\\", br"\"),
            (br"a\nb\Nc", b"a\nb\nc"),
            (br"\\n", b"\\n"),
        ];
        for (written, resolved) in cases {
            assert_eq!(unescape_text(written).as_ref(), resolved, "{written:?}");
        }

        // Writing picks one spelling, so this is not a round trip and is not used as one.
        assert_eq!(escape_text(unescape_text(br"\N").as_ref()).as_ref(), br"\n");
    }

    /// A value that stops mid-escape is the "no terminator at the end" case for this unit.
    #[test]
    fn a_backslash_with_nothing_left_to_escape_is_kept_as_itself() {
        assert!(!text_needs_unescaping(br"ends with \"));
        assert!(matches!(unescape_text(br"ends with \"), Cow::Borrowed(_)));
        assert_eq!(unescape_text(br"ends with \").as_ref(), br"ends with \");
        assert_eq!(unescape_text(br"\").as_ref(), br"\");
    }

    /// A backslash before an octet the table does not define is a backslash, and it must not
    /// swallow the escape that follows it.
    #[test]
    fn an_undefined_escape_is_passed_through_beside_a_defined_one() {
        assert!(!text_needs_unescaping(br"a\qb"));
        assert_eq!(unescape_text(br"a\qb").as_ref(), br"a\qb");
        assert_eq!(unescape_text(br"a\qb\;c").as_ref(), br"a\qb;c");
    }

    /// Escaping and resolving are inverse over octets this crate wrote, including octets no
    /// decoder would accept: neither direction inspects a lead byte.
    #[test]
    fn resolving_what_this_crate_escaped_returns_the_octets_it_started_with() {
        let cases: [&[u8]; 6] = [
            b"",
            b"plain",
            b"semi; comma, back\\ feed\n",
            b"\\\\\\",
            b"\n\n\n",
            // CP1252 accented octets, which are not UTF-8 and still round trip.
            b"note \xe9\xe8\xfc end",
        ];
        for original in cases {
            let written = escape_text(original);
            assert_eq!(
                unescape_text(written.as_ref()).as_ref(),
                original,
                "{original:?}"
            );
        }
    }

    /// The buffer forms append rather than replace, which is what makes them usable from a
    /// caller assembling one line out of several pieces.
    #[test]
    fn the_buffer_forms_append_to_what_is_already_there() {
        let mut out: Vec<u8> = vec![b'>'];
        escape_text_into(b"a,b", &mut out);
        assert_eq!(out.as_slice(), br">a\,b");

        let mut resolved: Vec<u8> = vec![b'>'];
        unescape_text_into(br"a\,b", &mut resolved);
        assert_eq!(resolved.as_slice(), b">a,b");

        let mut parameter: Vec<u8> = vec![b'='];
        quote_parameter_into(b"a,b", &mut parameter);
        quote_parameter_into(b"plain", &mut parameter);
        assert_eq!(parameter.as_slice(), b"=\"a,b\"plain");
    }

    /// A `TEXT` value has no length limit of its own, so the long case is a value made of
    /// nothing but escapes: the capacity arithmetic and the single pass both have to hold.
    #[test]
    fn a_value_made_entirely_of_escapes_resolves_in_one_pass() {
        const COUNT: usize = 4096;

        let mut written: Vec<u8> = Vec::with_capacity(COUNT.saturating_mul(2));
        for _ in 0..COUNT {
            written.extend_from_slice(br"\n");
        }

        let resolved = unescape_text(&written);
        assert_eq!(resolved.len(), COUNT);
        assert!(resolved.iter().all(|octet| *octet == b'\n'));
        assert_eq!(escape_text(resolved.as_ref()).as_ref(), written.as_slice());
    }

    /// The case this unit answers with a value that a caller turns into a diagnostic. Nothing
    /// is refused and no octet is lost; the caller adds the offset and the severity.
    #[test]
    fn an_unterminated_quote_is_reported_as_a_value_and_keeps_its_octets() {
        let cut_short = unquote_parameter(b"\"Europe/Paris");
        assert_eq!(cut_short.value(), b"Europe/Paris");
        assert_eq!(cut_short.quoting(), ParameterQuoting::Unterminated);
        assert!(cut_short.is_unterminated());
        assert_eq!(
            cut_short.diagnostic_code(),
            Some(DiagnosticCode::UnterminatedQuotedParameter)
        );

        // One `DQUOTE` opens a quote it never closes; it is not an empty quoted value.
        let lone = unquote_parameter(b"\"");
        assert_eq!(lone.value(), b"");
        assert!(lone.is_unterminated());
    }

    /// A closed pair comes off, and an empty pair is an empty value rather than a violation.
    #[test]
    fn a_closed_quote_pair_comes_off_and_a_bare_value_is_left_alone() {
        let quoted = unquote_parameter(b"\"a,b:c;d\"");
        assert_eq!(quoted.value(), b"a,b:c;d");
        assert_eq!(quoted.quoting(), ParameterQuoting::Quoted);
        assert_eq!(quoted.diagnostic_code(), None);

        let empty_pair = unquote_parameter(b"\"\"");
        assert_eq!(empty_pair.value(), b"");
        assert_eq!(empty_pair.quoting(), ParameterQuoting::Quoted);

        let bare = unquote_parameter(b"TENTATIVE");
        assert_eq!(bare.value(), b"TENTATIVE");
        assert_eq!(bare.quoting(), ParameterQuoting::Bare);
    }

    /// Quoting is added only where section 3.2 forces it, so a value that never needed quotes
    /// is written back without any.
    #[test]
    fn a_parameter_is_quoted_only_where_the_grammar_forces_it() {
        for octet in PARAMETER_MUST_QUOTE {
            let value = [b'a', octet, b'b'];
            assert!(parameter_needs_quoting(&value));
            let written = quote_parameter(&value);
            assert_eq!(written.first(), Some(&b'"'));
            assert_eq!(written.last(), Some(&b'"'));
            assert_eq!(
                unquote_parameter(written.as_ref()).value(),
                value.as_slice()
            );
        }

        assert!(!parameter_needs_quoting(b"TENTATIVE"));
        assert!(matches!(quote_parameter(b"TENTATIVE"), Cow::Borrowed(_)));
    }

    /// Some octets cannot be written as a parameter value at all. Saying so is this module's
    /// whole part in it; the refusal belongs to whoever owns the property.
    #[test]
    fn octets_with_no_parameter_form_are_named_rather_than_refused() {
        assert!(parameter_is_representable(b"Europe/Paris"));
        assert!(
            parameter_is_representable(b"a\tb"),
            "HTAB is whitespace, not a control character, per section 3.1"
        );
        assert!(!parameter_is_representable(b"say \"hi\""));
        assert!(!parameter_is_representable(b"line\r\nATTENDEE:x"));
        assert!(!parameter_is_representable(b"bell\x07"));
        assert!(!parameter_is_representable(b"delete\x7f"));

        // Named, not refused: the octets still come back out.
        assert_eq!(quote_parameter(b"say \"hi\"").as_ref(), b"say \"hi\"");
    }

    /// A name is representable when the reader would give it back whole. Every delimiter that
    /// hands the rest of the name to something else is refused; the vendor spellings producers
    /// actually write are not, because those do come back.
    #[test]
    fn a_parameter_name_that_would_not_read_back_whole_is_named_as_unwritable() {
        assert!(parameter_name_is_representable(b"TZID"));
        assert!(parameter_name_is_representable(b"X-VENDOR_FLAG.2"));
        assert!(!parameter_name_is_representable(b""));
        for octet in PARAMETER_NAME_DELIMITERS {
            let name = [b'X', octet, b'A'];
            assert!(!parameter_name_is_representable(&name), "{octet:?}");
        }
        assert!(!parameter_name_is_representable(b"X\r\nATTENDEE"));
        assert!(!parameter_name_is_representable(b"X\x07"));
    }

    /// The same claim one level up: a name the reader would hand back in pieces is a name this
    /// crate declines to author, and the empty one reads back as a blank line rather than as a
    /// property nobody named.
    #[test]
    fn a_property_name_that_would_not_read_back_whole_is_named_as_unwritable() {
        assert!(property_name_is_representable(b"SUMMARY"));
        assert!(property_name_is_representable(
            b"X-MICROSOFT-CDO-BUSYSTATUS"
        ));
        assert!(property_name_is_representable(b"VEVENT"));
        assert!(!property_name_is_representable(b""));
        assert!(!property_name_is_representable(b"SUMMARY:x"));
        assert!(!property_name_is_representable(b"SUMMARY;X-A=1"));
        assert!(!property_name_is_representable(b"SUM\r\nATTENDEE"));
        assert!(!property_name_is_representable(b"SUM\x07"));
        assert!(
            property_name_is_representable(b"X-VENDOR_FLAG.2"),
            "a spelling producers write and this crate reads back is one it may write"
        );
    }
}
