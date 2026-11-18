// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resource identity for WebDAV response hrefs.

struct ResourceReference<'a> {
    origin: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
}

impl<'a> ResourceReference<'a> {
    fn parse(uri: &'a str) -> Option<Self> {
        if uri.contains('#') || uri.starts_with("//") {
            return None;
        }
        let (origin, rest) = if uri.starts_with('/') {
            (None, uri)
        } else {
            let (scheme, rest) = uri.split_once("://")?;
            let port = if scheme.eq_ignore_ascii_case("https") {
                ":443"
            } else if scheme.eq_ignore_ascii_case("http") {
                ":80"
            } else {
                return None;
            };
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            let (authority, rest) = rest.split_at(end);
            if authority.is_empty() || authority.contains('@') {
                return None;
            }
            let origin = uri.strip_suffix(rest)?;
            (Some(origin.strip_suffix(port).unwrap_or(origin)), rest)
        };
        let (path, query) = rest
            .split_once('?')
            .map_or((rest, None), |(path, query)| (path, Some(query)));
        Some(Self {
            origin,
            path: if path.is_empty() { "/" } else { path },
            query,
        })
    }
}

pub(super) fn same_resource(target: &str, href: &str) -> bool {
    let (Some(left), Some(right)) = (
        ResourceReference::parse(target),
        ResourceReference::parse(href),
    ) else {
        return false;
    };
    path_octets(left.path).eq(path_octets(right.path))
        && left.query == right.query
        && left
            .origin
            .zip(right.origin)
            .is_none_or(|(one, other)| one.eq_ignore_ascii_case(other))
}
fn path_octets(path: &str) -> impl Iterator<Item = (bool, u8)> + '_ {
    let mut remaining = path.as_bytes();
    core::iter::from_fn(move || {
        if let [b'%', hi, lo, rest @ ..] = remaining
            && let (Some(hi), Some(lo)) =
                (char::from(*hi).to_digit(16), char::from(*lo).to_digit(16))
            && let Ok(byte) = u8::try_from((hi << 4) | lo)
        {
            remaining = rest;
            let unreserved =
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
            return Some((!unreserved, byte));
        }
        let (&first, rest) = remaining.split_first()?;
        remaining = rest;
        Some((false, first))
    })
}
