// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The syntax of a content line, kept so it can be written back exactly as it arrived.
//!
//! Unfolding runs to completion into a fresh owned buffer, and nothing downstream of it may
//! slice pre-unfold bytes. That is what makes a fold splitting a multi-byte codepoint
//! harmless. It also destroys the only record of where the producer folded, and real
//! producers fold at 73 octets, or at 76, or with a tab, or not at all — so a canonical
//! refold on write would rewrite every file in the corpus on its first save.
//!
//! [`LineLayout`] is the price of not doing that: the fold positions, the whitespace octet at
//! each of them, the terminator that was actually used, and whether a `:` was present at all,
//! recorded alongside the unfolded text rather than inside it.

use alloc::vec::Vec;

/// How a line was terminated.
///
/// A bare `LF` and a bare `CR` both violate RFC 5545 section 3.1, and both are in the corpus.
/// Recording which one arrived is what lets the violation be a diagnostic rather than a
/// rewrite: the file is written back as its producer wrote it, and the caller is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineEnding {
    /// `CRLF`, as RFC 5545 section 3.1 requires.
    CrLf,
    /// A bare `LF`, as most Unix-authored exports carry.
    Lf,
    /// A bare `CR`, which is rarer and equally not what the specification says.
    Cr,
}

impl LineEnding {
    /// The terminator this crate writes for a line it authored itself.
    pub const CANONICAL: Self = Self::CrLf;

    /// The octets this terminator is written as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::CrLf => b"\r\n",
            Self::Lf => b"\n",
            Self::Cr => b"\r",
        }
    }

    /// How many octets this terminator occupies when written.
    #[must_use]
    pub const fn written_len(self) -> usize {
        self.as_bytes().len()
    }

    /// Whether this terminator is the one RFC 5545 section 3.1 requires.
    #[must_use]
    pub const fn is_canonical(self) -> bool {
        matches!(self, Self::CrLf)
    }
}

/// One place a producer folded a content line, and how.
///
/// The fields are public because a fold point is three independent observations about the
/// input with no invariant relating them, and an accessor for each would be three functions
/// saying nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FoldPoint {
    /// Octet position in the *unfolded* content line, counted from the first octet of the
    /// property name.
    ///
    /// Counted from the name rather than from the value so that one number addresses the
    /// name, the parameters and the value uniformly; a producer that folded inside a
    /// parameter list is as ordinary as one that folded inside a `DESCRIPTION`.
    pub offset: u32,
    /// Whether the continuation line was indented with `HTAB` rather than `SP`.
    pub tab: bool,
    /// The terminator that preceded the continuation.
    pub newline: LineEnding,
}

impl FoldPoint {
    /// The whitespace octet that introduced the continuation line.
    #[must_use]
    pub const fn whitespace(self) -> u8 {
        if self.tab { b'\t' } else { b' ' }
    }
}

/// The syntax of the content line a property arrived on.
///
/// Held on the property rather than derived at write time, because "an unedited property
/// re-serializes byte-identically" and "unfolding runs to completion into a fresh buffer"
/// are only compatible if the layout survives the unfold.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineLayout {
    /// Where the producer folded, in order.
    folds: Vec<FoldPoint>,
    /// The terminator, or `None` for a final line that carried none.
    ending: Option<LineEnding>,
    /// Whether a `:` separated the header from the value.
    has_separator: bool,
    /// Whether the recorded folds have been discarded and the line is to be refolded.
    refold: bool,
}

impl LineLayout {
    /// The layout for a line this crate authored, to be folded canonically on write.
    #[must_use]
    pub fn canonical(ending: LineEnding) -> Self {
        Self {
            folds: Vec::new(),
            ending: Some(ending),
            has_separator: true,
            refold: true,
        }
    }

    /// The layout observed on a line that was read.
    ///
    /// A public constructor because the reader that observes these three things lives on the
    /// far side of a crate boundary from the tree that stores them. `has_separator` being
    /// `false` is not an error: a line with no `:` is stored as a property that serializes
    /// without one, which is how a colonless line survives a round trip.
    #[must_use]
    pub fn preserved(
        folds: Vec<FoldPoint>,
        ending: Option<LineEnding>,
        has_separator: bool,
    ) -> Self {
        Self {
            folds,
            ending,
            has_separator,
            refold: false,
        }
    }

    /// Discard the recorded folds and mark the line for canonical refolding.
    ///
    /// Called when a write replaces the text the folds were positions into. Keeping them
    /// would place a fold at an octet that no longer exists.
    ///
    /// A separator is asserted at the same time. A line whose text this crate wrote is a line
    /// this crate authored, and RFC 5545 section 3.1 gives an authored line a `:`; the
    /// colonless shape exists to preserve what a producer wrote, not to be written into. The
    /// terminator is left alone, so a final line that arrived without one still ends without
    /// one.
    pub fn mark_refolded(&mut self) {
        self.folds.clear();
        self.has_separator = true;
        self.refold = true;
    }

    /// Where the producer folded, in order. Empty once the line is marked for refolding.
    #[must_use]
    pub fn folds(&self) -> &[FoldPoint] {
        &self.folds
    }

    /// The terminator, or `None` for a final line that carried none.
    #[must_use]
    pub const fn ending(&self) -> Option<LineEnding> {
        self.ending
    }

    /// Whether a `:` separated the header from the value.
    #[must_use]
    pub const fn has_separator(&self) -> bool {
        self.has_separator
    }

    /// Whether this line is to be refolded canonically rather than as it arrived.
    #[must_use]
    pub const fn is_refolded(&self) -> bool {
        self.refold
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{FoldPoint, LineEnding, LineLayout};

    #[test]
    fn a_bare_terminator_is_recorded_rather_than_corrected() {
        assert_eq!(LineEnding::Lf.as_bytes(), b"\n");
        assert!(!LineEnding::Lf.is_canonical());
        assert!(LineEnding::CANONICAL.is_canonical());
    }

    #[test]
    fn a_fold_records_the_whitespace_octet_that_introduced_it() {
        let tabbed = FoldPoint {
            offset: 73,
            tab: true,
            newline: LineEnding::CrLf,
        };
        assert_eq!(tabbed.whitespace(), b'\t');
        assert_eq!(
            FoldPoint {
                tab: false,
                ..tabbed
            }
            .whitespace(),
            b' '
        );
    }

    #[test]
    fn a_preserved_layout_keeps_its_folds_until_a_write_discards_them() {
        let fold = FoldPoint {
            offset: 40,
            tab: false,
            newline: LineEnding::CrLf,
        };
        let mut layout = LineLayout::preserved(vec![fold], Some(LineEnding::CrLf), true);
        assert_eq!(layout.folds(), [fold]);
        assert!(!layout.is_refolded());

        layout.mark_refolded();
        assert!(
            layout.folds().is_empty(),
            "a fold into replaced text is a fold into nothing"
        );
        assert!(layout.is_refolded());
    }

    #[test]
    fn a_colonless_line_is_a_layout_and_not_an_error() {
        let layout = LineLayout::preserved(vec![], None, false);
        assert!(!layout.has_separator());
        assert_eq!(layout.ending(), None);
    }
}
