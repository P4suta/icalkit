// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The identity of a property name, normalized for lookup and never for writing.
//!
//! Deliberately not a closed enum. A closed enum is const-constructible and orderable and it
//! re-introduces the known/unknown split the tree's shape was spent to avoid: RFC 5545 puts
//! no limit on `X-` properties, and a property from an RFC published after this code has to
//! be as reachable as `DTSTART`. A `&'static` / owned split gets both without the split.
//!
//! The identity is normalized because RFC 5545 names are case-insensitive. The *spelling*
//! stays on the property, so a producer that wrote `dtstart` gets `dtstart` back.

use alloc::boxed::Box;
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};

/// Where a property identity's octets live.
///
/// Private because the two cases must never be observable: a `&'static` name and an owned
/// one that spell the same property are one identity, and code that could tell them apart
/// could sort them apart.
#[derive(Clone, Debug)]
enum Name {
    /// A name known when this crate was compiled.
    Static(&'static [u8]),
    /// A name read from a calendar, uppercased on the way in.
    Owned(Box<[u8]>),
}

/// The normalized identity of a property name.
///
/// `Ord`, `Eq` and `Hash` are hand-written over the octets rather than derived. A derived
/// `Ord` would sort by representation, so a vendor property read from a file would sort into
/// a different place than the same name written as a constant — the kind of bug that only
/// surfaces once one of them reaches a `BTreeMap`.
#[derive(Clone, Debug)]
pub struct PropertyId(Name);

impl PropertyId {
    /// An identity for a name known at compile time.
    ///
    /// `const`, which is what makes the well-known constants below possible, and which is
    /// also why this cannot uppercase: the caller passes an already-uppercase name. Every
    /// name that arrives from a calendar goes through [`PropertyId::from_name`] instead.
    #[must_use]
    pub const fn from_static(name: &'static [u8]) -> Self {
        Self(Name::Static(name))
    }

    /// An identity for a name read from a calendar, ASCII-uppercased.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        let mut owned = Box::<[u8]>::from(name);
        owned.make_ascii_uppercase();
        Self(Name::Owned(owned))
    }

    /// The normalized octets. Every comparison in this type is stated over these.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Name::Static(name) => name,
            Name::Owned(name) => name,
        }
    }

    /// Whether `name`, as written in a calendar, is this identity.
    #[must_use]
    pub fn matches(&self, name: &[u8]) -> bool {
        self.as_bytes().eq_ignore_ascii_case(name)
    }
}

impl PartialEq for PropertyId {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for PropertyId {}

impl PartialOrd for PropertyId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PropertyId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl Hash for PropertyId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl Display for PropertyId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        // A property name is ASCII by grammar, so anything that is not is a name this crate
        // was handed rather than one RFC 5545 describes. It is shown as an escape rather
        // than replaced, because replacing it would make two distinct names print alike.
        for octet in self.as_bytes() {
            if octet.is_ascii_graphic() {
                formatter.write_str(core::str::from_utf8(&[*octet]).unwrap_or("?"))?;
            } else {
                write!(formatter, "\\x{octet:02X}")?;
            }
        }
        Ok(())
    }
}

/// The well-known names, as constants.
///
/// These are a convenience over [`PropertyId::from_static`] and never a closed set: a name
/// missing from this list is reached exactly the same way, and reaches the same storage.
impl PropertyId {
    /// RFC 5545 section 3.7.2.
    pub const PRODID: Self = Self::from_static(b"PRODID");
    /// RFC 5545 section 3.7.4.
    pub const VERSION: Self = Self::from_static(b"VERSION");
    /// RFC 5545 section 3.7.1.
    pub const CALSCALE: Self = Self::from_static(b"CALSCALE");
    /// RFC 5545 section 3.7.2, and RFC 5546's scheduling method.
    pub const METHOD: Self = Self::from_static(b"METHOD");
    /// RFC 5545 section 3.8.4.7.
    pub const UID: Self = Self::from_static(b"UID");
    /// RFC 5545 section 3.8.7.2.
    pub const DTSTAMP: Self = Self::from_static(b"DTSTAMP");
    /// RFC 5545 section 3.8.2.4.
    pub const DTSTART: Self = Self::from_static(b"DTSTART");
    /// RFC 5545 section 3.8.2.2.
    pub const DTEND: Self = Self::from_static(b"DTEND");
    /// RFC 5545 section 3.8.2.3.
    pub const DURATION: Self = Self::from_static(b"DURATION");
    /// RFC 5545 section 3.8.1.12.
    pub const SUMMARY: Self = Self::from_static(b"SUMMARY");
    /// RFC 5545 section 3.8.1.5.
    pub const DESCRIPTION: Self = Self::from_static(b"DESCRIPTION");
    /// RFC 5545 section 3.8.1.7.
    pub const LOCATION: Self = Self::from_static(b"LOCATION");
    /// RFC 5545 section 3.8.1.6.
    pub const GEO: Self = Self::from_static(b"GEO");
    /// RFC 5545 section 3.8.7.4.
    pub const SEQUENCE: Self = Self::from_static(b"SEQUENCE");
    /// RFC 5545 section 3.8.1.11.
    pub const STATUS: Self = Self::from_static(b"STATUS");
    /// RFC 5545 section 3.8.2.7.
    pub const TRANSP: Self = Self::from_static(b"TRANSP");
    /// RFC 5545 section 3.8.1.3.
    pub const CLASS: Self = Self::from_static(b"CLASS");
    /// RFC 5545 section 3.8.1.2.
    pub const CATEGORIES: Self = Self::from_static(b"CATEGORIES");
    /// RFC 5545 section 3.8.1.9.
    pub const PRIORITY: Self = Self::from_static(b"PRIORITY");
    /// RFC 5545 section 3.8.4.3.
    pub const ORGANIZER: Self = Self::from_static(b"ORGANIZER");
    /// RFC 5545 section 3.8.4.1.
    pub const ATTENDEE: Self = Self::from_static(b"ATTENDEE");
    /// RFC 5545 section 3.8.1.1.
    pub const ATTACH: Self = Self::from_static(b"ATTACH");
    /// RFC 5545 section 3.8.5.3. Its value stays preserved text here; the grammar is
    /// `ical-recur`'s.
    pub const RRULE: Self = Self::from_static(b"RRULE");
    /// RFC 5545 section 3.8.5.2.
    pub const RDATE: Self = Self::from_static(b"RDATE");
    /// RFC 5545 section 3.8.5.1.
    pub const EXDATE: Self = Self::from_static(b"EXDATE");
    /// RFC 5545 section 3.8.4.4.
    pub const RECURRENCE_ID: Self = Self::from_static(b"RECURRENCE-ID");
    /// RFC 5545 section 3.8.4.5.
    pub const RELATED_TO: Self = Self::from_static(b"RELATED-TO");
    /// RFC 5545 section 3.8.7.1.
    pub const CREATED: Self = Self::from_static(b"CREATED");
    /// RFC 5545 section 3.8.7.3.
    pub const LAST_MODIFIED: Self = Self::from_static(b"LAST-MODIFIED");
    /// RFC 5545 section 3.8.4.6.
    pub const URL: Self = Self::from_static(b"URL");
    /// RFC 5545 section 3.8.1.4.
    pub const COMMENT: Self = Self::from_static(b"COMMENT");
    /// RFC 5545 section 3.8.4.2.
    pub const CONTACT: Self = Self::from_static(b"CONTACT");
    /// RFC 5545 section 3.8.8.3.
    pub const REQUEST_STATUS: Self = Self::from_static(b"REQUEST-STATUS");
    /// RFC 5545 section 3.8.2.6.
    pub const FREEBUSY: Self = Self::from_static(b"FREEBUSY");
    /// RFC 5545 section 3.8.3.1.
    pub const TZID: Self = Self::from_static(b"TZID");
    /// RFC 5545 section 3.8.3.3 and section 3.8.3.4.
    pub const TZOFFSETFROM: Self = Self::from_static(b"TZOFFSETFROM");
    /// RFC 5545 section 3.8.3.4.
    pub const TZOFFSETTO: Self = Self::from_static(b"TZOFFSETTO");
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::PropertyId;

    #[test]
    fn a_static_name_and_a_read_name_are_one_identity() {
        let read = PropertyId::from_name(b"dtstart");
        assert_eq!(read, PropertyId::DTSTART);
        assert_eq!(read.cmp(&PropertyId::DTSTART), core::cmp::Ordering::Equal);
    }

    #[test]
    fn identities_sort_by_name_and_not_by_representation() {
        let vendor = PropertyId::from_name(b"x-microsoft-cdo-busystatus");
        assert!(
            PropertyId::DTSTART < vendor,
            "D sorts before X whoever wrote it"
        );
        assert_eq!(vendor.as_bytes(), b"X-MICROSOFT-CDO-BUSYSTATUS");
    }

    #[test]
    fn matching_is_case_insensitive_against_the_spelling_in_the_file() {
        assert!(PropertyId::SUMMARY.matches(b"Summary"));
        assert!(!PropertyId::SUMMARY.matches(b"SUMMARIES"));
    }

    #[test]
    fn a_name_that_is_not_ascii_prints_without_becoming_another_name() {
        let odd = PropertyId::from_name(b"X-\xff");
        assert_eq!(format!("{odd}"), "X-\\xFF");
    }
}
