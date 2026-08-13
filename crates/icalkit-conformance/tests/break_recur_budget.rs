// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Attacks on `ical-recur`'s two bounding claims: that a search is bounded, and that the
//! exhaustion of that bound is always reported.
//!
//! `docs/adr/0002` says exhausting the candidate budget "is a reported outcome ... not a hang
//! and not a silent empty result", and names three reports of that one fact, in decreasing
//! order of survivability: the terminal [`SearchStep`], the caller's own `Meter` whose
//! "exhaustion flag latches and outlives every combinator applied to the iterator", and
//! `RecurrenceSearch::outcome`. The whole reason `Item` is a crate-owned enum rather than a
//! `Result` is that `.flatten()`, `.filter_map(Result::ok)` and `.take_while(Result::is_ok)`
//! are one-line idioms that discard a terminal marker while leaving a plausible answer behind.
//!
//! This file asks whether the replacement holds. Two questions, each with its own section: can
//! the terminal state still be discarded by an idiom, and does the bound actually bound the
//! rules `docs/adr/0002` names as the reason it exists.
//!
//! Every test here drives the public surface only.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, Instant, Limits, Meter, UtcOffset,
};
use ical_recur::{
    DEFAULT_CANDIDATE_BUDGET, Occurrence, Override, OverrideRange, OverrideSet, PropertyDiff,
    RecurrenceInput, RecurrenceRule, SearchOutcome, SearchStep, ValueKind, Window, parse_recur,
};

/// One instant on the UTC timeline, which is where a caller has already resolved everything.
///
/// `Option` rather than a panic, and the same for the two helpers below: this workspace's lint
/// profile permits `unwrap` and `expect` inside a `#[test]` body and nowhere else, so a helper
/// that panics is a helper that cannot be linted. Every caller is a test, and a mistyped
/// literal is still a named failure of the case that carries it.
fn at(year: u16, month: u8, day: u8, hour: u8) -> Option<Instant> {
    let date = CivilDate::from_ymd(year, month, day)?;
    let time = CivilTime::from_hms(hour, 0, 0)?;
    CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)
}

/// The rule a fixture under `fixtures/break_recur_budget` holds.
fn rule_from(fixture: &str, meter: &mut Meter) -> Option<RecurrenceRule> {
    let path = format!(
        "{}/tests/fixtures/break_recur_budget/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read(&path).ok()?;
    let mut sink: Vec<Diagnostic> = Vec::new();
    parse_recur(text.trim_ascii_end(), meter, &mut sink).ok()
}

/// A series with a rule and nothing else.
fn series<'a>(
    dtstart: Instant,
    rule: &'a RecurrenceRule,
    meter: &mut Meter,
) -> Option<RecurrenceInput<'a>> {
    RecurrenceInput::new(
        dtstart,
        ValueKind::DateTime,
        Some(rule),
        &[],
        &[],
        OverrideSet::empty(),
        meter,
    )
    .ok()
}

/// What one search produced.
///
/// Wall clock is deliberately not a field. Every case below finished in well under a second
/// when this file was written, and a duration asserted here would be flakiness on a shared
/// runner; what holds "bounded" to a number is nextest's own slow threshold, which every case
/// in this file passes by two orders of magnitude.
#[derive(Debug)]
struct Run {
    /// Occurrences the search emitted.
    emitted: usize,
    /// Whether the last step was the terminal one.
    terminal: bool,
    /// What the search said about itself once it was done.
    outcome: SearchOutcome,
}

/// Drive a whole search, honestly, keeping every report.
fn drain(input: RecurrenceInput<'_>, window: Window, meter: &mut Meter) -> Run {
    let mut sink: Vec<Diagnostic> = Vec::new();
    let mut search = input.search(window, meter, &mut sink);
    let mut emitted = 0_usize;
    let mut terminal = false;
    for step in &mut search {
        // Every step that is not an occurrence is a terminal one, this arm included: the item
        // type is `#[non_exhaustive]`, so a terminal state added later lands here and is
        // reported rather than counted as an occurrence.
        match step.occurrence() {
            Some(_) => emitted = emitted.saturating_add(1),
            None => terminal = true,
        }
    }
    Run {
        emitted,
        terminal,
        outcome: search.outcome(),
    }
}

// ---------------------------------------------------------------------------------------------
// Section 1: can the terminal state still be discarded by an idiom?
// ---------------------------------------------------------------------------------------------

/// `.map_while(SearchStep::occurrence)` is `.take_while(Result::is_ok)` under another name.
///
/// `docs/adr/0002` rejected `Result` because `.take_while(Result::is_ok)` "converts budget
/// exhaustion back into the truncated-but-plausible answer this ADR exists to prevent", and
/// claims the crate-owned enum makes that not compile. `SearchStep::occurrence` has the exact
/// shape `Iterator::map_while` wants — `fn(Item) -> Option<B>` — so the idiom is reconstructed
/// in one line, with no `match` and no visible discard, and it *reads* as though it handles
/// termination: `map_while` stops at the first `None`, which is precisely the terminal step.
#[test]
fn map_while_reconstructs_take_while_is_ok_and_drops_the_terminal_state() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 512);
    let rule = rule_from("daily_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1997, 9, 1, 0).unwrap(), at(2097, 9, 1, 0).unwrap())
        .expect("a century of days");
    let mut sink: Vec<Diagnostic> = Vec::new();

    let starts: Vec<Instant> = input
        .search(window, &mut meter, &mut sink)
        .map_while(SearchStep::occurrence)
        .map(Occurrence::start)
        .collect();

    assert!(
        !starts.is_empty(),
        "the truncated answer is plausible, which is what makes it dangerous"
    );
    assert!(
        starts.len() < 36_000,
        "a century of daily occurrences did not fit in the budget, and the caller was not told"
    );
}

/// The second report — the meter — latches for every one of the three ways a search stops.
///
/// The name records the state this case was written against: `Meter::is_exhausted` reported the
/// octet ledger alone, and `Limits::occurrences_per_search` and `Limits::candidates_per_period`
/// were refused without touching it, so a search terminated by either left the meter reading
/// clean beside a truncated answer. `docs/adr/0002` amendment 1 calls that meter the report
/// "whose exhaustion flag latches and outlives every combinator applied to the iterator", and
/// `docs/design/ical-recur-api.md` says "a reviewer who cannot find the terminal arm can still
/// find `meter.is_exhausted()`". Every bound the ledger keeps latches it now.
#[test]
fn the_occurrence_ceiling_terminates_a_search_and_leaves_the_meter_reading_clean() {
    let limits = Limits::DEFAULT.with_occurrences_per_search(16);
    let mut meter = Meter::new(limits);
    let rule = rule_from("daily_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1997, 9, 1, 0).unwrap(), at(1998, 9, 1, 0).unwrap())
        .expect("a year of days");

    let run = drain(input, window, &mut meter);

    assert!(run.terminal, "the search did stop at a limit");
    assert!(!run.outcome.is_complete(), "and says the answer is partial");
    assert!(
        meter.is_exhausted(),
        "the second report must latch, or a caller that kept only the meter is told the \
         truncated answer was complete: emitted {}, outcome {:?}",
        run.emitted,
        run.outcome
    );
}

/// The same report, reached through the per-period ceiling instead.
///
/// `crates/ical-recur/src/accounting.rs` used to assert the hole as intended — "a period ceiling
/// refuses before it touches the shared ledger" — which made it a stated property rather than an
/// accident, and it was the property that broke the second report. That assertion is inverted
/// there now and this is the same claim from outside the crate.
#[test]
fn the_period_ceiling_terminates_a_search_and_leaves_the_meter_reading_clean() {
    let limits = Limits::DEFAULT.with_candidates_per_period(64);
    let mut meter = Meter::new(limits);
    let rule = rule_from("yearly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1997, 9, 1, 0).unwrap(), at(2010, 9, 1, 0).unwrap())
        .expect("a decade of years");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal,
        "a year holds 365 days and the ceiling is 64, so the first period cannot be filled"
    );
    assert!(
        meter.is_exhausted(),
        "the second report must latch: outcome {:?}",
        run.outcome
    );
}

/// The two holes composed: an idiom takes the first report, the meter never had the second, and
/// the third went with the iterator that was moved into the adapter.
///
/// This is the shape the ADR says cannot happen — a truncated answer with no surviving report —
/// and it was reachable with `Limits::DEFAULT` and no budget the caller chose, because
/// `occurrences_per_search` defaulted to 65,536 while a day of `FREQ=SECONDLY` is 86,400. Two
/// answers were open: latch the meter, or stop refusing a workload the candidate budget already
/// admits. Both landed. This case asserts the second, because a caller writing the one-line
/// idiom over an unconfigured meter should get the whole day rather than a report about why it
/// did not — and the case above asserts the first, which is what happens when a caller does
/// state a bound and the answer really is cut short.
#[test]
fn a_truncated_answer_can_be_produced_with_no_surviving_report_of_the_truncation() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let rule = rule_from("secondly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 0).unwrap(), &rule, &mut meter).unwrap();
    let window =
        Window::new(at(1997, 9, 2, 0).unwrap(), at(1997, 9, 3, 0).unwrap()).expect("one day");
    let mut sink: Vec<Diagnostic> = Vec::new();

    let starts: Vec<Instant> = input
        .search(window, &mut meter, &mut sink)
        .map_while(SearchStep::occurrence)
        .map(Occurrence::start)
        .collect();

    assert_eq!(
        starts.len(),
        86_400,
        "a day of FREQ=SECONDLY is 86,400 occurrences; anything less is a truncation, and the \
         meter reads exhausted={}",
        meter.is_exhausted()
    );
}

/// The accessor still composes into a one-line discard, whatever it is spelled with.
///
/// `.flat_map(SearchStep::occurrence)` is `.flatten()` with one word changed, because `Option`
/// implements `IntoIterator` — and this workspace's own Clippy profile refuses that spelling
/// (`clippy::flat_map_option`, pedantic) and directs the author to the `filter_map` written
/// below, which discards exactly as thoroughly. That is the ADR's claim holding and failing at
/// once: the gate catches the idiom inside this repository, and binds nobody downstream, which
/// is why `docs/adr/0002` says the guarantee's real boundary is the crate edge.
#[test]
fn the_accessor_composes_into_a_one_line_discard_of_the_terminal_state() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, 512);
    let rule = rule_from("daily_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1997, 9, 1, 0).unwrap(), at(2097, 9, 1, 0).unwrap())
        .expect("a century of days");
    let mut sink: Vec<Diagnostic> = Vec::new();

    let count = input
        .search(window, &mut meter, &mut sink)
        .filter_map(SearchStep::occurrence)
        .count();

    assert!(
        count < 36_000,
        "the answer is truncated, and the adapter said nothing about it"
    );
    assert!(
        meter.is_exhausted(),
        "the octet ledger did latch here, which is the one case the second report covers"
    );
}

/// The four idioms `docs/adr/0002` names are in fact refused by the compiler.
///
/// Not a runtime test — it cannot be, because the point is that the code does not exist. Each
/// of `search.flatten()`, `.filter_map(Result::ok)`, `.take_while(Result::is_ok)` and
/// `.collect::<Result<Vec<_>, _>>()` was compiled against this surface and rejected with
/// `E0277`, `E0631`, `E0631` and `E0277` respectively. `for step in search {}` and
/// `search.last()` do compile, which the ADR already states. This test records the finding so
/// the surviving half of the claim is evidence rather than assertion.
#[test]
fn the_result_shaped_idioms_the_adr_names_do_not_compile() {
    // The compile probe is in this test's history; what remains asserted here is that the
    // accessor the crate offers instead is deliberately not named `ok`.
    let step: SearchStep<'_> = SearchStep::BudgetExhausted(ical_recur::BudgetExhausted::new(
        Instant::from_unix_seconds(0),
        1,
    ));
    assert!(step.is_terminal());
    assert!(step.occurrence().is_none());
}

// ---------------------------------------------------------------------------------------------
// Section 2: is the terminal report's own number true?
// ---------------------------------------------------------------------------------------------

/// `BudgetExhausted::candidates_spent` counts candidates a period *produced*, not what the
/// budget paid for.
///
/// `crates/ical-recur/src/search.rs` says the terminal state carries what it spent "because a
/// caller deciding whether to retry with a larger budget needs to know it was close rather than
/// nowhere". The engine accumulates `set.len()` after each *successful* expansion, so a rule
/// that generates candidates in every period and instances in none — `accounting`'s own
/// worked example — spends the whole budget and reports having spent nothing.
#[test]
fn the_terminal_report_says_nothing_was_spent_when_the_whole_budget_was() {
    const BUDGET: u64 = 4_096;
    let mut meter = Meter::with_budget(Limits::DEFAULT, BUDGET);
    let rule = rule_from("yearly_february_thirtieth.recur", &mut meter).unwrap();
    let input = series(at(2001, 2, 1, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(2001, 1, 1, 0).unwrap(), at(9999, 1, 1, 0).unwrap())
        .expect("eight millennia");
    let mut sink: Vec<Diagnostic> = Vec::new();

    let terminal = {
        let mut search = input.search(window, &mut meter, &mut sink);
        let mut last = None;
        for step in &mut search {
            if let SearchStep::BudgetExhausted(exhausted) = step {
                last = Some(exhausted);
            }
        }
        last.expect("a rule no year satisfies must exhaust the budget")
    };

    assert!(
        meter.spent() >= BUDGET,
        "the budget really was spent: {}",
        meter.spent()
    );
    assert!(
        terminal.candidates_spent() > 0,
        "the terminal report claims {} candidates while the ledger paid for {}; a caller \
         deciding whether to retry is told it got nowhere",
        terminal.candidates_spent(),
        meter.spent()
    );
}

/// The same number, reached through the per-period ceiling.
///
/// A period refused while filling charged every candidate up to the ceiling, and the terminal
/// report names none of them.
#[test]
fn a_period_refused_while_filling_reports_none_of_the_candidates_it_charged() {
    const CEILING: u32 = 64;
    let limits = Limits::DEFAULT.with_candidates_per_period(CEILING);
    let mut meter = Meter::new(limits);
    let rule = rule_from("yearly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 2, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1997, 9, 1, 0).unwrap(), at(2010, 9, 1, 0).unwrap())
        .expect("a decade of years");
    let mut sink: Vec<Diagnostic> = Vec::new();

    let terminal = {
        let mut search = input.search(window, &mut meter, &mut sink);
        let mut last = None;
        for step in &mut search {
            if let SearchStep::BudgetExhausted(exhausted) = step {
                last = Some(exhausted);
            }
        }
        last.expect("a 365-day year cannot be filled under a ceiling of 64")
    };

    assert_eq!(
        terminal.candidates_spent(),
        u64::from(CEILING),
        "the period charged {CEILING} candidates before it was refused"
    );
}

// ---------------------------------------------------------------------------------------------
// Section 3: does the bound bound the rules it exists for?
// ---------------------------------------------------------------------------------------------

/// `FREQ=SECONDLY` with no `COUNT` and no `UNTIL` over a month, under the calibrated default.
#[test]
fn secondly_over_a_month_is_a_reported_outcome_under_the_calibrated_budget() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, DEFAULT_CANDIDATE_BUDGET);
    let rule = rule_from("secondly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 1, 0).unwrap(), &rule, &mut meter).unwrap();
    let window =
        Window::new(at(1997, 9, 1, 0).unwrap(), at(1997, 10, 1, 0).unwrap()).expect("one month");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal,
        "an infinite series inside a month must stop: {run:?}"
    );
    assert!(!run.outcome.is_complete());
}

/// The same rule under the meter a caller gets by saying only `Meter::new(Limits::DEFAULT)`.
///
/// `DEFAULT_CANDIDATE_BUDGET` is a constant a caller must pass to `Meter::with_budget`;
/// `Meter::new` budgets the ledger at `Limits::max_input_bytes`, which is 16 MiB. The two
/// numbers differ by sixty-four times, so the calibration `accounting`'s workload table argues
/// for is not what an unconfigured caller gets.
#[test]
fn secondly_over_a_month_is_a_reported_outcome_under_an_unconfigured_meter() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let rule = rule_from("secondly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1997, 9, 1, 0).unwrap(), &rule, &mut meter).unwrap();
    let window =
        Window::new(at(1997, 9, 1, 0).unwrap(), at(1997, 10, 1, 0).unwrap()).expect("one month");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal,
        "an infinite series inside a month must stop: {run:?}"
    );
}

/// `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1`, the rule `docs/adr/0002` opens with.
///
/// Its matches are rare enough that a naive generator walks years between them. The claim is
/// that it is bounded either way, and that whichever way it ends is said out loud.
#[test]
fn a_rule_that_walks_years_between_matches_is_bounded_and_says_how_it_ended() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, DEFAULT_CANDIDATE_BUDGET);
    let rule = rule_from("yearly_monday_the_first.recur", &mut meter).unwrap();
    let input = series(at(2001, 1, 1, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(2001, 1, 1, 0).unwrap(), at(9999, 1, 1, 0).unwrap())
        .expect("eight millennia");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal || run.outcome.is_complete(),
        "the walk ended with an answer rather than running: {run:?}"
    );
}

/// A negative `BYSETPOS` over a period whose candidate set is enormous.
///
/// `docs/adr/0002` amendment 8 says a negative position "selects from a period that was charged
/// as it filled, so a negative position cannot do unbounded uncharged work inside one `next()`".
#[test]
fn a_negative_by_set_pos_over_an_enormous_period_is_charged_before_it_selects() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, DEFAULT_CANDIDATE_BUDGET);
    let rule = rule_from("negative_setpos_enormous_period.recur", &mut meter).unwrap();
    let input = series(at(2001, 1, 1, 0).unwrap(), &rule, &mut meter).unwrap();
    let window =
        Window::new(at(2001, 1, 1, 0).unwrap(), at(2101, 1, 1, 0).unwrap()).expect("a century");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal || run.outcome.is_complete(),
        "one `next()` filled a year of seconds and answered: {run:?}"
    );
}

/// A window spanning ten thousand years, which is the whole calendar RFC 5545 can write.
#[test]
fn a_window_spanning_the_whole_calendar_is_a_reported_outcome() {
    let mut meter = Meter::with_budget(Limits::DEFAULT, DEFAULT_CANDIDATE_BUDGET);
    let rule = rule_from("yearly_unbounded.recur", &mut meter).unwrap();
    let input = series(at(1, 1, 1, 9).unwrap(), &rule, &mut meter).unwrap();
    let window = Window::new(at(1, 1, 1, 0).unwrap(), at(9999, 12, 31, 0).unwrap())
        .expect("the whole calendar");

    let run = drain(input, window, &mut meter);

    assert!(
        run.terminal || run.outcome.is_complete(),
        "ten thousand years of a yearly rule ended with an answer: {run:?}"
    );
}

/// An `EXDATE` list with a million entries.
#[test]
fn a_million_exdates_are_refused_rather_than_walked() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let rule = rule_from("daily_unbounded.recur", &mut meter).unwrap();
    let exdates: Vec<Instant> = (0..1_000_000_i64)
        .map(|step| Instant::from_unix_seconds(step.saturating_mul(86_400)))
        .collect();
    let refused = RecurrenceInput::new(
        at(1997, 9, 2, 9).unwrap(),
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &exdates,
        OverrideSet::empty(),
        &mut meter,
    );

    assert!(
        refused.is_err(),
        "a million exclusions is past `Limits::exdate_entries` and must be told so"
    );
}

/// An override set with a million entries, reachable through `RECURRENCE-ID`.
#[test]
fn a_million_overrides_are_refused_rather_than_scanned() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let entries: Vec<Override<'static>> = (0..1_000_000_i64)
        .map(|step| {
            Override::new(
                Instant::from_unix_seconds(step.saturating_mul(86_400)),
                OverrideRange::ThisOnly,
                None,
                PropertyDiff::empty(),
            )
        })
        .collect();

    assert!(
        OverrideSet::new(&entries, &mut meter).is_err(),
        "a million overrides is past `Limits::override_entries` and must be told so"
    );
}
