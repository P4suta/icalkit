// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resource failures from token sources and tree assembly share the caller's ledger.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    ContentLineReader, ContentLineSource, Document, GrammarLimits, LimitExceeded, Limits, Meter,
    ParseError, Token,
};

fn parse(input: &[u8], limits: Limits, meter: &mut Meter) -> Result<Document, ParseError> {
    let mut reader = ContentLineReader::new(input, limits.grammar());
    Document::from_tokens(&mut reader, meter, &mut Vec::new())
}

fn refuses_and_stays_exhausted(input: &[u8], limits: Limits, expected: ParseError) {
    let mut meter = Meter::new(limits);
    assert_eq!(parse(input, limits, &mut meter).unwrap_err(), expected);
    assert!(meter.is_exhausted());
    let spent = meter.spent();
    assert_eq!(meter.try_charge_occurrence(), Err(LimitExceeded::Exhausted));
    assert_eq!(
        parse(b"X:v\r\n", limits, &mut meter).unwrap_err(),
        ParseError::Exhausted
    );
    assert_eq!(meter.spent(), spent);
    let mut fresh = Meter::new(limits);
    assert!(parse(b"X:v\r\n", limits, &mut fresh).is_ok());
    assert!(!fresh.is_exhausted());
}

#[test]
fn a_header_refusal_latches_the_document_meter() {
    refuses_and_stays_exhausted(
        b"XX:v\r\n",
        Limits::DEFAULT.with_grammar(GrammarLimits::DEFAULT.with_max_header_bytes(1)),
        ParseError::HeaderTooLarge { limit: 1 },
    );
}

#[test]
fn a_parameter_refusal_latches_the_document_meter() {
    refuses_and_stays_exhausted(
        b"X;P=v:v\r\n",
        Limits::DEFAULT.with_grammar(GrammarLimits::DEFAULT.with_max_parameters(0)),
        ParseError::TooManyParameters { limit: 0 },
    );
}

#[test]
fn a_fold_refusal_latches_the_document_meter() {
    refuses_and_stays_exhausted(
        b"X:v\r\n x\r\n",
        Limits::DEFAULT.with_grammar(GrammarLimits::DEFAULT.with_max_folds_per_line(0)),
        ParseError::TooManyFolds { limit: 0 },
    );
}

#[test]
fn a_value_refusal_latches_the_document_meter() {
    refuses_and_stays_exhausted(
        b"X:vv\r\n",
        Limits::DEFAULT.with_max_value_bytes(1),
        ParseError::ValueTooLarge { limit: 1 },
    );
}

struct RefusingSource {
    refusal: Option<ParseError>,
    pulls: usize,
}

impl ContentLineSource for RefusingSource {
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>> {
        self.pulls = self.pulls.saturating_add(1);
        self.refusal.take().map(Err)
    }
}

#[test]
fn every_supplied_source_refusal_keeps_its_error_and_latches_the_caller_meter() {
    let refusals = [
        ParseError::Exhausted,
        ParseError::InputTooLarge { limit: 9 },
        ParseError::ValueTooLarge { limit: 9 },
        ParseError::HeaderTooLarge { limit: 9 },
        ParseError::TooManyParameters { limit: 9 },
        ParseError::TooManyFolds { limit: 9 },
        ParseError::TooManyItems { limit: 9 },
        ParseError::TooDeep { limit: 9 },
    ];
    for refusal in refusals {
        let mut source = RefusingSource {
            refusal: Some(refusal),
            pulls: 0,
        };
        let mut meter = Meter::new(Limits::DEFAULT);
        assert_eq!(
            Document::from_tokens(&mut source, &mut meter, &mut Vec::new()).unwrap_err(),
            refusal
        );
        assert!(meter.is_exhausted(), "{refusal:?}");
        assert_eq!(source.pulls, 1);
        assert_eq!(meter.try_charge_bytes(0), Err(LimitExceeded::Exhausted));
    }
}

#[test]
fn an_exhausted_meter_refuses_before_polling_even_an_empty_source() {
    let mut source = RefusingSource {
        refusal: None,
        pulls: 0,
    };
    let mut meter = Meter::new(Limits::DEFAULT.with_occurrences_per_search(0));
    assert_eq!(
        meter.try_charge_occurrence(),
        Err(LimitExceeded::Occurrences)
    );
    assert_eq!(
        Document::from_tokens(&mut source, &mut meter, &mut Vec::new()).unwrap_err(),
        ParseError::Exhausted
    );
    assert_eq!(source.pulls, 0);
    let mut fresh = Meter::new(Limits::DEFAULT);
    assert!(Document::from_tokens(&mut source, &mut fresh, &mut Vec::new()).is_ok());
    assert_eq!(source.pulls, 1);
    assert!(!fresh.is_exhausted());
}
