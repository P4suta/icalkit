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
//! internals, and they are evaluated through a trait, so a competing implementation can run
//! the identical suite and compare answers. This workspace supplies one implementation of
//! that trait (see `docs/adr/0006`).
//!
//! The corpus is real. Calendars exported from real clients are committed verbatim and
//! round-tripped byte for byte, which is what makes the fidelity claim in `docs/adr/0001`
//! verifiable rather than asserted. Each file is reduced to the smallest form that still
//! shows the behavior and stripped of personal data before the case is accepted — a case
//! that cannot be anonymized is not accepted — and every case records which client and
//! version produced the original.
//!
//! Where implementations diverge, a case records each observed behavior and says which one
//! this project chose and why. Where the RFC permits alternatives, every permitted outcome
//! is recorded rather than one being canonized. A case reading "Microsoft 365 emits this,
//! the RFC forbids it, this project accepts it on read and never emits it" is documentation
//! that exists nowhere else.
//!
//! This is the one crate here that uses `std`, and the purity gate covers the five beneath
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
//! The case vocabulary, the subject contract and the two runners are designed and compiled;
//! `docs/design/ical-conform-api.md` carries them. The cases beside them in `tests/` are the
//! ones M0 owes: what real clients export and what an adversary sends (`break_clients.rs`,
//! `break_grammar.rs`, `break_hostile.rs`, `break_hostile_stack_overflow.rs`), what the write
//! side may author (`write_side_grammar.rs`, `break_construction.rs`), and the two readings
//! this workspace had to choose about parameters and RFC 6868 (`break_parameters.rs`).
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
