// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An attack on `ical-recur` from the specification's own answers.
//!
//! `crates/icalkit-conformance/tests/rfc5545_recurrence_examples.rs` already holds every worked example
//! RFC 5545 section 3.8.5.3 prints, asked about the window the RFC drew around its own output.
//! This file asks the specification the same questions from three angles that file does not
//! cover, each of which the RFC answers on its own:
//!
//! 1. **The same worked examples, through a window that opens partway into the answer.** A
//!    recurrence set is one set; a caller naming a later window asks for the part of it that
//!    lands inside. `COUNT` is still counted from `DTSTART`, an `INTERVAL` still counts from
//!    `DTSTART`'s own period, and `UNTIL` still ends where the RFC says. None of the three is a
//!    property of where the caller happened to point, so the expected column is the RFC's
//!    printed answer intersected with the window and nothing more was derived.
//! 2. **Section 3.3.10's prose answers.** "-1MO represents the last Monday of the month", "-306
//!    represents the 306th to the last day of the year (March 1st)", "'the last work day of the
//!    month' could be represented as FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1". Each of
//!    those sentences is an answer the RFC gives outside the worked table.
//! 3. **The expand/limit table, row by row.** For each cell under attack the rule is chosen so
//!    that reading the cell as `Expand`, as `Limit` and as `N/A` give three different answers. A
//!    row implemented backwards is invisible on the rules whose `DTSTART` already sits where the
//!    part points, which is most of them, and wrong on the rest.
//!
//! Every expectation is transcribed into `tests/fixtures/break_recur_rfc/` beside the sentence
//! of the RFC it comes from, so a reviewer checks the table against the document rather than
//! against this code. Nothing here was read off the implementation.
//!
//! The harness reports every disagreeing case in one failure rather than stopping at the first,
//! because a lens is worth more as a list than as a single row.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, Instant, Limits, Meter, UtcOffset,
};
use icalkit_conformance::internal::recur::{
    OverrideSet, RecurrenceInput, ValueKind, Window, parse_recur,
};

/// One line of a fixture: a rule, a window, and the answer the RFC gives for the pair.
#[derive(Clone, Debug)]
struct Case {
    /// The RFC's own words for the question, or the cell of the table under attack.
    name: String,
    /// `DTSTART`, read onto the UTC timeline as the sibling corpus file reads it.
    dtstart: Instant,
    /// The `RECUR` value, with the RFC's folds joined.
    rule: String,
    /// The `EXDATE` values the case carries, ascending.
    exdates: Vec<Instant>,
    /// The half-open window the caller asks about.
    window: Window,
    /// The answer, in order.
    expected: Vec<Instant>,
}

/// Seconds in one day, which is the stride the rendering of an instant divides by first.
const SECONDS_PER_DAY: i64 = 86_400;

/// Seconds in one hour.
const SECONDS_PER_HOUR: i64 = 3_600;

/// Seconds in one minute.
const SECONDS_PER_MINUTE: i64 = 60;

/// Days from the proleptic Gregorian civil epoch to the Unix epoch, as the shift-to-March
/// algorithm this file's rendering uses counts them.
const EPOCH_SHIFT_DAYS: i64 = 719_468;

/// Days in one four-hundred-year Gregorian era.
const DAYS_PER_ERA: i64 = 146_097;

/// The instant `YYYYMMDDTHHMMSS` names on the UTC timeline.
///
/// `None` rather than a panic for a mistyped literal, so a fixture typo is a named failure of
/// the case that carries it instead of a plausible wrong instant compared against another one.
fn instant_of(text: &str) -> Option<Instant> {
    let bytes = text.as_bytes();
    if bytes.len() != 15 || bytes.get(8) != Some(&b'T') {
        return None;
    }
    let year = u16::try_from(number(text.get(0..4)?)?).ok()?;
    let month = u8::try_from(number(text.get(4..6)?)?).ok()?;
    let day = u8::try_from(number(text.get(6..8)?)?).ok()?;
    let hour = u8::try_from(number(text.get(9..11)?)?).ok()?;
    let minute = u8::try_from(number(text.get(11..13)?)?).ok()?;
    let second = u8::try_from(number(text.get(13..15)?)?).ok()?;
    let date = CivilDate::from_ymd(year, month, day)?;
    let time = CivilTime::from_hms(hour, minute, second)?;
    CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)
}

/// `text` read as a decimal number, or `None` when it is not all digits.
fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Every instant a comma-separated fixture column names.
fn instants_of(column: &str) -> Option<Vec<Instant>> {
    if column.trim().is_empty() {
        return Some(Vec::new());
    }
    column
        .split(',')
        .map(|item| instant_of(item.trim()))
        .collect()
}

/// The fixture file at `name`, as its cases.
fn cases_of(name: &str) -> Option<Vec<Case>> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/break_recur_rfc");
    path.push(name);
    let text = fs::read_to_string(path).ok()?;
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        found.push(case_of(line)?);
    }
    Some(found)
}

/// One fixture line, as a case.
fn case_of(line: &str) -> Option<Case> {
    let columns: Vec<&str> = line.split('|').map(str::trim).collect();
    let [name, dtstart, rule, exdates, from, until, expected] = columns.as_slice() else {
        return None;
    };
    Some(Case {
        name: (*name).to_owned(),
        dtstart: instant_of(dtstart)?,
        rule: (*rule).to_owned(),
        exdates: instants_of(exdates)?,
        window: Window::new(instant_of(from)?, instant_of(until)?)?,
        expected: instants_of(expected)?,
    })
}

/// What one search produced and whether it finished.
#[derive(Debug)]
struct Run {
    /// The effective start of every occurrence, in the order it was emitted.
    starts: Vec<Instant>,
    /// Whether the search ran to the end of the rule or the window rather than to the budget.
    complete: bool,
}

/// Expand one case through `ical-recur`'s public surface only.
///
/// One meter for the decode and the expansion together, which is the shape a caller reading a
/// file has.
fn run(case: &Case) -> Result<Run, String> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let decoded = parse_recur(case.rule.as_bytes(), &mut meter, &mut reported)
        .map_err(|error| format!("the rule did not decode: {error:?}"))?;
    let input = RecurrenceInput::new(
        case.dtstart,
        ValueKind::DateTime,
        Some(&decoded),
        &[],
        &case.exdates,
        OverrideSet::empty(),
        &mut meter,
    )
    .map_err(|error| format!("the input was refused: {error:?}"))?;

    let mut starts = Vec::new();
    let complete = {
        let mut search = input.search(case.window, &mut meter, &mut reported);
        for step in search.by_ref() {
            match step.occurrence() {
                Some(occurrence) => starts.push(occurrence.start()),
                None => break,
            }
        }
        search.outcome().is_complete()
    };
    Ok(Run { starts, complete })
}

/// `YYYYMMDDTHHMMSS` for an instant, so a disagreement reads as dates rather than as epochs.
fn shown(at: Instant) -> String {
    civil_of(at).unwrap_or_else(|| format!("unix:{}", at.unix_seconds()))
}

/// The UTC civil reading of `at`, written the way a fixture writes one.
fn civil_of(at: Instant) -> Option<String> {
    let seconds = at.unix_seconds();
    let days = seconds.checked_div_euclid(SECONDS_PER_DAY)?;
    let inside = seconds.checked_rem_euclid(SECONDS_PER_DAY)?;
    let hour = inside.checked_div_euclid(SECONDS_PER_HOUR)?;
    let minute = inside
        .checked_rem_euclid(SECONDS_PER_HOUR)?
        .checked_div_euclid(SECONDS_PER_MINUTE)?;
    let second = inside.checked_rem_euclid(SECONDS_PER_MINUTE)?;
    let (year, month, day) = ymd_of(days)?;
    Some(format!(
        "{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}"
    ))
}

/// The civil year, month and day `days` after the Unix epoch.
///
/// The shift-to-March calendar algorithm, written in checked arithmetic because this crate's
/// profile denies the bare operators everywhere including a test's own scaffolding. It exists
/// only so a failure message reads in dates; nothing asserted here depends on it.
fn ymd_of(days: i64) -> Option<(i64, i64, i64)> {
    let shifted = days.checked_add(EPOCH_SHIFT_DAYS)?;
    let era = shifted.checked_div_euclid(DAYS_PER_ERA)?;
    let day_of_era = shifted.checked_rem_euclid(DAYS_PER_ERA)?;
    let year_of_era = day_of_era
        .checked_sub(day_of_era.checked_div_euclid(1460)?)?
        .checked_add(day_of_era.checked_div_euclid(36_524)?)?
        .checked_sub(day_of_era.checked_div_euclid(146_096)?)?
        .checked_div_euclid(365)?;
    let shifted_year = year_of_era.checked_add(era.checked_mul(400)?)?;
    let day_of_year = day_of_era.checked_sub(
        year_of_era
            .checked_mul(365)?
            .checked_add(year_of_era.checked_div_euclid(4)?)?
            .checked_sub(year_of_era.checked_div_euclid(100)?)?,
    )?;
    let shifted_month = day_of_year
        .checked_mul(5)?
        .checked_add(2)?
        .checked_div_euclid(153)?;
    let day = day_of_year
        .checked_sub(
            shifted_month
                .checked_mul(153)?
                .checked_add(2)?
                .checked_div_euclid(5)?,
        )?
        .checked_add(1)?;
    let month = if shifted_month < 10 {
        shifted_month.checked_add(3)?
    } else {
        shifted_month.checked_sub(9)?
    };
    let year = if month <= 2 {
        shifted_year.checked_add(1)?
    } else {
        shifted_year
    };
    Some((year, month, day))
}

/// Every instant of `found`, rendered.
fn listed(found: &[Instant]) -> String {
    found
        .iter()
        .map(|at| shown(*at))
        .collect::<Vec<_>>()
        .join(",")
}

/// Run every case of one fixture file and describe the ones that disagreed with the RFC.
fn disagreements(file: &str) -> Vec<String> {
    let Some(cases) = cases_of(file) else {
        return vec![format!("{file}: the fixture did not parse")];
    };
    let mut broken = Vec::new();
    for case in &cases {
        let mut note = String::new();
        match run(case) {
            Err(reason) => {
                let _unused = write!(note, "{}: {reason}", case.name);
            },
            Ok(produced) => {
                if produced.starts == case.expected && produced.complete {
                    continue;
                }
                let _unused = write!(
                    note,
                    "{}\n    rule      {}\n    expected  [{}]\n    produced  [{}]{}",
                    case.name,
                    case.rule,
                    listed(&case.expected),
                    listed(&produced.starts),
                    if produced.complete {
                        ""
                    } else {
                        "\n    and the search did not finish"
                    }
                );
            },
        }
        broken.push(note);
    }
    broken
}

/// Report every disagreement of one fixture at once.
fn assert_agrees(file: &str) {
    let broken = disagreements(file);
    assert!(
        broken.is_empty(),
        "{file}: {} of its cases disagree with RFC 5545\n\n{}\n",
        broken.len(),
        broken.join("\n\n")
    );
}

/// Section 3.8.5.3's examples answer the same set when the window opens partway through them.
#[test]
fn rfc5545_3_8_5_3_a_later_window_yields_the_tail_of_the_printed_answer() {
    assert_agrees("window_offsets_of_3_8_5_3.txt");
}

/// Section 3.3.10's prose answers are answers.
#[test]
fn rfc5545_3_3_10_answers_the_specification_gives_outside_the_worked_table() {
    assert_agrees("section_3_3_10_worked_answers.txt");
}

/// Every cell of the expand/limit table under attack behaves as the printed table says.
#[test]
fn rfc5545_3_3_10_every_row_of_the_expand_limit_table_reads_the_way_it_is_printed() {
    assert_agrees("expand_limit_table.txt");
}

/// The recurrence set begins at `DTSTART`, `UNTIL` bounds it inclusively, `COUNT` counts from
/// the start and not from the window.
#[test]
fn rfc5545_3_3_10_the_recurrence_set_begins_and_ends_where_the_specification_says() {
    assert_agrees("bounds_of_the_recurrence_set.txt");
}

/// Section 3.8.5.3's five intraday examples answer the same set from a window inside a day.
#[test]
fn rfc5545_3_8_5_3_an_intraday_window_yields_the_tail_of_the_printed_readings() {
    assert_agrees("intraday_windows_of_3_8_5_3.txt");
}

/// An instance whose date does not exist is ignored and is not counted.
#[test]
fn rfc5545_3_3_10_an_instance_with_an_invalid_date_is_ignored_and_not_counted() {
    assert_agrees("invalid_dates_and_leap_days.txt");
}

/// `BYSETPOS` counts within one interval, and week one is where the numbering rule puts it.
#[test]
fn rfc5545_3_3_10_by_set_pos_counts_one_interval_and_week_one_is_where_it_falls() {
    assert_agrees("bysetpos_and_week_numbering.txt");
}

/// The cells of the table under `SECONDLY`, `MINUTELY` and `HOURLY`, and the clock rows'
/// remaining `Expand` cells.
#[test]
fn rfc5545_3_3_10_the_sub_daily_cells_of_the_expand_limit_table_read_the_way_they_print() {
    assert_agrees("expand_limit_table_sub_daily.txt");
}

/// Rules the RFC's own sentences make equal to answers it has already printed.
#[test]
fn rfc5545_3_3_10_a_restated_rule_answers_what_the_rfc_printed_for_its_first_spelling() {
    assert_agrees("restatements_of_printed_answers.txt");
}

/// Ordinals that count far back, and spellings the specification makes equal.
#[test]
fn rfc5545_3_3_10_an_ordinal_counting_back_measures_the_run_it_counts_within() {
    assert_agrees("negative_ordinals_and_equivalences.txt");
}

/// Fields derived from `DTSTART`, intervals that cross something, and the end of the calendar.
#[test]
fn rfc5545_3_3_10_an_unstated_field_comes_from_dtstart_and_an_interval_crosses_the_calendar() {
    assert_agrees("defaults_intervals_and_calendar_edges.txt");
}
