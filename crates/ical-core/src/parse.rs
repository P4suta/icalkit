// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Building a document out of the token stream, and the recovery rules that make that total.
//!
//! Specification: RFC 5545 section 3.4 and section 3.6.
//!
//! There is one recovery rule and it is worth stating on its own, because it is what makes
//! "never discards the file" mechanical rather than promised: anything that is not a
//! well-formed component boundary is stored as an ordinary [`Property`]. A line with no `:`,
//! a blank line, a `BEGIN` carrying parameters, an `END` with nothing open, an `END` naming
//! another component — each becomes a property, each is reported through the caller's sink,
//! and each is written back octet for octet. A `BEGIN` whose `END` never arrived keeps its
//! component and serializes without one. Every one of those is a diagnostic *and* a
//! byte-identical round trip, which are two claims and not one (`docs/adr/0001`).
//!
//! The builder reads tokens and never octets. [`Document::parse`] is
//! [`Document::from_tokens`] over a `ContentLineReader` and nothing else, so a caller with 64
//! KB of RAM and a caller who wants a tree are reading through one grammar rather than two
//! that drifted (`docs/adr/0008`).
//!
//! Octets are charged as they are appended rather than counted once at the end, so a value
//! folded across five million continuation lines is refused at the octet that crosses the
//! budget rather than after it is resident. A refusal is never a truncation: writing back
//! fewer octets than were read is the one thing this crate may not do, which is why an
//! oversized value is a [`ParseError`] and not a diagnostic (`docs/adr/0007`).
//!
//! Five of the codes reported here are about octets rather than about structure, and not one
//! of them changes what is stored: a control character where RFC 5545 section 3.1 excludes
//! one, a physical line past the seventy-five octets it allows, a parameter value whose
//! `DQUOTE` was never closed, a parameter name that arrived without an `=`, and an RFC 6868
//! caret pair the specification gives no meaning. Each is offered once the line has ended,
//! because a fold is what stands between an offset in the unfolded line and an offset in the
//! octets the caller handed in, and the folds arrive with the line's last token.

use alloc::vec::Vec;
use core::mem;

use ical_grammar::{
    ContentLineReader, ContentLineSource, Diagnostic, DiagnosticCode, DiagnosticSink, FoldPoint,
    Limits, LineEnding, LineLayout, Location, Meter, ParseError, Severity, Span, Token,
    is_control_octet, report_diagnostic, undefined_caret_escapes, unquote_parameter,
};

use crate::octets::RawText;
use crate::tree::{Boundary, Component, Document, Item, Parameter, Property};

impl Document {
    /// Read a document out of `input` under `limits`, reporting what was wrong into `sink`.
    ///
    /// This is a `ContentLineReader` handed to [`Document::from_tokens`], and deliberately
    /// nothing else. A private fast path is how one name acquires two grammars: the tree
    /// builder takes raw octets because that was convenient, the public reader grows its own
    /// rules, and the divergence surfaces as a file one path accepts and the other does not.
    ///
    /// Nothing the reader could not make sense of is discarded. Every structural anomaly
    /// degrades to an ordinary property carrying the octets it arrived with, so a caller that
    /// wants strictness reads the diagnostics and a caller that wants to show the user their
    /// meeting still can.
    ///
    /// The meter is minted here and dropped here. A caller that needs the budget to span more
    /// than one parse — the fan-out case a shared ledger exists for — calls
    /// [`Document::from_tokens`] with a meter of its own.
    pub fn parse<S: DiagnosticSink + ?Sized>(
        input: &[u8],
        limits: Limits,
        sink: &mut S,
    ) -> Result<Self, ParseError> {
        let mut meter = Meter::new(limits);
        let mut reader = ContentLineReader::new(input, limits.grammar());
        Self::from_tokens(&mut reader, &mut meter, sink)
    }

    /// Read a document out of a token source, charging `meter` as the octets are appended.
    ///
    /// The entry point for a caller that has a source of its own — a filter wrapped around a
    /// reader, a stream arriving over CalDAV — and the one [`Document::parse`] itself calls.
    /// The ledger is the caller's so that its lifetime is the caller's choice and not the
    /// call's: one meter handed to five thousand parses makes five thousand individually
    /// bounded parses bounded in aggregate (`docs/adr/0010`).
    ///
    /// A sink that refuses is not a reason to stop. Refusal is counted against the meter and
    /// the rest of the input is read, which is what makes the promise that a violation never
    /// discards the file hold with no allocator linked (`docs/adr/0009`).
    pub fn from_tokens<T, S>(
        tokens: &mut T,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Result<Self, ParseError>
    where
        T: ContentLineSource + ?Sized,
        S: DiagnosticSink + ?Sized,
    {
        TreeBuilder::read(tokens, meter, sink).map(Self::new)
    }
}

/// The most octets RFC 5545 section 3.1 allows one physical line, terminator excluded.
///
/// Counted over a physical line rather than over a content line, which is the distinction
/// section 3.1 draws: a producer that folded a long value obeyed it, and the whitespace octet
/// that introduced each continuation is one of the octets that continuation spent.
const MAX_PHYSICAL_LINE_OCTETS: usize = 75;

/// A byte count as a charge against the ledger, saturating rather than wrapping.
///
/// `usize` is not `u64` on every target these crates build for. A count that does not fit is
/// charged as the largest the ledger can hold, which refuses rather than admits.
fn as_units(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Whether `bytes` holds an octet RFC 5545 section 3.1 excludes from a value or a parameter.
///
/// The predicate is the grammar crate's rather than a second reading of section 3.1 written
/// here. `HTAB` is whitespace and belongs in a value; the write side already refuses to author
/// what this side reports, and the two answers drifting apart is the whole failure a shared
/// predicate exists to prevent.
fn holds_control_octet(bytes: &[u8]) -> bool {
    bytes.iter().any(|octet| is_control_octet(*octet))
}

/// The diagnostic a terminator earns, or `None` for the one RFC 5545 section 3.1 asks for.
fn terminator_code(ending: LineEnding) -> Option<DiagnosticCode> {
    match ending {
        LineEnding::CrLf => None,
        LineEnding::Lf => Some(DiagnosticCode::BareLineFeed),
        LineEnding::Cr => Some(DiagnosticCode::BareCarriageReturn),
    }
}

/// Whether a content line with this name opens or closes a component rather than holding a
/// value.
///
/// Compared case-insensitively because RFC 5545 section 3.1 compares names that way, and the
/// spelling that arrived is kept on the [`Boundary`] rather than corrected.
///
/// The reader's own question, and also the write side's: a line the tree stores as a property
/// and writes back as `END:VEVENT` is read as a component boundary by the next reader, whoever
/// it is, so the doors that author a line consult this before authoring one. One predicate,
/// because two readings of what a boundary is are two trees for one file.
pub(crate) fn names_a_component_boundary(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"BEGIN") || name.eq_ignore_ascii_case(b"END")
}

/// Which boundary keyword a property name is, for the two that are keywords.
fn boundary_kind(name: &[u8]) -> Option<LineKind> {
    if name.eq_ignore_ascii_case(b"BEGIN") {
        Some(LineKind::Begin)
    } else if name.eq_ignore_ascii_case(b"END") {
        Some(LineKind::End)
    } else {
        None
    }
}

/// What one content line turned out to be, before the open components have their say.
#[derive(Clone, Copy, Debug)]
enum LineKind {
    /// An ordinary property, carrying the code its anomaly earned when it had one.
    Property(Option<DiagnosticCode>),
    /// A well-formed `BEGIN`.
    Begin,
    /// A well-formed `END`, which may still be unmatched or may name another component.
    End,
}

/// Where one content line sat in the octets the caller handed in.
///
/// Reconstructed rather than reported, because a token carries no offset. It is exact for as
/// long as the round trip is: what is counted here is what a write puts back.
#[derive(Clone, Copy, Debug)]
struct Extent {
    /// Offset of the line's first octet.
    start: u64,
    /// Offset one past its last, folds and terminator included.
    end: u64,
}

impl Extent {
    /// The location a diagnostic about this whole line points at.
    fn location(self) -> Location {
        if let Some(span) = Span::new(self.start, self.end) {
            Location::at(span)
        } else {
            // `end` is only ever `start` plus a saturating sum, so the range cannot run
            // backwards; the first octet is the honest answer if the arithmetic saturates.
            Location::at_offset(self.start)
        }
    }
}

/// A forward-only walk along one unfolded line, keeping input offsets in step with it.
///
/// A [`FoldPoint`] addresses the unfolded line, and the octets a fold spends — its terminator
/// and the whitespace that introduced the continuation — sit *between* the octets it addresses
/// rather than inside them. Turning an unfolded offset into an offset in the caller's input is
/// therefore a sum over the folds already passed.
///
/// Forward-only because the line is taken apart from left to right, and because the honest
/// alternative is a sum over every fold at every lookup. `max_folds_per_line` admits a million
/// of them and `max_parameters` a thousand parameters on the same line, so the quadratic form
/// is a denial of service wearing a location's clothes.
#[derive(Debug)]
struct FoldWalk<'a> {
    /// The folds recorded for this line, in the order the producer wrote them.
    folds: &'a [FoldPoint],
    /// How many of those the walk has passed.
    passed: usize,
    /// What the folds already passed add to every offset from here on.
    carried: u64,
    /// Where the line's first octet sits in the caller's input.
    line_start: u64,
}

impl<'a> FoldWalk<'a> {
    /// A walk over `folds`, for a line whose first octet sits at `line_start`.
    fn new(folds: &'a [FoldPoint], line_start: u64) -> Self {
        Self {
            folds,
            passed: 0,
            carried: 0,
            line_start,
        }
    }

    /// Where the unfolded octet at `offset` sits in the input.
    ///
    /// A fold recorded *at* `offset` breaks the line immediately before that octet, so the
    /// octets it spends come first and are counted here.
    fn opens(&mut self, offset: usize) -> u64 {
        self.advance(offset, true)
    }

    /// Where a range of unfolded octets that ends at `offset` stops, in the input.
    ///
    /// A fold recorded at `offset` breaks before an octet the range does not contain, so its
    /// octets lie outside the range and are left for the next lookup to count.
    fn closes(&mut self, offset: usize) -> u64 {
        self.advance(offset, false)
    }

    /// Where the unfolded octets `from .. from + len` sit in the input.
    fn extent(&mut self, from: usize, len: usize) -> Extent {
        let start = self.opens(from);
        let end = self.closes(from.saturating_add(len));
        Extent { start, end }
    }

    /// The input offset of `offset`, having passed the folds that precede it.
    ///
    /// `leading` distinguishes the two answers a fold sitting exactly at `offset` deserves: it
    /// precedes the octet there and does not precede the end of a range that stops there.
    fn advance(&mut self, offset: usize, leading: bool) -> u64 {
        let cursor = as_units(offset);
        // Bound by value, because the walk mutates itself inside the loop a borrow of it
        // would still be alive across.
        while let Some(&point) = self.folds.get(self.passed) {
            let at = u64::from(point.offset);
            if at > cursor || (!leading && at == cursor) {
                break;
            }
            self.passed = self.passed.saturating_add(1);
            // The terminator, plus the one whitespace octet that introduced the continuation.
            self.carried = self
                .carried
                .saturating_add(as_units(point.newline.written_len()))
                .saturating_add(1);
        }
        self.line_start
            .saturating_add(cursor)
            .saturating_add(self.carried)
    }
}

/// Offer one diagnostic about a stretch of the caller's octets.
fn report_extent<S>(
    extent: Extent,
    code: DiagnosticCode,
    severity: Severity,
    meter: &mut Meter,
    sink: &mut S,
) where
    S: DiagnosticSink + ?Sized,
{
    let diagnostic = Diagnostic::new(code, severity, extent.location());
    report_diagnostic(sink, meter, diagnostic);
}

/// Report a physical line that ran past section 3.1's bound, and nothing for one that did not.
fn report_overlong_line<S>(start: u64, width: usize, meter: &mut Meter, sink: &mut S)
where
    S: DiagnosticSink + ?Sized,
{
    if width <= MAX_PHYSICAL_LINE_OCTETS {
        return;
    }
    let extent = Extent {
        start,
        end: start.saturating_add(as_units(width)),
    };
    report_extent(
        extent,
        DiagnosticCode::LineTooLong,
        Severity::Violation,
        meter,
        sink,
    );
}

/// Report what one stretch of stored octets was wrong about, and say where it sat.
///
/// One diagnostic per piece rather than per octet. `ControlCharacterInText` says that a value
/// or a parameter held a control character, and a `DESCRIPTION` of a million `NUL`s held one
/// defect rather than a million — a caller with a fixed-capacity sink would otherwise lose
/// every other diagnostic in the file to that one line.
fn report_piece<S>(
    walk: &mut FoldWalk<'_>,
    from: usize,
    bytes: &[u8],
    meter: &mut Meter,
    sink: &mut S,
) -> Extent
where
    S: DiagnosticSink + ?Sized,
{
    let extent = walk.extent(from, bytes.len());
    if holds_control_octet(bytes) {
        report_extent(
            extent,
            DiagnosticCode::ControlCharacterInText,
            Severity::Violation,
            meter,
            sink,
        );
    }
    extent
}

/// Report what one parameter value, quotes and carets and all, was wrong about.
///
/// [`report_piece`] answers for the octets themselves. What the `DQUOTE`s were doing and what a
/// caret was for are questions only a parameter value is asked, and both are answered by the
/// grammar crate rather than by a second reading of section 3.2 taken here.
fn report_parameter_value<S>(
    walk: &mut FoldWalk<'_>,
    from: usize,
    bytes: &[u8],
    meter: &mut Meter,
    sink: &mut S,
) where
    S: DiagnosticSink + ?Sized,
{
    let extent = report_piece(walk, from, bytes, meter, sink);
    let unquoted = unquote_parameter(bytes);
    if let Some(code) = unquoted.diagnostic_code() {
        // The octet that was never closed is the `DQUOTE` that opened, and it is that one
        // octet: everything after it is what the producer meant to send, and it is all here.
        let quote = Extent {
            start: extent.start,
            end: extent.start.saturating_add(1),
        };
        report_extent(quote, code, Severity::Violation, meter, sink);
    }
    // Asked of the value between the quotes, because that is the scope RFC 6868 defines the
    // encoding over: a `DQUOTE` is a delimiter, and a caret standing against one is not a pair.
    // The answer is about the value and not about one octet of it, so the whole value is what
    // it points at — the grammar crate walks the pairs left to right and reports whether any
    // was undefined, which is the reading that keeps `^^x` from looking like `^x`.
    if undefined_caret_escapes(unquoted.value()) {
        report_extent(
            extent,
            DiagnosticCode::UndefinedCaretEscape,
            // RFC 6868 section 2 requires such a pair be left as it is, so nothing was violated
            // and nothing was repaired; the caller is told a producer may have meant something.
            Severity::Note,
            meter,
            sink,
        );
    }
}

/// Report one parameter, and say where the octets after it begin.
///
/// `at` is the offset of the `;` that opened it, counted in the unfolded line.
fn report_parameter<S>(
    walk: &mut FoldWalk<'_>,
    at: usize,
    entry: &Parameter,
    meter: &mut Meter,
    sink: &mut S,
) -> usize
where
    S: DiagnosticSink + ?Sized,
{
    let name = entry.name().as_bytes();
    let from = at.saturating_add(1);
    let extent = report_piece(walk, from, name, meter, sink);
    let after = from.saturating_add(name.len());
    if !entry.has_value() {
        // Section 3.2 gives no shape to a name with no `=`, and producers write one anyway.
        // The octets it concerns are the name, because the name is all that arrived.
        report_extent(
            extent,
            DiagnosticCode::ParameterWithoutValue,
            Severity::Violation,
            meter,
            sink,
        );
        return after;
    }
    let value = entry.value().as_bytes();
    let value_at = after.saturating_add(1);
    report_parameter_value(walk, value_at, value, meter, sink);
    value_at.saturating_add(value.len())
}

/// One content line's octets, copied out of the tokens as they arrive.
///
/// Copied rather than borrowed because a token borrows the source and the next token
/// invalidates it, and because the tree owns its memory end to end (`docs/adr/0007`).
#[derive(Debug, Default)]
struct PendingLine {
    /// The property name, as written.
    name: Vec<u8>,
    /// The parameters, in the order they were written.
    parameters: Vec<Parameter>,
    /// The value, unfolded, with nothing unescaped and nothing normalized.
    value: Vec<u8>,
}

impl PendingLine {
    /// Whether no octet of a line has been accumulated yet.
    fn is_empty(&self) -> bool {
        self.name.is_empty() && self.parameters.is_empty() && self.value.is_empty()
    }

    /// How many octets this line occupies once unfolded, name through value.
    ///
    /// This is the count a write reproduces, which is why it is the count offsets are
    /// measured against: a `FoldPoint` addresses the unfolded line from the first octet of
    /// the name.
    fn unfolded_len(&self, has_separator: bool) -> usize {
        let mut total = self.name.len();
        for entry in &self.parameters {
            // `;` then the name, then `=` and the value when one was written at all.
            total = total.saturating_add(1).saturating_add(entry.name().len());
            if entry.has_value() {
                total = total.saturating_add(1).saturating_add(entry.value().len());
            }
        }
        if has_separator {
            total = total.saturating_add(1);
        }
        total.saturating_add(self.value.len())
    }
}

/// A component whose `BEGIN` arrived and whose `END` has not.
#[derive(Debug)]
struct OpenComponent {
    /// The `BEGIN` line, in the spelling it arrived in.
    begin: Boundary,
    /// The entries read so far, in order.
    items: Vec<Item>,
    /// Where the `BEGIN` line sat, so an unclosed component can say where it opened.
    extent: Extent,
}

/// The document under construction, and the components still open above it.
#[derive(Debug, Default)]
struct TreeBuilder {
    /// Entries at the top level of the document.
    root: Vec<Item>,
    /// Components awaiting an `END`, outermost first.
    open: Vec<OpenComponent>,
    /// The line being accumulated.
    pending: PendingLine,
    /// Offset of the first octet of that line.
    line_start: u64,
}

impl TreeBuilder {
    /// Read every token the source has, and hand back the top-level entries.
    fn read<T, S>(source: &mut T, meter: &mut Meter, sink: &mut S) -> Result<Vec<Item>, ParseError>
    where
        T: ContentLineSource + ?Sized,
        S: DiagnosticSink + ?Sized,
    {
        let mut builder = Self::default();
        while let Some(next) = source.next_token() {
            builder.take(next?, meter, sink)?;
        }
        builder.into_items(meter, sink)
    }

    /// Fold one token into the line being accumulated, finishing that line at its end.
    fn take<S>(
        &mut self,
        token: Token<'_>,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Result<(), ParseError>
    where
        S: DiagnosticSink + ?Sized,
    {
        match token {
            Token::Name(bytes) => self.take_name(bytes, meter),
            Token::Parameter {
                name,
                value,
                has_value,
            } => self.take_parameter(name, value, has_value, meter),
            Token::Value { bytes, .. } => self.take_value(bytes, meter),
            Token::EndOfLine {
                folds,
                ending,
                has_separator,
            } => {
                Self::charge_folds(folds, meter)?;
                let layout = LineLayout::preserved(folds.to_vec(), ending, has_separator);
                self.finish_line(layout, meter, sink)
            },
            // `Token` is `#[non_exhaustive]`, and that guarantee is what the seam between the
            // grammar crate and this one spends: a variant added there must not break the one
            // consumer that has to handle every line. A token this build does not know
            // contributes no octets rather than ending a parse that can still finish.
            _ => Ok(()),
        }
    }

    /// Charge the octets this line's folds occupy, before any of them is retained.
    ///
    /// A fold is the one thing the tree keeps that no other charge counts. A name, a parameter
    /// and a value are charged as they are appended; a fold's terminator and the whitespace
    /// octet that introduced the continuation are neither, and yet a [`FoldPoint`] is stored
    /// for each of them so the writer can put the fold back. A line of a hundred thousand `LF
    /// SP` pairs is therefore one item, one octet of value and no header — nothing the ledger
    /// sees — while the tree grows with the input rather than with the caller's policy.
    ///
    /// Charged against the same octet budget the rest of the line is, because they *are*
    /// octets of the caller's input: a caller who stated sixty-four octets has not agreed to
    /// sixteen megabytes being read on its behalf (`docs/adr/0010`). The reader's own
    /// `max_folds_per_line` bounds what one line may retain before this is ever reached; this
    /// is the half that binds in aggregate, across every line of the document.
    fn charge_folds(folds: &[FoldPoint], meter: &mut Meter) -> Result<(), ParseError> {
        for point in folds {
            // The terminator, plus the one whitespace octet that introduced the continuation.
            let cost = as_units(point.newline.written_len()).saturating_add(1);
            meter.charge_bytes(cost)?;
        }
        Ok(())
    }

    /// Take the property name, charging its octets before they are appended.
    fn take_name(&mut self, bytes: &[u8], meter: &mut Meter) -> Result<(), ParseError> {
        meter.charge_bytes(as_units(bytes.len()))?;
        self.pending.name.clear();
        self.pending.name.extend_from_slice(bytes);
        Ok(())
    }

    /// Take one parameter, quotes and all, charging its octets before they are appended.
    fn take_parameter(
        &mut self,
        name: &[u8],
        value: &[u8],
        has_value: bool,
        meter: &mut Meter,
    ) -> Result<(), ParseError> {
        meter.charge_bytes(as_units(name.len()))?;
        meter.charge_bytes(as_units(value.len()))?;
        let stored = if has_value {
            Parameter::new(RawText::from_bytes(name), RawText::from_bytes(value))
        } else {
            // RFC 5545 section 3.2 has no such shape, and producers write it anyway. It is
            // kept as it arrived so that it is written back as it arrived.
            Parameter::without_value(RawText::from_bytes(name))
        };
        self.pending.parameters.push(stored);
        Ok(())
    }

    /// Take one chunk of the value, refusing before appending rather than after.
    ///
    /// The per-value bound is checked against what the append would produce, so a value that
    /// crosses it is refused at the chunk that crosses it and never becomes resident.
    /// Truncating instead would write back fewer octets than were read, and a truncated value
    /// is indistinguishable from a preserved one at the serializer.
    fn take_value(&mut self, bytes: &[u8], meter: &mut Meter) -> Result<(), ParseError> {
        let limit = meter.limits().max_value_bytes();
        let grown = self.pending.value.len().saturating_add(bytes.len());
        if as_units(grown) > u64::from(limit) {
            return Err(ParseError::ValueTooLarge { limit });
        }
        meter.charge_bytes(as_units(bytes.len()))?;
        self.pending.value.extend_from_slice(bytes);
        Ok(())
    }

    /// Store the line that just ended, and report what was wrong with it.
    ///
    /// The terminators are reported before the verdict because that is the order they were
    /// observed in: how a line ended is known while it is read, what it turned out to be is
    /// known only once it has. What the octets themselves were wrong about sits between the
    /// two, walked from the name to the value, so that a line's diagnostics arrive in
    /// roughly the order its octets did.
    ///
    /// None of the three can refuse. A line that violates section 3.1 in four separate ways
    /// is stored exactly as it arrived and written back exactly as it arrived, and the four
    /// diagnostics are what the caller is given instead of a repair (`docs/adr/0009`).
    fn finish_line<S>(
        &mut self,
        layout: LineLayout,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Result<(), ParseError>
    where
        S: DiagnosticSink + ?Sized,
    {
        let extent = self.measure(&layout);
        self.report_terminators(&layout, extent, meter, sink);
        self.report_widths(&layout, meter, sink);
        self.report_text(&layout, meter, sink);
        let kind = self.classify(&layout);
        if let Some(code) = self.apply(kind, layout, extent, meter)? {
            let diagnostic = Diagnostic::new(code, Severity::Violation, extent.location());
            report_diagnostic(sink, meter, diagnostic);
        }
        self.line_start = extent.end;
        Ok(())
    }

    /// Where the line that just ended sat in the caller's octets.
    fn measure(&self, layout: &LineLayout) -> Extent {
        let unfolded = self.pending.unfolded_len(layout.has_separator());
        let mut end = self.line_start.saturating_add(as_units(unfolded));
        for point in layout.folds() {
            // Each fold costs its terminator plus the one whitespace octet that introduced
            // the continuation line.
            end = end
                .saturating_add(as_units(point.newline.written_len()))
                .saturating_add(1);
        }
        if let Some(ending) = layout.ending() {
            end = end.saturating_add(as_units(ending.written_len()));
        }
        Extent {
            start: self.line_start,
            end,
        }
    }

    /// Report what each of this line's terminators was, in the order they were read.
    ///
    /// One diagnostic per physical terminator rather than one per content line. A folded line
    /// ends a physical line at every fold, and a producer that wrote a bare `LF` at each of
    /// them violated RFC 5545 section 3.1 at each of them.
    fn report_terminators<S>(
        &self,
        layout: &LineLayout,
        extent: Extent,
        meter: &mut Meter,
        sink: &mut S,
    ) where
        S: DiagnosticSink + ?Sized,
    {
        let mut carried = 0_u64;
        for point in layout.folds() {
            let mark = self
                .line_start
                .saturating_add(u64::from(point.offset))
                .saturating_add(carried);
            if let Some(code) = terminator_code(point.newline) {
                let at = Location::at_offset(mark);
                report_diagnostic(sink, meter, Diagnostic::new(code, Severity::Violation, at));
            }
            carried = carried
                .saturating_add(as_units(point.newline.written_len()))
                .saturating_add(1);
        }
        let Some(ending) = layout.ending() else {
            let code = DiagnosticCode::MissingFinalLineBreak;
            let diagnostic = Diagnostic::new(code, Severity::Violation, extent.location());
            report_diagnostic(sink, meter, diagnostic);
            return;
        };
        if let Some(code) = terminator_code(ending) {
            let mark = extent.end.saturating_sub(as_units(ending.written_len()));
            let at = Location::at_offset(mark);
            report_diagnostic(sink, meter, Diagnostic::new(code, Severity::Violation, at));
        }
    }

    /// Report every physical line of this content line that ran past what section 3.1 allows.
    ///
    /// Computed from the recorded folds and the unfolded length rather than from the input,
    /// because those two are what a write reproduces: the number reported here is the width
    /// the serializer will put back, which is the only width a caller can act on.
    ///
    /// A fold that landed outside the unfolded line is clamped into it, exactly as the writer
    /// clamps it. Neither is a repair — a fold there addresses an octet that does not exist,
    /// and the two of them agreeing is what keeps a diagnostic from naming octets the file
    /// does not have.
    fn report_widths<S>(&self, layout: &LineLayout, meter: &mut Meter, sink: &mut S)
    where
        S: DiagnosticSink + ?Sized,
    {
        let unfolded = self.pending.unfolded_len(layout.has_separator());
        let mut start = self.line_start;
        let mut from = 0_usize;
        // The first physical line spends nothing on an indent; every continuation spends one.
        let mut indent = 0_usize;
        for point in layout.folds() {
            let at = usize::try_from(point.offset)
                .unwrap_or(usize::MAX)
                .clamp(from, unfolded);
            let width = indent.saturating_add(at.saturating_sub(from));
            report_overlong_line(start, width, meter, sink);
            start = start
                .saturating_add(as_units(width))
                .saturating_add(as_units(point.newline.written_len()));
            from = at;
            indent = 1;
        }
        let width = indent.saturating_add(unfolded.saturating_sub(from));
        report_overlong_line(start, width, meter, sink);
    }

    /// Report what the octets of the line that just ended were wrong about.
    ///
    /// Walked in the order they were written — the name, then each parameter, then the value —
    /// because that is the order [`FoldWalk`] can answer in and because it is the order the
    /// producer wrote them in.
    fn report_text<S>(&self, layout: &LineLayout, meter: &mut Meter, sink: &mut S)
    where
        S: DiagnosticSink + ?Sized,
    {
        let mut walk = FoldWalk::new(layout.folds(), self.line_start);
        let name = self.pending.name.as_slice();
        report_piece(&mut walk, 0, name, meter, sink);
        let mut at = name.len();
        for entry in &self.pending.parameters {
            at = report_parameter(&mut walk, at, entry, meter, sink);
        }
        if layout.has_separator() {
            at = at.saturating_add(1);
        }
        report_piece(&mut walk, at, self.pending.value.as_slice(), meter, sink);
    }

    /// Decide what the accumulated line is, by the one recovery rule.
    fn classify(&self, layout: &LineLayout) -> LineKind {
        if self.pending.name.is_empty() {
            // A blank line is the degenerate case of an empty name, and its absent `:` is not
            // a second independent defect: it is what a blank line is made of.
            return LineKind::Property(Some(DiagnosticCode::EmptyPropertyName));
        }
        if !layout.has_separator() {
            return LineKind::Property(Some(DiagnosticCode::MissingValueSeparator));
        }
        let Some(kind) = boundary_kind(&self.pending.name) else {
            return LineKind::Property(None);
        };
        if self.pending.parameters.is_empty() {
            kind
        } else {
            // Illegal, and seen in the wild. A `Boundary` has nowhere to keep parameters, so
            // the line is stored as the property it syntactically already is.
            LineKind::Property(Some(DiagnosticCode::ParametersOnComponentBoundary))
        }
    }

    /// Store the accumulated line as what it turned out to be, and say what was wrong.
    fn apply(
        &mut self,
        kind: LineKind,
        layout: LineLayout,
        extent: Extent,
        meter: &mut Meter,
    ) -> Result<Option<DiagnosticCode>, ParseError> {
        match kind {
            LineKind::Property(code) => {
                self.store_property(layout, meter)?;
                Ok(code)
            },
            LineKind::Begin => {
                self.open_component(layout, extent, meter)?;
                Ok(None)
            },
            LineKind::End => self.close_component(layout, meter),
        }
    }

    /// Store the accumulated line as an ordinary property.
    fn store_property(&mut self, layout: LineLayout, meter: &mut Meter) -> Result<(), ParseError> {
        meter.charge_item()?;
        let line = mem::take(&mut self.pending);
        let property = Property::new(
            RawText::from_vec(line.name),
            line.parameters,
            RawText::from_vec(line.value),
            layout,
        );
        self.push_item(Item::Property(property));
        Ok(())
    }

    /// Open a component for a well-formed `BEGIN`.
    fn open_component(
        &mut self,
        layout: LineLayout,
        extent: Extent,
        meter: &mut Meter,
    ) -> Result<(), ParseError> {
        meter.charge_item()?;
        meter.enter()?;
        // The line's parameters are empty here by classification: a boundary that carried any
        // never reaches this arm, precisely because a `Boundary` could not write them back.
        let line = mem::take(&mut self.pending);
        self.open.push(OpenComponent {
            begin: Boundary::new(
                RawText::from_vec(line.name),
                RawText::from_vec(line.value),
                layout,
            ),
            items: Vec::new(),
            extent,
        });
        Ok(())
    }

    /// Close the innermost open component, or degrade the `END` to a property.
    ///
    /// An `END` that named another component leaves that component open on purpose. Guessing
    /// which of the open components it meant would reorder entries the producer wrote, and
    /// reordering is the one repair this crate does not perform.
    fn close_component(
        &mut self,
        layout: LineLayout,
        meter: &mut Meter,
    ) -> Result<Option<DiagnosticCode>, ParseError> {
        let closes = self
            .open
            .last()
            .map(|open| open.begin.name().eq_name(&self.pending.value));
        match closes {
            None => {
                self.store_property(layout, meter)?;
                Ok(Some(DiagnosticCode::UnmatchedEnd))
            },
            Some(false) => {
                self.store_property(layout, meter)?;
                Ok(Some(DiagnosticCode::MismatchedEndName))
            },
            Some(true) => {
                self.close_open(layout, meter);
                Ok(None)
            },
        }
    }

    /// Close the innermost open component with the `END` line that arrived for it.
    ///
    /// The component was charged as an item when its `BEGIN` opened it, so its `END` is not
    /// charged again: one component is one item however many lines delimit it.
    fn close_open(&mut self, layout: LineLayout, meter: &mut Meter) {
        let Some(open) = self.open.pop() else {
            return;
        };
        meter.leave();
        let line = mem::take(&mut self.pending);
        let end = Boundary::new(
            RawText::from_vec(line.name),
            RawText::from_vec(line.value),
            layout,
        );
        let component = Component::new(open.begin, open.items, Some(end));
        self.push_item(Item::Component(component));
    }

    /// Append an entry to the innermost open component, or to the document.
    fn push_item(&mut self, item: Item) {
        if let Some(open) = self.open.last_mut() {
            open.items.push(item);
        } else {
            self.root.push(item);
        }
    }

    /// Close what the input left open, and hand back the top-level entries.
    fn into_items<S>(mut self, meter: &mut Meter, sink: &mut S) -> Result<Vec<Item>, ParseError>
    where
        S: DiagnosticSink + ?Sized,
    {
        // A source that stopped in the middle of a line would otherwise lose the octets it
        // had already handed over. The reader ends every line with an `EndOfLine`, so this is
        // insurance against a source that does not rather than a path a file reaches. A `:`
        // is assumed exactly when a value arrived, since a value is what follows one.
        if !self.pending.is_empty() {
            let separated = !self.pending.value.is_empty();
            let layout = LineLayout::preserved(Vec::new(), None, separated);
            self.finish_line(layout, meter, sink)?;
        }
        while let Some(open) = self.open.pop() {
            meter.leave();
            let diagnostic = Diagnostic::new(
                DiagnosticCode::UnclosedComponent,
                Severity::Violation,
                open.extent.location(),
            );
            report_diagnostic(sink, meter, diagnostic);
            let component = Component::new(open.begin, open.items, None);
            self.push_item(Item::Component(component));
        }
        Ok(self.root)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use ical_grammar::{
        ContentLineSource, Diagnostic, DiagnosticCode, FoldPoint, IgnoreDiagnostics, Limits,
        LineEnding, LineLayout, Meter, ParseError, Severity, Token,
    };

    use crate::octets::RawText;
    use crate::tree::{Boundary, Component, Document, Item, Property};

    /// One token as a test writes it: owned, so a source can hand out borrows of it.
    #[derive(Debug)]
    enum Scripted {
        Name(Vec<u8>),
        Parameter {
            name: Vec<u8>,
            value: Vec<u8>,
            has_value: bool,
        },
        Value {
            bytes: Vec<u8>,
            more: bool,
        },
        EndOfLine {
            folds: Vec<FoldPoint>,
            ending: Option<LineEnding>,
            has_separator: bool,
        },
    }

    impl Scripted {
        /// The borrowed token a source yields for this entry.
        fn as_token(&self) -> Token<'_> {
            match self {
                Self::Name(bytes) => Token::Name(bytes.as_slice()),
                Self::Parameter {
                    name,
                    value,
                    has_value,
                } => Token::Parameter {
                    name: name.as_slice(),
                    value: value.as_slice(),
                    has_value: *has_value,
                },
                Self::Value { bytes, more } => Token::Value {
                    bytes: bytes.as_slice(),
                    more: *more,
                },
                Self::EndOfLine {
                    folds,
                    ending,
                    has_separator,
                } => Token::EndOfLine {
                    folds: folds.as_slice(),
                    ending: *ending,
                    has_separator: *has_separator,
                },
            }
        }
    }

    /// A token source over a fixed script.
    ///
    /// The reader is a separate implementation unit, and a builder tested through it would be
    /// tested through somebody else's bugs. A script says exactly what the grammar layer
    /// handed over, which is the only input this unit has.
    #[derive(Debug)]
    struct ScriptedSource {
        /// The tokens to yield, in order.
        script: Vec<Scripted>,
        /// How many have been yielded.
        next: usize,
    }

    impl ContentLineSource for ScriptedSource {
        fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>> {
            let at = self.next;
            self.next = self.next.saturating_add(1);
            Some(Ok(self.script.get(at)?.as_token()))
        }
    }

    fn source(script: Vec<Scripted>) -> ScriptedSource {
        ScriptedSource { script, next: 0 }
    }

    fn name(bytes: &[u8]) -> Scripted {
        Scripted::Name(bytes.to_vec())
    }

    fn value(bytes: &[u8]) -> Scripted {
        Scripted::Value {
            bytes: bytes.to_vec(),
            more: false,
        }
    }

    fn parameter(key: &[u8], text: &[u8]) -> Scripted {
        Scripted::Parameter {
            name: key.to_vec(),
            value: text.to_vec(),
            has_value: true,
        }
    }

    /// A parameter that arrived as a bare name, with no `=` after it.
    fn flag(key: &[u8]) -> Scripted {
        Scripted::Parameter {
            name: key.to_vec(),
            value: Vec::new(),
            has_value: false,
        }
    }

    /// The end of a line that carried a `:` and no fold.
    fn eol(ending: Option<LineEnding>) -> Scripted {
        Scripted::EndOfLine {
            folds: Vec::new(),
            ending,
            has_separator: true,
        }
    }

    /// The end of a line that carried no `:` at all.
    fn colonless(ending: Option<LineEnding>) -> Scripted {
        Scripted::EndOfLine {
            folds: Vec::new(),
            ending,
            has_separator: false,
        }
    }

    fn crlf() -> Scripted {
        eol(Some(LineEnding::CrLf))
    }

    /// Build a document from `script`, keeping every diagnostic it produced.
    fn build(script: Vec<Scripted>) -> (Document, Vec<Diagnostic>) {
        let mut reading = source(script);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut kept: Vec<Diagnostic> = Vec::new();
        let document = Document::from_tokens(&mut reading, &mut meter, &mut kept).unwrap();
        (document, kept)
    }

    fn codes(kept: &[Diagnostic]) -> Vec<DiagnosticCode> {
        kept.iter().copied().map(Diagnostic::code).collect()
    }

    /// Write the tree back out, so a test can say what octets went in and came out.
    ///
    /// The serializer is another implementation unit; this is the oracle that says the tree
    /// kept enough for one to be possible at all.
    fn rewrite(document: &Document) -> Vec<u8> {
        let mut out = Vec::new();
        write_items(document.items(), &mut out);
        out
    }

    fn write_items(items: &[Item], out: &mut Vec<u8>) {
        for entry in items {
            match entry {
                Item::Property(property) => {
                    write_line(&unfolded(property), property.layout(), out);
                },
                Item::Component(component) => write_component(component, out),
            }
        }
    }

    fn write_component(component: &Component, out: &mut Vec<u8>) {
        let begin = component.begin();
        write_line(&unfolded_boundary(begin), begin.layout(), out);
        write_items(component.items(), out);
        if let Some(end) = component.end() {
            write_line(&unfolded_boundary(end), end.layout(), out);
        }
    }

    fn unfolded(property: &Property) -> Vec<u8> {
        let mut line = Vec::new();
        line.extend_from_slice(property.name().as_bytes());
        for entry in property.parameters() {
            line.push(b';');
            line.extend_from_slice(entry.name().as_bytes());
            if entry.has_value() {
                line.push(b'=');
                line.extend_from_slice(entry.value().as_bytes());
            }
        }
        if property.layout().has_separator() {
            line.push(b':');
        }
        line.extend_from_slice(property.value_text().as_bytes());
        line
    }

    fn unfolded_boundary(boundary: &Boundary) -> Vec<u8> {
        let mut line = Vec::new();
        line.extend_from_slice(boundary.keyword().as_bytes());
        if boundary.layout().has_separator() {
            line.push(b':');
        }
        line.extend_from_slice(boundary.name().as_bytes());
        line
    }

    fn write_line(line: &[u8], layout: &LineLayout, out: &mut Vec<u8>) {
        let mut written = 0_usize;
        for point in layout.folds() {
            let at = usize::try_from(point.offset)
                .unwrap()
                .clamp(written, line.len());
            out.extend_from_slice(&line[written..at]);
            out.extend_from_slice(point.newline.as_bytes());
            out.push(point.whitespace());
            written = at;
        }
        out.extend_from_slice(&line[written..]);
        if let Some(ending) = layout.ending() {
            out.extend_from_slice(ending.as_bytes());
        }
    }

    /// Rewrite the value of every `SUMMARY` directly inside a top-level component.
    fn retitle(document: &mut Document, text: &[u8]) {
        for component in document.components_mut() {
            for entry in component.items_mut() {
                let Some(property) = entry.as_property_mut() else {
                    continue;
                };
                if property.is_named(b"SUMMARY") {
                    property.set_value_text(RawText::from_bytes(text));
                }
            }
        }
    }

    #[test]
    fn an_empty_input_is_an_empty_document_and_not_a_failure() {
        let (document, kept) = build(Vec::new());
        assert!(document.items().is_empty());
        assert!(kept.is_empty());
        assert!(rewrite(&document).is_empty());
    }

    #[test]
    fn a_calendar_comes_back_out_the_way_it_went_in() {
        let (document, kept) = build(vec![
            name(b"BEGIN"),
            value(b"VCALENDAR"),
            crlf(),
            name(b"SUMMARY"),
            parameter(b"LANGUAGE", b"en-US"),
            value(b"Team sync"),
            crlf(),
            name(b"END"),
            value(b"VCALENDAR"),
            crlf(),
        ]);
        assert_eq!(
            rewrite(&document),
            b"BEGIN:VCALENDAR\r\nSUMMARY;LANGUAGE=en-US:Team sync\r\nEND:VCALENDAR\r\n"
        );
        assert!(kept.is_empty(), "a conforming calendar earns no diagnostic");
        assert_eq!(document.components().count(), 1);
    }

    #[test]
    fn a_final_line_with_no_terminator_is_written_back_without_one() {
        let (document, kept) = build(vec![name(b"SUMMARY"), value(b"tail"), eol(None)]);
        assert_eq!(rewrite(&document), b"SUMMARY:tail");
        assert_eq!(codes(&kept), vec![DiagnosticCode::MissingFinalLineBreak]);
    }

    #[test]
    fn a_value_delivered_in_chunks_is_one_value() {
        let (document, _) = build(vec![
            name(b"DESCRIPTION"),
            Scripted::Value {
                bytes: b"one ".to_vec(),
                more: true,
            },
            Scripted::Value {
                bytes: b"two".to_vec(),
                more: false,
            },
            crlf(),
        ]);
        let stored = document.items().first().unwrap().as_property().unwrap();
        assert_eq!(stored.value_text().as_bytes(), b"one two");
    }

    #[test]
    fn every_structural_anomaly_becomes_a_property_and_is_written_back() {
        let cases: Vec<(Vec<Scripted>, &[u8], DiagnosticCode, usize)> = vec![
            (
                vec![name(b"HELLO"), colonless(Some(LineEnding::CrLf))],
                b"HELLO\r\n",
                DiagnosticCode::MissingValueSeparator,
                0,
            ),
            (
                vec![name(b""), colonless(Some(LineEnding::CrLf))],
                b"\r\n",
                DiagnosticCode::EmptyPropertyName,
                0,
            ),
            (
                vec![
                    name(b"BEGIN"),
                    parameter(b"X-A", b"1"),
                    value(b"VEVENT"),
                    crlf(),
                ],
                b"BEGIN;X-A=1:VEVENT\r\n",
                DiagnosticCode::ParametersOnComponentBoundary,
                0,
            ),
            (
                vec![name(b"END"), value(b"VEVENT"), crlf()],
                b"END:VEVENT\r\n",
                DiagnosticCode::UnmatchedEnd,
                0,
            ),
            (
                vec![
                    name(b"BEGIN"),
                    value(b"VEVENT"),
                    crlf(),
                    name(b"END"),
                    value(b"VTODO"),
                    crlf(),
                ],
                b"BEGIN:VEVENT\r\nEND:VTODO\r\n",
                DiagnosticCode::MismatchedEndName,
                1,
            ),
        ];
        for (script, expected, code, components) in cases {
            let (document, kept) = build(script);
            assert_eq!(rewrite(&document), expected, "{code}");
            assert!(codes(&kept).contains(&code), "{code}");
            assert_eq!(document.components().count(), components, "{code}");
        }
    }

    #[test]
    fn a_begin_whose_end_never_arrives_keeps_the_component() {
        let (document, kept) = build(vec![
            name(b"BEGIN"),
            value(b"VCALENDAR"),
            crlf(),
            name(b"BEGIN"),
            value(b"VEVENT"),
            crlf(),
            name(b"UID"),
            value(b"1"),
            crlf(),
        ]);
        assert_eq!(
            rewrite(&document),
            b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n"
        );
        assert_eq!(
            codes(&kept),
            vec![DiagnosticCode::UnclosedComponent; 2],
            "one per component left open, innermost first"
        );
        let outer = document.components().next().unwrap();
        assert!(outer.end().is_none());
        assert!(outer.components().next().unwrap().end().is_none());
    }

    #[test]
    fn a_folded_line_is_written_back_where_the_producer_folded_it() {
        // The most decorated line this unit sees: quoted and unquoted parameters, folded
        // twice, once after a bare LF with a space and once after a CRLF with a tab.
        let folds = vec![
            FoldPoint {
                offset: 20,
                tab: false,
                newline: LineEnding::Lf,
            },
            FoldPoint {
                offset: 46,
                tab: true,
                newline: LineEnding::CrLf,
            },
        ];
        let (document, kept) = build(vec![
            name(b"DESCRIPTION"),
            parameter(b"ALTREP", b"\"cid:x\""),
            parameter(b"LANGUAGE", b"en-US"),
            value(b"one two three"),
            Scripted::EndOfLine {
                folds,
                ending: Some(LineEnding::CrLf),
                has_separator: true,
            },
            name(b"X-JUNK"),
            colonless(Some(LineEnding::CrLf)),
        ]);
        assert_eq!(
            rewrite(&document),
            b"DESCRIPTION;ALTREP=\"\n cid:x\";LANGUAGE=en-US:one \r\n\ttwo three\r\nX-JUNK\r\n"
        );
        assert_eq!(
            codes(&kept),
            vec![
                DiagnosticCode::BareLineFeed,
                DiagnosticCode::MissingValueSeparator,
            ]
        );
        let bare = kept.first().unwrap().location().span().unwrap();
        assert_eq!(bare.start(), 20, "the fold's terminator, not the line's");
        let orphan = kept.last().unwrap().location().span().unwrap();
        assert_eq!(
            (orphan.start(), orphan.end()),
            (62, 70),
            "the second line starts where the first, folds included, ended"
        );
    }

    #[test]
    fn a_value_longer_than_the_bound_is_refused_rather_than_truncated() {
        let mut reading = source(vec![name(b"X"), value(b"12345"), crlf()]);
        let mut meter = Meter::new(Limits::DEFAULT.with_max_value_bytes(4));
        let outcome = Document::from_tokens(&mut reading, &mut meter, &mut IgnoreDiagnostics);
        assert_eq!(outcome, Err(ParseError::ValueTooLarge { limit: 4 }));
        assert!(
            meter.spent() < 5,
            "the octets that crossed the bound were never charged, let alone stored"
        );
    }

    #[test]
    fn nesting_deeper_than_the_bound_is_refused() {
        let mut reading = source(vec![
            name(b"BEGIN"),
            value(b"VCALENDAR"),
            crlf(),
            name(b"BEGIN"),
            value(b"VEVENT"),
            crlf(),
        ]);
        let mut meter = Meter::new(Limits::DEFAULT.with_max_component_depth(1));
        let outcome = Document::from_tokens(&mut reading, &mut meter, &mut IgnoreDiagnostics);
        assert_eq!(outcome, Err(ParseError::TooDeep { limit: 1 }));
    }

    #[test]
    fn a_sink_that_refuses_does_not_stop_the_reader() {
        let mut reading = source(vec![
            name(b""),
            colonless(Some(LineEnding::Lf)),
            name(b"UID"),
            value(b"1"),
            eol(Some(LineEnding::Lf)),
        ]);
        let mut meter = Meter::new(Limits::DEFAULT);
        let document =
            Document::from_tokens(&mut reading, &mut meter, &mut IgnoreDiagnostics).unwrap();
        assert_eq!(rewrite(&document), b"\nUID:1\n");
        assert_eq!(
            meter.diagnostics_dropped(),
            3,
            "which violations occurred is lost, that they occurred is not"
        );
    }

    #[test]
    fn every_octet_appended_is_charged_against_the_ledger() {
        let mut reading = source(vec![
            name(b"UID"),
            parameter(b"A", b"b"),
            value(b"xy"),
            crlf(),
        ]);
        let mut meter = Meter::new(Limits::DEFAULT);
        Document::from_tokens(&mut reading, &mut meter, &mut IgnoreDiagnostics).unwrap();
        assert_eq!(meter.spent(), 7, "UID, A, b and xy, and nothing invented");
        assert_eq!(meter.items(), 1);
    }

    /// The octets one diagnostic named, as the pair a table can state.
    fn extent_of(kept: &[Diagnostic], at: usize) -> (u64, u64) {
        let span = kept.get(at).unwrap().location().span().unwrap();
        (span.start(), span.end())
    }

    /// A run of value octets, for the cases that are about widths rather than about content.
    fn filler(count: usize) -> Vec<u8> {
        vec![b'a'; count]
    }

    /// Section 3.1 bounds a physical line at seventy-five octets, terminator excluded, so the
    /// line of exactly that width is the one that must earn nothing.
    #[test]
    fn a_physical_line_is_measured_against_the_octets_section_3_1_allows() {
        // Fourteen octets of name and one `:` leave sixty for the value at the bound.
        let cases: Vec<(usize, Vec<DiagnosticCode>)> = vec![
            (59, Vec::new()),
            (60, Vec::new()),
            (61, vec![DiagnosticCode::LineTooLong]),
        ];
        for (width, wanted) in cases {
            let text = filler(width);
            let (document, kept) = build(vec![name(b"X-SEVENTY-FIVE"), value(&text), crlf()]);
            let mut expected = b"X-SEVENTY-FIVE:".to_vec();
            expected.extend_from_slice(&text);
            expected.extend_from_slice(b"\r\n");
            assert_eq!(rewrite(&document), expected, "{width} octets of value");
            assert_eq!(codes(&kept), wanted, "{width} octets of value");
        }
    }

    #[test]
    fn an_overlong_line_names_the_octets_before_its_terminator() {
        let (_, kept) = build(vec![
            name(b"X-SEVENTY-SIX"),
            value(&filler(62)),
            eol(Some(LineEnding::Lf)),
        ]);
        assert_eq!(
            codes(&kept),
            vec![DiagnosticCode::BareLineFeed, DiagnosticCode::LineTooLong,]
        );
        assert_eq!(
            extent_of(&kept, 1),
            (0, 76),
            "the terminator is not one of the octets section 3.1 counts"
        );
    }

    /// The distinction section 3.1 draws is between physical lines, so a content line longer
    /// than the bound is inside it once its producer folded, and the leading whitespace of each
    /// continuation is one of the octets that continuation spent.
    #[test]
    fn a_folded_line_is_measured_one_physical_line_at_a_time() {
        let cases: Vec<(u32, usize, Vec<DiagnosticCode>)> = vec![
            (60, 58, Vec::new()),
            (75, 73, Vec::new()),
            (76, 74, vec![DiagnosticCode::LineTooLong]),
        ];
        for (fold_at, head, wanted) in cases {
            let text = filler(118);
            let folds = vec![FoldPoint {
                offset: fold_at,
                tab: false,
                newline: LineEnding::CrLf,
            }];
            let (document, kept) = build(vec![
                name(b"X"),
                value(&text),
                Scripted::EndOfLine {
                    folds,
                    ending: Some(LineEnding::CrLf),
                    has_separator: true,
                },
            ]);
            let mut expected = b"X:".to_vec();
            expected.extend_from_slice(&filler(head));
            expected.extend_from_slice(b"\r\n ");
            expected.extend_from_slice(&filler(118usize.saturating_sub(head)));
            expected.extend_from_slice(b"\r\n");
            assert_eq!(rewrite(&document), expected, "folded at {fold_at}");
            assert_eq!(codes(&kept), wanted, "folded at {fold_at}");
        }
    }

    /// A scripted line, the octets it has to be written back as, and the span the one
    /// diagnostic it earns must name.
    type PlacedCase<'a> = (Vec<Scripted>, &'a [u8], (u64, u64));

    /// Four places section 3.1 excludes a control character from, and one it does not.
    #[test]
    fn a_control_character_is_reported_wherever_section_3_1_excludes_it() {
        let cases: Vec<PlacedCase<'_>> = vec![
            (
                vec![name(b"X-BEL"), value(b"a\x07b"), crlf()],
                b"X-BEL:a\x07b\r\n",
                (6, 9),
            ),
            (
                vec![name(b"X-IN\x07NAME"), value(b"v"), crlf()],
                b"X-IN\x07NAME:v\r\n",
                (0, 9),
            ),
            (
                vec![
                    name(b"X-P"),
                    parameter(b"A\x0bB", b"1"),
                    value(b"v"),
                    crlf(),
                ],
                b"X-P;A\x0bB=1:v\r\n",
                (4, 7),
            ),
            (
                vec![
                    name(b"X-NUL"),
                    parameter(b"P", b"a\x00b"),
                    value(b"v"),
                    crlf(),
                ],
                b"X-NUL;P=a\x00b:v\r\n",
                (8, 11),
            ),
        ];
        for (script, expected, at) in cases {
            let (document, kept) = build(script);
            assert_eq!(rewrite(&document), expected, "{expected:?}");
            assert_eq!(
                codes(&kept),
                vec![DiagnosticCode::ControlCharacterInText],
                "{expected:?}"
            );
            assert_eq!(extent_of(&kept, 0), at, "{expected:?}");
        }
    }

    #[test]
    fn a_horizontal_tab_is_whitespace_and_not_a_control_character() {
        let (document, kept) = build(vec![name(b"SUMMARY"), value(b"a\tb"), crlf()]);
        assert_eq!(rewrite(&document), b"SUMMARY:a\tb\r\n");
        assert!(kept.is_empty(), "section 3.1 counts HTAB as whitespace");
    }

    /// A shape section 3.2 has no grammar for, kept exactly as it arrived so that it can be.
    #[test]
    fn a_parameter_with_no_value_is_reported_and_written_back_as_it_arrived() {
        let (document, kept) = build(vec![
            name(b"X-SHAPES"),
            flag(b"NOVALUE"),
            parameter(b"EMPTY", b""),
            value(b"v"),
            crlf(),
        ]);
        assert_eq!(rewrite(&document), b"X-SHAPES;NOVALUE;EMPTY=:v\r\n");
        assert_eq!(codes(&kept), vec![DiagnosticCode::ParameterWithoutValue]);
        assert_eq!(
            extent_of(&kept, 0),
            (9, 16),
            "the name, which is the whole of what arrived; `EMPTY=` earns nothing"
        );
    }

    /// The case the reader used to describe only as a missing `:`. Both are true and both are
    /// reported: the quote was never closed, and the line really has no separator left.
    #[test]
    fn an_unterminated_quote_is_named_beside_the_separator_it_swallowed() {
        let (document, kept) = build(vec![
            name(b"X-UNTERMINATED"),
            parameter(b"CN", b"\"never closed:still the header"),
            colonless(Some(LineEnding::CrLf)),
        ]);
        assert_eq!(
            rewrite(&document),
            b"X-UNTERMINATED;CN=\"never closed:still the header\r\n"
        );
        assert_eq!(
            codes(&kept),
            vec![
                DiagnosticCode::UnterminatedQuotedParameter,
                DiagnosticCode::MissingValueSeparator,
            ]
        );
        assert_eq!(
            extent_of(&kept, 0),
            (18, 19),
            "the octet that was never closed is the quote that opened"
        );
    }

    #[test]
    fn a_closed_quote_earns_nothing_at_all() {
        let (document, kept) = build(vec![
            name(b"ATTENDEE"),
            parameter(b"CN", b"\"Doe, John; the third: esq.\""),
            value(b"mailto:d@example.test"),
            crlf(),
        ]);
        assert_eq!(
            rewrite(&document),
            b"ATTENDEE;CN=\"Doe, John; the third: esq.\":mailto:d@example.test\r\n"
        );
        assert!(kept.is_empty());
    }

    /// A parameter value as written, and the span of the note it earns — `None` where RFC 6868
    /// gives every pair in it a meaning and nothing is reported.
    type CaretCase<'a> = (&'a [u8], Option<(u64, u64)>);

    /// RFC 6868 section 2 requires an undefined pair be left as it is, so this is the one code
    /// this unit reports as a note: nothing was violated and nothing was repaired. The value it
    /// points at is the value the caret sits in, quotes and all, because that is the scope the
    /// grammar crate answers over.
    #[test]
    fn an_undefined_caret_pair_is_a_note_over_the_value_that_stays_as_it_is() {
        let cases: Vec<CaretCase<'_>> = vec![
            (b"^n^^^'", None),
            (b"^^x", None),
            (b"trailing ^", None),
            (b"^x undefined", Some((12, 24))),
            (b"\"^n^^^'^x\"", Some((12, 22))),
            (b"100 and ^q and ^z", Some((12, 29))),
        ];
        for (text, wanted) in cases {
            let (document, kept) = build(vec![
                name(b"ATTENDEE"),
                parameter(b"CN", text),
                value(b"mailto:a@example.test"),
                crlf(),
            ]);
            let mut expected = b"ATTENDEE;CN=".to_vec();
            expected.extend_from_slice(text);
            expected.extend_from_slice(b":mailto:a@example.test\r\n");
            assert_eq!(rewrite(&document), expected, "{text:?}");
            let Some(span) = wanted else {
                assert!(kept.is_empty(), "{text:?}");
                continue;
            };
            assert_eq!(
                codes(&kept),
                vec![DiagnosticCode::UndefinedCaretEscape],
                "{text:?}"
            );
            assert_eq!(kept.first().unwrap().severity(), Severity::Note, "{text:?}");
            assert_eq!(extent_of(&kept, 0), span, "{text:?}");
        }
    }

    /// The claim every one of these diagnostics rests on: an offset into the caller's octets is
    /// not an offset into the unfolded line, and a fold is exactly the difference.
    #[test]
    fn an_offset_inside_a_folded_line_counts_the_octets_the_folds_spent() {
        let folds = vec![FoldPoint {
            offset: 3,
            tab: false,
            newline: LineEnding::CrLf,
        }];
        let (document, kept) = build(vec![
            name(b"X"),
            value(b"abc\x07"),
            Scripted::EndOfLine {
                folds,
                ending: Some(LineEnding::CrLf),
                has_separator: true,
            },
        ]);
        assert_eq!(rewrite(&document), b"X:a\r\n bc\x07\r\n");
        assert_eq!(codes(&kept), vec![DiagnosticCode::ControlCharacterInText]);
        assert_eq!(
            extent_of(&kept, 0),
            (2, 9),
            "the value begins before the fold and ends after it"
        );
    }

    /// Every one of these is a diagnostic and none of them is a refusal, which is the half of
    /// `docs/adr/0001` that a code cannot state on its own.
    #[test]
    fn a_line_wrong_in_six_ways_is_kept_whole_and_diagnosed_six_times() {
        let (document, kept) = build(vec![
            name(b"X-EVERYTHING"),
            flag(b"NOVALUE"),
            parameter(b"CN", b"\"^q\x07"),
            value(&filler(80)),
            eol(None),
        ]);
        let mut expected = b"X-EVERYTHING;NOVALUE;CN=\"^q\x07:".to_vec();
        expected.extend_from_slice(&filler(80));
        assert_eq!(
            rewrite(&document),
            expected,
            "a line nothing accepted is a line nothing rewrote"
        );
        assert_eq!(
            codes(&kept),
            vec![
                DiagnosticCode::MissingFinalLineBreak,
                DiagnosticCode::LineTooLong,
                DiagnosticCode::ParameterWithoutValue,
                DiagnosticCode::ControlCharacterInText,
                DiagnosticCode::UnterminatedQuotedParameter,
                DiagnosticCode::UndefinedCaretEscape,
            ]
        );
        assert_eq!(
            extent_of(&kept, 1),
            (0, 109),
            "one physical line, and every octet of it is still there"
        );
    }

    #[test]
    fn changing_one_value_leaves_every_other_octet_alone() {
        let (mut document, _) = build(vec![
            name(b"BEGIN"),
            value(b"VEVENT"),
            crlf(),
            name(b"SUMMARY"),
            value(b"before"),
            eol(Some(LineEnding::Lf)),
            name(b"X-VENDOR"),
            parameter(b"X-P", b"\"q\""),
            value(b"kept"),
            crlf(),
            name(b"END"),
            value(b"VEVENT"),
            crlf(),
        ]);
        retitle(&mut document, b"after");
        assert_eq!(
            rewrite(&document),
            b"BEGIN:VEVENT\r\nSUMMARY:after\nX-VENDOR;X-P=\"q\":kept\r\nEND:VEVENT\r\n",
            "the edited line changed, the bare LF beside it and the vendor line did not"
        );
    }
}
