// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Borrowed syntax checks for [RFC 6068](https://www.rfc-editor.org/rfc/rfc6068.html#section-2) agent URIs.

use core::str;

use super::{DecodeValue as _, UriValue};

#[derive(Clone, Copy)]
struct UriCursor<'a> {
    remaining: &'a [u8],
}

impl<'a> UriCursor<'a> {
    const fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }

    fn octet(&mut self) -> Option<u8> {
        let (&first, tail) = self.remaining.split_first()?;
        self.remaining = tail;
        if first != b'%' {
            return Some(first);
        }
        let (encoded, rest) = tail.split_at_checked(2)?;
        self.remaining = rest;
        u8::from_str_radix(str::from_utf8(encoded).ok()?, 16).ok()
    }

    fn character(&mut self) -> Option<char> {
        let first = self.octet()?;
        let width = match first {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return None,
        };
        let mut encoded = [first, 0, 0, 0];
        for octet in encoded.iter_mut().take(width).skip(1) {
            *octet = self.octet()?;
        }
        str::from_utf8(encoded.get(..width)?).ok()?.chars().next()
    }

    fn peek(self) -> Option<char> {
        let mut lookahead = self;
        lookahead.character()
    }

    fn literal(&mut self, expected: u8) -> Option<()> {
        let (&first, tail) = self.remaining.split_first()?;
        if first != expected {
            return None;
        }
        self.remaining = tail;
        Some(())
    }

    fn atom(&mut self, delimiter: u8) -> Option<()> {
        let mut complete = false;
        while !self.remaining.is_empty() && self.remaining.first() != Some(&delimiter) {
            match self.character()? {
                '.' if complete => complete = false,
                character if is_atext(character) => complete = true,
                _ => return None,
            }
        }
        complete.then_some(())
    }

    fn quoted_local(&mut self) -> Option<()> {
        if self.character()? != '"' {
            return None;
        }
        loop {
            match self.character()? {
                '"' => return Some(()),
                '\\' => {
                    // RFC 6068 section 6.2 uses a quoted pair for mailbox whitespace.
                    if !matches!(self.character()?, '\t' | ' '..='~') {
                        return None;
                    }
                },
                character
                    if !character.is_ascii()
                        || matches!(character, '!' | '#'..='[' | ']'..='~') => {},
                _ => return None,
            }
        }
    }

    fn domain_literal(&mut self) -> Option<()> {
        if self.character()? != '[' {
            return None;
        }
        loop {
            match self.character()? {
                ']' => return Some(()),
                '!'..='Z' | '^'..='~' => {},
                _ => return None,
            }
        }
    }

    fn mailbox(&mut self) -> Option<()> {
        if self.peek() == Some('"') {
            self.quoted_local()?;
        } else {
            self.atom(b'@')?;
        }
        self.literal(b'@')?;
        if self.peek() == Some('[') {
            self.domain_literal()
        } else {
            self.atom(b',')
        }
    }
}

fn is_atext(character: char) -> bool {
    !character.is_ascii()
        || character.is_ascii_alphanumeric()
        || matches!(
            character,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
        )
}

fn mailboxes_are_valid(bytes: &[u8]) -> Option<()> {
    if bytes
        .iter()
        .any(|octet| matches!(octet, b'/' | b'[' | b']'))
    {
        return None;
    }
    let mut cursor = UriCursor::new(bytes);
    while !cursor.remaining.is_empty() {
        cursor.mailbox()?;
        if cursor.remaining.is_empty() {
            return Some(());
        }
        cursor.literal(b',')?;
        if cursor.remaining.is_empty() {
            return None;
        }
    }
    Some(())
}

fn header_part_is_valid(bytes: &[u8]) -> bool {
    if !bytes.iter().all(|octet| {
        octet.is_ascii_alphanumeric()
            || matches!(
                octet,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'%'
                    | b'!'
                    | b'$'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b':'
                    | b'@'
            )
    }) {
        return false;
    }
    let mut cursor = UriCursor::new(bytes);
    while !cursor.remaining.is_empty() {
        if cursor.character().is_none() {
            return false;
        }
    }
    true
}

fn headers_are_valid(bytes: &[u8]) -> bool {
    bytes.split(|octet| *octet == b'&').all(|field| {
        let Some(separator) = field.iter().position(|octet| *octet == b'=') else {
            return false;
        };
        let (name, after) = field.split_at(separator);
        after
            .strip_prefix(b"=")
            .is_some_and(|value| header_part_is_valid(name) && header_part_is_valid(value))
    })
}

#[must_use]
pub(crate) fn is_mailto_uri(bytes: &[u8]) -> bool {
    if !bytes
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"mailto:"))
        || UriValue::decode_value(bytes).is_err()
    {
        return false;
    }
    let Some(body) = bytes.get(7..) else {
        return false;
    };
    let mut fragment = body.splitn(2, |octet| *octet == b'#');
    let Some(before_fragment) = fragment.next() else {
        return false;
    };
    let mut query = before_fragment.splitn(2, |octet| *octet == b'?');
    let Some(addresses) = query.next() else {
        return false;
    };
    mailboxes_are_valid(addresses).is_some() && query.next().is_none_or(headers_are_valid)
}
