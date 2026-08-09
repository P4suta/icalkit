// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The content-line reader: unfolding, line splitting and parameter lexing in one pass.
//!
//! Specification: RFC 5545 section 3.1, "Content Lines"
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.1>.
//!
//! One pass rather than an unfold stage feeding a separate lexer. Two stages need a second
//! owned copy of the whole input to hand the first stage's output to the second, and the fold
//! positions the serializer needs would have to be carried across the seam anyway. A separate
//! unfold stage is also a second place that decides what a fold is, and the two decisions
//! disagree on the first bare `CR` that arrives followed by a space.
//!
//! What is buffered and what is not is the asymmetry the whole module is arranged around. A
//! name and its parameters are reassembled across folds into a scratch buffer, because they
//! are bounded by [`GrammarLimits::max_header_bytes`] and because a parameter split across a
//! fold would otherwise reach the consumer as two slices it has to join — reintroducing, one
//! layer up, the unfolding this layer exists to perform. A value is never buffered: its chunks
//! are the runs between folds and they borrow the caller's octets, so a 400 MB inline
//! `ATTACH` costs this layer one slice (`docs/adr/0008`).
//!
//! Nothing here judges a calendar. A line with no `:`, a bare `LF`, a quoted parameter value
//! whose closing `DQUOTE` never arrives — each is lexed into tokens that still reconstruct the
//! octets it was given, and each is diagnosed by the consumer, which is where the sink is. The
//! reader has none, and the only failure it can report is a bound the caller stated
//! (`docs/adr/0009`).
//!
//! The token sequence is what a serializer inverts, so the shape is written down once here.
//! Per logical line: one [`Token::Name`], one [`Token::Parameter`] per parameter in the order
//! written, one [`Token::Value`] chunk per run between folds when a `:` was present, and one
//! [`Token::EndOfLine`]. Writing the name, then `;` and each parameter and `=` where
//! `has_value`, then `:` where `has_separator`, then every chunk in order, reproduces the
//! unfolded line. Inserting each fold's terminator and whitespace octet at its recorded offset
//! and appending the recorded terminator reproduces the input exactly, which is the round-trip
//! guarantee of `docs/adr/0001` as this layer can state it.

use alloc::vec::Vec;

use crate::budget::GrammarLimits;
use crate::failure::ParseError;
use crate::syntax::{FoldPoint, LineEnding};
use crate::token::{ContentLineSource, Token};

/// Whether `octet` is whitespace that may introduce a folded continuation line.
///
/// RFC 5545 section 3.1 allows exactly `SP` and `HTAB`, and which one arrived is recorded
/// rather than normalized: producers differ, and rewriting a tab as a space would change a
/// file nobody asked to change.
#[must_use]
pub const fn is_fold_whitespace(octet: u8) -> bool {
    matches!(octet, b' ' | b'\t')
}

/// The terminator beginning at `at`, and the number of octets it occupies.
///
/// A `CR` immediately followed by an `LF` is one terminator and not two. That is the only
/// place the three spellings interact; everywhere downstream a bare `CR` and a bare `LF` are
/// handled exactly as a `CRLF` is, because recording which one arrived is what lets the
/// violation be a diagnostic rather than a rewrite.
fn terminator_at(input: &[u8], at: usize) -> Option<(LineEnding, usize)> {
    match input.get(at) {
        Some(&b'\r') if input.get(at.saturating_add(1)) == Some(&b'\n') => {
            Some((LineEnding::CrLf, 2))
        },
        Some(&b'\r') => Some((LineEnding::Cr, 1)),
        Some(&b'\n') => Some((LineEnding::Lf, 1)),
        _ => None,
    }
}

/// Where the header lexer stands inside one line's name and parameters.
///
/// The quoted state is the one that earns its place. RFC 5545 section 3.2 lets a parameter
/// value be a quoted string, and `DELEGATED-TO="mailto:a","mailto:b":value` puts a `:` inside
/// one — so the octet that ends the header cannot be recognized without knowing whether the
/// reader is inside quotes. A `DQUOTE` opens a quoted string only where a value may begin,
/// which keeps one unbalanced quote in the middle of a `CN` from swallowing the rest of the
/// line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderState {
    /// Inside the property name, before any `;` or `:`.
    Name,
    /// Inside a parameter name, after a `;` and before an `=`.
    ParameterName,
    /// Where a parameter value may begin: after an `=`, or after a `,` inside one.
    ValueStart,
    /// Inside an unquoted parameter value.
    ValueText,
    /// Inside a quoted parameter value, where `;` and `:` are ordinary octets.
    Quoted,
}

/// One parameter's position inside the reassembled header.
///
/// Offsets rather than slices, because the buffer they point into is still growing when they
/// are recorded and a slice would borrow a buffer the next octet may reallocate.
#[derive(Clone, Copy, Debug)]
struct ParameterSpan {
    /// Where the parameter name begins, just past the `;`.
    name_start: usize,
    /// Where the parameter name ends: at the `=`, the next `;`, or the end of the header.
    name_end: usize,
    /// Where the parameter value begins, just past the `=`.
    value_start: usize,
    /// Where the parameter value ends: at the next `;` or the end of the header.
    value_end: usize,
    /// Whether an `=` and a value were present at all.
    has_value: bool,
}

impl ParameterSpan {
    /// A parameter beginning at `at`, so far with an empty name and no value.
    const fn opening(at: usize) -> Self {
        Self {
            name_start: at,
            name_end: at,
            value_start: at,
            value_end: at,
            has_value: false,
        }
    }
}

/// What the reader hands back next, named as positions rather than as borrows.
///
/// The reader has to mutate itself to work out what comes next and then borrow itself to hand
/// the answer over. Naming the answer in owned data keeps those two apart, so a token is
/// always built from a reader that nothing is writing to.
#[derive(Clone, Copy, Debug)]
enum Emit {
    /// The property name, from the scratch buffer.
    Name,
    /// The parameter at this index, from the scratch buffer.
    Parameter(usize),
    /// One run of the value, borrowed from `start .. end` of the input.
    Value {
        /// Offset of the first octet of the chunk.
        start: usize,
        /// Offset one past the last octet of the chunk.
        end: usize,
        /// Whether another chunk of the same value follows.
        more: bool,
    },
    /// The end of the line, from the recorded folds and terminator.
    EndOfLine,
}

/// Which token of the current logical line comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Nothing of the next line has been read yet.
    LineStart,
    /// The header has been read; the name is owed.
    Name,
    /// The parameter at this index is owed, if there is one.
    Parameter(usize),
    /// A value chunk is owed.
    Value,
    /// Only the end of the line is owed.
    Ending,
    /// The input, or the reader's ability to continue past a bound, is finished.
    Done,
}

/// A pull reader over `.ics` octets, yielding one line's tokens at a time.
///
/// Construction reads nothing: every octet is examined during the [`ContentLineSource`] call
/// that needs it, so a caller that stops early has paid for what it read and no more.
#[derive(Debug)]
pub struct ContentLineReader<'a> {
    /// The octets the caller handed in, never copied and never modified.
    input: &'a [u8],
    /// The bounds this reader was constructed under.
    limits: GrammarLimits,
    /// The next octet of the input to examine.
    cursor: usize,
    /// The current line's name and parameters, unfolded, delimiters included.
    scratch: Vec<u8>,
    /// Where each parameter of the current line sits inside `scratch`.
    parameters: Vec<ParameterSpan>,
    /// Where the producer folded the current line, in order.
    folds: Vec<FoldPoint>,
    /// The parameter whose end has not been seen yet.
    pending: Option<ParameterSpan>,
    /// Where the header lexer stands.
    header_state: HeaderState,
    /// Where the property name ends inside `scratch`.
    name_end: usize,
    /// Octets of the current line, unfolded, that have already been accounted for.
    ///
    /// This is what a [`FoldPoint::offset`] is measured in, so it counts the header, the `:`
    /// and every value chunk, and it counts neither the terminators nor the whitespace octets
    /// that folding removes.
    line_offset: u32,
    /// The terminator the current line actually carried.
    ending: Option<LineEnding>,
    /// Whether the current line carried a `:`.
    has_separator: bool,
    /// Which token of the current line comes next.
    stage: Stage,
}

impl<'a> ContentLineReader<'a> {
    /// A reader over `input`, bounded by `limits`.
    #[must_use]
    pub const fn new(input: &'a [u8], limits: GrammarLimits) -> Self {
        Self {
            input,
            limits,
            cursor: 0,
            scratch: Vec::new(),
            parameters: Vec::new(),
            folds: Vec::new(),
            pending: None,
            header_state: HeaderState::Name,
            name_end: 0,
            line_offset: 0,
            ending: None,
            has_separator: false,
            stage: Stage::LineStart,
        }
    }

    /// Work out what comes next, without borrowing anything out.
    fn advance(&mut self) -> Option<Result<Emit, ParseError>> {
        loop {
            // Copied out first, so that an arm may write the next stage back.
            let stage = self.stage;
            match stage {
                Stage::Done => return None,
                Stage::LineStart => match self.start_line() {
                    None => return None,
                    Some(Err(error)) => return Some(Err(error)),
                    Some(Ok(())) => {},
                },
                Stage::Name => {
                    self.stage = Stage::Parameter(0);
                    return Some(Ok(Emit::Name));
                },
                Stage::Parameter(index) => {
                    if index < self.parameters.len() {
                        self.stage = Stage::Parameter(index.saturating_add(1));
                        return Some(Ok(Emit::Parameter(index)));
                    }
                    // A line with no `:` has no value at all, which is not the same claim as
                    // a value of zero octets and must not be delivered as one.
                    self.stage = if self.has_separator {
                        Stage::Value
                    } else {
                        Stage::Ending
                    };
                },
                Stage::Value => return Some(Ok(self.next_value_chunk())),
                Stage::Ending => {
                    self.stage = Stage::LineStart;
                    return Some(Ok(Emit::EndOfLine));
                },
            }
        }
    }

    /// Read the next line's header, or answer that the input is finished.
    ///
    /// A bound crossed here ends the reader. The alternative is to resume at the next
    /// terminator, and there is no honest way to find one: the octets that would be skipped
    /// are exactly the octets that were too large to examine.
    fn start_line(&mut self) -> Option<Result<(), ParseError>> {
        if self.cursor >= self.input.len() {
            self.stage = Stage::Done;
            return None;
        }
        self.begin_line();
        match self.scan_header() {
            Ok(()) => {
                self.stage = Stage::Name;
                Some(Ok(()))
            },
            Err(error) => {
                self.stage = Stage::Done;
                Some(Err(error))
            },
        }
    }

    /// Forget the previous line. Nothing may borrow it: the token that carried its folds
    /// borrowed this reader, so the caller has dropped it to be able to ask for another.
    fn begin_line(&mut self) {
        self.scratch.clear();
        self.parameters.clear();
        self.folds.clear();
        self.pending = None;
        self.header_state = HeaderState::Name;
        self.name_end = 0;
        self.line_offset = 0;
        self.ending = None;
        self.has_separator = false;
    }

    /// Read the name and parameters, stopping at the `:` or at the end of the line.
    fn scan_header(&mut self) -> Result<(), ParseError> {
        loop {
            let Some(&octet) = self.input.get(self.cursor) else {
                self.close_header(false);
                self.ending = None;
                return Ok(());
            };
            if let Some((newline, width)) = terminator_at(self.input, self.cursor) {
                if !self.take_fold(newline, width) {
                    self.close_header(false);
                    self.ending = Some(newline);
                    self.cursor = self.cursor.saturating_add(width);
                    return Ok(());
                }
            } else if octet == b':' && self.header_state != HeaderState::Quoted {
                self.close_header(true);
                self.cursor = self.cursor.saturating_add(1);
                self.advance_line_offset(1);
                return Ok(());
            } else {
                self.append_header_octet(octet)?;
                self.cursor = self.cursor.saturating_add(1);
            }
        }
    }

    /// The next run of the value, which ends at the next fold, terminator, or end of input.
    fn next_value_chunk(&mut self) -> Emit {
        let start = self.cursor;
        let end = self.scan_to_break();
        self.advance_line_offset(end.saturating_sub(start));
        let Some((newline, width)) = terminator_at(self.input, end) else {
            self.ending = None;
            self.stage = Stage::Ending;
            return Emit::Value {
                start,
                end,
                more: false,
            };
        };
        if self.take_fold(newline, width) {
            return Emit::Value {
                start,
                end,
                more: true,
            };
        }
        self.ending = Some(newline);
        self.cursor = end.saturating_add(width);
        self.stage = Stage::Ending;
        Emit::Value {
            start,
            end,
            more: false,
        }
    }

    /// Advance the cursor to the next `CR` or `LF`, or to the end of the input.
    fn scan_to_break(&mut self) -> usize {
        let rest = self.input.get(self.cursor..).unwrap_or(&[]);
        let width = rest
            .iter()
            .position(|&octet| octet == b'\r' || octet == b'\n')
            .unwrap_or(rest.len());
        self.cursor = self.cursor.saturating_add(width);
        self.cursor
    }

    /// Consume a terminator at the cursor as a fold, or leave the cursor where it is.
    ///
    /// Any of the three terminators may introduce a continuation. RFC 5545 section 3.1 spells
    /// a fold `CRLF` followed by one whitespace octet, and a file written with bare `LF`s
    /// folds with bare `LF`s; recording which one arrived is what lets it be written back.
    fn take_fold(&mut self, newline: LineEnding, width: usize) -> bool {
        let after = self.cursor.saturating_add(width);
        let Some(&octet) = self.input.get(after) else {
            return false;
        };
        if !is_fold_whitespace(octet) {
            return false;
        }
        self.folds.push(FoldPoint {
            offset: self.line_offset,
            tab: octet == b'\t',
            newline,
        });
        self.cursor = after.saturating_add(1);
        true
    }

    /// Count `octets` more of the unfolded line.
    ///
    /// Saturating, because a fold past the last offset a [`FoldPoint`] can address cannot be
    /// recorded faithfully and a wrap would place it near the start of the line instead — a
    /// position that is wrong rather than merely unreachable.
    fn advance_line_offset(&mut self, octets: usize) {
        let counted = u32::try_from(octets).unwrap_or(u32::MAX);
        self.line_offset = self.line_offset.saturating_add(counted);
    }

    /// Append one octet to the header and let the header lexer see it.
    fn append_header_octet(&mut self, octet: u8) -> Result<(), ParseError> {
        let ceiling = self.limits.max_header_bytes();
        // Compared as a `u32` rather than casting the ceiling up, so a header longer than a
        // `u32` can count is refused rather than silently compared against a wrapped bound.
        if !u32::try_from(self.scratch.len()).is_ok_and(|used| used < ceiling) {
            return Err(ParseError::HeaderTooLarge { limit: ceiling });
        }
        let at = self.scratch.len();
        self.scratch.push(octet);
        self.advance_line_offset(1);
        self.step_header_state(octet, at)
    }

    /// Move the header lexer over the octet now sitting at `at`.
    fn step_header_state(&mut self, octet: u8, at: usize) -> Result<(), ParseError> {
        if self.header_state == HeaderState::Quoted {
            if octet == b'"' {
                self.header_state = HeaderState::ValueText;
            }
            return Ok(());
        }
        if octet == b';' {
            return self.open_parameter(at);
        }
        self.step_unquoted(octet, at);
        Ok(())
    }

    /// Move the header lexer for an octet that is neither a `;` nor inside quotes.
    fn step_unquoted(&mut self, octet: u8, at: usize) {
        match self.header_state {
            // The name runs until a delimiter the caller already handled, and a quoted value
            // never reaches here: the step above this one answers that state first.
            HeaderState::Name | HeaderState::Quoted => {},
            HeaderState::ParameterName => {
                if octet == b'=' {
                    self.begin_parameter_value(at);
                    self.header_state = HeaderState::ValueStart;
                }
            },
            HeaderState::ValueStart => {
                self.header_state = match octet {
                    b'"' => HeaderState::Quoted,
                    b',' => HeaderState::ValueStart,
                    _ => HeaderState::ValueText,
                };
            },
            HeaderState::ValueText => {
                // A comma separates the values of a multi-valued parameter, and each of them
                // may be quoted independently, so it returns to where a quote may open.
                if octet == b',' {
                    self.header_state = HeaderState::ValueStart;
                }
            },
        }
    }

    /// Close whatever the `;` at `at` ended and open a parameter after it.
    fn open_parameter(&mut self, at: usize) -> Result<(), ParseError> {
        self.close_pending(at);
        let ceiling = self.limits.max_parameters();
        if !u32::try_from(self.parameters.len()).is_ok_and(|used| used < ceiling) {
            return Err(ParseError::TooManyParameters { limit: ceiling });
        }
        self.pending = Some(ParameterSpan::opening(at.saturating_add(1)));
        self.header_state = HeaderState::ParameterName;
        Ok(())
    }

    /// Record that the `=` at `at` ended a parameter name and began its value.
    fn begin_parameter_value(&mut self, at: usize) {
        if let Some(span) = self.pending.as_mut() {
            span.name_end = at;
            span.has_value = true;
            span.value_start = at.saturating_add(1);
            span.value_end = span.value_start;
        }
    }

    /// End the header at the end of the scratch buffer, with or without a separator.
    fn close_header(&mut self, has_separator: bool) {
        let end = self.scratch.len();
        self.close_pending(end);
        self.has_separator = has_separator;
    }

    /// Close the parameter under construction at `end`, or end the name there.
    ///
    /// One helper for both because which of the two is open is the same question: before the
    /// first `;` the octets belong to the name, and after it they belong to a parameter.
    fn close_pending(&mut self, end: usize) {
        if let Some(mut span) = self.pending.take() {
            if span.has_value {
                span.value_end = end;
            } else {
                span.name_end = end;
            }
            self.parameters.push(span);
        } else {
            self.name_end = end;
        }
    }

    /// Build the token an `Emit` names, borrowing the buffers it points into.
    fn materialize(&self, emit: Emit) -> Token<'_> {
        match emit {
            Emit::Name => Token::Name(self.header_slice(0, self.name_end)),
            Emit::Parameter(index) => self.parameter_token(index),
            Emit::Value { start, end, more } => Token::Value {
                bytes: self.input.get(start..end).unwrap_or(&[]),
                more,
            },
            Emit::EndOfLine => Token::EndOfLine {
                folds: &self.folds,
                ending: self.ending,
                has_separator: self.has_separator,
            },
        }
    }

    /// The parameter at `index`, or an empty one if no such parameter was recorded.
    fn parameter_token(&self, index: usize) -> Token<'_> {
        // An index this module did not record cannot exist. Answering with an empty parameter
        // rather than indexing keeps a bug here from becoming a panic inside a caller's parser.
        let Some(span) = self.parameters.get(index) else {
            return Token::Parameter {
                name: &[],
                value: &[],
                has_value: false,
            };
        };
        Token::Parameter {
            name: self.header_slice(span.name_start, span.name_end),
            value: self.header_slice(span.value_start, span.value_end),
            has_value: span.has_value,
        }
    }

    /// The header octets in `start .. end`, or none if that is not a range of them.
    fn header_slice(&self, start: usize, end: usize) -> &[u8] {
        self.scratch.get(start..end).unwrap_or(&[])
    }
}

impl ContentLineSource for ContentLineReader<'_> {
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>> {
        // Bound before the match, so that the write half of the reader is finished with
        // before the read half hands a borrow of it to the caller.
        let step = self.advance();
        match step {
            None => None,
            Some(Err(error)) => Some(Err(error)),
            Some(Ok(emit)) => Some(Ok(self.materialize(emit))),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{ContentLineReader, is_fold_whitespace};
    use crate::budget::GrammarLimits;
    use crate::failure::ParseError;
    use crate::syntax::{FoldPoint, LineEnding};
    use crate::token::{ContentLineSource, Token};

    /// One token, detached from the reader that produced it.
    ///
    /// A token borrows the reader it came from, so two of them cannot be held at once. Owning
    /// the payload is what lets a test state a whole line's expected sequence in one value.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Owned {
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

    /// Every token `input` lexes into, in order.
    ///
    /// `Token` is `#[non_exhaustive]`, but that binds consumers of this crate rather than this
    /// crate, so the match below is exhaustive and a catch-all arm would be dead code. A test
    /// living beside the enum is the one place a new variant should stop compiling.
    fn lex(input: &[u8], limits: GrammarLimits) -> Result<Vec<Owned>, ParseError> {
        let mut reader = ContentLineReader::new(input, limits);
        let mut tokens = Vec::new();
        while let Some(token) = reader.next_token() {
            tokens.push(match token? {
                Token::Name(name) => Owned::Name(name.to_vec()),
                Token::Parameter {
                    name,
                    value,
                    has_value,
                } => Owned::Parameter {
                    name: name.to_vec(),
                    value: value.to_vec(),
                    has_value,
                },
                Token::Value { bytes, more } => Owned::Value {
                    bytes: bytes.to_vec(),
                    more,
                },
                Token::EndOfLine {
                    folds,
                    ending,
                    has_separator,
                } => Owned::EndOfLine {
                    folds: folds.to_vec(),
                    ending,
                    has_separator,
                },
            });
        }
        Ok(tokens)
    }

    /// Write one unfolded line back out with its folds reinserted.
    fn write_folded(out: &mut Vec<u8>, line: &[u8], folds: &[FoldPoint]) {
        let mut written = 0_usize;
        for point in folds {
            let offset = usize::try_from(point.offset).unwrap_or(usize::MAX);
            out.extend_from_slice(line.get(written..offset).unwrap_or(&[]));
            out.extend_from_slice(point.newline.as_bytes());
            out.push(point.whitespace());
            written = offset;
        }
        out.extend_from_slice(line.get(written..).unwrap_or(&[]));
    }

    /// Rebuild the input from its tokens alone, the way a serializer has to.
    ///
    /// This is the round-trip property stated in the terms this layer can state it: no octet
    /// of the original may be reachable only through the input the reader was handed.
    fn rebuild(input: &[u8], limits: GrammarLimits) -> Result<Vec<u8>, ParseError> {
        let mut out: Vec<u8> = Vec::new();
        let mut line: Vec<u8> = Vec::new();
        let mut separated = false;
        for token in lex(input, limits)? {
            match token {
                Owned::Name(name) => {
                    line.clear();
                    separated = false;
                    line.extend_from_slice(&name);
                },
                Owned::Parameter {
                    name,
                    value,
                    has_value,
                } => {
                    line.push(b';');
                    line.extend_from_slice(&name);
                    if has_value {
                        line.push(b'=');
                        line.extend_from_slice(&value);
                    }
                },
                Owned::Value { bytes, .. } => {
                    if !separated {
                        line.push(b':');
                        separated = true;
                    }
                    line.extend_from_slice(&bytes);
                },
                Owned::EndOfLine {
                    folds,
                    ending,
                    has_separator,
                } => {
                    assert_eq!(
                        has_separator, separated,
                        "a separator is claimed exactly when a value was delivered"
                    );
                    write_folded(&mut out, &line, &folds);
                    if let Some(newline) = ending {
                        out.extend_from_slice(newline.as_bytes());
                    }
                },
            }
        }
        Ok(out)
    }

    /// Inputs whose octets must survive being taken apart and put back together.
    ///
    /// Every one of them is either something a real producer writes or something the corpus
    /// of `docs/adr/0006` says a real producer wrote once.
    const ROUND_TRIP_CASES: &[&[u8]] = &[
        b"",
        b"BEGIN:VEVENT\r\n",
        b"X:1",
        b"X",
        b"\r\n",
        b"\r\n\r\n",
        b"FOO\r\n",
        b"DESCRIPTION:Hello\r\n World\r\n",
        b"DESCR\r\n IPTION:x\n",
        b"X:a\r\n\t b\r",
        b"X:\r\n \r\n",
        b"X:a\r\n ",
        b"X:",
        b"ATTENDEE;CN=\"Doe, John\";ROLE=REQ-PARTICIPANT:mailto:a@example.test\r\n",
        b"X;A=\"m:1\",\"m:2\":v\r\n",
        b"X;A=\"oops:v\r\n",
        b"X;:v\r\n",
        b"X;A:v\r\n",
        b"X;A=:v\r\n",
        b"X;A=1;B=2:v;w:z\r\n",
        b"SUMMARY:\xe9\xff\xfe\r\n",
        b"A:1\nB:2\r\nC:3\rD:4",
        b" leading whitespace at the start of a file is not a fold\r\n",
        b"X;A=\"quoted\r\n  continues\":v\r\n",
    ];

    #[test]
    fn every_shape_reconstructs_the_octets_it_was_given() {
        for input in ROUND_TRIP_CASES {
            let rebuilt = rebuild(input, GrammarLimits::DEFAULT).unwrap();
            assert_eq!(rebuilt, *input, "rebuilding {input:?}");
        }
    }

    #[test]
    fn an_empty_input_yields_no_tokens_at_all() {
        assert!(lex(b"", GrammarLimits::DEFAULT).unwrap().is_empty());
    }

    #[test]
    fn a_line_arrives_as_a_name_then_parameters_then_chunks_then_one_ending() {
        let tokens = lex(b"DTSTART;TZID=UTC:20260810\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens,
            vec![
                Owned::Name(b"DTSTART".to_vec()),
                Owned::Parameter {
                    name: b"TZID".to_vec(),
                    value: b"UTC".to_vec(),
                    has_value: true,
                },
                Owned::Value {
                    bytes: b"20260810".to_vec(),
                    more: false,
                },
                Owned::EndOfLine {
                    folds: vec![],
                    ending: Some(LineEnding::CrLf),
                    has_separator: true,
                },
            ]
        );
    }

    #[test]
    fn a_value_arrives_as_one_chunk_per_run_between_folds() {
        let tokens = lex(b"D:0123\r\n 4567\r\n 89\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens.get(1..4),
            Some(
                &[
                    Owned::Value {
                        bytes: b"0123".to_vec(),
                        more: true,
                    },
                    Owned::Value {
                        bytes: b"4567".to_vec(),
                        more: true,
                    },
                    Owned::Value {
                        bytes: b"89".to_vec(),
                        more: false,
                    },
                ][..]
            )
        );
    }

    #[test]
    fn a_fold_offset_counts_unfolded_octets_from_the_first_octet_of_the_name() {
        let tokens = lex(b"D:0123\r\n 4567\r\n\t89\r\n", GrammarLimits::DEFAULT).unwrap();
        let ending = tokens.last().cloned().unwrap();
        assert_eq!(
            ending,
            Owned::EndOfLine {
                // "D:0123" is six octets unfolded, "D:01234567" is ten; the tab is the second
                // continuation's, and it is recorded rather than rewritten as a space.
                folds: vec![
                    FoldPoint {
                        offset: 6,
                        tab: false,
                        newline: LineEnding::CrLf,
                    },
                    FoldPoint {
                        offset: 10,
                        tab: true,
                        newline: LineEnding::CrLf,
                    },
                ],
                ending: Some(LineEnding::CrLf),
                has_separator: true,
            }
        );
    }

    #[test]
    fn a_fold_inside_the_name_is_reassembled_and_still_recorded() {
        let tokens = lex(b"DESCR\r\n IPTION:x\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(tokens.first(), Some(&Owned::Name(b"DESCRIPTION".to_vec())));
        assert_eq!(
            tokens.last(),
            Some(&Owned::EndOfLine {
                folds: vec![FoldPoint {
                    offset: 5,
                    tab: false,
                    newline: LineEnding::CrLf,
                }],
                ending: Some(LineEnding::CrLf),
                has_separator: true,
            })
        );
    }

    #[test]
    fn a_separator_with_nothing_after_it_still_delivers_one_empty_chunk() {
        let tokens = lex(b"X:\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens.get(1),
            Some(&Owned::Value {
                bytes: vec![],
                more: false,
            })
        );
    }

    #[test]
    fn a_line_with_no_separator_delivers_no_chunk_at_all() {
        // Not the same claim as an empty value: a colonless line serializes without a `:`,
        // and a value of zero octets serializes with one.
        let tokens = lex(b"X\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens,
            vec![
                Owned::Name(b"X".to_vec()),
                Owned::EndOfLine {
                    folds: vec![],
                    ending: Some(LineEnding::CrLf),
                    has_separator: false,
                },
            ]
        );
    }

    #[test]
    fn a_blank_line_is_a_line_with_an_empty_name() {
        let tokens = lex(b"\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens,
            vec![
                Owned::Name(vec![]),
                Owned::EndOfLine {
                    folds: vec![],
                    ending: Some(LineEnding::Lf),
                    has_separator: false,
                },
            ]
        );
    }

    #[test]
    fn a_final_line_without_a_terminator_reports_that_it_had_none() {
        let tokens = lex(b"X:1", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens.last(),
            Some(&Owned::EndOfLine {
                folds: vec![],
                ending: None,
                has_separator: true,
            })
        );
    }

    #[test]
    fn each_terminator_is_recorded_as_the_one_that_arrived() {
        let tokens = lex(b"A:1\nB:2\r\nC:3\r", GrammarLimits::DEFAULT).unwrap();
        let mut endings: Vec<Option<LineEnding>> = Vec::new();
        for token in &tokens {
            if let Owned::EndOfLine { ending, .. } = *token {
                endings.push(ending);
            }
        }
        assert_eq!(
            endings,
            vec![
                Some(LineEnding::Lf),
                Some(LineEnding::CrLf),
                Some(LineEnding::Cr),
            ]
        );
    }

    #[test]
    fn a_parameter_value_keeps_the_quotes_and_the_colons_it_was_written_with() {
        let tokens = lex(
            b"ATTENDEE;DELEGATED-TO=\"mailto:a\",\"mailto:b\":mailto:c\r\n",
            GrammarLimits::DEFAULT,
        )
        .unwrap();
        assert_eq!(
            tokens.get(1),
            Some(&Owned::Parameter {
                name: b"DELEGATED-TO".to_vec(),
                value: b"\"mailto:a\",\"mailto:b\"".to_vec(),
                has_value: true,
            })
        );
        assert_eq!(
            tokens.get(2),
            Some(&Owned::Value {
                bytes: b"mailto:c".to_vec(),
                more: false,
            })
        );
    }

    #[test]
    fn a_parameter_with_no_equals_sign_says_so_rather_than_inventing_a_value() {
        let tokens = lex(b"X;A;B=:v\r\n", GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens.get(1..3),
            Some(
                &[
                    Owned::Parameter {
                        name: b"A".to_vec(),
                        value: vec![],
                        has_value: false,
                    },
                    Owned::Parameter {
                        name: b"B".to_vec(),
                        value: vec![],
                        has_value: true,
                    },
                ][..]
            )
        );
    }

    #[test]
    fn a_line_a_consumer_has_to_diagnose_is_still_lexed_into_tokens() {
        // An unterminated quote, a bare `LF` and a missing `:` are three violations of
        // RFC 5545 section 3.1 in one line. None of them is this layer's to report — it has
        // no sink, and a reader that refused them would discard the rest of the file. What it
        // owes instead is a sequence the original octets can be rebuilt from.
        let malformed: &[u8] = b"X;A=\"oops\n";
        let tokens = lex(malformed, GrammarLimits::DEFAULT).unwrap();
        assert_eq!(
            tokens,
            vec![
                Owned::Name(b"X".to_vec()),
                Owned::Parameter {
                    name: b"A".to_vec(),
                    value: b"\"oops".to_vec(),
                    has_value: true,
                },
                Owned::EndOfLine {
                    folds: vec![],
                    ending: Some(LineEnding::Lf),
                    has_separator: false,
                },
            ]
        );
        assert_eq!(
            rebuild(malformed, GrammarLimits::DEFAULT).unwrap(),
            malformed
        );
    }

    #[test]
    fn octets_that_are_not_utf8_are_never_looked_at_as_text() {
        // An orphaned continuation octet in a parameter and a lone lead octet in the value:
        // the layer that must never reject a calendar is not the layer that demands UTF-8.
        let input: &[u8] = b"SUMMARY;X-P=\x80\x81:\xe9\xff\r\n";
        assert_eq!(rebuild(input, GrammarLimits::DEFAULT).unwrap(), input);
    }

    #[test]
    fn the_largest_header_the_bounds_allow_is_read_whole() {
        let limits = GrammarLimits::DEFAULT
            .with_max_header_bytes(9)
            .with_max_parameters(2);
        // "N;A=1;B=2" is nine octets and two parameters: both bounds are met exactly.
        let input: &[u8] = b"N;A=1;B=2:value\r\n";
        assert_eq!(rebuild(input, limits).unwrap(), input);
    }

    #[test]
    fn one_octet_past_the_header_ceiling_is_refused() {
        let limits = GrammarLimits::DEFAULT.with_max_header_bytes(8);
        assert_eq!(
            rebuild(b"ABCDEFGH:v\r\n", limits).unwrap(),
            b"ABCDEFGH:v\r\n"
        );
        assert_eq!(
            rebuild(b"ABCDEFGHI:v\r\n", limits),
            Err(ParseError::HeaderTooLarge { limit: 8 })
        );
    }

    #[test]
    fn a_header_folded_past_the_ceiling_is_refused_at_the_octet_that_crosses_it() {
        // Folding is not a way around the bound: the ceiling counts the reassembled header,
        // not the longest physical line.
        let limits = GrammarLimits::DEFAULT.with_max_header_bytes(4);
        assert_eq!(
            rebuild(b"AB\r\n CDE:v\r\n", limits),
            Err(ParseError::HeaderTooLarge { limit: 4 })
        );
    }

    #[test]
    fn one_parameter_past_the_parameter_ceiling_is_refused() {
        let limits = GrammarLimits::DEFAULT.with_max_parameters(2);
        assert_eq!(
            rebuild(b"X;A=1;B=2:v\r\n", limits).unwrap(),
            b"X;A=1;B=2:v\r\n"
        );
        // The count is reported, not the octet ceiling: raising the wrong one leaves the
        // refusal exactly where it was.
        assert_eq!(
            rebuild(b"X;A=1;B=2;C=3:v\r\n", limits),
            Err(ParseError::TooManyParameters { limit: 2 })
        );
    }

    #[test]
    fn a_reader_that_has_reported_a_bound_stops_rather_than_resuming() {
        // The octets that would be skipped to find the next line are exactly the octets that
        // were too large to examine, so there is no boundary left to resume from.
        let limits = GrammarLimits::DEFAULT.with_max_header_bytes(2);
        let mut reader = ContentLineReader::new(b"ABC:v\r\nX:1\r\n", limits);
        assert_eq!(
            reader.next_token(),
            Some(Err(ParseError::HeaderTooLarge { limit: 2 }))
        );
        assert!(reader.next_token().is_none());
    }

    #[test]
    fn a_value_is_never_bounded_by_the_header_ceiling() {
        // A value is delivered in chunks the reader never buffers, so the bound that applies
        // to a reassembled header does not apply to it.
        let limits = GrammarLimits::DEFAULT.with_max_header_bytes(2);
        let input: &[u8] = b"X:a value far longer than any header this reader would accept\r\n";
        assert_eq!(rebuild(input, limits).unwrap(), input);
    }

    #[test]
    fn only_space_and_tab_introduce_a_continuation() {
        assert!(is_fold_whitespace(b' '));
        assert!(is_fold_whitespace(b'\t'));
        assert!(!is_fold_whitespace(b'\r'));
        assert!(!is_fold_whitespace(0));
    }
}
