// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Time zones, attacked from both sides of the seam `ical-recur` and `ical-tz` share.
//!
//! This is the only file in the workspace that names both crates, which is the point of it.
//! `ical-recur` walks a cadence and has no zone; `ical-tz` answers about a zone and expands no
//! rule; and M1 left the timeline between them half-specified with the words "the two ends have
//! to agree or every zoned series is an hour out for half the year". Everything below either
//! holds that agreement or measures the hour.
//!
//! # What each case is addressed to
//!
//! Per `docs/adr/0006` every case names the specification text it comes from, and every
//! expectation is transcribed from a real zone's published transition rules rather than read
//! off this workspace's answer:
//!
//! - **RFC 5545 section 3.6.5** (`VTIMEZONE`) — Europe/Berlin's 01:00 UTC transitions,
//!   `America/New_York`'s 02:00 local ones before and after the 2007 rule change,
//!   `Australia/Lord_Howe`'s thirty-minute step, and a definition whose transitions are
//!   `RDATE` lines that simply run out.
//! - **RFC 5545 section 3.3.5** (`DATE-TIME` form 3) — the hour that repeats and the hour that
//!   does not exist, as values.
//! - **RFC 5545 section 3.3.10** (`RECUR`) — an `UNTIL` written in UTC, an `UNTIL` written
//!   floating, and an `UNTIL` written as a `DATE` against a date-time `DTSTART`.
//! - **RFC 5545 section 3.8.5.1** (`EXDATE`) — a `DATE` exclusion against a date-time series.
//! - **RFC 5545 section 3.8.4.4** (`RECURRENCE-ID`) and **section 3.2.13** (`RANGE`) — an
//!   override that crossed a transition, and one that addresses no instance at all.
//! - **RFC 5545 section 3.2.19** (`TZID`) — the identifiers real files carry, none of which
//!   this workspace is entitled to translate.
//!
//! # The arithmetic every table below was written from
//!
//! Europe/Berlin runs CET (`+01:00`) and CEST (`+02:00`), moving on the last Sunday of March
//! and the last Sunday of October at 01:00 UTC. In 2026 those Sundays are March 29th and
//! October 25th, so a 09:00 wall clock is 08:00Z until March 29th, 07:00Z from March 29th to
//! October 24th, and 08:00Z again from October 25th. That is the whole content of the seam
//! case: the wall clock is what stays put and the instant is what moves.
//!
//! `America/New_York` moved its rules in 2007. Before that, daylight time began on the first
//! Sunday of April; after it, the second Sunday of March. So 2005-03-13T02:30 is an ordinary
//! Eastern Standard morning in a file that carries both rules, and 2026-03-08T02:30 does not
//! exist at all — the same wall clock, two answers, decided by which rule was in force.
//!
//! `Australia/Lord_Howe` moves by thirty minutes rather than an hour, so its gap is half an
//! hour wide and its fold is half an hour long. A resolver that special-cased one hour
//! anywhere answers this zone wrongly, which is why it is here.

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Component, DateTimeValue, DecodeValue, Diagnostic,
    DiagnosticCode, Document, Instant, Limits, Meter, UtcOffset, ValueType,
};
use icalkit_conformance::internal::recur::{
    Freq, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput, RecurrenceRule,
    RecurrenceRuleBuilder, RuleLimit, SearchStep, UntilClock, ValueKind, Window, generation_window,
    parse_recur,
};
use icalkit_conformance::internal::tz::{
    AnswerBasis, CombinedZoneSource, ExclusionReading, FixedOffsetSource, GapPolicy,
    LocalResolution, OrphanScan, PolicyOutcome, Reading, ResolutionPolicy, ResolvedExclusions,
    TransitionTable, Tzid, TzidForm, UntilReading, VtimezoneSet, WallClockShift, ZoneAnswer,
    ZoneProvenance, ZoneSource, ZonedSeries, extra_widening, nominal, read_calendar_zones,
    wall_clock,
};

/// The property identities this corpus reads, as statics.
///
/// `Component::properties_named` ties the lifetime of the walk to the borrow of the identity it
/// walks for, so an identity built at the call site would not outlive the values read through
/// it. A `static` is that lifetime, spelled once.
mod ids {
    use icalkit_conformance::internal::core::PropertyId;

    /// `DTSTART`, the wall clock a zoned series is anchored at.
    pub(crate) static DTSTART: PropertyId = PropertyId::DTSTART;
    /// `RRULE`, the one rule a series is expanded from.
    pub(crate) static RRULE: PropertyId = PropertyId::RRULE;
}

/// Europe/Berlin with its transitions as rules, and a daily 09:00 series inside it.
const BERLIN_DAILY_NINE: &[u8] = include_bytes!("fixtures/break_zones/berlin_daily_nine.ics");

/// Europe/Berlin with its transitions as `RDATE` lines that stop in 2029.
const BERLIN_RDATE_THROUGH_2029: &[u8] =
    include_bytes!("fixtures/break_zones/berlin_rdate_through_2029.ics");

/// The three identifier shapes real files carry, one `VTIMEZONE` each.
const VENDOR_TZIDS: &[u8] = include_bytes!("fixtures/break_zones/vendor_tzids.ics");

/// `America/New_York` carrying both the pre-2007 rules and the ones that replaced them.
const NEW_YORK_RULES_CHANGED: &[u8] =
    include_bytes!("fixtures/break_zones/new_york_rules_changed_in_2007.ics");

/// `Australia/Lord_Howe`, whose daylight saving step is thirty minutes.
const LORD_HOWE: &[u8] = include_bytes!("fixtures/break_zones/lord_howe_half_hour.ics");

/// The identifier every Europe/Berlin case names, spelled once.
const BERLIN: &str = "Europe/Berlin";

/// A wall clock as the tables below write one: year, month, day, hour, minute.
type Stamp = (u16, u8, u8, u8, u8);

/// One reading of a wall clock as a table writes it.
///
/// The instant it names, the offset in force in seconds, and whether the observance in force is
/// the zone's daylight one — the last taken from `DAYLIGHT` against `STANDARD` in the file and
/// never inferred from which offset is larger, because `Australia/Lord_Howe` would break that.
type Expectation = (Stamp, i32, bool);

/// What a wall clock is expected to name under a zone.
///
/// Transcribed from the zone's published transition rules. Nothing here is read off what this
/// workspace answers, which is the only thing that makes the table evidence.
#[derive(Clone, Copy, Debug)]
enum Expected {
    /// One instant, which is every day of the year but two.
    Unique(Expectation),
    /// Two instants: the earlier under the offset before the fall back, then the later.
    Ambiguous(Expectation, Expectation),
    /// None at all, because the zone sprang over this wall clock.
    Nonexistent {
        /// `TZOFFSETFROM`, in seconds.
        before: i32,
        /// `TZOFFSETTO`, in seconds.
        after: i32,
        /// The instant RFC 5545 section 3.3.5 reads this wall clock as, with the offset
        /// in force before the gap.
        shifted: Stamp,
    },
}

/// One zone, one wall clock, and what that zone's published rules say it names.
#[derive(Clone, Copy, Debug)]
struct ZoneCase {
    /// The name a failure is reported under.
    name: &'static str,
    /// The fixture the `VTIMEZONE` is read from.
    octets: &'static [u8],
    /// The identifier, exactly as the fixture writes it.
    tzid: &'static str,
    /// The wall clock put to the zone.
    asked: Stamp,
    /// What the zone's real rules say that wall clock names.
    expected: Expected,
}

/// The two awkward hours, under three zones that move their clocks differently.
const AWKWARD_HOURS: &[ZoneCase] = &[
    // Europe/Berlin falls back on the last Sunday of October at 01:00 UTC, so 02:30 on
    // October 25th 2026 is 00:30Z under CEST and 01:30Z under CET.
    ZoneCase {
        name: "berlin fold",
        octets: BERLIN_DAILY_NINE,
        tzid: BERLIN,
        asked: (2026, 10, 25, 2, 30),
        expected: Expected::Ambiguous(
            ((2026, 10, 25, 0, 30), 7200, true),
            ((2026, 10, 25, 1, 30), 3600, false),
        ),
    },
    // It springs forward on the last Sunday of March, also at 01:00 UTC: 02:00 CET becomes
    // 03:00 CEST, so the whole of 02:00-03:00 on March 29th 2026 is missing.
    ZoneCase {
        name: "berlin gap",
        octets: BERLIN_DAILY_NINE,
        tzid: BERLIN,
        asked: (2026, 3, 29, 2, 30),
        expected: Expected::Nonexistent {
            before: 3600,
            after: 7200,
            shifted: (2026, 3, 29, 1, 30),
        },
    },
    // An ordinary morning under the same table, which is what makes the two above findings
    // rather than a resolver that answers oddly everywhere.
    ZoneCase {
        name: "berlin ordinary winter morning",
        octets: BERLIN_DAILY_NINE,
        tzid: BERLIN,
        asked: (2026, 1, 15, 9, 0),
        expected: Expected::Unique(((2026, 1, 15, 8, 0), 3600, false)),
    },
    // America/New_York moves at 02:00 local. Under the post-2007 rules daylight time begins on
    // the second Sunday of March, which in 2026 is the 8th.
    ZoneCase {
        name: "new york gap under the modern rule",
        octets: NEW_YORK_RULES_CHANGED,
        tzid: "America/New_York",
        asked: (2026, 3, 8, 2, 30),
        expected: Expected::Nonexistent {
            before: -18_000,
            after: -14_400,
            shifted: (2026, 3, 8, 7, 30),
        },
    },
    // It ends on the first Sunday of November, which in 2026 is the 1st: 01:30 is 05:30Z under
    // EDT and 06:30Z under EST.
    ZoneCase {
        name: "new york fold under the modern rule",
        octets: NEW_YORK_RULES_CHANGED,
        tzid: "America/New_York",
        asked: (2026, 11, 1, 1, 30),
        expected: Expected::Ambiguous(
            ((2026, 11, 1, 5, 30), -14_400, true),
            ((2026, 11, 1, 6, 30), -18_000, false),
        ),
    },
    // Australia/Lord_Howe springs forward on the first Sunday of October by thirty minutes:
    // 02:00 becomes 02:30, so 02:15 is missing and 03:00 is not.
    ZoneCase {
        name: "lord howe half hour gap",
        octets: LORD_HOWE,
        tzid: "Australia/Lord_Howe",
        asked: (2026, 10, 4, 2, 15),
        expected: Expected::Nonexistent {
            before: 37_800,
            after: 39_600,
            shifted: (2026, 10, 3, 15, 45),
        },
    },
    ZoneCase {
        name: "lord howe just past the half hour gap",
        octets: LORD_HOWE,
        tzid: "Australia/Lord_Howe",
        asked: (2026, 10, 4, 3, 0),
        expected: Expected::Unique(((2026, 10, 3, 16, 0), 39_600, true)),
    },
    // It falls back on the first Sunday of April, by the same thirty minutes: 01:30-02:00
    // happens twice, once at +11:00 and once at +10:30.
    ZoneCase {
        name: "lord howe half hour fold",
        octets: LORD_HOWE,
        tzid: "Australia/Lord_Howe",
        asked: (2026, 4, 5, 1, 45),
        expected: Expected::Ambiguous(
            ((2026, 4, 4, 14, 45), 39_600, true),
            ((2026, 4, 4, 15, 15), 37_800, false),
        ),
    },
];

/// One zone, two eras, and the same wall clock answered differently in each.
const RULES_CHANGED_IN_2007: &[ZoneCase] = &[
    // Under the pre-2007 rule daylight time began on the first Sunday of April, which in 2005
    // was the 3rd, so 02:30 that morning does not exist.
    ZoneCase {
        name: "new york gap under the rule in force in 2005",
        octets: NEW_YORK_RULES_CHANGED,
        tzid: "America/New_York",
        asked: (2005, 4, 3, 2, 30),
        expected: Expected::Nonexistent {
            before: -18_000,
            after: -14_400,
            shifted: (2005, 4, 3, 7, 30),
        },
    },
    // The same March morning that does not exist in 2026 is an ordinary Eastern Standard one in
    // 2005, because the rule that removed it had not been written yet.
    ZoneCase {
        name: "new york march morning that existed in 2005",
        octets: NEW_YORK_RULES_CHANGED,
        tzid: "America/New_York",
        asked: (2005, 3, 13, 2, 30),
        expected: Expected::Unique(((2005, 3, 13, 7, 30), -18_000, false)),
    },
    // And the old rule ended daylight time on the last Sunday of October, the 30th in 2005,
    // rather than in November.
    ZoneCase {
        name: "new york fold under the rule in force in 2005",
        octets: NEW_YORK_RULES_CHANGED,
        tzid: "America/New_York",
        asked: (2005, 10, 30, 1, 30),
        expected: Expected::Ambiguous(
            ((2005, 10, 30, 5, 30), -14_400, true),
            ((2005, 10, 30, 6, 30), -18_000, false),
        ),
    },
];

/// The real UTC instants the March window's occurrences happen at.
///
/// 09:00 Berlin is 08:00Z under CET and 07:00Z under CEST, and the zone moves on the 29th.
const MARCH_STARTS: &[Stamp] = &[
    (2026, 3, 26, 8, 0),
    (2026, 3, 27, 8, 0),
    (2026, 3, 28, 8, 0),
    (2026, 3, 29, 7, 0),
    (2026, 3, 30, 7, 0),
    (2026, 3, 31, 7, 0),
    (2026, 4, 1, 7, 0),
];

/// The same series across the October transition, where the day is 25 hours long.
const OCTOBER_STARTS: &[Stamp] = &[
    (2026, 10, 22, 7, 0),
    (2026, 10, 23, 7, 0),
    (2026, 10, 24, 7, 0),
    (2026, 10, 25, 8, 0),
    (2026, 10, 26, 8, 0),
    (2026, 10, 27, 8, 0),
    (2026, 10, 28, 8, 0),
];

/// The two windows the seam case is measured over, and what Berlin's rules say happens inside.
const SEAM_WINDOWS: &[(Stamp, Stamp, &[Stamp])] = &[
    ((2026, 3, 26, 0, 0), (2026, 4, 2, 0, 0), MARCH_STARTS),
    ((2026, 10, 22, 0, 0), (2026, 10, 29, 0, 0), OCTOBER_STARTS),
];

/// The identifiers real files carry, with the shape each has and no translation of any.
const VENDOR_IDENTIFIERS: &[(&str, TzidForm)] = &[
    ("W. Europe Standard Time", TzidForm::Opaque),
    (
        "/mozilla.org/20050126_1/Europe/Berlin",
        TzidForm::GloballyUnique,
    ),
    ("Customized Time Zone", TzidForm::Opaque),
];

/// The civil date and time a table entry spells.
fn local(stamp: Stamp) -> Option<CivilDateTime> {
    let date = CivilDate::from_ymd(stamp.0, stamp.1, stamp.2)?;
    Some(CivilDateTime::new(
        date,
        CivilTime::from_hms(stamp.3, stamp.4, 0)?,
    ))
}

/// The real UTC instant a table entry spells, for a column written with a trailing `Z`.
fn utc(stamp: Stamp) -> Option<Instant> {
    local(stamp)?.at_offset(UtcOffset::UTC)
}

/// The nominal instant a wall clock projects onto, which is the timeline `ical-recur` walks.
///
/// Arithmetically the same number `utc` returns for the same fields, and a different fact:
/// one is an instant the world reached and the other is a position on a series' own clock. That
/// they coincide is exactly why a caller can hand `ical-recur` the wrong one and see nothing
/// wrong for half the year.
fn cadence(stamp: Stamp) -> Option<Instant> {
    nominal(local(stamp)?)
}

/// The offset `seconds` names.
fn offset(seconds: i32) -> Option<UtcOffset> {
    UtcOffset::from_seconds(seconds)
}

/// One reading as a table writes it.
fn reading(wanted: Expectation) -> Option<Reading> {
    Some(Reading::new(utc(wanted.0)?, offset(wanted.1)?, wanted.2))
}

/// One instant as the UTC wall clock a person reading a failure needs.
fn render(instant: Instant) -> String {
    match CivilDateTime::from_instant(instant, UtcOffset::UTC) {
        None => format!("<{}>", instant.unix_seconds()),
        Some(civil) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}Z",
            civil.date().year(),
            civil.date().month(),
            civil.date().day(),
            civil.time().hour(),
            civil.time().minute()
        ),
    }
}

/// The `VCALENDAR` a fixture's document holds.
fn calendar(document: &Document) -> Option<&Component> {
    document
        .components()
        .find(|component| component.is_named(b"VCALENDAR"))
}

/// The fixture's one master `VEVENT`.
fn event_of(document: &Document) -> Option<&Component> {
    calendar(document)?
        .components()
        .find(|component| component.is_named(b"VEVENT"))
}

/// The `DTSTART` of that `VEVENT`, carrying the zone its `TZID` parameter names.
fn dtstart_of(document: &Document) -> Option<DateTimeValue<'_>> {
    let property = event_of(document)?.properties_named(&ids::DTSTART).next()?;
    DateTimeValue::decode_property(property).ok()
}

/// The one `RRULE` that `VEVENT` carries, read through the grammar rather than rebuilt.
fn rrule_of(
    document: &Document,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<RecurrenceRule> {
    let property = event_of(document)?.properties_named(&ids::RRULE).next()?;
    parse_recur(property.value_text().as_bytes(), meter, sink).ok()
}

/// A fixture's document and the zone definitions its `VCALENDAR` carries.
fn zones_of(
    octets: &[u8],
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<(Document, VtimezoneSet)> {
    let document = Document::parse(octets, Limits::DEFAULT, sink).ok()?;
    let set = read_calendar_zones(calendar(&document)?, meter, sink);
    Some((document, set))
}

/// What one table case's zone said about the wall clock the case asked about.
fn resolved(case: ZoneCase) -> Option<ZoneAnswer> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(case.octets, &mut meter, &mut sink)?;
    let table = zones.table(case.tzid)?;
    table.resolve(case.tzid, local(case.asked)?)
}

/// Whether `resolution` is what the zone's published rules say.
///
/// A gap's own edges are compared only against the invariant `ical-tz` states for them, because
/// where a gap begins and ends is a question about instants that the wall clock either side of
/// the transition does not answer on its own.
fn agrees(resolution: LocalResolution, expected: Expected) -> Option<bool> {
    let agreed = match (resolution, expected) {
        (LocalResolution::Unique { reading: found }, Expected::Unique(wanted)) => {
            found == reading(wanted)?
        },
        (LocalResolution::Ambiguous { earlier, later }, Expected::Ambiguous(first, second)) => {
            earlier == reading(first)? && later == reading(second)?
        },
        (
            LocalResolution::Nonexistent {
                gap_start,
                gap_end,
                offset_before,
                offset_after,
                shifted,
            },
            Expected::Nonexistent {
                before,
                after,
                shifted: wanted,
            },
        ) => {
            gap_start < gap_end
                && offset_before.seconds() == before
                && offset_after.seconds() == after
                && shifted == utc(wanted)?
        },
        // `LocalResolution` is `#[non_exhaustive]`, and a state this table does not know about
        // is a disagreement rather than a pass.
        _ => false,
    };
    Some(agreed)
}

/// Every case in `table` that answered something other than what its zone's rules say.
///
/// Collected rather than asserted one at a time, so one run names every disagreement instead of
/// stopping at the first and hiding the shape of the defect behind it.
fn disagreements(table: &[ZoneCase]) -> Vec<String> {
    let mut found = Vec::new();
    for case in table {
        let name = case.name;
        let Some(answer) = resolved(*case) else {
            found.push(format!("{name}: the fixture defines no such zone"));
            continue;
        };
        if agrees(answer.resolution, case.expected) != Some(true) {
            let got = answer.resolution;
            let want = case.expected;
            found.push(format!(
                "{name}: answered {got:?}, and its rules say {want:?}"
            ));
        }
        if answer.source != ZoneProvenance::EmbeddedVtimezone {
            found.push(format!(
                "{name}: the answer did not name the file's own VTIMEZONE"
            ));
        }
        if answer.basis != AnswerBasis::Computed {
            let basis = answer.basis;
            found.push(format!("{name}: a rule-driven zone answered on {basis:?}"));
        }
    }
    found
}

/// One Berlin window, expanded and resolved.
#[derive(Debug)]
struct Expansion {
    /// The nominal cadence keys `ical-recur` emitted.
    keys: Vec<Instant>,
    /// What each key actually happens at, resolved one occurrence at a time.
    starts: Vec<Instant>,
    /// What each of those instants is on the zone's own wall clock.
    clocks: Vec<CivilDateTime>,
}

/// The cadence keys and effective starts one daily series emits over `window`, unresolved.
///
/// The occurrences are collected before anything is resolved because the search borrows the
/// meter and the sink for its whole life, which is the shape of the seam rather than an
/// inconvenience: `ical-recur` is finished with an occurrence before `ical-tz` is asked about it.
fn daily_occurrences(
    anchor: Instant,
    rule: &RecurrenceRule,
    window: Window,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<Vec<(Instant, Instant)>> {
    let input = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(rule),
        &[],
        &[],
        OverrideSet::empty(),
        meter,
    )
    .ok()?;
    let mut emitted = Vec::new();
    for step in input.search(window, meter, sink) {
        if let SearchStep::Occurrence(occurrence) = step {
            emitted.push((occurrence.key(), occurrence.start()));
        }
    }
    Some(emitted)
}

/// Resolve every occurrence through the zone, one at a time, which is the whole mechanism.
///
/// The wall clock resolved is the occurrence's *effective start*, which is its cadence key for
/// every occurrence no override moved and the moved value for the rest. `ZonedSeries::actual`
/// resolves the wall clock it is handed and has no way to ask which of the two it was given, so
/// which one a caller passes is the caller's discipline; this is the workspace's own example of
/// it.
fn resolved_starts(
    series: &ZonedSeries<'_, TransitionTable>,
    emitted: &[(Instant, Instant)],
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<Vec<Instant>> {
    emitted
        .iter()
        .map(|(_key, start)| series.actual(*start, meter, sink))
        .collect()
}

/// Expand the Berlin fixture's daily 09:00 series over one window and resolve every key.
fn berlin_expansion(from: Stamp, until: Stamp) -> Option<Expansion> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink)?;
    let table = zones.table(BERLIN)?;
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let anchor = series.anchor(dtstart_of(&document)?)?;
    let rule = rrule_of(&document, &mut meter, &mut sink)?;
    let window = Window::new(cadence(from)?, cadence(until)?)?;
    let emitted = daily_occurrences(anchor, &rule, window, &mut meter, &mut sink)?;
    let starts = resolved_starts(&series, &emitted, &mut meter, &mut sink)?;
    let keys = emitted.iter().map(|(key, _start)| *key).collect();
    let clocks = starts
        .iter()
        .map(|moment| {
            let running = table.offset_at(BERLIN, *moment)?;
            CivilDateTime::from_instant(*moment, running.offset)
        })
        .collect::<Option<Vec<CivilDateTime>>>()?;
    Some(Expansion {
        keys,
        starts,
        clocks,
    })
}

/// The Berlin daily series' cadence keys under one `UNTIL`, however that bound was arrived at.
fn keys_until(bound: Instant, kind: ValueKind, clock: UntilClock) -> Option<Vec<Instant>> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink)?;
    let table = zones.table(BERLIN)?;
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let anchor = series.anchor(dtstart_of(&document)?)?;
    let rule = RecurrenceRuleBuilder::new(Freq::Daily)
        .limit(RuleLimit::Until {
            at: bound,
            value_kind: kind,
            clock,
        })
        .build()
        .ok()?;
    let window = Window::new(cadence((2026, 3, 26, 0, 0))?, cadence((2026, 4, 4, 0, 0))?)?;
    Some(
        daily_occurrences(anchor, &rule, window, &mut meter, &mut sink)?
            .into_iter()
            .map(|(key, _start)| key)
            .collect(),
    )
}

/// The day the last cadence key in `keys` falls on, read back off the nominal timeline.
fn last_day(keys: &[Instant]) -> Option<CivilDate> {
    Some(wall_clock(*keys.last()?)?.date())
}

/// How many seconds actually elapse between two wall clocks in the series' own zone.
fn elapsed_across(
    series: &ZonedSeries<'_, TransitionTable>,
    from: Stamp,
    until: Stamp,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<i64> {
    let opened = series.actual(cadence(from)?, meter, sink)?;
    let closed = series.actual(cadence(until)?, meter, sink)?;
    opened.checked_seconds_until(closed)
}

/// RFC 5545 section 3.3.5. The hour that repeats and the hour that does not exist are values,
/// under three zones that move their clocks by different amounts in different months.
#[test]
fn the_two_awkward_hours_are_values_under_three_real_zones() {
    let found = disagreements(AWKWARD_HOURS);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// RFC 5545 section 3.6.5. A file carrying two eras of one zone answers each year under the
/// rule that was in force in it, and never under whichever rule the table happens to hold last.
#[test]
fn a_zone_that_changed_its_rules_answers_each_year_under_the_rule_in_force() {
    let found = disagreements(RULES_CHANGED_IN_2007);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// RFC 5545 section 3.3.5, against the convention three units had to share and none owned.
///
/// A gap has no width on the UTC timeline — the clock moves at one instant — so `gap_start`
/// and `gap_end` are a convention rather than a measurement, and `GapPolicy::ClampToTransition`
/// is the only thing that reads one of them. The table that produces the pair and the driver
/// that collapses it were written separately, each against its own fixture, so the two could
/// have agreed with themselves and not with each other and every test above would still pass.
/// This asks the real parsed table for Europe/Berlin's March gap and pins the numbers: the
/// clock moves at 01:00Z, `gap_end` is that instant and reads as 03:00 under the new offset,
/// `gap_start` is the second before it, and the three policies give the three answers a caller
/// stating one has a right to expect.
#[test]
fn where_a_gap_begins_and_ends_is_one_convention_across_the_table_and_the_driver() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let moves_at = utc((2026, 3, 29, 1, 0)).expect("Berlin springs forward at 01:00 UTC");
    let asked = local((2026, 3, 29, 2, 30)).expect("a wall clock the zone sprang over");

    let answer = table
        .resolve(BERLIN, asked)
        .expect("the identifier is the table's");
    let LocalResolution::Nonexistent {
        gap_start, gap_end, ..
    } = answer.resolution
    else {
        panic!("02:30 on the last Sunday of March is not an instant in Berlin");
    };
    assert_eq!(gap_end, moves_at, "the gap closes when the clock moves");
    assert_eq!(
        gap_start.checked_add_seconds(1),
        Some(gap_end),
        "the last instant before the gap opened is the second before it closed"
    );
    assert_eq!(
        CivilDateTime::from_instant(gap_end, offset(7200).expect("CEST")).map(CivilDateTime::time),
        CivilTime::from_hms(3, 0, 0),
        "read under the offset that took force, the gap closes at 03:00"
    );

    let key = cadence((2026, 3, 29, 2, 30)).expect("a cadence key in the gap");
    for (gaps, expected) in [
        (GapPolicy::Skip, None),
        (GapPolicy::ShiftForward, utc((2026, 3, 29, 1, 30))),
        (GapPolicy::ClampToTransition, Some(moves_at)),
    ] {
        let policy = ResolutionPolicy::DEFAULT.with_gaps(gaps);
        let series = ZonedSeries::new(table, BERLIN, policy);
        assert_eq!(
            series.actual(key, &mut meter, &mut sink),
            expected,
            "{gaps:?} against the table's own gap"
        );
    }
}

/// RFC 5545 section 3.8.5.3 and section 3.6.5, together. This is the seam.
///
/// A daily 09:00 series in Europe/Berlin, expanded through `ical-recur` and resolved one
/// occurrence at a time through `ical-tz`. The wall clock must not move across either 2026
/// transition and the UTC instant must, which is the same claim stated twice: the timeline the
/// cadence is generated on is the series' own clock, and the offsets are applied afterwards
/// because a transition is only visible one occurrence at a time.
#[test]
fn a_zoned_daily_series_keeps_its_wall_clock_while_its_utc_instants_move() {
    let nine = CivilTime::from_hms(9, 0, 0).expect("nine o'clock");
    for (from, until, expected) in SEAM_WINDOWS {
        let run = berlin_expansion(*from, *until).expect("the fixture expands");
        let wanted: Vec<String> = expected
            .iter()
            .copied()
            .map(utc)
            .collect::<Option<Vec<Instant>>>()
            .expect("the table holds real instants")
            .into_iter()
            .map(render)
            .collect();
        let got: Vec<String> = run.starts.iter().copied().map(render).collect();
        assert_eq!(got, wanted, "the instants Europe/Berlin's rules give");
        for (clock, key) in run.clocks.iter().zip(&run.keys) {
            assert_eq!(
                clock.time(),
                nine,
                "the wall clock moved across a transition"
            );
            assert_eq!(
                clock.date(),
                wall_clock(*key).expect("a key reads back").date(),
                "the occurrence landed on another day"
            );
        }
    }
}

/// RFC 5545 section 3.8.5.3. The failure this seam exists to prevent, asserted as itself.
///
/// A caller that anchors at the real UTC instant and never re-resolves gets a series that is
/// exactly one hour out from the moment Europe/Berlin springs forward. If the seam is ever
/// bypassed — if the cadence is walked on the UTC timeline instead of the nominal one — the
/// resolved answer becomes the naive one and this test fails.
#[test]
fn the_naive_reading_of_a_zoned_series_is_an_hour_out_after_the_transition() {
    let run = berlin_expansion((2026, 3, 26, 0, 0), (2026, 4, 2, 0, 0)).expect("the fixture");
    let first = *run.starts.first().expect("the window holds occurrences");
    for (step, moment) in run.starts.iter().enumerate() {
        let walked = i64::try_from(step).expect("a week of steps fits");
        let naive = first
            .checked_add_seconds(walked.checked_mul(86_400).expect("a week of seconds"))
            .expect("on the timeline");
        let drift = moment
            .checked_seconds_until(naive)
            .expect("a representable difference");
        let expected = if step < 3 { 0 } else { 3600 };
        assert_eq!(drift, expected, "step {step} of the naive reading");
    }
}

/// RFC 5545 section 3.3.10. A `Z`-terminated `UNTIL` names a real UTC instant, which is not on
/// the nominal timeline the cadence keys live on.
///
/// `UNTIL=20260330T080000Z` is 10:00 in Berlin, because the zone had already sprung forward, so
/// the projected bound admits March 30th's 09:00 instance. The same value handed over
/// unprojected is compared as 08:00 against a key at 09:00 and cuts the series a day early.
#[test]
fn a_z_terminated_until_ends_a_zoned_series_a_day_later_than_an_unprojected_one() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let start = dtstart_of(&document).expect("a zoned DTSTART");

    let written = local((2026, 3, 30, 8, 0)).expect("the UNTIL the file wrote");
    let projected = series
        .project_until(DateTimeValue::Utc(written), start, &mut meter, &mut sink)
        .expect("the bound projects");
    let unprojected = written
        .at_offset(UtcOffset::UTC)
        .expect("the instant Z names");
    assert_ne!(
        projected, unprojected,
        "a real UTC instant is not already on the nominal timeline"
    );

    let kept = keys_until(projected, ValueKind::DateTime, UntilClock::Utc).expect("expansion");
    let cut = keys_until(unprojected, ValueKind::DateTime, UntilClock::Utc).expect("expansion");
    assert_eq!(last_day(&kept), CivilDate::from_ymd(2026, 3, 30));
    assert_eq!(last_day(&cut), CivilDate::from_ymd(2026, 3, 29));
}

/// RFC 5545 section 3.3.10, which requires `UNTIL` to be UTC and which Google violates.
///
/// A floating `UNTIL` is already nominal and needs no conversion at all. What it needs is to be
/// reported, because reading it in `DTSTART`'s own zone recovers the producer's intent and is
/// nonetheless not what the specification says the file means.
#[test]
fn a_floating_until_against_a_zoned_dtstart_is_read_in_that_zone_and_reported() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let start = dtstart_of(&document).expect("a zoned DTSTART");

    let floating = local((2026, 3, 30, 8, 0)).expect("the UNTIL the file wrote");
    let mut reported: Vec<Diagnostic> = Vec::new();
    let projected = series
        .project_until(
            DateTimeValue::Local(floating),
            start,
            &mut meter,
            &mut reported,
        )
        .expect("the bound is readable");
    assert_eq!(
        projected,
        nominal(floating).expect("a floating value is already nominal"),
        "a wall clock read at UTC is the projection itself"
    );
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::RecurrenceUntilNotUtc),
        "an UNTIL that is not UTC is a violation the caller has to be able to see"
    );
}

/// RFC 5545 section 3.3.10. A `DATE` `UNTIL` against a date-time `DTSTART` names a day, and
/// where in that day it sits is a policy the specification does not settle.
///
/// Midnight is what libical, dateutil and the Google engine do, and it drops the named day's own
/// instances. End of day is what the person who typed the date meant. Both are here because a
/// library that picked one silently would move somebody's last meeting.
#[test]
fn a_date_until_admits_the_named_day_only_under_the_end_of_day_reading() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let start = dtstart_of(&document).expect("a zoned DTSTART");
    let named = CivilDate::from_ymd(2026, 3, 30).expect("a real date");
    let day_before = CivilDate::from_ymd(2026, 3, 29).expect("a real date");

    for (placement, holds) in [
        (UntilReading::Midnight, false),
        (UntilReading::EndOfDay, true),
    ] {
        let policy = ResolutionPolicy::DEFAULT.with_until(placement);
        let series = ZonedSeries::new(table, BERLIN, policy);
        let bound = series
            .project_until(DateTimeValue::Date(named), start, &mut meter, &mut sink)
            .expect("the bound projects");
        let keys = keys_until(bound, ValueKind::Date, UntilClock::Floating).expect("expansion");
        let wanted = if holds { named } else { day_before };
        assert_eq!(
            last_day(&keys),
            Some(wanted),
            "the named day's own instances under {placement:?}"
        );
    }
}

/// RFC 5545 section 3.8.5.1. A `DATE` `EXDATE` read as an instant removes nothing at all.
///
/// This is the silent failure the code exists for: the exception resolves to midnight, the
/// series has no instance there, the meeting the user cancelled reappears, and without the
/// diagnostic nothing anywhere says a word.
#[test]
fn a_date_exdate_read_as_an_instant_removes_nothing_and_still_says_so() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let excluded = [DateTimeValue::Date(
        CivilDate::from_ymd(2026, 10, 25).expect("a real date"),
    )];

    let mut reported: Vec<Diagnostic> = Vec::new();
    let resolved_dates = ResolvedExclusions::read(
        &series,
        ValueType::DateTime,
        &excluded,
        &mut meter,
        &mut reported,
    );
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::ExdateValueTypeMismatch),
        "a silent no-op is the one outcome that is indefensible"
    );
    assert!(
        resolved_dates.spans().is_empty(),
        "an instantaneous reading names no span"
    );

    let run = berlin_expansion((2026, 10, 22, 0, 0), (2026, 10, 29, 0, 0)).expect("the fixture");
    let kept = run
        .keys
        .iter()
        .filter(|key| !resolved_dates.excludes(**key))
        .count();
    assert_eq!(
        kept,
        run.keys.len(),
        "the exception the producer wrote removed nothing, which is the point"
    );
    for key in &run.keys {
        assert!(
            !resolved_dates.instants().contains(key),
            "midnight is not an instant a nine o'clock series ever has"
        );
    }
}

/// RFC 5545 section 3.8.5.1, read the way several clients implement it.
///
/// The date names the day in the series' own zone, and a day is 23, 24 or 25 hours long
/// depending on whether the zone moved inside it — which is why this reading needs a zone and
/// why it cannot live in `ical-recur`. Both days measured here are the awkward lengths, proved
/// through the zone itself rather than assumed.
#[test]
fn a_whole_day_exdate_removes_a_day_the_zone_made_twenty_three_or_twenty_five_hours_long() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let policy = ResolutionPolicy::DEFAULT.with_exclusions(ExclusionReading::WholeDay);
    let series = ZonedSeries::new(table, BERLIN, policy);
    let excluded = [
        DateTimeValue::Date(CivilDate::from_ymd(2026, 3, 29).expect("a real date")),
        DateTimeValue::Date(CivilDate::from_ymd(2026, 10, 25).expect("a real date")),
    ];

    let mut reported: Vec<Diagnostic> = Vec::new();
    let whole_days = ResolvedExclusions::read(
        &series,
        ValueType::DateTime,
        &excluded,
        &mut meter,
        &mut reported,
    );
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::ExdateValueTypeMismatch),
        "the mismatch travels under either reading"
    );
    for removed in [(2026, 3, 29, 9, 0), (2026, 10, 25, 9, 0)] {
        let key = cadence(removed).expect("a real wall clock");
        assert!(whole_days.excludes(key), "{removed:?} was not removed");
    }
    for survives in [
        (2026, 3, 28, 9, 0),
        (2026, 3, 30, 9, 0),
        (2026, 10, 24, 9, 0),
        (2026, 10, 26, 9, 0),
    ] {
        let key = cadence(survives).expect("a real wall clock");
        assert!(!whole_days.excludes(key), "{survives:?} was removed too");
    }

    let sprang = elapsed_across(
        &series,
        (2026, 3, 29, 0, 0),
        (2026, 3, 30, 0, 0),
        &mut meter,
        &mut sink,
    );
    assert_eq!(
        sprang,
        Some(82_800),
        "March 29th 2026 is 23 hours long in Berlin"
    );
    let fell = elapsed_across(
        &series,
        (2026, 10, 25, 0, 0),
        (2026, 10, 26, 0, 0),
        &mut meter,
        &mut sink,
    );
    assert_eq!(
        fell,
        Some(90_000),
        "October 25th 2026 is 25 hours long in Berlin"
    );
}

/// RFC 5545 section 3.8.4.4 and section 3.2.13. An override's move is two numbers once a zone
/// is involved, and a scalar shift is only ever one of them.
///
/// Moving a Berlin meeting from March 28th 09:00 to March 30th 09:00 is two days on the
/// organizer's clock and two days less an hour on the timeline, because the zone sprang forward
/// in between. Every implementation that carries one number looks correct until exactly here.
#[test]
fn an_override_across_a_transition_moves_two_different_numbers_of_seconds() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");

    let from = utc((2026, 3, 28, 8, 0)).expect("09:00 CET");
    let onto = utc((2026, 3, 30, 7, 0)).expect("09:00 CEST");
    let moved = WallClockShift::measure(table, BERLIN, from, onto).expect("both are in the zone");
    assert_eq!(
        moved.wall_clock_seconds(),
        172_800,
        "two days on the organizer's clock"
    );
    assert_eq!(
        moved.elapsed_seconds(),
        169_200,
        "one hour less on the timeline, because Berlin sprang forward in between"
    );
    assert!(moved.crossed_a_transition());

    let summer = utc((2026, 8, 10, 7, 0)).expect("09:00 CEST");
    let later = utc((2026, 8, 12, 7, 0)).expect("09:00 CEST");
    let steady = WallClockShift::measure(table, BERLIN, summer, later).expect("in the zone");
    assert_eq!(steady.wall_clock_seconds(), steady.elapsed_seconds());
    assert!(!steady.crossed_a_transition());

    assert_eq!(extra_widening(&[]), 0, "no shift needs no widening");
    assert_eq!(
        extra_widening(&[steady]),
        0,
        "a move inside one observance needs none either"
    );
    assert!(
        extra_widening(&[moved]) >= 3600,
        "the widening has to cover the hour the two readings differ by"
    );
}

/// RFC 5545 section 3.2.13. The widening keeps a shifted occurrence inside the window it has to
/// be generated in.
///
/// The asked window is one day and the anchor's cadence key is two days before it, so the
/// occurrence exists only because generation widened backwards. A widening that measured the
/// move on the wrong clock would be short by the hour the zone moved.
#[test]
fn the_extra_widening_keeps_a_shifted_occurrence_inside_the_generated_window() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let anchor = series
        .anchor(dtstart_of(&document).expect("a zoned DTSTART"))
        .expect("the anchor projects");
    let rule = rrule_of(&document, &mut meter, &mut sink).expect("FREQ=DAILY");

    let addressed = cadence((2026, 3, 28, 9, 0)).expect("a real wall clock");
    let landed = cadence((2026, 3, 30, 9, 0)).expect("a real wall clock");
    let entries = [Override::new(
        addressed,
        OverrideRange::ThisAndFuture,
        Some(landed),
        PropertyDiff::empty(),
    )];
    let overrides = OverrideSet::new(&entries, &mut meter).expect("one ascending override");

    let from = utc((2026, 3, 28, 8, 0)).expect("09:00 CET");
    let onto = utc((2026, 3, 30, 7, 0)).expect("09:00 CEST");
    let moved = WallClockShift::measure(table, BERLIN, from, onto).expect("both are in the zone");
    let asked = Window::new(
        cadence((2026, 3, 30, 0, 0)).expect("a real wall clock"),
        cadence((2026, 3, 31, 0, 0)).expect("a real wall clock"),
    )
    .expect("a day is not empty");
    let generated = generation_window(asked, overrides).expect("a representable window");
    let extra = extra_widening(&[moved]);
    let widened = generated
        .widened(extra, extra)
        .expect("still representable");
    assert!(
        widened.contains(addressed),
        "the key the override addresses has to be generated for the move to happen"
    );

    let input = RecurrenceInput::new(
        anchor,
        ValueKind::DateTime,
        Some(&rule),
        &[],
        &[],
        overrides,
        &mut meter,
    )
    .expect("the input assembles");
    let mut found: Vec<(Instant, Instant)> = Vec::new();
    for step in input.search(widened, &mut meter, &mut sink) {
        // A hand-written discard of the terminal step, which `ical-recur`'s own documentation
        // calls visible-but-possible: the budget is not what this case is about.
        let Some(occurrence) = step.occurrence() else {
            continue;
        };
        if occurrence.starts_within(asked) {
            found.push((occurrence.key(), occurrence.start()));
        }
    }
    assert!(
        found.contains(&(addressed, landed)),
        "the occurrence the anchor moved into the day did not survive the widening"
    );
}

/// RFC 5545 section 3.8.4.4. An override whose `RECURRENCE-ID` names no generated instance is
/// inert, and before this milestone was reported by nothing at all.
///
/// Clients produce these routinely: an instance is edited, then the rule beneath it is
/// rewritten, and the file keeps a meeting the expanded series does not have.
#[test]
fn a_recurrence_id_naming_no_generated_instance_is_reported() {
    let run = berlin_expansion((2026, 3, 26, 0, 0), (2026, 4, 2, 0, 0)).expect("the fixture");
    let real = cadence((2026, 3, 28, 9, 0)).expect("a key the series generates");
    let orphan = cadence((2026, 3, 28, 10, 30)).expect("a key it does not");
    let addressed = [real, orphan];

    let mut scan = OrphanScan::new(&addressed);
    for key in &run.keys {
        scan.observe(*key);
    }
    assert_eq!(
        scan.unmatched(),
        1,
        "one of the two identifiers names a generated key and one does not"
    );

    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    assert_eq!(scan.finish(&mut meter, &mut reported), 1);
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::OverrideMatchesNoInstance),
        "every other silent drop in these crates has a code and so does this one"
    );
}

/// RFC 5545 section 3.6.5. A `VTIMEZONE` whose transitions are `RDATE` lines simply runs out.
///
/// This file knows Europe/Berlin through October 2029 and is asked about June 2035. Continuing
/// the final observance puts that morning on CET, and real Europe/Berlin runs CEST in June — so
/// the answer is defensible and wrong, and the basis is the only thing that says so. Clamping
/// quietly, or extrapolating a rule nobody wrote, are the two silent lies this replaces.
#[test]
fn a_date_driven_zone_asked_six_years_past_its_last_transition_says_so() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) =
        zones_of(BERLIN_RDATE_THROUGH_2029, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let ran_out = CivilDate::from_ymd(2029, 10, 28).expect("the last transition the file writes");
    assert_eq!(
        table.coverage_end(),
        Some(ran_out),
        "an explicit table covers its last date and nothing after it"
    );

    let asked = local((2035, 6, 15, 9, 0)).expect("a real wall clock");
    let answer = table
        .resolve(BERLIN, asked)
        .expect("the identifier is one this file defines");
    assert_eq!(answer.basis, AnswerBasis::BeyondKnownTransitions(ran_out));
    assert_eq!(answer.basis.nearest_known(), Some(ran_out));
    assert_eq!(answer.source, ZoneProvenance::EmbeddedVtimezone);
    assert_eq!(
        answer.basis.diagnostic_code(),
        Some(DiagnosticCode::TimeZoneCoverageExhausted)
    );
    let continued = Reading::new(
        utc((2035, 6, 15, 8, 0)).expect("09:00 CET"),
        offset(3600).expect("CET"),
        false,
    );
    assert_eq!(
        answer.resolution,
        LocalResolution::Unique { reading: continued },
        "the final observance is continued rather than a rule being guessed at"
    );

    let series = ZonedSeries::new(table, BERLIN, ResolutionPolicy::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let happened = series
        .actual(nominal(asked).expect("nominal"), &mut meter, &mut reported)
        .expect("an answer exists and is marked");
    assert_eq!(happened, utc((2035, 6, 15, 8, 0)).expect("09:00 CET"));
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::TimeZoneCoverageExhausted),
        "an answer past the end of the data has to be distinguishable from a real one"
    );
}

/// RFC 5545 section 3.2.19. A `TZID` is not an IANA identifier and this workspace may not
/// pretend otherwise.
///
/// Exchange writes `W. Europe Standard Time`, Lightning writes
/// `/mozilla.org/20050126_1/Europe/Berlin`, and Outlook writes `Customized Time Zone`. Each is
/// read, classified by shape, and looked up by exact bytes; none is translated, and in
/// particular the prefixed one is not unwrapped into the IANA name sitting inside it.
#[test]
fn the_identifiers_real_files_carry_are_read_classified_and_never_rewritten() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(VENDOR_TZIDS, &mut meter, &mut sink).expect("fixture");
    assert_eq!(zones.len(), 3);
    for &(text, shape) in VENDOR_IDENTIFIERS {
        let table = zones
            .table(text)
            .expect("the fixture defines this identifier");
        assert_eq!(table.tzid().as_str(), text, "the identifier was rewritten");
        assert_eq!(table.tzid().form(), shape, "{text}");
    }
    assert!(
        zones.table(BERLIN).is_none(),
        "a vendor prefix is not silently unwrapped into the IANA name inside it"
    );
    let prefixed = Tzid::new("/mozilla.org/20050126_1/Europe/Berlin");
    assert_eq!(
        prefixed.strip_global_prefix().map(Tzid::as_str),
        Some("mozilla.org/20050126_1/Europe/Berlin"),
        "one solidus comes off, which is the whole of the rewriting section 3.2.19 licenses"
    );
}

/// RFC 5545 section 3.2.19, the other half. An identifier nobody recognizes is reported.
///
/// Not defaulted to UTC, not aliased inside somebody's implementation, and not settled by
/// preferring whichever source happened to answer — because none did.
#[test]
fn an_identifier_no_source_knows_is_reported_rather_than_defaulted() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let (_document, zones) = zones_of(BERLIN_DAILY_NINE, &mut meter, &mut sink).expect("fixture");
    let table = zones
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    let elsewhere = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let combined = CombinedZoneSource::new(table, &elsewhere);

    let moment = utc((2026, 8, 10, 12, 0)).expect("a real instant");
    let outcome = combined.offset_at("Mars/Olympus_Mons", moment);
    assert!(
        matches!(outcome, PolicyOutcome::Neither),
        "neither source knows this identifier, and got {outcome:?}"
    );
    assert_eq!(
        outcome.embedded_first(),
        None,
        "an identifier nobody knows yields no answer at all"
    );

    let mut reported: Vec<Diagnostic> = Vec::new();
    combined.report(outcome, moment, &mut meter, &mut reported);
    assert!(
        reported
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::UnknownTimeZone),
        "the hole stays visible"
    );
    assert_eq!(
        Tzid::new("Mars/Olympus_Mons").form(),
        TzidForm::IanaLike,
        "the shape of a database name is not membership of the database"
    );
}

/// RFC 5545 section 3.6.5 under `docs/adr/0010`. A bound nobody charges is decoration.
///
/// A million `RDATE` transitions is a file somebody can write. A table cut to hold the caller's
/// policy says it was cut, and its coverage ends earlier rather than the table acquiring a hole
/// — so a question the dropped transitions would have answered becomes an extrapolation the
/// caller can see rather than a confident wrong answer.
#[test]
fn a_zone_with_more_transitions_than_the_policy_admits_is_cut_and_says_where() {
    let mut whole_meter = Meter::new(Limits::DEFAULT);
    let mut whole_sink: Vec<Diagnostic> = Vec::new();
    let (_kept, whole) =
        zones_of(BERLIN_RDATE_THROUGH_2029, &mut whole_meter, &mut whole_sink).expect("fixture");
    let full = whole
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");
    assert!(!full.is_truncated());

    let tight = Limits::DEFAULT.with_max_vtimezone_observances(3);
    let mut narrow_meter = Meter::new(tight);
    let mut narrow_sink: Vec<Diagnostic> = Vec::new();
    let (_dropped, narrow) = zones_of(
        BERLIN_RDATE_THROUGH_2029,
        &mut narrow_meter,
        &mut narrow_sink,
    )
    .expect("fixture");
    let short = narrow
        .table(BERLIN)
        .expect("the calendar defines Europe/Berlin");

    assert!(
        short.is_truncated(),
        "a table cut to hold the bound says so"
    );
    assert!(short.observances().len() <= 3);
    assert!(
        narrow_sink
            .iter()
            .any(|entry| entry.code() == DiagnosticCode::VtimezoneObservancesTruncated),
        "the truncation is reported and not merely recorded on the table"
    );
    let (Some(near), Some(far)) = (short.coverage_end(), full.coverage_end()) else {
        panic!("both tables are date driven, so both have a coverage end");
    };
    assert!(
        near < far,
        "coverage ends earlier rather than the table acquiring a hole"
    );
}
