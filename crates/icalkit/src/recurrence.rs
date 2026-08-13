// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded recurrence workflows over the public Jiff time boundary.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::internal::core::{Component, Diagnostic, Instant, Item, Meter};
use crate::internal::dav::TimeRange;
use crate::internal::query::{Budget, QueryError, Undecided, Zones, expand_component};
use crate::internal::recur::{
    OverrideSet, RecurrenceInput, RecurrenceRule, RecurrenceSearch, SearchCursor, SearchStep,
    ValueKind,
};

use crate::failure::Issue;
use crate::time::{Timestamp, ZoneAdapter};
use crate::{Calendar, Error, ResourcePolicy, Session};

/// A strictly parsed recurrence rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    inner: RecurrenceRule,
    issues: Vec<Issue>,
}

impl Rule {
    /// Parse and validate an RFC 5545 recurrence rule.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        let mut meter = Meter::new(policy.limits);
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let inner =
            crate::internal::recur::parse_recur(value.as_bytes(), &mut meter, &mut diagnostics)
                .map_err(|_| Error::single("icalkit.recurrence.invalid-rule"))?;
        let issues: Vec<Issue> = diagnostics
            .into_iter()
            .map(Issue::from_diagnostic)
            .collect();
        if issues
            .iter()
            .any(|issue| issue.is_error() || issue.is_warning())
        {
            return Err(Error::new("icalkit.recurrence.invalid-rule", issues));
        }
        Ok(Self { inner, issues })
    }

    /// Standards-compliant notes retained while parsing this rule.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// Search this rule over a mandatory half-open window.
    pub fn occurrences<'a>(
        &'a self,
        session: &'a mut Session<'_>,
        dtstart: Timestamp,
        window: Window,
    ) -> Result<Occurrences<'a>, Error> {
        self.start_search(session, dtstart, window, None)
    }

    /// Resume after the position captured from an earlier search.
    pub fn resume<'a>(
        &'a self,
        session: &'a mut Session<'_>,
        dtstart: Timestamp,
        window: Window,
        cursor: Cursor,
    ) -> Result<Occurrences<'a>, Error> {
        let CursorInner::Rule(cursor) = cursor.inner else {
            return Err(Error::single("icalkit.recurrence.cursor-mismatch"));
        };
        self.start_search(session, dtstart, window, Some(cursor))
    }

    fn start_search<'a>(
        &'a self,
        session: &'a mut Session<'_>,
        dtstart: Timestamp,
        window: Window,
        cursor: Option<SearchCursor>,
    ) -> Result<Occurrences<'a>, Error> {
        let start = instant(dtstart)
            .ok_or_else(|| Error::single("icalkit.recurrence.fractional-dtstart"))?;
        let input = RecurrenceInput::new(
            start,
            ValueKind::DateTime,
            Some(&self.inner),
            &[],
            &[],
            OverrideSet::empty(),
            &mut session.meter,
        )
        .map_err(|_| Error::single("icalkit.recurrence.invalid-input"))?;
        session.recurrence_diagnostics.clear();
        let search = match cursor {
            Some(cursor) => input.resume(
                cursor,
                window.inner,
                &mut session.meter,
                &mut session.recurrence_diagnostics,
            ),
            None => input.search(
                window.inner,
                &mut session.meter,
                &mut session.recurrence_diagnostics,
            ),
        };
        Ok(Occurrences {
            source: OccurrenceSource::Rule(Box::new(search)),
        })
    }
}

/// A mandatory half-open recurrence window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    start: Timestamp,
    end: Timestamp,
    inner: crate::internal::recur::Window,
}

impl Window {
    /// Construct a whole-second window, or return `None` for an empty or fractional one.
    #[must_use]
    pub fn new(start: Timestamp, end: Timestamp) -> Option<Self> {
        let inner = crate::internal::recur::Window::new(instant(start)?, instant(end)?)?;
        Some(Self { start, end, inner })
    }

    /// The included start.
    #[must_use]
    pub const fn start(self) -> Timestamp {
        self.start
    }

    /// The excluded end.
    #[must_use]
    pub const fn end(self) -> Timestamp {
        self.end
    }

    /// Whether a timestamp lies within this window.
    #[must_use]
    pub fn contains(self, timestamp: Timestamp) -> bool {
        self.start <= timestamp && timestamp < self.end
    }
}

/// One occurrence ordered by effective start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    key: Timestamp,
    start: Timestamp,
}

impl Occurrence {
    /// The base cadence key addressed by RECURRENCE-ID.
    #[must_use]
    pub const fn key(self) -> Timestamp {
        self.key
    }

    /// The effective start after recurrence-set processing.
    #[must_use]
    pub const fn start(self) -> Timestamp {
        self.start
    }
}

/// An opaque position in the recurrence algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    inner: CursorInner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorInner {
    Rule(SearchCursor),
    Calendar { position: usize },
}

/// A fallible lazy occurrence stream.
pub struct Occurrences<'a> {
    source: OccurrenceSource<'a>,
}

enum OccurrenceSource<'a> {
    Rule(Box<RecurrenceSearch<'a, Vec<Diagnostic>>>),
    Calendar {
        occurrences: Vec<Occurrence>,
        position: usize,
        terminal_error: Option<&'static str>,
    },
}

impl Occurrences<'_> {
    /// Pull one occurrence. Budget exhaustion is an error and never an end marker.
    pub fn try_next(&mut self) -> Result<Option<Occurrence>, Error> {
        match &mut self.source {
            OccurrenceSource::Rule(search) => match search.next() {
                Some(SearchStep::Occurrence(occurrence)) => {
                    let key = timestamp(occurrence.key())?;
                    let start = timestamp(occurrence.start())?;
                    Ok(Some(Occurrence { key, start }))
                },
                Some(SearchStep::BudgetExhausted(_)) => {
                    Err(Error::single("icalkit.recurrence.budget-exhausted"))
                },
                None => Ok(None),
            },
            OccurrenceSource::Calendar {
                occurrences,
                position,
                terminal_error,
            } => {
                if let Some(occurrence) = occurrences.get(*position).copied() {
                    *position = position.saturating_add(1);
                    return Ok(Some(occurrence));
                }
                if let Some(code) = terminal_error.take() {
                    return Err(Error::single(code));
                }
                Ok(None)
            },
        }
    }

    /// Capture an opaque resumable position.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        let inner = match &self.source {
            OccurrenceSource::Rule(search) => CursorInner::Rule(search.cursor()),
            OccurrenceSource::Calendar { position, .. } => CursorInner::Calendar {
                position: *position,
            },
        };
        Cursor { inner }
    }
}

impl core::fmt::Debug for Occurrences<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Occurrences")
            .field("cursor", &self.cursor())
            .finish_non_exhaustive()
    }
}

pub(crate) fn calendar_occurrences<'a>(
    calendar: &'a Calendar,
    session: &'a mut Session<'_>,
    uid: &str,
    window: Window,
) -> Result<Occurrences<'a>, Error> {
    start_calendar_occurrences(calendar, session, uid, window, None)
}

pub(crate) fn resume_calendar_occurrences<'a>(
    calendar: &'a Calendar,
    session: &'a mut Session<'_>,
    uid: &str,
    window: Window,
    cursor: Cursor,
) -> Result<Occurrences<'a>, Error> {
    let CursorInner::Calendar { position } = cursor.inner else {
        return Err(Error::single("icalkit.recurrence.cursor-mismatch"));
    };
    start_calendar_occurrences(calendar, session, uid, window, Some(position))
}

fn start_calendar_occurrences<'a>(
    calendar: &'a Calendar,
    session: &'a mut Session<'_>,
    uid: &str,
    window: Window,
    position: Option<usize>,
) -> Result<Occurrences<'a>, Error> {
    let (master, siblings) = recurrence_master(calendar, uid.as_bytes())?;
    let range = TimeRange::new(Some(window.inner.start()), Some(window.inner.end()))
        .map_err(|_| Error::single("icalkit.recurrence.invalid-window"))?;
    let engine = session.engine;
    let source = ZoneAdapter::new(engine.zone_database());
    let zones = Zones::new(&source);
    let mut budget = Budget::new(engine.policy.limits, &mut session.meter);
    let expansion = expand_component(master, siblings, range, zones, &mut budget)
        .map_err(recurrence_query_error)?;
    let terminal_error = expansion.incomplete().map(recurrence_incomplete);
    let mut occurrences: Vec<Occurrence> = expansion
        .instances()
        .iter()
        .map(|instance| {
            Ok(Occurrence {
                key: timestamp(instance.recurrence_id())?,
                start: timestamp(instance.start())?,
            })
        })
        .collect::<Result<_, Error>>()?;
    occurrences.sort_by_key(|occurrence| (occurrence.start, occurrence.key));
    let position = position.unwrap_or(0);
    if position > occurrences.len() {
        return Err(Error::single("icalkit.recurrence.cursor-mismatch"));
    }
    Ok(Occurrences {
        source: OccurrenceSource::Calendar {
            occurrences,
            position,
            terminal_error,
        },
    })
}

fn recurrence_master<'a>(
    calendar: &'a Calendar,
    uid: &[u8],
) -> Result<(&'a Component, &'a [Item]), Error> {
    let root = calendar
        .document
        .components()
        .next()
        .ok_or_else(|| Error::single("icalkit.recurrence.component-not-found"))?;
    let mut masters = root
        .items()
        .iter()
        .filter_map(Item::as_component)
        .filter(|component| {
            component
                .properties()
                .find(|property| property.is_named(b"UID"))
                .is_some_and(|property| property.value_text().as_bytes() == uid)
                && !component
                    .properties()
                    .any(|property| property.is_named(b"RECURRENCE-ID"))
        });
    let master = masters
        .next()
        .ok_or_else(|| Error::single("icalkit.recurrence.component-not-found"))?;
    if masters.next().is_some() {
        return Err(Error::single("icalkit.recurrence.component-ambiguous"));
    }
    Ok((master, root.items()))
}

fn recurrence_query_error(error: QueryError) -> Error {
    match error {
        QueryError::Limit(_) => Error::single("icalkit.recurrence.budget-exhausted"),
        _ => Error::single("icalkit.recurrence.invalid-calendar"),
    }
}

const fn recurrence_incomplete(reason: Undecided) -> &'static str {
    match reason {
        Undecided::SearchExhausted => "icalkit.recurrence.budget-exhausted",
        Undecided::ZoneUnstated | Undecided::ZoneUnknown | Undecided::ZoneAmbiguous => {
            "icalkit.recurrence.zone-unresolved"
        },
        Undecided::ValueUnreadable | Undecided::OverlapUndefined => {
            "icalkit.recurrence.invalid-calendar"
        },
    }
}

fn instant(timestamp: Timestamp) -> Option<Instant> {
    (timestamp.subsec_nanosecond() == 0).then(|| Instant::from_unix_seconds(timestamp.as_second()))
}

fn timestamp(instant: Instant) -> Result<Timestamp, Error> {
    Timestamp::new(instant.unix_seconds(), 0)
        .map_err(|_| Error::single("icalkit.recurrence.timestamp-out-of-range"))
}
