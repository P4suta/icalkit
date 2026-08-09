// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The channel for "nothing could be built at all".
//!
//! [`ParseError`] is deliberately small, and every variant of it is a caller-stated bound
//! that guards memory rather than a judgment about the calendar. A specification violation
//! never travels here; it is a [`Diagnostic`](crate::Diagnostic) attached to the item it
//! concerns, because a parser that returns an error for a malformed `DTSTART` is a parser
//! that threw away the rest of the file (`docs/adr/0009`).
//!
//! Both types implement [`core::error::Error`] by hand. The core crates declare no
//! dependency, so a derive is not available, and `core::error::Error` is implemented
//! unconditionally so that error interoperability does not wait on the `std` feature.

use core::error::Error;
use core::fmt::{self, Display, Formatter};

/// A refusal to build anything, always because a caller-stated bound was crossed.
///
/// The bound is reported with the limit that was crossed rather than the value that crossed
/// it: the limit is the number the caller can act on, and the value is attacker-controlled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParseError {
    /// The input was longer than `limit` octets, or its parse charged more than that.
    InputTooLarge {
        /// The budget in octets, as the caller set it.
        limit: u64,
    },
    /// One property value exceeded `limit` octets.
    ///
    /// This is fatal rather than a truncation on purpose. Truncating writes back fewer
    /// octets than were read, which contradicts the round-trip guarantee directly, and a
    /// truncated value is indistinguishable from a preserved one at the serializer.
    ValueTooLarge {
        /// The per-value bound in octets.
        limit: u32,
    },
    /// One content line's name and parameters together exceeded `limit` octets.
    ///
    /// Names and parameters are reassembled across folds through a bounded scratch buffer,
    /// so unlike a value they have a ceiling the reader must be able to state.
    HeaderTooLarge {
        /// The per-line header bound in octets.
        limit: u32,
    },
    /// One content line carried more parameters than `limit`.
    ///
    /// A separate variant from `HeaderTooLarge` because the two bounds are raised separately.
    /// Reporting a parameter count against the octet ceiling would name a number the caller
    /// can raise without the refusal ever going away.
    TooManyParameters {
        /// The per-line parameter-count bound.
        limit: u32,
    },
    /// The document held more properties and components together than `limit`.
    TooManyItems {
        /// The item-count bound.
        limit: u32,
    },
    /// Components nested deeper than `limit`.
    TooDeep {
        /// The nesting-depth bound.
        limit: u16,
    },
}

impl Display for ParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InputTooLarge { limit } => {
                write!(formatter, "input exceeds the {limit} octet budget")
            },
            Self::ValueTooLarge { limit } => {
                write!(formatter, "a property value exceeds {limit} octets")
            },
            Self::HeaderTooLarge { limit } => {
                write!(formatter, "a content line header exceeds {limit} octets")
            },
            Self::TooManyParameters { limit } => {
                write!(
                    formatter,
                    "a content line carries more than {limit} parameters"
                )
            },
            Self::TooManyItems { limit } => {
                write!(formatter, "the document holds more than {limit} items")
            },
            Self::TooDeep { limit } => {
                write!(formatter, "components nest deeper than {limit}")
            },
        }
    }
}

impl Error for ParseError {}

/// Which bound a charge against the shared ledger crossed.
///
/// The dimension is named rather than left field-free because a caller tunes a policy per
/// dimension: `ical-dav` distinguishes a body that was too long from a nesting that was too
/// deep from an `href` that was too long, and "some bound was crossed" is not something any
/// of those three can be raised or lowered from. A discriminant costs no allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum LimitExceeded {
    /// The octet budget for the whole operation.
    Budget,
    /// The number of items — properties and components — in one document.
    Items,
    /// The nesting depth of components or of XML elements.
    Depth,
    /// The number of XML elements in one request or response.
    Elements,
    /// The length of one `href`.
    Href,
}

impl Display for LimitExceeded {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let dimension = match *self {
            Self::Budget => "octet budget",
            Self::Items => "item count",
            Self::Depth => "nesting depth",
            Self::Elements => "element count",
            Self::Href => "href length",
        };
        write!(formatter, "the {dimension} limit was exceeded")
    }
}

impl Error for LimitExceeded {}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::{LimitExceeded, ParseError};

    #[test]
    fn an_error_names_the_limit_and_not_the_value_that_crossed_it() {
        let rendered = format!("{}", ParseError::ValueTooLarge { limit: 1_048_576 });
        assert!(rendered.contains("1048576"), "{rendered}");
    }

    #[test]
    fn every_dimension_renders_distinctly() {
        let budget = format!("{}", LimitExceeded::Budget);
        let depth = format!("{}", LimitExceeded::Depth);
        assert_ne!(budget, depth);
    }
}
