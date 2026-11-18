// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Independent checks for the preservation contracts of `tests/sweep.rs`.
//!
//! `ADR-0001` defines byte preservation, serialization stability, mutation locality, and diagnostic reporting.
//! The tests check whether the input could have crossed the bound named in a refusal.
//! They compare fixture coverage with the reported sample counts.
//! Generated inputs also exercise property writes and diagnostic reporting.
//!
//! Integration tests compile as separate binaries.
//! The local generator uses the alphabet and checks from `sweep.rs`.
//! The constants give the sample sizes and fold limits.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use icalkit_conformance::internal::core::{
    Component, Diagnostic, Document, GrammarLimits, Item, Limits, MutationError, ParameterEdit,
    ParseError, Property, PropertyId, ProposedChange, TextValue,
};

/// The first property starts below a fold at octet zero of its content line.
///
/// A fold followed by `SP` and then `HTAB` unfolds to a content line whose first octet is `HTAB`.
/// The property name begins with whitespace.
/// The reader and writer preserve the original octets.
const FOLD_AT_OFFSET_ZERO: &[u8] = include_bytes!("fixtures/break_sweeps/fold_at_offset_zero.ics");

/// A calendar whose `END` names another component, which section 3.6 recovery keeps as a property.
const MISMATCHED_END: &[u8] =
    include_bytes!("fixtures/break_sweeps/mismatched_end_kept_as_property.ics");

/// Forty nested components with folded `BEGIN` lines exceed the default depth limit.
const FOLDED_BEGIN_NEST: &[u8] =
    include_bytes!("fixtures/break_sweeps/folded_begin_past_the_depth_bound.ics");

/// The octets `sweep.rs` enumerates exhaustively, reproduced unchanged.
const ALPHABET: &[u8] = b":;,\"\\^\r\n \tA\xE9";

/// A separate seed gives the generated sweep a different sequence of inputs.
const SEED: u64 = 0x5EED_0000_B4EA_C001;

/// The exhaustive sample count from the M0-alpha report.
const RECORDED_EXHAUSTIVE: u64 = 1_900_000;

/// The randomized sample count from the M0-alpha report.
const RECORDED_RANDOMIZED: u64 = 2_200_000;

/// The generative sample count from the M0-alpha report.
const RECORDED_GENERATIVE: u64 = 135_000;

/// How much of a failing input an assertion message carries before it stops being readable.
const RENDER_LIMIT: usize = 200;

/// The limits from `sweep.rs` exercise both accepted and refused inputs.
const BOUNDED: Limits = Limits::DEFAULT
    .with_grammar(
        GrammarLimits::DEFAULT
            .with_max_header_bytes(12)
            .with_max_parameters(2)
            .with_max_folds_per_line(1),
    )
    .with_max_input_bytes(48)
    .with_max_value_bytes(8)
    .with_max_items(4)
    .with_max_component_depth(2);

/// What one input turned out to be, which is the only two answers `sweep.rs` accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Read, written back octet for octet, and a second pass agreed with the first.
    Preserved,
    /// The reader refused the input and identified the bound to check.
    Refused(ParseError),
}

/// `sweep.rs`'s own three claims over one input, reproduced so this file can name the same ones.
fn examine(octets: &[u8], limits: Limits) -> Result<Verdict, String> {
    let mut kept: Vec<Diagnostic> = Vec::new();
    let tree = match Document::parse(octets, limits, &mut kept) {
        Ok(read) => read,
        Err(refusal) if crossed(octets, refusal) => return Ok(Verdict::Refused(refusal)),
        Err(refusal) => {
            return Err(format!(
                "{refusal:?} names a bound {} never crossed",
                render(octets)
            ));
        },
    };
    let written = tree.to_bytes();
    if written != octets {
        return Err(format!(
            "{} was written back as {}",
            render(octets),
            render(&written)
        ));
    }
    match Document::parse(&written, limits, &mut Vec::new()) {
        Ok(reread) if reread.to_bytes() == written => Ok(Verdict::Preserved),
        Ok(reread) => Err(format!(
            "{} is no fixed point: a second pass wrote {}",
            render(&written),
            render(&reread.to_bytes())
        )),
        Err(refusal) => Err(format!(
            "{refusal:?}: what this crate wrote for {} was refused on the next read",
            render(octets)
        )),
    }
}

/// Whether the octets independently confirm the bound `refusal` names, as `sweep.rs` asks it.
fn crossed(octets: &[u8], refusal: ParseError) -> bool {
    let held = as_units(octets.len());
    match refusal {
        ParseError::Exhausted => false,
        ParseError::InputTooLarge { limit } => held > limit,
        ParseError::ValueTooLarge { limit } | ParseError::HeaderTooLarge { limit } => {
            held > u64::from(limit)
        },
        ParseError::TooManyParameters { limit } => count_octet(octets, b';') > u64::from(limit),
        ParseError::TooManyFolds { limit } => continuations(octets) > u64::from(limit),
        ParseError::TooManyItems { limit } => segments(octets) > u64::from(limit),
        // Unfold before counting component starts such as `BEG\r\n IN:VEVENT`.
        ParseError::TooDeep { limit } => {
            keyword_count(&unfolded(octets), b"BEGIN") > u64::from(limit)
        },
    }
}

/// The input with RFC 5545 section 3.1's folds taken out, as `sweep.rs` now takes them.
fn unfolded(octets: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(octets.len());
    let mut at = 0_usize;
    while at < octets.len() {
        let width = terminator_width(octets, at);
        let after = at.saturating_add(width);
        if width > 0 && matches!(octets.get(after), Some(&(b' ' | b'\t'))) {
            at = after.saturating_add(1);
            continue;
        }
        if let Some(&octet) = octets.get(at) {
            out.push(octet);
        }
        at = at.saturating_add(1);
    }
    out
}

/// A count as a charge-sized number, saturating rather than wrapping.
fn as_units(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// How many times `wanted` occurs in `octets`.
fn count_octet(octets: &[u8], wanted: u8) -> u64 {
    octets.iter().fold(0, |seen, held| {
        if *held == wanted {
            seen.saturating_add(1)
        } else {
            seen
        }
    })
}

/// How many times `keyword` occurs in `octets`, ignoring case as section 3.1 does.
fn keyword_count(octets: &[u8], keyword: &[u8]) -> u64 {
    as_units(
        octets
            .windows(keyword.len())
            .filter(|window| window.eq_ignore_ascii_case(keyword))
            .count(),
    )
}

/// The width of the terminator at `at`, or zero where there is none.
fn terminator_width(octets: &[u8], at: usize) -> usize {
    match octets.get(at) {
        Some(&b'\r') if octets.get(at.saturating_add(1)) == Some(&b'\n') => 2,
        Some(&(b'\r' | b'\n')) => 1,
        _ => 0,
    }
}

/// Count terminated segments, including an empty final segment.
fn segments(octets: &[u8]) -> u64 {
    let mut counted = 1_u64;
    let mut at = 0_usize;
    while at < octets.len() {
        let width = terminator_width(octets, at);
        if width == 0 {
            at = at.saturating_add(1);
            continue;
        }
        counted = counted.saturating_add(1);
        at = at.saturating_add(width);
    }
    counted
}

/// How many continuation lines the input holds: a terminator followed by `SP` or `HTAB`.
fn continuations(octets: &[u8]) -> u64 {
    let mut counted = 0_u64;
    let mut at = 0_usize;
    while at < octets.len() {
        let width = terminator_width(octets, at);
        if width == 0 {
            at = at.saturating_add(1);
            continue;
        }
        let after = at.saturating_add(width);
        if matches!(octets.get(after), Some(&(b' ' | b'\t'))) {
            counted = counted.saturating_add(1);
        }
        at = after;
    }
    counted
}

/// Octets as something an assertion message can carry, with nothing left to guess at.
fn render(octets: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &held in octets.iter().take(RENDER_LIMIT) {
        match held {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'"' => out.push_str("\\\""),
            _ if held == b' ' || held.is_ascii_graphic() => out.push(char::from(held)),
            _ => {
                let _ = write!(out, "\\x{held:02X}");
            },
        }
    }
    out.push('"');
    if octets.len() > RENDER_LIMIT {
        let _ = write!(out, " (first {RENDER_LIMIT} of {} octets)", octets.len());
    }
    out
}

/// A deterministic source of draws, seeded from a committed constant: `sweep.rs`'s `splitmix64`.
#[derive(Debug)]
struct Stream {
    /// The whole state, advanced once per draw.
    state: u64,
}

impl Stream {
    /// A stream at `seed`.
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next sixty-four bits.
    fn draw(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mixed = self.state;
        let once = (mixed ^ mixed.wrapping_shr(30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let twice = (once ^ once.wrapping_shr(27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        twice ^ twice.wrapping_shr(31)
    }

    /// A number below `bound`, and zero when `bound` is zero.
    fn below(&mut self, bound: usize) -> usize {
        let drawn = usize::try_from(self.draw() & 0xFFFF_FFFF).unwrap_or(0);
        drawn.checked_rem(bound).unwrap_or(0)
    }

    /// One of `choices`, and nothing when there are none.
    fn pick<'a, T>(&mut self, choices: &'a [T]) -> Option<&'a T> {
        choices.get(self.below(choices.len()))
    }
}

/// Names a generated line may carry, as `sweep.rs` lists them.
const NAMES: &[&[u8]] = &[
    b"SUMMARY",
    b"DTSTART",
    b"UID",
    b"X-VENDOR",
    b"BEGIN",
    b"END",
    b"",
];

/// Parameter runs, written as they appear in a header.
const PARAMETERS: &[&[u8]] = &[
    b"",
    b";TZID=Etc/UTC",
    b";X-Q=\"a;b,c:d\"",
    b";CN=\"never closed",
    b";BARE",
    b";=empty",
];

/// Values, chosen for what a reader has to do with them.
const VALUES: &[&[u8]] = &[
    b"",
    b"VEVENT",
    b"vevent",
    b"20260810T120000Z",
    b"^'quoted^'",
    b"\xE9\xE9\xE9",
    b"has a space",
];

/// Generate CRLF, lone LF, lone CR, and unterminated lines.
const TERMINATORS: &[&[u8]] = &[b"\r\n", b"\r\n", b"\n", b"\r", b""];

/// The ways a producer may introduce a continuation line.
const FOLDS: &[&[u8]] = &[b"\r\n ", b"\r\n\t", b"\n ", b"\r\t"];

/// Append what `chosen` holds, and nothing where it holds nothing.
fn extend(out: &mut Vec<u8>, chosen: Option<&&[u8]>) {
    if let Some(fragment) = chosen {
        out.extend_from_slice(fragment);
    }
}

/// Assemble one content line and fold it somewhere a producer would not have chosen.
fn append_line(stream: &mut Stream, out: &mut Vec<u8>) {
    let mut assembled: Vec<u8> = Vec::new();
    extend(&mut assembled, stream.pick(NAMES));
    let runs = stream.below(3);
    for _ in 0..runs {
        extend(&mut assembled, stream.pick(PARAMETERS));
    }
    if stream.below(8) > 0 {
        assembled.push(b':');
    }
    extend(&mut assembled, stream.pick(VALUES));
    // Two folds let the generator place a fold at octet zero.
    for _ in 0..stream.below(3) {
        let at = stream.below(assembled.len().saturating_add(1));
        let fold = stream.pick(FOLDS).copied().unwrap_or(b"\r\n ");
        let mut folded: Vec<u8> = Vec::new();
        folded.extend_from_slice(assembled.get(..at).unwrap_or_default());
        folded.extend_from_slice(fold);
        folded.extend_from_slice(assembled.get(at..).unwrap_or_default());
        assembled = folded;
    }
    out.append(&mut assembled);
    extend(out, stream.pick(TERMINATORS));
}

/// A whole calendar, or something shaped enough like one to be worth reading.
fn calendar(stream: &mut Stream) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let wrapped = stream.below(5);
    for _ in 0..wrapped {
        out.extend_from_slice(b"BEGIN:VCALENDAR\r\n");
    }
    for _ in 0..stream.below(12) {
        append_line(stream, &mut out);
    }
    for _ in 0..stream.below(wrapped.saturating_add(1)) {
        out.extend_from_slice(b"END:VCALENDAR\r\n");
    }
    out
}

/// The `index`th string of exactly `length` octets over [`ALPHABET`].
fn nth_input(length: usize, index: usize, out: &mut Vec<u8>) {
    out.clear();
    let mut left = index;
    for _ in 0..length {
        let slot = left.checked_rem(ALPHABET.len()).unwrap_or(0);
        left = left.checked_div(ALPHABET.len()).unwrap_or(0);
        out.push(ALPHABET.get(slot).copied().unwrap_or(b'A'));
    }
}

/// How many strings of exactly `length` octets there are over [`ALPHABET`].
fn population(length: usize) -> usize {
    u32::try_from(length)
        .ok()
        .and_then(|exponent| ALPHABET.len().checked_pow(exponent))
        .unwrap_or(0)
}

/// Every committed `.ics` fixture under this crate's `tests/fixtures`, in a stable order.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("tests");
    root.push("fixtures");
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    collect_fixtures(&root, &mut found);
    found.sort();
    found
}

/// Append every `.ics` file at or under `directory`.
fn collect_fixtures(directory: &Path, found: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures(&path, found);
            continue;
        }
        if path.extension().is_none_or(|kind| kind != "ics") {
            continue;
        }
        if let Ok(octets) = fs::read(&path) {
            found.push((path.display().to_string(), octets));
        }
    }
}

/// The source of the maintained sweep for checking its input counts and write operations.
fn sweep_source() -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("sweep.rs");
    fs::read_to_string(&path).unwrap_or_default()
}

/// Read a numeric constant from the source, ignoring digit separators.
fn constant(source: &str, name: &str) -> Option<u64> {
    let after = source.split_once(&format!("const {name}:"))?.1;
    let value = after.split_once('=')?.1.split_once(';')?.0;
    value.trim().replace('_', "").parse().ok()
}

/// Render each node as text so a failed locality check can show the changed node.
fn outline_property(depth: usize, property: &Property, out: &mut Vec<String>) {
    let mut row = format!("{depth} property {} ", render(property.name().as_bytes()));
    for parameter in property.parameters() {
        let _ = write!(
            row,
            "{}={} ",
            render(parameter.name().as_bytes()),
            render(parameter.value().as_bytes())
        );
    }
    let _ = write!(row, "value {}", render(property.value_text().as_bytes()));
    out.push(row);
}

/// The same for a component, opening and closing so that nesting is part of the comparison.
fn outline_component(depth: usize, component: &Component, out: &mut Vec<String>) {
    out.push(format!(
        "{depth} begin {}",
        render(component.begin().name().as_bytes())
    ));
    for item in component.items() {
        outline_item(depth.saturating_add(1), item, out);
    }
    out.push(format!(
        "{depth} end {}",
        component
            .end()
            .map_or_else(|| "absent".to_owned(), |end| render(end.name().as_bytes()))
    ));
}

/// Render a property or component at the given depth.
fn outline_item(depth: usize, item: &Item, out: &mut Vec<String>) {
    match item {
        Item::Property(property) => outline_property(depth, property, out),
        Item::Component(component) => outline_component(depth, component, out),
    }
}

/// Every node of a document, in the order it serializes.
fn outline(document: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in document.items() {
        outline_item(0, item, &mut out);
    }
    out
}

/// Count the positions whose nodes differ between the two outlines.
/// P3 expects one changed property.
fn moved(before: &[String], after: &[String]) -> usize {
    before
        .iter()
        .zip(after.iter())
        .filter(|(left, right)| left != right)
        .count()
}

/// The identity of the first property directly inside `component`, if it has one.
fn first_identity(component: &Component) -> Option<PropertyId> {
    component
        .items()
        .iter()
        .filter_map(Item::as_property)
        .map(|property| PropertyId::from_name(property.name().as_bytes()))
        .next()
}

/// Read the input, write a property of the first component, and reopen the output.
/// Use the first property when `named` supplies no identity.
/// Return the serialized octets and both outlines if the component has a matching property.
fn write_one_value(
    octets: &[u8],
    named: Option<&PropertyId>,
    value: &[u8],
) -> Option<(Vec<u8>, Vec<String>, Vec<String>)> {
    let mut document = Document::parse(octets, Limits::DEFAULT, &mut Vec::new()).ok()?;
    let before = outline(&document);
    let component = document.components_mut().next()?;
    let chosen = match named {
        Some(identity) => identity.clone(),
        None => first_identity(component)?,
    };
    component
        .get_mut::<TextValue<'_>>(&chosen)?
        .set_raw(value)
        .ok()?;
    let written = document.to_bytes();
    let reread = Document::parse(&written, Limits::DEFAULT, &mut Vec::new()).ok()?;
    let after = outline(&reread);
    Some((written, before, after))
}

#[test]
fn p3_a_write_below_a_fold_at_octet_zero_stays_below_it() {
    assert_eq!(
        examine(FOLD_AT_OFFSET_ZERO, Limits::DEFAULT),
        Ok(Verdict::Preserved),
        "the fixture has to hold on its own before a write means anything"
    );
    let (written, before, after) =
        write_one_value(FOLD_AT_OFFSET_ZERO, None, b"written").expect("the fixture holds one");
    assert_eq!(
        before.len(),
        after.len(),
        "a write named one property and the document went from {before:?} to {after:?}, \
         because the fixture's property name begins with the `HTAB` the fold left at octet zero \
         and the write dropped the fold that kept it on a line of its own: {}",
        render(&written)
    );
    assert_eq!(moved(&before, &after), 1, "{before:?} became {after:?}");
}

/// The reader interprets `END:VEVENT` as a component boundary.
/// Refusing the write preserves the original property and its bytes.
#[test]
fn p3_a_write_to_a_property_does_not_turn_it_into_a_component_boundary() {
    assert_eq!(
        examine(MISMATCHED_END, Limits::DEFAULT),
        Ok(Verdict::Preserved),
        "the fixture has to hold on its own before a write means anything"
    );
    let mut document =
        Document::parse(MISMATCHED_END, Limits::DEFAULT, &mut Vec::new()).expect("the fixture");
    let component = document
        .components_mut()
        .next()
        .expect("the fixture holds one");
    let identity = PropertyId::from_name(b"END");
    let refusal = component
        .get_mut::<TextValue<'_>>(&identity)
        .expect("section 3.6 recovery kept the mismatched END as a property")
        .set_raw(b"VEVENT");
    assert!(
        refusal.is_err(),
        "a write to the value of the property section 3.6 recovery kept for a mismatched `END` \
         made that property close its component on the next read"
    );
    assert_eq!(
        document.to_bytes(),
        MISMATCHED_END,
        "and a refused write costs the file nothing"
    );
}

#[test]
fn p3_a_scoped_write_reaches_one_property_across_a_generated_corpus() {
    let mut stream = Stream::new(SEED);
    let mut checked = 0_u64;
    let mut broken = 0_u64;
    let mut first = String::new();
    for _ in 0..20_000 {
        let octets = calendar(&mut stream);
        let raw = stream
            .pick(&[b"written".as_slice(), b"", b"a:b;c,d"])
            .copied();
        let Some((written, before, after)) =
            write_one_value(&octets, None, raw.unwrap_or(b"written"))
        else {
            continue;
        };
        checked = checked.saturating_add(1);
        if before.len() != after.len() || moved(&before, &after) > 1 {
            broken = broken.saturating_add(1);
            if first.is_empty() {
                first = format!(
                    "{} wrote {}: {before:?} became {after:?}",
                    render(&octets),
                    render(&written)
                );
            }
        }
    }
    println!("P3 over generated calendars: {checked} writes checked, {broken} not local");
    assert_eq!(broken, 0, "seed {SEED:#x}: {first}");
}

#[test]
fn the_refusal_predicate_the_sweep_rests_on_confirms_the_bound_it_accepts() {
    let refusal = Document::parse(FOLDED_BEGIN_NEST, Limits::DEFAULT, &mut Vec::new())
        .err()
        .unwrap_or(ParseError::TooDeep { limit: 0 });
    assert!(
        crossed(FOLDED_BEGIN_NEST, refusal),
        "the reader refused {refusal:?} for a bound the input really crosses, and `crossed` \
         counts the octets `BEGIN` rather than the names the reader unfolded, so it found {} of \
         them and calls a sound refusal a defect in ical-core",
        keyword_count(FOLDED_BEGIN_NEST, b"BEGIN")
    );
}

#[test]
fn every_committed_fixture_is_examined_rather_than_counted_as_skipped() {
    let corpus = fixtures();
    assert!(corpus.len() >= 10, "the corpus directory moved");
    let mut skipped: Vec<String> = Vec::new();
    for (name, octets) in &corpus {
        if let Err(report) = examine(octets, Limits::DEFAULT) {
            skipped.push(format!("{name}: {report}"));
        }
    }
    assert!(
        skipped.is_empty(),
        "`sweep.rs` drops a fixture it cannot examine and prints a count instead of failing, so \
         {} of {} fixtures are swept by nothing at all: {skipped:?}",
        skipped.len(),
        corpus.len()
    );
}

#[test]
fn the_sweep_reaches_the_two_claims_it_is_cited_for_and_not_only_two() {
    let source = sweep_source();
    assert!(
        !source.is_empty(),
        "sweep.rs is where this milestone put it"
    );
    // Match property write methods without counting mutable access to input bytes.
    let doors = [
        "get_mut::<",
        "set_raw",
        "dtstart_mut",
        ".apply(",
        "ProposedChange",
        "ParameterEdit",
        "PropertyMut",
        "MutationError",
    ];
    let reached: Vec<&str> = doors
        .into_iter()
        .filter(|door| source.contains(door))
        .collect();
    assert!(
        !reached.is_empty(),
        "P3 is mutation locality and P4 is that a diagnosed input still round trips, and the \
         sweep names no mutation entry point at all — so of the four claims `docs/adr/0001` \
         states it exercises two, and three of M0-alpha's six breaks lived behind this door"
    );
}

#[test]
fn the_sweep_covers_the_evidence_it_was_landed_to_replace() {
    let source = sweep_source();
    let length = constant(&source, "EXHAUSTIVE_LENGTH").unwrap_or(0);
    let exhaustive = as_units(
        (0..=usize::try_from(length).unwrap_or(0))
            .map(population)
            .sum(),
    )
    .saturating_mul(2);
    let randomized = constant(&source, "CALENDARS")
        .unwrap_or(0)
        .saturating_mul(2);
    let per_fixture = constant(&source, "EDITS_PER_FIXTURE").unwrap_or(0);
    let generative = as_units(fixtures().len())
        .saturating_mul(per_fixture)
        .saturating_mul(5);
    println!(
        "committed sweep: {exhaustive} exhaustive, {randomized} randomized, {generative} \
         generative"
    );
    assert!(
        exhaustive >= RECORDED_EXHAUSTIVE
            && randomized >= RECORDED_RANDOMIZED
            && generative >= RECORDED_GENERATIVE,
        "M0-alpha reported {RECORDED_EXHAUSTIVE} exhaustive inputs, {RECORDED_RANDOMIZED} \
         randomized documents and {RECORDED_GENERATIVE} generative mutations, and the sweep \
         landed to hold that evidence covers {exhaustive}, {randomized} and {generative} — \
         which the nextest profile has room for, since the whole sweep binary finishes in two \
         seconds of the sixty it is given"
    );
}

#[test]
fn p1_and_p2_survive_a_sweep_several_times_the_committed_one() {
    let mut octets: Vec<u8> = Vec::new();
    let mut examined = 0_u64;
    for length in 0..=5 {
        for index in 0..population(length) {
            nth_input(length, index, &mut octets);
            examined = examined.saturating_add(1);
            if let Err(report) = examine(&octets, Limits::DEFAULT) {
                panic!("length {length}, index {index}: {report}");
            }
        }
    }
    let mut stream = Stream::new(SEED);
    for index in 0..20_000 {
        let drawn = calendar(&mut stream);
        for policy in [Limits::DEFAULT, BOUNDED] {
            examined = examined.saturating_add(1);
            if let Err(report) = examine(&drawn, policy) {
                panic!("calendar {index} from seed {SEED:#x}: {report}");
            }
        }
    }
    println!("P1 and P2 survived {examined} inputs this file drew");
    assert!(examined > 200_000, "only {examined} inputs were examined");
}

struct ParameterTarget<'a> {
    before: &'a Document,
    component: usize,
    properties: Vec<(usize, &'a Property)>,
    identity: PropertyId,
}

impl<'a> ParameterTarget<'a> {
    fn first(document: &'a Document) -> Option<Self> {
        let (index, component) = document
            .items()
            .iter()
            .enumerate()
            .find_map(|(index, item)| Some((index, item.as_component()?)))?;
        let identity = first_identity(component)?;
        let properties = component
            .items()
            .iter()
            .enumerate()
            .filter_map(|(position, item)| {
                let property = item.as_property()?;
                property.has_id(&identity).then_some((position, property))
            })
            .collect();
        Some(Self {
            before: document,
            component: index,
            properties,
            identity,
        })
    }

    fn check(
        &self,
        source: &[u8],
        written: &[u8],
        assignment: (&[u8], &[u8]),
        limits: Limits,
    ) -> Result<(), String> {
        let describe = |reason: &str| {
            format!(
                "{reason}: input {} output {}",
                source.escape_ascii(),
                written.escape_ascii()
            )
        };
        let after = Document::parse(written, limits, &mut Vec::new())
            .map_err(|error| describe(&format!("reopen failed with {error:?}")))?;
        if self.keeps_other_rows(after, assignment) {
            Ok(())
        } else {
            Err(describe(
                "the parameter assignment or an unchanged row did not survive reopening",
            ))
        }
    }

    fn keeps_other_rows(&self, mut after: Document, (name, value): (&[u8], &[u8])) -> bool {
        let Some(edited) = after
            .items_mut()
            .get_mut(self.component)
            .and_then(Item::as_component_mut)
        else {
            return false;
        };
        for &(index, old) in &self.properties {
            let Some(new) = edited
                .items_mut()
                .get_mut(index)
                .and_then(Item::as_property_mut)
            else {
                return false;
            };
            let assigned = new
                .parameters_named(name)
                .map(|parameter| (parameter.has_value(), parameter.unquoted()));
            if new.name() != old.name()
                || new.value_text() != old.value_text()
                || !assigned.eq([(true, value)])
            {
                return false;
            }
            // Restore each permitted row before comparing the complete trees and their layouts.
            new.clone_from(old);
        }
        *self.before == after
    }
}

#[test]
fn parameter_edit_checks_accept_duplicate_targets_and_reject_other_rows() {
    let source = concat!(
        "X-BEFORE:top\r\nBEGIN:VEVENT\r\nX-STATE:first\r\n",
        "BEGIN:VALARM\r\nX-STATE:nested\r\nEND:VALARM\r\n",
        "x-state:second\r\nSUMMARY:other\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nX-STATE:sibling\r\nEND:VEVENT\r\n",
    );
    let before = Document::parse(source.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
    let target = ParameterTarget::first(&before).unwrap();
    let assignment = (b"LANGUAGE".as_slice(), b"en".as_slice());
    let check = |written: &str| {
        target.check(
            source.as_bytes(),
            written.as_bytes(),
            assignment,
            Limits::DEFAULT,
        )
    };
    let both = source
        .replace("X-STATE:first", "X-STATE;LANGUAGE=en:first")
        .replace("x-state:second", "x-state;LANGUAGE=en:second");
    check(&both).unwrap();
    let mut edited = before.clone();
    edited.items_mut()[target.component]
        .as_component_mut()
        .unwrap()
        .apply(
            &target.identity,
            &ProposedChange::SetParameters(vec![ParameterEdit::set(assignment.0, assignment.1)]),
            Limits::DEFAULT,
        )
        .unwrap();
    target
        .check(
            source.as_bytes(),
            &edited.to_bytes(),
            assignment,
            Limits::DEFAULT,
        )
        .unwrap();
    for unapplied in [
        source.to_owned(),
        both.replace(";LANGUAGE=en:", ";LANGUAGE=fr:"),
        both.replace(";LANGUAGE=en:", ";LANGUAGE=en;LANGUAGE=en:"),
        source.replace("X-STATE:first", "X-STATE;LANGUAGE=en:first"),
        source.replace("x-state:second", "x-state;LANGUAGE=en:second"),
    ] {
        assert!(check(&unapplied).is_err());
    }
    for outside in [
        "X-STATE:nested",
        "X-STATE:sibling",
        "SUMMARY:other",
        "X-BEFORE:top",
    ] {
        let changed = both.replace(outside, &outside.replace(':', ";LANGUAGE=en:"));
        assert!(check(&changed).is_err(), "{outside}");
    }
    let changed_value = both.replace("LANGUAGE=en:first", "LANGUAGE=en:other");
    assert!(check(&changed_value).is_err());
    let refolded = both.replace("X-STATE:nested", "X-STATE:nes\r\n ted");
    assert!(check(&refolded).is_err());
}

#[test]
fn parameter_edit_checks_report_a_refused_reread() {
    let source = b"BEGIN:VEVENT\r\nSUMMARY:v\r\nEND:VEVENT\r\n";
    let before = Document::parse(source, Limits::DEFAULT, &mut Vec::new()).unwrap();
    let target = ParameterTarget::first(&before).unwrap();
    let written = b"BEGIN:VEVENT\r\nSUMMARY;LANGUAGE=en:v\r\nEND:VEVENT\r\n";
    let error = target
        .check(
            source,
            written,
            (b"LANGUAGE", b"en"),
            Limits::DEFAULT.with_max_input_bytes(1),
        )
        .unwrap_err();
    assert!(error.contains("InputTooLarge"), "{error}");
    assert!(
        error.contains(&source.escape_ascii().to_string()),
        "{error}"
    );
    assert!(
        error.contains(&written.escape_ascii().to_string()),
        "{error}"
    );
}

#[test]
fn a_parameter_edit_reaches_one_property_across_a_generated_corpus() {
    let mut stream = Stream::new(SEED);
    let mut checked = 0_u64;
    for index in 0..8_000 {
        let octets = calendar(&mut stream);
        let Ok(before) = Document::parse(&octets, Limits::DEFAULT, &mut Vec::new()) else {
            continue;
        };
        let Some(target) = ParameterTarget::first(&before) else {
            continue;
        };
        let mut document = before.clone();
        let component = document.items_mut()[target.component]
            .as_component_mut()
            .unwrap();
        let value = stream
            .pick(&[b"a:b".as_slice(), b"plain", b""])
            .copied()
            .unwrap();
        let edit = ParameterEdit::set(b"X-STATE", value);
        if let Err(error) = component.apply(
            &target.identity,
            &ProposedChange::SetParameters(vec![edit]),
            Limits::DEFAULT,
        ) {
            assert!(
                matches!(
                    error,
                    MutationError::ComponentBoundary | MutationError::NotRepresentable
                ),
                "seed {SEED:#x}, calendar {index}: {error:?} for {}",
                octets.escape_ascii()
            );
            assert_eq!(
                document.to_bytes(),
                octets,
                "refused edit at calendar {index}"
            );
            continue;
        }
        let written = document.to_bytes();
        target
            .check(&octets, &written, (b"X-STATE", value), Limits::DEFAULT)
            .unwrap_or_else(|report| panic!("seed {SEED:#x}, calendar {index}: {report}"));
        checked = checked.saturating_add(1);
    }
    assert!(
        checked > 0,
        "no parameter edits were checked for seed {SEED:#x}"
    );
    println!("parameter edits checked: {checked}");
}
