// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The lexical layer: what the octets of an XML document mean before any vocabulary does.
//!
//! Every function here answers a question XML 1.0 and XML Namespaces 1.0 answer, and none of
//! them knows what the document is about. Where an element name ends, whether an attribute name
//! declares a prefix, how a qualified name splits, which encodings a declaration may name — a
//! `DAV:` body, a CardDAV body and a body of a vocabulary nobody has written yet all answer
//! these identically, which is what makes this a layer rather than a helper module.

use xmlparser::Stream;

use super::fault::{XmlFault, XmlSyntax};

/// The URI XML Namespaces 1.0 section 3 binds the `xml` prefix to, declared or not.
///
/// RFC 4918 section 14 writes `xml:lang` on `DAV:displayname` and on `responsedescription`, so a
/// reader that demanded a declaration for it would refuse bodies the specification writes itself.
pub(crate) const XML_URI: &[u8] = b"http://www.w3.org/XML/1998/namespace";

/// The prefix bound to [`XML_URI`] without a declaration.
pub(crate) const XML_PREFIX: &[u8] = b"xml";

/// The attribute name that declares a default namespace.
pub(crate) const XMLNS: &[u8] = b"xmlns";

/// The prefix of an attribute name that declares a prefix.
pub(crate) const XMLNS_COLON: &[u8] = b"xmlns:";

/// The URI of no namespace at all, which is what an unprefixed attribute is in.
pub(crate) const NO_NAMESPACE: &[u8] = b"";

/// The UTF-8 byte order mark, which a peer is free to put in front of a body.
pub(crate) const BYTE_ORDER_MARK: &[u8] = b"\xef\xbb\xbf";

/// The head of an XML declaration, which is the one processing instruction this layer reads.
pub(crate) const DECLARATION_OPEN: &[u8] = b"<?xml";

/// The head of a comment.
pub(crate) const COMMENT_OPEN: &[u8] = b"<!--";

/// The tail of a comment.
pub(crate) const COMMENT_CLOSE: &[u8] = b"-->";

/// The head of a `CDATA` section, which is character data rather than markup that ends a run.
pub(crate) const CDATA_OPEN: &[u8] = b"<![CDATA[";

/// The tail of a `CDATA` section.
pub(crate) const CDATA_CLOSE: &[u8] = b"]]>";

/// Whether an octet is XML whitespace.
pub(crate) const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Where the whitespace starting at `from` ends.
pub(crate) fn space_end(body: &[u8], from: usize) -> usize {
    let mut at = from;
    while body.get(at).is_some_and(|byte| is_space(*byte)) {
        at = at.saturating_add(1);
    }
    at
}

/// Whether an octet ends an element name.
pub(crate) fn is_name_end(byte: u8) -> bool {
    is_space(byte) || matches!(byte, b'/' | b'>')
}

/// Whether an octet ends an attribute name, which `=` does and an element name's does not.
pub(crate) fn is_attribute_name_end(byte: u8) -> bool {
    is_name_end(byte) || byte == b'='
}

/// Validate one already-delimited XML 1.0 `Name`.
///
/// The whole-document lexer cannot run when a `calendar-data` payload carries arbitrary
/// octets. Its public stream primitive keeps the same NameStartChar/NameChar authority on that
/// fallback path without copying or teaching this wrapper a second spelling of the production.
pub(crate) fn check_name(name: &[u8]) -> Result<(), XmlFault> {
    let text = core::str::from_utf8(name).map_err(|_| XmlSyntax::ForbiddenCharacter)?;
    let mut stream = Stream::from(text);
    let _ = stream
        .consume_name()
        .map_err(|_| XmlFault::from(XmlSyntax::Malformed))?;
    if stream.at_end() {
        Ok(())
    } else {
        Err(XmlSyntax::Malformed.into())
    }
}

/// The first position of `needle` in `haystack`.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The prefix an attribute name declares, if it is a namespace declaration at all.
///
/// `xmlns` declares the default namespace, whose prefix is the empty one; `xmlns:p` declares
/// `p`. Everything else is an ordinary attribute.
pub(crate) fn declared_prefix(name: &[u8]) -> Option<&[u8]> {
    if name == XMLNS {
        return Some(NO_NAMESPACE);
    }
    name.strip_prefix(XMLNS_COLON)
        .filter(|prefix| !prefix.is_empty())
}

/// Split a name into its prefix and its local part.
pub(crate) fn split_name(name: &[u8]) -> Result<(&[u8], &[u8]), XmlFault> {
    let Some(colon) = name.iter().position(|byte| *byte == b':') else {
        return Ok((NO_NAMESPACE, name));
    };
    let prefix = name.get(..colon).ok_or(XmlSyntax::Malformed)?;
    let local = name
        .get(colon.saturating_add(1)..)
        .ok_or(XmlSyntax::Malformed)?;
    // XML Namespaces 1.0 section 4 gives a qualified name exactly one colon. Two is a name no
    // vocabulary defines, and reading it as a prefix and a local name with a colon in it is the
    // kind of guess this layer does not make.
    if prefix.is_empty() || local.is_empty() || local.contains(&b':') {
        return Err(XmlSyntax::Malformed.into());
    }
    Ok((prefix, local))
}

/// Refuse a declaration naming an encoding this layer does not read.
///
/// An absent encoding declaration is UTF-8, which XML 1.0 section 4.3.3 makes the default for an
/// entity that carries none, and which RFC 4918 section 20 requires of a `DAV:` body anyway.
pub(crate) fn check_encoding(declaration: &[u8]) -> Result<(), XmlFault> {
    let Some(at) = find(declaration, b"encoding") else {
        return Ok(());
    };
    let rest = declaration
        .get(at.saturating_add(b"encoding".len())..)
        .unwrap_or(&[]);
    let named = quoted_value(rest).ok_or(XmlSyntax::Malformed)?;
    if named.eq_ignore_ascii_case(b"utf-8") {
        Ok(())
    } else {
        Err(XmlSyntax::Encoding.into())
    }
}

/// The value of an `= "..."` at the head of `rest`, quotes excluded.
///
/// Both quote characters, because Radicale writes its declaration with apostrophes and a reader
/// that took only `"` would refuse the body of a server people run.
fn quoted_value(rest: &[u8]) -> Option<&[u8]> {
    let at = space_end(rest, 0);
    if rest.get(at) != Some(&b'=') {
        return None;
    }
    let at = space_end(rest, at.saturating_add(1));
    let quote = *rest.get(at)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let opens = at.saturating_add(1);
    let tail = rest.get(opens..)?;
    let end = tail.iter().position(|byte| *byte == quote)?;
    tail.get(..end)
}

#[cfg(test)]
mod tests {
    use super::{
        XmlFault, XmlSyntax, check_encoding, check_name, declared_prefix, is_attribute_name_end,
        is_name_end, space_end, split_name,
    };

    #[test]
    fn a_name_uses_the_private_lexers_complete_xml_production() {
        assert!(check_name(b"D:multistatus").is_ok());
        assert!(check_name("Διακοπές".as_bytes()).is_ok());
        assert_eq!(
            check_name(b"1multistatus"),
            Err(XmlFault::Syntax(XmlSyntax::Malformed))
        );
        assert_eq!(
            check_name(b"name=value"),
            Err(XmlFault::Syntax(XmlSyntax::Malformed))
        );
    }

    #[test]
    fn a_qualified_name_has_exactly_one_colon() {
        assert_eq!(
            split_name(b"multistatus"),
            Ok((b"".as_slice(), b"multistatus".as_slice()))
        );
        assert_eq!(
            split_name(b"D:href"),
            Ok((b"D".as_slice(), b"href".as_slice()))
        );
        assert_eq!(
            split_name(b"a:b:c"),
            Err(XmlFault::Syntax(XmlSyntax::Malformed))
        );
        assert_eq!(
            split_name(b":href"),
            Err(XmlFault::Syntax(XmlSyntax::Malformed))
        );
    }

    #[test]
    fn a_declaration_is_told_apart_from_an_ordinary_attribute() {
        assert_eq!(declared_prefix(b"xmlns"), Some(b"".as_slice()));
        assert_eq!(declared_prefix(b"xmlns:D"), Some(b"D".as_slice()));
        assert_eq!(declared_prefix(b"xmlns:"), None);
        assert_eq!(declared_prefix(b"name"), None);
    }

    #[test]
    fn only_utf_eight_is_a_declared_encoding_this_layer_reads() {
        assert!(check_encoding(b" version=\"1.0\"").is_ok());
        assert!(check_encoding(b" version='1.0' encoding='utf-8' ").is_ok());
        assert_eq!(
            check_encoding(b" encoding=\"iso-8859-1\""),
            Err(XmlFault::Syntax(XmlSyntax::Encoding))
        );
    }

    #[test]
    fn a_name_ends_where_the_production_says_it_does() {
        assert!(is_name_end(b'>'));
        assert!(is_name_end(b'/'));
        assert!(!is_name_end(b'='));
        assert!(is_attribute_name_end(b'='));
        assert_eq!(space_end(b"   x", 0), 3);
    }
}
