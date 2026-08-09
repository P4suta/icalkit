// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The vocabulary for describing a change without making one.
//!
//! It lives here rather than in `ical-itip` because the crate that applies a change is the
//! crate that owns the storage, and because `EncodeValue` needs the same words to state the
//! parameters a written value implies. A scheduling transition is a map from property
//! identity to one of these — a map so that two conflicting changes to one property cannot
//! both be constructed — and applying it is one call per entry.
//!
//! A value of these types is inert. It describes; only applying it acts.

use alloc::vec::Vec;

use crate::octets::RawText;

/// An assignment or an unassignment of one parameter.
///
/// Removal is a variant of the same type rather than a separate one, so that a list of edits
/// is one ordered sequence a caller can inspect and reorder, instead of two lists whose
/// relative order is undefined.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterEdit {
    /// The parameter name, as it will be written.
    name: RawText,
    /// The value to assign, or `None` to unassign the parameter.
    value: Option<RawText>,
}

impl ParameterEdit {
    /// Assign `value` to the parameter `name`.
    #[must_use]
    pub fn set(name: &[u8], value: &[u8]) -> Self {
        Self {
            name: RawText::from_bytes(name),
            value: Some(RawText::from_bytes(value)),
        }
    }

    /// Unassign the parameter `name`.
    #[must_use]
    pub fn remove(name: &[u8]) -> Self {
        Self {
            name: RawText::from_bytes(name),
            value: None,
        }
    }

    /// The parameter name.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        self.name.as_bytes()
    }

    /// The value to assign, `None` for an unassignment.
    #[must_use]
    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_ref().map(RawText::as_bytes)
    }

    /// Whether this edit unassigns the parameter.
    #[must_use]
    pub fn is_removal(&self) -> bool {
        self.value.is_none()
    }
}

/// A change to one property, described and not yet made.
///
/// `SetParameters` is the variant that earns its place. A `RANGE=THISANDFUTURE` edit changes
/// a parameter and not a value, and expressing it as a `Replace` would discard the value's
/// preserved text to say something that was never about the value. The recorded line layout
/// still goes, because the parameters are part of that line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProposedChange {
    /// Add a whole content line, name and parameters and value together.
    Add(RawText),
    /// Replace a whole content line, name and parameters and value together.
    Replace(RawText),
    /// Edit parameters only. The value's text is untouched.
    SetParameters(Vec<ParameterEdit>),
    /// Remove the property.
    Remove,
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{ParameterEdit, ProposedChange};
    use crate::octets::RawText;

    #[test]
    fn an_edit_distinguishes_assignment_from_unassignment() {
        let assign = ParameterEdit::set(b"RANGE", b"THISANDFUTURE");
        assert_eq!(assign.name(), b"RANGE");
        assert_eq!(assign.value(), Some(&b"THISANDFUTURE"[..]));
        assert!(!assign.is_removal());

        let unassign = ParameterEdit::remove(b"TZID");
        assert_eq!(unassign.value(), None);
        assert!(unassign.is_removal());
    }

    #[test]
    fn a_change_is_inert_until_something_applies_it() {
        let change = ProposedChange::SetParameters(vec![ParameterEdit::remove(b"TZID")]);
        assert_ne!(change, ProposedChange::Remove);
        assert_ne!(
            ProposedChange::Add(RawText::from_bytes(b"COMMENT:hi")),
            ProposedChange::Replace(RawText::from_bytes(b"COMMENT:hi")),
            "adding and replacing are different intentions over the same octets"
        );
    }
}
