# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Workspace bootstrap: crate skeletons, quality gates, and the day-one architectural
  decision records. No parsing, recurrence, or scheduling logic yet.
- Five architectural decisions from the design bake-off — allocation policy, parser layering
  and the pull API, the error and diagnostic model, shared resource limits, and civil-time
  arithmetic — and a per-crate design document for each crate's committed public surface.
- `ical-grammar`, holding the content line grammar and the diagnostic vocabulary below
  `ical-core`, so a linter or a fuzz harness never compiles the typed model.
- The content line layer, and the model on top of it: `ContentLineReader` unfolds and lexes in
  one pass, `Document::parse` builds a tree from that same public token path, and
  `Document::serialize` writes it back octet for octet — folds where the producer put them,
  the terminator it chose, parameters in the order and case it wrote them. Around that, the
  checked civil arithmetic, a decoder for every value type of RFC 5545 sections 3.3.4 through
  3.3.14, the typed accessors as views over preserved text, and scoped mutation through
  `PropertyMut`, which is the only way to write and which discards exactly one line's layout.
  A structural violation is a diagnostic and never costs an octet; a caller-stated bound is a
  refusal and never a truncation.
- The rest of RFC 5545 section 3.3: `FLOAT`, `BINARY`, `URI`/`CAL-ADDRESS` and `PERIOD` decode,
  and the last three encode with the parameters their own shape implies — `VALUE=BINARY` with
  `ENCODING=BASE64`, and a `TZID` applied to both bounds of a period. A `DateTimeValue::Zoned`
  variant means a date-time can no longer be constructed apart from the parameter set it
  implies, which is what ADR 0001 asked for.
- Section 3.6 as a reading rather than as storage: `ComponentKind::cardinality` for the nine
  components, an advisory `Component::audit` that reports a missing, duplicated or misplaced
  property and a `VALUE` naming a type nobody defines, and seventeen typed accessors.
- RFC 6868's caret encoding in both directions (`decode_caret`, `encode_caret`), as a codec a
  caller opts into: storage still keeps the octets a producer wrote, so a `DQUOTE` written `^'`
  stays `^'` on the wire and is a `"` only in the decoded view.
- Five diagnostics the workspace defined and nothing reported now have emitters: a physical
  line over 75 octets, a control character in a name, a parameter name or a value, a parameter
  that arrived with no value, an unterminated quoted parameter, and an RFC 6868 pair nothing
  defines. Each names the octets it is about, and none of them charges the meter.
- `docs/diagnostic-codes.md`, the golden list ADR 0009 requires, and `just codes`, which fails
  unless every declared code carries a row whose meaning is the variant's own first paragraph
  byte for byte. A meaning can now only be edited by editing both files, which is the review a
  frozen meaning is owed.
- Conformance cases for the two readings this workspace had to choose — where a `DQUOTE`
  delimits a parameter value, and what an undefined caret pair means — and `sweep.rs`, the
  adversarial evidence M0-alpha reported and never committed, as a seeded, deterministic,
  time-bounded sweep that accepts a refusal only where the input itself confirms the bound.

### Changed

- The purity gate reads the package a dependency links rather than the key it was written
  under. A `package = "..."` rename defeated every leg of the old gate; it is now a violation
  in both the inline and the sub-table spelling, alongside a dependency taken from a registry
  and a `no_std` crate missing from the gate's own crate list.
- `ParseError` gained `TooManyParameters`, so the parameter-count bound is reported as itself.
  It had been reported as `HeaderTooLarge` carrying the octet ceiling, which named a number a
  caller could raise without the refusal ever going away. `MonthAddOutcome` is
  `#[non_exhaustive]`, as ADR 0011 and `docs/design/ical-tz-api.md` always described it and the
  code did not; adding it before `ical-recur` and `ical-tz` match on the enum costs nothing.
- `Component`'s `Clone`, `PartialEq`, `Ord`, `Hash` and `Debug` are written over an explicit
  stack instead of derived. Each of the five recursed one frame per level of nesting, and
  `max_component_depth` is a `u16` a caller raises, so `document.clone()` on a twenty-thousand
  deep tree took the process down — an abort, which no `catch_unwind` sees.
- Construction is no longer an unchecked door. `Property::new`, `Parameter::new` and
  `Boundary::new` are crate-private; `Property::create`, `Parameter::create` and
  `Component::create` refuse the octets RFC 5545 section 3.1 cannot write back, so a value
  carrying its own `CRLF` can no longer be pushed into a tree and serialize as two content
  lines. Octets that were never read from anywhere have no producer's spelling to preserve, so
  the refusal costs the round-trip claim nothing.
- `max_folds_per_line` is justified by a measurement rather than by a division: the largest
  inline `ATTACH` the default policy admits is retained across 14,170 continuations folded at
  75 octets and 14,564 at 73, both under the bound, which is now documented as what the
  headroom buys — a producer folding tighter than section 3.1 asks, not a longer value.
