// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The protocol's own values: what an `href`, a status, an `ETag`, a sync token and a depth
//! are, apart from the bodies and the headers they travel in.
//!
//! ## Where this crate stops, and why the line is here
//!
//! `docs/adr/0004` says this crate produces requests and interprets responses while the caller
//! moves the octets. That leaves a boundary to state rather than to discover, and it runs
//! between a header that *is* the protocol and a header that is the transport carrying it.
//!
//! Protocol semantics, modeled here: `If-Match` and `If-None-Match`, because a conditional
//! write that compares wrongly overwrites somebody else's edit; `Depth`, because it changes
//! which resources a `PROPFIND` is about; `Prefer`, because `return=minimal` changes what the
//! response body contains; and the `ETag`, `Schedule-Tag` and `DAV:sync-token` values those
//! three carry. Each of them is a value this crate renders and reads.
//!
//! Transport, modeled nowhere: `Host`, `Content-Length`, `Content-Type`, `Connection`,
//! `Authorization` and every other credential, redirects, retries, and the request method and
//! URL themselves. This crate has no request type, no header map, and no HTTP model, and
//! inventing one would make the choice `docs/adr/0004` exists to leave open.
//!
//! The rendering doors below take a [`ByteSink`] and write a header *value* — never a name,
//! never a `CRLF`, never a whole header line — because framing is the caller's client's job
//! and a value is the part the protocol defines.

use alloc::boxed::Box;
use alloc::vec::Vec;

use ical_core::{LimitExceeded, Limits, Meter};

use crate::failure::{DavError, ValueError};
use crate::sink::ByteSink;

/// A resource path or URI, as octets.
///
/// Byte-shaped rather than `String`: a server is free to emit octets that are not UTF-8 in a
/// path, and a type that cannot model a response one can read is the failure this workspace
/// exists to prevent. [`Href::as_str`] is the fallible view.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Href {
    /// The octets, exactly as they arrived or were handed in.
    octets: Box<[u8]>,
}

impl Href {
    /// An `href` over `octets`, charged and bounded.
    pub fn new(octets: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        let length = u32::try_from(octets.len()).map_err(|_| LimitExceeded::Href)?;
        if length > limits.max_href_bytes() {
            return Err(DavError::Limit(LimitExceeded::Href));
        }
        meter.try_charge_bytes(u64::from(length))?;
        Ok(Self {
            octets: copy(octets)?,
        })
    }

    /// The octets.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.octets
    }

    /// The octets as text, when they are text.
    pub fn as_str(&self) -> Result<&str, DavError> {
        core::str::from_utf8(&self.octets).map_err(|_| DavError::Invalid(ValueError::NotUtf8))
    }
}

/// An HTTP status code, in the range a status line may carry.
///
/// Held as a number rather than as a reason phrase: RFC 4918 section 14.28 puts a whole status
/// line inside a `DAV:status` element and only the code carries meaning, so the phrase is read
/// and discarded rather than modeled as though a caller should branch on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Status {
    /// The code, always in `100..600`.
    code: u16,
}

impl Status {
    /// `200 OK`.
    pub const OK: Self = Self { code: 200 };
    /// `403 Forbidden`, which a server sends for a property it will not disclose.
    pub const FORBIDDEN: Self = Self { code: 403 };
    /// `404 Not Found`, which a `PROPFIND` sends for a property the resource does not carry.
    pub const NOT_FOUND: Self = Self { code: 404 };
    /// `507 Insufficient Storage`.
    pub const INSUFFICIENT_STORAGE: Self = Self { code: 507 };

    /// A status from a code, which must be one a status line may carry.
    pub const fn new(code: u16) -> Result<Self, DavError> {
        if code < 100 || code >= 600 {
            return Err(DavError::Invalid(ValueError::StatusLine));
        }
        Ok(Self { code })
    }

    /// The code.
    #[must_use]
    pub const fn code(self) -> u16 {
        self.code
    }

    /// Whether the code is a success.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.code >= 200 && self.code < 300
    }

    /// Read the status line RFC 4918 section 14.28 puts inside a `DAV:status`.
    ///
    /// The grammar is RFC 9110 section 15's: a version, a space, three digits, a space, and a
    /// reason phrase that may be empty. Only the digits are kept.
    pub fn parse_status_line(line: &[u8]) -> Result<Self, DavError> {
        let trimmed = trim_ascii(line);
        let after_version = trimmed
            .iter()
            .position(|byte| *byte == b' ')
            .and_then(|space| trimmed.get(space.saturating_add(1)..))
            .ok_or(ValueError::StatusLine)?;
        let digits = after_version.get(..3).ok_or(ValueError::StatusLine)?;
        let mut code: u16 = 0;
        for byte in digits {
            let digit = char::from(*byte)
                .to_digit(10)
                .ok_or(ValueError::StatusLine)?;
            let digit = u16::try_from(digit).map_err(|_| ValueError::StatusLine)?;
            code = code
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(ValueError::StatusLine)?;
        }
        Self::new(code)
    }

    /// Write the status line a `DAV:status` element carries.
    ///
    /// The reason phrase is omitted rather than invented. RFC 9110 section 15 permits an empty
    /// one, and a phrase this crate made up would be a claim about a code the peer chose.
    pub fn write_status_line(self, out: &mut dyn ByteSink) -> Result<(), DavError> {
        out.write(b"HTTP/1.1 ")?;
        out.write(&decimal(u32::from(self.code)))?;
        out.write(b" ")?;
        Ok(())
    }
}

/// An entity tag, as RFC 9110 section 8.8.3 defines one.
///
/// The weakness flag travels with the tag because the two comparison functions RFC 9110
/// section 8.8.3.2 defines differ on it, and a conditional write that used the wrong one
/// silently overwrites another client's edit. There is no `Eq` shortcut for that reason:
/// [`ETag::strongly_matches`] and [`ETag::weakly_matches`] are separate questions and a
/// derived equality would answer whichever one the caller did not mean.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ETag {
    /// The opaque octets between the quotes, without them.
    tag: Box<[u8]>,
    /// Whether the tag arrived with the `W/` prefix.
    weak: bool,
}

impl ETag {
    /// Read an `ETag` header value, or the content of a `DAV:getetag` element.
    ///
    /// Refuses anything that is not a quoted string, optionally prefixed by `W/`. An unquoted
    /// tag is a real server bug and reading one leniently means this crate cannot tell a tag
    /// from a tag with quotes in it.
    pub fn parse(value: &[u8]) -> Result<Self, DavError> {
        let trimmed = trim_ascii(value);
        let (weak, rest) = match trimmed.strip_prefix(b"W/".as_slice()) {
            Some(after) => (true, after),
            None => (false, trimmed),
        };
        let quoted = rest
            .strip_prefix(b"\"".as_slice())
            .and_then(|open| open.strip_suffix(b"\"".as_slice()))
            .ok_or(ValueError::EtagSyntax)?;
        if quoted.contains(&b'"') {
            return Err(DavError::Invalid(ValueError::EtagSyntax));
        }
        Ok(Self {
            tag: copy(quoted)?,
            weak,
        })
    }

    /// The opaque octets, without the quotes and without the `W/`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.tag
    }

    /// Whether the tag is weak.
    #[must_use]
    pub const fn is_weak(&self) -> bool {
        self.weak
    }

    /// RFC 9110 section 8.8.3.2 strong comparison: both strong, and the same octets.
    ///
    /// This is the one `If-Match` uses, and it is the one a conditional `PUT` needs: a weak
    /// tag says two representations are equivalent, which is not the same claim as their
    /// being the octets this client read.
    #[must_use]
    pub fn strongly_matches(&self, other: &Self) -> bool {
        !self.weak && !other.weak && self.tag == other.tag
    }

    /// RFC 9110 section 8.8.3.2 weak comparison: the same octets, weakness disregarded.
    ///
    /// This is the one `If-None-Match` uses.
    #[must_use]
    pub fn weakly_matches(&self, other: &Self) -> bool {
        self.tag == other.tag
    }

    /// Write the tag as a header value or as element content.
    pub fn write_value(&self, out: &mut dyn ByteSink) -> Result<(), DavError> {
        if self.weak {
            out.write(b"W/")?;
        }
        out.write(b"\"")?;
        out.write(&self.tag)?;
        out.write(b"\"")?;
        Ok(())
    }
}

/// An RFC 6578 synchronization token, which is opaque and stays opaque.
///
/// No accessor parses it, compares it for ordering, or reads a number out of it. RFC 6578
/// section 3 makes the token the server's own private state that round-trips through the
/// client untouched, and an implementation that interprets one has invented a coupling to a
/// particular server that will break on the next.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncToken {
    /// The octets the server chose.
    octets: Box<[u8]>,
}

impl SyncToken {
    /// A token over the octets a server sent, charged and bounded.
    ///
    /// Bounded by `max_href_bytes` because a token is a URI in every implementation that
    /// writes one and there is no separate dimension worth inventing for it.
    pub fn new(octets: &[u8], limits: Limits, meter: &mut Meter) -> Result<Self, DavError> {
        let length = u32::try_from(octets.len()).map_err(|_| LimitExceeded::Href)?;
        if length > limits.max_href_bytes() {
            return Err(DavError::Limit(LimitExceeded::Href));
        }
        meter.try_charge_bytes(u64::from(length))?;
        Ok(Self {
            octets: copy(octets)?,
        })
    }

    /// The octets, to be handed back to the server that issued them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.octets
    }
}

/// The `Depth` header's value, RFC 4918 section 10.2.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Depth {
    /// The resource itself.
    #[default]
    Zero,
    /// The resource and its immediate members.
    One,
    /// The resource and every member below it.
    ///
    /// A server is permitted to refuse this, and a client asking for it on a calendar home is
    /// asking for every event the user has.
    Infinity,
}

impl Depth {
    /// The header value.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Zero => b"0",
            Self::One => b"1",
            Self::Infinity => b"infinity",
        }
    }

    /// Read a `Depth` header value.
    pub fn parse(value: &[u8]) -> Result<Self, DavError> {
        match trim_ascii(value) {
            b"0" => Ok(Self::Zero),
            b"1" => Ok(Self::One),
            b"infinity" => Ok(Self::Infinity),
            _ => Err(DavError::Invalid(ValueError::DepthValue)),
        }
    }
}

/// What a conditional write requires of the copy the server holds.
///
/// The three states a calendar client actually needs, named for what they mean rather than for
/// the header that carries them: replace this exact revision, replace whatever is there, or
/// create only if nothing is. A caller renders one into `If-Match` or `If-None-Match` and the
/// comparison rules that decide the outcome are [`ETag::strongly_matches`] and
/// [`ETag::weakly_matches`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Precondition<'a> {
    /// `If-Match: "..."` — the stored copy must be this revision.
    Replace(&'a ETag),
    /// `If-Match: *` — a copy must exist and any revision of it will do.
    ReplaceAny,
    /// `If-None-Match: *` — no copy may exist.
    CreateOnly,
}

impl Precondition<'_> {
    /// The header name this precondition travels under.
    #[must_use]
    pub const fn header_name(self) -> &'static [u8] {
        match self {
            Self::Replace(_) | Self::ReplaceAny => b"If-Match",
            Self::CreateOnly => b"If-None-Match",
        }
    }

    /// Write the header value, without the name and without a terminator.
    pub fn write_value(self, out: &mut dyn ByteSink) -> Result<(), DavError> {
        match self {
            Self::Replace(tag) => tag.write_value(out),
            Self::ReplaceAny | Self::CreateOnly => out.write(b"*").map_err(DavError::from),
        }
    }

    /// Whether a stored revision satisfies this precondition.
    ///
    /// `None` is "nothing is stored". The comparison is RFC 9110 section 8.8.3.2's, strong for
    /// `If-Match` and weak for `If-None-Match`, which is the difference that decides whether a
    /// write lands on the revision the caller read.
    #[must_use]
    pub fn is_satisfied_by(self, stored: Option<&ETag>) -> bool {
        match (self, stored) {
            (Self::Replace(wanted), Some(held)) => wanted.strongly_matches(held),
            (Self::Replace(_) | Self::ReplaceAny, None) | (Self::CreateOnly, Some(_)) => false,
            (Self::ReplaceAny, Some(_)) | (Self::CreateOnly, None) => true,
        }
    }
}

/// What `Prefer` asks a server to put in the response body, RFC 8144 section 2.
///
/// Modeled because it changes the body this crate then has to read: under `return=minimal` a
/// `PROPFIND` answers with the properties that failed and omits the ones that succeeded, and a
/// reader expecting every requested property would report absences the server did not mean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prefer {
    /// Say nothing; take whatever the server sends.
    #[default]
    Unstated,
    /// `return=minimal`: omit what succeeded.
    ReturnMinimal,
    /// `depth-noroot`: exclude the collection itself from a `Depth: 1` answer.
    DepthNoRoot,
}

impl Prefer {
    /// The header value, or `None` when nothing is preferred.
    #[must_use]
    pub const fn as_bytes(self) -> Option<&'static [u8]> {
        match self {
            Self::Unstated => None,
            Self::ReturnMinimal => Some(b"return=minimal"),
            Self::DepthNoRoot => Some(b"depth-noroot"),
        }
    }
}

/// Which of the `DAV:resourcetype` values a resource claims.
///
/// The two that decide how a client walks a server are named fields; anything else is kept
/// rather than dropped, because a resource type this crate has no row for is still a claim the
/// server made about the resource.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceType {
    /// `DAV:collection`.
    pub collection: bool,
    /// `CALDAV:calendar`.
    pub calendar: bool,
    /// `DAV:principal`.
    pub principal: bool,
    /// Every other child element of `resourcetype`, kept as names.
    others: crate::bound::Bounded<ExtensionName>,
}

impl ResourceType {
    /// A resource that claims nothing yet.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            collection: false,
            calendar: false,
            principal: false,
            others: crate::bound::Bounded::with_cap(
                bounded_cap(limits.max_props_per_response()),
                LimitExceeded::Properties,
            ),
        }
    }

    /// Record a resource type this crate has no field for.
    pub fn push_other(&mut self, name: ExtensionName, meter: &mut Meter) -> Result<(), DavError> {
        self.others.push(name, meter)
    }

    /// The resource types this crate has no field for.
    #[must_use]
    pub fn others(&self) -> &[ExtensionName] {
        self.others.as_slice()
    }
}

/// The name of an element outside the closed vocabulary, kept whole.
///
/// A namespace URI and a local name, owned, because a foreign property's *name* is the part a
/// caller needs in order to ask a human what it was. Never a prefix: the prefix was the
/// document's own choice and means nothing outside it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExtensionName {
    /// The resolved namespace URI.
    namespace: Box<[u8]>,
    /// The local name.
    local_name: Box<[u8]>,
}

impl ExtensionName {
    /// A name outside the vocabulary, charged against the caller's ledger.
    pub fn new(namespace: &[u8], local_name: &[u8], meter: &mut Meter) -> Result<Self, DavError> {
        let length = namespace.len().saturating_add(local_name.len());
        meter.try_charge_bytes(u64::try_from(length).unwrap_or(u64::MAX))?;
        Ok(Self {
            namespace: copy(namespace)?,
            local_name: copy(local_name)?,
        })
    }

    /// The resolved namespace URI.
    #[must_use]
    pub fn namespace(&self) -> &[u8] {
        &self.namespace
    }

    /// The local name.
    #[must_use]
    pub fn local_name(&self) -> &[u8] {
        &self.local_name
    }
}

/// A `u32` bound as a collection cap, which cannot overflow a `usize` on any supported target.
pub(crate) fn bounded_cap(bound: u32) -> usize {
    usize::try_from(bound).unwrap_or(usize::MAX)
}

/// Copy octets into an owned box through a fallible allocation.
pub(crate) fn copy(octets: &[u8]) -> Result<Box<[u8]>, DavError> {
    let mut owned = Vec::new();
    owned
        .try_reserve(octets.len())
        .map_err(|_| LimitExceeded::Budget)?;
    owned.extend_from_slice(octets);
    Ok(owned.into_boxed_slice())
}

/// Drop leading and trailing spaces and tabs, which a header value may carry either side of.
fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(*byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !matches!(*byte, b' ' | b'\t'))
        .map_or(start, |last| last.saturating_add(1));
    value.get(start..end).unwrap_or(&[])
}

/// The decimal digits of a number, without allocating.
fn decimal(mut value: u32) -> Vec<u8> {
    let mut digits = Vec::new();
    if value == 0 {
        digits.push(b'0');
    }
    while value > 0 {
        let digit = u8::try_from(value.checked_rem(10).unwrap_or(0)).unwrap_or(0);
        digits.push(b'0'.saturating_add(digit));
        value = value.checked_div(10).unwrap_or(0);
    }
    digits.reverse();
    digits
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{Limits, Meter};

    use super::{Depth, ETag, Href, Precondition, Status, SyncToken};
    use crate::failure::{DavError, ValueError};

    #[test]
    fn a_status_line_is_read_for_its_code_and_nothing_else() {
        assert_eq!(
            Status::parse_status_line(b"HTTP/1.1 404 Not Found"),
            Ok(Status::NOT_FOUND)
        );
        assert_eq!(Status::parse_status_line(b"HTTP/1.1 200 "), Ok(Status::OK));
        assert_eq!(
            Status::parse_status_line(b"200 OK"),
            Err(DavError::Invalid(ValueError::StatusLine))
        );
    }

    #[test]
    fn a_status_writes_back_the_line_it_reads() {
        let mut out: Vec<u8> = Vec::new();
        Status::new(507)
            .unwrap()
            .write_status_line(&mut out)
            .unwrap();
        assert_eq!(out, b"HTTP/1.1 507 ");
        assert_eq!(
            Status::parse_status_line(&out),
            Ok(Status::INSUFFICIENT_STORAGE)
        );
    }

    #[test]
    fn the_two_etag_comparisons_are_different_questions() {
        let strong = ETag::parse(b"\"abc\"").unwrap();
        let weak = ETag::parse(b"W/\"abc\"").unwrap();
        // RFC 9110 section 8.8.3.2: a weak tag never satisfies a strong comparison, which is
        // what keeps a conditional PUT from landing on a revision the client never read.
        assert!(!strong.strongly_matches(&weak));
        assert!(strong.strongly_matches(&strong.clone()));
        assert!(strong.weakly_matches(&weak));
    }

    #[test]
    fn an_unquoted_etag_is_refused_rather_than_guessed_at() {
        assert_eq!(
            ETag::parse(b"abc"),
            Err(DavError::Invalid(ValueError::EtagSyntax))
        );
    }

    #[test]
    fn a_precondition_decides_a_write_against_what_is_stored() {
        let read = ETag::parse(b"\"v1\"").unwrap();
        let moved = ETag::parse(b"\"v2\"").unwrap();
        assert!(Precondition::Replace(&read).is_satisfied_by(Some(&read)));
        assert!(!Precondition::Replace(&read).is_satisfied_by(Some(&moved)));
        assert!(!Precondition::Replace(&read).is_satisfied_by(None));
        assert!(Precondition::CreateOnly.is_satisfied_by(None));
        assert!(!Precondition::CreateOnly.is_satisfied_by(Some(&read)));
        assert!(Precondition::ReplaceAny.is_satisfied_by(Some(&moved)));
    }

    #[test]
    fn a_depth_round_trips_through_its_header_value() {
        for depth in [Depth::Zero, Depth::One, Depth::Infinity] {
            assert_eq!(Depth::parse(depth.as_bytes()), Ok(depth));
        }
        assert_eq!(
            Depth::parse(b"2"),
            Err(DavError::Invalid(ValueError::DepthValue))
        );
    }

    #[test]
    fn an_href_past_the_bound_is_refused_and_a_sync_token_stays_opaque() {
        let limits = Limits::DEFAULT.with_max_href_bytes(8);
        let mut meter = Meter::new(limits);
        assert!(Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter).is_err());
        let token = SyncToken::new(b"http://x/ns/sync/42", Limits::DEFAULT, &mut meter).unwrap();
        assert_eq!(token.as_bytes(), b"http://x/ns/sync/42");
    }
}
