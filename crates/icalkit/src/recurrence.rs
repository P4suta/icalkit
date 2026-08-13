// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded recurrence workflows over the public Jiff time boundary.

use alloc::vec::Vec;

use crate::internal::recur::{
    OverrideSet, RecurrenceInput, RecurrenceRule, RecurrenceSearch, SearchCursor, SearchStep,
    ValueKind,
};
use ical_core::{Diagnostic, Instant, Meter};

use crate::failure::Issue;
use crate::time::Timestamp;
use crate::{Error, ResourcePolicy, Session};

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
        self.start_search(session, dtstart, window, Some(cursor.inner))
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
        Ok(Occurrences { search })
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
    inner: SearchCursor,
}

/// A fallible lazy occurrence stream.
pub struct Occurrences<'a> {
    search: RecurrenceSearch<'a, Vec<Diagnostic>>,
}

impl Occurrences<'_> {
    /// Pull one occurrence. Budget exhaustion is an error and never an end marker.
    pub fn try_next(&mut self) -> Result<Option<Occurrence>, Error> {
        match self.search.next() {
            Some(SearchStep::Occurrence(occurrence)) => {
                let key = timestamp(occurrence.key())?;
                let start = timestamp(occurrence.start())?;
                Ok(Some(Occurrence { key, start }))
            },
            Some(SearchStep::BudgetExhausted(_)) => {
                Err(Error::single("icalkit.recurrence.budget-exhausted"))
            },
            None => Ok(None),
        }
    }

    /// Capture an opaque resumable position.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor {
            inner: self.search.cursor(),
        }
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

fn instant(timestamp: Timestamp) -> Option<Instant> {
    (timestamp.subsec_nanosecond() == 0).then(|| Instant::from_unix_seconds(timestamp.as_second()))
}

fn timestamp(instant: Instant) -> Result<Timestamp, Error> {
    Timestamp::new(instant.unix_seconds(), 0)
        .map_err(|_| Error::single("icalkit.recurrence.timestamp-out-of-range"))
}
