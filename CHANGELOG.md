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
