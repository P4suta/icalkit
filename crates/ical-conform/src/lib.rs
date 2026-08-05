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
//! # Status
//!
//! Bootstrap. No cases are written yet; see `ROADMAP.md` (M5).
