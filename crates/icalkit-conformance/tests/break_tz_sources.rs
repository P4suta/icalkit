// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where a zone answer came from, and what it is worth — `docs/adr/0003` attacked at its center.
//!
//! `break_zones.rs` asks whether `ical-tz` reads real transition rules correctly. This file asks
//! the other question the ADR turns on: when an answer reaches a caller, does the caller learn
//! **which source produced it** and **how much that source actually knew**? Every case below is
//! about provenance rather than about arithmetic, and each one is addressed to a claim somebody
//! wrote down:
//!
//! - **`docs/adr/0003`, "Every result says which source produced it."** — an embedded
//!   `VTIMEZONE` and a caller-supplied database that disagree, with both answers reachable and
//!   each naming itself.
//! - **`docs/adr/0003` Mechanism, "A source that does not recognize an identifier returns
//!   `None`. That is what the `Option` is for."** — restated by `ical-tz`'s own `answer` module
//!   as "`None` for exactly one condition: this source does not recognize this `TZID`". A
//!   definition that exists and carries no observance is not that condition.
//! - **`docs/adr/0003` amendment 5 and `AnswerBasis`** — a table whose data runs out answers by
//!   continuing its final observance and says so. The claim is symmetric in nothing: it is
//!   stated for the future end of a table only, and a table has two ends.
//! - **RFC 5545 section 3.6.5** — a `VTIMEZONE` with neither `STANDARD` nor `DAYLIGHT`, and one
//!   identifier declared twice in one file.
//! - **RFC 5545 section 3.2.19** — identifiers that are not IANA names: a Windows zone name, a
//!   Lightning prefix, a name a user typed, and one carrying a colon.
//!
//! # Writing a caller-supplied source by hand
//!
//! [`CallerZoneinfo`] below is what `docs/adr/0003` asks every caller to supply, written from a
//! published transition list rather than from anything this workspace computed. Two things about
//! writing it are findings in their own right and are recorded here rather than in a comment
//! nobody reads. First, [`icalkit_conformance::internal::tz::VtimezoneSet`] is not a [`ZoneSource`], so wiring "the file's
//! own definitions" as the embedded half of a pair needs the [`FileZones`] adapter below —
//! twelve lines every caller writes identically, and the place each of them independently
//! decides what an empty definition means. Second, building a
//! [`LocalResolution::Nonexistent`] by hand means re-deriving `gap_start`, `gap_end` and
//! `shifted` and the invariants tying them together, with no constructor or helper in the crate
//! to do it: the type states the invariants in prose and checks none of them.

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Component, Diagnostic, DiagnosticCode, Document,
    IgnoreDiagnostics, Instant, Limits, Meter, UtcOffset,
};
use icalkit_conformance::internal::tz::{
    AnswerBasis, CombinedZoneSource, LocalResolution, OffsetAnswer, PolicyOutcome, Reading, Tzid,
    TzidForm, VtimezoneSet, ZoneAnswer, ZoneProvenance, ZoneSource, read_calendar_zones,
};

/// A `VTIMEZONE` for `Europe/Berlin` that carries neither a `STANDARD` nor a `DAYLIGHT`.
const BERLIN_WITHOUT_OBSERVANCE: &[u8] =
    include_bytes!("fixtures/break_tz_sources/berlin_definition_without_observance.ics");

/// The same event with no `VTIMEZONE` in the file at all.
const BERLIN_WITHOUT_DEFINITION: &[u8] =
    include_bytes!("fixtures/break_tz_sources/berlin_no_definition_at_all.ics");

/// `America/New_York` written as `RDATE` lines running from 2027 through 2029 and no further.
const NEW_YORK_DATED: &[u8] =
    include_bytes!("fixtures/break_tz_sources/new_york_rdate_2027_through_2029.ics");

/// `Europe/Berlin` declared twice: an empty placeholder first, then the real definition.
const BERLIN_PLACEHOLDER_FIRST: &[u8] =
    include_bytes!("fixtures/break_tz_sources/berlin_placeholder_before_real_definition.ics");

/// `America/New_York` declared twice, with the rules of two different eras.
const NEW_YORK_TWICE: &[u8] =
    include_bytes!("fixtures/break_tz_sources/new_york_two_definitions.ics");

/// `America/New_York` as a client wrote it before the 2007 rule change.
const NEW_YORK_OLD: &[u8] =
    include_bytes!("fixtures/break_tz_sources/new_york_rules_before_2007.ics");

/// `America/New_York` as a caller's own database publishes it today, in the same syntax.
const NEW_YORK_TODAY: &[u8] =
    include_bytes!("fixtures/break_tz_sources/new_york_rules_after_2007.ics");

/// Identifiers that are not IANA names, one of them carrying a colon.
const PUNCTUATED_IDENTIFIERS: &[u8] =
    include_bytes!("fixtures/break_tz_sources/identifiers_with_punctuation.ics");

/// One identifier declared twice, differing only in a `DQUOTE` pair the file wrote.
const BERLIN_QUOTED_AND_NOT: &[u8] =
    include_bytes!("fixtures/break_tz_sources/berlin_quoted_and_unquoted_identifier.ics");

/// An identifier written with a `TEXT` escape, which the two sides of the file spell alike as
/// octets and differently as values.
const IDENTIFIER_WITH_ESCAPE: &[u8] =
    include_bytes!("fixtures/break_tz_sources/identifier_with_an_escaped_newline.ics");

/// Two zones, both defined by the file and both referenced by one event.
const TWO_ZONES: &[u8] = include_bytes!("fixtures/break_tz_sources/two_zones_both_referenced.ics");

/// Three `TZID` parameters that no `VTIMEZONE` in the file defines.
const THREE_UNDEFINED: &[u8] =
    include_bytes!("fixtures/break_tz_sources/three_identifiers_nothing_defines.ics");

/// The identifier the Berlin cases name.
const BERLIN: &str = "Europe/Berlin";

/// The identifier the New York cases name.
const NEW_YORK: &str = "America/New_York";

// ---------------------------------------------------------------------------------------------
// A caller-supplied source, written by hand, as `docs/adr/0003` requires every caller to do.
// ---------------------------------------------------------------------------------------------

/// One transition, as a published zone database records one.
#[derive(Clone, Copy, Debug)]
struct Shift {
    /// The instant the clocks moved.
    moment: Instant,
    /// The offset they left.
    vacated: UtcOffset,
    /// The offset they took.
    adopted: UtcOffset,
    /// Whether the offset they took is the zone's daylight one.
    daylight: bool,
}

/// A caller's own zone database, holding one zone as a list of transitions.
///
/// Every answer it gives names [`ZoneProvenance::CallerDatabase`], because that is what it is.
#[derive(Clone, Debug)]
struct CallerZoneinfo {
    /// The identifier it answers to, compared by exact bytes.
    tzid: &'static str,
    /// The offset in force before the first transition.
    standing: UtcOffset,
    /// The transitions, ascending.
    shifts: Vec<Shift>,
}

impl CallerZoneinfo {
    /// The offset in force at `moment`, and whether it is the zone's daylight one.
    fn running_at(&self, moment: Instant) -> (UtcOffset, bool) {
        let mut current = (self.standing, false);
        for moved in &self.shifts {
            if moved.moment <= moment {
                current = (moved.adopted, moved.daylight);
            }
        }
        current
    }

    /// Every reading of `local` this database admits, earliest first.
    ///
    /// A candidate offset names a real instant only where the offset in force at that instant is
    /// the candidate itself. None is a gap, one is an ordinary day, two is a fold.
    fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
        let mut candidates = vec![(self.standing, false)];
        candidates.extend(
            self.shifts
                .iter()
                .map(|moved| (moved.adopted, moved.daylight)),
        );
        let mut found: Vec<Reading> = Vec::new();
        for (offset, daylight) in candidates {
            let Some(moment) = local.at_offset(offset) else {
                continue;
            };
            if self.running_at(moment) != (offset, daylight) {
                continue;
            }
            if found.iter().any(|reading| reading.instant == moment) {
                continue;
            }
            found.push(Reading::new(moment, offset, daylight));
        }
        found.sort_unstable();
        found
    }

    /// The gap `local` fell into, when a transition sprang over it.
    fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
        let moved = *self
            .shifts
            .iter()
            .find(|candidate| sprang_over(**candidate, local))?;
        Some(LocalResolution::Nonexistent {
            gap_start: moved.moment.checked_add_seconds(-1)?,
            gap_end: moved.moment,
            offset_before: moved.vacated,
            offset_after: moved.adopted,
            shifted: local.at_offset(moved.vacated)?,
        })
    }
}

/// Whether `moved` sprang over `local`, that is, whether the wall clock jumped past it.
fn sprang_over(moved: Shift, local: CivilDateTime) -> bool {
    let opened = CivilDateTime::from_instant(moved.moment, moved.vacated);
    let closed = CivilDateTime::from_instant(moved.moment, moved.adopted);
    matches!((opened, closed), (Some(from), Some(to)) if from <= local && local < to)
}

impl ZoneSource for CallerZoneinfo {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let readings = self.readings(local);
        let resolution = match readings.as_slice() {
            [] => self.gap(local)?,
            [only] => LocalResolution::Unique { reading: *only },
            [earlier, later, ..] => LocalResolution::Ambiguous {
                earlier: *earlier,
                later: *later,
            },
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        if tzid != self.tzid {
            return None;
        }
        let (offset, daylight) = self.running_at(instant);
        Some(OffsetAnswer::new(
            offset,
            daylight,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }
}

/// A caller-supplied source that recognizes no identifier at all.
///
/// The wiring of a caller that has no database, which `docs/adr/0003` names as the ordinary
/// `no_std` case. It is here because it is the only way to ask what the pair does when exactly
/// one half can answer.
#[derive(Clone, Copy, Debug)]
struct NoZonesAtAll;

impl ZoneSource for NoZonesAtAll {
    fn resolve(&self, _tzid: &str, _local: CivilDateTime) -> Option<ZoneAnswer> {
        None
    }

    fn offset_at(&self, _tzid: &str, _instant: Instant) -> Option<OffsetAnswer> {
        None
    }
}

/// The file's own definitions, wired in as one source.
///
/// `ical-tz` ships no adapter from a [`VtimezoneSet`] to a [`ZoneSource`], so this is what every
/// caller writes to put "what the calendar carries" on one side of a [`CombinedZoneSource`].
#[derive(Clone, Copy, Debug)]
struct FileZones<'a> {
    /// The definitions the calendar carried.
    zones: &'a VtimezoneSet,
}

impl ZoneSource for FileZones<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        self.zones.table(tzid)?.resolve(tzid, local)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        self.zones.table(tzid)?.offset_at(tzid, instant)
    }
}

/// Europe/Berlin's 2026 transitions, from the European Union's published rule: the last Sunday
/// in March and the last Sunday in October, both at 01:00 UTC.
fn berlin_2026() -> Option<CallerZoneinfo> {
    let winter = UtcOffset::from_seconds(3_600)?;
    let summer = UtcOffset::from_seconds(7_200)?;
    Some(CallerZoneinfo {
        tzid: BERLIN,
        standing: winter,
        shifts: vec![
            Shift {
                moment: stamp(2026, 3, 29, 1, 0)?.at_offset(UtcOffset::UTC)?,
                vacated: winter,
                adopted: summer,
                daylight: true,
            },
            Shift {
                moment: stamp(2026, 10, 25, 1, 0)?.at_offset(UtcOffset::UTC)?,
                vacated: summer,
                adopted: winter,
                daylight: false,
            },
        ],
    })
}

// ---------------------------------------------------------------------------------------------
// Reading a fixture.
// ---------------------------------------------------------------------------------------------

/// A wall clock, in the fields a `DATE-TIME` writes one.
fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<CivilDateTime> {
    Some(CivilDateTime::new(
        CivilDate::from_ymd(year, month, day)?,
        CivilTime::from_hms(hour, minute, 0)?,
    ))
}

/// The `VCALENDAR` a fixture's document holds.
fn calendar(document: &Document) -> Option<&Component> {
    document
        .components()
        .find(|component| component.is_named(b"VCALENDAR"))
}

/// The zone definitions a fixture carries, and what reading them said.
///
/// The parse has its own sink, so that what comes back is what `read_calendar_zones` reported
/// and nothing the grammar said on the way past.
fn zones_of(octets: &[u8]) -> Option<(VtimezoneSet, Vec<DiagnosticCode>)> {
    let document = Document::parse(octets, Limits::DEFAULT, &mut IgnoreDiagnostics).ok()?;
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let zones = read_calendar_zones(calendar(&document)?, &mut meter, &mut reported);
    let codes = reported.iter().copied().map(Diagnostic::code).collect();
    Some((zones, codes))
}

/// Which of the five outcomes this is, with the answers left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Both sources answered and said the same thing.
    Agreed,
    /// Both answered and did not.
    Disagreed,
    /// Only the calendar's own definitions answered.
    OnlyEmbedded,
    /// Only the caller's database answered.
    OnlyFallback,
    /// Neither recognized the identifier.
    Neither,
    /// A variant a later version of `ical-tz` added, which `#[non_exhaustive]` requires an arm
    /// for and this corpus has nothing to say about.
    Unnamed,
}

/// The shape of an outcome, which is all a caller reading only the variant learns.
fn shape<A: Copy>(outcome: PolicyOutcome<A>) -> Shape {
    match outcome {
        PolicyOutcome::Agreed { .. } => Shape::Agreed,
        PolicyOutcome::Disagreed { .. } => Shape::Disagreed,
        PolicyOutcome::OnlyEmbedded(_) => Shape::OnlyEmbedded,
        PolicyOutcome::OnlyFallback(_) => Shape::OnlyFallback,
        PolicyOutcome::Neither => Shape::Neither,
        _ => Shape::Unnamed,
    }
}

/// What one calendar, wired against one caller database, tells a caller about `BERLIN`.
///
/// The whole of what reaches a caller: what reading the file said, which outcome the pair
/// formed, and what reporting the outcome put on the sink. Two calendars stating different
/// things must not produce the same tuple.
fn what_a_caller_learns(
    octets: &[u8],
) -> Option<(Vec<DiagnosticCode>, Shape, Vec<DiagnosticCode>)> {
    let (zones, read) = zones_of(octets)?;
    let embedded = FileZones { zones: &zones };
    let database = berlin_2026()?;
    let combined = CombinedZoneSource::new(&embedded, &database);
    let local = stamp(2026, 7, 1, 12, 0)?;
    let moment = local.at_offset(UtcOffset::UTC)?;
    let outcome = combined.offset_at(BERLIN, moment);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut told: Vec<Diagnostic> = Vec::new();
    combined.report(outcome, moment, &mut meter, &mut told);
    let codes = told.iter().copied().map(Diagnostic::code).collect();
    Some((read, shape(combined.resolve(BERLIN, local)), codes))
}

// ---------------------------------------------------------------------------------------------
// The cases.
// ---------------------------------------------------------------------------------------------

/// RFC 5545 section 3.6.5 requires an observance and files without one exist. `ical-tz`'s own
/// `answer` module says `resolve` returns `None` "for exactly one condition: this source does not
/// recognize this `TZID`" — so a definition that is present and empty must not answer `None`.
///
/// What a caller can see: a calendar declaring `Europe/Berlin` with no observance and a calendar
/// declaring nothing at all produce the same outcome variant and the same diagnostics, so
/// "the file says nothing about this zone" and "the file has no definition for this zone" arrive
/// as one fact.
#[test]
fn a_definition_with_no_observance_is_not_an_unrecognized_identifier() {
    let (zones, declared) = zones_of(BERLIN_WITHOUT_OBSERVANCE).unwrap();
    assert_eq!(
        declared,
        vec![DiagnosticCode::VtimezoneWithoutObservance],
        "reading the file does say what is wrong with it, which is the half that works"
    );
    let table = zones.table(BERLIN).unwrap();
    assert_eq!(table.tzid().as_str(), BERLIN, "the source knows this TZID");
    assert!(
        table
            .resolve(BERLIN, stamp(2026, 7, 1, 12, 0).unwrap())
            .is_some(),
        "a source that recognizes an identifier may not answer None: None means unrecognized"
    );
}

/// The same collapse as it reaches a caller through the pair: an empty definition and no
/// definition at all produce the same outcome variant and the same reported codes, so the two
/// facts a reader would act on differently arrive as one.
#[test]
fn an_empty_definition_and_a_missing_definition_are_two_different_facts() {
    let declared = what_a_caller_learns(BERLIN_WITHOUT_OBSERVANCE).unwrap();
    let absent = what_a_caller_learns(BERLIN_WITHOUT_DEFINITION).unwrap();
    // Reading the file distinguishes them; resolving against a source does not.
    assert_eq!(declared.0, vec![DiagnosticCode::VtimezoneWithoutObservance]);
    assert_eq!(absent.0, vec![DiagnosticCode::MissingTimeZoneDefinition]);
    assert_ne!(
        (declared.1, declared.2),
        (absent.1, absent.2),
        "a definition that exists and is empty is not the same fact as no definition"
    );
}

/// The same collapse from the other side: with no caller database wired in, an empty definition
/// is reported as `unknown-time-zone` — a violation asserting that nobody supplied this zone,
/// about a zone the file supplies.
#[test]
fn an_empty_definition_is_not_reported_as_a_zone_nobody_supplied() {
    let (zones, _) = zones_of(BERLIN_WITHOUT_OBSERVANCE).unwrap();
    let embedded = FileZones { zones: &zones };
    let combined = CombinedZoneSource::new(&embedded, &NoZonesAtAll);
    let moment = stamp(2026, 7, 1, 12, 0)
        .unwrap()
        .at_offset(UtcOffset::UTC)
        .unwrap();
    let outcome = combined.offset_at(BERLIN, moment);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut told: Vec<Diagnostic> = Vec::new();
    combined.report(outcome, moment, &mut meter, &mut told);
    let codes: Vec<DiagnosticCode> = told.iter().copied().map(Diagnostic::code).collect();
    assert!(
        !codes.contains(&DiagnosticCode::UnknownTimeZone),
        "the calendar declares this identifier; what it lacks is an observance, and \
         vtimezone-without-observance is the code that says so"
    );
}

/// `docs/adr/0003` amendment 5: a table whose transitions run out answers by continuing its final
/// observance and says so on `AnswerBasis`. A table has two ends, and the claim is made about one.
///
/// The `RDATE` table here runs 2027 through 2029. Asked about 2035 it says
/// `BeyondKnownTransitions`, which is the ADR working. Asked about July 2020 — seven years before
/// its earliest transition — it answers `-05:00`, standard, `Computed`: the offset that
/// observance's `TZOFFSETFROM` states, extended backwards forever. New York was on `-04:00` that
/// July, so the answer is wrong, and nothing in it is distinguishable from an answer the table
/// had data for.
#[test]
fn an_answer_from_before_a_tables_first_transition_says_so_too() {
    let (zones, codes) = zones_of(NEW_YORK_DATED).unwrap();
    assert!(codes.is_empty());
    let table = zones.table(NEW_YORK).unwrap();
    assert_eq!(table.coverage_end(), CivilDate::from_ymd(2029, 11, 4));

    let past = table
        .resolve(NEW_YORK, stamp(2020, 7, 1, 12, 0).unwrap())
        .unwrap();
    let future = table
        .resolve(NEW_YORK, stamp(2035, 7, 1, 12, 0).unwrap())
        .unwrap();
    assert!(
        future.basis.is_beyond_known_transitions(),
        "the future end of the table is labeled, which is the half that works"
    );
    assert_eq!(
        reading_offset(past.resolution),
        Some(-18_000),
        "the answer is the first observance's TZOFFSETFROM, extended backwards"
    );
    assert_ne!(
        past.basis,
        AnswerBasis::Computed,
        "July 2020 is seven years before anything this table knows, and New York was on -04:00"
    );
}

/// The offset of a unique reading, in seconds east of UTC.
fn reading_offset(resolution: LocalResolution) -> Option<i32> {
    match resolution {
        LocalResolution::Unique { reading } => Some(reading.offset.seconds()),
        _ => None,
    }
}

/// RFC 5545 section 3.6.5 with `docs/adr/0003`: one identifier, two definitions, and the ADR's
/// whole subject is that a disagreement is reported and both readings stay reachable.
///
/// `VtimezoneSet::insert` does hand the refused table back — its own doc says "so a caller that
/// wants the later of two definitions can take it" — but `read_calendar_zones`, the only
/// whole-calendar entry point, drops it on the floor after reading its code. What reaches a
/// caller is one definition, a code that names neither the identifier nor which definition was
/// lost, and no route to the other reading at all.
#[test]
fn two_definitions_of_one_identifier_both_stay_reachable() {
    let (zones, codes) = zones_of(NEW_YORK_TWICE).unwrap();
    assert_eq!(codes, vec![DiagnosticCode::DuplicateTimeZoneIdentifier]);
    let table = zones.table(NEW_YORK).unwrap();
    // The 15th of March 2026 is EST under the pre-2007 rules and EDT under today's: the two
    // definitions are an hour apart on this wall clock, which is what makes them two answers.
    let local = stamp(2026, 3, 15, 12, 0).unwrap();
    let kept = table.resolve(NEW_YORK, local).unwrap();
    assert_eq!(
        reading_offset(kept.resolution),
        Some(-18_000),
        "the first won"
    );
    assert_eq!(
        zones
            .tables()
            .iter()
            .filter(|held| held.tzid().as_str() == NEW_YORK)
            .count(),
        2,
        "a file with two readings of one zone must not arrive as one reading and a note"
    );
}

/// The same loss with the stakes visible: a placeholder `VTIMEZONE` declared before the real one.
///
/// The empty definition is admitted first, the real definition is refused as a duplicate and
/// dropped, and the zone the file fully defines now answers nothing. A caller wiring the file's
/// zones against nothing is told `unknown-time-zone` about a zone the file defines with rules.
#[test]
fn a_placeholder_definition_does_not_swallow_the_real_one() {
    let (zones, codes) = zones_of(BERLIN_PLACEHOLDER_FIRST).unwrap();
    assert_eq!(
        codes,
        vec![
            DiagnosticCode::VtimezoneWithoutObservance,
            DiagnosticCode::DuplicateTimeZoneIdentifier,
        ],
        "both facts are reported, which is the half that works"
    );
    let table = zones.table(BERLIN).unwrap();
    let local = stamp(2026, 7, 1, 12, 0).unwrap();
    // Berlin is on CEST, +02:00, at noon on the first of July: the real definition says so and
    // the placeholder says nothing.
    assert_eq!(
        table
            .resolve(BERLIN, local)
            .and_then(|answer| reading_offset(answer.resolution)),
        Some(7_200),
        "the definition carrying the rules was dropped in favor of the one carrying nothing"
    );
}

/// `docs/adr/0003`: "Every result says which source produced it."
///
/// The ordinary server wiring is an embedded `VTIMEZONE` against a database the caller holds —
/// and a caller's database of zones is very often itself a set of `VTIMEZONE` definitions, which
/// is what RFC 7808's time zone service distributes and what every CalDAV server stores.
/// `TransitionTable`'s `ZoneSource` impl writes `ZoneProvenance::EmbeddedVtimezone` into every
/// answer unconditionally, so wiring one as the caller's half produces a `Disagreed` whose two
/// answers name the same source, and the fallback's answer names a source it did not come from.
#[test]
fn a_table_wired_in_as_the_callers_database_does_not_claim_to_be_the_file_s_own() {
    let (file, _) = zones_of(NEW_YORK_OLD).unwrap();
    let (database, _) = zones_of(NEW_YORK_TODAY).unwrap();
    let embedded = file.table(NEW_YORK).unwrap();
    let fallback = database.table(NEW_YORK).unwrap();
    let combined = CombinedZoneSource::new(embedded, fallback);
    let outcome = combined.resolve(NEW_YORK, stamp(2026, 3, 15, 12, 0).unwrap());
    let PolicyOutcome::Disagreed {
        embedded: older,
        fallback: newer,
    } = outcome
    else {
        panic!("the pre-2007 rules and today's are an hour apart on the 15th of March");
    };
    assert_eq!(reading_offset(older.resolution), Some(-18_000));
    assert_eq!(reading_offset(newer.resolution), Some(-14_400));
    assert_eq!(older.source, ZoneProvenance::EmbeddedVtimezone);
    assert_ne!(
        newer.source, older.source,
        "two answers from two sources may not name one source; provenance is the whole claim"
    );
    assert_eq!(
        newer.source,
        ZoneProvenance::CallerDatabase,
        "this table is the database the caller wired in, whatever syntax it was published in"
    );
}

/// The disagreement `docs/adr/0003` exists for, with the embedded half read from a file and the
/// caller half written by hand: both answers reachable, each naming itself, and the fact
/// reported once where the caller asked for it.
#[test]
fn an_embedded_definition_and_a_caller_database_that_disagree_keep_both_answers() {
    let (zones, _) = zones_of(NEW_YORK_OLD).unwrap();
    let embedded = FileZones { zones: &zones };
    let database = CallerZoneinfo {
        tzid: NEW_YORK,
        standing: UtcOffset::from_seconds(-18_000).unwrap(),
        shifts: vec![Shift {
            // The second Sunday in March 2026 at 02:00 EST, which is 07:00 UTC.
            moment: stamp(2026, 3, 8, 7, 0)
                .unwrap()
                .at_offset(UtcOffset::UTC)
                .unwrap(),
            vacated: UtcOffset::from_seconds(-18_000).unwrap(),
            adopted: UtcOffset::from_seconds(-14_400).unwrap(),
            daylight: true,
        }],
    };
    let combined = CombinedZoneSource::new(&embedded, &database);
    let local = stamp(2026, 3, 15, 12, 0).unwrap();
    let PolicyOutcome::Disagreed {
        embedded: from_file,
        fallback: from_database,
    } = combined.resolve(NEW_YORK, local)
    else {
        panic!("a file written under the old rules and a database on the new ones disagree");
    };
    assert_eq!(reading_offset(from_file.resolution), Some(-18_000));
    assert_eq!(reading_offset(from_database.resolution), Some(-14_400));
    assert_eq!(from_file.source, ZoneProvenance::EmbeddedVtimezone);
    assert_eq!(from_database.source, ZoneProvenance::CallerDatabase);

    let moment = local.at_offset(UtcOffset::UTC).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut told: Vec<Diagnostic> = Vec::new();
    combined.report(
        combined.offset_at(NEW_YORK, moment),
        moment,
        &mut meter,
        &mut told,
    );
    let codes: Vec<DiagnosticCode> = told.iter().copied().map(Diagnostic::code).collect();
    assert_eq!(codes, vec![DiagnosticCode::TimeZoneSourceDisagreement]);
}

/// A source that returns nothing for an identifier is one source, not a hole: the pair says
/// `OnlyEmbedded` and reports nothing, and the answer that exists still names its own source.
#[test]
fn a_caller_database_that_knows_nothing_leaves_the_file_s_answer_naming_itself() {
    let (zones, _) = zones_of(NEW_YORK_OLD).unwrap();
    let embedded = FileZones { zones: &zones };
    let combined = CombinedZoneSource::new(&embedded, &NoZonesAtAll);
    let local = stamp(2026, 3, 15, 12, 0).unwrap();
    let PolicyOutcome::OnlyEmbedded(answer) = combined.resolve(NEW_YORK, local) else {
        panic!("one source answered and the other recognized nothing");
    };
    assert_eq!(answer.source, ZoneProvenance::EmbeddedVtimezone);
    let moment = local.at_offset(UtcOffset::UTC).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut told: Vec<Diagnostic> = Vec::new();
    combined.report(
        combined.offset_at(NEW_YORK, moment),
        moment,
        &mut meter,
        &mut told,
    );
    assert!(
        told.is_empty(),
        "one source answering is not a defect in anything"
    );
    assert_eq!(
        shape(combined.resolve("Mars/Olympus_Mons", local)),
        Shape::Neither,
        "an identifier nobody supplied is reported rather than defaulted to UTC"
    );
}

/// RFC 5545 section 3.2.19: a `TZID` is an identifier, not a path and not an IANA key. One
/// carrying a colon has to be quoted where it is written as a parameter and not where it is
/// written as a property value, and both spellings must reach the same definition.
#[test]
fn an_identifier_carrying_a_colon_is_still_one_identifier() {
    let (zones, codes) = zones_of(PUNCTUATED_IDENTIFIERS).unwrap();
    assert_eq!(zones.len(), 2);
    assert!(
        zones.table("GMT+09:30").is_some(),
        "the property value after the first colon is the identifier, colons and all"
    );
    assert!(zones.table("Customized Time Zone").is_some());
    assert_eq!(
        codes,
        Vec::new(),
        "the event's quoted TZID parameter names the definition the file carries"
    );
    assert_eq!(Tzid::new("GMT+09:30").form(), TzidForm::Opaque);
    assert_eq!(Tzid::new("Customized Time Zone").form(), TzidForm::Opaque);
    assert_eq!(
        Tzid::new("/mozilla.org/20050126_1/Europe/Berlin").form(),
        TzidForm::GloballyUnique
    );
}

/// RFC 5545 section 3.2's `DQUOTE` rule is stated over *parameter* values; a `TZID` property's
/// value is `TEXT`, where a `DQUOTE` is an ordinary character. `ical-tz`'s reader strips a
/// matched pair from the property value anyway, so that a producer that quoted one side is still
/// reachable from the other — a defensible interoperability choice with a cost this case records
/// rather than asserts against: a file declaring both spellings declares one identifier, and one
/// of its two definitions is refused as a duplicate.
#[test]
fn a_quoted_identifier_and_an_unquoted_one_are_read_as_one_zone() {
    let (zones, codes) = zones_of(BERLIN_QUOTED_AND_NOT).unwrap();
    assert_eq!(codes, vec![DiagnosticCode::DuplicateTimeZoneIdentifier]);
    assert_eq!(zones.len(), 1);
    assert!(
        zones.table("\"Europe/Berlin\"").is_none(),
        "the quotes are gone from the key, which is what makes the two spellings one zone"
    );
}

/// RFC 5545 section 3.8.3.1 gives the `TZID` property the `TEXT` value type, where `\n` is an
/// escape, and section 3.2 gives a parameter value no escapes at all. So a file writing the same
/// two octets on both sides has written one name as octets and two names as values. `ical-tz`
/// compares the octets on both sides, which is the reading that makes the file self-consistent
/// and the one this case records: the definition is reachable and nothing is reported missing.
#[test]
fn an_identifier_carrying_a_text_escape_is_compared_as_the_octets_both_sides_wrote() {
    let (zones, codes) = zones_of(IDENTIFIER_WITH_ESCAPE).unwrap();
    assert_eq!(zones.len(), 1);
    assert!(zones.table("Europe\\nBerlin").is_some(), "filed as written");
    assert!(
        zones.table("Europe\nBerlin").is_none(),
        "no TEXT unescaping happens on either side, so the escape stays two octets"
    );
    assert_eq!(codes, Vec::new());
}

/// `docs/adr/0010` bounds how many zones one calendar may declare, and `docs/adr/0003` amendment
/// 6 records that the refusal carries no code. What it does carry is a *different* code, about a
/// different fact, that is false: the zones the bound refused are then reported as identifiers
/// no `VTIMEZONE` defines. The file defines them. The caller's own policy dropped them.
#[test]
fn a_zone_the_limit_refused_is_not_reported_as_one_the_file_never_defined() {
    let limits = Limits::DEFAULT.with_max_vtimezone_components(1);
    let document = Document::parse(TWO_ZONES, limits, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(limits);
    let mut told: Vec<Diagnostic> = Vec::new();
    let zones = read_calendar_zones(calendar(&document).unwrap(), &mut meter, &mut told);
    assert_eq!(zones.len(), 1, "the policy admitted one of the two");
    let codes: Vec<DiagnosticCode> = told.iter().copied().map(Diagnostic::code).collect();
    assert!(
        !codes.contains(&DiagnosticCode::MissingTimeZoneDefinition),
        "this calendar defines both zones it references; what refused one was the limit"
    );
}

/// A `TZID` a calendar references and no `VTIMEZONE` defines is reported — and the report names
/// neither the identifier nor anything else that tells one of them from another. Three undefined
/// zones arrive as three diagnostics that are equal as values.
#[test]
fn an_identifier_nothing_defines_is_reported_by_name() {
    let document =
        Document::parse(THREE_UNDEFINED, Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap();
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut told: Vec<Diagnostic> = Vec::new();
    let zones = read_calendar_zones(calendar(&document).unwrap(), &mut meter, &mut told);
    assert!(zones.is_empty());
    assert_eq!(
        told.iter()
            .copied()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        vec![DiagnosticCode::MissingTimeZoneDefinition; 3],
        "one report per identifier, which is the half that works"
    );
    let first = *told.first().unwrap();
    assert!(
        told.iter().any(|entry| *entry != first),
        "three different missing zones may not arrive as three identical values: a caller \
         cannot say which zone it must go and find"
    );
}
