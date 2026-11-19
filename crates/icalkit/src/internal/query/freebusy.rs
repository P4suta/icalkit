// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Generate free-busy reports under RFC 4791 sections 7.10 and 9.11.
//!
//! Each [`Placement`] supplies a component, its expanded intervals in UTC, and any incomplete expansion reason.
//! The overlap and expansion code supplies the same intervals for reports and time-range filters.
//! For an all-day event, the interval follows its placement zone.
//!
//! An opaque `VEVENT` contributes busy time unless its status is `CANCELLED`.
//! An absent `TRANSP` means `OPAQUE`.
//! An absent `STATUS` means `CONFIRMED`.
//! A tentative event contributes `BUSY-TENTATIVE`.
//! An opaque event with an extension status contributes `BUSY`.
//! A transparent event contributes no busy period.
//!
//! A `VFREEBUSY` contributes its explicit busy periods.
//! The report clips them to the window and coalesces consecutive or overlapping periods of the same type.
//! Different types can cover the same instant.
//!
//! [`BusyAnswer::unplaced`] identifies resources with incomplete coverage.
//! For example, placing a floating `DTSTART` in UTC requires a zone.
//! A missing period in an incomplete report gives no assurance of free time.
//! The shared period limit bounds the report.
//! Failure to store a reason within the shared allocation budget ends the operation.

use alloc::vec::Vec;

use crate::internal::core::{
    Component, ComponentKind, DateTimeValue, DecodeValue, DiagnosticCode, Instant, LimitExceeded,
    Meter, Parameter, Period, Property, UtcOffset,
};
use crate::internal::dav::FreeBusyQuery;

use crate::internal::query::expand::Instance;
use crate::internal::query::vocabulary::{
    Budget, BusyPeriod, BusyType, FreeBusyReport, QueryError, Undecided,
};

/// Specification sections for the mapping, clipping, coalescing, and limits of a free-busy report.
pub const FREE_BUSY_SECTIONS: &[&str] = &[
    "RFC 4791 section 7.10, the CALDAV:free-busy-query REPORT",
    "RFC 4791 section 7.10, the TRANSP and STATUS to FBTYPE mapping",
    "RFC 4791 section 7.10, coalescing consecutive or overlapping periods of one type",
    "RFC 4791 section 9.9, the half-open window a time-range states",
    "RFC 4791 section 9.11, the free-busy-query body",
    "RFC 5545 section 3.2.9, FBTYPE",
    "RFC 5545 section 3.8.1.11, STATUS",
    "RFC 5545 section 3.8.2.6, FREEBUSY: a UTC period list",
    "RFC 5545 section 3.8.2.7, TRANSP",
];

/// Seconds in a UTC day when placing a `FREEBUSY` start and duration.
const SECONDS_PER_DAY: i64 = 86_400;

/// The parameter RFC 5545 section 3.2.9 states a `FREEBUSY` period's type in.
const FBTYPE: &[u8] = b"FBTYPE";

/// `TRANSP:OPAQUE`, RFC 5545 section 3.8.2.7, and the reading an absent property gets.
const OPAQUE: &[u8] = b"OPAQUE";

/// `TRANSP:TRANSPARENT`, RFC 5545 section 3.8.2.7.
const TRANSPARENT: &[u8] = b"TRANSPARENT";

/// `STATUS:CANCELLED`, RFC 5545 section 3.8.1.11.
const CANCELLED: &[u8] = b"CANCELLED";

/// `STATUS:TENTATIVE`, RFC 5545 section 3.8.1.11.
const TENTATIVE: &[u8] = b"TENTATIVE";

/// One component and its expansion within the query window.
///
/// The caller selects children of `VCALENDAR`.
/// Components outside RFC 4791 section 7.10 contribute no busy time.
#[derive(Clone, Copy, Debug)]
pub struct Placement<'a> {
    /// The component the instances are of.
    pub component: &'a Component,
    /// Instances that expansion placed in the window.
    /// Each interval starts and ends in UTC.
    /// An empty slice contributes no busy periods.
    pub instances: &'a [Instance],
    /// The reason the expansion stopped early, if any.
    /// The report charges the shared meter before storing the reason.
    pub incomplete: Option<Undecided>,
}

/// A resource with incomplete free-busy coverage and the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unplaced {
    /// The index in the placements passed to [`free_busy`].
    /// An incomplete expansion and an unreadable value can produce separate entries for the same resource.
    pub placement: usize,
    /// The cause of the incomplete coverage.
    pub reason: Undecided,
}

/// A free-busy report with the reasons for incomplete coverage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyAnswer {
    /// The busy periods, coalesced and clipped to the query's window.
    report: FreeBusyReport,
    /// Resources with incomplete coverage, in input order.
    unplaced: Vec<Unplaced>,
}

impl BusyAnswer {
    /// The busy periods the report collected.
    #[must_use]
    pub const fn report(&self) -> &FreeBusyReport {
        &self.report
    }

    /// The resources with incomplete coverage.
    #[must_use]
    pub fn unplaced(&self) -> &[Unplaced] {
        &self.unplaced
    }

    /// Whether the report accounts for every resource.
    /// Unplaced resources and truncation both make coverage incomplete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unplaced.is_empty() && !self.report.is_truncated()
    }

    /// The diagnostic code for an unplaced resource, or `None` if all resources have placements.
    #[must_use]
    pub fn code(&self) -> Option<DiagnosticCode> {
        if self.unplaced.is_empty() {
            None
        } else {
            Some(Undecided::CODE)
        }
    }

    /// Return the report and discard its record of unplaced resources.
    #[must_use]
    pub fn into_report(self) -> FreeBusyReport {
        self.report
    }
}

/// Generate busy periods for a `CALDAV:free-busy-query` under RFC 4791 section 7.10.
///
/// Supply each component's expanded instances and any reason for incomplete expansion in one placement.
/// The caller can share a meter across reports to apply one period budget to the entire exchange.
/// Resource-specific reasons remain available on [`BusyAnswer::unplaced`].
///
/// # Errors
///
/// Returns [`QueryError::Unrepresentable`] for a value the report can't write.
/// Insufficient space in the shared budget also ends the operation.
pub fn free_busy(
    query: FreeBusyQuery,
    placements: &[Placement<'_>],
    budget: &mut Budget<'_>,
) -> Result<BusyAnswer, QueryError> {
    let window = (query.range.start(), query.range.end());
    let mut coalesced = Coalesced::new(budget.limits.max_freebusy_periods());
    let mut unplaced = Vec::new();
    for (index, entry) in placements.iter().enumerate() {
        if let Some(reason) = entry.incomplete {
            record(&mut unplaced, index, reason, budget.meter)?;
        }
        match contribute(*entry, window, &mut coalesced) {
            Ok(()) => {},
            Err(Refusal::Unplaceable(reason)) => {
                record(&mut unplaced, index, reason, budget.meter)?;
            },
            Err(Refusal::Refused(error)) => return Err(error),
        }
    }
    Ok(BusyAnswer {
        report: assemble(window, coalesced, budget.meter),
        unplaced,
    })
}

/// A resource-level placement failure or an error that prevents constructing the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// This resource has no place on the timeline, and the answer says so.
    Unplaceable(Undecided),
    /// The whole evaluation stops here.
    Refused(QueryError),
}

impl From<Undecided> for Refusal {
    fn from(reason: Undecided) -> Self {
        Self::Unplaceable(reason)
    }
}

impl From<QueryError> for Refusal {
    fn from(error: QueryError) -> Self {
        Self::Refused(error)
    }
}

/// Add what one component contributes to `coalesced`.
fn contribute(
    placement: Placement<'_>,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    match placement.component.kind() {
        Some(ComponentKind::Event) => collect_event(placement, window, coalesced),
        Some(ComponentKind::FreeBusy) => collect_free_busy(placement.component, window, coalesced),
        // RFC 4791 section 7.10 includes only events and free-busy components.
        Some(_) | None => Ok(()),
    }
}

/// Add each placed event instance under its component's `TRANSP` and `STATUS`.
/// Overrides with different properties arrive as separate placements.
fn collect_event(
    placement: Placement<'_>,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    let Some(kind) = event_kind(placement.component)? else {
        return Ok(());
    };
    for instance in placement.instances {
        let period = BusyPeriod::new(instance.start(), instance.end(), kind);
        coalesced.insert(clip(period, window))?;
    }
    Ok(())
}

/// What one `VEVENT` contributes, or `None` for the rows of the table that state `FREE`.
fn event_kind(component: &Component) -> Result<Option<BusyType>, Undecided> {
    if !is_opaque(component)? {
        // Transparent events contribute no period under any status.
        return Ok(None);
    }
    let status = component.status();
    let Some(text) = status.value() else {
        return if status.is_present() {
            Err(Undecided::ValueUnreadable)
        } else {
            // The table's default row: no `STATUS` is `CONFIRMED`, which is `BUSY`.
            Ok(Some(BusyType::Busy))
        };
    };
    // RFC 5545 section 3.1 compares an enumerated property value case-insensitively.
    let value = text.as_bytes();
    if value.eq_ignore_ascii_case(CANCELLED) {
        Ok(None)
    } else if value.eq_ignore_ascii_case(TENTATIVE) {
        Ok(Some(BusyType::Tentative))
    } else {
        // RFC 4791 section 7.10 permits `BUSY` for an opaque event with an extension status.
        Ok(Some(BusyType::Busy))
    }
}

/// Whether a component's time is its own, RFC 5545 section 3.8.2.7.
fn is_opaque(component: &Component) -> Result<bool, Undecided> {
    let transp = component.transp();
    let Some(text) = transp.value() else {
        return if transp.is_present() {
            Err(Undecided::ValueUnreadable)
        } else {
            // Section 3.8.2.7's default, which section 7.10's table names as such.
            Ok(true)
        };
    };
    let value = text.as_bytes();
    if value.eq_ignore_ascii_case(OPAQUE) {
        Ok(true)
    } else if value.eq_ignore_ascii_case(TRANSPARENT) {
        Ok(false)
    } else {
        // RFC 5545 section 3.8.2.7 defines two transparency values.
        Err(Undecided::ValueUnreadable)
    }
}

/// Add busy periods from a `VFREEBUSY` under RFC 5545 section 3.8.2.6.
/// The component's `DTSTART` and `DTEND` describe the reporting window.
fn collect_free_busy(
    component: &Component,
    window: (Option<Instant>, Option<Instant>),
    coalesced: &mut Coalesced,
) -> Result<(), Refusal> {
    for property in component.freebusy() {
        let kind = free_busy_kind(property)?;
        for (_, period) in freebusy_fields(
            property.value_text().as_bytes(),
            property.parameters().iter(),
        ) {
            let period = utc_period(period.ok_or(Undecided::ValueUnreadable)?, kind)?;
            if kind != BusyType::Free {
                coalesced.insert(clip(period, window))?;
            }
        }
    }
    Ok(())
}

pub(super) fn freebusy_fields<'a>(
    value: &'a [u8],
    mut parameters: impl Iterator<Item = &'a Parameter>,
) -> impl Iterator<Item = (&'a [u8], Option<Period<'a>>)> {
    let mut explicit = false;
    let valid = parameters.all(|parameter| {
        if parameter.is_named(b"VALUE") {
            return !core::mem::replace(&mut explicit, true)
                && parameter.unquoted().eq_ignore_ascii_case(b"PERIOD");
        }
        !parameter.is_named(b"TZID")
    });
    value.split(|&byte| byte == b',').map(move |field| {
        (
            field,
            if valid {
                Period::decode_value(field).ok()
            } else {
                None
            },
        )
    })
}

/// Which type a `FREEBUSY` line states, RFC 5545 section 3.2.9.
fn free_busy_kind(property: &Property) -> Result<BusyType, Undecided> {
    match property.parameters_named(FBTYPE).next() {
        // Section 3.2.9 makes `BUSY` the value of an absent parameter.
        None => Ok(BusyType::Busy),
        Some(parameter) => BusyType::parse(parameter.unquoted()).ok_or(Undecided::ValueUnreadable),
    }
}

/// One `FREEBUSY` period, whose bounds RFC 5545 section 3.8.2.6 requires in UTC.
fn utc_period(written: Period<'_>, kind: BusyType) -> Result<BusyPeriod, Refusal> {
    let begins = utc_instant(written.start())?;
    let ends = match written {
        Period::Explicit { end, .. } => utc_instant(end)?,
        Period::Starting { duration, .. } => {
            // In UTC, each duration day contributes 86,400 seconds.
            let carried = duration
                .days()
                .checked_mul(SECONDS_PER_DAY)
                .ok_or(QueryError::Unrepresentable)?;
            let length = carried
                .checked_add(duration.seconds())
                .ok_or(QueryError::Unrepresentable)?;
            begins
                .checked_add_seconds(length)
                .ok_or(QueryError::Unrepresentable)?
        },
    };
    Ok(BusyPeriod::new(begins, ends, kind))
}

/// Read a UTC bound as RFC 5545 section 3.8.2.6 requires.
fn utc_instant(bound: DateTimeValue<'_>) -> Result<Instant, Refusal> {
    match bound {
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(Refusal::Refused(QueryError::Unrepresentable)),
        // Applying a query zone would invent an instant for this bound.
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Zoned { .. } => {
            Err(Refusal::Unplaceable(Undecided::ValueUnreadable))
        },
    }
}

/// Intersect a busy period with the half-open query window.
/// A period that just touches the window contributes no busy time.
fn clip(period: BusyPeriod, window: (Option<Instant>, Option<Instant>)) -> BusyPeriod {
    let start = match window.0 {
        Some(bound) if bound > period.start => bound,
        Some(_) | None => period.start,
    };
    let end = match window.1 {
        Some(bound) if bound < period.end => bound,
        Some(_) | None => period.end,
    };
    BusyPeriod::new(start, end, period.kind)
}

/// Whether two periods of the same type meet or overlap under RFC 4791 section 7.10.
fn touches(held: BusyPeriod, other: BusyPeriod) -> bool {
    held.start <= other.end && other.start <= held.end
}

/// Sorted, coalesced busy periods.
/// Merging on insertion applies the resource limit to the working list.
#[derive(Debug)]
struct Coalesced {
    /// The periods, ordered by start and pairwise non-touching within each type.
    periods: Vec<BusyPeriod>,
    /// The most periods this list may hold, from the caller's policy.
    cap: usize,
    /// Whether that bound turned a period away.
    truncated: bool,
}

impl Coalesced {
    /// An empty list bounded at `cap` periods.
    fn new(cap: u32) -> Self {
        Self {
            periods: Vec::new(),
            // A policy larger than this target's addressable memory binds at the allocator instead, which refuses in the same direction.
            cap: usize::try_from(cap).unwrap_or(usize::MAX),
            truncated: false,
        }
    }

    /// Merge the period with every touching or overlapping period of the same type.
    /// Start order and prior coalescing make one pass sufficient.
    fn insert(&mut self, period: BusyPeriod) -> Result<(), QueryError> {
        if period.is_empty() {
            return Ok(());
        }
        let mut grown = period;
        let before = self.periods.len();
        self.periods.retain(|held| {
            if held.kind != grown.kind || !touches(*held, grown) {
                return true;
            }
            grown.start = grown.start.min(held.start);
            grown.end = grown.end.max(held.end);
            false
        });
        if self.periods.len() == before && before >= self.cap {
            // An entry that merges can fit even when the list has reached its limit.
            self.truncated = true;
            return Ok(());
        }
        self.periods
            .try_reserve(1)
            .map_err(|_| LimitExceeded::FreeBusyPeriods)?;
        let at = self
            .periods
            .partition_point(|held| held.start <= grown.start);
        self.periods.insert(at, grown);
        Ok(())
    }
}

/// Transfer the periods to the report, marking truncation if the remaining period budget runs out.
fn assemble(
    window: (Option<Instant>, Option<Instant>),
    coalesced: Coalesced,
    meter: &mut Meter,
) -> FreeBusyReport {
    let mut report = FreeBusyReport::new(window.0, window.1);
    if coalesced.truncated {
        report.note_truncated();
    }
    for period in coalesced.periods {
        if report.push(period, meter).is_err() {
            report.note_truncated();
            break;
        }
    }
    report
}

/// Charge the byte budget before storing a resource with incomplete coverage.
/// Such a resource can contribute no periods.
fn record(
    unplaced: &mut Vec<Unplaced>,
    placement: usize,
    reason: Undecided,
    meter: &mut Meter,
) -> Result<(), QueryError> {
    let cost = u64::try_from(core::mem::size_of::<Unplaced>()).unwrap_or(u64::MAX);
    meter.try_charge(cost)?;
    unplaced.try_reserve(1).map_err(|_| LimitExceeded::Budget)?;
    unplaced.push(Unplaced { placement, reason });
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{Component, Document, IgnoreDiagnostics, Instant, Limits, Meter};
    use crate::internal::dav::{FreeBusyQuery, TimeRange};

    use super::{BusyAnswer, Placement, free_busy};
    use crate::internal::query::expand::Instance;
    use crate::internal::query::vocabulary::{Budget, BusyType, Undecided};

    #[test]
    fn freebusy_parameters_are_read_once_per_line() {
        let extension = super::Parameter::create(b"X-LABEL", b"room").unwrap();
        let parameters = core::iter::repeat_n(&extension, 8);
        let visited = core::cell::Cell::new(0);
        let fields = b"20260815T090000Z/PT1H,20260815T100000Z/PT1H,20260815T110000Z/PT1H";
        let periods = super::freebusy_fields(
            fields,
            parameters.inspect(|_| visited.set(visited.get() + 1)),
        );
        assert_eq!(periods.filter(|(_, period)| period.is_some()).count(), 3);
        assert_eq!(visited.get(), 8);
    }

    /// 2026-01-01T00:00:00Z, where every window below opens.
    const WINDOW_START: i64 = 1_767_225_600;

    /// 2026-01-08T00:00:00Z, one week later.
    const WINDOW_END: i64 = 1_767_830_400;

    /// 2026-01-02T09:00:00Z.
    const AT_NINE: i64 = 1_767_344_400;

    /// 2026-01-02T10:00:00Z.
    const AT_TEN: i64 = 1_767_348_000;

    /// 2026-01-02T11:00:00Z.
    const AT_ELEVEN: i64 = 1_767_351_600;

    /// 2026-01-02T12:00:00Z.
    const AT_TWELVE: i64 = 1_767_355_200;

    /// 2026-01-02T13:00:00Z.
    const AT_THIRTEEN: i64 = 1_767_358_800;

    /// 2026-01-02T14:00:00Z.
    const AT_FOURTEEN: i64 = 1_767_362_400;

    /// 2025-12-31T23:00:00Z, one hour before the window opens.
    const BEFORE_WINDOW: i64 = 1_767_222_000;

    /// 2026-01-01T01:00:00Z, one hour after it opens.
    const INSIDE_WINDOW: i64 = 1_767_229_200;

    /// 2026-01-08T01:00:00Z, one hour after it closes.
    const AFTER_WINDOW: i64 = 1_767_834_000;

    /// One row of the transcription table.
    struct Case {
        /// What the row asserts, quoted at whichever assertion fails.
        what: &'static str,
        /// The calendar in component order.
        ics: &'static str,
        /// Each child's expanded start and end.
        instances: &'static [(i64, i64)],
        /// The periods RFC 4791 section 7.10 says come back, in order.
        periods: &'static [(i64, i64, BusyType)],
        /// Placement failures in input order.
        unplaced: &'static [Undecided],
    }

    /// The calendar `ics` spells.
    fn parse(ics: &str) -> Document {
        Document::parse(ics.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// Return every child so the test exercises component selection.
    fn children(document: &Document) -> Vec<&Component> {
        document
            .components()
            .flat_map(Component::components)
            .collect()
    }

    /// The instances `spans` names, each addressed by its own start.
    fn placed(spans: &[(i64, i64)]) -> Vec<Instance> {
        spans
            .iter()
            .map(|&(start, end)| {
                let at = Instant::from_unix_seconds(start);
                Instance::new(at, at, Instant::from_unix_seconds(end))
            })
            .collect()
    }

    /// Give each child one complete placement.
    fn one_each<'a>(components: &[&'a Component], found: &'a [Instance]) -> Vec<Placement<'a>> {
        components
            .iter()
            .zip(found.iter())
            .map(|(&child, at)| Placement {
                component: child,
                instances: core::slice::from_ref(at),
                incomplete: None,
            })
            .collect()
    }

    /// A query over `start ..< end`.
    fn query(start: i64, end: i64) -> FreeBusyQuery {
        FreeBusyQuery {
            range: TimeRange::new(
                Some(Instant::from_unix_seconds(start)),
                Some(Instant::from_unix_seconds(end)),
            )
            .unwrap(),
        }
    }

    /// The report as the triples a table row states.
    fn periods_of(answer: &BusyAnswer) -> Vec<(i64, i64, BusyType)> {
        answer
            .report()
            .periods()
            .iter()
            .map(|held| {
                (
                    held.start.unix_seconds(),
                    held.end.unix_seconds(),
                    held.kind,
                )
            })
            .collect()
    }

    /// Return the reasons in report order.
    fn reasons_of(answer: &BusyAnswer) -> Vec<Undecided> {
        answer.unplaced().iter().map(|held| held.reason).collect()
    }

    /// Expected outcomes from RFC 4791 sections 7.10 and 9.9 for status mapping, coalescing, and window boundaries.
    const CASES: &[Case] = &[
        Case {
            what: "an opaque confirmed event is busy for its whole period",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:OPAQUE\r\nSTATUS:CONFIRMED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a VEVENT stating neither property takes the table's two defaults",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "TRANSPARENT states the time is free, so nothing is reported",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:c\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:TRANSPARENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "TRANSPARENT wins over a STATUS that would otherwise be busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:TRANSPARENT\r\nSTATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an opaque CANCELLED event maps to FREE, so nothing is reported",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "TENTATIVE maps to BUSY-TENTATIVE",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:f\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Tentative)],
            unplaced: &[],
        },
        Case {
            what: "a STATUS nothing defines takes the x-name row, which is BUSY",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:g\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:X-CATERING\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a TRANSP section 3.8.2.7 does not define is undecidable rather than opaque",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:h\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  TRANSP:X-MAYBE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "a busy hour and a tentative hour over the same time stay two periods",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:i1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:i2\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                  STATUS:TENTATIVE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_NINE, AT_TEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_NINE, AT_TEN, BusyType::Tentative),
            ],
            unplaced: &[],
        },
        Case {
            what: "two consecutive busy periods of one type coalesce into one",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:j1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:j2\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_TEN, AT_ELEVEN)],
            periods: &[(AT_NINE, AT_ELEVEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "two overlapping busy periods of one type coalesce into one",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:k1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:k2\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_ELEVEN), (AT_TEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TWELVE, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a period the second one swallows whole leaves one period",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:l1\r\n\
                  DTSTART:20260102T100000Z\r\nDTEND:20260102T110000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:l2\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_TEN, AT_ELEVEN), (AT_NINE, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TWELVE, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an event that begins before the window is clipped to it",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m\r\n\
                  DTSTART:20251231T230000Z\r\nDTEND:20260101T010000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(BEFORE_WINDOW, INSIDE_WINDOW)],
            periods: &[(WINDOW_START, INSIDE_WINDOW, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an event ending exactly at the window start is outside a half-open window",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:n\r\n\
                  DTSTART:20251231T230000Z\r\nDTEND:20260101T000000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(BEFORE_WINDOW, WINDOW_START)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an event beginning exactly at the window end is outside it",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:o\r\n\
                  DTSTART:20260108T000000Z\r\nDTEND:20260108T010000Z\r\n\
                  END:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(WINDOW_END, AFTER_WINDOW)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an instance of no length states no busy time, section 9.9's instant row",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:p\r\n\
                  DTSTART:20260102T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_NINE)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "a VFREEBUSY's FREEBUSY periods are aggregated",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:q\r\n\
                  FREEBUSY:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE of BUSY-TENTATIVE is carried through",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:r\r\n\
                  FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Tentative)],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE of FREE states free time, which is not busy time",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:s\r\n\
                  FREEBUSY;FBTYPE=FREE:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "an FBTYPE nothing defines is undecidable rather than busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:t\r\n\
                  FREEBUSY;FBTYPE=X-VACATION:20260102T090000Z/20260102T100000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "one FREEBUSY line carrying two periods contributes both",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:u\r\n\
                  FREEBUSY:20260102T090000Z/20260102T100000Z,\
                  20260102T110000Z/20260102T120000Z\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_THIRTEEN, AT_FOURTEEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_ELEVEN, AT_TWELVE, BusyType::Busy),
            ],
            unplaced: &[],
        },
        Case {
            what: "a period written as a start and a duration is aggregated too",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:v\r\n\
                  FREEBUSY:20260102T090000Z/PT1H\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[(AT_NINE, AT_TEN, BusyType::Busy)],
            unplaced: &[],
        },
        Case {
            what: "a FREEBUSY bound that is not in UTC is undecidable rather than placed",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:w\r\n\
                  FREEBUSY:20260102T090000/20260102T100000\r\n\
                  END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_ELEVEN, AT_TWELVE)],
            periods: &[],
            unplaced: &[Undecided::ValueUnreadable],
        },
        Case {
            what: "a VTODO is not one of the two component types section 7.10 considers",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:x\r\n\
                  DTSTART:20260102T090000Z\r\nDUE:20260102T100000Z\r\n\
                  END:VTODO\r\nEND:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN)],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "a collection with nothing in it reports nothing and hides nothing",
            ics: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
            instances: &[],
            periods: &[],
            unplaced: &[],
        },
        Case {
            what: "every component of a collection contributes when every one is busy",
            ics: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:y1\r\n\
                  DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VEVENT\r\nUID:y2\r\n\
                  DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                  BEGIN:VFREEBUSY\r\nUID:y3\r\n\
                  FREEBUSY:20260102T130000Z/20260102T140000Z\r\nEND:VFREEBUSY\r\n\
                  END:VCALENDAR\r\n",
            instances: &[(AT_NINE, AT_TEN), (AT_ELEVEN, AT_TWELVE), (AT_NINE, AT_TEN)],
            periods: &[
                (AT_NINE, AT_TEN, BusyType::Busy),
                (AT_ELEVEN, AT_TWELVE, BusyType::Busy),
                (AT_THIRTEEN, AT_FOURTEEN, BusyType::Busy),
            ],
            unplaced: &[],
        },
    ];

    #[test]
    fn section_7_10_decides_every_row_of_the_table() {
        for case in CASES {
            let document = parse(case.ics);
            let components = children(&document);
            assert_eq!(
                components.len(),
                case.instances.len(),
                "the row states one instance per child: {}",
                case.what
            );
            let found = placed(case.instances);
            let placements = one_each(&components, &found);
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            let answer =
                free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

            assert_eq!(periods_of(&answer), case.periods, "{}", case.what);
            assert_eq!(reasons_of(&answer), case.unplaced, "{}", case.what);
            assert_eq!(
                answer.is_complete(),
                case.unplaced.is_empty(),
                "a resource that could not be accounted for is an incomplete report: {}",
                case.what
            );
        }
    }

    /// The report preserves the supplied 23-hour and 25-hour intervals.
    #[test]
    fn a_day_that_is_not_twenty_four_hours_long_survives_the_report_unchanged() {
        // 2026-03-08T05:00:00Z to 2026-03-09T04:00:00Z, and 2026-11-01T04:00:00Z to 2026-11-02T05:00:00Z.
        let cases = [
            (
                "the day a zone springs forward",
                1_772_946_000_i64,
                1_773_028_800_i64,
                82_800_i64,
            ),
            (
                "the day it falls back",
                1_793_505_600_i64,
                1_793_595_600_i64,
                90_000_i64,
            ),
        ];
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:all-day\r\n\
                   DTSTART;VALUE=DATE:20260308\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);

        for (what, begins, ends, seconds) in cases {
            let found = placed(&[(begins, ends)]);
            let placements = one_each(&components, &found);
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            // A year-wide window, so what shortens or lengthens the day is the zone that placed it and never the query's own clipping.
            let answer =
                free_busy(query(WINDOW_START, 1_798_761_600), &placements, &mut budget).unwrap();

            assert_eq!(
                periods_of(&answer),
                [(begins, ends, BusyType::Busy)],
                "{what}"
            );
            assert_eq!(
                Instant::from_unix_seconds(begins)
                    .checked_seconds_until(Instant::from_unix_seconds(ends)),
                Some(seconds),
                "{what}"
            );
            assert_ne!(
                seconds, 86_400,
                "a day of exactly 24 hours would prove nothing: {what}"
            );
        }
    }

    /// A report the period bound stopped says so, because a missing period reads as free time.
    #[test]
    fn a_report_stopped_at_its_bound_is_truncated_rather_than_short_and_silent() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:3\r\n\
                   DTSTART:20260102T130000Z\r\nDTEND:20260102T140000Z\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[
            (AT_NINE, AT_TEN),
            (AT_ELEVEN, AT_TWELVE),
            (AT_THIRTEEN, AT_FOURTEEN),
        ]);
        let placements = one_each(&components, &found);
        let limits = Limits::DEFAULT.with_max_freebusy_periods(2);
        let mut meter = Meter::new(limits);
        let mut budget = Budget::new(limits, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(answer.report().periods().len(), 2);
        assert!(answer.report().is_truncated());
        assert!(!answer.is_complete());
        assert_eq!(
            answer.report().window(),
            (
                Some(Instant::from_unix_seconds(WINDOW_START)),
                Some(Instant::from_unix_seconds(WINDOW_END))
            )
        );
    }

    /// An earlier report can consume part of the shared period budget.
    #[test]
    fn a_shared_ledger_truncates_the_report_and_latches() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000Z\r\nDTEND:20260102T120000Z\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN), (AT_ELEVEN, AT_TWELVE)]);
        let placements = one_each(&components, &found);
        let limits = Limits::DEFAULT.with_max_freebusy_periods(2);
        let mut meter = Meter::new(limits);
        meter.try_charge_freebusy_period().unwrap();
        let mut budget = Budget::new(limits, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(answer.report().periods().len(), 1);
        assert!(answer.report().is_truncated());
        assert!(meter.is_exhausted(), "the ledger latches on the refusal");
    }

    /// An incomplete expansion leaves uncertainty in the report.
    #[test]
    fn an_incomplete_expansion_is_reported_rather_than_read_as_free_time() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:2\r\n\
                   DTSTART:20260102T110000\r\nDTEND:20260102T120000\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN)]);
        let placements = [
            Placement {
                component: components[0],
                instances: &found,
                incomplete: None,
            },
            Placement {
                component: components[1],
                instances: &[],
                incomplete: Some(Undecided::ZoneUnstated),
            },
        ];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TEN, BusyType::Busy)]);
        assert_eq!(answer.unplaced().len(), 1);
        assert_eq!(answer.unplaced()[0].placement, 1);
        assert_eq!(answer.unplaced()[0].reason, Undecided::ZoneUnstated);
        assert_eq!(answer.code(), Some(Undecided::CODE));
        assert!(!answer.is_complete());
        assert!(
            !answer.report().is_truncated(),
            "an unaccounted resource is not a truncation: no bound turned anything away"
        );
    }

    /// A partial expansion contributes its known instances and a reason for the missing ones.
    #[test]
    fn a_partial_expansion_contributes_its_instances_and_reports_the_rest() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:series\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=DAILY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(AT_NINE, AT_TEN)]);
        let placements = [Placement {
            component: components[0],
            instances: &found,
            incomplete: Some(Undecided::SearchExhausted),
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TEN, BusyType::Busy)]);
        assert_eq!(reasons_of(&answer), [Undecided::SearchExhausted]);
        assert!(!answer.is_complete());
    }

    /// Every instance of a series contributes its own period, and adjacent ones coalesce.
    #[test]
    fn every_instance_of_a_series_contributes_its_own_period() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:series\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=HOURLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[
            (AT_NINE, AT_TEN),
            (AT_TEN, AT_ELEVEN),
            (AT_ELEVEN, AT_TWELVE),
        ]);
        let placements = [Placement {
            component: components[0],
            instances: &found,
            incomplete: None,
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        // Three consecutive hours of one type, which section 7.10 says to coalesce.
        assert_eq!(periods_of(&answer), [(AT_NINE, AT_TWELVE, BusyType::Busy)]);
        assert!(answer.is_complete());
    }

    /// A complete expansion can find zero instances within the window.
    #[test]
    fn a_series_with_no_instance_in_the_window_is_not_an_unaccounted_resource() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:elsewhere\r\n\
                   DTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\n\
                   RRULE:FREQ=YEARLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let placements = [Placement {
            component: components[0],
            instances: &[],
            incomplete: None,
        }];
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(query(WINDOW_START, WINDOW_END), &placements, &mut budget).unwrap();

        assert_eq!(answer.report().periods(), []);
        assert!(answer.is_complete());
    }

    /// An open-ended window clips at the bound it has and nowhere else.
    #[test]
    fn a_window_open_at_one_end_clips_only_at_the_other() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:open\r\n\
                   DTSTART:20251231T230000Z\r\nDTEND:20260101T010000Z\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        let document = parse(ics);
        let components = children(&document);
        let found = placed(&[(BEFORE_WINDOW, INSIDE_WINDOW)]);
        let placements = one_each(&components, &found);
        let open_ended = FreeBusyQuery {
            range: TimeRange::starting_at(Instant::from_unix_seconds(WINDOW_START)),
        };
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        let answer = free_busy(open_ended, &placements, &mut budget).unwrap();

        assert_eq!(
            periods_of(&answer),
            [(WINDOW_START, INSIDE_WINDOW, BusyType::Busy)]
        );
        assert_eq!(
            answer.report().window(),
            (Some(Instant::from_unix_seconds(WINDOW_START)), None)
        );
    }
}
