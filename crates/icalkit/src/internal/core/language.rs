// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Language tag syntax from RFC 5646 sections 2.1 and 2.2.9.

use super::{LimitExceeded, Meter};

pub(crate) fn is_language_tag(bytes: &[u8], meter: &mut Meter) -> Result<bool, LimitExceeded> {
    const IRREGULAR: &[&[u8]] = &[
        b"en-GB-oed",
        b"i-ami",
        b"i-bnn",
        b"i-default",
        b"i-enochian",
        b"i-hak",
        b"i-klingon",
        b"i-lux",
        b"i-mingo",
        b"i-navajo",
        b"i-pwn",
        b"i-tao",
        b"i-tay",
        b"i-tsu",
        b"sgn-BE-FR",
        b"sgn-BE-NL",
        b"sgn-CH-DE",
    ];
    meter.try_charge(bytes.len() as u64)?;
    if IRREGULAR.iter().any(|tag| bytes.eq_ignore_ascii_case(tag)) {
        return Ok(true);
    }
    let mut subtags = bytes.split(|octet| *octet == b'-').peekable();
    let Some(language) = subtags.next() else {
        return Ok(false);
    };
    if language.eq_ignore_ascii_case(b"x") {
        return Ok(subtags.peek().is_some() && subtags.all(|tag| is_alphanumeric(tag, 1)));
    }
    if !(2..=8).contains(&language.len()) || !language.iter().all(u8::is_ascii_alphabetic) {
        return Ok(false);
    }
    if language.len() <= 3 {
        for _ in 0..3 {
            if subtags.next_if(|tag| is_alphabetic(tag, 3)).is_none() {
                break;
            }
        }
    }
    subtags.next_if(|tag| is_alphabetic(tag, 4));
    subtags.next_if(|tag| {
        is_alphabetic(tag, 2) || (tag.len() == 3 && tag.iter().all(u8::is_ascii_digit))
    });

    let variants = subtags.clone();
    let mut seen = 0_usize;
    while let Some(variant) = subtags.next_if(|tag| is_variant(tag)) {
        for earlier in variants.clone().take(seen) {
            meter.try_charge((earlier.len() as u64).saturating_add(1))?;
            if variant.eq_ignore_ascii_case(earlier) {
                return Ok(false);
            }
        }
        seen = seen.saturating_add(1);
    }

    let mut singletons = 0_u64;
    while let Some(tag) = subtags.next() {
        if tag.eq_ignore_ascii_case(b"x") {
            return Ok(subtags.peek().is_some() && subtags.all(|tag| is_alphanumeric(tag, 1)));
        }
        let [singleton] = tag else {
            return Ok(false);
        };
        let lower = singleton.to_ascii_lowercase();
        let index = match lower {
            b'0'..=b'9' => lower.saturating_sub(b'0'),
            b'a'..=b'z' => lower.saturating_sub(b'a').saturating_add(10),
            _ => return Ok(false),
        };
        let bit = 1_u64 << index;
        if singletons & bit != 0 {
            return Ok(false);
        }
        singletons |= bit;
        if subtags.next_if(|tag| is_alphanumeric(tag, 2)).is_none() {
            return Ok(false);
        }
        while subtags.next_if(|tag| is_alphanumeric(tag, 2)).is_some() {}
    }
    Ok(true)
}

fn is_alphabetic(bytes: &[u8], length: usize) -> bool {
    bytes.len() == length && bytes.iter().all(u8::is_ascii_alphabetic)
}

fn is_alphanumeric(bytes: &[u8], minimum: usize) -> bool {
    (minimum..=8).contains(&bytes.len()) && bytes.iter().all(u8::is_ascii_alphanumeric)
}

fn is_variant(bytes: &[u8]) -> bool {
    is_alphanumeric(bytes, 5)
        || (bytes.len() == 4
            && bytes.first().is_some_and(u8::is_ascii_digit)
            && bytes.iter().all(u8::is_ascii_alphanumeric))
}
