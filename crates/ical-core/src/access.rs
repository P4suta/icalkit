// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The typed accessors, all of which are one accessor.
//!
//! Specification: RFC 5545 section 3.2, section 3.7 and section 3.8.
//!
//! The two levels are named on different axes, and that is what keeps a `geo()` from being
//! callable on a property that is not `GEO`. [`Property::value`] is a *value type* decoder:
//! section 3.3 asks what a value is, never what it is called. [`Component::get`] is a
//! *property name* accessor: sections 3.7 and 3.8 ask what a property is called and say
//! nothing about how its octets are shaped. There is exactly one accessor at each level and
//! every convenience here is one line of delegation, so the three states a caller matches on
//! cannot drift apart property by property as more of them are added (`docs/adr/0001`).
//!
//! Nothing is cached. A decoded value kept beside the octets is a second place for the answer
//! to live and therefore a second place for the two to disagree, and the octets are the half
//! that gets written back. `dtstart()` decodes again on every call, and a caller sorting a
//! thousand events by start time caches at its own call site.
//!
//! A name the specification declares at most once, arriving more than once, is
//! [`View::Malformed`] carrying [`DiagnosticCode::DuplicateProperty`] rather than a silently
//! chosen winner. "I cannot hand you one trusted value here" is what malformed already means,
//! so no fourth state is needed, and every occurrence stays reachable through
//! [`Component::properties_named`] — which is the only shape that can name more than one of
//! them.

use ical_grammar::{Diagnostic, DiagnosticCode, Location, Severity};

use crate::gregorian::DateTimeValue;
use crate::ident::PropertyId;
use crate::tree::{Component, Parameter, Property};
use crate::view::{DecodeValue, Geo, TextValue, View};

/// A diagnostic about a property whose octets are present and could not be read.
///
/// The location is [`Location::NOWHERE`], and that is a statement rather than an omission. A
/// [`Property`] owns fresh unfolded octets and not the offsets they were read from, so any
/// span produced here would address a buffer the caller never handed in. A plausible-looking
/// offset into the wrong buffer is worse than admitting there is none, and the caller holds
/// the property itself in both non-absent arms of a [`View`] anyway.
fn about_this_property(code: DiagnosticCode) -> Diagnostic {
    Diagnostic::new(code, Severity::Violation, Location::NOWHERE)
}

impl Property {
    /// This property's value, read as `T`.
    ///
    /// The only accessor at this level. `T` names a section 3.3 value type rather than a
    /// property, so there is nothing here to misapply: asking a `SUMMARY` for a
    /// [`DateTimeValue`] is a question about octets, and the answer is a diagnostic rather
    /// than a category error the caller has to have anticipated.
    ///
    /// The octets are untouched whichever arm comes back, and they are read again on the next
    /// call.
    #[must_use]
    pub fn value<'a, T: DecodeValue<'a>>(&'a self) -> View<'a, T> {
        match T::decode_value(self.value_text().as_bytes()) {
            Ok(value) => View::Valid {
                source: self,
                value,
            },
            Err(code) => View::Malformed {
                source: self,
                diagnostic: about_this_property(code),
            },
        }
    }
}

impl Parameter {
    /// This parameter's value with a section 3.2 `DQUOTE` pair removed.
    ///
    /// Removed on read and never in storage. RFC 5545 lets a producer quote a value that did
    /// not need quoting, so stripping the pair at parse time would write back a line the
    /// producer did not send; a caller matching `TZID="Europe/Paris"` against a zone name
    /// still needs it gone. Only a matched pair goes: a value that opens a quote and never
    /// closes it comes back whole, because deciding where the missing `DQUOTE` belonged would
    /// invent an octet, and a lone `"` is one octet rather than an empty quoted string.
    ///
    /// A parameter that arrived with no `=` at all has no value, and this reports the empty
    /// octets stored in place of one.
    #[must_use]
    pub fn unquoted(&self) -> &[u8] {
        let written = self.value().as_bytes();
        written
            .strip_prefix(b"\"")
            .and_then(|inside| inside.strip_suffix(b"\""))
            .unwrap_or(written)
    }
}

impl Component {
    /// The one property of this component named `id`, read as `T`.
    ///
    /// For the identities RFC 5545 gives a cardinality of at most one. Everything repeatable —
    /// which is every `X-` property and every property from an RFC published after this code —
    /// goes through [`Component::properties_named`], an iterator that cannot silently keep the
    /// first match.
    ///
    /// Cardinality is a claim about well-formed input and not about the documents this library
    /// is handed, so a name that arrives twice is [`View::Malformed`] with
    /// [`DiagnosticCode::DuplicateProperty`] even when both occurrences decode. The source is
    /// the first occurrence, so a caller has somewhere to start reading, and the occurrences
    /// themselves are reached through the general lookup.
    ///
    /// Nested components are not searched: a `DTSTART` inside a `VALARM` is the alarm's, and
    /// answering with it would answer a question nobody asked.
    #[must_use]
    pub fn get<'a, T: DecodeValue<'a>>(&'a self, id: &PropertyId) -> View<'a, T> {
        let mut occurrences = self.properties().filter(|property| property.has_id(id));
        let Some(first) = occurrences.next() else {
            return View::Absent;
        };
        if occurrences.next().is_some() {
            return View::Malformed {
                source: first,
                diagnostic: about_this_property(DiagnosticCode::DuplicateProperty),
            };
        }
        first.value()
    }

    /// The `DTSTART`, RFC 5545 section 3.8.2.4.
    #[must_use]
    pub fn dtstart(&self) -> View<'_, DateTimeValue> {
        self.get(&PropertyId::DTSTART)
    }

    /// The `DTEND`, RFC 5545 section 3.8.2.2.
    #[must_use]
    pub fn dtend(&self) -> View<'_, DateTimeValue> {
        self.get(&PropertyId::DTEND)
    }

    /// The `SUMMARY`, RFC 5545 section 3.8.1.12, with its escapes still in it.
    #[must_use]
    pub fn summary(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::SUMMARY)
    }

    /// The `UID`, RFC 5545 section 3.8.4.7.
    #[must_use]
    pub fn uid(&self) -> View<'_, TextValue<'_>> {
        self.get(&PropertyId::UID)
    }

    /// The `GEO` pair, RFC 5545 section 3.8.1.6, derived from text that stays authoritative.
    #[must_use]
    pub fn geo(&self) -> View<'_, Geo> {
        self.get(&PropertyId::GEO)
    }

    /// The `SEQUENCE`, RFC 5545 section 3.8.7.4.
    #[must_use]
    pub fn sequence(&self) -> View<'_, i32> {
        self.get(&PropertyId::SEQUENCE)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use ical_grammar::{Diagnostic, DiagnosticCode, Limits, LineEnding, LineLayout, Severity};

    use crate::ident::PropertyId;
    use crate::octets::RawText;
    use crate::tree::{Boundary, Component, Item, Parameter, Property};
    use crate::view::{DecodeValue, View};

    /// A decoder with no dependence on the value codecs, so that what these tests measure is
    /// this unit rather than that one: a non-empty run of ASCII digits, and nothing else.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Digits<'a>(&'a [u8]);

    impl<'a> DecodeValue<'a> for Digits<'a> {
        fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
            if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
                return Err(DiagnosticCode::MalformedInteger);
            }
            Ok(Self(bytes))
        }
    }

    /// How many times [`Counted`] has been decoded. One test reads it, and it reads a
    /// difference rather than an absolute, so a second reader would not have to be added here.
    static DECODES: AtomicUsize = AtomicUsize::new(0);

    /// A decoder that accepts anything and says how often it was asked.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Counted<'a>(&'a [u8]);

    impl<'a> DecodeValue<'a> for Counted<'a> {
        fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
            DECODES.fetch_add(1, Ordering::Relaxed);
            Ok(Self(bytes))
        }
    }

    /// A content line as a well-behaved producer wrote it.
    fn line(name: &[u8], value: &[u8]) -> Property {
        Property::new(
            RawText::from_bytes(name),
            Vec::new(),
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    /// The last line of a file that stopped without a terminator.
    fn unterminated(name: &[u8], value: &[u8]) -> Property {
        Property::new(
            RawText::from_bytes(name),
            Vec::new(),
            RawText::from_bytes(value),
            LineLayout::preserved(Vec::new(), None, true),
        )
    }

    /// A closed component of the given name.
    fn closed(name: &[u8], items: Vec<Item>) -> Component {
        let edge = |keyword: &[u8]| {
            Boundary::new(
                RawText::from_bytes(keyword),
                RawText::from_bytes(name),
                LineLayout::canonical(LineEnding::CANONICAL),
            )
        };
        Component::new(edge(b"BEGIN"), items, Some(edge(b"END")))
    }

    fn event(items: Vec<Item>) -> Component {
        closed(b"VEVENT", items)
    }

    /// A vendor property may repeat as often as its producer liked, and the general lookup is
    /// what every name goes through — so the singular accessors cannot be the only route to a
    /// value, and no occurrence is dropped on the way.
    #[test]
    fn every_occurrence_of_a_repeatable_name_is_reachable() {
        let vendor = PropertyId::from_name(b"X-MICROSOFT-CDO-BUSYSTATUS");
        let component = event(vec![
            Item::Property(line(b"X-MICROSOFT-CDO-BUSYSTATUS", b"BUSY")),
            Item::Property(line(b"SUMMARY", b"standup")),
            Item::Property(line(b"x-microsoft-cdo-busystatus", b"FREE")),
        ]);

        let found: Vec<&[u8]> = component
            .properties_named(&vendor)
            .map(|property| property.value_text().as_bytes())
            .collect();
        assert_eq!(found, vec![&b"BUSY"[..], &b"FREE"[..]]);
    }

    /// Both occurrences decode here, which is the point: the refusal is about there being two
    /// of them, not about either one being unreadable.
    #[test]
    fn a_name_the_specification_allows_once_arriving_twice_is_a_refusal_with_evidence() {
        let component = event(vec![
            Item::Property(line(b"SEQUENCE", b"1")),
            Item::Property(line(b"SEQUENCE", b"2")),
        ]);

        let view: View<'_, Digits<'_>> = component.get(&PropertyId::SEQUENCE);
        assert_eq!(
            view.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::DuplicateProperty)
        );
        assert_eq!(
            view.diagnostic().map(Diagnostic::severity),
            Some(Severity::Violation)
        );
        assert!(view.is_present(), "two of a thing is not none of it");
        assert_eq!(view.value(), None, "no winner is picked");
        assert_eq!(
            view.source()
                .map(|property| property.value_text().as_bytes()),
            Some(&b"1"[..]),
            "the first occurrence is where a caller starts reading"
        );
        assert_eq!(
            component.properties_named(&PropertyId::SEQUENCE).count(),
            2,
            "both stay reachable through the general lookup"
        );
    }

    #[test]
    fn a_name_that_is_not_there_is_absent_rather_than_malformed() {
        let component = event(vec![Item::Property(line(b"SUMMARY", b"standup"))]);
        let view: View<'_, Digits<'_>> = component.get(&PropertyId::SEQUENCE);
        assert!(!view.is_present());
        assert!(view.source().is_none());
        assert!(view.diagnostic().is_none());
    }

    /// The empty value: a diagnostic, and the octets are still there to be written back.
    #[test]
    fn a_value_with_no_octets_is_diagnosed_and_kept() {
        let property = line(b"SEQUENCE", b"");
        let view: View<'_, Digits<'_>> = property.value();
        assert_eq!(
            view.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::MalformedInteger)
        );
        assert!(view.is_present(), "an unreadable value is still a value");
        assert_eq!(
            view.source().map(|source| source.value_text().as_bytes()),
            Some(&b""[..])
        );
        assert_eq!(property.value_text().as_bytes(), b"");
    }

    /// A missing terminator is the serializer's problem and never the accessor's; reading a
    /// value must not depend on how the line it arrived on ended.
    #[test]
    fn a_line_that_ended_without_a_terminator_still_reads_its_value() {
        let property = unterminated(b"SEQUENCE", b"7");
        assert!(property.layout().ending().is_none());
        let view: View<'_, Digits<'_>> = property.value();
        assert_eq!(view.value(), Some(Digits(b"7")));
    }

    /// The largest value the default policy admits. Nothing here bounds a value on its own —
    /// the bound is the caller's, charged where the octets are read — so the accessor has to
    /// hand back the whole of one.
    #[test]
    fn the_longest_value_the_default_policy_admits_decodes_whole() {
        let size = usize::try_from(Limits::DEFAULT.max_value_bytes()).unwrap();
        let long = vec![b'7'; size];
        let property = line(b"SEQUENCE", &long);
        let view: View<'_, Digits<'_>> = property.value();
        assert_eq!(view.value().map(|digits| digits.0.len()), Some(size));
    }

    #[test]
    fn a_nested_components_property_is_not_this_components() {
        let alarm = closed(b"VALARM", vec![Item::Property(line(b"SEQUENCE", b"4"))]);
        let component = event(vec![Item::Component(alarm)]);
        let view: View<'_, Digits<'_>> = component.get(&PropertyId::SEQUENCE);
        assert!(!view.is_present());
    }

    #[test]
    fn a_name_is_matched_however_the_producer_spelled_it() {
        let component = event(vec![Item::Property(line(b"sequence", b"9"))]);
        let view: View<'_, Digits<'_>> = component.get(&PropertyId::SEQUENCE);
        assert_eq!(view.value(), Some(Digits(b"9")));
    }

    /// Two reads are two decodes. Were the answer kept anywhere, it would be a second place
    /// for it to disagree with the octets.
    #[test]
    fn nothing_decoded_is_kept_between_calls() {
        let property = line(b"X-VENDOR", b"anything at all");
        let before = DECODES.load(Ordering::Relaxed);
        let first: View<'_, Counted<'_>> = property.value();
        let second: View<'_, Counted<'_>> = property.value();
        assert_eq!(first, second, "the same octets give the same answer");
        assert_eq!(
            DECODES.load(Ordering::Relaxed).checked_sub(before),
            Some(2),
            "one decode per call, and no cache in between"
        );
    }

    #[test]
    fn a_quoted_parameter_reads_without_its_pair_and_is_stored_with_it() {
        let parameter = Parameter::new(
            RawText::from_bytes(b"TZID"),
            RawText::from_bytes(b"\"Europe/Paris\""),
        );
        assert_eq!(parameter.unquoted(), &b"Europe/Paris"[..]);
        assert_eq!(
            parameter.value().as_bytes(),
            &b"\"Europe/Paris\""[..],
            "storage keeps what the producer wrote"
        );
    }

    /// Only a matched pair is a pair. Everything else is octets a producer chose and this
    /// crate writes back.
    #[test]
    fn an_unmatched_quote_is_not_a_pair() {
        let cases: [(&[u8], &[u8]); 7] = [
            (b"", b""),
            (b"\"", b"\""),
            (b"\"\"", b""),
            (b"\"open", b"\"open"),
            (b"close\"", b"close\""),
            (b"plain", b"plain"),
            (b"has\"quote", b"has\"quote"),
        ];
        for (written, read_back) in cases {
            let parameter =
                Parameter::new(RawText::from_bytes(b"X-P"), RawText::from_bytes(written));
            assert_eq!(
                parameter.unquoted(),
                read_back,
                "{written:?} should read back as {read_back:?}"
            );
        }
    }

    #[test]
    fn a_parameter_that_arrived_without_a_value_reads_as_no_octets() {
        let parameter = Parameter::without_value(RawText::from_bytes(b"X-BROKEN"));
        assert!(!parameter.has_value());
        assert_eq!(parameter.unquoted(), &b""[..]);
    }

    /// Each convenience is a call into the general accessor, so each inherits absence and the
    /// duplicate rule from it rather than restating either.
    #[test]
    fn every_convenience_answers_the_way_the_general_accessor_does() {
        let empty = event(Vec::new());
        assert!(!empty.dtstart().is_present());
        assert!(!empty.dtend().is_present());
        assert!(!empty.summary().is_present());
        assert!(!empty.uid().is_present());
        assert!(!empty.geo().is_present());
        assert!(!empty.sequence().is_present());

        let twice = event(vec![
            Item::Property(line(b"SUMMARY", b"standup")),
            Item::Property(line(b"summary", b"stand down")),
        ]);
        assert_eq!(
            twice.summary().diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::DuplicateProperty)
        );
        assert_eq!(
            twice
                .summary()
                .source()
                .map(|property| property.value_text().as_bytes()),
            Some(&b"standup"[..])
        );
    }
}
