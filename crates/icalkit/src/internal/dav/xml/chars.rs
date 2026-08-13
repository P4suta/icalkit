// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Characters, references and attribute-value normalization: XML 1.0 sections 2.2, 3.3.3 and 4.6.
//!
//! Nothing here writes to a sink and nothing here knows an element from another. What it owns is
//! the answer to three questions any XML processor has to answer the same way: is this run of
//! octets a run of characters, what does this reference resolve to, and what value does an
//! attribute actually have.
//!
//! The escaping *table* is here too, as data, while the plumbing that pushes it into a caller's
//! sink stays above: the rules are XML's and the sink is `ical-dav`'s.

use alloc::vec::Vec;

use ical_core::LimitExceeded;

use super::fault::{XmlFault, XmlSyntax};

/// The most octets a reference may occupy, `&` and `;` included.
///
/// `&#x10FFFF;` is ten, and no predefined entity name is longer than `apos`. A `&` followed by
/// megabytes of digits is not a reference anybody wrote, and scanning for its terminator without
/// a ceiling is work an attacker chooses the size of.
const MAX_REFERENCE_BYTES: usize = 12;

/// Whether a code point is one XML 1.0 section 2.2's `Char` production admits.
const fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

/// Refuse octets no conformant XML processor would deliver as characters.
///
/// Two rules in one pass, because they are one question — "is this a run of characters?" — asked
/// of octets a peer chose. XML 1.0 section 4.3.3 makes the document entity UTF-8, and section
/// 2.2's `Char` production excludes `U+0000`, the C0 controls other than tab, line feed and
/// carriage return, the surrogates, and `U+FFFE`/`U+FFFF`.
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
/// The value between the quotes is not the value the attribute has. Section 3.3.3 resolves
/// character and entity references in it and replaces every literal tab, line feed and carriage
/// return — the last two after section 2.11 has already folded `CRLF` to one line feed — with a
/// single space, all before the value is delivered. Handing back the raw octets made a
/// `comp-filter name="VE&#78;T"` name a component spelled `VE&#78;T` here and `VENT` in every
/// conformant processor, so two implementations disagreed about which components a hostile query
/// selects; it also made the request round trip grow by four octets a hop, because the encoder
/// escaped the `&` that the reader had never resolved.
///
/// Every attribute this workspace writes is `CDATA`-typed, so no further whitespace collapsing
/// applies.
pub(crate) fn normalize_attribute(raw: &[u8], out: &mut Vec<u8>) -> Result<(), XmlFault> {
    let mut at = 0;
    while let Some(&byte) = raw.get(at) {
        match byte {
            b'&' => {
                at = push_reference(raw, at, out)?;
                continue;
            },
            b'\r' => {
                push(out, b" ")?;
                // Section 2.11 makes `CRLF` one line break, and section 3.3.3 then makes that
                // one break one space rather than two.
                let paired = raw.get(at.saturating_add(1)) == Some(&b'\n');
                at = at.saturating_add(if paired { 2 } else { 1 });
                continue;
            },
            b'\n' | b'\t' => push(out, b" ")?,
            _ => push(out, &[byte])?,
        }
        at = at.saturating_add(1);
    }
    check_chars(out)
}

/// Resolve one reference into `out` and answer where the octet after its `;` is.
///
/// Nothing beyond the five entities XML 1.0 section 4.6 predefines is resolvable, because this
/// layer accepts no `DOCTYPE` and so nothing can ever have been declared. An undefined name is
/// refused rather than passed through: a reader that emitted `&file;` unchanged would hand its
/// caller octets the peer did not write, and one that dropped it would hide an attempt.
pub(crate) fn push_reference(
    raw: &[u8],
    start: usize,
    out: &mut Vec<u8>,
) -> Result<usize, XmlFault> {
    let window = raw
        .get(start..)
        .and_then(|rest| rest.get(..MAX_REFERENCE_BYTES.min(rest.len())))
        .ok_or(XmlSyntax::Malformed)?;
    let semicolon = window
        .iter()
        .position(|byte| *byte == b';')
        .ok_or(XmlSyntax::Malformed)?;
    let name = window.get(1..semicolon).ok_or(XmlSyntax::Malformed)?;
    match name {
        b"amp" => push(out, b"&")?,
        b"lt" => push(out, b"<")?,
        b"gt" => push(out, b">")?,
        b"quot" => push(out, b"\"")?,
        b"apos" => push(out, b"'")?,
        _ => push_character_reference(name, out)?,
    }
    Ok(start
        .saturating_add(semicolon)
        .saturating_add(1)
        .min(raw.len()))
}

/// Resolve a numeric character reference, which is what carries a `CR` past section 2.11.
fn push_character_reference(name: &[u8], out: &mut Vec<u8>) -> Result<(), XmlFault> {
    let digits = name.strip_prefix(b"#").ok_or(XmlSyntax::UndefinedEntity)?;
    let (radix, body) = match digits.strip_prefix(b"x") {
        Some(hex) => (16_u32, hex),
        None => (10_u32, digits),
    };
    if body.is_empty() {
        return Err(XmlSyntax::Malformed.into());
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
    let character = char::from_u32(code)
        .filter(|found| is_xml_char(*found))
        .ok_or(XmlSyntax::ForbiddenCharacter)?;
    let mut encoded = [0_u8; 4];
    push(out, character.encode_utf8(&mut encoded).as_bytes())
}

/// What one octet is written as, or `None` when it is written as itself.
///
/// `CR` becomes `&#13;` because a literal one would be folded to `LF` by XML 1.0 section 2.11
/// before any reader saw it, and a reference is markup resolved *after* normalization. `>` is
/// escaped although XML does not require it, so that a `]]>` inside a value cannot end a `CDATA`
/// section this workspace never opens. Inside quotes the three whitespace characters section
/// 3.3.3 would replace with a space are written as references instead, so they survive.
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

    use super::{XmlFault, XmlSyntax, check_chars, escape_for, normalize_attribute};

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
