// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Characters, references and attribute-value normalization: XML 1.0 sections 2.2, 3.3.3 and 4.6.
//!
//! Nothing here writes to a sink and nothing here knows an element from another.
//! What it owns is the answer to three questions any XML processor has to answer the same way: is this run of octets a run of characters, what does this reference resolve to, and what value does an attribute actually have.
//!
//! The escaping *table* is here too, as data, while the plumbing that pushes it into a caller's sink stays above: the rules are XML's and the sink is `ical-dav`'s.

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::hash::{Hash, Hasher};

use ical_core::LimitExceeded;

use super::fault::{XmlFault, XmlSyntax};

/// The most octets a reference may occupy, `&` and `;` included.
///
/// `&#x10FFFF;` is ten, and no predefined entity name is longer than `apos`.
/// A `&` followed by megabytes of digits is not a reference anybody wrote, and scanning for its terminator without a ceiling is work an attacker chooses the size of.
const MAX_REFERENCE_BYTES: usize = 12;

#[derive(Clone, Copy, Debug)]
enum AttributeSource<'a> {
    Literal(&'a [u8]),
    Encoded(&'a [u8]),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AttributeValue<'a> {
    source: AttributeSource<'a>,
    length: usize,
}

impl<'a> AttributeValue<'a> {
    pub(crate) const fn literal(bytes: &'a [u8]) -> Self {
        Self {
            source: AttributeSource::Literal(bytes),
            length: bytes.len(),
        }
    }

    pub(crate) fn parse(raw: &'a [u8]) -> Result<Self, XmlFault> {
        check_chars(raw)?;
        if !raw
            .iter()
            .any(|byte| matches!(byte, b'&' | b'\r' | b'\n' | b'\t'))
        {
            return Ok(Self::literal(raw));
        }
        let mut value = Self {
            source: AttributeSource::Encoded(raw),
            length: 0,
        };
        for byte in value.bytes() {
            byte?;
            value.length = value.length.checked_add(1).ok_or(XmlSyntax::Malformed)?;
        }
        Ok(value)
    }

    pub(crate) const fn len(self) -> usize {
        self.length
    }

    pub(crate) const fn contiguous(self) -> Option<&'a [u8]> {
        match self.source {
            AttributeSource::Literal(bytes) => Some(bytes),
            AttributeSource::Encoded(_) => None,
        }
    }

    pub(crate) fn is(self, bytes: &[u8]) -> bool {
        match self.contiguous() {
            Some(literal) => literal == bytes,
            None => self.bytes().eq(bytes.iter().copied().map(Ok)),
        }
    }

    pub(crate) fn bytes(self) -> AttributeBytes<'a> {
        AttributeBytes {
            source: self.source,
            at: 0,
            pending: [0; 4],
            pending_at: 0,
            pending_len: 0,
        }
    }
}

impl PartialEq for AttributeValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for AttributeValue<'_> {}

impl PartialOrd for AttributeValue<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AttributeValue<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.contiguous(), other.contiguous()) {
            (Some(left), Some(right)) => left.cmp(right),
            _ => self.bytes().cmp(other.bytes()),
        }
    }
}

impl Hash for AttributeValue<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.length.hash(state);
        for byte in self.bytes() {
            byte.hash(state);
        }
    }
}

#[derive(Debug)]
pub(crate) struct AttributeBytes<'a> {
    source: AttributeSource<'a>,
    at: usize,
    pending: [u8; 4],
    pending_at: usize,
    pending_len: usize,
}

impl Iterator for AttributeBytes<'_> {
    type Item = Result<u8, XmlSyntax>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pending_at < self.pending_len {
            let byte = self.pending.get(self.pending_at).copied();
            self.pending_at = self.pending_at.saturating_add(1);
            return Some(byte.ok_or(XmlSyntax::Malformed));
        }
        let raw = match self.source {
            AttributeSource::Literal(raw) | AttributeSource::Encoded(raw) => raw,
        };
        let byte = *raw.get(self.at)?;
        if matches!(self.source, AttributeSource::Literal(_)) {
            self.at = self.at.saturating_add(1);
            return Some(Ok(byte));
        }
        match byte {
            b'&' => match read_reference(raw, self.at) {
                Ok((character, after)) => {
                    self.at = after;
                    self.pending_len = character.encode_utf8(&mut self.pending).len();
                    self.pending_at = 1;
                    Some(self.pending.first().copied().ok_or(XmlSyntax::Malformed))
                },
                Err(error) => {
                    self.at = raw.len();
                    Some(Err(error))
                },
            },
            b'\r' => {
                let paired = raw.get(self.at.saturating_add(1)) == Some(&b'\n');
                self.at = self.at.saturating_add(if paired { 2 } else { 1 });
                Some(Ok(b' '))
            },
            b'\n' | b'\t' => {
                self.at = self.at.saturating_add(1);
                Some(Ok(b' '))
            },
            _ => {
                self.at = self.at.saturating_add(1);
                Some(Ok(byte))
            },
        }
    }
}

/// Whether a code point is one XML 1.0 section 2.2's `Char` production admits.
const fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

/// Refuse octets no conformant XML processor would deliver as characters.
///
/// Two rules in one pass, because they are one question — "is this a run of characters?"
/// — asked of octets a peer chose.
/// XML 1.0 section 4.3.3 makes the document entity UTF-8, and section 2.2's `Char` production excludes `U+0000`, the C0 controls other than tab, line feed and carriage return, the surrogates, and `U+FFFE`/`U+FFFF`.
pub(crate) fn check_chars(bytes: &[u8]) -> Result<(), XmlFault> {
    let text = core::str::from_utf8(bytes).map_err(|_| XmlSyntax::ForbiddenCharacter)?;
    if text.chars().all(is_xml_char) {
        Ok(())
    } else {
        Err(XmlSyntax::ForbiddenCharacter.into())
    }
}

/// Append to a buffer that reports a refusing allocator rather than aborting on one.
pub(crate) fn push(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), XmlFault> {
    out.try_reserve(bytes.len())
        .map_err(|_| LimitExceeded::Text)?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Normalize an attribute value the way XML 1.0 section 3.3.3 requires, into `out`.
///
/// The value between the quotes is not the value the attribute has.
/// Section 3.3.3 resolves character and entity references in it and replaces every literal tab, line feed and carriage return — the last two after section 2.11 has already folded `CRLF` to one line feed — with a single space, all before the value is delivered.
/// Handing back the raw octets made a `comp-filter name="VE&#78;T"` name a component spelled `VE&#78;T` here and `VENT` in every conformant processor, so two implementations disagreed about which components a hostile query selects; it also made the request round trip grow by four octets a hop, because the encoder escaped the `&` that the reader had never resolved.
///
/// Every attribute this workspace writes is `CDATA`-typed, so no further whitespace collapsing applies.
pub(crate) fn normalize_attribute(raw: &[u8], out: &mut Vec<u8>) -> Result<(), XmlFault> {
    let value = AttributeValue::parse(raw)?;
    for byte in value.bytes() {
        push(out, &[byte?])?;
    }
    check_chars(out)
}

/// Resolve one reference into `out` and answer where the octet after its `;` is.
///
/// Nothing beyond the five entities XML 1.0 section 4.6 predefines is resolvable, because this layer accepts no `DOCTYPE` and so nothing can ever have been declared.
/// An undefined name is refused rather than passed through: a reader that emitted `&file;` unchanged would hand its caller octets the peer did not write, and one that dropped it would hide an attempt.
pub(crate) fn push_reference(
    raw: &[u8],
    start: usize,
    out: &mut Vec<u8>,
) -> Result<usize, XmlFault> {
    let (character, after) = read_reference(raw, start)?;
    let mut encoded = [0; 4];
    push(out, character.encode_utf8(&mut encoded).as_bytes())?;
    Ok(after)
}

fn read_reference(raw: &[u8], start: usize) -> Result<(char, usize), XmlSyntax> {
    let window = raw
        .get(start..)
        .and_then(|rest| rest.get(..MAX_REFERENCE_BYTES.min(rest.len())))
        .ok_or(XmlSyntax::Malformed)?;
    let semicolon = window
        .iter()
        .position(|byte| *byte == b';')
        .ok_or(XmlSyntax::Malformed)?;
    let name = window.get(1..semicolon).ok_or(XmlSyntax::Malformed)?;
    let character = match name {
        b"amp" => '&',
        b"lt" => '<',
        b"gt" => '>',
        b"quot" => '"',
        b"apos" => '\'',
        _ => character_reference(name)?,
    };
    let after = start
        .saturating_add(semicolon)
        .saturating_add(1)
        .min(raw.len());
    Ok((character, after))
}

/// Resolve a numeric character reference, which is what carries a `CR` past section 2.11.
fn character_reference(name: &[u8]) -> Result<char, XmlSyntax> {
    let digits = name.strip_prefix(b"#").ok_or(XmlSyntax::UndefinedEntity)?;
    let (radix, body) = match digits.strip_prefix(b"x") {
        Some(hex) => (16_u32, hex),
        None => (10_u32, digits),
    };
    if body.is_empty() {
        return Err(XmlSyntax::Malformed);
    }
    let mut code: u32 = 0;
    for byte in body {
        let digit = char::from(*byte)
            .to_digit(radix)
            .ok_or(XmlSyntax::Malformed)?;
        code = code
            .checked_mul(radix)
            .and_then(|shifted| shifted.checked_add(digit))
            .ok_or(XmlSyntax::ForbiddenCharacter)?;
    }
    char::from_u32(code)
        .filter(|found| is_xml_char(*found))
        .ok_or(XmlSyntax::ForbiddenCharacter)
}

/// What one octet is written as, or `None` when it is written as itself.
///
/// `CR` becomes `&#13;` because a literal one would be folded to `LF` by XML 1.0 section 2.11 before any reader saw it, and a reference is markup resolved *after* normalization.
/// `>` is escaped although XML does not require it, so that a `]]>` inside a value cannot end a `CDATA` section this workspace never opens.
/// Inside quotes the three whitespace characters section 3.3.3 would replace with a space are written as references instead, so they survive.
pub(crate) const fn escape_for(byte: u8, in_attribute: bool) -> Option<&'static [u8]> {
    match byte {
        b'&' => Some(b"&amp;"),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'\r' => Some(b"&#13;"),
        b'"' if in_attribute => Some(b"&quot;"),
        b'\n' if in_attribute => Some(b"&#10;"),
        b'\t' if in_attribute => Some(b"&#9;"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{
        AttributeValue, XmlFault, XmlSyntax, check_chars, escape_for, normalize_attribute,
    };

    #[test]
    fn normalized_views_match_literal_octets() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"a\r\nb\tc\n", b"a b c "),
            (b"&#9;&#10;&#13;", b"\t\n\r"),
            (b"&amp;&lt;&gt;&quot;&apos;", b"&<>\"'"),
            (b"&#x1F600;", b"\xf0\x9f\x98\x80"),
            (b"literal", b"literal"),
        ];
        for (raw, expected) in cases {
            let value = AttributeValue::parse(raw).unwrap();
            assert_eq!(value.len(), expected.len());
            assert_eq!(value, AttributeValue::literal(expected));
            assert!(value.is(expected));
            assert_eq!(
                value.bytes().collect::<Result<Vec<_>, _>>().unwrap(),
                *expected
            );
        }
    }

    #[test]
    fn an_attribute_value_is_the_value_and_not_the_octets_between_the_quotes() {
        let mut out: Vec<u8> = Vec::new();
        normalize_attribute(b"VE&#78;T", &mut out).unwrap();
        assert_eq!(out, b"VENT", "a conformant processor reads this name");

        let mut spaced: Vec<u8> = Vec::new();
        normalize_attribute(b"a\r\nb\tc", &mut spaced).unwrap();
        assert_eq!(spaced, b"a b c", "one CRLF is one break and one space");
    }

    #[test]
    fn nothing_beyond_the_five_predefined_entities_resolves() {
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            normalize_attribute(b"&file;", &mut out),
            Err(XmlFault::Syntax(XmlSyntax::UndefinedEntity)),
            "with no DOCTYPE accepted, nothing can ever have been declared"
        );
    }

    #[test]
    fn a_reference_to_a_code_point_xml_excludes_is_refused() {
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            normalize_attribute(b"&#0;", &mut out),
            Err(XmlFault::Syntax(XmlSyntax::ForbiddenCharacter))
        );
        assert_eq!(
            check_chars(b"\xff\xfe"),
            Err(XmlFault::Syntax(XmlSyntax::ForbiddenCharacter)),
            "the document entity is UTF-8, section 4.3.3"
        );
    }

    #[test]
    fn the_escaping_table_differs_inside_quotes_and_only_there() {
        assert_eq!(escape_for(b'\r', false), Some(b"&#13;".as_slice()));
        assert_eq!(escape_for(b'"', false), None);
        assert_eq!(escape_for(b'"', true), Some(b"&quot;".as_slice()));
        assert_eq!(escape_for(b'\t', false), None);
        assert_eq!(escape_for(b'\t', true), Some(b"&#9;".as_slice()));
    }
}
