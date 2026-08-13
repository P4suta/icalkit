// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The token layer, which is the parser.
//!
//! The document tree is one consumer of this path rather than a parallel implementation.
//! Keeping a lexer private and exposing it "later, if anyone asks" is how a codebase
//! acquires two parsers with one name: the tree builder takes raw octets because that was
//! convenient, the public lexer grows its own grammar, and the divergence surfaces as a
//! corpus case that one path accepts and the other does not (`docs/adr/0008`).
//!
//! Every payload is `&[u8]`. A str-shaped token would force this layer either to reject a
//! CP1252 `SUMMARY` — throwing away a file over a violation that should be a diagnostic — or
//! to substitute `U+FFFD`, which destroys the byte-identical round trip that is the whole
//! acceptance criterion. Neither is permitted, so UTF-8 is demanded only in the typed view.
//!
//! `BEGIN` and `END` are ordinary names here. The component model belongs to the crate that
//! has one.

use super::{FoldPoint, LineEnding, ParseError};

/// One lexical piece of a content line.
///
/// A value arrives in chunks, and that is the point: chunks are the runs between folds, they
/// borrow the input and are never buffered by the reader, so a 400 MB inline
/// `ATTACH;ENCODING=BASE64` never has to be contiguous and resident for a token to exist.
/// Names and parameters *are* reassembled, through a scratch buffer bounded by
/// [`GrammarLimits::max_header_bytes`](crate::GrammarLimits::max_header_bytes), because they
/// have a bound and values do not.
///
/// `#[non_exhaustive]` so that adding a *variant* is a minor release for a caller outside this
/// workspace rather than a break. It says nothing about the fields: the variants are not
/// individually non-exhaustive, external code writes `Token::Value { bytes, more }` with a
/// complete field list — a `ContentLineSource` implementor has to — and adding a field to one is
/// a major release. Growth in that direction is the likelier one, and it is a decision rather
/// than an oversight: destructuring a token is what consuming it *is*, and hiding the fields
/// behind accessors would buy compatibility with a shape nobody wants to write.
///
/// Inside the crate that defines the type the attribute buys nothing at all: an in-crate match
/// must be exhaustive whatever it says, so adding a variant is a compile error at every consumer
/// here, which is the answer wanted at both distances. A wildcard arm over this type is therefore
/// a variant silently ignored. `unreachable_patterns = "deny"` was recorded as what stops one
/// being written and does not: that lint fires on a catch-all after every variant is already
/// covered, and the shape that loses data is a match that omits a variant and adds `_`, which is
/// reachable and which the lint is silent about. The fourth rule of `xtask purity` is what
/// refuses it (`docs/adr/0004`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Token<'a> {
    /// The property name, reassembled across any folds inside it.
    Name(&'a [u8]),
    /// One parameter, reassembled across any folds inside it.
    Parameter {
        /// The parameter name, as written.
        name: &'a [u8],
        /// The parameter value with any surrounding `DQUOTE` still present.
        value: &'a [u8],
        /// Whether an `=` and a value were present at all.
        has_value: bool,
    },
    /// One chunk of the property value.
    Value {
        /// The octets of this chunk, borrowed from the input.
        bytes: &'a [u8],
        /// Whether another chunk of the same value follows.
        more: bool,
    },
    /// The end of one content line, carrying the syntax that has to be written back.
    EndOfLine {
        /// Where the producer folded, in order, as offsets into the unfolded line.
        folds: &'a [FoldPoint],
        /// The terminator, or `None` for a final line that carried none.
        ending: Option<LineEnding>,
        /// Whether a `:` separated the header from the value.
        has_separator: bool,
    },
}

/// A source of content-line tokens.
///
/// Object-safe on purpose: `&mut dyn ContentLineSource` is a legal argument, so a consumer
/// that only counts `VEVENT`s does not have to be generic, and a sibling crate that wants a
/// pull parser without a type parameter can have one. The blanket implementation over
/// `&mut T` is what lets a generic consumer accept a `dyn` source without a second signature.
pub trait ContentLineSource {
    /// The next token, `None` at the end of the input.
    ///
    /// An error here is a caller-stated bound that was crossed, never a judgment about the
    /// calendar: a malformed line is lexed into tokens and diagnosed above, because a reader
    /// that stops at the first violation is a reader that discards the rest of the file.
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>>;
}

impl<T: ContentLineSource + ?Sized> ContentLineSource for &mut T {
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>> {
        (**self).next_token()
    }
}
