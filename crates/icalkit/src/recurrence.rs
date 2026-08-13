// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded recurrence workflow vocabulary.

pub use ical_recur::{Occurrence, RecurrenceRule as Rule, SearchCursor as Cursor, Window};

/// A fallible occurrence stream whose terminal budget state cannot be discarded as `None`.
#[derive(Debug)]
pub struct Occurrences {
    finished: bool,
    failure: Option<crate::Error>,
}

impl Occurrences {
    /// An empty, complete occurrence stream.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            finished: false,
            failure: None,
        }
    }

    /// Pull the next occurrence.
    pub fn try_next(&mut self) -> Result<Option<Occurrence<'static>>, crate::Error> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.finished = true;
        Ok(None)
    }
}
