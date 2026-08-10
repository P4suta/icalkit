// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writing the tree back out, fold for fold.
//!
//! Specification: RFC 5545 section 3.1, "Content Lines"
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.1>.
//!
//! This is the half of `docs/adr/0001` that can be measured rather than asserted: parse then
//! serialize is the input octet for octet, or the claim is false. So nothing here decides
//! anything. The order is the tree's, the spelling is the producer's, a parameter value keeps
//! the `DQUOTE`s it arrived with, and a fold is written where the producer put it — at 73
//! octets, or at 76, or with an `HTAB`, or not at all. Those positions were recorded through
//! the unfold precisely so that writing could stop guessing at them.
//!
//! One kind of line is written differently from how it arrived, and the data says which. A
//! write through the mutation guard replaces the text its folds were positions into and marks
//! the layout for refolding; such a line is broken at `REFOLD_WIDTH` octets and continued with
//! `SP`, backing off to the previous codepoint boundary rather than splitting one. Splitting
//! is legal under section 3.1, which counts octets and not characters, and it is also the
//! thing that breaks every naive consumer — so this crate accepts it on the way in and
//! declines to author it on the way out.
//!
//! Serialization cannot refuse. It reports no diagnostic and has no failure of its own; the
//! only error it returns is the sink's. A calendar that violated the specification on the way
//! in violates it identically on the way out, because the violation was reported when it was
//! read and rewriting it here would turn a diagnostic into a repair (`docs/adr/0009`).

use alloc::vec::Vec;
use core::{mem, slice};

use ical_grammar::{FoldPoint, LineEnding, LineLayout};

use crate::output::Writer;
use crate::tree::{Boundary, Document, Item, Property};

/// Octets of line content this crate puts on one physical line when it folds a line itself.
///
/// RFC 5545 section 3.1 bounds a physical line at 75 octets excluding the terminator, and a
/// continuation spends one of those on the whitespace that introduces it. Leaving that octet
/// unclaimed on every line, first and continuation alike, lets one number describe both.
const REFOLD_WIDTH: usize = 74;

/// The most octets a UTF-8 codepoint can carry after its leading one.
const MAX_CONTINUATION_OCTETS: usize = 3;

impl Document {
    /// Write this document to `out`, octet for octet as it was read.
    ///
    /// One walk of the tree in [`items`](Self::items) order, with no decision taken along the
    /// way: nothing is reordered, deduplicated, uppercased, requoted or repaired, and a line
    /// whose layout was preserved is written where its producer folded it.
    ///
    /// # Errors
    ///
    /// Only what the sink reports. Serialization has no failure of its own and hands back no
    /// diagnostic: a writer that could refuse would make the round trip conditional on the
    /// document being well formed, and the documents this crate exists for are not.
    pub fn serialize<W: Writer + ?Sized>(&self, out: &mut W) -> Result<(), W::Error> {
        write_items(self.items(), out)
    }

    /// The document's octets, in a buffer of their own.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        match self.serialize(&mut buffer) {
            Ok(()) => buffer,
            // A growable buffer reports `Infallible`, which has no value to bind, so this arm
            // is a type-level statement that the failure cannot arise rather than a fallback.
            Err(never) => match never {},
        }
    }
}

/// A component whose `BEGIN` has been written and whose entries are still being walked.
struct OpenComponent<'a> {
    /// The entries of the component *above* this one, not yet reached.
    rest: slice::Iter<'a, Item>,
    /// The `END` line to write once this component's entries run out, if one ever arrived.
    end: Option<&'a Boundary>,
}

/// Write each entry in order, whichever kind it is.
///
/// Nesting is walked with an explicit stack rather than by recursion. Recursion here was
/// justified by the depth bound the tree is built under, and that bound is
/// `Limits::max_component_depth` — a `u16` a caller raises through a public builder, while the
/// stack gives out several thousand frames sooner. Sixteen thousand nested components then
/// parse cleanly and abort the process on the way out, which is not a failure a caller can
/// catch or a server can survive. The stack this allocates is one entry per open component,
/// which is memory the tree it is walking already holds (`docs/adr/0007`).
fn write_items<W: Writer + ?Sized>(items: &[Item], sink: &mut W) -> Result<(), W::Error> {
    let mut open: Vec<OpenComponent<'_>> = Vec::new();
    let mut cursor = items.iter();
    loop {
        let Some(entry) = cursor.next() else {
            // This component's entries are done: close it and carry on where its parent was.
            let Some(finished) = open.pop() else {
                return Ok(());
            };
            if let Some(closing) = finished.end {
                write_boundary(closing, sink)?;
            }
            cursor = finished.rest;
            continue;
        };
        match entry {
            Item::Property(property) => write_property(property, sink)?,
            Item::Component(component) => {
                write_boundary(component.begin(), sink)?;
                open.push(OpenComponent {
                    rest: mem::replace(&mut cursor, component.items().iter()),
                    end: component.end(),
                });
            },
        }
    }
}

/// Write one `BEGIN` or `END` line in the spelling and the case it arrived in.
fn write_boundary<W: Writer + ?Sized>(boundary: &Boundary, sink: &mut W) -> Result<(), W::Error> {
    let layout = boundary.layout();
    let mut line = LineWriter::new(sink, layout);
    line.push(boundary.keyword().as_bytes())?;
    if layout.has_separator() {
        line.push(b":")?;
    }
    line.push(boundary.name().as_bytes())?;
    line.finish()
}

/// Write one content line: the name, the parameters in order, the `:` if one was there, and
/// the value.
///
/// A parameter that arrived with no `=` is written with none, and a value that arrived quoted
/// keeps its `DQUOTE`s, because both are stored as the producer wrote them and neither is this
/// crate's to correct.
fn write_property<W: Writer + ?Sized>(property: &Property, sink: &mut W) -> Result<(), W::Error> {
    let layout = property.layout();
    let mut line = LineWriter::new(sink, layout);
    line.push(property.name().as_bytes())?;
    for parameter in property.parameters() {
        line.push(b";")?;
        line.push(parameter.name().as_bytes())?;
        if parameter.has_value() {
            line.push(b"=")?;
            line.push(parameter.value().as_bytes())?;
        }
    }
    if layout.has_separator() {
        line.push(b":")?;
    }
    line.push(property.value_text().as_bytes())?;
    line.finish()
}

/// One content line being written, and the fold decisions that apply to it.
///
/// The octet counter runs from the first octet of the name, because that is where
/// `FoldPoint::offset` is counted from. One number then addresses the name, the parameters and
/// the value uniformly, and the code assembling the line never has to know which of the three
/// a recorded fold fell inside.
struct LineWriter<'a, W: ?Sized> {
    /// Where the octets go.
    sink: &'a mut W,
    /// The recorded folds this line has not reached yet, in order.
    pending: &'a [FoldPoint],
    /// The terminator to write at the end, or `None` for a line that carried none.
    ending: Option<LineEnding>,
    /// Whether the recorded folds were discarded and this line is broken at the canonical
    /// width instead. Set by a write, never by a read.
    refold: bool,
    /// Octets of the unfolded line written so far, counted from the first octet of the name.
    position: usize,
    /// Octets on the current physical line, not counting the whitespace of a continuation.
    column: usize,
}

impl<'a, W: Writer + ?Sized> LineWriter<'a, W> {
    /// A writer for one line with the syntax `layout` recorded.
    fn new(sink: &'a mut W, layout: &'a LineLayout) -> Self {
        Self {
            sink,
            pending: layout.folds(),
            ending: layout.ending(),
            refold: layout.is_refolded(),
            position: 0,
            column: 0,
        }
    }

    /// Append one span of the unfolded line: a name, a delimiter, a parameter, or a value.
    ///
    /// Spans arrive in the order they were written and are never buffered, so a value of any
    /// size costs the same here as an empty one.
    fn push(&mut self, bytes: &[u8]) -> Result<(), W::Error> {
        if self.refold {
            self.push_refolded(bytes)
        } else {
            self.push_recorded(bytes)
        }
    }

    /// Append a span, injecting every recorded fold the span reaches.
    ///
    /// A fold whose offset the line never reaches is dropped: an offset past the last octet
    /// addresses text that is not there, and only a hand-built layout can carry one. Two folds
    /// at one offset are both written, which is what a doubly folded line looks like once the
    /// unfold has run.
    fn push_recorded(&mut self, bytes: &[u8]) -> Result<(), W::Error> {
        let mut rest = bytes;
        while let Some((&fold, later)) = self.pending.split_first() {
            let offset = usize::try_from(fold.offset).unwrap_or(usize::MAX);
            // A fold behind the octets already written cannot address anything still to come,
            // so it is injected at the earliest position that does exist.
            let taken = offset.saturating_sub(self.position);
            if taken > rest.len() {
                break;
            }
            let (head, tail) = rest.split_at(taken);
            self.emit(head)?;
            self.pending = later;
            self.break_line(fold.newline, fold.whitespace())?;
            rest = tail;
        }
        self.emit(rest)
    }

    /// Append a span, breaking the line at the canonical width as often as the span needs.
    fn push_refolded(&mut self, bytes: &[u8]) -> Result<(), W::Error> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let room = REFOLD_WIDTH.saturating_sub(self.column);
            if rest.len() <= room {
                return self.emit(rest);
            }
            // With no codepoint boundary inside the allowance, a line that already has octets
            // on it gives the codepoint a fresh line to fit in. A line that has none has just
            // offered a full width and been refused, so these octets are not UTF-8 at all and
            // no cut avoids splitting something; take the whole allowance and move on.
            let cut = match fold_at(rest, room) {
                Some(boundary) => boundary,
                None if self.column > 0 => 0,
                None => room,
            };
            let (head, tail) = rest.split_at(cut);
            self.emit(head)?;
            rest = tail;
            self.break_line(LineEnding::CANONICAL, b' ')?;
        }
        Ok(())
    }

    /// Write a span through to the sink and count what it took.
    fn emit(&mut self, bytes: &[u8]) -> Result<(), W::Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.sink.write_bytes(bytes)?;
        // A line long enough to saturate either counter is longer than addressable memory; the
        // saturating form is here so the arithmetic is total, not because it can be reached.
        self.position = self.position.saturating_add(bytes.len());
        self.column = self.column.saturating_add(bytes.len());
        Ok(())
    }

    /// End the physical line and open its continuation.
    ///
    /// Neither the terminator nor the whitespace is part of the unfolded line, so neither is
    /// counted: `position` addresses the same octets the recorded offsets do.
    fn break_line(&mut self, newline: LineEnding, whitespace: u8) -> Result<(), W::Error> {
        self.sink.write_bytes(newline.as_bytes())?;
        self.sink.write_bytes(&[whitespace])?;
        self.column = 0;
        Ok(())
    }

    /// Terminate the line, if its producer terminated it.
    ///
    /// A final line that arrived with no terminator is written back with none. Appending one
    /// would be this crate adding an octet the file did not have, which is the same class of
    /// change as dropping one.
    fn finish(self) -> Result<(), W::Error> {
        match self.ending {
            Some(terminator) => self.sink.write_bytes(terminator.as_bytes()),
            None => Ok(()),
        }
    }
}

/// The last position at or before `room` where a canonical fold may cut, if there is one.
///
/// `None` means the whole allowance sits inside one codepoint's tail. The caller decides what
/// to do about that, because the answer depends on whether the physical line has room left to
/// offer at all.
fn fold_at(bytes: &[u8], room: usize) -> Option<usize> {
    let floor = room.saturating_sub(MAX_CONTINUATION_OCTETS);
    let mut cut = room;
    loop {
        if cut == 0 {
            return None;
        }
        if bytes
            .get(cut)
            .copied()
            .is_none_or(|octet| !is_continuation_octet(octet))
        {
            return Some(cut);
        }
        // Backing off further than a codepoint can reach would be searching for a boundary
        // these octets do not have.
        if cut == floor {
            return None;
        }
        cut = cut.saturating_sub(1);
    }
}

/// Whether this octet continues a UTF-8 codepoint rather than starting one.
const fn is_continuation_octet(octet: u8) -> bool {
    matches!(octet, 0x80..=0xBF)
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::convert::Infallible;

    use ical_grammar::{FoldPoint, LineEnding, LineLayout};

    use super::{REFOLD_WIDTH, fold_at};
    use crate::octets::RawText;
    use crate::output::Writer;
    use crate::tree::{Boundary, Component, Document, Item, Parameter, Property};

    /// A fold at `offset`, indented with `SP` after a `CRLF`, as most producers write one.
    fn fold(offset: u32) -> FoldPoint {
        FoldPoint {
            offset,
            tab: false,
            newline: LineEnding::CrLf,
        }
    }

    /// The layout of a `CRLF`-terminated line with a `:`, folded at each of `offsets`.
    fn folded(offsets: &[u32]) -> LineLayout {
        let points = offsets.iter().copied().map(fold).collect();
        LineLayout::preserved(points, Some(LineEnding::CrLf), true)
    }

    /// The layout of an unfolded line with a `:` and the given terminator.
    fn ended(ending: Option<LineEnding>) -> LineLayout {
        LineLayout::preserved(Vec::new(), ending, true)
    }

    /// The layout of a line that carried no `:` at all: junk, or a blank line.
    fn colonless() -> LineLayout {
        LineLayout::preserved(Vec::new(), Some(LineEnding::CrLf), false)
    }

    fn parameter(name: &[u8], value: &[u8]) -> Parameter {
        Parameter::new(RawText::from_bytes(name), RawText::from_bytes(value))
    }

    fn valueless(name: &[u8]) -> Parameter {
        Parameter::without_value(RawText::from_bytes(name))
    }

    fn line(name: &[u8], parameters: Vec<Parameter>, value: &[u8], layout: LineLayout) -> Item {
        Item::Property(Property::new(
            RawText::from_bytes(name),
            parameters,
            RawText::from_bytes(value),
            layout,
        ))
    }

    /// A property with no parameters, on an ordinary `CRLF`-terminated line.
    fn plain(name: &[u8], value: &[u8]) -> Item {
        line(name, Vec::new(), value, folded(&[]))
    }

    fn boundary(keyword: &[u8], name: &[u8]) -> Boundary {
        Boundary::new(
            RawText::from_bytes(keyword),
            RawText::from_bytes(name),
            folded(&[]),
        )
    }

    fn nested(name: &[u8], items: Vec<Item>) -> Item {
        Item::Component(Component::new(
            boundary(b"BEGIN", name),
            items,
            Some(boundary(b"END", name)),
        ))
    }

    /// Undo RFC 5545 folding, so a written line can be compared against what it says.
    fn unfold(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while let Some((&first, tail)) = rest.split_first() {
            let continued = first == b'\r'
                && tail.first().copied() == Some(b'\n')
                && matches!(tail.get(1).copied(), Some(b' ' | b'\t'));
            if continued {
                rest = tail.get(2..).unwrap_or_default();
                continue;
            }
            out.push(first);
            rest = tail;
        }
        out
    }

    /// The widest physical line in octets, terminators excluded.
    fn widest_line(bytes: &[u8]) -> usize {
        let mut widest = 0_usize;
        let mut width = 0_usize;
        for &octet in bytes {
            if octet == b'\r' || octet == b'\n' {
                widest = widest.max(width);
                width = 0;
            } else {
                width = width.saturating_add(1);
            }
        }
        widest.max(width)
    }

    #[test]
    fn an_empty_document_writes_no_octets() {
        assert!(Document::default().to_bytes().is_empty());
        let mut sink: Vec<u8> = Vec::new();
        Document::new(Vec::new()).serialize(&mut sink).unwrap();
        assert!(sink.is_empty());
    }

    /// The octets a producer wrote, beside the tree a parser would have built from them.
    fn recorded_cases() -> Vec<(&'static str, &'static [u8], Document)> {
        vec![
            (
                "an ordinary calendar",
                b"BEGIN:VCALENDAR\r\nPRODID:-//Example Corp//Example//EN\r\nEND:VCALENDAR\r\n",
                Document::new(vec![nested(
                    b"VCALENDAR",
                    vec![plain(b"PRODID", b"-//Example Corp//Example//EN")],
                )]),
            ),
            (
                "a fold inside the value",
                b"DESCRIPTION:hello wo\r\n rld\r\n",
                Document::new(vec![line(
                    b"DESCRIPTION",
                    Vec::new(),
                    b"hello world",
                    folded(&[20]),
                )]),
            ),
            (
                "a fold inside the parameters, addressed by the same counter",
                b"ATTENDEE;CN=Ex\r\n ample:mailto:a@example.test\r\n",
                Document::new(vec![line(
                    b"ATTENDEE",
                    vec![parameter(b"CN", b"Example")],
                    b"mailto:a@example.test",
                    folded(&[14]),
                )]),
            ),
            (
                "two folds at one offset",
                b"X-Q:ab\r\n \r\n cd\r\n",
                Document::new(vec![line(b"X-Q", Vec::new(), b"abcd", folded(&[6, 6]))]),
            ),
            (
                "a quoted parameter value, and one with no value at all",
                b"ATTENDEE;CN=\"Doe, J\";X-FLAG:mailto:j@example.test\r\n",
                Document::new(vec![line(
                    b"ATTENDEE",
                    vec![parameter(b"CN", b"\"Doe, J\""), valueless(b"X-FLAG")],
                    b"mailto:j@example.test",
                    folded(&[]),
                )]),
            ),
            (
                "octets that are not text",
                b"SUMMARY;LANGUAGE=en-GB:\xe9t\xe9\r\n",
                Document::new(vec![line(
                    b"SUMMARY",
                    vec![parameter(b"LANGUAGE", b"en-GB")],
                    b"\xe9t\xe9",
                    folded(&[]),
                )]),
            ),
        ]
    }

    #[test]
    fn every_recorded_line_is_written_back_octet_for_octet() {
        for (label, want, document) in recorded_cases() {
            assert_eq!(document.to_bytes(), want, "{label}");
        }
    }

    /// Each of these is reported as a diagnostic when it is read and is still written back
    /// unchanged, which are two claims and not one (`docs/adr/0001`).
    fn diagnosed_cases() -> Vec<(&'static str, &'static [u8], Document)> {
        vec![
            (
                "a line with no colon, and a blank line",
                b"JUNK\r\n\r\n",
                Document::new(vec![
                    line(b"JUNK", Vec::new(), b"", colonless()),
                    line(b"", Vec::new(), b"", colonless()),
                ]),
            ),
            (
                "a BEGIN carrying parameters, which is a property and not a boundary",
                b"BEGIN;X-Q=1:VEVENT\r\n",
                Document::new(vec![line(
                    b"BEGIN",
                    vec![parameter(b"X-Q", b"1")],
                    b"VEVENT",
                    folded(&[]),
                )]),
            ),
            (
                "an END that disagreed in case with its BEGIN",
                b"BEGIN:VEVENT\r\nend:vevent\r\n",
                Document::new(vec![Item::Component(Component::new(
                    boundary(b"BEGIN", b"VEVENT"),
                    Vec::new(),
                    Some(boundary(b"end", b"vevent")),
                ))]),
            ),
            (
                "a bare LF, a bare CR, and a final line with no terminator",
                b"SUMMARY:one\nSUMMARY:two\rSUMMARY:three",
                Document::new(vec![
                    line(b"SUMMARY", Vec::new(), b"one", ended(Some(LineEnding::Lf))),
                    line(b"SUMMARY", Vec::new(), b"two", ended(Some(LineEnding::Cr))),
                    line(b"SUMMARY", Vec::new(), b"three", ended(None)),
                ]),
            ),
            (
                "a component whose END never arrived",
                b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n",
                Document::new(vec![Item::Component(Component::new(
                    boundary(b"BEGIN", b"VCALENDAR"),
                    vec![Item::Component(Component::new(
                        boundary(b"BEGIN", b"VEVENT"),
                        vec![plain(b"UID", b"1")],
                        None,
                    ))],
                    None,
                ))]),
            ),
        ]
    }

    #[test]
    fn a_diagnosed_line_is_still_written_back_octet_for_octet() {
        for (label, want, document) in diagnosed_cases() {
            assert_eq!(document.to_bytes(), want, "{label}");
        }
    }

    /// The `VEVENT` of the composite case: folds in two places, a parameter with no value,
    /// three different terminators, octets that are not text, a colonless line, a blank line,
    /// and a nested component whose `END` disagreed in case.
    fn composite_event() -> Item {
        Item::Component(Component::new(
            boundary(b"BEGIN", b"VEVENT"),
            vec![
                line(
                    b"SUMMARY",
                    vec![
                        parameter(b"LANGUAGE", b"en-GB"),
                        parameter(b"X-Q", b"\"a;b\""),
                    ],
                    b"\xe9t\xe9",
                    folded(&[]),
                ),
                line(
                    b"ATTENDEE",
                    vec![parameter(b"CN", b"Example")],
                    b"mailto:a@example.test",
                    folded(&[14]),
                ),
                line(
                    b"DESCRIPTION",
                    vec![parameter(b"X-ALT", b"\"cid:x\"")],
                    b"hello world",
                    LineLayout::preserved(vec![fold(34)], Some(LineEnding::Lf), true),
                ),
                line(
                    b"X-VENDOR",
                    vec![valueless(b"X-FLAG")],
                    b"1",
                    ended(Some(LineEnding::Cr)),
                ),
                line(b"JUNK", Vec::new(), b"", colonless()),
                line(b"", Vec::new(), b"", colonless()),
                Item::Component(Component::new(
                    boundary(b"BEGIN", b"VALARM"),
                    vec![plain(b"TRIGGER", b"-PT15M")],
                    Some(boundary(b"end", b"valarm")),
                )),
            ],
            Some(boundary(b"END", b"VEVENT")),
        ))
    }

    /// The longest thing this unit sees: every recorded shape at once, nested two deep, with
    /// the outer component and the last line both cut short.
    fn composite_octets() -> Vec<u8> {
        let mut out = Vec::new();
        for physical in [
            &b"BEGIN:VCALENDAR\r\n"[..],
            &b"PRODID:-//Example Corp//Example//EN\r\n"[..],
            &b"BEGIN:VEVENT\r\n"[..],
            &b"SUMMARY;LANGUAGE=en-GB;X-Q=\"a;b\":\xe9t\xe9\r\n"[..],
            &b"ATTENDEE;CN=Ex\r\n ample:mailto:a@example.test\r\n"[..],
            &b"DESCRIPTION;X-ALT=\"cid:x\":hello wo\r\n rld\n"[..],
            &b"X-VENDOR;X-FLAG:1\r"[..],
            &b"JUNK\r\n"[..],
            &b"\r\n"[..],
            &b"BEGIN:VALARM\r\n"[..],
            &b"TRIGGER:-PT15M\r\n"[..],
            &b"end:valarm\r\n"[..],
            &b"END:VEVENT\r\n"[..],
            &b"BEGIN:VTODO\r\n"[..],
            &b"DUE:20260810"[..],
        ] {
            out.extend_from_slice(physical);
        }
        out
    }

    #[test]
    fn the_composite_calendar_is_written_back_octet_for_octet() {
        let unfinished = Item::Component(Component::new(
            boundary(b"BEGIN", b"VTODO"),
            vec![line(b"DUE", Vec::new(), b"20260810", ended(None))],
            None,
        ));
        let document = Document::new(vec![Item::Component(Component::new(
            boundary(b"BEGIN", b"VCALENDAR"),
            vec![
                plain(b"PRODID", b"-//Example Corp//Example//EN"),
                composite_event(),
                unfinished,
            ],
            None,
        ))]);
        assert_eq!(document.to_bytes(), composite_octets());
    }

    #[test]
    fn serialize_agrees_with_to_bytes_through_a_dyn_sink() {
        let document = Document::new(vec![plain(b"UID", b"1")]);
        let mut sink: Vec<u8> = Vec::new();
        let erased: &mut dyn Writer<Error = Infallible> = &mut sink;
        document.serialize(erased).unwrap();
        assert_eq!(sink, document.to_bytes());
    }

    /// A property this crate wrote itself, which is the only line it folds on its own terms.
    fn authored(value: Vec<u8>) -> Document {
        Document::new(vec![Item::Property(Property::new(
            RawText::from_bytes(b"X-A"),
            Vec::new(),
            RawText::from_vec(value),
            LineLayout::canonical(LineEnding::CrLf),
        ))])
    }

    #[test]
    fn a_refolded_line_breaks_at_the_canonical_width_and_continues_with_a_space() {
        let written = authored(vec![b'a'; 100]).to_bytes();

        let mut want = Vec::new();
        want.extend_from_slice(b"X-A:");
        want.extend_from_slice(&[b'a'; 70]);
        want.extend_from_slice(b"\r\n ");
        want.extend_from_slice(&[b'a'; 30]);
        want.extend_from_slice(b"\r\n");

        assert_eq!(written, want);
        assert_eq!(widest_line(&written), REFOLD_WIDTH);
    }

    #[test]
    fn a_refold_moves_a_codepoint_whole_rather_than_splitting_it() {
        let mut value = vec![b'a'; 69];
        value.extend_from_slice("é".as_bytes());
        let written = authored(value).to_bytes();

        let mut want = Vec::new();
        want.extend_from_slice(b"X-A:");
        want.extend_from_slice(&[b'a'; 69]);
        want.extend_from_slice(b"\r\n ");
        want.extend_from_slice("é".as_bytes());
        want.extend_from_slice(b"\r\n");

        assert_eq!(
            written, want,
            "the cut backs off to the codepoint's leading octet"
        );
    }

    #[test]
    fn a_long_refolded_value_stays_within_the_octet_bound_and_unfolds_to_itself() {
        let written = authored(vec![b'a'; 300]).to_bytes();

        let mut want = Vec::new();
        want.extend_from_slice(b"X-A:");
        want.extend_from_slice(&[b'a'; 300]);
        want.extend_from_slice(b"\r\n");

        assert_eq!(unfold(&written), want);
        assert!(
            widest_line(&written) <= 75,
            "RFC 5545 section 3.1 bounds a physical line at 75 octets"
        );
    }

    #[test]
    fn octets_that_are_not_text_are_folded_rather_than_searched_for_a_boundary_forever() {
        let written = authored(vec![0x80; 160]).to_bytes();

        let mut want = Vec::new();
        // The first attempt offers the run a whole line to fit on; the second finds it does
        // not fit there either, and splits, because these octets have no boundary to find.
        want.extend_from_slice(b"X-A:\r\n ");
        want.extend_from_slice(&[0x80; 74]);
        want.extend_from_slice(b"\r\n ");
        want.extend_from_slice(&[0x80; 74]);
        want.extend_from_slice(b"\r\n ");
        want.extend_from_slice(&[0x80; 12]);
        want.extend_from_slice(b"\r\n");

        assert_eq!(written, want);
    }

    /// Nesting far past what any stack would survive being recursed over.
    ///
    /// `Limits::max_component_depth` is a `u16` the caller raises through a public builder, so
    /// a tree this deep is one the reader will build when it is asked to. Walking it by
    /// recursion aborts the process — not a panic, so no caller catches it and no sibling test
    /// in the same binary survives it. Both the walk and the teardown are asserted here,
    /// because the teardown is the one that fires even for a caller that never serializes.
    ///
    /// Built from the inside out, since a helper that nested by recursion would abort in the
    /// test rather than in the code under test.
    #[test]
    fn a_tree_nested_far_deeper_than_the_stack_is_written_and_dropped_without_recursing() {
        const DEPTH: usize = 20_000;

        let mut innermost = Component::new(
            boundary(b"BEGIN", b"X"),
            vec![plain(b"UID", b"1")],
            Some(boundary(b"END", b"X")),
        );
        for _ in 1..DEPTH {
            innermost = Component::new(
                boundary(b"BEGIN", b"X"),
                vec![Item::Component(innermost)],
                Some(boundary(b"END", b"X")),
            );
        }
        let document = Document::new(vec![Item::Component(innermost)]);

        let mut want = Vec::new();
        for _ in 0..DEPTH {
            want.extend_from_slice(b"BEGIN:X\r\n");
        }
        want.extend_from_slice(b"UID:1\r\n");
        for _ in 0..DEPTH {
            want.extend_from_slice(b"END:X\r\n");
        }
        assert_eq!(document.to_bytes(), want);
        // The drop that runs when `document` leaves this scope is the second half of the
        // claim, and it is asserted by this test returning at all.
        drop(document);
    }

    #[test]
    fn fold_at_backs_off_only_as_far_as_a_codepoint_can_reach() {
        let text = "aaé".as_bytes();
        assert_eq!(fold_at(text, 3), Some(2), "back off to the leading octet");
        assert_eq!(fold_at(text, 2), Some(2), "already on a boundary");
        assert_eq!(fold_at(&[0x80; 8], 4), None, "no boundary within reach");
        assert_eq!(fold_at(b"abc", 0), None, "no room to cut anything at all");
    }
}
