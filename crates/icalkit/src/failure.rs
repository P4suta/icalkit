// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter};

use crate::internal::core::{Diagnostic, Severity};

/// A stable machine-readable diagnostic identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IssueCode(&'static str);

impl IssueCode {
    /// Construct a code owned by icalkit.
    pub(crate) const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// The stable string spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Display for IssueCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// One validation note, warning, or error retained from an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    code: IssueCode,
    level: Level,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Note,
    Warning,
    Error,
}

impl Issue {
    pub(crate) fn from_diagnostic(diagnostic: Diagnostic) -> Self {
        let level = match diagnostic.severity() {
            Severity::Note => Level::Note,
            Severity::LimitReached => Level::Warning,
            Severity::Violation => Level::Error,
        };
        Self {
            code: IssueCode::new(diagnostic.code().as_str()),
            level,
        }
    }

    pub(crate) const fn error(code: &'static str) -> Self {
        Self {
            code: IssueCode::new(code),
            level: Level::Error,
        }
    }

    /// The stable code for this issue.
    #[must_use]
    pub const fn code(&self) -> IssueCode {
        self.code
    }

    /// Whether this issue prevents promotion to [`Calendar`](crate::Calendar).
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self.level, Level::Error)
    }

    /// Whether this issue reports bounded work that could not be completed.
    #[must_use]
    pub const fn is_warning(&self) -> bool {
        matches!(self.level, Level::Warning)
    }

    /// Whether this issue is informational and standards-compliant.
    #[must_use]
    pub const fn is_note(&self) -> bool {
        matches!(self.level, Level::Note)
    }
}

/// An operation that could not produce a validated result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    code: IssueCode,
    issues: Vec<Issue>,
}

impl Error {
    pub(crate) const fn new(code: &'static str, issues: Vec<Issue>) -> Self {
        Self {
            code: IssueCode::new(code),
            issues,
        }
    }

    pub(crate) fn single(code: &'static str) -> Self {
        Self::new(code, alloc::vec![Issue::error(code)])
    }

    /// The stable code for the operation failure.
    #[must_use]
    pub const fn code(&self) -> IssueCode {
        self.code
    }

    /// Diagnostics that explain the refusal.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code)
    }
}

impl core::error::Error for Error {}
