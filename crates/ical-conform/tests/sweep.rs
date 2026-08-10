// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The evidence M0-alpha reported and never committed, as a sweep that can be rerun.
//!
//! The round-trip claim of `docs/adr/0001` is stated over every input; the corpus beside this
//! file states it over the inputs somebody thought to commit. Those are different claims, and
//! the gap between them is where a reader keeps the bug nobody wrote a fixture for: a bare
//! `CR` immediately before a `DQUOTE`, a `\` at the last octet of a value, a fold introduced
//! between the two octets of one codepoint. This file closes the gap the only way a test can,
//! by covering the inputs nobody would have chosen.
//!
//! Four sweeps, one set of claims. Every input, wherever it came from, has to satisfy the
//! same three. **Parse then serialize is byte-identical, and stays byte-identical on a second
//! pass.** **Nothing panics or aborts.** **A refusal is a [`ParseError`] naming a bound the
//! octets are independently confirmed to have crossed.** The third is what makes the first two
//! mean anything: a reader can satisfy byte identity by refusing every input it finds
//! difficult, so a refusal is accepted here only when the input alone — counted by [`crossed`],
//! which never asks the reader anything — shows that what the bound governs was really there.
//!
//! The inputs come from four places. Exhaustively, every string of at most six octets over the
//! twelve octets that decide how RFC 5545 section 3.1 reads a line. Randomly, calendars drawn
//! from a hand-rolled generator whose seed is a constant in this file rather than a clock or an
//! environment variable, because a sweep nobody else can reproduce reports nothing.
//! Generatively, octet-level edits to the calendars real clients exported that are already
//! committed under `tests/fixtures`, which is the only material here whose shape nobody chose.
//! And through the mutation door, one scoped write of each kind the change vocabulary has,
//! applied to those same fixtures.
//!
//! **Why the fourth exists.** `docs/adr/0001` states four claims and the first three sweeps
//! reach two of them: everything above is a parse and a serialize, and P3 — a write reaches the
//! property it named and nothing else — has no parse-only expression at all. Three of the six
//! breaks this milestone was measured against lived behind that door, and reintroducing two of
//! them left every sweep here passing. So the write leg is not extra coverage of the same
//! claim; it is the other half of the evidence, and P4 rides along with it, since what a write
//! is checked against includes the diagnostics the reread earns.
//!
//! **Why the legs are cut into shards.** Every shard is one `#[test]`, so each is one process
//! with its own share of the nextest time bound, and a sweep wide enough to be evidence does
//! not become a sweep that trips a timeout on the slowest machine that runs it. The shards of
//! one leg partition its inputs; nothing is examined twice and nothing is skipped.
//!
//! The generator is hand-rolled for the same reason the crates below this one carry no
//! dependency: `just purity` reads dev-dependencies too and `cargo deny` refuses a second copy
//! of anything, so the fact that this crate is `std` and outside the gate does not make a
//! generator crate reachable from it. Sixty lines of `splitmix64` and a fragment table cost
//! less than the argument would.
//!
//! Every sweep prints what it covered. A budget that quietly shrinks — a length lowered, a
//! draw count reduced, a fixture directory that moved and was silently found empty — is the
//! failure mode a generative test has and a committed fixture does not, and a printed count is
//! the cheapest thing that makes it visible. The figures the whole file is sized against are
//! the ones M0-alpha reported and did not commit — 1,900,000 exhaustive inputs, 2,200,000
//! randomized documents, 135,000 generative mutations — and `break_sweeps.rs` computes what
//! the constants below actually cover and fails if it has fallen under any of them.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ical_core::{
    Component, Diagnostic, DiagnosticCode, Document, GrammarLimits, Item, Limits, MutationError,
    ParameterEdit, ParseError, PropertyId, ProposedChange, RawText, TextValue,
};

/// The seed every randomized sweep in this file starts from.
///
/// A committed constant rather than a clock or an environment variable: a sweep whose inputs
/// depend on when it ran reports a failure nobody else can reproduce, and a failure nobody can
/// reproduce is a flake rather than a finding. Changing this number changes which inputs are
/// covered, which is a deliberate act, and the counts each sweep prints are what makes it
/// visible rather than silent.
const SEED: u64 = 0x1CA1_C017_0BEE_F001;

/// The octets that decide how a content line is read, and one that is not text at all.
///
/// RFC 5545 gives every one of these a structural meaning: the value separator, the parameter
/// separator, the value list separator of section 3.2, the `DQUOTE` that lets a parameter carry
/// the other three, the escape prefix of section 3.3.11, RFC 6868's caret, the three
/// terminators section 3.1 leaves a reader to tell apart, and the two octets a fold may
/// continue with. `A` stands for every octet that means nothing in particular, and `0xE9` for
/// the CP1252 `SUMMARY` that has to survive a round trip it can never be decoded from.
const ALPHABET: &[u8] = b":;,\"\\^\r\n \tA\xE9";

/// The longest input the exhaustive sweep enumerates.
///
/// Four would be the floor rather than the answer: a fold is three octets — a terminator and
/// the whitespace that continues it — so only at four does anything sit on the far side of one,
/// and only at six can a line carry a fold and a header and a value with something after each.
/// Six over twelve octets is 3,257,437 inputs and two policies apiece, which is above the
/// evidence this sweep was landed to hold and inside the time it is given.
const EXHAUSTIVE_LENGTH: usize = 6;

/// How many tests the exhaustive leg is cut into.
///
/// The inputs are dealt out by index, so each shard gets every length in the same proportion
/// and no input is examined twice. Raising this makes each test shorter without covering less.
const EXHAUSTIVE_SHARDS: usize = 12;

/// A policy small enough that a four-octet input crosses something.
///
/// The exhaustive sweep is worth running twice only if one of the passes reaches the refusal
/// path at all, and the default policy is far too generous for four octets to trouble. Every
/// bound here is set one or two above zero so that what a short input earns names a dimension
/// [`crossed`] can confirm from the octets. The input budget is four rather than lower because
/// the ledger is charged with octets drawn from the input, so a budget below the longest input
/// would be crossed by inputs this sweep cannot distinguish from a double charge.
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

/// A policy a generated calendar crosses about as often as it does not.
///
/// Chosen so that the randomized sweep exercises both answers rather than one: under the
/// default policy a calendar of a few hundred octets crosses nothing, and under a policy this
/// tight roughly half of them cross something, which is what puts [`crossed`] under load.
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

/// How many tests the randomized leg is cut into.
///
/// Each shard runs its own stream from a seed derived from [`SEED`] and its own index, so the
/// draws are disjoint, every one of them is reproducible from a committed constant, and a
/// failure names the shard that found it.
const RANDOMIZED_SHARDS: usize = 16;

/// How many edits each committed fixture is put through.
const EDITS_PER_FIXTURE: usize = 480;

/// The same, for a fixture large enough that this is the expensive sweep.
const EDITS_PER_LARGE_FIXTURE: usize = 12;

/// How many tests the generative and write legs are each cut into.
const FIXTURE_SHARDS: usize = 4;

/// How many scoped writes of each kind each committed fixture is put through.
const WRITES_PER_FIXTURE: usize = 24;

/// The same, for a fixture large enough that a write to it costs a copy of the whole tree.
const WRITES_PER_LARGE_FIXTURE: usize = 2;

/// The size past which a fixture is swept fewer times.
///
/// The attacker's directory holds a fold bomb and a nesting bomb measured in the tens of
/// thousands, and copying one of those into a fresh buffer per edit is the only thing in this
/// file that costs real time. The cut is stated as a constant so that lowering it is a visible
/// reduction in what is covered rather than an invisible one.
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
    /// Refused, naming a bound the octets independently confirm was crossed.
    Refused(ParseError),
}

/// Put one input through the three claims this whole file exists to check.
///
/// The claims are stated together because they are not separable in practice. An
/// implementation that satisfies byte identity by refusing everything satisfies nothing, so a
/// refusal is accepted only where [`crossed`] can confirm it from the input; and an
/// implementation that writes back what it read but disagrees with itself on the next read has
/// normalized something, so the octets it wrote are read again rather than taken on trust.
///
/// The third claim — that nothing panics or aborts — is the absence of any return value below.
/// It is asserted by this function being reached a second time.
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

/// Whether the octets independently confirm the bound `refusal` names.
///
/// A reader can always make an input go away by refusing it, so "it refused" is evidence of
/// nothing on its own. Each arm below is a *necessary* condition computed from the input and
/// from nothing else: a header, a value and a parameter are all drawn from octets the caller
/// handed in, so a bound stated over them cannot be crossed by an input too small to hold what
/// crossed it. The conditions are deliberately weaker than the reader's own, which is what
/// keeps this a check on the reader rather than a second copy of it.
fn crossed(octets: &[u8], refusal: ParseError) -> bool {
    let held = as_units(octets.len());
    match refusal {
        // Every octet charged against the ledger is an octet of the input — a name, a
        // parameter, a value chunk, or the terminator and continuation whitespace of a fold —
        // and none of them is charged twice. A budget a parse crossed is therefore a budget
        // smaller than the input, and a failure here says the ledger charged for octets the
        // caller never supplied, which is a finding rather than a false alarm.
        ParseError::InputTooLarge { limit } => held > limit,
        // A value and a header are both runs of octets taken out of the input.
        ParseError::ValueTooLarge { limit } | ParseError::HeaderTooLarge { limit } => {
            held > u64::from(limit)
        },
        // Every parameter is opened by a `;`, though not every `;` opens one.
        ParseError::TooManyParameters { limit } => count_octet(octets, b';') > u64::from(limit),
        // A fold is a terminator followed by `SP` or `HTAB`, and this counts exactly those.
        ParseError::TooManyFolds { limit } => continuations(octets) > u64::from(limit),
        // Each item — property or component — occupies at least one terminated segment.
        ParseError::TooManyItems { limit } => segments(octets) > u64::from(limit),
        // Each open component was opened by a line whose name is `BEGIN`, compared the way
        // RFC 5545 section 3.1 compares a name — and counted after the folds are taken out,
        // because section 3.1 folds at octets and `BEG\r\n IN:VEVENT` opens a component while
        // carrying no `BEGIN` at all. Counting the raw octets made this arm miss those, which
        // turned a sound refusal into an accusation against the reader.
        ParseError::TooDeep { limit } => {
            keyword_count(&unfolded(octets), b"BEGIN") > u64::from(limit)
        },
    }
}

/// A count as a charge-sized number, saturating rather than wrapping.
///
/// `usize` is not `u64` on every target, and a count that does not fit is better compared as
/// the largest number there is than as a wrapped small one.
fn as_units(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// How many times `wanted` occurs in `octets`.
///
/// Folded rather than counted through `filter(..).count()`, which `clippy::naive_bytecount`
/// asks to replace with a crate this workspace does not take a dependency on.
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
///
/// An over-count, since it does not ask whether the occurrence began a line. That is the safe
/// direction: the count is used as a ceiling on how deep the reader could have nested.
fn keyword_count(octets: &[u8], keyword: &[u8]) -> u64 {
    as_units(
        octets
            .windows(keyword.len())
            .filter(|window| window.eq_ignore_ascii_case(keyword))
            .count(),
    )
}

/// The input with RFC 5545 section 3.1's folds taken out.
///
/// A terminator followed by `SP` or `HTAB` is a continuation and not a line break, so the name
/// a reader sees may be spelled across two physical lines. Every condition [`crossed`] states
/// about names has to be stated over these octets rather than over the ones on disk; the
/// counts stated about octets and terminators do not, because those are what the input holds
/// either way.
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

/// The width of the terminator at `at`, or zero where there is none.
///
/// `CRLF` is one terminator and not two, which is the whole reason this is a function: a file
/// written the way section 3.1 asks for would otherwise be counted as twice the lines it has.
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

/// How many terminated segments the input holds, counting a last one that is empty.
///
/// An over-count on purpose. This is the ceiling on how many items a reader could have built
/// out of the input, and a necessary condition wants the ceiling rather than the exact number.
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
///
/// A failing sweep reports an input nobody wrote by hand, so the message has to be the input
/// itself in a form that can be pasted straight back into a test as a fixed case. Long inputs
/// are cut, because a hundred kilobytes of fold bomb in a failure message hides the failure.
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
            // `write!` rather than `push_str(&format!(..))`: the same octets without the
            // intermediate `String` each one would otherwise allocate. Writing into a `String`
            // cannot fail, and this helper is not a `#[test]` body, so the result is dropped
            // rather than unwrapped.
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

/// A deterministic source of draws, seeded from a committed constant.
///
/// `splitmix64`: one addition into the state, two multiply-and-shift rounds out of it. Enough
/// mixing for a sweep and short enough to read, which matters more here than quality would,
/// because what this generator has to be is reproducible.
///
/// Every step is a `wrapping_*` method rather than an operator. `arithmetic_side_effects` is an
/// error in this workspace and a mixing function is the one place in it where a wrap is the
/// intent rather than the bug the lint exists to catch.
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
        // Masked to thirty-two bits before the conversion, so the draw fits a `usize` on the
        // targets where one is not sixty-four bits wide and the sequence is the same on both.
        let drawn = usize::try_from(self.draw() & 0xFFFF_FFFF).unwrap_or(0);
        drawn.checked_rem(bound).unwrap_or(0)
    }

    /// One of `choices`, and nothing when there are none.
    fn pick<'a, T>(&mut self, choices: &'a [T]) -> Option<&'a T> {
        choices.get(self.below(choices.len()))
    }
}

/// Names a generated line may carry, including the two that are component boundaries and the
/// empty one that makes a blank line.
const NAMES: &[&[u8]] = &[
    b"SUMMARY",
    b"DTSTART",
    b"UID",
    b"X-VENDOR",
    b"BEGIN",
    b"END",
    b"",
];

/// Parameter runs, written as they appear in a header rather than assembled from pieces.
///
/// Each is a shape RFC 5545 section 3.2 has an opinion about: a quoted value carrying every
/// separator the quotes exist for, a quote that never closes, a name with no `=`, and an `=`
/// with no name.
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

/// The three terminators section 3.1 leaves a reader to tell apart, and none at all.
///
/// `CRLF` appears twice so that the one the specification asks for is drawn about as often as
/// the three deviations together, which is roughly the mix a real corpus has.
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
///
/// The fold is inserted at an arbitrary octet of the assembled line rather than at a boundary
/// the generator understands, so it lands inside a name, inside a quoted parameter value, or
/// between the two octets of one codepoint as readily as anywhere else. That is the point:
/// section 3.1 folds at octets and says nothing about what they spell.
fn append_line(stream: &mut Stream, out: &mut Vec<u8>) {
    let mut assembled: Vec<u8> = Vec::new();
    extend(&mut assembled, stream.pick(NAMES));
    let runs = stream.below(3);
    for _ in 0..runs {
        extend(&mut assembled, stream.pick(PARAMETERS));
    }
    // A line with no `:` at all is the degenerate case section 3.1 has no syntax for and every
    // producer eventually writes, so one line in eight is drawn without one.
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

/// A whole calendar, or something shaped enough like one to be worth reading.
///
/// The wrapping boundaries are written rather than drawn, because nesting deep enough for the
/// depth bound to be reachable never arrives by accident out of a fragment table. The lines
/// between them close those boundaries only by accident, which is the case the reader has to
/// survive: `docs/adr/0001` promises an `END` that never came and an `END` nobody opened are
/// both kept and both diagnosed.
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

/// One octet-level edit to a committed fixture.
///
/// A calendar a real client exported is the only material here whose shape nobody chose, and
/// the interesting inputs near one are the files a truncated download, a gateway that
/// re-encoded the body, or a hand edit could produce from it. Each variant is one of those.
#[derive(Clone, Copy, Debug)]
enum Edit {
    /// Replace one octet with one that decides how a line is read.
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

/// Every edit each fixture is put through, applied in this order so a failure is locatable.
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
    /// Inputs refused at a bound the octets confirmed.
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

/// The `index`th string of exactly `length` octets over [`ALPHABET`].
///
/// Counted in base twelve rather than recursed, so the order does not depend on a call stack
/// and a failure can be reproduced from its length and its index alone.
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
///
/// Read from disk rather than named one by one, so a fixture a later milestone commits is swept
/// without this file being edited. Sorted by path, because a sweep whose order depends on the
/// filesystem is a sweep whose failures depend on the filesystem.
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
///
/// An unreadable entry is passed over rather than reported, because the count this sweep prints
/// and the assertion that it found a corpus at all are what catch a directory that moved. A
/// helper outside a test function is production code as far as the workspace lint profile is
/// concerned, so nothing here unwraps.
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
// The write leg: P3 and P4, which nothing above this line reaches
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

/// Every write each fixture is put through, applied in this order so a failure is locatable.
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

/// The component at `outer`, or the one at `inner` inside it, mutably.
///
/// Two levels rather than a walk to an arbitrary depth, and the reason is the corpus: one
/// committed fixture nests sixteen thousand components, and a recursive search for the `n`th of
/// them would overflow the stack inside this file rather than finding anything out about the
/// crate under it.
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
/// Walked on an explicit stack, for the reason every other traversal in this workspace uses
/// one. This is the shape a second client reads, and no scoped write may change it.
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

/// Apply one write to `component`, answering how many properties the document should gain.
///
/// `None` is a refusal or an absence, which are answers rather than failures: what P3 is about
/// is what happens to the file when a write *is* applied.
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

/// Put one fixture through every kind of scoped write, checking P3 and P4 after each.
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

/// One write into one component of one fixture, and the three things asked of it afterwards.
fn sweep_one_write(
    stream: &mut Stream,
    original: &[u8],
    write: Write,
    tally: &mut WriteTally,
) -> Result<(), String> {
    // A fixture the default policy refuses has no tree to write into, and its refusal is
    // already the subject of the leg above: `generative_shard` asks of every committed fixture
    // whether what it earns is a bound the octets independently confirm.
    let Ok(mut document) = Document::parse(original, Limits::DEFAULT, &mut Vec::new()) else {
        tally.absent = tally.absent.saturating_add(1);
        return Ok(());
    };
    let shape = nesting(&document);
    let held = property_count(&document);
    // Drawn against what the document actually holds, so a draw lands on a component rather
    // than on a position no fixture has.
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
    // P1 and P2 over what the write produced, through the same examination every other leg
    // uses: the file this crate wrote reads back as itself and is a fixed point.
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
    /// The input is refused, naming this bound.
    Refused(ParseError),
}

/// The octets this sweep's own machinery has to agree with a person about.
///
/// Table-driven and written by hand, because everything else in this file is generated and a
/// generated corpus cannot tell anyone that the generator is wrong. The rows cover the empty
/// input, the terminator section 3.1 asks for beside the three shapes that earn a diagnostic
/// instead of a refusal, and both sides of every bound [`TIGHT`] states — the largest input
/// each one admits, and the one octet, parameter, fold or item past it.
const ANCHORS: &[(&str, &[u8], Limits, Anchor)] = &[
    ("the empty input", b"", Limits::DEFAULT, Anchor::Clean),
    // An `X-` name rather than a registered one, because section 3.8.8 allows an extension
    // property anywhere and this row is the only one in the table that claims *no* diagnostic.
    // A row that claimed silence for a name some later unit teaches the reader to have an
    // opinion about would be a row that fails for being right.
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

/// The one place this file names a code rather than counting diagnostics.
///
/// A sweep that asserted a code per input would be asserting the emission table of units still
/// being written, so everywhere else here says only that something was reported. These two are
/// the exception because they are what the committed reader already reports, and because "a
/// violation is a diagnostic and an error means nothing could be built" (`docs/adr/0009`) is
/// the claim every count in this file rests on: if a bare `LF` were a refusal, the sweeps below
/// would be counting refusals that are really violations and calling that a bound.
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
/// Derived from the committed seed and the shard's own index rather than drawn from a shared
/// stream, so that the shards are independent processes covering disjoint inputs and each one's
/// failure is reproducible from two numbers that are both in this file.
fn shard_stream(shard: usize) -> Stream {
    let offset = as_units(shard).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    Stream::new(SEED.wrapping_add(offset))
}

/// Every input of at most [`EXHAUSTIVE_LENGTH`] octets over [`ALPHABET`], under two policies.
///
/// Exhaustive rather than sampled at this size, because the interesting inputs are the ones
/// nobody would draw: a `DQUOTE` that opens where no value may begin, a `\` with nothing after
/// it, a `CR` followed by a `CR`, a fold introduced before the first octet of the line.
///
/// One shard of the deal, and the shards partition the whole: input `index` of `length` is
/// examined by the shard `index` is congruent to, so nothing here is covered twice and the six
/// of them together cover every string there is at these lengths.
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

/// Calendars drawn from [`SEED`], read under a generous policy and a binding one.
///
/// The default policy is where byte identity is the claim; [`BOUNDED`] is where the refusal is,
/// and both are asserted over the same octets so that a shape which round-trips is also a shape
/// whose refusal names something real. Nothing here reads a clock, so the calendars are the
/// same on every machine and a failure arrives with the seed that produced it.
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

/// Every committed fixture this shard is responsible for, with its index.
///
/// Dealt out by position in the sorted corpus. A fixture committed later shifts the deal, which
/// changes which shard examines it and not whether one does — the draws each shard makes are
/// its own, so a fixture added at the end cannot move another fixture's inputs.
fn shard_of_corpus(shard: usize) -> Vec<(String, Vec<u8>)> {
    fixtures()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index.checked_rem(FIXTURE_SHARDS) == Some(shard))
        .map(|(_, held)| held)
        .collect()
}

/// Octet-level edits to every calendar already committed under `tests/fixtures`.
///
/// What this sweep adds is the claim the fixture files cannot make — that the inputs *around* a
/// real export are preserved or refused just as the export is.
///
/// A fixture that does not hold on its own fails this sweep rather than being skipped past it.
/// The earlier reading — that somebody else's failing case is somebody else's business — let a
/// fixture that `examine` rejected be counted and printed rather than reported, and a corpus
/// could have gone half unswept behind a `swept > skipped` guard. A committed fixture is part
/// of the evidence or it is not committed.
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

/// One scoped write of each kind, into every calendar this shard is responsible for.
///
/// The leg that reaches P3 and P4. Each write is checked against the three things a second
/// client depends on: what was written reads back and is a fixed point, the document still
/// nests exactly as it did, and the number of properties in it changed by what the change said
/// it would and by nothing more.
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

/// Declare one `#[test]` that runs `leg` over shard `index` of that leg's inputs.
///
/// A macro rather than the same five lines eighteen times. Each shard has to be its own test
/// function, because what a shard is for is being its own process with its own share of the
/// time bound; a loop inside one test would put every shard back under one clock.
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
