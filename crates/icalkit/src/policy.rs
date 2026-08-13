// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use ical_core::Limits;

/// Resource ceilings applied across a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePolicy {
    pub(crate) limits: Limits,
}

impl ResourcePolicy {
    /// Secure defaults suitable for untrusted calendar and DAV input.
    #[must_use]
    pub const fn secure() -> Self {
        Self {
            limits: Limits::DEFAULT,
        }
    }

    /// The same policy with a different aggregate input-octet ceiling.
    #[must_use]
    pub const fn with_max_input_bytes(self, bytes: u64) -> Self {
        Self {
            limits: self.limits.with_max_input_bytes(bytes),
        }
    }

    /// The configured aggregate input-octet ceiling.
    #[must_use]
    pub const fn max_input_bytes(self) -> u64 {
        self.limits.max_input_bytes()
    }
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self::secure()
    }
}
