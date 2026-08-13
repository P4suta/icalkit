// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The UTC scalar the whole workspace shares.
//!
//! This type sits below the diagnostic vocabulary because a diagnostic may name an instant:
//! `ical-recur` and `ical-tz` report about occurrences that exist at no byte offset in any
//! file, and a diagnostic that can only carry a span has nothing to say about them. Putting
//! the scalar here rather than in `ical-tz` also keeps `ical-dav`, which needs it for
//! `time-range` filters and does not depend on `ical-tz`, off a dependency it has no other
//! reason to take.
//!
//! What an instant *means* under a zone belongs to `ical-tz`; converting one to civil fields
//! belongs to the model above this layer, which owns the civil types. Nothing here reads a
//! clock.

/// A point on the UTC timeline, counted in seconds from the Unix epoch.
///
/// Seconds rather than a finer unit because RFC 5545 has no sub-second value type: a
/// resolution the format cannot express is a resolution this type would only ever lose.
/// Leap seconds are not represented, matching the Unix convention every calendar producer
/// already writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    /// Seconds since 1970-01-01T00:00:00Z, negative before it.
    unix_seconds: i64,
}

impl Instant {
    /// The Unix epoch itself.
    pub const EPOCH: Self = Self { unix_seconds: 0 };

    /// The instant `seconds` after the Unix epoch.
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self {
            unix_seconds: seconds,
        }
    }

    /// Seconds since the Unix epoch.
    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.unix_seconds
    }

    /// The instant `seconds` later, or `None` when that instant is not representable.
    ///
    /// Checked rather than wrapping: an arithmetic wrap here would move an event by
    /// approximately 585 billion years and report success.
    #[must_use]
    pub const fn checked_add_seconds(self, seconds: i64) -> Option<Self> {
        match self.unix_seconds.checked_add(seconds) {
            Some(unix_seconds) => Some(Self { unix_seconds }),
            None => None,
        }
    }

    /// The number of seconds from `self` to `later`, or `None` when the difference is not
    /// representable.
    #[must_use]
    pub const fn checked_seconds_until(self, later: Self) -> Option<i64> {
        later.unix_seconds.checked_sub(self.unix_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::Instant;

    #[test]
    fn the_epoch_is_zero_and_ordering_follows_the_timeline() {
        assert_eq!(Instant::EPOCH.unix_seconds(), 0);
        assert!(Instant::from_unix_seconds(-1) < Instant::EPOCH);
    }

    #[test]
    fn overflow_is_none_rather_than_a_wrapped_instant() {
        assert_eq!(
            Instant::from_unix_seconds(i64::MAX).checked_add_seconds(1),
            None
        );
        assert_eq!(
            Instant::EPOCH
                .checked_add_seconds(60)
                .map(Instant::unix_seconds),
            Some(60)
        );
    }

    #[test]
    fn the_distance_between_two_instants_is_checked_too() {
        let epoch = Instant::EPOCH;
        assert_eq!(
            epoch.checked_seconds_until(Instant::from_unix_seconds(90)),
            Some(90)
        );
        assert_eq!(
            Instant::from_unix_seconds(i64::MIN)
                .checked_seconds_until(Instant::from_unix_seconds(i64::MAX)),
            None
        );
    }
}
