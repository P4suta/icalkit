// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed access as a view over preserved text, and the guard a write goes through.
//!
//! One shape for every typed accessor, distinguishing three states: absent,
//! present-but-malformed carrying its diagnostic, and present-and-valid. `dtstart()` may not
//! differ in shape from `geo()` or from a property added later, and that is enforced by there
//! being exactly one accessor rather than by review (`docs/adr/0001`).
//!
//! Both non-absent arms carry `&'a Property`, so a caller always holds the original text next
//! to the interpretation. That is the whole answer to `GEO`, where the text is authoritative
//! and the pair of floats is derived from it; it is also why nothing decoded is cached, since
//! a second place to keep the answer is a second place for the two to disagree.

use alloc::vec::Vec;
use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter, Write};
use core::marker::PhantomData;

use ical_grammar::{Diagnostic, DiagnosticCode};

use crate::change::ParameterEdit;
use crate::gregorian::{DateTimeValue, Duration};
use crate::octets::RawText;
use crate::tree::Property;

/// The result of reading one typed value out of a component or a property.
///
/// Malformed is not an error. The property is still there, its octets are still written back,
/// and the caller is handed both the diagnostic and the text that produced it — which is what
/// lets an application show the user a meeting whose `DTSTART` it could not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View<'a, T> {
    /// No property of that identity is present.
    Absent,
    /// A property is present and its value could not be read as this type.
    Malformed {
        /// The property, with its text intact.
        source: &'a Property,
        /// Why it could not be read.
        diagnostic: Diagnostic,
    },
    /// A property is present and its value was read.
    Valid {
        /// The property, with its text intact.
        source: &'a Property,
        /// The value, derived from that text and not replacing it.
        value: T,
    },
}

impl<'a, T> View<'a, T> {
    /// The value, if there was one.
    #[must_use]
    pub fn value(self) -> Option<T> {
        match self {
            Self::Valid { value, .. } => Some(value),
            Self::Absent | Self::Malformed { .. } => None,
        }
    }

    /// The property the value was read from, present whether or not it could be read.
    #[must_use]
    pub const fn source(&self) -> Option<&'a Property> {
        match *self {
            Self::Absent => None,
            Self::Malformed { source, .. } | Self::Valid { source, .. } => Some(source),
        }
    }

    /// Why the value could not be read, if it could not be.
    #[must_use]
    pub const fn diagnostic(&self) -> Option<Diagnostic> {
        match *self {
            Self::Malformed { diagnostic, .. } => Some(diagnostic),
            Self::Absent | Self::Valid { .. } => None,
        }
    }

    /// Whether a property is there at all, readable or not.
    #[must_use]
    pub const fn is_present(&self) -> bool {
        !matches!(*self, Self::Absent)
    }

    /// Whether a value was read.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(*self, Self::Valid { .. })
    }
}

/// A value type that can be read out of a property's preserved octets.
///
/// The lifetime ties the decoded value to the octets it came from, so a borrowed view such as
/// [`TextValue`] cannot outlive the property it describes. The failure is a
/// [`DiagnosticCode`] rather than a message, because the emission site owns the location and
/// the severity and the decoder owns only what was wrong.
pub trait DecodeValue<'a>: Sized {
    /// Read this type out of `bytes`, or say which diagnostic describes the failure.
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode>;

    /// Read this type out of a whole property, for a value whose shape its parameters decide.
    ///
    /// One value type needs this and the rest must not: RFC 5545 makes a date-time's zone a
    /// `TZID` parameter rather than part of the value, so a decoder handed only the octets
    /// cannot tell a floating time from a zoned one — and a caller that read one and wrote it
    /// back would silently drop the zone. `docs/adr/0001` requires the two to be inseparable,
    /// which is a requirement about the read side as much as the write side.
    ///
    /// The default is the value's own octets and nothing else, so adding this changed no
    /// existing codec. What an implementation may read is this property's parameters and its
    /// value; the property's *name* is not a question a value type is allowed to ask, because
    /// the two accessor levels are named on different axes and a decoder that looked at the
    /// name would be a property accessor wearing a value type's clothes.
    fn decode_property(property: &'a Property) -> Result<Self, DiagnosticCode> {
        Self::decode_value(property.value_text().as_bytes())
    }
}

/// A value type that can be written into a property.
///
/// Two methods rather than one, because a value's shape decides some of its parameters. RFC
/// 5545 makes `VALUE` and `TZID` a function of what a date-time is, so converting a zoned
/// `DTSTART` to a date has to emit `VALUE=DATE` and drop the stale `TZID` — and a trait that
/// could only write the value would leave the syntactically invalid pairing behind.
/// [`EncodeValue::coupled_parameters`] has no default implementation on purpose: a default
/// would let a value type added later silently emit nothing, which is precisely the
/// per-property judgment call the transition table exists to replace.
pub trait EncodeValue {
    /// Write this value's octets into `out`.
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError>;

    /// State the parameters this value's shape implies, as assignments and unassignments.
    ///
    /// Parameters that are *not* a function of the value — `RANGE`, `FBTYPE`, `X-`
    /// parameters — must not appear here. They belong to the caller and survive a write
    /// untouched.
    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>);
}

/// The buffer a value is encoded into.
///
/// Octet-shaped, with a [`core::fmt::Write`] implementation on top, because `core` has no
/// `io::Write` and `core::fmt::Write` alone takes `&str` — which is exactly what storage is
/// not. An encoder that has a formattable value uses `write!`; one that has octets pushes
/// them.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueBuf {
    /// The octets written so far.
    bytes: Vec<u8>,
}

impl ValueBuf {
    /// An empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Append octets.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Append one octet.
    pub fn push_octet(&mut self, octet: u8) {
        self.bytes.push(octet);
    }

    /// The octets written so far.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many octets have been written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Discard everything written so far, keeping the capacity.
    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    /// Take the octets as storage, without copying them again.
    #[must_use]
    pub fn into_raw_text(self) -> RawText {
        RawText::from_vec(self.bytes)
    }

    /// Take the octets.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for ValueBuf {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.push_bytes(text.as_bytes());
        Ok(())
    }
}

/// Why a write was refused.
///
/// This is the one place this crate rejects caller input outright rather than diagnosing it,
/// and it is a write-side check, so it costs the round-trip guarantee nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MutationError {
    /// There is no such property to write through.
    Absent,
    /// The octets held a control character RFC 5545 section 3.1 excludes from a value.
    ///
    /// Refused rather than escaped, because a caller that could write a bare `CRLF` into a
    /// value could write a whole new content line after it: a `SUMMARY` taken from a web form
    /// becoming a second `ATTENDEE` is a real injection and not a theoretical one.
    IllegalControlCharacter,
    /// A replacement content line was empty, unparsable, or more than one line.
    MalformedReplacement,
    /// The value cannot be written in any form RFC 5545 defines.
    NotRepresentable,
    /// The line the write would author is a component boundary rather than a property.
    ///
    /// A property named `BEGIN` or `END` is written as a line the next reader opens or closes
    /// a component on, so a write that produced one would move every line after it into a
    /// component nobody added. The reader stores such a line — it has to, since the file holds
    /// one — and this crate declines to author one: a component is built with
    /// [`Component::create`](crate::Component::create), which writes both of its boundaries.
    ComponentBoundary,
    /// The written value exceeded the caller's per-value bound.
    ValueTooLarge {
        /// The per-value bound in octets.
        limit: u32,
    },
}

impl Display for MutationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Absent => formatter.write_str("no such property"),
            Self::IllegalControlCharacter => {
                formatter.write_str("a value may not hold a control character")
            },
            Self::MalformedReplacement => {
                formatter.write_str("a replacement must be exactly one content line")
            },
            Self::NotRepresentable => formatter.write_str("the value has no RFC 5545 form"),
            Self::ComponentBoundary => {
                formatter.write_str("a property named BEGIN or END is a component boundary")
            },
            Self::ValueTooLarge { limit } => {
                write!(formatter, "the value exceeds {limit} octets")
            },
        }
    }
}

impl Error for MutationError {}

/// The value types RFC 5545 section 3.2.20's `VALUE` parameter can name.
///
/// `#[non_exhaustive]` because a later RFC may add one, and a `VALUE` this crate does not
/// know is a diagnostic rather than a refusal: the property keeps its octets either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ValueType {
    /// Section 3.3.1.
    Binary,
    /// Section 3.3.2.
    Boolean,
    /// Section 3.3.3.
    CalAddress,
    /// Section 3.3.4.
    Date,
    /// Section 3.3.5.
    DateTime,
    /// Section 3.3.6.
    Duration,
    /// Section 3.3.7.
    Float,
    /// Section 3.3.8.
    Integer,
    /// Section 3.3.9.
    Period,
    /// Section 3.3.10. Its grammar belongs to `ical-recur`; here it names preserved text.
    Recur,
    /// Section 3.3.11.
    Text,
    /// Section 3.3.12.
    Time,
    /// Section 3.3.13.
    Uri,
    /// Section 3.3.14.
    UtcOffset,
}

impl ValueType {
    /// Every value type this crate knows, paired with the name a `VALUE` parameter spells.
    const SPELLINGS: [(Self, &'static [u8]); 14] = [
        (Self::Binary, b"BINARY"),
        (Self::Boolean, b"BOOLEAN"),
        (Self::CalAddress, b"CAL-ADDRESS"),
        (Self::Date, b"DATE"),
        (Self::DateTime, b"DATE-TIME"),
        (Self::Duration, b"DURATION"),
        (Self::Float, b"FLOAT"),
        (Self::Integer, b"INTEGER"),
        (Self::Period, b"PERIOD"),
        (Self::Recur, b"RECUR"),
        (Self::Text, b"TEXT"),
        (Self::Time, b"TIME"),
        (Self::Uri, b"URI"),
        (Self::UtcOffset, b"UTC-OFFSET"),
    ];

    /// The name a `VALUE` parameter spells this type as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Binary => b"BINARY",
            Self::Boolean => b"BOOLEAN",
            Self::CalAddress => b"CAL-ADDRESS",
            Self::Date => b"DATE",
            Self::DateTime => b"DATE-TIME",
            Self::Duration => b"DURATION",
            Self::Float => b"FLOAT",
            Self::Integer => b"INTEGER",
            Self::Period => b"PERIOD",
            Self::Recur => b"RECUR",
            Self::Text => b"TEXT",
            Self::Time => b"TIME",
            Self::Uri => b"URI",
            Self::UtcOffset => b"UTC-OFFSET",
        }
    }

    /// The value type `name` spells, or `None` for one this crate does not know.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Self::SPELLINGS
            .iter()
            .find(|(_, spelling)| spelling.eq_ignore_ascii_case(name))
            .map(|(kind, _)| *kind)
    }
}

/// A borrowed `TEXT` value, before its escapes are resolved.
///
/// Validation comes before unescaping, and that order is the whole defense against a
/// multi-byte lead octet whose trail was eaten by an escape: every substitution RFC 5545
/// section 3.3.11 defines is ASCII, so none can satisfy a UTF-8 continuation requirement, and
/// an orphaned lead octet fails deterministically instead of being completed by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextValue<'a> {
    /// The value's octets, escapes and all.
    bytes: &'a [u8],
}

impl<'a> TextValue<'a> {
    /// A view over octets that are a `TEXT` value.
    ///
    /// Public because the escaping lives on the far side of a crate boundary from the storage
    /// it describes.
    #[must_use]
    pub const fn from_bytes(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The octets, escapes and all, exactly as they will be written back.
    #[must_use]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

/// A borrowed `BINARY` value, before it is read out of the base 64 it is written in.
///
/// [`TextValue`]'s posture, for [`TextValue`]'s reason. RFC 5545 section 3.3.1 fixes the
/// alphabet and not the line breaks, the padding a producer chose, or the case of the octets
/// it padded with, so the written text is authoritative and the decoded octets are derived
/// from it. Reading is a step a caller asks for and never one storage takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BinaryValue<'a> {
    /// The base 64 text, exactly as it will be written back.
    bytes: &'a [u8],
}

impl<'a> BinaryValue<'a> {
    /// A view over octets that are a `BINARY` value.
    #[must_use]
    pub const fn from_bytes(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The base 64 text, exactly as it will be written back.
    #[must_use]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

/// A borrowed `URI`, as RFC 5545 section 3.3.13 writes one.
///
/// Section 3.3.3's `CAL-ADDRESS` is this type as well: that section defines a calendar address
/// as a URI and adds no syntax of its own, so a second type would be a second name for one
/// grammar. What tells them apart is the property, which the caller is holding either way.
///
/// Borrowed and unresolved. A URI's percent-encoding, its case-insensitive scheme and its
/// case-sensitive path are all things a normalizer would rewrite, and rewriting one is how a
/// `mailto:` stops matching the `ATTENDEE` a scheduling reply names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UriValue<'a> {
    /// The URI, exactly as it will be written back.
    bytes: &'a [u8],
}

impl<'a> UriValue<'a> {
    /// A view over octets that are a `URI`.
    #[must_use]
    pub const fn from_bytes(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The URI, exactly as it will be written back.
    #[must_use]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

/// A span between two points in time, as RFC 5545 section 3.3.9 writes one.
///
/// Two variants because section 3.3.9's ABNF has two productions and they are not
/// interchangeable: `period-explicit` names an end and `period-start` names a length, and a
/// producer that wrote one gets it back rather than the other. Which one arrived is therefore
/// part of the value and not a normalization this crate is free to take.
///
/// The bounds are [`DateTimeValue`]s so that a `PERIOD` under a `TZID` carries the zone the
/// property gave it, exactly as a bare `DTSTART` does. Section 3.3.9 permits no `DATE` at
/// either end; refusing one is the decoder's business rather than this type's, because a value
/// that cannot be read is still a value that must be written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Period<'a> {
    /// `period-explicit`: a start and an end.
    Explicit {
        /// When it begins.
        start: DateTimeValue<'a>,
        /// When it ends.
        end: DateTimeValue<'a>,
    },
    /// `period-start`: a start and how long it lasts.
    Starting {
        /// When it begins.
        start: DateTimeValue<'a>,
        /// How long it lasts, which section 3.3.9 requires be positive.
        duration: Duration,
    },
}

impl<'a> Period<'a> {
    /// When the span begins, whichever form it was written in.
    #[must_use]
    pub const fn start(self) -> DateTimeValue<'a> {
        match self {
            Self::Explicit { start, .. } | Self::Starting { start, .. } => start,
        }
    }
}

/// A latitude and longitude pair, as RFC 5545 section 3.8.1.6 writes one.
///
/// Readable and not writable, which is the honest answer for a value whose text cannot be
/// reproduced from its parsed form: a `GEO` that arrived as `37.386013` must be written back
/// as `37.386013` and not as whatever the shortest round-trip formatting of the nearest `f64`
/// happens to be.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Geo {
    /// Degrees north, negative for south.
    latitude: f64,
    /// Degrees east, negative for west.
    longitude: f64,
}

impl Geo {
    /// The pair.
    #[must_use]
    pub const fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
        }
    }

    /// Degrees north, negative for south.
    #[must_use]
    pub const fn latitude(self) -> f64 {
        self.latitude
    }

    /// Degrees east, negative for west.
    #[must_use]
    pub const fn longitude(self) -> f64 {
        self.longitude
    }
}

/// A short-lived handle naming exactly one property, through which a write may happen.
///
/// The guard borrows the whole component mutably and names one property, so reaching another
/// property's storage requires visibly widening a signature. That is a borrow the compiler
/// enforces, unlike a returned marker value a caller may simply drop.
///
/// `T` names the typed view a caller writes *through*; it does not bound what the guard
/// reaches. The unit the guard scopes is the whole property — its name, its parameters and
/// its value together — because RFC 5545 makes some of those parameters a function of the
/// value, and a value-only guard would leave an invalid pairing behind.
pub struct PropertyMut<'a, T> {
    /// The one property this guard names.
    property: &'a mut Property,
    /// The typed view writes go through. Carried for inference only; no value is stored.
    written_as: PhantomData<fn(T)>,
}

impl<'a, T> PropertyMut<'a, T> {
    /// A guard over `property`.
    #[must_use]
    pub fn new(property: &'a mut Property) -> Self {
        Self {
            property,
            written_as: PhantomData,
        }
    }

    /// The property as it stands, with whatever text it still has.
    #[must_use]
    pub fn property(&self) -> &Property {
        self.property
    }

    /// The property as it stands, mutably.
    ///
    /// This is the guard's whole reach, and it is one property. Every write path is built on
    /// it, and each one must discard the preserved layout of this property and of nothing
    /// else. It grants nothing a caller holding `&mut Property` did not already have; what it
    /// withholds is every *other* property in the component, which stays borrowed for as long
    /// as this guard lives.
    pub fn property_mut(&mut self) -> &mut Property {
        self.property
    }
}

impl<T> Debug for PropertyMut<'_, T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PropertyMut")
            .field("property", &self.property)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use ical_grammar::{Diagnostic, DiagnosticCode, Location, Severity};

    use super::{MutationError, TextValue, ValueBuf, ValueType, View};
    use crate::octets::RawText;
    use crate::tree::Property;

    /// A property with nothing on it, for the shape tests below.
    fn bare_property() -> Property {
        Property::new(
            RawText::from_bytes(b"SUMMARY"),
            alloc::vec::Vec::new(),
            RawText::from_bytes(b"hi"),
            ical_grammar::LineLayout::canonical(ical_grammar::LineEnding::CANONICAL),
        )
    }

    #[test]
    fn a_malformed_value_still_hands_back_the_text_it_came_from() {
        let property = bare_property();
        let diagnostic = Diagnostic::new(
            DiagnosticCode::MalformedDate,
            Severity::Violation,
            Location::at_offset(0),
        );
        let view: View<'_, u32> = View::Malformed {
            source: &property,
            diagnostic,
        };
        assert!(view.is_present(), "malformed is present, not absent");
        assert!(!view.is_valid());
        assert_eq!(view.diagnostic(), Some(diagnostic));
        assert!(view.source().is_some());
        assert_eq!(view.value(), None);
    }

    #[test]
    fn absence_carries_neither_a_source_nor_a_diagnostic() {
        let view: View<'_, u32> = View::Absent;
        assert!(!view.is_present());
        assert!(view.source().is_none());
        assert!(view.diagnostic().is_none());
    }

    #[test]
    fn a_value_buffer_takes_octets_and_formatted_text_alike() {
        let mut buffer = ValueBuf::new();
        buffer.push_bytes(b"20260815T");
        write!(&mut buffer, "{:02}{:02}{:02}Z", 12, 0, 0).unwrap();
        assert_eq!(buffer.as_bytes(), b"20260815T120000Z");
        assert_eq!(buffer.len(), 16);

        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn value_type_names_round_trip_case_insensitively() {
        for (kind, spelling) in ValueType::SPELLINGS {
            assert_eq!(ValueType::from_name(spelling), Some(kind));
            assert_eq!(kind.as_bytes(), spelling);
        }
        assert_eq!(
            ValueType::from_name(b"date-time"),
            Some(ValueType::DateTime)
        );
        assert_eq!(ValueType::from_name(b"X-VENDOR"), None);
    }

    #[test]
    fn a_text_view_holds_the_escapes_rather_than_resolving_them_early() {
        let value = TextValue::from_bytes(b"a\\,b");
        assert_eq!(value.as_bytes(), b"a\\,b");
    }

    #[test]
    fn a_refusal_says_which_refusal_it_is() {
        assert_ne!(
            MutationError::Absent,
            MutationError::IllegalControlCharacter
        );
    }
}
