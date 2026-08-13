// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The identifier, classified and never translated.
//!
//! Specification: RFC 5545 section 3.2.19, the `TZID` parameter, and section 3.8.3.1, the
//! `TZID` property <https://www.rfc-editor.org/rfc/rfc5545#section-3.2.19>.
//!
//! A `TZID` is not an IANA identifier. The grammar is `paramtext` with an optional leading
//! solidus, and what real files carry under it includes `W. Europe Standard Time` from
//! Exchange, `/mozilla.org/20050126_1/Europe/Berlin` from Lightning, `Customized Time Zone`
//! from Outlook, and names with spaces, dots and no separator at all. A crate that assumed it
//! could parse one would be wrong on a large fraction of the calendars in the world, and a
//! crate that failed when it could not would refuse to read them.
//!
//! So this module classifies and stops. [`Tzid::form`] says which of three shapes an
//! identifier has and asserts nothing about what zone it names; [`Tzid::strip_global_prefix`]
//! removes the leading solidus, which is the only rewriting section 3.2.19 licenses. Nothing
//! here looks for `Europe/Berlin` inside `/mozilla.org/20050126_1/Europe/Berlin`: that is a
//! vendor convention, and guessing at it is how a confidently wrong zone gets applied.
//!
//! Comparison and lookup are by exact bytes as written, including case. `docs/adr/0003`
//! assigns the mapping of a vendor identifier onto an IANA one to the caller, where it is
//! visible and where its failure is visible too, and refusing to case-fold here is part of
//! keeping that promise: `EUROPE/BERLIN` is not a zone this workspace claims to know.

/// What shape an identifier has, which is not the same question as what zone it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TzidForm {
    /// A globally unique identifier: RFC 5545 section 3.2.19's leading solidus.
    GloballyUnique,
    /// The shape a tz database name has: separated by solidi, with no space and no dot.
    ///
    /// *Shape*, not membership. `Mars/Olympus_Mons` classifies here and names nothing.
    IanaLike,
    /// Anything else, which includes every Windows zone name.
    Opaque,
}

/// One `TZID` as it was written.
///
/// Borrowed and `Copy`: the octets belong to the property the value was read from or to the
/// caller writing one, and a type that owned them would allocate at every lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tzid<'a> {
    /// The identifier's text, exactly as the parameter carried it, `DQUOTE`s removed.
    text: &'a str,
}

impl<'a> Tzid<'a> {
    /// The identifier `text` spells.
    #[must_use]
    pub const fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// The identifier's text, exactly as written.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.text
    }

    /// Which of the three shapes this identifier has.
    ///
    /// A classification over the bytes and nothing more. The tz database shape is recognized
    /// by the character set its names are drawn from — letters, digits, `+`, `-`, `_` and the
    /// solidus that separates the parts — which is what separates `America/New_York` from
    /// `W. Europe Standard Time` without either a table or a claim that the first one exists.
    #[must_use]
    pub fn form(self) -> TzidForm {
        if self.text.starts_with('/') {
            return TzidForm::GloballyUnique;
        }
        let separated = self.text.contains('/');
        let plain = !self.text.is_empty()
            && !self.text.ends_with('/')
            && self.text.chars().all(is_database_name_character);
        if separated && plain {
            TzidForm::IanaLike
        } else {
            TzidForm::Opaque
        }
    }

    /// The identifier with its leading solidus removed, absent when it has none.
    ///
    /// The whole of the rewriting RFC 5545 section 3.2.19 permits. What is left is still an
    /// opaque vendor string — `mozilla.org/20050126_1/Europe/Berlin` — and this crate does not
    /// go looking inside it.
    #[must_use]
    pub fn strip_global_prefix(self) -> Option<Self> {
        self.text.strip_prefix('/').map(Self::new)
    }
}

/// Whether `character` may appear in a tz database name.
///
/// A free function rather than a closure so the set has a name to be wrong under. The `+` is
/// there for `Etc/GMT+5`, and the underscore for `America/New_York`.
fn is_database_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '+')
}

#[cfg(test)]
mod tests {
    use super::{Tzid, TzidForm};

    /// The shapes the corpus actually carries, classified and not translated.
    #[test]
    fn the_identifiers_real_files_carry_are_each_classified_and_none_are_rewritten() {
        let cases = [
            ("America/New_York", TzidForm::IanaLike),
            ("Etc/GMT+5", TzidForm::IanaLike),
            ("America/Argentina/Buenos_Aires", TzidForm::IanaLike),
            (
                "/mozilla.org/20050126_1/Europe/Berlin",
                TzidForm::GloballyUnique,
            ),
            (
                "/citadel.org/20190914_1/Europe/Berlin",
                TzidForm::GloballyUnique,
            ),
            ("W. Europe Standard Time", TzidForm::Opaque),
            ("Customized Time Zone", TzidForm::Opaque),
            ("UTC", TzidForm::Opaque),
            ("GMT", TzidForm::Opaque),
            ("", TzidForm::Opaque),
            ("Europe/", TzidForm::Opaque),
            ("Europe/Berlin ", TzidForm::Opaque),
        ];
        for (text, expected) in cases {
            assert_eq!(Tzid::new(text).form(), expected, "{text}");
            assert_eq!(Tzid::new(text).as_str(), text, "{text}");
        }
    }

    /// The one rewrite the specification licenses, and the one it does not.
    #[test]
    fn the_leading_solidus_comes_off_and_nothing_else_does() {
        let prefixed = Tzid::new("/mozilla.org/20050126_1/Europe/Berlin");
        let stripped = prefixed.strip_global_prefix().unwrap();
        assert_eq!(stripped.as_str(), "mozilla.org/20050126_1/Europe/Berlin");
        assert_eq!(
            stripped.form(),
            TzidForm::Opaque,
            "what is left is a vendor string and not a database name"
        );
        assert_eq!(
            stripped.strip_global_prefix(),
            None,
            "one solidus, because that is what the grammar has"
        );
        assert_eq!(Tzid::new("America/New_York").strip_global_prefix(), None);
    }

    /// Lookup is by exact bytes, which is what makes the alias question the caller's.
    #[test]
    fn identifiers_compare_by_exact_bytes_including_case() {
        assert_ne!(Tzid::new("Europe/Berlin"), Tzid::new("EUROPE/BERLIN"));
        assert_eq!(Tzid::new("Europe/Berlin"), Tzid::new("Europe/Berlin"));
    }
}
