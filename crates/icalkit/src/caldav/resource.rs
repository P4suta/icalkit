// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resource identity for WebDAV response hrefs.

/// RFC 9110 section 4.3.1 identifies an origin by scheme, host, and normalized port.
struct HttpOrigin<'a> {
    scheme: &'a str,
    host: &'a str,
    port: &'a str,
}

impl<'a> HttpOrigin<'a> {
    fn parse(scheme: &'a str, authority: &'a str) -> Option<Self> {
        let default_port = if scheme.eq_ignore_ascii_case("https") {
            "443"
        } else if scheme.eq_ignore_ascii_case("http") {
            "80"
        } else {
            return None;
        };
        let host_end = if authority.starts_with('[') {
            authority.find(']')?.checked_add(1)?
        } else {
            authority.find(':').unwrap_or(authority.len())
        };
        let (host, suffix) = authority.split_at(host_end);
        if host.is_empty() || authority.contains('@') {
            return None;
        }
        let port = match suffix {
            "" | ":" => default_port,
            suffix => {
                let digits = suffix.strip_prefix(':')?;
                if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                match digits.trim_start_matches('0') {
                    "" => "0",
                    port => port,
                }
            },
        };
        Some(Self { scheme, host, port })
    }

    fn matches(&self, other: &Self) -> bool {
        self.scheme.eq_ignore_ascii_case(other.scheme)
            && self.host.eq_ignore_ascii_case(other.host)
            && self.port == other.port
    }
}

struct ResourceReference<'a> {
    origin: Option<HttpOrigin<'a>>,
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
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            let (authority, rest) = rest.split_at(end);
            (Some(HttpOrigin::parse(scheme, authority)?), rest)
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
    let (Some(target), Some(href)) = (
        ResourceReference::parse(target),
        ResourceReference::parse(href),
    ) else {
        return false;
    };
    let same_origin = match (target.origin, href.origin) {
        (_, None) => true,
        (Some(request), Some(response)) => request.matches(&response),
        (None, Some(_)) => false,
    };
    let same_query = match (target.query, href.query) {
        (Some(one), Some(other)) => normalized_octets(one).eq(normalized_octets(other)),
        (None, None) => true,
        _ => false,
    };
    same_origin && same_query && normalized_octets(target.path).eq(normalized_octets(href.path))
}

/// RFC 3986 section 2.3 permits escaped unreserved octets to match their literal forms.
fn normalized_octets(component: &str) -> impl Iterator<Item = (bool, u8)> + '_ {
    let mut remaining = component.as_bytes();
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
