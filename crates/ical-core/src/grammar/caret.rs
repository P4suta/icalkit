// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 6868 caret encoding for parameter values.
//!
//! Specification: <https://www.rfc-editor.org/rfc/rfc6868>.
//!
//! RFC 5545 section 3.2 has no spelling for a `DQUOTE` inside a parameter value and no
//! spelling for a newline, which is why
//! [`parameter_is_representable`](crate::parameter_is_representable) answers `false` for both.
//! RFC 6868 supplies the missing one: three pairs, written with a caret, understood by every
//! producer that implements it and by no other.
//!
//! The posture is [`escape`](crate::unescape_text)'s exactly. Storage keeps the octets a
//! producer wrote; resolving them is a view a caller asks for, so a `CN` written `^'quoted^'`
//! is written back as `^'quoted^'` whether or not anything ever reads it as `"quoted"`.
//! Encoding is the other direction and a narrower one — it is for parameter values this crate
//! authors, where there is no producer whose spelling to preserve.
//!
//! Both directions or neither: a value written `^'` and read back as two octets is a round
//! trip the crate fails against itself.
//!
//! Parameter values only, which is a scope and not an oversight. RFC 6868 section 2 defines the
//! encoding for the parameter value of a content line and for nothing else, so a `^` inside a
//! `SUMMARY` is a caret; applying this to a property value would invent an encoding no producer
//! agreed to and change text that already had a meaning.
//!
//! Allocation is the caller's, in the two forms `escape` uses: a form returning [`Cow`] that
//! borrows when there is nothing to do, and an `_into` form that appends to a buffer the caller
//! has already charged against its meter (`docs/adr/0007`). Nothing here reports. A `^` the
//! table gives no meaning is a question [`undefined_caret_escapes`] answers, and the offset and
//! the severity that turn that answer into a diagnostic are the half only the caller holding
//! the input can supply (`docs/adr/0009`).

use alloc::borrow::Cow;
use alloc::vec::Vec;

/// The RFC 6868 section 2 substitutions, as `(octet after the caret, octet it stands for)`.
///
/// A table rather than a match arm per case, for the reason
/// [`TEXT_ESCAPES`](crate::TEXT_ESCAPES) is one: the same rows drive both directions, and two
/// hand-written directions are two places for them to disagree.
pub const CARET_ESCAPES: [(u8, u8); 3] = [(b'n', b'\n'), (b'^', b'^'), (b'\'', b'"')];

/// The octet `spelling` stands for when it follows a caret, or `None` if it stands for nothing.
///
/// `None` is RFC 6868 section 2's "leave it as it is" case, and it is a case rather than a
/// failure: the two octets are what a producer wrote, and a caller that wants to say so reports
/// [`DiagnosticCode::UndefinedCaretEscape`](crate::DiagnosticCode::UndefinedCaretEscape).
#[must_use]
pub fn caret_escape_meaning(spelling: u8) -> Option<u8> {
    CARET_ESCAPES
        .into_iter()
        .find(|(written, _)| *written == spelling)
        .map(|(_, stands_for)| stands_for)
}

/// How this crate spells `octet` when it authors a parameter value, or `None` for an octet it
/// writes as itself.
///
/// Ungated, where [`text_escape_spelling`](crate::text_escape_spelling) consults a separate
/// policy table before it answers. The rows above are a bijection, so no octet has two
/// spellings for a writer to choose between and none reads back without being safe to write;
/// one table is therefore the whole policy. That is also what makes the round trip here total
/// where the `TEXT` one is not, and it is why `^` is encoded alongside the two octets the base
/// grammar lacks: leave a literal `^` alone and the `^n` a caller handed us comes back a
/// newline.
#[must_use]
pub fn caret_escape_spelling(octet: u8) -> Option<u8> {
    CARET_ESCAPES
        .into_iter()
        .find(|(_, stands_for)| *stands_for == octet)
        .map(|(written, _)| written)
}

/// The substitution at the front of `rest`, and what follows it.
///
/// Split out so that the three walks below share one reading of what a pair is. A caret that
/// begins nothing the table defines is not an encoding at all, and neither is one with nothing
/// after it.
fn take_caret(rest: &[u8]) -> Option<(u8, &[u8])> {
    let (&head, tail) = rest.split_first()?;
    if head != b'^' {
        return None;
    }
    let (&spelling, after) = tail.split_first()?;
    Some((caret_escape_meaning(spelling)?, after))
}

/// Whether resolving the carets in `bytes` would change anything.
///
/// False for a caret that begins no defined pair, which is why a value carrying only such a
/// caret is borrowed rather than copied. Two octets are enough to decide it: the leftmost
/// defined pair cannot be preceded by a caret, since that caret would itself begin the defined
/// pair `^^` and be leftmost instead — so no defined pair found here is one the left-to-right
/// walk swallows as the tail of an earlier one.
#[must_use]
pub fn caret_needs_decoding(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .any(|pair| matches!(pair, [b'^', spelling] if caret_escape_meaning(*spelling).is_some()))
}

/// Whether any `^` in `bytes` begins a pair RFC 6868 gives no meaning.
///
/// The answer a caller turns into
/// [`DiagnosticCode::UndefinedCaretEscape`](crate::DiagnosticCode::UndefinedCaretEscape),
/// adding the offset and the severity. Walked left to right rather than tested pairwise,
/// because the two readings disagree: in `^^x` the `^^` is an encoded caret and the `x` is a
/// plain octet, so there is no undefined pair, while a pairwise test would find `^x` sitting
/// across the boundary and report a producer for something it did not write.
///
/// A trailing `^` is not reported. The code above is frozen at "a `^` was followed by an octet
/// RFC 6868 gives no meaning" (`docs/adr/0009`), and a caret with nothing after it was followed
/// by no octet; reporting it here would make the code mean something its own definition does
/// not say, which is the one edit that vocabulary does not allow. Either way the octets stay.
#[must_use]
pub fn undefined_caret_escapes(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some((&head, tail)) = rest.split_first() {
        if let Some((_, after)) = take_caret(rest) {
            rest = after;
        } else if head == b'^' && !tail.is_empty() {
            return true;
        } else {
            rest = tail;
        }
    }
    false
}

/// Resolve the carets in a parameter value, borrowing when there are none.
///
/// A caret that begins no defined pair — including one at the very end of the value, with
/// nothing left for it to encode — is passed through as itself, which is what RFC 6868
/// section 2 requires of a receiver. That is not leniency dressed up as a rule: the stored
/// octets are what get written back either way, so a repair invented here would reach a
/// caller's display and nothing else, while making the text it showed disagree with the file.
///
/// ```
/// use ical_core::decode_caret;
///
/// assert_eq!(decode_caret(b"^'quoted^'").as_ref(), b"\"quoted\"");
/// assert_eq!(decode_caret(b"^x undefined").as_ref(), b"^x undefined");
/// ```
#[must_use]
pub fn decode_caret(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !caret_needs_decoding(bytes) {
        return Cow::Borrowed(bytes);
    }
    // Resolving only ever shortens, so this capacity is never grown past.
    let mut resolved = Vec::with_capacity(bytes.len());
    decode_caret_into(bytes, &mut resolved);
    Cow::Owned(resolved)
}

/// Resolve the carets in a parameter value, appending to `out`.
///
/// The form for a caller that owns a buffer and has already charged the octets it is about to
/// append; [`decode_caret`] is the form for one that has not.
pub fn decode_caret_into(bytes: &[u8], out: &mut Vec<u8>) {
    let mut rest = bytes;
    while let Some((&head, tail)) = rest.split_first() {
        if let Some((stands_for, after)) = take_caret(rest) {
            out.push(stands_for);
            rest = after;
        } else {
            out.push(head);
            rest = tail;
        }
    }
}

/// Whether writing `bytes` as a parameter value would encode anything.
#[must_use]
pub fn caret_needs_encoding(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|octet| caret_escape_spelling(*octet).is_some())
}

/// Encode a parameter value for writing, borrowing when nothing needs it.
///
/// [`decode_caret`] undoes this for every octet string there is, which is the claim the crate
/// owes itself: every caret this writes is the head of a defined pair, so the left-to-right
/// walk that reads them back pairs them the same way they were written.
///
/// The other composition does not hold and is not meant to. A value that arrived as `^x` is
/// two octets RFC 6868 leaves alone, and passing it through here would spell the caret `^^` —
/// a different value, correctly encoded. Nothing on the round-trip path hands preserved octets
/// to this function; it is for values this crate authors, where no producer's spelling is being
/// overwritten. It is also for a parameter value only, never a property value: RFC 6868 gives
/// no meaning to a caret outside one.
///
/// ```
/// use ical_core::encode_caret;
///
/// assert_eq!(encode_caret(b"say \"hi\"").as_ref(), b"say ^'hi^'");
/// assert_eq!(encode_caret(b"Europe/Paris").as_ref(), b"Europe/Paris");
/// ```
#[must_use]
pub fn encode_caret(bytes: &[u8]) -> Cow<'_, [u8]> {
    // Counted rather than estimated: each encoded octet costs exactly one more, so one extra
    // pass buys an exact capacity and no reallocation on the way out.
    let encoded_count = bytes
        .iter()
        .filter(|octet| caret_escape_spelling(**octet).is_some())
        .count();
    if encoded_count == 0 {
        return Cow::Borrowed(bytes);
    }
    // Saturating because a length plus a count of a subset of the same slice cannot overflow
    // a `usize` for any slice that exists; the saturation is unreachable, not a truncation.
    let mut written = Vec::with_capacity(bytes.len().saturating_add(encoded_count));
    encode_caret_into(bytes, &mut written);
    Cow::Owned(written)
}

/// Encode a parameter value for writing, appending to `out`.
///
/// The form for a caller that owns a buffer and has already charged the octets it is about to
/// append; [`encode_caret`] is the form for one that has not.
pub fn encode_caret_into(bytes: &[u8], out: &mut Vec<u8>) {
    for &octet in bytes {
        match caret_escape_spelling(octet) {
            Some(spelling) => {
                out.push(b'^');
                out.push(spelling);
            },
            None => out.push(octet),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        CARET_ESCAPES, caret_escape_meaning, caret_escape_spelling, caret_needs_decoding,
        caret_needs_encoding, decode_caret, decode_caret_into, encode_caret, encode_caret_into,
        undefined_caret_escapes,
    };

    /// The parameter values inside `ical-conform`'s `rfc6868_carets.ics`, beside what RFC 6868
    /// section 2 says each one means. Copied rather than read, because this crate cannot see
    /// that fixture and the point of naming them here is that both files agree.
    const FIXTURE_VALUES: [(&[u8], &[u8]); 5] = [
        (b"Ann ^n Marie", b"Ann \n Marie"),
        (b"^'quoted^'", b"\"quoted\""),
        (b"100^^", b"100^"),
        (b"^x undefined", b"^x undefined"),
        (b"^n^^^'^x", b"\n^\"^x"),
    ];

    /// The table is a bijection, which is what lets one set of rows drive both directions.
    #[test]
    fn every_spelling_stands_for_exactly_one_octet_and_back() {
        for (written, stands_for) in CARET_ESCAPES {
            assert_eq!(caret_escape_meaning(written), Some(stands_for));
            assert_eq!(caret_escape_spelling(stands_for), Some(written));
        }
        assert_eq!(caret_escape_meaning(b'x'), None);
        assert_eq!(caret_escape_spelling(b'x'), None);
    }

    /// The two octets section 3.2 cannot write at all are exactly the two this encoding adds.
    #[test]
    fn the_encoding_supplies_the_spellings_the_base_grammar_lacks() {
        assert_eq!(caret_escape_spelling(b'"'), Some(b'\''));
        assert_eq!(caret_escape_spelling(b'\n'), Some(b'n'));
    }

    /// The empty value is legal everywhere and is the shape a bare `X-FLAG=` produces.
    #[test]
    fn an_empty_value_costs_nothing_in_any_direction() {
        assert!(matches!(decode_caret(b""), Cow::Borrowed(_)));
        assert!(matches!(encode_caret(b""), Cow::Borrowed(_)));
        assert!(decode_caret(b"").is_empty());
        assert!(encode_caret(b"").is_empty());
        assert!(!caret_needs_decoding(b""));
        assert!(!caret_needs_encoding(b""));
        assert!(!undefined_caret_escapes(b""));

        let mut out: Vec<u8> = Vec::new();
        decode_caret_into(b"", &mut out);
        encode_caret_into(b"", &mut out);
        assert!(out.is_empty());
    }

    /// Every row resolves, and an octet the table does not name keeps its caret.
    #[test]
    fn every_defined_pair_resolves_and_an_undefined_one_is_left_as_it_is() {
        let cases: [(&[u8], &[u8]); 8] = [
            (b"^n", b"\n"),
            (b"^^", b"^"),
            (b"^'", b"\""),
            (b"a^nb^'c", b"a\nb\"c"),
            (b"^x", b"^x"),
            (b"^N", b"^N"),
            (b"no carets here", b"no carets here"),
            (b"^x^n", b"^x\n"),
        ];
        for (written, resolved) in cases {
            assert_eq!(decode_caret(written).as_ref(), resolved, "{written:?}");
        }

        // `^N` is not a second spelling of `^n`. RFC 6868 section 2 lists three pairs and this
        // is not one of them, so the two octets stay and a caller may say so.
        assert!(undefined_caret_escapes(b"^N"));
    }

    /// A value that stops mid-pair is the "nothing left to encode" boundary for this unit.
    #[test]
    fn a_caret_with_nothing_after_it_is_kept_as_itself_and_is_not_reported() {
        assert!(!caret_needs_decoding(b"ends with ^"));
        assert!(matches!(decode_caret(b"ends with ^"), Cow::Borrowed(_)));
        assert_eq!(decode_caret(b"ends with ^").as_ref(), b"ends with ^");
        assert_eq!(decode_caret(b"^").as_ref(), b"^");
        assert!(
            !undefined_caret_escapes(b"^"),
            "the frozen code is about a caret followed by an octet, and this one is not"
        );
    }

    /// The case this unit answers with a value a caller turns into a diagnostic. Nothing is
    /// refused and no octet is lost; the caller adds the offset and the severity.
    #[test]
    fn an_undefined_pair_is_reported_as_a_question_and_keeps_its_octets() {
        assert!(undefined_caret_escapes(b"^x undefined"));
        assert!(undefined_caret_escapes(b"CN=^ "));
        assert!(undefined_caret_escapes(b"^n then ^q"));

        // Reported and still unrewritten: with no defined pair present there is nothing to
        // resolve, so the value is borrowed rather than copied and repaired.
        assert!(matches!(decode_caret(b"^x undefined"), Cow::Borrowed(_)));
        assert_eq!(decode_caret(b"^x undefined").as_ref(), b"^x undefined");

        assert!(!undefined_caret_escapes(b"^n^^^'"));
        assert!(!undefined_caret_escapes(b"nothing at all"));
    }

    /// Pairing runs left to right, so an encoded caret cannot capture the octet after it.
    /// A pairwise reading gets both of these wrong, in opposite directions.
    #[test]
    fn an_encoded_caret_does_not_capture_what_follows_it() {
        assert_eq!(decode_caret(b"^^n").as_ref(), b"^n");
        assert_eq!(decode_caret(b"^^'").as_ref(), b"^'");
        assert_eq!(decode_caret(b"^^^^").as_ref(), b"^^");
        assert!(
            !undefined_caret_escapes(b"^^x"),
            "the `^^` is an encoded caret and the `x` is a plain octet"
        );
    }

    /// The obligation, over every octet string short enough to enumerate: what this crate
    /// wrote, it reads back unchanged. Two octets is enough to reach every pairing there is,
    /// since no substitution is longer than that.
    #[test]
    fn decoding_what_this_crate_encoded_returns_the_octets_it_started_with() {
        for lead in 0..=u8::MAX {
            let single = [lead];
            assert_eq!(
                decode_caret(encode_caret(&single).as_ref()).as_ref(),
                single.as_slice(),
                "{single:?}"
            );
            for follow in 0..=u8::MAX {
                let pair = [lead, follow];
                assert_eq!(
                    decode_caret(encode_caret(&pair).as_ref()).as_ref(),
                    pair.as_slice(),
                    "{pair:?}"
                );
            }
        }

        // Longer strings, including octets no UTF-8 decoder would accept: neither direction
        // inspects a lead byte.
        let cases: [&[u8]; 5] = [
            b"say \"hi\" ^ and\n more",
            b"^^^^^",
            b"\"\"\"",
            b"note \xe9\xe8\xfc end",
            b"Europe/Paris",
        ];
        for original in cases {
            let written = encode_caret(original);
            assert_eq!(
                decode_caret(written.as_ref()).as_ref(),
                original,
                "{original:?}"
            );
        }
    }

    /// The other composition is not a round trip and is not used as one: an undefined pair is
    /// preserved text, and encoding preserved text spells its caret and changes the value.
    #[test]
    fn encoding_what_was_decoded_is_not_the_identity_and_is_kept_off_the_round_trip_path() {
        let resolved = decode_caret(b"^x");
        assert_eq!(resolved.as_ref(), b"^x");
        assert_eq!(encode_caret(resolved.as_ref()).as_ref(), b"^^x");
    }

    /// Encoding is total over the three octets, and free for a value carrying none of them.
    #[test]
    fn encoding_covers_the_octets_the_base_grammar_cannot_write() {
        let cases: [(&[u8], &[u8]); 4] = [
            (b"\n", b"^n"),
            (b"\"", b"^'"),
            (b"^", b"^^"),
            (b"line\nsays \"x\" ^", b"line^nsays ^'x^' ^^"),
        ];
        for (original, written) in cases {
            assert!(caret_needs_encoding(original), "{original:?}");
            assert_eq!(encode_caret(original).as_ref(), written, "{original:?}");
        }

        assert!(!caret_needs_encoding(b"Europe/Paris"));
        assert!(matches!(encode_caret(b"Europe/Paris"), Cow::Borrowed(_)));
    }

    /// The values the committed fixture carries, decoded. The fixture is written back octet
    /// for octet because nothing here touches storage — these are the meanings a caller sees
    /// on top of octets that never moved.
    #[test]
    fn the_fixture_values_decode_to_what_the_specification_says_they_mean() {
        for (stored, meaning) in FIXTURE_VALUES {
            assert_eq!(decode_caret(stored).as_ref(), meaning, "{stored:?}");
        }
    }

    /// The buffer forms append rather than replace, which is what makes them usable from a
    /// caller assembling one line out of several pieces.
    #[test]
    fn the_buffer_forms_append_to_what_is_already_there() {
        let mut out: Vec<u8> = vec![b'='];
        encode_caret_into(b"a\"b", &mut out);
        encode_caret_into(b"plain", &mut out);
        assert_eq!(out.as_slice(), b"=a^'bplain");

        let mut resolved: Vec<u8> = vec![b'='];
        decode_caret_into(b"a^'b", &mut resolved);
        assert_eq!(resolved.as_slice(), b"=a\"b");
    }

    /// A parameter value has no length limit of its own, so the long case is a value made of
    /// nothing but pairs: the capacity arithmetic and the single pass both have to hold.
    #[test]
    fn a_value_made_entirely_of_pairs_resolves_in_one_pass() {
        const COUNT: usize = 4096;

        let mut written: Vec<u8> = Vec::with_capacity(COUNT.saturating_mul(2));
        for _ in 0..COUNT {
            written.extend_from_slice(b"^'");
        }

        let resolved = decode_caret(&written);
        assert_eq!(resolved.len(), COUNT);
        assert!(resolved.iter().all(|octet| *octet == b'"'));
        assert!(!undefined_caret_escapes(&written));
        assert_eq!(encode_caret(resolved.as_ref()).as_ref(), written.as_slice());
    }
}
