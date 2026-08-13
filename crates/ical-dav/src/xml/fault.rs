// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What this layer refuses, in a vocabulary that names no protocol.
//!
//! `ical-dav`'s own `SyntaxError` is a public type and cannot live here, because nothing in this
//! layer is exported. So the layer classifies for itself and the crate above maps: one total
//! `From` impl in `failure.rs`, one arm per variant. That duplication is the price the boundary
//! costs and it is the shape the extraction will take anyway — a `webdav-core` would own this
//! classification and its consumer would map it, exactly as `ical-dav` does now.
//!
//! Several of these refuse constructs XML 1.0 defines and this workspace declines to implement.
//! That is the posture `SECURITY.md` asks for: a hand-rolled reader that is merely *incomplete*
//! is safer than one that is accidentally *complete*, so a construct no `DAV:` body needs is
//! refused under its own name rather than guessed at, dropped, or passed through.

use ical_core::LimitExceeded;

/// What was wrong with the octets, at the XML layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum XmlSyntax {
    /// An entity reference named something other than the five XML 1.0 predefines.
    ///
    /// With no `DOCTYPE` accepted anywhere in this workspace, nothing can ever have been
    /// declared, so `&anything;` beyond the five names an entity that does not exist.
    UndefinedEntity,
    /// The document declared an encoding other than UTF-8, or is not UTF-8.
    Encoding,
    /// The octets are not well-formed XML in a way neither of the above names.
    Malformed,
    /// A character reference named a code point XML 1.0 section 2.2 excludes.
    ForbiddenCharacter,
}

// Four classes and not the ten `ical-dav`'s own `SyntaxError` carries, because these are the
// four this layer raises. `Doctype`, `ProcessingInstruction`, `UnboundPrefix`, `MismatchedTag`,
// `Truncated` and `DuplicateAttribute` are the tokenizer's state machine's, and that half has
// not moved down here yet. A layer that declared a variant it never constructs would be
// describing somebody else's refusals.

/// A refusal that ends a read or a write inside this layer.
///
/// Two channels and no more: the octets are wrong, or a bound the caller stated was crossed.
/// A full output sink is not here, because nothing in this layer writes to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum XmlFault {
    /// A caller-stated bound was crossed, and the dimension says which one.
    Limit(LimitExceeded),
    /// The body is not the XML this layer reads.
    Syntax(XmlSyntax),
}

impl From<LimitExceeded> for XmlFault {
    fn from(exceeded: LimitExceeded) -> Self {
        Self::Limit(exceeded)
    }
}

impl From<XmlSyntax> for XmlFault {
    fn from(error: XmlSyntax) -> Self {
        Self::Syntax(error)
    }
}
