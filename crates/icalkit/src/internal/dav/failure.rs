// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The channel for "this exchange cannot be completed".
//!
//! [`DavError`] is `docs/adr/0009`'s structural channel raised one layer: nothing in it is
//! recoverable by ignoring it. A body a reader cannot finish, an element the build cannot
//! honor, a value that contradicts itself, a sink with no room left — each of those ends the
//! read or the write, and there is no partial answer to hand back.
//!
//! Everything tolerable travels the other channel instead, as an `crate::internal::core::Diagnostic` on
//! the caller's sink with the read continuing: a foreign element skipped per RFC 4918
//! section 17, a property this crate has no model for, a `calendar-data` payload that had to
//! be copied. A reader that turned any of those into an error would discard a multistatus a
//! server was entitled to send.
//!
//! Every type here is `Copy` and allocates nothing, so a refusal costs no memory on the path
//! where memory has already run out.

use core::error::Error;
use core::fmt::{self, Display, Formatter};

use crate::internal::core::LimitExceeded;

use crate::internal::dav::element::ElementName;

/// A refusal that ends a read or a write.
///
/// The variants divide by *who* has to act: a limit is the caller's policy, a syntax error is
/// the peer's body, an unsupported element is this build's own configuration, an unexpected
/// element is the peer using a legal name in an illegal place, an invalid value is a
/// self-contradicting value, and a full sink is the caller's buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DavError {
    /// A caller-stated bound was crossed, and the dimension says which one.
    Limit(LimitExceeded),
    /// The body is not the XML this crate reads.
    Syntax(SyntaxError),
    /// The element is in the vocabulary and this build cannot honor it.
    ///
    /// Distinct from a foreign element, which is skipped with a diagnostic. A row exists for
    /// every element unconditionally, precisely so that a build without a feature refuses the
    /// `REPORT` it cannot answer instead of quietly ignoring it.
    Unsupported(ElementName),
    /// The element is in the vocabulary and is not permitted where it appeared.
    Unexpected(ElementName),
    /// An element outside the closed vocabulary appeared where none may be tolerated.
    ///
    /// A foreign element is normally skipped with a diagnostic (RFC 4918 section 17), so this
    /// is what [`crate::internal::dav::UnknownPolicy::Reject`] raises, plus the two places where skipping is
    /// not an option at all: a document whose root is foreign, where skipping would leave no
    /// body, and a foreign element in a position whose content model this crate reads
    /// structurally.
    ///
    /// It carries nothing. Naming the element would mean holding octets borrowed from a body
    /// this type outlives, and the whole point of a foreign element is that it has no row in
    /// [`ElementName`] to name it by. What a caller wants is on the diagnostic sink beside it;
    /// what a caller needs here is to know the body was refused for extending the vocabulary
    /// rather than for being ill-formed.
    Foreign,
    /// A value was read and contradicts itself or its grammar.
    Invalid(ValueError),
    /// The caller's output buffer has no room left.
    Output(SinkFull),
}

impl From<LimitExceeded> for DavError {
    fn from(exceeded: LimitExceeded) -> Self {
        Self::Limit(exceeded)
    }
}

impl From<SyntaxError> for DavError {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

impl From<ValueError> for DavError {
    fn from(error: ValueError) -> Self {
        Self::Invalid(error)
    }
}

impl From<SinkFull> for DavError {
    fn from(full: SinkFull) -> Self {
        Self::Output(full)
    }
}

impl Display for DavError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Limit(exceeded) => write!(formatter, "{exceeded}"),
            Self::Syntax(error) => write!(formatter, "{error}"),
            Self::Unsupported(name) => {
                write!(formatter, "this build cannot honor <{}>", name.local_name())
            },
            Self::Unexpected(name) => {
                write!(formatter, "<{}> is not permitted here", name.local_name())
            },
            Self::Foreign => formatter
                .write_str("an element outside this crate's vocabulary is not tolerated here"),
            Self::Invalid(error) => write!(formatter, "{error}"),
            Self::Output(full) => write!(formatter, "{full}"),
        }
    }
}

impl Error for DavError {}

/// What was wrong with the octets, at the XML layer.
///
/// Several of these are refusals of constructs XML 1.0 defines and this crate declines to
/// accept. The private lexer recognizes XML grammar; this wrapper still refuses constructs the
/// DAV vocabulary does not need, loudly rather than guessing at their semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SyntaxError {
    /// A `DOCTYPE` declaration appeared. This crate accepts none, ever.
    ///
    /// No CalDAV or WebDAV body needs one, and every entity-expansion attack — the billion
    /// laughs, an external parameter entity, a general entity pointing at a local file —
    /// needs one to declare the entity it expands. Refusing the declaration closes the class
    /// rather than bounding it.
    Doctype,
    /// An entity reference named something other than the five XML 1.0 predefines.
    ///
    /// With no `DOCTYPE` accepted, nothing can be declared, so `&anything;` beyond `&amp;`,
    /// `&lt;`, `&gt;`, `&quot;` and `&apos;` names an entity that does not exist. A reader
    /// that silently dropped it would deliver a value the peer did not write.
    UndefinedEntity,
    /// A processing instruction appeared outside the XML declaration.
    ProcessingInstruction,
    /// The document declared an encoding other than UTF-8, or is not UTF-8.
    Encoding,
    /// A namespace prefix was used with no declaration in scope.
    UnboundPrefix,
    /// An end tag named something other than the start tag it closed.
    MismatchedTag,
    /// The document ended inside a construct — a tag, a comment, a `CDATA` section.
    Truncated,
    /// The octets are not well-formed XML in a way none of the above names.
    Malformed,
    /// One element carried the same attribute name twice, which XML forbids.
    DuplicateAttribute,
    /// A character reference named a code point XML 1.0 section 2.2 excludes.
    ForbiddenCharacter,
}

impl Display for SyntaxError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let said = match *self {
            Self::Doctype => "a DOCTYPE declaration, which this reader accepts none of",
            Self::UndefinedEntity => "an entity reference naming no predefined entity",
            Self::ProcessingInstruction => "a processing instruction",
            Self::Encoding => "an encoding other than UTF-8",
            Self::UnboundPrefix => "a namespace prefix with no declaration in scope",
            Self::MismatchedTag => "an end tag naming another element",
            Self::Truncated => "an unterminated construct at the end of the input",
            Self::Malformed => "octets that are not well-formed XML",
            Self::DuplicateAttribute => "one attribute name twice on one element",
            Self::ForbiddenCharacter => "a character reference to a code point XML excludes",
        };
        write!(formatter, "the body carries {said}")
    }
}

impl Error for SyntaxError {}

impl From<crate::internal::dav::xml::fault::XmlSyntax> for SyntaxError {
    /// The one seam between the private XML layer's classification and this public one.
    ///
    /// Written out arm by arm rather than shared, because the two types are the same list for a
    /// reason that will not survive the extraction `docs/adr/0012` deferred: `webdav-core` would
    /// own the layer's copy and every consumer would map it, exactly as this crate does now.
    /// Nothing in `src/xml/` may name this type, so the mapping has to live on this side.
    fn from(error: crate::internal::dav::xml::fault::XmlSyntax) -> Self {
        match error {
            crate::internal::dav::xml::fault::XmlSyntax::UndefinedEntity => Self::UndefinedEntity,
            crate::internal::dav::xml::fault::XmlSyntax::Encoding => Self::Encoding,
            crate::internal::dav::xml::fault::XmlSyntax::Malformed => Self::Malformed,
            crate::internal::dav::xml::fault::XmlSyntax::ForbiddenCharacter => {
                Self::ForbiddenCharacter
            },
        }
    }
}

impl From<crate::internal::dav::xml::fault::XmlFault> for DavError {
    fn from(fault: crate::internal::dav::xml::fault::XmlFault) -> Self {
        match fault {
            crate::internal::dav::xml::fault::XmlFault::Limit(exceeded) => Self::Limit(exceeded),
            crate::internal::dav::xml::fault::XmlFault::Syntax(error) => Self::Syntax(error.into()),
        }
    }
}

/// A value was read and is not one the protocol defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ValueError {
    /// A `time-range` carried neither a `start` nor an `end`.
    ///
    /// RFC 4791 section 9.9 permits either bound to be absent and requires at least one, so
    /// a range with neither states no interval at all.
    TimeRangeUnbounded,
    /// A `time-range` ended at or before it started.
    TimeRangeInverted,
    /// A `DAV:status` did not carry the status line RFC 4918 section 14.28 requires.
    StatusLine,
    /// An `ETag` was not the quoted string RFC 9110 section 8.8.3 requires.
    EtagSyntax,
    /// A value that must be text was not valid UTF-8.
    NotUtf8,
    /// A `calendar-data` selection states "every property" and names properties beside it.
    ///
    /// RFC 4791 section 9.6.1 writes `comp ((allprop | prop*), (allcomp | comp*))`, so the two
    /// halves of each pair are alternatives. A value holding both is one no body can express,
    /// and reducing it to one of them silently would send a request the caller did not write.
    SelectionContradiction,
    /// A filter states a condition and its own negation.
    ///
    /// RFC 4791 section 9.7.1 makes `is-not-defined` exclusive with every other test in the
    /// same filter: a component that is not defined has no time range and no properties.
    FilterContradiction,
    /// An instant does not fit the `YYYYMMDDTHHMMSSZ` form RFC 4791 section 9.9 writes.
    TimeUnrepresentable,
    /// A `Depth` header carried a value other than `0`, `1` or `infinity`.
    DepthValue,
    /// A `DAV:sync-level` carried a value other than `1` or `infinite`.
    SyncLevel,
    /// An element did not carry an attribute its grammar requires.
    ///
    /// `comp-filter`, `prop-filter`, `param-filter`, `comp` and `prop` each require a `name`
    /// (RFC 4791 sections 9.6 and 9.7). A body missing one is well-formed XML that states no
    /// filter, so it is a value refusal and not a syntax one.
    AttributeMissing,
    /// An attribute carried a value outside the enumeration its specification declares.
    ///
    /// RFC 4791 section 9.7.5 writes `negate-condition` as `(yes | no)`, and section 9.6.4
    /// writes `novalue` the same way. Guessing at a third spelling would invert a filter.
    AttributeValue,
}

impl Display for ValueError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let said = match *self {
            Self::TimeRangeUnbounded => "a time-range with neither a start nor an end",
            Self::TimeRangeInverted => "a time-range ending at or before its start",
            Self::StatusLine => "a status element carrying no readable status line",
            Self::EtagSyntax => "an ETag that is not a quoted string",
            Self::NotUtf8 => "text that is not valid UTF-8",
            Self::SelectionContradiction => {
                "a calendar-data selection naming properties beside allprop"
            },
            Self::FilterContradiction => "a filter stating a condition and its own negation",
            Self::TimeUnrepresentable => "an instant no UTC date-time can write",
            Self::DepthValue => "a Depth other than 0, 1 or infinity",
            Self::SyncLevel => "a sync-level other than 1 or infinite",
            Self::AttributeMissing => "an element missing an attribute its grammar requires",
            Self::AttributeValue => "an attribute outside the enumeration its grammar declares",
        };
        write!(formatter, "the value is {said}")
    }
}

impl Error for ValueError {}

/// The caller's output buffer has no room for what an encoder was about to write.
///
/// A distinct type from [`DavError`] so that [`crate::internal::dav::ByteSink`] can be implemented without
/// naming the protocol's whole failure vocabulary — a sink knows about room and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SinkFull;

impl Display for SinkFull {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("the output sink has no room left")
    }
}

impl Error for SinkFull {}

#[cfg(test)]
mod tests {
    use alloc::format;

    use crate::internal::core::LimitExceeded;

    use super::{DavError, SinkFull, SyntaxError, ValueError};
    use crate::internal::dav::element::ElementName;

    #[test]
    fn a_limit_refusal_names_the_dimension_it_crossed() {
        let rendered = format!("{}", DavError::from(LimitExceeded::PrefixBindings));
        assert!(rendered.contains("namespace binding"), "{rendered}");
    }

    #[test]
    fn an_unsupported_element_names_itself() {
        let rendered = format!("{}", DavError::Unsupported(ElementName::SyncCollection));
        assert!(rendered.contains("sync-collection"), "{rendered}");
    }

    #[test]
    fn every_channel_converts_into_the_one_error() {
        assert_eq!(
            DavError::from(SyntaxError::Doctype),
            DavError::Syntax(SyntaxError::Doctype)
        );
        assert_eq!(
            DavError::from(ValueError::StatusLine),
            DavError::Invalid(ValueError::StatusLine)
        );
        assert_eq!(DavError::from(SinkFull), DavError::Output(SinkFull));
    }
}
