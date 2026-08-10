# Diagnostic codes

`DiagnosticCode` is one vocabulary for the whole workspace, and
[ADR 0009](adr/0009-error-and-diagnostic-model.md) freezes each code's meaning as hard as its
name: a variant may be added, and the meaning of one that already exists may not be edited
without a rename or a deprecation.
[ADR 0006](adr/0006-conformance-corpus-as-artifact.md)'s corpus asserts "this input produces
this code, on this channel" across releases, and an edited meaning or a retuned channel breaks
that claim while every variant and every doc comment sits still.

This file is the golden list that makes the freeze enforceable rather than merely documented.
`just codes` reads it and `crates/ical-grammar/src/report.rs`, and nothing else, with the same
hand-rolled scan `just purity` uses and for the same reason: the tool that enforces "the core
has no outside dependencies" may not acquire one.

## The columns

- **code** — the `DiagnosticCode::as_str` key. That key, not the variant name, is what a
  conformance case names and what this table is keyed on. Rows are in declaration order, and
  the gate checks that they are, so a reordering is a diff nobody has to notice by eye.
- **meaning** — the first paragraph of the variant's doc comment, verbatim. It is written
  twice on purpose. The gate fails unless the two copies agree, so editing a meaning means
  editing this file too, which is the review a frozen meaning is owed. Improving the prose
  *below* the first paragraph stays free; the first paragraph is the meaning.
- **channel** — the `Severity` the emitter passes. A severity retune changes what one input
  does to a strict caller while every variant and doc comment sits still, which is why the
  channel is a column here and not a remark in a doc comment. Every `Severity` must be carried
  by at least one code, so a severity cannot sit in the enum with nothing traveling on it.
- **milestone** — the milestone in [ROADMAP.md](../ROADMAP.md) that owns the emission site.

## Notes rather than violations

Five codes travel on `Severity::Note`, and each is a case where the input is legal and the
caller still needs telling. RFC 6868 section 2 requires an undefined caret pair to be left
exactly as it is. RFC 5545 section 3.2.20 permits a `VALUE` type this workspace has no decoder
for, so not knowing one is a gap here rather than a fault in the file. Section 3.3.10 requires
a recurrence instance whose date does not exist to be ignored rather than moved. A local time
that occurs twice does occur, and choosing between the two is a caller's policy rather than a
repair. Two zone sources that disagree are each internally consistent, and neither is the one
that violated something. A caller enforcing strictness rejects on `Severity::Violation`, and
would reject half the calendars in the world if it also rejected on `Severity::Note`.

## Codes with no M0 emitter

Every row whose milestone is not `M0` is a code this workspace declares today and does not
emit yet. Two of those absences are worth naming, because both look like dead API to anyone
reading `ical-grammar` alone and neither is:

- `Severity::LimitReached` is carried by exactly one code, `recurrence-budget-exhausted`, and
  that code belongs to M1. Nothing in M0 emits it, and nothing in M0 is supposed to: ADR 0009
  routes M0's limit breaches to `ParseError`, because the alternative is a truncated value,
  which writes back fewer bytes than it read and contradicts
  [ADR 0001](adr/0001-lossless-round-trip.md). The severity is unbuilt, not unused.
- `Diagnostic::at_instant` has no M0 caller either. It exists for diagnostics about an
  occurrence that exists at no offset in any file: `nonexistent-recurrence-instance` in M1,
  and `ambiguous-local-time`, `nonexistent-local-time`, and `time-zone-source-disagreement`
  in M2.

## What this list does not prove

ADR 0009 records the gap and this file does not close it: the gate proves that the table and
the declarations agree, and nothing yet proves that either agrees with the emission sites. A
code can carry a row, a channel, and a milestone, and still be emitted by nobody or emitted on
the other channel. Deriving one artifact from the other is the real fix and is not here. The
milestone column is the honest half of the answer — it says which milestone owes the emitter,
so an unemitted code reads as unbuilt work rather than as a mystery.

## The table

| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| invalid-utf8-text | Text that a typed view had to decode was not valid UTF-8. The octets are preserved. | Violation | M0 |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
| empty-property-name | A content line had an empty property name. A blank line is the degenerate case. | Violation | M0 |
| parameters-on-component-boundary | A `BEGIN` or `END` line carried parameters, which a component boundary cannot hold. | Violation | M0 |
| unmatched-end | An `END` arrived with no `BEGIN` open. | Violation | M0 |
| mismatched-end-name | An `END` named a different component than the `BEGIN` it closed. | Violation | M0 |
| unclosed-component | A `BEGIN` was never closed before the input ended. | Violation | M0 |
| bare-line-feed | A line was terminated by a bare `LF`, where RFC 5545 requires `CRLF`. | Violation | M0 |
| bare-carriage-return | A line was terminated by a bare `CR`, where RFC 5545 requires `CRLF`. | Violation | M0 |
| missing-final-line-break | The last line of the input carried no terminator at all. | Violation | M0 |
| line-too-long | A physical line ran past the 75 octets RFC 5545 section 3.1 allows one. | Violation | M0 |
| control-character-in-text | A value or parameter held a control character RFC 5545 section 3.1 excludes. | Violation | M0 |
| unterminated-quoted-parameter | A `DQUOTE`-delimited parameter value was never closed. | Violation | M0 |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
| parameter-without-value | A parameter arrived with a name and no `=`. | Violation | M0 |
| duplicate-property | A property the specification declares at most once occurred more than once. | Violation | M0 |
| malformed-date | A `DATE` value did not match RFC 5545 section 3.3.4. | Violation | M0 |
| malformed-date-time | A `DATE-TIME` value did not match RFC 5545 section 3.3.5. | Violation | M0 |
| malformed-time | A `TIME` value did not match RFC 5545 section 3.3.12. | Violation | M0 |
| malformed-duration | A `DURATION` value did not match RFC 5545 section 3.3.6. | Violation | M0 |
| malformed-period | A `PERIOD` value did not match RFC 5545 section 3.3.9. | Violation | M0 |
| malformed-utc-offset | A `UTC-OFFSET` value did not match RFC 5545 section 3.3.14. | Violation | M0 |
| malformed-geo | A `GEO` value was not the `FLOAT;FLOAT` pair RFC 5545 section 3.8.1.6 requires. | Violation | M0 |
| malformed-integer | An `INTEGER` value did not match RFC 5545 section 3.3.8, or did not fit. | Violation | M0 |
| malformed-float | A `FLOAT` value did not match RFC 5545 section 3.3.7. | Violation | M0 |
| malformed-boolean | A `BOOLEAN` value was neither `TRUE` nor `FALSE` in any casing. | Violation | M0 |
| malformed-binary | A `BINARY` value was not the base 64 RFC 5545 section 3.3.1 requires. | Violation | M0 |
| malformed-uri | A `URI` value did not match RFC 5545 section 3.3.13, or section 3.3.3's `CAL-ADDRESS`. | Violation | M0 |
| unknown-value-type | A `VALUE` parameter named a value type this workspace does not know. | Note | M0 |
| missing-required-property | A component did not carry a property RFC 5545 section 3.6 requires of it. | Violation | M0 |
| property-not-allowed-here | A component carried a property RFC 5545 section 3.6 does not define for it. | Violation | M0 |
| mutually-exclusive-properties | A component carried two properties RFC 5545 section 3.6 does not allow together. | Violation | M0 |
| recurrence-budget-exhausted | A recurrence search stopped at the candidate budget rather than at the rule's end. | LimitReached | M1 |
| nonexistent-recurrence-instance | A recurrence rule generated an instance whose date does not exist, so it was filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one. | Note | M1 |
| unknown-time-zone | A `TZID` named a zone no supplied source could resolve. | Violation | M2 |
| missing-time-zone-definition | A `TZID` parameter named a zone with no `VTIMEZONE` in the same calendar. | Violation | M2 |
| ambiguous-local-time | A local time occurs twice under its zone, at the end of a daylight saving period. | Note | M2 |
| nonexistent-local-time | A local time does not occur under its zone, at the start of a daylight saving period. | Violation | M2 |
| time-zone-source-disagreement | An embedded `VTIMEZONE` and the caller's other zone source disagreed about an offset. | Note | M2 |
