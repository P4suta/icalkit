// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The calendaring conformance and interoperability corpus, runnable against any
//! implementation.
//!
//! RFC 5545 is a large specification, but interoperability is settled less by it than by
//! what Google Calendar, Microsoft 365, and Apple Calendar actually emit and accept. Those
//! three disagree with the RFC and with each other, and the disagreements are folklore:
//! everyone who has implemented this knows a few, and nobody has written them down anywhere
//! a machine can check.
//!
//! This crate is that missing artifact, and it is deliberately not a `tests/` directory.
//! Cases are addressed to specification sections rather than to any implementation's
//! internals, and the contract that would let a competing implementation answer them is
//! designed in `docs/design/ical-conform-api.md` and not yet built (see `docs/adr/0006`).
//! Until it is, every case here links this workspace's own crates and names their types,
//! which makes the suite evidence about one implementation and not yet a measurement anybody
//! else can reproduce.
//!
//! The corpus is not yet real, and saying so is cheaper than discovering it. Every fixture
//! committed here is synthetic and shaped like something a named client writes — a fold
//! landing inside a quoted `X-APPLE-STRUCTURED-LOCATION`, the `X-MICROSOFT-CDO-` family, a
//! `/mozilla.org/`-prefixed `TZID` — and each is round-tripped byte for byte, so it is
//! evidence for `docs/adr/0001` and not evidence about any client. Collecting the real
//! exports, reducing each to the smallest form that still shows the behavior, anonymizing it,
//! and recording which client and version produced it is M5's, and the case vocabulary that
//! would make skipping the anonymization an act rather than an omission is designed for
//! exactly that.
//!
//! Where implementations diverge, a case records each observed behavior and says which one
//! this project chose and why. Where the RFC permits alternatives, every permitted outcome
//! is recorded rather than one being canonized. A case reading "Microsoft 365 emits this,
//! the RFC forbids it, this project accepts it on read and never emits it" is documentation
//! that exists nowhere else.
//!
//! This is the one crate here that uses `std`, and the purity gate covers the six beneath
//! it rather than this one: a harness that cannot read a corpus file or print a report is
//! not a harness. Nothing about the implementations under test changes as a result — they
//! are still handed bytes.
//!
//! A case states the `Limits` policy it ran under and asserts a `DiagnosticCode` rather than
//! a message, because an outcome that depends on a budget is not reproducible without the
//! budget, and a code whose meaning is frozen is the only thing an assertion can outlive
//! (see `docs/adr/0009` and `docs/adr/0010`).
//!
//! # Status
//!
//! The case vocabulary, the subject contract and the two runners are designed and not built:
//! `docs/design/ical-conform-api.md` carries them and this crate exports nothing.
//!
//! The cases in `tests/` are thirty-one files that arrived a milestone at a time. M0's are what
//! real clients export and what an adversary sends (`break_clients.rs`, `break_grammar.rs`,
//! `break_hostile.rs`, `break_hostile_stack_overflow.rs`), what the write side may author
//! (`write_side_grammar.rs`, `break_construction.rs`), and the two readings this workspace had
//! to choose about parameters and RFC 6868 (`break_parameters.rs`). What followed is the RFC's
//! own forty-two recurrence examples (`rfc5545_recurrence_examples.rs`) and, for each of M1
//! through M4, the four adversarial lenses that were run against the built crate and the case
//! each finding left behind: `break_recur_*.rs`, `break_tz_*.rs` with `break_zones.rs`,
//! `break_itip_*.rs`, and `break_dav_*.rs`.
//!
//! `sweep.rs` is the other kind of evidence: a seeded, deterministic, time-bounded sweep over
//! inputs nobody chose — exhaustively every short string over the octets that decide a line,
//! randomly from a committed seed, and generatively as edits to every committed fixture — each
//! asserting the round trip and accepting a refusal only where the input itself confirms the
//! bound was crossed. It prints what it covered, because a generative test that quietly stops
//! generating passes.
//!
//! What is still owed is M5's: a foreign implementation actually run, so that a "where
//! implementations differ" note records a measurement rather than what a project documents.

// The chapters that are prose plus a case table rather than a fixture plus a runner. A
// scheduling case is a triple — prior state, incoming message, applying party — and the state
// is reached through a trait rather than through a file, so the cases live beside the
// documentation that says which section each one is addressed to.
pub mod itip;
