// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which revision a caller read, and the precondition a write must carry to land on it.
//!
//! A calendar client's second turn is always conditional. It reads a resource, decides
//! something about what it read, and writes the decision back — and between those two turns
//! another client, a phone, or the server's own scheduling machinery may have replaced the
//! octets it decided about. [`Revision`] is what the first turn learned: the `DAV:getetag` the
//! read returned, the `CALDAV:schedule-tag` beside it when the server sent one, the
//! `DAV:sync-token` the collection stood at, and — because none of those means anything apart
//! from the resource they describe — the `href` they were read from.
//! [`Revision::precondition`] turns that into the [`Precondition`] the second turn must carry.
//!
//! # What this holds, and what it does not
//!
//! It authenticates nobody and holds no authority. Nothing here checks a signature, names a
//! principal, or decides that a write is permitted; a `DAV:current-user-principal` is what the
//! *server* says the caller is, and this type never reads one. The freshness a caller ends up
//! with is the freshness the server enforces when it compares the `If-Match`, and every claim
//! below is about what a caller may safely ask for, never about what it may take.
//!
//! [`Revision::digest`] is a checksum and **not** a MAC. It exists so a caller can remember a
//! revision in one small `Copy` number — a column in a store, a key in a `BTreeMap` — and
//! notice later that the world moved. It is FNV-1a, it is not keyed, and anyone who can choose
//! the octets a server sends can choose what it comes out as. What that buys an attacker is
//! bounded by where the digest is used: a forged or colliding digest can make a caller believe
//! its revision still stands, whereupon the caller writes with the `ETag` it holds and the
//! server refuses the write. The ability an attacker gains is the ability to have a write
//! refused, because the comparison that decides the outcome is the server's and never this
//! one's. A caller that instead treated a matching digest as a reason to *skip* the
//! precondition would have converted that into data loss, which is why no door here writes an
//! unconditional request.
//!
//! Nothing here reads a clock, and `DAV:getlastmodified` is deliberately not a revision. It is
//! a weak validator at one-second granularity (RFC 9110 section 8.8.2), two writes inside one
//! second are indistinguishable through it, and comparing it needs a notion of now that
//! `docs/adr/0004` does not give this workspace.
//!
//! # There are two things called a revision, and they are not the same thing
//!
//! `ical_itip::Revision` is `SEQUENCE` and `DTSTAMP`: the calendar object's own claim about its
//! version, written by whoever composed it and copied by anyone who forwards it. This one is
//! the server's claim about the octets it currently stores, minted by the server and enforced
//! by the server. A caller binds an `ical_itip::Commitment` to one of these — storing the
//! digest beside the commitment and re-deriving it after the confirming read — at its own
//! layer, because `ical-dav` may not depend on `ical-itip` and neither crate may name the
//! other's type. What this layer supplies is the value that binding is made of.
//!
//! # Which headers are the protocol's
//!
//! `If-Match` and `If-None-Match` through [`Precondition`], [`IF_SCHEDULE_TAG_MATCH`] through
//! [`Revision::schedule_tag`], [`Depth`] and [`Prefer`] through the two doors at the end of
//! this module: each of those changes what a request means or what a response body contains.
//! `Host`, `Content-Length`, `Content-Type`, `Authorization` and every other credential, the
//! method, the URL, redirects and retries are the transport's, and this crate models none of
//! them. Every door writes a header *value* — never a name, never a `CRLF`, never a whole
//! line — because framing belongs to the client the caller already has.
//!
//! # The comparison rules are two, and using the wrong one loses an edit
//!
//! RFC 9110 section 8.8.3.2 defines strong and weak comparison, and RFC 9110 section 13.1.1
//! makes `If-Match` use the strong one. So a weak `ETag` — `W/"..."`, which says two
//! representations are equivalent rather than that they are the octets this caller read — can
//! never satisfy an `If-Match`, and [`Revision::precondition`] answers `None` for one rather
//! than rendering a write no server can accept or quietly downgrading to `If-Match: *`. That
//! downgrade is the bug this whole module exists to prevent: it turns "replace what I read"
//! into "replace whatever is there", and the trace it leaves on somebody else's edit is a
//! changed `ETag`. [`Precondition::ReplaceAny`] is available to a caller that means it, and
//! nothing here returns it on a caller's behalf.
//!
//! RFC 6638 section 8.3 adds a third rule for a fourth header: a client updating a scheduling
//! object resource should send `If-Schedule-Tag-Match` rather than `If-Match`, so that an
//! attendee's reply landing on the organizer's copy does not invalidate the edit the organizer
//! was making. Both tags are kept here and neither is preferred automatically, because which
//! one applies depends on whether the resource is a scheduling object — which is a question
//! about the calendar data, not about the protocol values.

use alloc::vec::Vec;

use crate::internal::core::{Limits, Meter};

use crate::internal::dav::element::ElementName;
use crate::internal::dav::failure::DavError;
use crate::internal::dav::request::PropName;
use crate::internal::dav::response::{DavProperty, DavResponse, PropStat, PropValue, ResponseBody};
use crate::internal::dav::sink::ByteSink;
use crate::internal::dav::value::{Depth, ETag, Href, Precondition, Prefer, Status, SyncToken};

/// The header a conditional write of a scheduling object resource carries, RFC 6638 section 8.3.
///
/// A name and nothing else, because this crate renders values and never lines. The value is the
/// `CALDAV:schedule-tag` from [`Revision::schedule_tag`], written through
/// [`ETag::write_value`].
pub const IF_SCHEDULE_TAG_MATCH: &[u8] = b"If-Schedule-Tag-Match";

/// What a read said about whether anything is stored at an `href`.
///
/// Three-valued rather than a `bool`, because "the server refused to tell me" is neither of the
/// other two and a caller that read it as either would be inventing a fact. A `403` on the
/// resource is not an absence, and treating it as one is how a client creates a second copy of
/// an event it was merely not allowed to see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Presence {
    /// The read did not say, which is the honest answer to anything but a clear one.
    #[default]
    Unknown,
    /// The read found a stored copy.
    Stored,
    /// The read found nothing stored: a `404`, or a `410` for something deleted.
    Absent,
}

impl Presence {
    /// A distinct octet per state, so [`Revision::digest`] separates the three.
    const fn marker(self) -> u8 {
        match self {
            Self::Unknown => b'?',
            Self::Stored => b'+',
            Self::Absent => b'-',
        }
    }
}

/// The revision of one resource, as a caller read it.
///
/// Deliberately not `Ord`. Nothing in here is ordered: a sync token is opaque octets the server
/// chose (RFC 6578 section 3), an `ETag` is a validator and not a counter, and a type that
/// sorted revisions would be offering a recency the protocol never gives it. A caller keying a
/// `BTreeMap` uses [`Revision::digest`], which is a number precisely because it is not a claim
/// about time.
///
/// `PartialEq` is structural — the same `href`, the same states, the same octets, weakness
/// included — and is not the RFC 9110 comparison. [`Revision::is_same_revision_as`] is that
/// question, and the two answers differ for a weak tag: two weak `W/"abc"` reads are equal as
/// values and are *not* the same revision, because no conditional write can distinguish them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Revision {
    /// The resource this revision is about.
    ///
    /// Not optional and not omitted: RFC 9110 section 8.8.3 makes an entity tag a validator
    /// "for differentiating between multiple representations of the same resource", so two tags
    /// from two `href`s are not comparable and a revision that could not say which resource it
    /// came from would invite exactly that comparison.
    href: Href,
    /// Whether the read found anything stored.
    presence: Presence,
    /// The `DAV:getetag`, when the server sent one.
    etag: Option<ETag>,
    /// The `CALDAV:schedule-tag`, when the server sent one.
    schedule_tag: Option<ETag>,
    /// The `DAV:sync-token` the collection stood at, when the caller knows it.
    ///
    /// Carried and never interpreted. It conditions no write — there is no header that takes
    /// one — and it is here because the revision a caller read and the point in the
    /// collection's history it read at are one fact about one read.
    sync_token: Option<SyncToken>,
}

impl Revision {
    /// The revision of `href` a read said nothing about.
    #[must_use]
    pub const fn unknown(href: Href) -> Self {
        Self::of(href, Presence::Unknown)
    }

    /// The revision of `href` at which nothing is stored.
    ///
    /// This is a revision like any other, and the one a create depends on: `If-None-Match: *`
    /// is how a client that read an absence writes a new resource without silently replacing
    /// one that appeared in between.
    #[must_use]
    pub const fn absent(href: Href) -> Self {
        Self::of(href, Presence::Absent)
    }

    /// The revision of `href` a read found stored with no `ETag` beside it.
    ///
    /// A server is not obliged to send `DAV:getetag`, and one that did not has left the caller
    /// unable to write conditionally; [`Revision::precondition`] says so by answering `None`.
    #[must_use]
    pub const fn stored(href: Href) -> Self {
        Self::of(href, Presence::Stored)
    }

    /// The revision of `href` a read found stored at `etag`.
    #[must_use]
    pub const fn at(href: Href, etag: ETag) -> Self {
        Self {
            href,
            presence: Presence::Stored,
            etag: Some(etag),
            schedule_tag: None,
            sync_token: None,
        }
    }

    /// The same revision with the `CALDAV:schedule-tag` the server sent beside its `ETag`.
    #[must_use]
    pub fn with_schedule_tag(mut self, schedule_tag: ETag) -> Self {
        self.schedule_tag = Some(schedule_tag);
        self
    }

    /// The same revision with the `DAV:sync-token` the collection stood at.
    #[must_use]
    pub fn with_sync_token(mut self, sync_token: SyncToken) -> Self {
        self.sync_token = Some(sync_token);
        self
    }

    /// The revision one response of a multistatus states.
    ///
    /// The tags are read through [`DavResponse::successful_value`], so a `getetag` a server
    /// reported under `404` or `403` is a tag the server refused rather than a tag: it does not
    /// become a precondition, and the caller is left unable to write conditionally instead of
    /// writing against a validator nobody supplied. A property whose value did not arrive as
    /// [`PropValue::Entity`] is passed over for the same reason — erring toward no precondition
    /// costs a refused write, and erring the other way costs somebody's edit.
    pub fn from_response(
        response: &DavResponse,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<Self, DavError> {
        let href = Href::new(response.href.as_bytes(), limits, meter)?;
        let mut revision = match &response.body {
            // A propstat is an answer about a resource the server has: a `404` inside one is
            // about the property and not the resource (RFC 4918 section 14.22). A bare status
            // is the shape a multiget and a sync report use for a member that is gone.
            ResponseBody::PropStats(_) => Self::stored(href),
            ResponseBody::Status(status) => Self::of(href, presence_of(*status)),
        };
        revision.etag = successful_tag(response, ElementName::Getetag, meter)?;
        revision.schedule_tag = successful_tag(response, ElementName::ScheduleTag, meter)?;
        Ok(revision)
    }

    /// State this revision's tags as properties of a group a server is building.
    ///
    /// The other direction of the same type: a server holding a revision writes `DAV:getetag`
    /// and `CALDAV:schedule-tag` out of it, and the client reads the same value back with
    /// [`Revision::from_response`]. Nothing is written for a revision with no tags, and the
    /// group's status is the caller's — a server reporting a tag it will not disclose puts an
    /// empty property under `403` rather than a tag under `200`.
    pub fn push_properties(&self, group: &mut PropStat, meter: &mut Meter) -> Result<(), DavError> {
        push_tag(group, ElementName::Getetag, self.etag.as_ref(), meter)?;
        push_tag(
            group,
            ElementName::ScheduleTag,
            self.schedule_tag.as_ref(),
            meter,
        )
    }

    /// The resource this revision is about.
    #[must_use]
    pub const fn href(&self) -> &Href {
        &self.href
    }

    /// Whether the read found anything stored.
    #[must_use]
    pub const fn presence(&self) -> Presence {
        self.presence
    }

    /// The `DAV:getetag` the read returned, if it returned one.
    #[must_use]
    pub fn etag(&self) -> Option<&ETag> {
        self.etag.as_ref()
    }

    /// The `CALDAV:schedule-tag` the read returned, if it returned one.
    ///
    /// The value of an [`IF_SCHEDULE_TAG_MATCH`] header, which RFC 6638 section 8.3 says a
    /// client should send instead of `If-Match` when it updates a scheduling object resource.
    /// Whether a resource is one is a fact about the calendar data inside it, so this crate
    /// hands back both tags and chooses neither.
    #[must_use]
    pub fn schedule_tag(&self) -> Option<&ETag> {
        self.schedule_tag.as_ref()
    }

    /// The `DAV:sync-token` the collection stood at, if the caller knows it.
    ///
    /// Opaque, and it stays opaque: nothing here parses it, orders it, or reads a number out of
    /// it. RFC 6578 section 3 makes the token the server's own private state round-tripping
    /// through the client untouched, and a client that interpreted one would have invented a
    /// coupling to the server that minted it.
    #[must_use]
    pub fn sync_token(&self) -> Option<&SyncToken> {
        self.sync_token.as_ref()
    }

    /// The precondition a conditional write must carry to land on this revision and no other.
    ///
    /// `None` means no precondition expresses it, which happens three ways and is never a
    /// reason to write unconditionally: the read said nothing about what is stored, the server
    /// sent no `ETag`, or the `ETag` is weak and RFC 9110 section 13.1.1's strong comparison
    /// can never be satisfied by one. A caller that means to replace whatever is there says so
    /// with [`Precondition::ReplaceAny`]; this door never says it for anyone.
    #[must_use]
    pub fn precondition(&self) -> Option<Precondition<'_>> {
        match self.presence {
            Presence::Absent => Some(Precondition::CreateOnly),
            Presence::Stored => self
                .etag
                .as_ref()
                .filter(|tag| !tag.is_weak())
                .map(Precondition::Replace),
            Presence::Unknown => None,
        }
    }

    /// Whether a write conditioned on `self` would land on `other`.
    ///
    /// RFC 9110 section 8.8.3.2's strong comparison, over the same resource, plus the one case
    /// the tags cannot express: two reads that both found nothing stored are the same revision,
    /// which is what makes a create idempotent under `If-None-Match: *`. Everything else is
    /// `false`, weak tags included — two `W/"abc"` reads are equal as values and are not the
    /// same revision, because nothing a client can send distinguishes them.
    ///
    /// The `href` is compared octet for octet and never as URI equivalence (RFC 3986 section
    /// 6), because normalizing somebody's path is a decision with no right answer at this
    /// layer. Two spellings of one resource therefore answer `false`, which costs a caller a
    /// re-read rather than a write aimed at a resource it did not mean.
    #[must_use]
    pub fn is_same_revision_as(&self, other: &Self) -> bool {
        if self.href != other.href {
            return false;
        }
        match (self.presence, other.presence) {
            (Presence::Absent, Presence::Absent) => true,
            (Presence::Stored, Presence::Stored) => match (self.etag.as_ref(), other.etag.as_ref())
            {
                (Some(mine), Some(theirs)) => mine.strongly_matches(theirs),
                _ => false,
            },
            _ => false,
        }
    }

    /// A checksum over everything this revision states. Not a MAC; see the module documentation.
    ///
    /// FNV-1a, so that a caller with one `u64` column can bind a decision to the revision it
    /// was made against and notice a later read that differs. It detects a resource that moved
    /// between two turns, which is an accident far more often than an adversary, and the
    /// enforcement of the write is the server's `If-Match` comparison either way.
    #[must_use]
    pub fn digest(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        let mut hash = mix_field(OFFSET, b'h', self.href.as_bytes());
        hash = mix_field(hash, b'p', &[self.presence.marker()]);
        if let Some(tag) = self.etag.as_ref() {
            // Weakness is part of the identity: a strong and a weak tag over the same octets
            // are different revisions to every conditional write that could be built from them.
            let label = if tag.is_weak() { b'w' } else { b'e' };
            hash = mix_field(hash, label, tag.as_bytes());
        }
        if let Some(tag) = self.schedule_tag.as_ref() {
            hash = mix_field(hash, b's', tag.as_bytes());
        }
        if let Some(token) = self.sync_token.as_ref() {
            hash = mix_field(hash, b't', token.as_bytes());
        }
        hash
    }

    /// A revision of `href` in a state, with no tags yet.
    const fn of(href: Href, presence: Presence) -> Self {
        Self {
            href,
            presence,
            etag: None,
            schedule_tag: None,
            sync_token: None,
        }
    }
}

/// What a response's own status says about whether anything is stored.
///
/// Only `404` and `410` assert an absence. A `403` says the caller may not know and a `500`
/// says the server could not answer, and reading either as "nothing is there" is how a client
/// creates a duplicate of a resource it merely could not see.
fn presence_of(status: Status) -> Presence {
    if status.is_success() {
        Presence::Stored
    } else if status.code() == 404 || status.code() == 410 {
        Presence::Absent
    } else {
        Presence::Unknown
    }
}

/// The tag a named property came back with under a successful status, if any.
fn successful_tag(
    response: &DavResponse,
    name: ElementName,
    meter: &mut Meter,
) -> Result<Option<ETag>, DavError> {
    match response.successful_value(&PropName::Known(name)) {
        Some(PropValue::Entity(tag)) => copy_tag(tag, meter).map(Some),
        _ => Ok(None),
    }
}

/// Push one tag into a property group, or nothing when there is no tag.
fn push_tag(
    group: &mut PropStat,
    name: ElementName,
    tag: Option<&ETag>,
    meter: &mut Meter,
) -> Result<(), DavError> {
    let Some(tag) = tag else {
        return Ok(());
    };
    group.push(
        DavProperty {
            name: PropName::Known(name),
            value: PropValue::Entity(copy_tag(tag, meter)?),
        },
        meter,
    )
}

/// Copy a tag through a charged, fallible allocation.
///
/// Rendered and read back rather than cloned: `Clone` on the boxed octets allocates through the
/// infallible path, and `docs/adr/0007` requires every allocation here to be one an unwilling
/// allocator can refuse, which `Vec` as a [`ByteSink`] gives through `try_reserve`. The round
/// trip is exact because [`ETag::parse`] refuses a tag containing a quote, so nothing it
/// accepted can be re-read as anything else.
fn copy_tag(tag: &ETag, meter: &mut Meter) -> Result<ETag, DavError> {
    meter.try_charge_bytes(u64::try_from(tag.as_bytes().len()).unwrap_or(u64::MAX))?;
    let mut rendered: Vec<u8> = Vec::new();
    tag.write_value(&mut rendered)?;
    ETag::parse(&rendered)
}

/// FNV-1a over `octets`, continuing `hash`.
fn mix(hash: u64, octets: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    octets.iter().fold(hash, |accumulated, octet| {
        (accumulated ^ u64::from(*octet)).wrapping_mul(PRIME)
    })
}

/// FNV-1a over one labeled, length-counted field.
///
/// The label and the length go in before the octets, so that moving a byte from one field into
/// the next changes the digest: without them a revision of `/a` tagged `"bc"` and one of `/ab`
/// tagged `"c"` would hash alike, and a caller comparing digests would call two resources one.
fn mix_field(hash: u64, label: u8, octets: &[u8]) -> u64 {
    let labeled = mix(hash, &[label]);
    let counted = mix(
        labeled,
        &u64::try_from(octets.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    mix(counted, octets)
}

/// Write a `Depth` header value into a sink, RFC 4918 section 10.2.
///
/// The value alone, so a caller building a request into a fixed buffer renders every protocol
/// header value through the same door [`Precondition::write_value`] and [`ETag::write_value`]
/// use, including on a target where [`crate::internal::dav::SliceSink`] is the only sink there is.
pub fn write_depth_value(depth: Depth, out: &mut dyn ByteSink) -> Result<(), DavError> {
    out.write(depth.as_bytes()).map_err(DavError::from)
}

/// Write a `Prefer` header value into a sink, RFC 8144 section 2.
///
/// `Ok(false)` means [`Prefer::Unstated`]: nothing was written and no header should be sent at
/// all. An empty `Prefer` value is not a way to prefer nothing — RFC 8144 gives it no meaning,
/// and a server is entitled to make its own of it.
pub fn write_prefer_value(prefer: Prefer, out: &mut dyn ByteSink) -> Result<bool, DavError> {
    match prefer.as_bytes() {
        Some(value) => {
            out.write(value)?;
            Ok(true)
        },
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{Limits, Meter};

    use super::{IF_SCHEDULE_TAG_MATCH, Presence, Revision, write_depth_value, write_prefer_value};
    use crate::internal::dav::element::{ElementName, Namespace};
    use crate::internal::dav::failure::{DavError, ValueError};
    use crate::internal::dav::request::PropName;
    use crate::internal::dav::response::{DavProperty, DavResponse, PropStat, PropValue};
    use crate::internal::dav::value::{
        Depth, ETag, ExtensionName, Href, Precondition, Prefer, Status, SyncToken,
    };

    fn href(path: &[u8], meter: &mut Meter) -> Href {
        Href::new(path, Limits::DEFAULT, meter).unwrap()
    }

    fn stored_at(path: &[u8], sent: &[u8], meter: &mut Meter) -> Revision {
        Revision::at(href(path, meter), ETag::parse(sent).unwrap())
    }

    /// One row of the table below: the tag a server sent, and the header name and value a
    /// conditional write carries for it.
    type ConditionalWrite = (&'static [u8], Option<(&'static [u8], &'static [u8])>);

    /// What a conditional write carries for an `ETag` a server sent, as octets both ways.
    ///
    /// The left column is the wire a real server writes: `SabreDAV` and Radicale quote a hex
    /// digest, Google sends a weak validator over a revision number, Calendar Server quotes a
    /// generated token, and a header value is allowed leading and trailing whitespace that is
    /// not part of the tag. The right column is the wire this crate writes back, or `None`
    /// where no conditional write can express what was read.
    const CONDITIONAL_WRITES: [ConditionalWrite; 6] = [
        // SabreDAV: the length-and-digest shape its ETag plugin emits.
        (
            b"\"2d9-5f1b0c4a\"",
            Some((b"If-Match", b"\"2d9-5f1b0c4a\"")),
        ),
        // Radicale: an md5 of the stored item, quoted.
        (
            b"\"e2f0a3b1c4d5e6f708192a3b4c5d6e7f\"",
            Some((b"If-Match", b"\"e2f0a3b1c4d5e6f708192a3b4c5d6e7f\"")),
        ),
        // Calendar Server: a generated token, quoted.
        (
            b"\"c2ae9a1e-5b3a-4c1d-9f60-7ab41d0e2f33\"",
            Some((b"If-Match", b"\"c2ae9a1e-5b3a-4c1d-9f60-7ab41d0e2f33\"")),
        ),
        // A header value with the whitespace a header field is allowed to carry around it.
        (
            b"  \"63558486825\" ",
            Some((b"If-Match", b"\"63558486825\"")),
        ),
        // Google's weak validator. Strong comparison can never be satisfied by one, so there
        // is no conditional write to build — and inventing `If-Match: *` here is the bug.
        (b"W/\"63558486825\"", None),
        // An empty tag is a legal quoted string and says nothing useful, but it is the octets
        // the server chose and it round-trips as they were sent.
        (b"\"\"", Some((b"If-Match", b"\"\""))),
    ];

    #[test]
    fn what_a_server_sent_decides_what_a_conditional_write_carries() {
        let mut meter = Meter::new(Limits::DEFAULT);
        for (sent, expected) in CONDITIONAL_WRITES {
            let tag = ETag::parse(sent).unwrap();
            let revision = Revision::at(href(b"/calendars/ann/work/1.ics", &mut meter), tag);
            match (revision.precondition(), expected) {
                (Some(precondition), Some((name, value))) => {
                    assert_eq!(precondition.header_name(), name, "{sent:?}");
                    let mut written: Vec<u8> = Vec::new();
                    precondition.write_value(&mut written).unwrap();
                    assert_eq!(written, value, "{sent:?}");
                },
                (None, None) => {},
                (rendered, wanted) => panic!("{sent:?}: {rendered:?} is not {wanted:?}"),
            }
        }
    }

    #[test]
    fn an_unquoted_tag_never_becomes_a_revision_at_all() {
        // A server that writes a bare tag is refused at the only door there is: nothing here
        // takes octets and calls them a validator. What the caller is left holding is a
        // resource it knows is stored and cannot write to conditionally, which is the outcome
        // that costs a refused write rather than somebody's edit.
        let mut meter = Meter::new(Limits::DEFAULT);
        assert_eq!(
            ETag::parse(b"2d9-5f1b0c4a"),
            Err(DavError::Invalid(ValueError::EtagSyntax))
        );
        let revision = Revision::stored(href(b"/calendars/ann/work/1.ics", &mut meter));
        assert_eq!(revision.presence(), Presence::Stored);
        assert_eq!(revision.precondition(), None);
    }

    /// What each read state can ask a write to require of the stored copy.
    const PRECONDITIONS: [(Presence, Option<&[u8]>); 3] = [
        // Nothing was stored, so a create must still find nothing: `If-None-Match: *`.
        (Presence::Absent, Some(b"If-None-Match")),
        // A copy is stored and the server sent no validator, so nothing can be required.
        (Presence::Stored, None),
        // The read said nothing, which is not the same as either of the above.
        (Presence::Unknown, None),
    ];

    #[test]
    fn an_untagged_state_asks_for_what_it_can_and_no_more() {
        let mut meter = Meter::new(Limits::DEFAULT);
        for (presence, expected) in PRECONDITIONS {
            let path = href(b"/calendars/ann/work/new.ics", &mut meter);
            let revision = match presence {
                Presence::Absent => Revision::absent(path),
                Presence::Stored => Revision::stored(path),
                Presence::Unknown => Revision::unknown(path),
            };
            assert_eq!(revision.presence(), presence);
            assert_eq!(
                revision.precondition().map(Precondition::header_name),
                expected,
                "{presence:?}"
            );
        }
        // The create case renders `*` and not a tag, which is the whole of RFC 4918 section
        // 8.6's answer to "write only if it is still not there".
        let revision = Revision::absent(href(b"/calendars/ann/work/new.ics", &mut meter));
        let mut written: Vec<u8> = Vec::new();
        revision
            .precondition()
            .unwrap()
            .write_value(&mut written)
            .unwrap();
        assert_eq!(written, b"*");
    }

    /// The prefix a document used, the URI it bound, and whether a revision comes of it.
    ///
    /// The prefix column is a label and is read by nothing: `SabreDAV` writes `d:`, Radicale's
    /// `ElementTree` writes `ns0:`, Calendar Server declares a default `xmlns="DAV:"` and
    /// writes no prefix at all, and all three are one element. The fourth carries the familiar
    /// `D:` over a namespace that is not `DAV:`, which is a different element and must not
    /// produce a validator a client would then write against.
    const SPELLINGS: [(&[u8], &[u8], bool); 4] = [
        (b"d:", b"DAV:", true),
        (b"ns0:", b"DAV:", true),
        (b"", b"DAV:", true),
        (b"D:", b"http://evil.example/not-dav", false),
    ];

    #[test]
    fn a_revision_is_read_off_a_resolved_name_and_never_off_a_prefix() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        for (prefix, uri, expected) in SPELLINGS {
            let namespace = Namespace::from_uri(uri);
            let name = match ElementName::resolve(namespace, b"getetag") {
                Some(known) => PropName::Known(known),
                None => {
                    PropName::Extension(ExtensionName::new(uri, b"getetag", &mut meter).unwrap())
                },
            };
            let mut group = PropStat::new(Status::OK, limits);
            group
                .push(
                    DavProperty {
                        name,
                        value: PropValue::Entity(ETag::parse(b"\"2d9-5f1b0c4a\"").unwrap()),
                    },
                    &mut meter,
                )
                .unwrap();
            let mut response =
                DavResponse::with_propstats(href(b"/calendars/ann/work/1.ics", &mut meter), limits);
            response.push_propstat(group, &mut meter).unwrap();

            let revision = Revision::from_response(&response, limits, &mut meter).unwrap();
            assert_eq!(revision.etag().is_some(), expected, "{prefix:?} {uri:?}");
            assert_eq!(revision.precondition().is_some(), expected, "{prefix:?}");
            // Either way the resource is stored; a foreign namespace costs the validator and
            // not the knowledge that something is there.
            assert_eq!(revision.presence(), Presence::Stored);
        }
    }

    #[test]
    fn a_tag_reported_under_a_refusal_is_not_a_tag() {
        // One href, two properties, two statuses: `getetag` at 404 beside `displayname` at 200,
        // which is ordinary rather than exotic. Reading the tag across the whole response
        // regardless of status is how a client writes against a validator nobody gave it.
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut missing = PropStat::new(Status::NOT_FOUND, limits);
        missing
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Getetag),
                    value: PropValue::Entity(ETag::parse(b"\"2d9-5f1b0c4a\"").unwrap()),
                },
                &mut meter,
            )
            .unwrap();
        let mut present = PropStat::new(Status::OK, limits);
        present
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Displayname),
                    value: PropValue::Text(b"Work".as_slice().into()),
                },
                &mut meter,
            )
            .unwrap();
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/1.ics", &mut meter), limits);
        response.push_propstat(missing, &mut meter).unwrap();
        response.push_propstat(present, &mut meter).unwrap();

        let revision = Revision::from_response(&response, limits, &mut meter).unwrap();
        assert_eq!(revision.etag(), None);
        assert_eq!(revision.precondition(), None);
    }

    /// A response's own status, and what it says about whether anything is stored.
    const STATUSES: [(u16, Presence); 5] = [
        (200, Presence::Stored),
        // What a multiget and a sync report send for a member that is gone.
        (404, Presence::Absent),
        (410, Presence::Absent),
        // Not an absence: the caller may not know, which is a third answer.
        (403, Presence::Unknown),
        (500, Presence::Unknown),
    ];

    #[test]
    fn only_a_stated_absence_is_read_as_one() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        for (code, expected) in STATUSES {
            let response = DavResponse::with_status(
                href(b"/calendars/ann/work/2.ics", &mut meter),
                Status::new(code).unwrap(),
            );
            let revision = Revision::from_response(&response, limits, &mut meter).unwrap();
            assert_eq!(revision.presence(), expected, "{code}");
        }
    }

    #[test]
    fn a_server_states_a_revision_and_a_client_reads_the_same_one_back() {
        // DP-15's structural test on this type. The server direction is `push_properties` into
        // a propstat; the client direction is `from_response` out of the response that carries
        // it. One shape, two directions, and the value survives the trip — including the
        // schedule tag, which is the half an `ETag`-only model would have dropped.
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let held = Revision::at(
            href(b"/calendars/ann/work/1.ics", &mut meter),
            ETag::parse(b"\"2d9-5f1b0c4a\"").unwrap(),
        )
        .with_schedule_tag(ETag::parse(b"\"3-1\"").unwrap());

        let mut granted = PropStat::new(Status::OK, limits);
        held.push_properties(&mut granted, &mut meter).unwrap();
        assert_eq!(granted.props().len(), 2);
        let mut refused = PropStat::new(Status::FORBIDDEN, limits);
        refused
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::Empty,
                },
                &mut meter,
            )
            .unwrap();
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/1.ics", &mut meter), limits);
        response.push_propstat(granted, &mut meter).unwrap();
        response.push_propstat(refused, &mut meter).unwrap();

        let read = Revision::from_response(&response, limits, &mut meter).unwrap();
        assert_eq!(read, held);
        assert!(read.is_same_revision_as(&held));
        assert_eq!(read.digest(), held.digest());

        // The scheduling half travels under its own header, and this crate renders the value
        // and names the header without ever writing a line.
        assert_eq!(IF_SCHEDULE_TAG_MATCH, b"If-Schedule-Tag-Match");
        let mut written: Vec<u8> = Vec::new();
        read.schedule_tag()
            .unwrap()
            .write_value(&mut written)
            .unwrap();
        assert_eq!(written, b"\"3-1\"");
    }

    #[test]
    fn the_two_tags_are_independently_optional() {
        // Like a `time-range`'s two bounds, and for the same reason: a server may send either,
        // both or neither, and a model that only admits the pair describes an exchange no
        // deployed server has.
        let mut meter = Meter::new(Limits::DEFAULT);
        let path = href(b"/calendars/ann/work/1.ics", &mut meter);
        let tagged = Revision::at(path, ETag::parse(b"\"a\"").unwrap());
        assert!(tagged.etag().is_some());
        assert!(tagged.schedule_tag().is_none());

        let scheduled = Revision::stored(href(b"/calendars/ann/work/1.ics", &mut meter))
            .with_schedule_tag(ETag::parse(b"\"3-1\"").unwrap());
        assert!(scheduled.etag().is_none());
        assert!(scheduled.schedule_tag().is_some());
        // A schedule tag is not an `If-Match` validator, so it buys no ordinary precondition.
        assert_eq!(scheduled.precondition(), None);
    }

    /// Tokens four deployed servers mint, none of which this crate reads anything out of.
    const SYNC_TOKENS: [&[u8]; 4] = [
        b"http://sabre.io/ns/sync/5",
        b"http://radicale.org/ns/sync/9f8e7d6c5b4a",
        b"data:,4f2c1a90_218",
        // One that looks like a number, which is exactly the shape that tempts a reader into
        // comparing two of them for recency. RFC 6578 section 3 gives no such ordering.
        b"42",
    ];

    #[test]
    fn a_sync_token_travels_whole_and_conditions_nothing() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        for octets in SYNC_TOKENS {
            let token = SyncToken::new(octets, limits, &mut meter).unwrap();
            let revision = Revision::absent(href(b"/calendars/ann/work/3.ics", &mut meter))
                .with_sync_token(token);
            assert_eq!(revision.sync_token().map(SyncToken::as_bytes), Some(octets));
            // The collection's point in history says nothing about the resource's, so the
            // precondition is the one the absence earned and not one the token changed.
            assert_eq!(revision.precondition(), Some(Precondition::CreateOnly));
        }
    }

    #[test]
    fn the_same_revision_is_a_comparison_and_not_an_equality() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let path = b"/calendars/ann/work/1.ics".as_slice();
        let strong = stored_at(path, b"\"v1\"", &mut meter);
        let again = stored_at(path, b"\"v1\"", &mut meter);
        let moved = stored_at(path, b"\"v2\"", &mut meter);
        let weak = stored_at(path, b"W/\"v1\"", &mut meter);
        let elsewhere = stored_at(b"/calendars/bob/work/1.ics", b"\"v1\"", &mut meter);

        assert!(strong.is_same_revision_as(&again));
        assert!(!strong.is_same_revision_as(&moved));
        // Equal as values, and not the same revision: RFC 9110 section 8.8.3.2's strong
        // comparison is never satisfied by a weak tag, on either side.
        assert_eq!(weak, weak.clone());
        assert!(!weak.is_same_revision_as(&weak.clone()));
        assert!(!strong.is_same_revision_as(&weak));
        // The same octets from another resource are another resource's octets.
        assert!(!strong.is_same_revision_as(&elsewhere));
        // Two reads that both found nothing are the same revision, which is what makes a
        // create conditioned on `If-None-Match: *` idempotent.
        let empty = Revision::absent(href(path, &mut meter));
        assert!(empty.is_same_revision_as(&Revision::absent(href(path, &mut meter))));
        assert!(!empty.is_same_revision_as(&Revision::unknown(href(path, &mut meter))));
    }

    #[test]
    fn a_digest_separates_what_a_conditional_write_would_separate() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let path = b"/calendars/ann/work/1.ics".as_slice();
        let base = stored_at(path, b"\"v1\"", &mut meter);
        let moved = stored_at(path, b"\"v2\"", &mut meter);
        let weak = stored_at(path, b"W/\"v1\"", &mut meter);
        let elsewhere = stored_at(b"/calendars/bob/work/1.ics", b"\"v1\"", &mut meter);
        let scheduled = stored_at(path, b"\"v1\"", &mut meter)
            .with_schedule_tag(ETag::parse(b"\"3-1\"").unwrap());
        let token = SyncToken::new(b"data:,4f2c1a90_218", Limits::DEFAULT, &mut meter).unwrap();
        let synchronized = stored_at(path, b"\"v1\"", &mut meter).with_sync_token(token);

        let digest = base.digest();
        assert_eq!(digest, stored_at(path, b"\"v1\"", &mut meter).digest());
        for other in [&moved, &weak, &elsewhere, &scheduled, &synchronized] {
            assert_ne!(digest, other.digest(), "{other:?}");
        }
        // The field boundary is real: moving an octet from the href into the tag is a
        // different revision, and a digest without lengths would have called them one.
        let shifted = stored_at(b"/calendars/ann/work/1.ic", b"\"sv1\"", &mut meter);
        assert_ne!(digest, shifted.digest());
    }

    #[test]
    fn the_header_values_this_crate_renders_are_values_and_not_lines() {
        let mut written: Vec<u8> = Vec::new();
        write_depth_value(Depth::One, &mut written).unwrap();
        assert_eq!(written, b"1");
        written.clear();
        write_depth_value(Depth::Infinity, &mut written).unwrap();
        assert_eq!(written, b"infinity");

        written.clear();
        assert!(write_prefer_value(Prefer::ReturnMinimal, &mut written).unwrap());
        assert_eq!(written, b"return=minimal");
        written.clear();
        // Nothing preferred means no header at all, rather than a header with nothing in it.
        assert!(!write_prefer_value(Prefer::Unstated, &mut written).unwrap());
        assert!(written.is_empty());
    }
}
