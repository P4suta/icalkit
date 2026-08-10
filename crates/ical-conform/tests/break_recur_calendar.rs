// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The calendar arithmetic of `ical-recur`, attacked where the Gregorian calendar is not a
//! uniform grid: months of unequal length, leap days, week-numbering years that do not
//! coincide with calendar years, and an `UNTIL` written on a clock the rule did not agree to.
//!
//! Every case here is a committed `.ics` fixture read through `ical-core` and expanded through
//! `ical-recur`'s public surface, so nothing asserted below depends on a shape only the crate's
//! own tests can reach. The expected column is transcribed from RFC 5545 section 3.3.10 and
//! from ISO 8601 week numbering as that section adopts it — never from what this engine
//! returns. Where the specification genuinely permits more than one answer, the case is in
//! [`DIVERGENCES`] and the comment beside it names every permitted outcome and which one this
//! implementation chose.
//!
//! # What the specification actually says about `BYWEEKNO`
//!
//! Section 3.3.10 puts `BYWEEKNO` in the "Expand" column of its Note 2 table for
//! `FREQ=YEARLY`. A yearly period names a calendar year, and `BYWEEKNO=1` expands that period
//! to *week one of that year* — a seven-day span that begins on `WKST` and that, in a year
//! whose January 1st is a Tuesday, Wednesday or Thursday, begins in December of the year
//! before. It does not mean "the days of this calendar year that happen to carry the week
//! number 1", which is a different set: that set omits week one's December days and takes in
//! the December days belonging to week one of the *following* year.
//!
//! The two readings agree on the union over consecutive years, which is why a rule with
//! `INTERVAL=1` and no `BYSETPOS` cannot tell them apart, and why the RFC's own week-20 example
//! passes under either. They disagree the moment a period is skipped, a set is selected from,
//! or a year's week count is asked about — and those are the three cases below.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, ContentLineReader, Diagnostic, DiagnosticCode, Document,
    Instant, Limits, Meter, PropertyId, UtcOffset,
};
use ical_recur::{
    DEFAULT_CANDIDATE_BUDGET, OverrideSet, RecurrenceInput, SearchOutcome, ValueKind, Window,
    parse_recur,
};

/// A wall-clock reading on the timeline the caller resolved, as the tables write one.
type Reading = (u16, u8, u8, u8, u8);

/// One case: a fixture, the window asked about, and the answer the specification gives.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// The fixture's file name, for the assertion message.
    name: &'static str,
    /// The fixture's octets exactly as committed.
    octets: &'static [u8],
    /// The half-open window the search is asked about.
    window: (Reading, Reading),
    /// Every occurrence start the answer holds, in order.
    expected: &'static [Reading],
}

/// Midnight, which is how both edges of every window below are written.
const MIDNIGHT: (u8, u8) = (0, 0);

/// Cases where RFC 5545 gives one answer and this engine gives another.
const BREAKS: &[Case] = &[
    // Week one of 2018 is January 1st-7th and week one of 2020 is December 30th 2019 to
    // January 5th 2020, so an every-other-year rule anchored in 2018 names the Mondays
    // 2018-01-01, 2019-12-30, 2022-01-03, 2024-01-01 and 2025-12-29. `DTSTART` is the first of
    // them, so the recurrence set is synchronized and section 3.8.5.3's "undefined" escape
    // does not apply.
    Case {
        name: "byweekno_interval_two.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_interval_two.ics"),
        window: ((2018, 1, 1, 0, 0), (2027, 1, 1, 0, 0)),
        expected: &[
            (2018, 1, 1, 9, 0),
            (2019, 12, 30, 9, 0),
            (2022, 1, 3, 9, 0),
            (2024, 1, 1, 9, 0),
            (2025, 12, 29, 9, 0),
        ],
    },
    // `BYSETPOS=1` over every weekday of week one selects that week's first day. Week one of
    // 2019 begins Monday 2018-12-31, which is `DTSTART`; week one of 2020 begins 2019-12-30;
    // 2021's begins 2021-01-04 and 2022's begins 2022-01-03. Period 2018's own first day,
    // 2018-01-01, precedes `DTSTART` and is not in the set.
    Case {
        name: "byweekno_setpos_first_day.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_setpos_first_day.ics"),
        window: ((2018, 1, 1, 0, 0), (2023, 1, 1, 0, 0)),
        expected: &[
            (2018, 12, 31, 9, 0),
            (2019, 12, 30, 9, 0),
            (2021, 1, 4, 9, 0),
            (2022, 1, 3, 9, 0),
        ],
    },
    // With weeks starting Sunday, week one of a year is the Sunday-to-Saturday week holding
    // January 4th, so 2014 runs to a week 53 (2014-12-28 to 2015-01-03) and 2020 does too
    // (2020-12-27 to 2021-01-02). 2016, 2018, 2022, 2024 and 2026 hold 52 weeks each and name
    // nothing. The Mondays and Thursdays of the two weeks that exist are the whole answer.
    Case {
        name: "byweekno_week_53_interval_two.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_week_53_interval_two.ics"),
        window: ((2014, 1, 1, 0, 0), (2027, 1, 1, 0, 0)),
        expected: &[
            (2014, 12, 29, 9, 0),
            (2015, 1, 1, 9, 0),
            (2020, 12, 28, 9, 0),
            (2020, 12, 31, 9, 0),
        ],
    },
    // 2015 holds 52 weeks when weeks start on Sunday: week one is 2015-01-04 to 2015-01-10 and
    // week 52, the last, is 2015-12-27 to 2016-01-02. `BYWEEKNO=53` therefore names no day of
    // 2015 at all and the answer is empty. January 1st 2015 belongs to week 53 of *2014*.
    Case {
        name: "byweekno_week_53_wkst_su_absent.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_week_53_wkst_su_absent.ics"),
        window: ((2015, 1, 1, 0, 0), (2016, 1, 1, 0, 0)),
        expected: &[],
    },
];

/// Cases where RFC 5545 gives one answer and this engine gives it.
///
/// These are the evidence that the four above are defects in one mechanism rather than the
/// engine being wrong about the calendar generally.
const SURVIVORS: &[Case] = &[
    // Section 3.8.5.3's own week-number example, which both readings of `BYWEEKNO` satisfy
    // because week 20 is nowhere near a year boundary.
    Case {
        name: "byweekno_week_20_monday.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_week_20_monday.ics"),
        window: ((1997, 1, 1, 0, 0), (2000, 1, 1, 0, 0)),
        expected: &[
            (1997, 5, 12, 9, 0),
            (1998, 5, 11, 9, 0),
            (1999, 5, 17, 9, 0),
        ],
    },
    // `WKST` is read, and it moves week one: starting Monday, week one of 2015 is 2014-12-29
    // to 2015-01-04, whose Monday precedes `DTSTART`.
    Case {
        name: "byweekno_first_week_wkst_mo.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_first_week_wkst_mo.ics"),
        window: ((2014, 6, 1, 0, 0), (2018, 1, 1, 0, 0)),
        expected: &[(2016, 1, 4, 9, 0), (2017, 1, 2, 9, 0)],
    },
    // Starting Sunday, the same week one is 2015-01-04 to 2015-01-10, whose Monday is
    // 2015-01-05 — a different first instance from the same fixture text but one letter.
    Case {
        name: "byweekno_first_week_wkst_su.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/byweekno_first_week_wkst_su.ics"),
        window: ((2014, 6, 1, 0, 0), (2018, 1, 1, 0, 0)),
        expected: &[(2015, 1, 5, 9, 0), (2016, 1, 4, 9, 0), (2017, 1, 2, 9, 0)],
    },
    // Section 3.3.10 requires an instance naming a date the month lacks to be ignored, never
    // moved. 2024 has seven months with a 31st.
    Case {
        name: "monthly_bymonthday_31.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/monthly_bymonthday_31.ics"),
        window: ((2024, 1, 1, 0, 0), (2025, 1, 1, 0, 0)),
        expected: &[
            (2024, 1, 31, 9, 0),
            (2024, 3, 31, 9, 0),
            (2024, 5, 31, 9, 0),
            (2024, 7, 31, 9, 0),
            (2024, 8, 31, 9, 0),
            (2024, 10, 31, 9, 0),
            (2024, 12, 31, 9, 0),
        ],
    },
    // A yearly rule takes its month and day from `DTSTART`, so a leap-day series exists only in
    // leap years and the intervening three are skipped rather than moved to February 28th.
    Case {
        name: "yearly_from_february_29.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/yearly_from_february_29.ics"),
        window: ((2024, 1, 1, 0, 0), (2037, 1, 1, 0, 0)),
        expected: &[
            (2024, 2, 29, 9, 0),
            (2028, 2, 29, 9, 0),
            (2032, 2, 29, 9, 0),
            (2036, 2, 29, 9, 0),
        ],
    },
    // A rule no Gregorian year satisfies, over a century-wide window: the answer is empty and
    // the search reaches the end of the window rather than searching forever.
    Case {
        name: "yearly_february_30_never_matches.ics",
        octets: include_bytes!(
            "fixtures/break_recur_calendar/yearly_february_30_never_matches.ics"
        ),
        window: ((2024, 1, 1, 0, 0), (2124, 1, 1, 0, 0)),
        expected: &[],
    },
    // The 366th day exists only in a leap year, and it is December 31st there.
    Case {
        name: "yearly_byyearday_366_leap.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/yearly_byyearday_366_leap.ics"),
        window: ((2024, 1, 1, 0, 0), (2040, 1, 1, 0, 0)),
        expected: &[
            (2024, 12, 31, 9, 0),
            (2028, 12, 31, 9, 0),
            (2032, 12, 31, 9, 0),
        ],
    },
    // The same part in a common year names nothing, and the `UNTIL` closes the series.
    Case {
        name: "yearly_byyearday_366_common.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/yearly_byyearday_366_common.ics"),
        window: ((2023, 1, 1, 0, 0), (2024, 1, 1, 0, 0)),
        expected: &[],
    },
    // `BYYEARDAY=-1` is December 31st in every year, leap or common.
    Case {
        name: "yearly_byyearday_minus_one.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/yearly_byyearday_minus_one.ics"),
        window: ((2023, 1, 1, 0, 0), (2025, 1, 1, 0, 0)),
        expected: &[(2023, 12, 31, 9, 0), (2024, 12, 31, 9, 0)],
    },
    // `BYYEARDAY=-366` is January 1st of a leap year and nothing at all in a common one.
    Case {
        name: "yearly_byyearday_minus_366.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/yearly_byyearday_minus_366.ics"),
        window: ((2023, 1, 1, 0, 0), (2025, 1, 1, 0, 0)),
        expected: &[(2024, 1, 1, 9, 0)],
    },
    // Section 3.3.10 makes `UNTIL` an inclusive bound, so an instance landing exactly on it is
    // the last one rather than the first one dropped.
    Case {
        name: "until_utc_on_the_key.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/until_utc_on_the_key.ics"),
        window: ((1997, 9, 1, 0, 0), (1997, 10, 1, 0, 0)),
        expected: &[
            (1997, 9, 2, 9, 0),
            (1997, 9, 3, 9, 0),
            (1997, 9, 4, 9, 0),
            (1997, 9, 5, 9, 0),
        ],
    },
];

/// Cases the specification does not settle, recorded with the answer this engine chose.
///
/// `docs/adr/0006` makes these corpus entries rather than defects. Each comment names the
/// outcomes the RFC permits and the ecosystem's split, and the assertion holds this engine to
/// the one it picked so that a silent change of mind is a failing test.
const DIVERGENCES: &[Case] = &[
    // Section 3.3.10 requires `UNTIL` to carry `DTSTART`'s value type and says nothing about
    // what a file violating that means. Reading a `DATE` `UNTIL` as midnight, as here, drops
    // the 09:00 instance of the named day; reading it as the end of that day would keep it.
    // dateutil, libical and ical.js all read midnight, so midnight is the answer to keep — and
    // the mismatch itself is reported, which is what makes the choice auditable.
    Case {
        name: "until_date_against_date_time.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/until_date_against_date_time.ics"),
        window: ((1997, 9, 1, 0, 0), (1997, 10, 1, 0, 0)),
        expected: &[(1997, 9, 2, 9, 0), (1997, 9, 3, 9, 0), (1997, 9, 4, 9, 0)],
    },
    // A `DATE-TIME` `UNTIL` with no trailing `Z` violates section 3.3.10 whenever `DTSTART` is
    // UTC or zoned, and Google has emitted exactly that. This crate holds no zone, so it reads
    // the wall clock at UTC and compares on the timeline the caller resolved — the reading that
    // agrees with the caller when the caller resolved `DTSTART` the same way, and the one that
    // is a day out when it did not. `UntilClock::Floating` is where the crate names the clock;
    // no diagnostic does, which is recorded rather than asserted here.
    Case {
        name: "until_floating_against_utc.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/until_floating_against_utc.ics"),
        window: ((1997, 9, 1, 0, 0), (1997, 10, 1, 0, 0)),
        expected: &[
            (1997, 9, 2, 9, 0),
            (1997, 9, 3, 9, 0),
            (1997, 9, 4, 9, 0),
            (1997, 9, 5, 9, 0),
        ],
    },
    // Section 3.8.5.3 does not say whether `COUNT` bounds the rule's expansion or the
    // recurrence set left after `EXDATE`. Counting the excluded instance yields four
    // occurrences here; not counting it would yield five. dateutil, libical, ical.js and
    // Google all count it, and this engine agrees with them.
    Case {
        name: "daily_count_with_exdate.ics",
        octets: include_bytes!("fixtures/break_recur_calendar/daily_count_with_exdate.ics"),
        window: ((1997, 9, 1, 0, 0), (1997, 10, 1, 0, 0)),
        expected: &[
            (1997, 9, 2, 9, 0),
            (1997, 9, 3, 9, 0),
            (1997, 9, 5, 9, 0),
            (1997, 9, 6, 9, 0),
        ],
    },
    // Section 3.8.5.3 calls the set generated from a `DTSTART` the rule does not name
    // "undefined". September 2nd 1997 is a Tuesday and the rule names Mondays. Dropping it, as
    // here and as dateutil does, is one permitted answer; emitting it as the first instance, as
    // Google and several servers do, is the other. Dropping it is the answer to keep, because
    // emitting it would put an occurrence in the set that no `RECURRENCE-ID` derived from the
    // rule can address.
    Case {
        name: "weekly_byday_monday_from_tuesday.ics",
        octets: include_bytes!(
            "fixtures/break_recur_calendar/weekly_byday_monday_from_tuesday.ics"
        ),
        window: ((1997, 9, 1, 0, 0), (1997, 10, 1, 0, 0)),
        expected: &[
            (1997, 9, 8, 9, 0),
            (1997, 9, 15, 9, 0),
            (1997, 9, 22, 9, 0),
            (1997, 9, 29, 9, 0),
        ],
    },
];

/// The instant a table reading names on the timeline the caller resolved.
///
/// Fallible rather than total, so a mistyped literal in a table fails the case that read it
/// by name instead of becoming a plausible wrong instant compared against another one.
fn at(reading: Reading) -> Option<Instant> {
    let date = CivilDate::from_ymd(reading.0, reading.1, reading.2)?;
    let time = CivilTime::from_hms(reading.3, reading.4, 0)?;
    CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)
}

/// Every reading a case expects, as instants, or `None` if a table holds a date that is not one.
fn expected_instants(readings: &[Reading]) -> Option<Vec<Instant>> {
    readings.iter().copied().map(at).collect()
}

/// The first value in the document carrying `id`, searched depth first.
fn first_value(document: &Document, id: &PropertyId) -> Option<Vec<u8>> {
    fn walk(items: &[ical_core::Item], id: &PropertyId) -> Option<Vec<u8>> {
        for entry in items {
            match entry {
                ical_core::Item::Property(property) if property.has_id(id) => {
                    return Some(property.value_text().as_bytes().to_vec());
                },
                ical_core::Item::Component(component) => {
                    if let Some(found) = walk(component.items(), id) {
                        return Some(found);
                    }
                },
                ical_core::Item::Property(_) => {},
            }
        }
        None
    }
    walk(document.items(), id)
}

/// Decode `YYYYMMDD`, `YYYYMMDDTHHMMSS` or `YYYYMMDDTHHMMSSZ` onto the UTC timeline.
///
/// The fixtures write only UTC forms, so reading them at UTC is the caller-side normalization
/// `docs/adr/0003` requires rather than a shortcut around it.
fn decode(text: &[u8]) -> Option<(Instant, ValueKind)> {
    let digits = |from: usize, len: usize| -> Option<u32> {
        let slice = text.get(from..from.checked_add(len)?)?;
        core::str::from_utf8(slice).ok()?.parse::<u32>().ok()
    };
    let year = u16::try_from(digits(0, 4)?).ok()?;
    let month = u8::try_from(digits(4, 2)?).ok()?;
    let day = u8::try_from(digits(6, 2)?).ok()?;
    let date = CivilDate::from_ymd(year, month, day)?;
    if text.len() == 8 {
        let midnight = CivilTime::from_hms(MIDNIGHT.0, MIDNIGHT.1, 0)?;
        let instant = CivilDateTime::new(date, midnight).at_offset(UtcOffset::UTC)?;
        return Some((instant, ValueKind::Date));
    }
    let hour = u8::try_from(digits(9, 2)?).ok()?;
    let minute = u8::try_from(digits(11, 2)?).ok()?;
    let second = u8::try_from(digits(13, 2)?).ok()?;
    let time = CivilTime::from_hms(hour, minute, second)?;
    let instant = CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)?;
    Some((instant, ValueKind::DateTime))
}

/// What one search produced, and everything it said on the way.
#[derive(Debug)]
struct Run {
    /// The effective start of every occurrence, in emission order.
    starts: Vec<Instant>,
    /// How the search finished.
    outcome: SearchOutcome,
    /// Every diagnostic the search and the decode raised, in order.
    reported: Vec<Diagnostic>,
}

/// Read a fixture and expand it over `window`.
///
/// One meter for the decode and the expansion, budgeted at the crate's own stated default, so
/// a case that runs long is refused by the number a caller stating no policy would get.
fn run_case(case: Case) -> Option<Run> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::with_budget(limits, DEFAULT_CANDIDATE_BUDGET);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let mut reader = ContentLineReader::new(case.octets, limits.grammar());
    let document = Document::from_tokens(&mut reader, &mut meter, &mut reported).ok()?;

    let start_text = first_value(&document, &PropertyId::DTSTART)?;
    let (dtstart, kind) = decode(&start_text)?;
    let rule_text = first_value(&document, &PropertyId::RRULE)?;
    let exdates: Vec<Instant> = first_value(&document, &PropertyId::EXDATE)
        .unwrap_or_default()
        .split(|octet| *octet == b',')
        .filter_map(|entry| decode(entry).map(|decoded| decoded.0))
        .collect();

    let rule = parse_recur(&rule_text, &mut meter, &mut reported).ok()?;
    let input = RecurrenceInput::new(
        dtstart,
        kind,
        Some(&rule),
        &[],
        &exdates,
        OverrideSet::empty(),
        &mut meter,
    )
    .ok()?;

    let asked = Window::new(at(case.window.0)?, at(case.window.1)?)?;
    let mut starts = Vec::new();
    let outcome = {
        let mut search = input.search(asked, &mut meter, &mut reported);
        for step in search.by_ref() {
            match step.occurrence() {
                Some(occurrence) => starts.push(occurrence.start()),
                None => break,
            }
        }
        search.outcome()
    };
    Some(Run {
        starts,
        outcome,
        reported,
    })
}

/// A wall-clock rendering of an instant, so a failure names dates rather than epoch seconds.
fn render(instant: Instant) -> String {
    let Some(civil) = CivilDateTime::from_instant(instant, UtcOffset::UTC) else {
        return format!("<{}>", instant.unix_seconds());
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        civil.date().year(),
        civil.date().month(),
        civil.date().day(),
        civil.time().hour(),
        civil.time().minute()
    )
}

/// Every case in `table` that answered something other than what its comment transcribes.
///
/// Collected rather than asserted one at a time, so one run names every disagreement instead
/// of stopping at the first and hiding the shape of the defect behind it.
fn disagreements(table: &[Case]) -> Vec<String> {
    let mut found = Vec::new();
    for case in table {
        let (Some(run), Some(wanted)) = (run_case(*case), expected_instants(case.expected)) else {
            found.push(format!(
                "{} could not be assembled from its fixture",
                case.name
            ));
            continue;
        };
        if run.starts != wanted {
            let got: Vec<String> = run.starts.iter().copied().map(render).collect();
            let want: Vec<String> = wanted.iter().copied().map(render).collect();
            found.push(format!(
                "{}\n  expected {want:?}\n  answered {got:?}",
                case.name
            ));
        }
        if !run.outcome.is_complete() {
            found.push(format!("{} did not finish its window", case.name));
        }
    }
    found
}

/// Assert every case in `table` answers exactly what its comment transcribes.
fn assert_table(table: &[Case]) {
    let found = disagreements(table);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// A yearly period is a calendar year, and `BYWEEKNO` must expand it to the weeks *of that
/// year* rather than filter it by the numbers its own days carry.
#[test]
fn byweekno_expands_a_year_to_its_own_weeks() {
    assert_table(BREAKS);
}

/// The calendar arithmetic that does hold, so the failures above are one mechanism and not a
/// general inability to count days.
#[test]
fn the_calendar_edges_the_engine_answers_correctly_still_answer_correctly() {
    assert_table(SURVIVORS);
}

/// Where the RFC permits more than one answer, this engine keeps the one recorded beside it.
#[test]
fn the_undecided_cases_answer_what_the_corpus_records() {
    assert_table(DIVERGENCES);
}

/// A `DATE` `UNTIL` over a `DATE-TIME` `DTSTART` is compared anyway, and reported.
///
/// The comparison happens on the UTC timeline the caller resolved, at midnight of the named
/// day. What makes that auditable rather than silent is the diagnostic, so its absence would
/// be the defect even though the instants would not move.
#[test]
fn a_mismatched_until_value_type_names_itself() {
    let case = DIVERGENCES
        .iter()
        .find(|entry| entry.name == "until_date_against_date_time.ics")
        .copied()
        .expect("the fixture is in the table");
    let run = run_case(case).expect("the fixture assembles");
    assert!(
        run.reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::RecurrenceUntilValueTypeMismatch),
        "the UNTIL/DTSTART value type disagreement is reported"
    );
}

/// A rule that can never match, walked one second at a time, ends at the budget.
///
/// `docs/adr/0011` says an unsatisfiable rule "ends as the budget-exhausted outcome instead of
/// searching forever". A ten-year window at `FREQ=SECONDLY` is 315 million periods and none of
/// them holds a February 30th, so this is the case that ADR exists for. A test that never
/// returns is the failure being looked for here.
#[test]
fn a_per_second_walk_toward_an_impossible_date_stops_at_the_budget() {
    let case = Case {
        name: "secondly_february_30_never_matches.ics",
        octets: include_bytes!(
            "fixtures/break_recur_calendar/secondly_february_30_never_matches.ics"
        ),
        window: ((2023, 1, 1, 0, 0), (2033, 1, 1, 0, 0)),
        expected: &[],
    };
    let run = run_case(case).expect("the fixture assembles");
    assert!(run.starts.is_empty());
    assert!(
        matches!(run.outcome, SearchOutcome::BudgetExhausted(_)),
        "an unsatisfiable per-second rule is refused rather than answered, got {:?}",
        run.outcome
    );
}

/// An instance the calendar does not hold is skipped audibly, never clamped to a nearby date.
#[test]
fn a_skipped_instance_is_reported_and_no_neighboring_date_is_invented() {
    let case = SURVIVORS
        .iter()
        .find(|entry| entry.name == "monthly_bymonthday_31.ics")
        .copied()
        .expect("the fixture is in the table");
    let run = run_case(case).expect("the fixture assembles");
    assert!(
        run.reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::NonexistentRecurrenceInstance),
        "the months without a 31st are reported"
    );
    for invented in [
        (2024, 2, 29, 9, 0),
        (2024, 4, 30, 9, 0),
        (2024, 6, 30, 9, 0),
    ] {
        assert!(
            !run.starts.contains(&at(invented).expect("a real date")),
            "a clamping engine would have invented {invented:?}"
        );
    }
}
