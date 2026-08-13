// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a reader carries with it: the caller's policies, ledger and diagnostic sink.
//!
//! Bundled into one value rather than threaded as four arguments, because every reading door
//! in this crate takes all of them and a signature that takes three of the four is a door
//! somebody forgot to charge.

use core::fmt::{self, Debug, Formatter};

use crate::internal::core::{
    Diagnostic, DiagnosticCode, DiagnosticSink, Limits, Location, Meter, Severity,
};

use crate::internal::dav::text::TextPolicy;

/// What to do about an element outside the closed vocabulary.
///
/// Both answers are correct for somebody, which is why this is a caller policy rather than a
/// decision this crate makes. RFC 4918 section 17 requires a client to tolerate the elements a
/// server extended its bodies with, and a server asked to honor a `REPORT` it does not
/// understand should refuse rather than answer a different question.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnknownPolicy {
    /// Skip the element and everything inside it, with a diagnostic.
    #[default]
    Skip,
    /// Refuse the body.
    Reject,
}

/// The policies, ledger and sink a read is performed under.
///
/// `Limits` is carried by value because it is `Copy` and small; the ledger and the sink are
/// borrowed because their lifetime is the caller's choice and not the call's, which is the
/// whole of `docs/adr/0010`'s argument about aggregate bounding.
pub struct DecodeContext<'a> {
    /// What to do about a foreign element.
    pub unknown: UnknownPolicy,
    /// How character data is delivered, and whether `calendar-data` keeps its line endings.
    pub text: TextPolicy,
    /// The caller's bounds.
    pub limits: Limits,
    /// The caller's running ledger.
    pub meter: &'a mut Meter,
    /// Where diagnostics go.
    pub sink: &'a mut dyn DiagnosticSink,
}

impl<'a> DecodeContext<'a> {
    /// A context over the caller's ledger and sink, with the default policies.
    pub fn new(limits: Limits, meter: &'a mut Meter, sink: &'a mut dyn DiagnosticSink) -> Self {
        Self {
            unknown: UnknownPolicy::Skip,
            text: TextPolicy::Verbatim,
            limits,
            meter,
            sink,
        }
    }

    /// The same context with a different unknown-element policy.
    #[must_use]
    pub fn with_unknown(mut self, unknown: UnknownPolicy) -> Self {
        self.unknown = unknown;
        self
    }

    /// The same context with a different text policy.
    #[must_use]
    pub fn with_text(mut self, text: TextPolicy) -> Self {
        self.text = text;
        self
    }

    /// Report a diagnostic at a body offset, charging a refusal to the ledger.
    ///
    /// The location is an offset and never a line number: an XML body has offsets and no
    /// content lines, and a zero in a line field would be a claim rather than an absence.
    pub fn report(&mut self, code: DiagnosticCode, severity: Severity, offset: u64) {
        let diagnostic = Diagnostic::new(code, severity, Location::at_offset(offset));
        crate::internal::core::report_diagnostic(self.sink, self.meter, diagnostic);
    }
}

impl Debug for DecodeContext<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        // The sink is a trait object with no `Debug` bound, and adding one would forbid a
        // caller's own sink from being anything it likes. The policies are what a reader
        // debugging a parse actually wants to see.
        formatter
            .debug_struct("DecodeContext")
            .field("unknown", &self.unknown)
            .field("text", &self.text)
            .field("limits", &self.limits)
            .field("meter", &self.meter)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{Diagnostic, DiagnosticCode, Limits, Meter, Severity, Span};

    use super::{DecodeContext, UnknownPolicy};
    use crate::internal::dav::text::TextPolicy;

    #[test]
    fn the_defaults_are_the_ones_that_lose_nothing() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let context = DecodeContext::new(Limits::DEFAULT, &mut meter, &mut reported);
        assert_eq!(context.unknown, UnknownPolicy::Skip);
        assert_eq!(context.text, TextPolicy::Verbatim);
    }

    #[test]
    fn a_report_reaches_the_caller_s_sink_with_its_offset() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        {
            let mut context = DecodeContext::new(Limits::DEFAULT, &mut meter, &mut reported);
            context.report(DiagnosticCode::DavForeignElementSkipped, Severity::Note, 42);
        }
        let first = reported.first().copied().unwrap();
        assert_eq!(first.code(), DiagnosticCode::DavForeignElementSkipped);
        assert_eq!(first.location().span().map(Span::start), Some(42));
    }
}
