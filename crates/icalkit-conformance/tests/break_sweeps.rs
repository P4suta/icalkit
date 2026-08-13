// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The sweep in `tests/sweep.rs`, attacked as an artifact rather than used as one.
//!
//! `docs/adr/0001` states four claims — a byte-identical round trip (P1), a fixed point (P2),
//! mutation locality (P3), and a diagnostic that costs no octet (P4) — and this milestone
//! landed `sweep.rs` as the evidence for them, replacing measurements that were run once and
//! discarded. A sweep is only evidence for what it examines, so this file examines the sweep.
//!
//! Three things are asked of it and each is answered by running it rather than by reading it.
//! Does its refusal predicate confirm bounds that were really crossed, or does it accuse the
//! reader of a defect the reader does not have? Does it reach the claims it is cited for, or
//! only two of the four? And is it wide enough that a defect has somewhere to show up — which
//! is asked by widening it, and by putting the two claims it never reaches under the same
//! generative pressure it puts P1 and P2 under.
//!
//! The generator, the octet alphabet and the `examine` / `crossed` pair below are the ones
//! `sweep.rs` committed, reproduced here because an integration test cannot call into a sibling
//! test binary. Where a constant differs it is because this file is deliberately wider, and the
//! difference is named at the constant.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ical_core::{
    Component, Diagnostic, Document, GrammarLimits, Item, Limits, ParameterEdit, ParseError,
    Property, PropertyId, ProposedChange, TextValue,
};

/// A calendar whose one property arrived below a fold at octet zero of its content line.
///
/// RFC 5545 section 3.1 folds at octets and says nothing about what they spell, so a fold
/// followed by `SP` and then `HTAB` unfolds to a content line whose first octet is `HTAB` —
/// which is to say, to a property whose *name* begins with whitespace. The file round trips.
const FOLD_AT_OFFSET_ZERO: &[u8] = include_bytes!("fixtures/break_sweeps/fold_at_offset_zero.ics");

/// A calendar whose `END` names another component, which section 3.6 recovery keeps as a
/// property.
const MISMATCHED_END: &[u8] =
    include_bytes!("fixtures/break_sweeps/mismatched_end_kept_as_property.ics");

/// Forty component boundaries whose `BEGIN` is folded, which is past the default depth bound.
const FOLDED_BEGIN_NEST: &[u8] =
    include_bytes!("fixtures/break_sweeps/folded_begin_past_the_depth_bound.ics");

/// The octets `sweep.rs` enumerates exhaustively, reproduced unchanged.
const ALPHABET: &[u8] = b":;,\"\\^\r\n \tA\xE9";

/// The seed this file's own draws start from, which is not `sweep.rs`'s.
///
/// A different constant on purpose: a sweep that reuses another sweep's seed covers the inputs
/// that one already covered and reports that they are still fine.
const SEED: u64 = 0x5EED_0000_B4EA_C001;

/// The evidence M0-alpha reported for the exhaustive leg and did not commit.
const RECORDED_EXHAUSTIVE: u64 = 1_900_000;

/// The evidence M0-alpha reported for the randomized leg and did not commit.
const RECORDED_RANDOMIZED: u64 = 2_200_000;

/// The evidence M0-alpha reported for the generative leg and did not commit.
const RECORDED_GENERATIVE: u64 = 135_000;

/// How much of a failing input an assertion message carries before it stops being readable.
const RENDER_LIMIT: usize = 200;

/// A policy a generated calendar crosses about as often as it does not, as `sweep.rs` states it.
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
    /// Refused, naming a bound the octets are supposed to independently confirm.
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
        ParseError::InputTooLarge { limit } => held > limit,
        ParseError::ValueTooLarge { limit } | ParseError::HeaderTooLarge { limit } => {
            held > u64::from(limit)
        },
        ParseError::TooManyParameters { limit } => count_octet(octets, b';') > u64::from(limit),
        ParseError::TooManyFolds { limit } => continuations(octets) > u64::from(limit),
        ParseError::TooManyItems { limit } => segments(octets) > u64::from(limit),
        // Counted after the folds are taken out: `BEG\r\n IN:VEVENT` opens a component and
        // carries no `BEGIN`, which is the arm this file found unsound in `sweep.rs`.
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

/// How many terminated segments the input holds, counting a last one that is empty.
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

/// The three terminators section 3.1 leaves a reader to tell apart, and none at all.
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
    // Up to two folds rather than `sweep.rs`'s one, because a second fold is what puts one at
    // octet zero often enough for the shape to be drawn rather than only constructed.
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

/// The committed `sweep.rs`, read as text so a claim about it can be stated mechanically.
fn sweep_source() -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("sweep.rs");
    fs::read_to_string(&path).unwrap_or_default()
}

/// The value of `const <name>: <type> = <value>;` in `source`, where it is a plain number.
///
/// Digit separators are dropped, because the file writes `1_500` and means fifteen hundred.
fn constant(source: &str, name: &str) -> Option<u64> {
    let after = source.split_once(&format!("const {name}:"))?.1;
    let value = after.split_once('=')?.1.split_once(';')?.0;
    value.trim().replace('_', "").parse().ok()
}

/// One node of a document, as the octets it holds rather than as a pointer into a tree.
///
/// A scoped write is supposed to reach one of these and no other, so the check that it did is a
/// comparison of two of these lists. Rendering to text rather than comparing nodes is what lets
/// a failure say which node moved and what it became.
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

/// One item, whichever of the two variants it is.
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

/// How many nodes of `before` are not the node at the same position of `after`.
///
/// A count rather than a diff: P3 admits exactly one, the property the write named.
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

/// Read `octets`, write `value` into one property of the first component, and read back.
///
/// `named` selects the property; `None` takes the first one the component holds. Returns the
/// octets that were written and the two outlines to compare, or nothing where the input holds
/// no component with a property to name.
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

/// The write is refused, which is the other answer the break named and the one that is
/// available: there is no octet string that stores `END:VEVENT` as a property and reads back as
/// one, so a door that wrote it would be choosing between losing the property and losing the
/// caller's value. The property keeps every octet it arrived with and stays reachable.
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
    // Spelled narrowly enough that `octets.get_mut(at)` in the mutation helper is not mistaken
    // for the tree's own door: what is being asked is whether the sweep ever writes a property.
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

#[test]
fn a_parameter_edit_reaches_one_property_across_a_generated_corpus() {
    let mut stream = Stream::new(SEED);
    let mut checked = 0_u64;
    let mut first = String::new();
    for _ in 0..8_000 {
        let octets = calendar(&mut stream);
        let Ok(mut document) = Document::parse(&octets, Limits::DEFAULT, &mut Vec::new()) else {
            continue;
        };
        let before = outline(&document);
        let Some(component) = document.components_mut().next() else {
            continue;
        };
        let Some(target) = component
            .items()
            .iter()
            .filter_map(Item::as_property)
            .map(|property| PropertyId::from_name(property.name().as_bytes()))
            .next()
        else {
            continue;
        };
        // A change is addressed to an identity rather than to an occurrence, so a component
        // carrying that identity twice has two nodes a parameter edit may reach — and both, or
        // the caller is left with the property it addressed carrying two different parameter
        // sets. What P3 forbids is a node outside that set moving, which is what a count
        // against the occurrences measures and a count against one does not.
        let occurrences = component
            .items()
            .iter()
            .filter_map(Item::as_property)
            .filter(|property| PropertyId::from_name(property.name().as_bytes()) == target)
            .count();
        let value = stream.pick(&[b"a:b".as_slice(), b"plain", b""]).copied();
        let edit = ParameterEdit::set(b"X-STATE", value.unwrap_or(b"plain"));
        if component
            .apply(
                &target,
                &ProposedChange::SetParameters(vec![edit]),
                Limits::DEFAULT,
            )
            .is_err()
        {
            continue;
        }
        checked = checked.saturating_add(1);
        let written = document.to_bytes();
        let Ok(reread) = Document::parse(&written, Limits::DEFAULT, &mut Vec::new()) else {
            continue;
        };
        let after = outline(&reread);
        if (before.len() != after.len() || moved(&before, &after) > occurrences) && first.is_empty()
        {
            first = format!(
                "{} wrote {}: {before:?} became {after:?}",
                render(&octets),
                render(&written)
            );
        }
    }
    println!("parameter edits checked: {checked}");
    assert!(first.is_empty(), "seed {SEED:#x}: {first}");
}
