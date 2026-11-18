// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Check the lossless read and scoped write contracts in `docs/adr/0001`.
//!
//! Accepted input must serialize byte for byte before and after a second parse.
//! A panic or process termination fails the test.
//! For a refusal, [`crossed`] checks an independent upper bound from the entire input.
//! An input can exceed the bound for a header even if every individual header fits.
//!
//! The exhaustive sweep enumerates strings of up to six octets from a twelve-octet alphabet.
//! Randomized shards generate calendars from a fixed seed and shard index.
//! Other shards edit fixture bytes and apply scoped writes to calendar components.
//! Each shard runs as a separate test under the nextest time limit.
//! Exhaustive shards partition indices at each length.
//! Randomized shards can draw the same inputs.
//!
//! Scoped writes check serialization stability, component nesting, and the expected property count.
//! Changes to unrelated property bytes can pass those checks.
//! The sweep collects reread diagnostics without checking their contents.
//!
//! Each sweep prints its coverage.
//! The `break_sweeps.rs` tests require at least 1,900,000 exhaustive inputs, 2,200,000 randomized documents, and 135,000 generative mutations.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use icalkit_conformance::internal::core::{
    Component, Diagnostic, DiagnosticCode, Document, GrammarLimits, Item, Limits, MutationError,
    ParameterEdit, ParseError, PropertyId, ProposedChange, RawText, TextValue,
};

/// A fixed seed makes randomized failures reproducible.
const SEED: u64 = 0x1CA1_C017_0BEE_F001;

/// Structural octets from RFC 5545 sections 3.1 and 3.2 plus an octet outside ASCII.
const ALPHABET: &[u8] = b":;,\"\\^\r\n \tA\xE9";

/// Longest exhaustive input in octets.
/// Six octets can include a fold with surrounding header and value data.
/// The twelve-octet alphabet yields 3,257,437 strings of up to six octets under each policy.
const EXHAUSTIVE_LENGTH: usize = 6;

/// Shards partition the indices at every input length.
const EXHAUSTIVE_SHARDS: usize = 12;

/// Low resource limits let short inputs exercise the refusal path.
const TIGHT: Limits = Limits::DEFAULT
    .with_grammar(
        GrammarLimits::DEFAULT
            .with_max_header_bytes(2)
            .with_max_parameters(1)
            .with_max_folds_per_line(1),
    )
    .with_max_input_bytes(4)
    .with_max_value_bytes(1)
    .with_max_items(2)
    .with_max_component_depth(1);

/// A generated calendar can fit or exceed these limits.
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

/// How many calendars the randomized sweep draws, across every shard of it.
const CALENDARS: usize = 1_100_000;

/// The seed and shard index determine each randomized sequence.
const RANDOMIZED_SHARDS: usize = 16;

/// Number of edits per committed fixture.
const EDITS_PER_FIXTURE: usize = 480;

/// The same, for a fixture large enough that this is the expensive sweep.
const EDITS_PER_LARGE_FIXTURE: usize = 12;

/// How many tests the generative and write legs are each cut into.
const FIXTURE_SHARDS: usize = 4;

/// Number of scoped writes per kind and fixture.
const WRITES_PER_FIXTURE: usize = 24;

/// The same, for a fixture large enough that a write to it costs a copy of the whole tree.
const WRITES_PER_LARGE_FIXTURE: usize = 2;

/// The sweep uses fewer edits for larger fixtures to bound copying costs.
const LARGE_FIXTURE: usize = 8192;

/// How much of a failing input an assertion message carries before it stops being readable.
const RENDER_LIMIT: usize = 160;

/// What one input turned out to be, which is the only two answers this sweep accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Read, written back octet for octet, and a second pass agreed with the first.
    Preserved {
        /// Whether the reader had anything to say about it.
        diagnosed: bool,
    },
    /// Refused with a limit whose necessary condition holds for the input.
    Refused(ParseError),
}

#[test]
fn a_decoded_zone_identifier_can_exceed_the_wire_length_budget() {
    let escaped = b"DTSTART;TZID=Test/^^:20240102T090000\r\n";
    let plain = b"DTSTART;TZID=Test/AA:20240102T090000\r\n";
    let limit = as_units(escaped.len());
    let limits = Limits::GENEROUS.with_max_input_bytes(limit);
    assert_eq!(
        examine(escaped, limits).unwrap(),
        Verdict::Refused(ParseError::InputTooLarge { limit })
    );
    assert!(matches!(
        examine(plain, limits).unwrap(),
        Verdict::Preserved { .. }
    ));
    assert!(!crossed(plain, ParseError::InputTooLarge { limit }));
}

/// Put one input through the three claims this whole file exists to check.
///
/// [`crossed`] checks whether the input could account for the reported refusal.
/// Accepted input must match the first serialization and the output of the second pass.
fn examine(octets: &[u8], limits: Limits) -> Result<Verdict, String> {
    let mut kept: Vec<Diagnostic> = Vec::new();
    let tree = match Document::parse(octets, limits, &mut kept) {
        Ok(read) => read,
        Err(refusal) => {
            if crossed(octets, refusal) {
                return Ok(Verdict::Refused(refusal));
            }
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
    let mut again: Vec<Diagnostic> = Vec::new();
    match Document::parse(&written, limits, &mut again) {
        Ok(reread) if reread.to_bytes() == written => Ok(Verdict::Preserved {
            diagnosed: !kept.is_empty(),
        }),
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

/// Whether a reported refusal is plausible from counts over the whole input.
///
/// Every branch checks a necessary condition independently of the parser's field boundaries and nesting state.
fn crossed(octets: &[u8], refusal: ParseError) -> bool {
    let held = as_units(octets.len());
    match refusal {
        ParseError::Exhausted => false,
        // An escaped `TZID` needs space for both its written and decoded values.
        ParseError::InputTooLarge { limit } => {
            held.saturating_add(zone_storage_bound(octets)) > limit
        },
        // A value and a header are both runs of octets taken out of the input.
        ParseError::ValueTooLarge { limit } | ParseError::HeaderTooLarge { limit } => {
            held > u64::from(limit)
        },
        // Every parameter starts with a semicolon.
        ParseError::TooManyParameters { limit } => count_octet(octets, b';') > u64::from(limit),
        // A fold is a terminator followed by `SP` or `HTAB`, and this counts exactly those.
        ParseError::TooManyFolds { limit } => continuations(octets) > u64::from(limit),
        // Each property or component occupies at least one segment.
        ParseError::TooManyItems { limit } => segments(octets) > u64::from(limit),
        // Unfold before counting `BEGIN` because a continuation can split the component marker.
        ParseError::TooDeep { limit } => {
            keyword_count(&unfolded(octets), b"BEGIN") > u64::from(limit)
        },
    }
}

fn zone_storage_bound(octets: &[u8]) -> u64 {
    if !octets.contains(&b'^') {
        return 0;
    }
    unfolded(octets)
        .split(|byte| matches!(byte, b'\r' | b'\n'))
        .map(|line| {
            let mut parameters = line.splitn(2, |byte| matches!(byte, b';' | b':'));
            let name = parameters.next().unwrap_or_default();
            let mut remaining = parameters.next().unwrap_or_default();
            let mut total = 0_u64;
            if line.get(name.len()) != Some(&b';') {
                return total;
            }
            while !remaining.is_empty() {
                let mut quoted = false;
                let mut in_value = false;
                let mut value_start = false;
                let end = remaining
                    .iter()
                    .position(|&byte| {
                        if quoted {
                            quoted = byte != b'"';
                        } else if matches!(byte, b';' | b':') {
                            return true;
                        } else if !in_value && byte == b'=' {
                            in_value = true;
                            value_start = true;
                        } else {
                            quoted = value_start && byte == b'"';
                            value_start = in_value && byte == b',';
                        }
                        false
                    })
                    .unwrap_or(remaining.len());
                let field = &remaining[..end];
                if let Some((name, raw)) = field.split_at_checked(5)
                    && name.eq_ignore_ascii_case(b"TZID=")
                    && raw
                        .windows(2)
                        .any(|pair| matches!(pair, [b'^', b'n' | b'^' | b'\'']))
                {
                    total = total.saturating_add(as_units(raw.len()));
                }
                if remaining.get(end) != Some(&b';') {
                    break;
                }
                remaining = &remaining[end.saturating_add(1)..];
            }
            total
        })
        .fold(0, u64::saturating_add)
}

/// Convert an input count to charge units, saturating at the largest representable charge.
fn as_units(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Count occurrences of `wanted` in the input.
fn count_octet(octets: &[u8], wanted: u8) -> u64 {
    octets.iter().fold(0, |seen, held| {
        if *held == wanted {
            seen.saturating_add(1)
        } else {
            seen
        }
    })
}

/// Count case-insensitive occurrences of `keyword`.
/// Matches inside a line give a conservative upper bound on nesting.
fn keyword_count(octets: &[u8], keyword: &[u8]) -> u64 {
    as_units(
        octets
            .windows(keyword.len())
            .filter(|window| window.eq_ignore_ascii_case(keyword))
            .count(),
    )
}

/// Remove folds as RFC 5545 section 3.1 specifies.
/// A terminator followed by `SP` or `HTAB` can split a name across physical lines.
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

/// Return the width of one terminator, counting `CRLF` as a single terminator.
fn terminator_width(octets: &[u8], at: usize) -> usize {
    match octets.get(at) {
        Some(&b'\r') => {
            if octets.get(at.saturating_add(1)) == Some(&b'\n') {
                2
            } else {
                1
            }
        },
        Some(&b'\n') => 1,
        _ => 0,
    }
}

/// Count segments, including an empty final segment, to bound the number of items.
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

/// Render a bounded, reproducible byte sample for a failing assertion.
fn render(octets: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &held in octets.iter().take(RENDER_LIMIT) {
        match held {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
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

/// A seeded `splitmix64` stream provides reproducible draws.
#[derive(Debug)]
struct Stream {
    /// The whole state, advanced once per draw.
    state: u64,
}

impl Stream {
    /// A stream at `seed`, which is where every sweep in this file starts.
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
        // A 32-bit draw gives the same sequence on 32-bit and 64-bit targets.
        let drawn = usize::try_from(self.draw() & 0xFFFF_FFFF).unwrap_or(0);
        drawn.checked_rem(bound).unwrap_or(0)
    }

    /// One of `choices`, and nothing when there are none.
    fn pick<'a, T>(&mut self, choices: &'a [T]) -> Option<&'a T> {
        choices.get(self.below(choices.len()))
    }
}

/// Names a generated line may carry, including the two that are component boundaries and the empty one that makes a blank line.
const NAMES: &[&[u8]] = &[
    b"SUMMARY",
    b"DTSTART",
    b"UID",
    b"X-VENDOR",
    b"BEGIN",
    b"END",
    b"",
];

/// Raw parameters include quoted delimiters, an unclosed quote, a missing value separator, and an empty name.
const PARAMETERS: &[&[u8]] = &[
    b"",
    b";TZID=Etc/UTC",
    b";X-Q=\"a;b,c:d\"",
    b";CN=\"never closed",
    b";BARE",
    b";=empty",
];

/// Values, chosen for what a reader has to do with them rather than for what they mean.
const VALUES: &[&[u8]] = &[
    b"",
    b"VEVENT",
    b"vevent",
    b"20260810T120000Z",
    b"a\\,b\\;c\\nd",
    b"^'quoted^'",
    b"^x",
    b"\xE9\xE9\xE9",
    b"has a space",
];

/// Two `CRLF` entries give the standard terminator extra weight.
const TERMINATORS: &[&[u8]] = &[b"\r\n", b"\r\n", b"\n", b"\r", b""];

/// The ways a producer may introduce a continuation line.
const FOLDS: &[&[u8]] = &[b"\r\n ", b"\r\n\t", b"\n ", b"\r\t"];

/// Append what `chosen` holds, and nothing where it holds nothing.
fn extend(out: &mut Vec<u8>, chosen: Option<&&[u8]>) {
    if let Some(fragment) = chosen {
        out.extend_from_slice(fragment);
    }
}

/// Insert a fold at an arbitrary octet, including inside a name, quoted value, or character.
fn append_line(stream: &mut Stream, out: &mut Vec<u8>) {
    let mut assembled: Vec<u8> = Vec::new();
    extend(&mut assembled, stream.pick(NAMES));
    let runs = stream.below(3);
    for _ in 0..runs {
        extend(&mut assembled, stream.pick(PARAMETERS));
    }
    // One draw in eight omits the value separator.
    if stream.below(8) > 0 {
        assembled.push(b':');
    }
    extend(&mut assembled, stream.pick(VALUES));

    if stream.below(3) == 0 {
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

/// Generate nested component boundaries and content lines, including incomplete calendars.
fn calendar(stream: &mut Stream) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let wrapped = stream.below(4);
    for _ in 0..wrapped {
        out.extend_from_slice(b"BEGIN:VCALENDAR\r\n");
    }
    let lines = stream.below(10);
    for _ in 0..lines {
        append_line(stream, &mut out);
    }
    let closed = stream.below(wrapped.saturating_add(1));
    for _ in 0..closed {
        out.extend_from_slice(b"END:VCALENDAR\r\n");
    }
    out
}

/// A byte-level edit to a committed calendar fixture.
#[derive(Clone, Copy, Debug)]
enum Edit {
    /// Replace one octet with a structural octet.
    Substitute,
    /// Remove one octet, which is what a fold looks like after a careless unfolder.
    Delete,
    /// Insert one of the structural octets where the producer wrote none.
    Insert,
    /// Repeat a run, which is how a retried transfer duplicates a block.
    Repeat,
    /// Cut the file short, which is every interrupted download.
    Truncate,
}

/// The fixed edit order makes a failing case reproducible.
const EDITS: &[Edit] = &[
    Edit::Substitute,
    Edit::Delete,
    Edit::Insert,
    Edit::Repeat,
    Edit::Truncate,
];

/// Apply `edit` to `original` at a place the stream chooses.
fn mutate(stream: &mut Stream, original: &[u8], edit: Edit) -> Vec<u8> {
    let mut octets = original.to_vec();
    if octets.is_empty() {
        return octets;
    }
    let at = stream.below(octets.len());
    let drawn = stream.pick(ALPHABET).copied().unwrap_or(b'A');
    match edit {
        Edit::Substitute => {
            if let Some(slot) = octets.get_mut(at) {
                *slot = drawn;
            }
        },
        Edit::Delete => {
            octets.remove(at);
        },
        Edit::Insert => octets.insert(at, drawn),
        Edit::Repeat => {
            let run = stream.below(64).min(octets.len().saturating_sub(at));
            let end = at.saturating_add(run);
            let mut grown: Vec<u8> = Vec::new();
            grown.extend_from_slice(octets.get(..end).unwrap_or_default());
            grown.extend_from_slice(octets.get(at..end).unwrap_or_default());
            grown.extend_from_slice(octets.get(end..).unwrap_or_default());
            octets = grown;
        },
        Edit::Truncate => octets.truncate(at),
    }
    octets
}

/// What a sweep covered, so a budget that shrank is visible rather than silent.
#[derive(Debug, Default)]
struct Tally {
    /// Inputs put through [`examine`].
    examined: u64,
    /// Inputs refused with a limit whose necessary condition held.
    refused: u64,
    /// Inputs kept, with something reported about them.
    diagnosed: u64,
}

impl Tally {
    /// Record one verdict.
    fn record(&mut self, verdict: Verdict) {
        self.examined = self.examined.saturating_add(1);
        match verdict {
            Verdict::Preserved { diagnosed } => {
                if diagnosed {
                    self.diagnosed = self.diagnosed.saturating_add(1);
                }
            },
            Verdict::Refused(_) => {
                self.refused = self.refused.saturating_add(1);
            },
        }
    }

    /// One line naming what `what` covered.
    fn summary(&self, what: &str) -> String {
        format!(
            "{what}: {} inputs examined, {} refused at a bound, {} kept with a diagnostic",
            self.examined, self.refused, self.diagnosed
        )
    }
}

/// Count strings in base twelve so length and index identify each case.
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

/// Read all committed `.ics` fixtures in stable path order.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("tests");
    root.push("fixtures");
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    collect_fixtures(&root, &mut found);
    found.sort();
    found
}

/// Collect readable `.ics` files recursively.
/// The corpus check detects an empty directory.
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

/// Put one fixture through every edit, recording what each one did.
fn sweep_fixture(
    stream: &mut Stream,
    name: &str,
    original: &[u8],
    tally: &mut Tally,
) -> Result<(), String> {
    let rounds = if original.len() > LARGE_FIXTURE {
        EDITS_PER_LARGE_FIXTURE
    } else {
        EDITS_PER_FIXTURE
    };
    for pass in 0..rounds {
        for edit in EDITS {
            let octets = mutate(stream, original, *edit);
            let verdict = examine(&octets, Limits::DEFAULT)
                .map_err(|report| format!("{name}, pass {pass}, {edit:?}: {report}"))?;
            tally.record(verdict);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Scoped writes over the fixture corpus
// ---------------------------------------------------------------------------------------

/// One scoped write, of each kind `ical-core`'s change vocabulary has.
#[derive(Clone, Copy, Debug)]
enum Write {
    /// A value written through the guard, which is the narrowest door there is.
    Value,
    /// A whole content line written over the identity the change names.
    Replace,
    /// A parameter assigned, with the value's own text untouched.
    Parameters,
    /// A line added among the properties.
    Add,
    /// Every occurrence of an identity removed.
    Remove,
}

/// The fixed write order makes a failing case reproducible.
const WRITES: &[Write] = &[
    Write::Value,
    Write::Replace,
    Write::Parameters,
    Write::Add,
    Write::Remove,
];

/// The values a write draws from: an ordinary one, an empty one, and one section 3.2 quotes.
const WRITTEN_VALUES: &[&[u8]] = &[b"written", b"", b"a:b;c,d", b"^caret", b"\xe9\xe9"];

/// What the write leg covered.
#[derive(Debug, Default)]
struct WriteTally {
    /// Writes the door accepted and this leg then checked.
    applied: u64,
    /// Writes the door refused, which is an answer and not a failure.
    refused: u64,
    /// Draws where the chosen component held nothing to write to.
    absent: u64,
}

/// Borrow a top-level component or one immediate child.
/// The fixture corpus includes nesting too deep for recursive selection.
fn component_at(
    document: &mut Document,
    outer: usize,
    inner: Option<usize>,
) -> Option<&mut Component> {
    let component = document.components_mut().nth(outer)?;
    match inner {
        Some(index) => component.components_mut().nth(index),
        None => Some(component),
    }
}

/// The nesting a document states, as `(depth, name)` in the order it serializes.
///
/// Walked on an explicit stack, for the reason every other traversal in this workspace uses one.
/// This is the shape a second client reads, and no scoped write may change it.
fn nesting(document: &Document) -> Vec<(usize, Vec<u8>)> {
    let mut out: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut pending: Vec<(usize, &Item)> = document
        .items()
        .iter()
        .map(|item| (0, item))
        .rev()
        .collect();
    while let Some((depth, item)) = pending.pop() {
        let Item::Component(component) = item else {
            continue;
        };
        out.push((depth, component.name().as_bytes().to_vec()));
        let nested = component
            .items()
            .iter()
            .map(|held| (depth.saturating_add(1), held));
        pending.extend(nested.collect::<Vec<_>>().into_iter().rev());
    }
    out
}

/// How many properties a document holds, at any depth.
fn property_count(document: &Document) -> usize {
    let mut counted = 0_usize;
    let mut pending: Vec<&Item> = document.items().iter().rev().collect();
    while let Some(item) = pending.pop() {
        match item {
            Item::Property(_) => counted = counted.saturating_add(1),
            Item::Component(component) => pending.extend(component.items().iter().rev()),
        }
    }
    counted
}

/// Apply a write and return the expected change in property count.
/// A refusal or missing target returns `None`.
fn apply_write(
    stream: &mut Stream,
    component: &mut Component,
    write: Write,
    identity: &PropertyId,
    occurrences: usize,
) -> Option<isize> {
    let value = stream.pick(WRITTEN_VALUES).copied().unwrap_or(b"written");
    let outcome = match write {
        Write::Value => component
            .get_mut::<TextValue<'_>>(identity)
            .map_or(Err(MutationError::Absent), |mut guard| guard.set_raw(value))
            .map(|()| 0),
        Write::Replace => {
            let mut line = identity.as_bytes().to_vec();
            line.push(b':');
            line.extend_from_slice(value);
            component
                .apply(
                    identity,
                    &ProposedChange::Replace(RawText::from_vec(line)),
                    Limits::DEFAULT,
                )
                .map(|()| 0)
        },
        Write::Parameters => component
            .apply(
                identity,
                &ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", value)]),
                Limits::DEFAULT,
            )
            .map(|()| 0),
        Write::Add => {
            let mut line = Vec::from(&b"X-ADDED:"[..]);
            line.extend_from_slice(value);
            line.extend_from_slice(b"\r\n");
            component
                .apply(
                    &PropertyId::from_name(b"X-ADDED"),
                    &ProposedChange::Add(RawText::from_vec(line)),
                    Limits::DEFAULT,
                )
                .map(|()| 1)
        },
        Write::Remove => component
            .apply(identity, &ProposedChange::Remove, Limits::DEFAULT)
            .map(|()| isize::try_from(occurrences).unwrap_or(0).saturating_neg()),
    };
    outcome.ok()
}

/// Apply every scoped write to the fixture and check the resulting document.
fn sweep_writes(
    stream: &mut Stream,
    name: &str,
    original: &[u8],
    tally: &mut WriteTally,
) -> Result<(), String> {
    let rounds = if original.len() > LARGE_FIXTURE {
        WRITES_PER_LARGE_FIXTURE
    } else {
        WRITES_PER_FIXTURE
    };
    for pass in 0..rounds {
        for write in WRITES {
            sweep_one_write(stream, original, *write, tally)
                .map_err(|report| format!("{name}, pass {pass}, {write:?}: {report}"))?;
        }
    }
    Ok(())
}

/// Check serialization, nesting, and property count after a successful scoped write.
fn sweep_one_write(
    stream: &mut Stream,
    original: &[u8],
    write: Write,
    tally: &mut WriteTally,
) -> Result<(), String> {
    // The generative sweep checks refusals for the original fixtures.
    let Ok(mut document) = Document::parse(original, Limits::DEFAULT, &mut Vec::new()) else {
        tally.absent = tally.absent.saturating_add(1);
        return Ok(());
    };
    let shape = nesting(&document);
    let held = property_count(&document);
    let outer = stream.below(document.components().count());
    let nested = document
        .components()
        .nth(outer)
        .map_or(0, |component| component.components().count());
    let inner = (nested > 0 && stream.below(2) == 0).then(|| stream.below(nested));
    let Some(component) = component_at(&mut document, outer, inner) else {
        tally.absent = tally.absent.saturating_add(1);
        return Ok(());
    };
    let Some(identity) = component
        .items()
        .iter()
        .filter_map(Item::as_property)
        .map(|property| PropertyId::from_name(property.name().as_bytes()))
        .next()
    else {
        tally.absent = tally.absent.saturating_add(1);
        return Ok(());
    };
    let occurrences = component
        .items()
        .iter()
        .filter_map(Item::as_property)
        .filter(|property| PropertyId::from_name(property.name().as_bytes()) == identity)
        .count();
    let Some(expected) = apply_write(stream, component, write, &identity, occurrences) else {
        tally.refused = tally.refused.saturating_add(1);
        return Ok(());
    };
    tally.applied = tally.applied.saturating_add(1);

    let written = document.to_bytes();
    // P1 and P2 over what the write produced, through the same examination every other leg uses: the file this crate wrote reads back as itself and is a fixed point.
    examine(&written, Limits::DEFAULT)?;
    let Ok(reread) = Document::parse(&written, Limits::DEFAULT, &mut Vec::new()) else {
        return Err(format!(
            "what the write produced is unreadable: {}",
            render(&written)
        ));
    };
    if nesting(&reread) != shape {
        return Err(format!(
            "a write to {:?} restructured the document: {}",
            identity.as_bytes(),
            render(&written)
        ));
    }
    let after = isize::try_from(property_count(&reread)).unwrap_or(0);
    let before = isize::try_from(held).unwrap_or(0);
    if after != before.saturating_add(expected) {
        return Err(format!(
            "a write to {:?} took the document from {before} properties to {after} rather than \
             {}: {}",
            identity.as_bytes(),
            before.saturating_add(expected),
            render(&written)
        ));
    }
    Ok(())
}

/// What one anchor row claims about its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    /// The input comes back out unchanged with nothing reported about it.
    Clean,
    /// The input comes back out unchanged and earns at least one diagnostic on the way.
    Diagnosed,
    /// The reader reports the expected limit.
    Refused(ParseError),
}

/// Handwritten examples check terminators, blank lines, missing separators, and resource limits.
const ANCHORS: &[(&str, &[u8], Limits, Anchor)] = &[
    ("the empty input", b"", Limits::DEFAULT, Anchor::Clean),
    // Section 3.8.8 permits the extension property in this clean fixture.
    (
        "a line terminated the way section 3.1 asks",
        b"X-TEST:v\r\n",
        Limits::DEFAULT,
        Anchor::Clean,
    ),
    (
        "a bare line feed",
        b"X:v\n",
        Limits::DEFAULT,
        Anchor::Diagnosed,
    ),
    (
        "a line carrying no separator",
        b"X\r\n",
        Limits::DEFAULT,
        Anchor::Diagnosed,
    ),
    ("a blank line", b"\r\n", Limits::DEFAULT, Anchor::Diagnosed),
    (
        "a last line with no terminator",
        b"X:v",
        Limits::DEFAULT,
        Anchor::Diagnosed,
    ),
    (
        "a header exactly at the ceiling",
        b"AA",
        TIGHT,
        Anchor::Diagnosed,
    ),
    (
        "one octet past the header ceiling",
        b"AAA",
        TIGHT,
        Anchor::Refused(ParseError::HeaderTooLarge { limit: 2 }),
    ),
    (
        "one octet past the value ceiling",
        b":AA",
        TIGHT,
        Anchor::Refused(ParseError::ValueTooLarge { limit: 1 }),
    ),
    (
        "one parameter past the ceiling",
        b";;",
        TIGHT,
        Anchor::Refused(ParseError::TooManyParameters { limit: 1 }),
    ),
    (
        "one fold past the ceiling",
        b"\n \n ",
        TIGHT,
        Anchor::Refused(ParseError::TooManyFolds { limit: 1 }),
    ),
    (
        "one item past the ceiling",
        b"\n\n\n",
        TIGHT,
        Anchor::Refused(ParseError::TooManyItems { limit: 2 }),
    ),
];

#[test]
fn the_anchors_a_generated_corpus_cannot_supply() {
    for (name, octets, limits, expected) in ANCHORS {
        let verdict = examine(octets, *limits).unwrap_or_else(|report| panic!("{name}: {report}"));
        match (*expected, verdict) {
            (Anchor::Clean, Verdict::Preserved { diagnosed }) => {
                assert!(!diagnosed, "{name} was diagnosed and is not supposed to be");
            },
            (Anchor::Diagnosed, Verdict::Preserved { diagnosed }) => {
                assert!(diagnosed, "{name} earned no diagnostic at all");
            },
            (Anchor::Refused(bound), Verdict::Refused(stated)) => {
                assert_eq!(stated, bound, "{name} refused at the wrong bound");
            },
            (want, got) => panic!("{name}: expected {want:?} and got {got:?}"),
        }
    }
    println!("anchors: {} rows", ANCHORS.len());
}

/// Check diagnostic identities for malformed input that the reader preserves.
#[test]
fn a_violation_arrives_as_a_diagnostic_and_not_as_a_refusal() {
    let input: &[u8] = b"X:v\nY\r\n";
    let mut kept: Vec<Diagnostic> = Vec::new();
    let written = Document::parse(input, Limits::DEFAULT, &mut kept)
        .map(|tree| tree.to_bytes())
        .expect("a violation is not a refusal");
    assert_eq!(written.as_slice(), input, "the violation was repaired");
    let codes: Vec<DiagnosticCode> = kept.iter().map(|held| held.code()).collect();
    assert!(codes.contains(&DiagnosticCode::BareLineFeed), "{codes:?}");
    assert!(
        codes.contains(&DiagnosticCode::MissingValueSeparator),
        "{codes:?}"
    );
}

/// The stream one shard of a randomized leg draws from.
///
/// A shard's index and the committed seed reproduce its stream, including any inputs that also occur in other shards.
fn shard_stream(shard: usize) -> Stream {
    let offset = as_units(shard).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    Stream::new(SEED.wrapping_add(offset))
}

/// Check every string of up to [`EXHAUSTIVE_LENGTH`] octets over [`ALPHABET`] under both policies.
fn exhaustive_shard(shard: usize) -> Result<(), String> {
    let mut tally = Tally::default();
    let mut octets: Vec<u8> = Vec::new();
    for length in 0..=EXHAUSTIVE_LENGTH {
        let mut index = shard;
        while index < population(length) {
            nth_input(length, index, &mut octets);
            for policy in [Limits::DEFAULT, TIGHT] {
                match examine(&octets, policy) {
                    Ok(verdict) => tally.record(verdict),
                    Err(report) => {
                        return Err(format!("length {length}, index {index}: {report}"));
                    },
                }
            }
            index = index.saturating_add(EXHAUSTIVE_SHARDS);
        }
    }
    println!(
        "{} (alphabet {} octets, lengths 0..={EXHAUSTIVE_LENGTH}, shard {shard} of \
         {EXHAUSTIVE_SHARDS}, two policies)",
        tally.summary("exhaustive short inputs"),
        ALPHABET.len(),
    );
    assert!(
        tally.refused > 0,
        "no short input crossed a bound, so the tight policy stopped binding"
    );
    assert!(
        tally.diagnosed > 0,
        "no short input was diagnosed, so the recovery paths were never reached"
    );
    Ok(())
}

/// Read seeded calendars under generous and tight limits.
/// Check byte identity or the reason for refusal.
fn randomized_shard(shard: usize) -> Result<(), String> {
    let mut stream = shard_stream(shard);
    let mut tally = Tally::default();
    let drawn = CALENDARS.checked_div(RANDOMIZED_SHARDS).unwrap_or(0);
    for index in 0..drawn {
        let octets = calendar(&mut stream);
        for policy in [Limits::DEFAULT, BOUNDED] {
            match examine(&octets, policy) {
                Ok(verdict) => tally.record(verdict),
                Err(report) => return Err(format!("calendar {index} of shard {shard}: {report}")),
            }
        }
    }
    println!(
        "{} (seed {SEED:#x}, shard {shard} of {RANDOMIZED_SHARDS}, {drawn} calendars, two \
         policies)",
        tally.summary("randomized calendars")
    );
    assert!(
        tally.refused > 0,
        "no generated calendar crossed a bound, so the bounded policy stopped binding"
    );
    assert!(
        tally.diagnosed > 0,
        "no generated calendar was diagnosed, so the generator stopped generating violations"
    );
    Ok(())
}

/// Assign each fixture to a shard by its position in the sorted corpus.
fn shard_of_corpus(shard: usize) -> Vec<(String, Vec<u8>)> {
    fixtures()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index.checked_rem(FIXTURE_SHARDS) == Some(shard))
        .map(|(_, held)| held)
        .collect()
}

/// Check byte-level edits to committed calendars.
/// Check the original fixture before any edits.
fn generative_shard(shard: usize) -> Result<(), String> {
    let corpus = shard_of_corpus(shard);
    assert!(
        corpus.len() >= 2,
        "only {} fixtures reached shard {shard}, so the corpus directory moved",
        corpus.len()
    );
    let mut stream = shard_stream(shard);
    let mut tally = Tally::default();
    for (name, original) in &corpus {
        examine(original, Limits::DEFAULT)
            .map_err(|report| format!("{name} does not hold on its own: {report}"))?;
        sweep_fixture(&mut stream, name, original, &mut tally)
            .map_err(|report| format!("seed {SEED:#x}: {report}"))?;
    }
    println!(
        "{} ({} fixtures, shard {shard} of {FIXTURE_SHARDS})",
        tally.summary("edits to committed fixtures"),
        corpus.len()
    );
    assert!(
        tally.diagnosed > 0,
        "no edited fixture was diagnosed, so the edits stopped reaching the recovery paths"
    );
    Ok(())
}

/// Apply scoped writes to each fixture and check serialization, nesting, and property count.
fn write_shard(shard: usize) -> Result<(), String> {
    let corpus = shard_of_corpus(shard);
    assert!(
        corpus.len() >= 2,
        "only {} fixtures reached shard {shard}, so the corpus directory moved",
        corpus.len()
    );
    let mut stream = shard_stream(shard);
    let mut tally = WriteTally::default();
    for (name, original) in &corpus {
        sweep_writes(&mut stream, name, original, &mut tally)
            .map_err(|report| format!("seed {SEED:#x}: {report}"))?;
    }
    println!(
        "scoped writes: {} applied, {} refused, {} left nothing to write ({} fixtures, shard \
         {shard} of {FIXTURE_SHARDS})",
        tally.applied,
        tally.refused,
        tally.absent,
        corpus.len()
    );
    assert!(
        tally.applied > 0,
        "no write was applied at all, so nothing here checked a write"
    );
    Ok(())
}

/// Declare a separate test so nextest applies its time limit to each shard.
macro_rules! shard {
    ($name:ident, $leg:ident, $index:expr) => {
        #[doc = concat!("Shard ", stringify!($index), " of `", stringify!($leg), "`.")]
        #[test]
        fn $name() {
            if let Err(report) = $leg($index) {
                panic!("{report}");
            }
        }
    };
}

shard!(
    every_short_input_over_the_octets_that_decide_a_line,
    exhaustive_shard,
    0
);
shard!(every_short_input_shard_1, exhaustive_shard, 1);
shard!(every_short_input_shard_2, exhaustive_shard, 2);
shard!(every_short_input_shard_3, exhaustive_shard, 3);
shard!(every_short_input_shard_4, exhaustive_shard, 4);
shard!(every_short_input_shard_5, exhaustive_shard, 5);
shard!(every_short_input_shard_6, exhaustive_shard, 6);
shard!(every_short_input_shard_7, exhaustive_shard, 7);
shard!(every_short_input_shard_8, exhaustive_shard, 8);
shard!(every_short_input_shard_9, exhaustive_shard, 9);
shard!(every_short_input_shard_10, exhaustive_shard, 10);
shard!(every_short_input_shard_11, exhaustive_shard, 11);

shard!(
    randomized_calendars_from_a_committed_seed,
    randomized_shard,
    0
);
shard!(randomized_calendars_shard_1, randomized_shard, 1);
shard!(randomized_calendars_shard_2, randomized_shard, 2);
shard!(randomized_calendars_shard_3, randomized_shard, 3);
shard!(randomized_calendars_shard_4, randomized_shard, 4);
shard!(randomized_calendars_shard_5, randomized_shard, 5);
shard!(randomized_calendars_shard_6, randomized_shard, 6);
shard!(randomized_calendars_shard_7, randomized_shard, 7);
shard!(randomized_calendars_shard_8, randomized_shard, 8);
shard!(randomized_calendars_shard_9, randomized_shard, 9);
shard!(randomized_calendars_shard_10, randomized_shard, 10);
shard!(randomized_calendars_shard_11, randomized_shard, 11);
shard!(randomized_calendars_shard_12, randomized_shard, 12);
shard!(randomized_calendars_shard_13, randomized_shard, 13);
shard!(randomized_calendars_shard_14, randomized_shard, 14);
shard!(randomized_calendars_shard_15, randomized_shard, 15);

shard!(edits_to_every_committed_fixture, generative_shard, 0);
shard!(edits_to_committed_fixtures_shard_1, generative_shard, 1);
shard!(edits_to_committed_fixtures_shard_2, generative_shard, 2);
shard!(edits_to_committed_fixtures_shard_3, generative_shard, 3);

shard!(scoped_writes_into_every_committed_fixture, write_shard, 0);
shard!(scoped_writes_shard_1, write_shard, 1);
shard!(scoped_writes_shard_2, write_shard, 2);
shard!(scoped_writes_shard_3, write_shard, 3);
