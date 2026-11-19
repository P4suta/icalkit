// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Content-line tokens and folds under RFC 5545 section 3.1.
//! Each logical line yields its name, its parameters in input order, and an end token.
//! When a colon introduces a value, the reader emits the value in slices borrowed from the input.
//! The reader unfolds names and parameters into a buffer bounded by [`GrammarLimits::max_header_bytes`].
//! Fold offsets, whitespace and line endings preserve the original octets for serialization.
//! The consumer diagnoses malformed lines and the reader reports resource limits.

use alloc::vec::Vec;

use super::{ContentLineSource, FoldPoint, GrammarLimits, LineEnding, ParseError, Token};

/// Recognize the `SP` and `HTAB` continuation octets from RFC 5545 section 3.1.
#[must_use]
pub const fn is_fold_whitespace(octet: u8) -> bool {
    matches!(octet, b' ' | b'\t')
}

/// Read one `CRLF`, bare `CR` or bare `LF` and return its length in octets.
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

/// Track quoted parameter values under RFC 5545 section 3.2.
/// A quote opens only at the start of a parameter value or list item.
/// Inside the quoted value, colon and semicolon are data.
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

impl HeaderState {
    const fn after_octet(self, octet: u8) -> Self {
        match (self, octet) {
            (Self::ParameterName, b'=') | (Self::ValueStart | Self::ValueText, b',') => {
                Self::ValueStart
            },
            (Self::ValueStart, b'"') => Self::Quoted,
            (Self::ValueStart, _) | (Self::Quoted, b'"') => Self::ValueText,
            _ => self,
        }
    }
}

/// Whether the next delimiter ends a parameter after its raw value octets.
#[must_use]
pub fn parameter_value_accepts_delimiter(value: &[u8]) -> bool {
    parameter_value_open_quote(value).is_none()
}

/// The unmatched opening quote's offset under the header lexer's transitions.
#[must_use]
pub fn parameter_value_open_quote(value: &[u8]) -> Option<usize> {
    let mut state = HeaderState::ValueStart;
    let mut opening = None;
    for (offset, &octet) in value.iter().enumerate() {
        let next = state.after_octet(octet);
        if next == HeaderState::Quoted && state != HeaderState::Quoted {
            opening = Some(offset);
        }
        state = next;
    }
    if state == HeaderState::Quoted {
        opening
    } else {
        None
    }
}

/// Offsets of one parameter in the growing header buffer.
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
/// The reader has to mutate itself to work out what comes next and then borrow itself to hand the answer over.
/// Naming the answer in owned data keeps those two apart, so a token is always built from a reader that nothing is writing to.
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
    /// The recorded folds and line terminator.
    EndOfLine,
}

/// Which token of the current logical line comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Start reading the next header.
    LineStart,
    /// Emit the name from the header.
    Name,
    /// Emit a parameter by index.
    Parameter(usize),
    /// Read a value chunk.
    Value,
    /// Emit the line ending.
    Ending,
    /// Stop at end of input or after a limit error.
    Done,
}

/// Read content-line tokens on demand from borrowed input.
#[derive(Debug)]
pub struct ContentLineReader<'a> {
    /// Borrowed input octets.
    input: &'a [u8],
    /// Caller-supplied grammar limits.
    limits: GrammarLimits,
    /// The next octet of the input to examine.
    cursor: usize,
    /// The current line's name and parameters, unfolded, delimiters included.
    scratch: Vec<u8>,
    /// Where each parameter of the current line sits inside `scratch`.
    parameters: Vec<ParameterSpan>,
    /// Where the producer folded the current line, in order.
    folds: Vec<FoldPoint>,
    /// The unfinished parameter.
    pending: Option<ParameterSpan>,
    /// Where the header lexer stands.
    header_state: HeaderState,
    /// Where the property name ends inside `scratch`.
    name_end: usize,
    /// [`FoldPoint::offset`] counts unfolded octets in the header, colon, and value.
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
                    // A colon introduces a value even when the value is empty.
                    self.stage = if self.has_separator {
                        Stage::Value
                    } else {
                        Stage::Ending
                    };
                },
                Stage::Value => {
                    return Some(self.next_value_chunk().inspect_err(|_bound| {
                        self.stage = Stage::Done;
                    }));
                },
                Stage::Ending => {
                    self.stage = Stage::LineStart;
                    return Some(Ok(Emit::EndOfLine));
                },
            }
        }
    }

    /// Read one header or stop at end of input.
    /// A grammar limit error ends tokenization.
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

    /// Forget the previous line.
    /// Nothing may borrow it: the token that carried its folds borrowed this reader, so the caller has dropped it to be able to ask for another.
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
                if !self.take_fold(newline, width)? {
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
    fn next_value_chunk(&mut self) -> Result<Emit, ParseError> {
        let start = self.cursor;
        let end = self.scan_to_break();
        self.advance_line_offset(end.saturating_sub(start));
        let Some((newline, width)) = terminator_at(self.input, end) else {
            self.ending = None;
            self.stage = Stage::Ending;
            return Ok(Emit::Value {
                start,
                end,
                more: false,
            });
        };
        if self.take_fold(newline, width)? {
            return Ok(Emit::Value {
                start,
                end,
                more: true,
            });
        }
        self.ending = Some(newline);
        self.cursor = end.saturating_add(width);
        self.stage = Stage::Ending;
        Ok(Emit::Value {
            start,
            end,
            more: false,
        })
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

    /// Consume a fold and record its original terminator and whitespace.
    /// Recovery accepts a bare `LF` or `CR` where RFC 5545 section 3.1 specifies `CRLF`.
    /// [`GrammarLimits::max_folds_per_line`] bounds the record count across the header and value.
    fn take_fold(&mut self, newline: LineEnding, width: usize) -> Result<bool, ParseError> {
        let after = self.cursor.saturating_add(width);
        let Some(&octet) = self.input.get(after) else {
            return Ok(false);
        };
        if !is_fold_whitespace(octet) {
            return Ok(false);
        }
        let ceiling = self.limits.max_folds_per_line();
        if !u32::try_from(self.folds.len()).is_ok_and(|held| held < ceiling) {
            return Err(ParseError::TooManyFolds { limit: ceiling });
        }
        self.folds.push(FoldPoint {
            offset: self.line_offset,
            tab: octet == b'\t',
            newline,
        });
        self.cursor = after.saturating_add(1);
        Ok(true)
    }

    /// Count unfolded octets, saturating at the largest [`FoldPoint`] offset.
    fn advance_line_offset(&mut self, octets: usize) {
        let counted = u32::try_from(octets).unwrap_or(u32::MAX);
        self.line_offset = self.line_offset.saturating_add(counted);
    }

    /// Append one octet to the header and let the header lexer see it.
    fn append_header_octet(&mut self, octet: u8) -> Result<(), ParseError> {
        let ceiling = self.limits.max_header_bytes();
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
        if self.header_state != HeaderState::Quoted && octet == b';' {
            return self.open_parameter(at);
        }
        if self.header_state == HeaderState::ParameterName && octet == b'=' {
            self.begin_parameter_value(at);
        }
        self.header_state = self.header_state.after_octet(octet);
        Ok(())
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

    /// Record whether the header ends with a colon.
    fn close_header(&mut self, has_separator: bool) {
        let end = self.scratch.len();
        self.close_pending(end);
        self.has_separator = has_separator;
    }

    /// Close the pending parameter or record the end of the property name.
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

    /// Return the indexed parameter or an empty parameter for an unknown index.
    fn parameter_token(&self, index: usize) -> Token<'_> {
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

    /// Borrow a header range or return an empty slice for an invalid range.
    fn header_slice(&self, start: usize, end: usize) -> &[u8] {
        self.scratch.get(start..end).unwrap_or(&[])
    }
}

impl ContentLineSource for ContentLineReader<'_> {
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>> {
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

    use super::{
        ContentLineReader, ContentLineSource, FoldPoint, GrammarLimits, LineEnding, ParseError,
        Token, is_fold_whitespace,
    };

    /// Owned token for comparing complete token sequences.
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
    /// `Token` is `#[non_exhaustive]`, but that binds consumers of this crate rather than this crate, so the match below is exhaustive and a catch-all arm would be dead code.
    /// A test living beside the enum is the one place a new variant should stop compiling.
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

    /// Reconstruct the original octets from tokens, fold offsets, and line endings.
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

    /// Input lines for byte-for-byte reconstruction.
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
        assert_eq!(lex(b"", GrammarLimits::DEFAULT).unwrap().as_slice(), []);
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
        // An unclosed quote, a bare `LF` and a missing colon must all survive reconstruction.
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
        let input: &[u8] = b"SUMMARY;X-P=\x80\x81:\xe9\xff\r\n";
        assert_eq!(rebuild(input, GrammarLimits::DEFAULT).unwrap(), input);
    }

    #[test]
    fn the_largest_header_the_bounds_allow_is_read_whole() {
        let limits = GrammarLimits::DEFAULT
            .with_max_header_bytes(9)
            .with_max_parameters(2);
        // The header holds nine octets and two parameters.
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
        assert_eq!(
            rebuild(b"X;A=1;B=2;C=3:v\r\n", limits),
            Err(ParseError::TooManyParameters { limit: 2 })
        );
    }

    #[test]
    fn a_reader_that_has_reported_a_bound_stops_rather_than_resuming() {
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
        let limits = GrammarLimits::DEFAULT.with_max_header_bytes(2);
        let input: &[u8] = b"X:a value far longer than any header this reader would accept\r\n";
        assert_eq!(rebuild(input, limits).unwrap(), input);
    }

    #[test]
    fn one_fold_past_the_per_line_ceiling_is_refused() {
        let limits = GrammarLimits::DEFAULT.with_max_folds_per_line(2);
        assert_eq!(
            rebuild(b"X:a\r\n b\r\n c\r\n", limits).unwrap(),
            b"X:a\r\n b\r\n c\r\n"
        );
        assert_eq!(
            rebuild(b"X:a\r\n b\r\n c\r\n d\r\n", limits),
            Err(ParseError::TooManyFolds { limit: 2 })
        );
        assert_eq!(
            rebuild(b"X-A\r\n B\r\n C\r\n D:v\r\n", limits),
            Err(ParseError::TooManyFolds { limit: 2 }),
            "a header folded past the bound is refused by the same count"
        );
    }

    #[test]
    fn a_line_of_nothing_but_continuations_is_refused_at_the_fold_that_crosses_the_bound() {
        let limits = GrammarLimits::DEFAULT.with_max_folds_per_line(8);
        let mut bomb: Vec<u8> = Vec::from(&b"X:"[..]);
        for _ in 0..100_u32 {
            bomb.extend_from_slice(b"\n ");
        }
        bomb.extend_from_slice(b"v\r\n");
        assert_eq!(
            rebuild(&bomb, limits),
            Err(ParseError::TooManyFolds { limit: 8 })
        );
    }

    #[test]
    fn only_space_and_tab_introduce_a_continuation() {
        assert!(is_fold_whitespace(b' '));
        assert!(is_fold_whitespace(b'\t'));
        assert!(!is_fold_whitespace(b'\r'));
        assert!(!is_fold_whitespace(0));
    }
}
