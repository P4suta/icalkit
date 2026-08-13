// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5545 section 3.3 value types, attacked where two octet strings mean one value.
//!
//! Every place a value has two spellings is a place a codec may canonize one into the other,
//! and section 3.3 is full of them: `007` and `7`, `true` and `TRUE`, `P1W` and `P7D`,
//! `+010000` and `+0100`, `\N` and `\n`, `1.500` and `1.5`, a period written as two bounds and
//! one written as a bound and a length, a base 64 quantum whose padding bits nobody defined.
//! Storage here is octets, so none of that can reach `Document::serialize` — and the cases
//! below say so rather than assume it, because "the parser keeps bytes" is a claim about the
//! parser and the codecs are a second surface with a second answer.
//!
//! The codecs come out of this well. Read against readers written here from the ABNF, section
//! 3.3.1's base 64, section 3.3.6's duration and section 3.3.14's offset all agree octet for
//! octet, RFC 4648 section 10's vectors decode as RFC 4648 says, and every value that decodes
//! and is written straight back through `PropertyMut::set` reads back as the same value while
//! every other line in the file stays where it was.
//!
//! Three things do not hold, and all three are on the *write* side, where this crate authors
//! octets that had no producer. A decoder that is "exact or it refuses" has a case where it is
//! neither; an encoder that "exists only where the value determines its own text" has a case
//! where the text it determines is one its own decoder rejects; and the zone the read side is
//! careful to say is not a zone is one the write side will happily write. Each is a failing
//! case below, addressed to the section it is about.

use ical_core::{
    BinaryValue, Component, DateTimeValue, DecodeValue, Diagnostic, DiagnosticCode, Document,
    Duration, EncodeValue, Geo, IgnoreDiagnostics, Item, Limits, Meter, MutationError, ParseError,
    Period, Property, PropertyId, TextValue, UriValue, UtcOffset, View,
};

/// The policy every case runs under.
///
/// Stated rather than left implicit: an outcome that depends on a budget is not reproducible
/// without the budget, and none of the fixtures here goes near one (`docs/adr/0010`).
const POLICY: Limits = Limits::DEFAULT;

/// One `VEVENT` carrying a second spelling of nearly every section 3.3 value type.
const AMBIGUOUS: &[u8] = include_bytes!("fixtures/break_values/ambiguous_spellings.ics");

/// Date-times and periods beside a `TZID`, including the one that names no zone.
const ZONED_BOUNDS: &[u8] = include_bytes!("fixtures/break_values/zoned_bounds.ics");

/// A section 3.3.7 `FLOAT` whose decimal expansion is past the largest `f64`, folded.
const FLOAT_EXTREMES: &[u8] = include_bytes!("fixtures/break_values/float_extremes.ics");

/// The same value spellings under a bare `LF`, which is what a mail gateway hands over.
const LF_ONLY: &[u8] = include_bytes!("fixtures/break_values/lf_only_values.ics");

/// Every fixture in this directory, named beside its octets.
///
/// Read at compile time rather than through a path, because `.gitattributes` marks these files
/// `-text`: a test that opened them could still be handed a working tree some tool normalized.
const FIXTURES: &[(&str, &[u8])] = &[
    ("ambiguous_spellings", AMBIGUOUS),
    ("zoned_bounds", ZONED_BOUNDS),
    ("float_extremes", FLOAT_EXTREMES),
    ("lf_only_values", LF_ONLY),
];

/// Parse `input` under [`POLICY`] and write it back, keeping what was diagnosed.
fn read(input: &[u8]) -> (Result<Vec<u8>, ParseError>, Vec<Diagnostic>) {
    let mut kept = Vec::new();
    let written = Document::parse(input, POLICY, &mut kept).map(|tree| tree.to_bytes());
    (written, kept)
}

/// Parse `input` under [`POLICY`], or hand back an empty document.
///
/// Total rather than fallible, so a fixture that crossed a bound surfaces as an assertion about
/// a document with nothing in it — which names the case — rather than as a panic in a helper.
fn tree_of(input: &[u8]) -> Document {
    let mut kept: Vec<Diagnostic> = Vec::new();
    Document::parse(input, POLICY, &mut kept).unwrap_or_default()
}

/// Every property of `document`, at any depth, in the order they were written.
fn properties(document: &Document) -> Vec<&Property> {
    let mut found = Vec::new();
    gather_properties(document.items(), &mut found);
    found
}

/// Append the properties under `items`, depth first.
fn gather_properties<'a>(items: &'a [Item], found: &mut Vec<&'a Property>) {
    for entry in items {
        match entry {
            Item::Property(property) => found.push(property),
            Item::Component(nested) => gather_properties(nested.items(), found),
        }
    }
}

/// Every component of `document`, at any depth.
fn components(document: &Document) -> Vec<&Component> {
    let mut found = Vec::new();
    gather_components(document.items(), &mut found);
    found
}

/// Append the components under `items`, depth first.
fn gather_components<'a>(items: &'a [Item], found: &mut Vec<&'a Component>) {
    for entry in items {
        if let Item::Component(nested) = entry {
            found.push(nested);
            gather_components(nested.items(), found);
        }
    }
}

/// The property whose value text is `text`, which is how a case names one line of a fixture.
fn line_of<'a>(document: &'a Document, text: &[u8]) -> Option<&'a Property> {
    properties(document)
        .into_iter()
        .find(|property| property.value_text().as_bytes() == text)
}

/// The value type `T` read from the first property named `id`, wherever in the tree it sits.
fn value_of<'a, T: DecodeValue<'a>>(document: &'a Document, id: &PropertyId) -> View<'a, T> {
    for component in components(document) {
        let found = component.get(id);
        if found.is_present() {
            return found;
        }
    }
    View::Absent
}

/// Write `value` through the one guard this crate offers, wherever the property sits.
fn write_value<T: EncodeValue>(
    document: &mut Document,
    id: &PropertyId,
    value: &T,
) -> Result<(), MutationError> {
    for calendar in document.components_mut() {
        for inner in calendar.components_mut() {
            if inner.properties().any(|property| property.has_id(id)) {
                let mut guard = inner.get_mut::<T>(id).ok_or(MutationError::Absent)?;
                return guard.set(value);
            }
        }
    }
    Err(MutationError::Absent)
}

/// Ask every typed accessor this crate has for its answer, and keep none of them.
///
/// The point is the octets afterwards, not the answers: reading is supposed to cost nothing.
fn read_every_accessor(document: &Document) {
    for property in properties(document) {
        let _ = property.value::<DateTimeValue<'_>>();
        let _ = property.value::<Duration>();
        let _ = property.value::<Period<'_>>();
        let _ = property.value::<UtcOffset>();
        let _ = property.value::<Geo>();
        let _ = property.value::<f64>();
        let _ = property.value::<i32>();
        let _ = property.value::<bool>();
        let _ = property.value::<UriValue<'_>>();
        let _ = property.declared_value_type();
        if let View::Valid { value, .. } = property.value::<BinaryValue<'_>>() {
            let _ = value.decode();
        }
        if let View::Valid { value, .. } = property.value::<TextValue<'_>>() {
            let _ = value.decode();
        }
    }
    for component in components(document) {
        read_every_component_accessor(component);
    }
}

/// The section 3.7 and section 3.8 half of [`read_every_accessor`].
fn read_every_component_accessor(component: &Component) {
    let _ = component.dtstart();
    let _ = component.dtend();
    let _ = component.dtstamp();
    let _ = component.due();
    let _ = component.duration();
    let _ = component.trigger();
    let _ = component.organizer();
    let _ = component.status();
    let _ = component.transp();
    let _ = component.priority();
    let _ = component.class();
    let _ = component.location();
    let _ = component.description();
    let _ = component.tzid();
    let _ = component.tzoffsetfrom();
    let _ = component.tzoffsetto();
    let _ = component.rrule();
    let _ = component.summary();
    let _ = component.uid();
    let _ = component.geo();
    let _ = component.sequence();
    let _ = component.kind();
    let _ = component.attendees().count();
    let _ = component.freebusy().count();
    let mut meter = Meter::new(POLICY);
    component.audit(&mut meter, &mut IgnoreDiagnostics);
}

// ---------------------------------------------------------------------------------------
// What holds
// ---------------------------------------------------------------------------------------

#[test]
fn p1_every_fixture_is_written_back_octet_for_octet() {
    for (name, octets) in FIXTURES {
        let (written, _) = read(octets);
        assert_eq!(written.as_deref(), Ok(*octets), "{name}");
    }
}

#[test]
fn p2_what_a_parse_wrote_is_a_fixed_point_of_parsing_it_again() {
    for (name, octets) in FIXTURES {
        let once = read(octets).0.expect("a fixture within the bounds");
        let twice = read(&once).0.expect("what this crate wrote is readable");
        assert_eq!(twice, once, "{name}");
    }
}

/// Reading is a view and never a stage: after every accessor has run, the octets are the same
/// octets, whether the value could be read or not.
///
/// Both halves matter. A malformed `GEO` and a `DTSTART` under a `TZID` this crate cannot
/// resolve are exactly the values a decoder would be tempted to repair, and the repair would
/// reach the file through the one door `docs/adr/0001` closes.
#[test]
fn p4_reading_every_typed_accessor_costs_the_file_no_octet() {
    for (name, octets) in FIXTURES {
        let document = tree_of(octets);
        read_every_accessor(&document);
        assert_eq!(document.to_bytes(), *octets, "{name}");
    }
}

/// RFC 5545 section 3.3: two spellings of one value read as one value, and each is written back
/// as the spelling its producer chose.
///
/// This is the claim the whole file exists to test, stated over the octets a real export
/// carries. Neither spelling is canonized; the fixture holds both and comes back holding both.
#[test]
fn rfc5545_3_3_two_spellings_of_one_value_read_alike_and_are_each_written_back() {
    let document = tree_of(AMBIGUOUS);

    let sequence: View<'_, i32> = value_of(&document, &PropertyId::SEQUENCE);
    assert_eq!(sequence.value(), Some(7), "`007` is seven");
    let priority: View<'_, i32> = value_of(&document, &PropertyId::PRIORITY);
    assert_eq!(priority.value(), Some(5), "`+5` is five");

    let short = line_of(&document, b"PT1H").expect("the fixture carries it");
    let terms = line_of(&document, b"PT1H0M0S").expect("the fixture carries it");
    assert_eq!(
        short.value::<Duration>().value(),
        terms.value::<Duration>().value(),
        "one hour, written twice"
    );

    let lower = line_of(&document, b"true").expect("the fixture carries it");
    let mixed = line_of(&document, b"True").expect("the fixture carries it");
    assert_eq!(lower.value::<bool>().value(), Some(true));
    assert_eq!(mixed.value::<bool>().value(), Some(true));

    let plain = line_of(&document, br"line one\nline two").expect("the fixture carries it");
    let capital = line_of(&document, br"line one\Nline two").expect("the fixture carries it");
    let resolve = |property: &Property| {
        property
            .value::<TextValue<'_>>()
            .value()
            .and_then(|held| held.decode().ok().map(std::borrow::Cow::into_owned))
    };
    assert_eq!(resolve(plain), resolve(capital), "`\\N` is `\\n`");
    assert_eq!(resolve(plain).as_deref(), Some("line one\nline two"));

    assert_eq!(read(AMBIGUOUS).0.as_deref(), Ok(AMBIGUOUS), "and unchanged");
}

/// One case of "read a value, write that same value, and see what moved".
///
/// The property is looked up by name, decoded as `$ty`, written straight back through
/// `PropertyMut::set`, and the whole file is read again. Three things are asserted about the
/// result: it is still readable and a fixed point, the value is the value that went in, and the
/// guard line beside it is untouched — which is `docs/adr/0001`'s mutation locality, stated
/// against a write whose whole content came out of the same file.
macro_rules! identity_write {
    ($ty:ty, $name:expr, $params:expr, $text:expr) => {{
        let input = one_property($name, $params, $text);
        assert_eq!(
            read(&input).0.as_deref(),
            Ok(input.as_slice()),
            "{:?}",
            $text
        );
        let start = tree_of(&input);
        let id = PropertyId::from_name($name);
        if let Some(before) = value_of::<$ty>(&start, &id).value() {
            let mut edited = tree_of(&input);
            if write_value(&mut edited, &id, &before).is_ok() {
                let written = edited.to_bytes();
                let again = tree_of(&written);
                assert_eq!(again.to_bytes(), written, "a fixed point: {:?}", $text);
                assert_eq!(
                    value_of::<$ty>(&again, &id).value(),
                    Some(before),
                    "the value moved: {:?} became {:?}",
                    $text,
                    written
                );
                assert!(
                    line_of(&again, b"untouched").is_some(),
                    "the write reached a second line: {:?}",
                    $text
                );
            }
        }
    }};
}

/// A one-event calendar carrying `text` as the value of `name`, and a guard line beside it.
fn one_property(name: &[u8], params: &[u8], text: &[u8]) -> Vec<u8> {
    let mut out = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n".to_vec();
    out.extend_from_slice(name);
    out.extend_from_slice(params);
    out.push(b':');
    out.extend_from_slice(text);
    out.extend_from_slice(b"\r\nX-GUARD:untouched\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
    out
}

/// Section 3.3.6 and section 3.3.14, whose written forms carry terms a value need not keep.
#[test]
fn p3_writing_back_a_span_or_an_offset_keeps_the_value_and_reaches_one_line() {
    let spans: &[&[u8]] = &[
        b"P1W",
        b"P1D",
        b"PT1H",
        b"PT1M",
        b"PT1S",
        b"PT0S",
        b"P0D",
        b"P1DT1H",
        b"P1DT0H0M0S",
        b"-P1D",
        b"-PT1H",
        b"P0W",
        b"PT24H",
        b"PT86400S",
        b"P99999999D",
        b"P1DT23H59M60S",
    ];
    for text in spans {
        identity_write!(Duration, b"DURATION", b"", text);
    }
    let offsets: &[&[u8]] = &[
        b"+0000", b"+000000", b"-0100", b"-010000", b"+0530", b"+053000", b"+235859", b"+235860",
        b"+0059", b"+005900", b"+000001", b"-000001",
    ];
    for text in offsets {
        identity_write!(UtcOffset, b"TZOFFSETTO", b"", text);
    }
}

/// Section 3.3.4, section 3.3.5 and section 3.3.9, where the zone lives beside the value.
#[test]
fn p3_writing_back_a_date_time_or_a_period_keeps_the_value_and_the_zone() {
    let stamps: &[&[u8]] = &[
        b"20260101",
        b"20260101T000000",
        b"20260101T000000Z",
        b"20240229",
        b"99991231T235960Z",
        b"20260101T235960",
    ];
    let zones: &[&[u8]] = &[
        b"",
        b";TZID=Europe/Paris",
        b";TZID=\"Europe/Paris\"",
        b";VALUE=DATE",
    ];
    for text in stamps {
        for params in zones {
            identity_write!(DateTimeValue<'_>, b"DTSTART", params, text);
        }
    }
    let spans: &[&[u8]] = &[
        b"20260101T000000Z/20260102T000000Z",
        b"20260101T000000/20260102T000000",
        b"20260101T000000Z/PT1H",
        b"20260101T000000/P1D",
        b"20260101T000000Z/P1W",
    ];
    for text in spans {
        identity_write!(Period<'_>, b"FREEBUSY", b"", text);
        identity_write!(Period<'_>, b"FREEBUSY", b";TZID=Europe/Paris", text);
    }
}

/// Section 3.3.1, section 3.3.2, section 3.3.8, section 3.3.11 and section 3.3.13.
#[test]
fn p3_writing_back_the_octet_shaped_value_types_keeps_the_value() {
    let numbers: &[&[u8]] = &[
        b"0",
        b"007",
        b"+5",
        b"-0",
        b"-007",
        b"2147483647",
        b"-2147483648",
    ];
    for text in numbers {
        identity_write!(i32, b"SEQUENCE", b"", text);
    }
    let flags: &[&[u8]] = &[b"TRUE", b"true", b"True", b"FALSE", b"false", b"fAlSe"];
    for text in flags {
        identity_write!(bool, b"X-FLAG", b"", text);
    }
    let quanta: &[&[u8]] = &[
        b"",
        b"aGVsbG8=",
        b"aGVsbG9=",
        b"AA==",
        b"AB==",
        b"////",
        b"++++",
    ];
    for text in quanta {
        identity_write!(
            BinaryValue<'_>,
            b"ATTACH",
            b";VALUE=BINARY;ENCODING=BASE64",
            text
        );
    }
    let addresses: &[&[u8]] = &[
        b"mailto:ann@example.test",
        b"MAILTO:Ann@Example.Test",
        b"http://example.test/a%2Fb",
        b"urn:uuid:1-2-3",
    ];
    for text in addresses {
        identity_write!(UriValue<'_>, b"ORGANIZER", b"", text);
    }
    let notes: &[&[u8]] = &[
        br"line one\nline two",
        br"line one\Nline two",
        br"a\,b",
        br"a\\b",
        br"trailing backslash \",
        br"undefined \q escape",
        b"plain",
    ];
    for text in notes {
        identity_write!(TextValue<'_>, b"SUMMARY", b"", text);
    }
}

/// RFC 4648 section 10's vectors, which is the alphabet section 3.3.1 points at.
#[test]
fn rfc5545_3_3_1_inline_octets_are_the_octets_rfc4648_says_they_are() {
    let vectors: &[(&[u8], &[u8])] = &[
        (b"", b""),
        (b"Zg==", b"f"),
        (b"Zm8=", b"fo"),
        (b"Zm9v", b"foo"),
        (b"Zm9vYg==", b"foob"),
        (b"Zm9vYmE=", b"fooba"),
        (b"Zm9vYmFy", b"foobar"),
    ];
    for (text, octets) in vectors {
        let view = BinaryValue::decode_value(text).expect("section 3.3.1's syntax");
        assert_eq!(view.decode().as_deref(), Ok(*octets), "{text:?}");
    }
    // Padding bits nobody defined: two texts stand for one octet and each comes back as itself.
    let padded = BinaryValue::decode_value(b"AB==").expect("section 3.3.1's syntax");
    assert_eq!(padded.decode(), Ok(vec![0]));
    assert_eq!(
        BinaryValue::decode_value(b"AA==").map(BinaryValue::decode),
        Ok(Ok(vec![0]))
    );
    // A `=` anywhere but the tail of the last quantum is not padding, and not a value.
    for wrong in [&b"=AAA"[..], b"A=AA", b"AAA=AAAA", b"A===", b"AAA"] {
        assert_eq!(
            BinaryValue::decode_value(wrong).err(),
            Some(DiagnosticCode::MalformedBinary),
            "{wrong:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// What does not
// ---------------------------------------------------------------------------------------

/// RFC 5545 section 3.3.6: a span this crate writes has to be a span this crate reads.
///
/// `codec.rs` opens with "a decoder is exact or it refuses" and "an encoder exists only where
/// the value determines its own text". `Duration` is a public type with a public `new` over two
/// `i64` fields, and the day count nearest the bottom of that range has no unsigned magnitude
/// that fits back into one — so the encoder writes twenty digits the decoder will not take.
///
/// Nothing refuses along the way. `EncodeValue::encode_value` answers `Ok`, `PropertyMut::set`
/// answers `Ok`, the file serializes, and the property that comes back is
/// `DiagnosticCode::MalformedDuration` — a value this crate authored and then declined to read.
/// The refusal belongs in the encoder, next to the one `single_signed` already makes for a span
/// whose two halves will not reconcile.
#[test]
fn rfc5545_3_3_6_a_span_this_crate_writes_is_a_span_this_crate_reads_again() {
    let input = one_property(b"DURATION", b"", b"PT1H");
    let mut document = tree_of(&input);

    // Section 3.3.6 carries one sign for the whole value, outside the number, so a magnitude
    // with no positive counterpart has no text at all. Both fields reach it.
    for unwritable in [Duration::new(i64::MIN, -1), Duration::new(-1, i64::MIN)] {
        assert_eq!(
            write_value(&mut document, &PropertyId::DURATION, &unwritable),
            Err(MutationError::NotRepresentable),
            "a span with no RFC 5545 form was written"
        );
        assert_eq!(
            document.to_bytes(),
            input,
            "and a refused write left the file as it was"
        );
    }

    // The refusal is exact rather than a range: the span one unit away is written, and reads
    // back as the span that was written. Both halves stay negative, because a negative day
    // count beside zero seconds is the shape `single_signed` reconciles by multiplying — which
    // is its own overflow and its own refusal, one branch earlier.
    let writable = Duration::new(i64::MIN.saturating_add(1), -1);
    assert_eq!(
        write_value(&mut document, &PropertyId::DURATION, &writable),
        Ok(())
    );
    let written = document.to_bytes();
    let again = tree_of(&written);
    let back: View<'_, Duration> = value_of(&again, &PropertyId::DURATION);
    assert_ne!(
        back.diagnostic().map(Diagnostic::code),
        Some(DiagnosticCode::MalformedDuration),
        "this crate wrote {:?} and then declined to read it",
        String::from_utf8_lossy(&written)
    );
    assert_eq!(back.value(), Some(writable), "and the span survived");
}

/// RFC 5545 section 3.2.19: the zone a value states is the zone the next read sees.
///
/// `zone_of` is deliberate about the empty case — "an empty `TZID` names no zone, so it is not
/// one: `TZID=:` reads as a floating date-time" — and `zoned_bounds.ics` carries exactly that
/// line, which reads as `Local`. The write side has no matching rule.
/// `DateTimeValue::Zoned` is a public variant with a public `tzid` field, and
/// `coupled_parameters` assigns whatever it holds, so writing a zoned date-time whose zone is
/// empty emits `TZID=` and the same read that was careful a moment ago now answers `Local`.
///
/// The value the caller stated is gone, `set` reported success, and no diagnostic was produced
/// anywhere: this is the zone loss `DateTimeValue::Zoned` was added to make impossible,
/// arriving through the door that adds it. `MutationError::NotRepresentable` is the answer the
/// write side already has for a `TZID` no line could name.
#[test]
fn rfc5545_3_2_19_a_zone_written_through_the_value_is_the_zone_read_back() {
    let nameless = tree_of(ZONED_BOUNDS);
    let carried = properties(&nameless)
        .into_iter()
        .find(|property| property.is_named(b"X-NAMELESS-ZONE"))
        .expect("the fixture carries the line");
    assert_eq!(
        carried.value::<DateTimeValue<'_>>().value(),
        DateTimeValue::decode_value(b"20260101T090000").ok(),
        "the read side says `TZID=` names no zone"
    );

    let DateTimeValue::Local(stamp) =
        DateTimeValue::decode_value(b"20260101T090000").expect("a floating date-time")
    else {
        panic!("a value with no `Z` and no zone is the floating one");
    };
    let stated = DateTimeValue::Zoned { stamp, tzid: b"" };

    let input = one_property(b"DTSTART", b"", b"20260101T090000");
    let mut document = tree_of(&input);
    assert_eq!(
        write_value(&mut document, &PropertyId::DTSTART, &stated),
        Err(MutationError::NotRepresentable),
        "a zone the read side has ruled is not a zone was written as one"
    );
    assert_eq!(
        document.to_bytes(),
        input,
        "and a refused write left the file as it was"
    );

    // The same refusal where the zone reaches the line through a period's bound, which is the
    // other door `coupled_parameters` writes a `TZID` from.
    let span = Period::Explicit {
        start: DateTimeValue::Zoned { stamp, tzid: b"" },
        end: DateTimeValue::Utc(stamp),
    };
    let mut freebusy = tree_of(&one_property(
        b"FREEBUSY",
        b"",
        b"20260101T090000Z/20260101T100000Z",
    ));
    assert_eq!(
        write_value(&mut freebusy, &PropertyId::from_name(b"FREEBUSY"), &span),
        Err(MutationError::NotRepresentable)
    );

    // The zone that does name something is written and read back as the zone it named.
    let named = DateTimeValue::Zoned {
        stamp,
        tzid: b"Europe/Paris",
    };
    assert_eq!(
        write_value(&mut document, &PropertyId::DTSTART, &named),
        Ok(())
    );
    let written = document.to_bytes();
    let again = tree_of(&written);
    let back: View<'_, DateTimeValue<'_>> = value_of(&again, &PropertyId::DTSTART);
    assert_eq!(
        back.value().and_then(DateTimeValue::tzid),
        Some(&b"Europe/Paris"[..]),
        "wrote {:?}",
        String::from_utf8_lossy(&written)
    );
}

/// RFC 5545 section 3.3.7: a `FLOAT` is read as the number it names, or it is not read.
///
/// `is_float_text` exists so that "the standard library's float reader accepts spellings this
/// format does not have — `1e5`, `.5`, `inf`, `NaN` — and accepting them would mean a value RFC
/// 5545 calls malformed arriving as a number". The guard is about spellings, and infinity does
/// not need one: `1` followed by three hundred and nine zeros is section 3.3.7's syntax exactly,
/// every octet a digit, and the nearest `f64` to it is `inf`.
///
/// So the decoder is neither exact nor refusing. `View::Valid` comes back for a `GEO` whose
/// latitude is not a number, `Geo::latitude` hands `inf` to a caller that has been told the pair
/// is derived from authoritative text, and every comparison that caller makes against it is
/// wrong in a way no diagnostic mentions. `DiagnosticCode::MalformedFloat` is the answer, for
/// the same reason `decimal` refuses a `SEQUENCE` of two hundred digits rather than saturating.
#[test]
fn rfc5545_3_3_7_a_float_is_read_as_the_number_it_names_or_it_is_not_read() {
    let mut past_the_range = b"1".to_vec();
    past_the_range.extend(core::iter::repeat_n(b'0', 309));
    assert_eq!(
        f64::decode_value(&past_the_range).err(),
        Some(DiagnosticCode::MalformedFloat),
        "a decimal expansion no `f64` holds reads as {:?} instead",
        f64::decode_value(&past_the_range)
    );

    let document = tree_of(FLOAT_EXTREMES);
    let pair: View<'_, Geo> = value_of(&document, &PropertyId::GEO);
    assert_eq!(
        pair.diagnostic().map(Diagnostic::code),
        Some(DiagnosticCode::MalformedFloat),
        "the fixture's GEO reads as {:?}",
        pair.value()
    );
}
