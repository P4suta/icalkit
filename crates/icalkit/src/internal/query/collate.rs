// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 1 — collation and substring matching. RFC 4791 sections 7.5 and 9.7.5.
//!
//! # What this unit owns
//!
//! One function, and everything under it: does this octet string contain that one, under the
//! collation the filter named. It is the primitive both filter units call and nothing else in
//! this crate implements, which is why it is a unit of its own rather than three copies inside
//! the two that use it.
//!
//! - Turn a `ical_dav::Collation` into a [`crate::internal::query::Collator`] with [`crate::internal::query::Collator::of`], and
//!   refuse the ones this crate does not implement as [`crate::internal::query::QueryError::UnsupportedCollation`].
//!   RFC 4791 section 7.5.1 gives a server the `CALDAV:supported-collation` precondition for
//!   exactly this, so a collation with no row here is refused and never silently downgraded.
//! - The reserved identifier `default` (RFC 4790 section 3.1) is one of the names that *does*
//!   have a row: RFC 4791 section 7.5 requires a server to use `i;ascii-casemap` both when the
//!   client states no collation and when it states `default`. `ical-dav` keeps that spelling as
//!   the peer wrote it, which is what a round trip needs and what a comparison cannot use, so
//!   the mapping is made here rather than in the reader.
//! - `i;ascii-casemap` (RFC 4790 section 9.2) folds `A`–`Z` against `a`–`z` and compares every
//!   other octet exactly. It is **not** Unicode case folding: `İ` does not fold to `i` here and
//!   must not, because a server that folded it would return different resources than the client
//!   asked for from every other server.
//! - `i;octet` (RFC 4790 section 9.3) compares octets.
//! - The substring search is over octets and must not assume the haystack is UTF-8. A value
//!   `ical-core` preserved because it did not decode is still a value a `text-match` is run
//!   against, and refusing to search it would silently exclude the resource.
//! - `negate-condition` is applied by the caller through [`crate::internal::query::Match::negate`], not here.
//!   This unit answers "contains", and a unit that also negated would apply it twice the day a
//!   caller composed two of them.
//!
//! # The contract two other units hold this one to
//!
//! `prop` and `walk` both call this with a value they did not decode and a needle that came off
//! the wire. The signature therefore takes octets on both sides and answers a plain `bool` —
//! never a [`crate::internal::query::Match`] — because there is nothing undecidable about a substring search and
//! a three-valued answer here would invite one.
//!
//! That is not a license to answer `false` when the question was not asked, though: it moves
//! the shape this crate is built around one door along. A collation this crate cannot make the
//! comparison under is [`crate::internal::query::QueryError::UnsupportedCollation`] and never "does not
//! contain", for the same reason an unresolvable zone is [`crate::internal::query::Match::Undecided`] and never
//! "does not match" — a `false` would exclude resources the client asked for and say nothing
//! about having done it.
//!
//! # What it must not do
//!
//! Allocate per comparison. A `calendar-query` runs a `text-match` against every property of
//! every component of every resource in a collection; a copy per comparison is the shape that
//! turns a bounded query into an unbounded one, and `docs/adr/0007` refuses it.
//!
//! # What it costs instead
//!
//! The naive scan: every window of the value, compared against the needle, which is
//! `value × needle` octet comparisons in the worst case and no allocation at all. A
//! table-driven search buys a better worst case with a table per needle, and a table is the
//! allocation the paragraph above refuses. Both operands are bounded before they arrive — the
//! needle by the charge `ical-dav` makes for reading a `text-match`, the value by the one
//! `ical-core` makes for reading the resource — so the product is bounded by the caller's own
//! policy rather than by anything this file would have to enforce again.

use ical_dav::{Collation, TextMatch};

use crate::internal::query::vocabulary::{Collator, QueryError};

/// What collation and substring matching is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const COLLATION_SECTIONS: &[&str] = &[
    "RFC 4791 section 7.5, collations",
    "RFC 4791 section 7.5.1, the CALDAV:supported-collation precondition",
    "RFC 4791 section 9.7.5, CALDAV:text-match",
    "RFC 4790 section 9.2, i;ascii-casemap",
    "RFC 4790 section 9.3, i;octet",
];

/// The reserved collation identifier of RFC 4790 section 3.1.
///
/// Compared octet for octet, and deliberately not case-insensitively. RFC 4790 fixes the
/// characters an identifier is spelled with and says nothing about comparing two spellings of
/// one, so `DEFAULT` is a name this crate has no row for. Refusing it is an answer RFC 4791
/// section 7.5.1 provides for and accepting it would be a guess, and the two are not
/// symmetric: a wrong refusal is a `REPORT` the client sees fail, and a wrong acceptance is a
/// `REPORT` that quietly returns the wrong resources.
const DEFAULT_IDENTIFIER: &[u8] = b"default";

/// The comparison a `collation` attribute asks for, or the refusal RFC 4791 section 7.5.1 gives.
///
/// Separate from [`contains_text`] because the question is worth asking without a value to
/// answer it against: a filter naming a collation no server implements is an unsupported
/// request whether or not any resource in the collection carries the property it tests, and a
/// server that only discovered it while walking data would refuse some queries and not others
/// depending on what happened to be stored.
pub(crate) fn collator_of(collation: &Collation) -> Result<Collator, QueryError> {
    // RFC 4791 section 7.5: "if the client specifies the 'default' collation identifier (as
    // defined in [RFC4790], Section 3.1), the server MUST default to using 'i;ascii-casemap'".
    // Read from the written form so that the reserved name is recognized wherever `ical-dav`
    // kept it, which for anything outside its own two rows is `Collation::Other`.
    if collation.as_bytes() == DEFAULT_IDENTIFIER {
        return Ok(Collator::AsciiCasemap);
    }
    Collator::of(collation).ok_or(QueryError::UnsupportedCollation)
}

/// Whether `haystack` contains `needle` under `collator`, RFC 4790 sections 9.2 and 9.3.
///
/// Octets on both sides. Neither operand is decoded, neither is assumed to be UTF-8, and a
/// needle that starts halfway through a multi-octet sequence is found where it occurs, because
/// RFC 4791 section 9.7.5 puts a `text-match` against a stored value rather than against a
/// reading of one.
#[must_use]
pub(crate) fn contains(haystack: &[u8], needle: &[u8], collator: Collator) -> bool {
    // RFC 4790 section 9.3: the substring operation "returns 'match' if the first string is the
    // empty string". It is also the width `<[u8]>::windows` refuses, so the specification's own
    // rule is the guard the scan below needs, and there is no second reading to keep in step.
    if needle.is_empty() {
        return true;
    }
    match collator {
        // RFC 4790 section 9.3: octets compared as octets.
        Collator::Octet => haystack
            .windows(needle.len())
            .any(|window| window == needle),
        // RFC 4790 section 9.2 changes octets 97-122 to 65-90 in both inputs and then compares
        // as `i;octet`. `eq_ignore_ascii_case` folds the other way, upper onto lower, which is
        // the same equivalence relation over the same twenty-six pairs; what matters is that
        // both leave every other octet alone, so 0x5b and 0x7b stay distinct where a fold
        // written as `octet | 0x20` would join them, and so section 9.2's "letters outside
        // ASCII are not treated case-insensitively" holds without a table of exceptions.
        Collator::AsciiCasemap => haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle)),
    }
}

/// Whether `value` contains the text `matcher` states, under the collation it names.
///
/// The door `prop` and `walk` call, and it answers the containment rather than the condition:
/// a `text-match` carrying `negate-condition="yes"` is satisfied by a value this answers
/// `false` for. Applying that negation is the caller's, through [`crate::internal::query::Match::negate`],
/// because a negation applied here *and* there is applied twice, and the two sites are far
/// enough apart that nothing at either one would say so.
pub(crate) fn contains_text(value: &[u8], matcher: &TextMatch) -> Result<bool, QueryError> {
    let collator = collator_of(&matcher.collation)?;
    Ok(contains(value, matcher.value(), collator))
}

#[cfg(test)]
mod tests {
    use ical_core::{Limits, Meter};
    use ical_dav::{Collation, TextMatch};

    use super::{collator_of, contains, contains_text};
    use crate::internal::query::vocabulary::{Collator, QueryError};

    /// Every collation this unit answers under, so a table states its rule once.
    const IMPLEMENTED: [Collator; 2] = [Collator::AsciiCasemap, Collator::Octet];

    /// A `text-match` looking for `needle` under `collation`, as a request would carry it.
    fn matcher(needle: &[u8], collation: Collation) -> TextMatch {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut built = TextMatch::new(needle, &mut meter).unwrap();
        built.collation = collation;
        built
    }

    /// RFC 4790 section 9.3: the substring operation "returns 'match' if the first string is
    /// the empty string". Section 9.2 defers its substring operation to that one, so the rule
    /// is the same under both — the filter that matches every value there is.
    #[test]
    fn an_empty_needle_matches_every_value() {
        const VALUES: &[&[u8]] = &[b"", b"SUMMARY", b"\xff\xfe", b"Lunch with Ann"];
        for &value in VALUES {
            for collator in IMPLEMENTED {
                assert!(
                    contains(value, b"", collator),
                    "{value:?} under {collator:?}"
                );
            }
        }
    }

    /// RFC 4790 section 9.3 asks for "a substring of the second string of length equal to the
    /// length of the first", and a value shorter than the needle has none — the filter that
    /// matches nothing, and the far side of the one bound this unit owns.
    #[test]
    fn a_needle_longer_than_the_value_never_matches() {
        const CASES: &[(&[u8], &[u8])] = &[
            (b"", b"A"),
            (b"SUMMARY", b"SUMMARYX"),
            (b"SUMMARY", b"XSUMMARY"),
            (b"Lunch", b"Lunches"),
        ];
        for &(value, needle) in CASES {
            for collator in IMPLEMENTED {
                assert!(
                    !contains(value, needle, collator),
                    "{needle:?} is longer than {value:?} under {collator:?}"
                );
            }
        }
    }

    /// The bound itself, from both ends. A needle as long as the value is one window and has
    /// to be tried; one octet longer is none. RFC 4790 section 9.3 states the equal-length
    /// condition and nothing about where in the value the window may sit, so the first and the
    /// last are both in.
    #[test]
    fn the_windows_reach_both_ends_and_stop_at_the_value_length() {
        const CASES: &[(&[u8], &[u8], bool)] = &[
            (b"Lunch with Ann", b"Lunch", true),
            (b"Lunch with Ann", b"Ann", true),
            (b"Lunch with Ann", b"Lunch with Ann", true),
            (b"Lunch with Ann", b"h with A", true),
            (b"Lunch with Ann", b"Lunch with Anna", false),
            (b"Lunch with Ann", b"nnA", false),
            (b"", b"", true),
        ];
        for &(value, needle, expected) in CASES {
            assert_eq!(
                contains(value, needle, Collator::Octet),
                expected,
                "{value:?} against {needle:?}"
            );
        }
    }

    /// RFC 4790 section 9.2 changes octets 97-122 to 65-90 and says in as many words that
    /// "letters outside ASCII are not treated case-insensitively". The rows below the letters —
    /// 0x40 against 0x60, 0x5b against 0x7b — are the ones a fold written as `octet | 0x20`
    /// joins, and the last two hold an accented letter against its own other case, which a
    /// Unicode fold joins; the very last one folds its ASCII half and still has to fail on the
    /// octets after it. Neither fold is this collation, and either mistake returns resources
    /// the client did not ask for.
    #[test]
    fn ascii_casemap_folds_the_twenty_six_letters_and_no_other_octet() {
        const CASES: &[(&[u8], &[u8], bool)] = &[
            (b"SUMMARY", b"summary", true),
            (b"summary", b"SUMMARY", true),
            (b"AZaz", b"azAZ", true),
            (b"@", b"`", false),
            (b"[", b"{", false),
            (b"]", b"}", false),
            (b"^", b"~", false),
            (b"\xc9", b"\xe9", false),
            (b"ANN\xc3\x89", b"ann\xc3\xa9", false),
        ];
        for &(value, needle, expected) in CASES {
            assert_eq!(
                contains(value, needle, Collator::AsciiCasemap),
                expected,
                "{value:?} against {needle:?} under i;ascii-casemap"
            );
        }
    }

    /// RFC 4790 section 9.3 compares octets, so every case difference section 9.2 erases is a
    /// difference again. The two collations disagreeing on one value is why RFC 4791 section
    /// 7.5.1 has a precondition instead of a fallback.
    #[test]
    fn octet_collation_keeps_the_case_difference_casemap_erases() {
        const CASES: &[(&[u8], &[u8])] = &[
            (b"SUMMARY", b"summary"),
            (b"Lunch with Ann", b"lunch"),
            (b"AZaz", b"azAZ"),
        ];
        for &(value, needle) in CASES {
            assert!(
                contains(value, needle, Collator::AsciiCasemap),
                "{value:?} against {needle:?} under i;ascii-casemap"
            );
            assert!(
                !contains(value, needle, Collator::Octet),
                "{value:?} against {needle:?} under i;octet"
            );
        }
    }

    /// A value `ical-core` preserved because it did not decode is still a value RFC 4791
    /// section 9.7.5 runs a `text-match` against, and neither that section nor RFC 4790 puts a
    /// character-set condition on either side. The first row starts the needle halfway through
    /// a UTF-8 sequence, which a character-wise search would never find; the second and third
    /// hold octets no character set this crate knows would decode.
    #[test]
    fn the_search_is_over_octets_rather_than_characters() {
        const CASES: &[(&[u8], &[u8], bool)] = &[
            (b"Ann\xc3\xa9 bar", b"\xa9 b", true),
            (b"\xff\xfe\x00SUMMARY", b"\xfe\x00S", true),
            (b"\xff\xfe", b"\xfe\xff", false),
            (b"a\x00b", b"\x00b", true),
        ];
        for &(value, needle, expected) in CASES {
            for collator in IMPLEMENTED {
                assert_eq!(
                    contains(value, needle, collator),
                    expected,
                    "{value:?} against {needle:?} under {collator:?}"
                );
            }
        }
    }

    /// RFC 4791 section 7.5.1: "Any XML attribute specifying a collation MUST specify a
    /// collation supported by the server", and the `CALDAV:supported-collation` precondition is
    /// what a server answers when it does not. This is this unit's shape of the rule the crate
    /// is built on — the comparison it cannot make is refused, never answered `false`. The last
    /// two rows are spellings a lenient reading would have accepted: RFC 4790 fixes no
    /// case-insensitive comparison of identifiers, and `i;octet;extra` is a different
    /// identifier from `i;octet` under section 3.1's own syntax.
    #[test]
    fn a_collation_this_crate_does_not_implement_is_refused_rather_than_answered() {
        const NAMES: &[&[u8]] = &[
            b"i;unicode-casemap",
            b"i;basic",
            b"i;ascii-numeric",
            b"",
            b"I;OCTET",
            b"i;octet;extra",
        ];
        for &name in NAMES {
            let collation = Collation::parse(name).unwrap();
            assert_eq!(
                collator_of(&collation),
                Err(QueryError::UnsupportedCollation),
                "{name:?}"
            );
            // And through the door the filter units call, with a value that would have
            // contained the needle under either collation this crate does implement.
            assert_eq!(
                contains_text(b"Lunch with Ann", &matcher(b"Lunch", collation)),
                Err(QueryError::UnsupportedCollation),
                "{name:?}"
            );
        }
    }

    /// RFC 4791 section 7.5: "In the absence of a collation explicitly specified by the client,
    /// or if the client specifies the 'default' collation identifier (as defined in [RFC4790],
    /// Section 3.1), the server MUST default to using 'i;ascii-casemap' as the collation." Both
    /// halves of that sentence, because the reserved name reaches this crate as an unrecognized
    /// one and refusing it would fail a query RFC 4791 requires a server to answer.
    #[test]
    fn the_reserved_default_identifier_is_the_ascii_casemap_collation() {
        let reserved = Collation::parse(b"default").unwrap();
        assert_eq!(collator_of(&reserved), Ok(Collator::AsciiCasemap));
        assert_eq!(
            contains_text(b"Lunch with Ann", &matcher(b"LUNCH", reserved)),
            Ok(true)
        );

        let unstated = matcher(b"LUNCH", Collation::default());
        assert_eq!(collator_of(&unstated.collation), Ok(Collator::AsciiCasemap));
        assert_eq!(contains_text(b"Lunch with Ann", &unstated), Ok(true));
    }

    /// The two collations RFC 4791 section 7.5 requires every server to support, mapped onto
    /// the two comparisons this crate makes, spelled the same on both sides. A server answers
    /// `CALDAV:supported-collation-set` from one of these names and reads the `collation`
    /// attribute into the other, and a difference between them would advertise a collation the
    /// evaluator then refused.
    #[test]
    fn each_implemented_collation_maps_to_its_own_comparison() {
        assert_eq!(
            collator_of(&Collation::AsciiCasemap),
            Ok(Collator::AsciiCasemap)
        );
        assert_eq!(collator_of(&Collation::Octet), Ok(Collator::Octet));
        assert_eq!(
            Collator::AsciiCasemap.as_bytes(),
            Collation::AsciiCasemap.as_bytes()
        );
        assert_eq!(Collator::Octet.as_bytes(), Collation::Octet.as_bytes());
    }

    /// RFC 4791 section 9.7.5 gives `text-match` a `negate-condition` attribute, and this unit
    /// answers the containment underneath it: the same value and the same needle produce the
    /// same answer whether or not the filter negates. The filter's own answer is
    /// `Match::of(..)` and then `Match::negate()`, made once, by the caller.
    #[test]
    fn negate_condition_does_not_reach_this_unit() {
        let plain = matcher(b"lunch", Collation::AsciiCasemap);
        let mut inverted = matcher(b"lunch", Collation::AsciiCasemap);
        inverted.negate = true;
        for value in [b"Lunch with Ann".as_slice(), b"Dinner with Ann".as_slice()] {
            assert_eq!(
                contains_text(value, &plain),
                contains_text(value, &inverted)
            );
        }
        assert_eq!(contains_text(b"Lunch with Ann", &inverted), Ok(true));
        assert_eq!(contains_text(b"Dinner with Ann", &inverted), Ok(false));
    }

    /// The door reads both halves of the `text-match`: the value it looks for and the collation
    /// it looks under. One value, one needle, two collations, two answers — which is the whole
    /// reason RFC 4791 section 7.5.1 refuses to let a server pick the collation itself.
    #[test]
    fn the_door_reads_both_halves_of_the_text_match() {
        let folded = matcher(b"SUMMARY", Collation::AsciiCasemap);
        let exact = matcher(b"SUMMARY", Collation::Octet);
        assert_eq!(contains_text(b"the summary line", &folded), Ok(true));
        assert_eq!(contains_text(b"the summary line", &exact), Ok(false));
        assert_eq!(contains_text(b"the SUMMARY line", &exact), Ok(true));
    }
}
