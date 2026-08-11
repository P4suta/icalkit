// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where in the input a diagnostic points, for the diagnostics that point anywhere.
//!
//! A span is octets, not lines and columns. Unfolding makes a line number a claim about
//! which of two numberings is meant — the physical line the octet sits on, or the logical
//! content line it belongs to — and the two disagree on every folded file. An offset into
//! the octets the caller handed in is the one answer both numberings can be derived from,
//! and it is the only one this crate can produce without keeping a second index.

/// A half-open range of octets in the input a reader was handed.
///
/// Offsets are counted from the first octet the caller passed, never from the start of an
/// unfolded buffer, so a caller can quote the bytes it owns without reconstructing anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// Offset of the first octet, inclusive.
    start: u64,
    /// Offset one past the last octet.
    end: u64,
}

impl Span {
    /// The span covering `start .. end`, or `None` when the range runs backwards.
    ///
    /// Backwards is rejected rather than swapped: a reader that computed its end before its
    /// start has a bug, and silently reordering the two hides it behind a plausible span.
    #[must_use]
    pub const fn new(start: u64, end: u64) -> Option<Self> {
        if start <= end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// The span covering exactly the octet at `offset`.
    ///
    /// At `u64::MAX` the end saturates and the span is empty, which is the honest answer for
    /// an octet that cannot be addressed rather than a wrapped range pointing at the start of
    /// the file.
    #[must_use]
    pub const fn at(offset: u64) -> Self {
        Self {
            start: offset,
            end: offset.saturating_add(1),
        }
    }

    /// Offset of the first octet, inclusive.
    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    /// Offset one past the last octet.
    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }

    /// The number of octets covered.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span covers no octet at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// Where a diagnostic points, including the case where it points nowhere.
///
/// [`Location::NOWHERE`] is not a failure to record an offset. A diagnostic about a
/// recurrence instance or a zone transition concerns something that exists at no offset in
/// any file, and giving those a plausible-looking span would be worse than admitting there
/// is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    /// The octets concerned, when the diagnostic concerns octets.
    span: Option<Span>,
}

impl Location {
    /// The location of something that is not in the input.
    pub const NOWHERE: Self = Self { span: None };

    /// The location covering `span`.
    #[must_use]
    pub const fn at(span: Span) -> Self {
        Self { span: Some(span) }
    }

    /// The location covering exactly the octet at `offset`.
    #[must_use]
    pub const fn at_offset(offset: u64) -> Self {
        Self::at(Span::at(offset))
    }

    /// The octets concerned, or `None` when the diagnostic is not about octets.
    #[must_use]
    pub const fn span(self) -> Option<Span> {
        self.span
    }

    /// Whether this location names octets in the input.
    #[must_use]
    pub const fn is_known(self) -> bool {
        self.span.is_some()
    }
}

impl From<Span> for Location {
    fn from(span: Span) -> Self {
        Self::at(span)
    }
}

#[cfg(test)]
mod tests {
    use super::{Location, Span};

    #[test]
    fn a_backwards_range_is_refused_rather_than_reordered() {
        assert_eq!(Span::new(9, 4), None);
        assert_eq!(Span::new(4, 4).map(Span::is_empty), Some(true));
    }

    #[test]
    fn a_single_octet_span_covers_one_octet() {
        let span = Span::at(7);
        assert_eq!((span.start(), span.end(), span.len()), (7, 8, 1));
    }

    #[test]
    fn the_last_addressable_octet_saturates_instead_of_wrapping() {
        let span = Span::at(u64::MAX);
        assert!(span.is_empty(), "an unaddressable octet must not wrap to 0");
    }

    #[test]
    fn nowhere_is_a_location_and_not_a_missing_one() {
        assert!(!Location::NOWHERE.is_known());
        assert_eq!(Location::NOWHERE.span(), None);
        assert!(Location::at_offset(3).is_known());
    }
}
