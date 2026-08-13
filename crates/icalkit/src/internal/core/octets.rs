// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Owned octet storage, and the one place octets are asked to be text.
//!
//! Storage is bytes rather than `str` because unfolding is a pure octet operation, a fold may
//! legally split a multi-byte codepoint, and a CP1252 `SUMMARY` exported by a real client is
//! in the corpus. Nothing between the fold and a typed accessor is allowed to demand
//! validity; if it did, the choice would be between rejecting such a file, substituting
//! `U+FFFD`, or losing the round trip, and all three are refused (`docs/adr/0001`).
//!
//! The cost is that every typed text read is fallible, and that cost is paid deliberately.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt::{self, Display, Formatter};
use core::str::{self, Utf8Error};

use crate::internal::core::DiagnosticCode;

/// Octets exactly as they were read, or exactly as a caller wrote them.
///
/// Owned rather than borrowed from the caller's buffer: a document that borrowed its input
/// would make every later mutation a negotiation with the borrow checker over memory the
/// caller owns, and would put a lifetime parameter in the central type of five crates
/// forever to save one copy per value (`docs/adr/0007`).
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawText(Box<[u8]>);

impl RawText {
    /// Copy `bytes` into fresh owned storage.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Box::from(bytes))
    }

    /// Take ownership of an already-accumulated buffer, without copying it again.
    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    /// The octets, as they will be written back.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// How many octets are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is stored. A property with an empty name is how a blank line survives.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Give up ownership of the octets so they can be edited and stored again.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0.into_vec()
    }

    /// The octets as text. The only place in this crate where validity is demanded.
    ///
    /// The error keeps the offset rather than collapsing to a bare `None`, so a caller can
    /// say *where* the octets went wrong and quote the part that was fine.
    pub fn as_str(&self) -> Result<&str, TextError> {
        str::from_utf8(&self.0).map_err(TextError::from)
    }

    /// Whether these octets are `other`, compared as RFC 5545 section 3.1 compares a name.
    ///
    /// Case-insensitively, and only over ASCII: an iCalendar name is ASCII by grammar, and
    /// case-folding beyond it would need a Unicode table this crate declines to carry and
    /// would fold names two different producers meant to be distinct.
    #[must_use]
    pub fn eq_name(&self, other: &[u8]) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl From<&[u8]> for RawText {
    fn from(bytes: &[u8]) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<Vec<u8>> for RawText {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

/// Octets that were asked to be text and were not.
///
/// This is a diagnostic's worth of information rather than a parse failure: the octets are
/// still stored, still written back, and still readable as octets. Only the caller who wanted
/// a `&str` is told no.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextError {
    /// The failure as `core` reported it, kept whole so detail can be added later without a
    /// breaking change.
    inner: Utf8Error,
}

impl TextError {
    /// The diagnostic code an emission site reports for this failure.
    ///
    /// Named here so that the code and the error type cannot drift apart: every site that
    /// turns a [`TextError`] into a [`Diagnostic`](crate::internal::core::Diagnostic) reads it from
    /// the error rather than choosing one.
    pub const CODE: DiagnosticCode = DiagnosticCode::InvalidUtf8Text;

    /// How many octets from the start were valid text.
    #[must_use]
    pub const fn valid_up_to(self) -> usize {
        self.inner.valid_up_to()
    }

    /// How long the invalid sequence was, or `None` when the input simply ended mid-sequence.
    #[must_use]
    pub fn error_len(self) -> Option<usize> {
        self.inner.error_len()
    }
}

impl From<Utf8Error> for TextError {
    fn from(inner: Utf8Error) -> Self {
        Self { inner }
    }
}

impl Display for TextError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the octets are not valid UTF-8 from offset {}",
            self.inner.valid_up_to()
        )
    }
}

impl Error for TextError {}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use crate::internal::core::DiagnosticCode;

    use super::{RawText, TextError};

    #[test]
    fn storage_survives_octets_that_are_not_text() {
        // A CP1252 `é` in a SUMMARY, which a real client exported and this crate must keep.
        let stored = RawText::from_bytes(b"\xe9t\xe9");
        assert!(stored.as_str().is_err());
        assert_eq!(stored.as_bytes(), b"\xe9t\xe9");
        assert_eq!(stored.len(), 3);
    }

    #[test]
    fn a_decode_failure_says_where_rather_than_only_that() {
        let error: TextError = RawText::from_bytes(b"ok\xff").as_str().unwrap_err();
        assert_eq!(error.valid_up_to(), 2);
        assert_eq!(TextError::CODE, DiagnosticCode::InvalidUtf8Text);
    }

    #[test]
    fn names_compare_case_insensitively_over_ascii_only() {
        let name = RawText::from_bytes(b"dtstart");
        assert!(name.eq_name(b"DTSTART"));
        assert!(!name.eq_name(b"DTEND"));
    }

    #[test]
    fn an_accumulated_buffer_is_taken_rather_than_copied_again() {
        let stored = RawText::from_vec(vec![b'a', b'b']);
        assert_eq!(stored.clone().into_vec(), vec![b'a', b'b']);
        assert!(!stored.is_empty());
        assert!(RawText::default().is_empty());
    }
}
