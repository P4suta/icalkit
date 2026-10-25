// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

pub(super) const fn released_count(current: u64, bytes: u64) -> u64 {
    current.saturating_sub(bytes)
}

#[cfg(test)]
mod tests {
    use super::released_count;

    #[test]
    fn releases_preserve_zero_exact_and_saturated_boundaries() {
        for (current, bytes, expected) in [
            (0, 0, 0),
            (0, 1, 0),
            (1, 0, 1),
            (1, 1, 0),
            (1, 2, 0),
            (u64::MAX, 0, u64::MAX),
            (u64::MAX, u64::MAX, 0),
            (u64::MAX, u64::MAX - 1, 1),
        ] {
            assert_eq!(released_count(current, bytes), expected);
        }
    }
}

#[cfg(kani)]
mod proofs {
    use super::released_count;

    #[kani::proof]
    #[kani::unwind(1)]
    fn released_count_is_saturating() {
        let current: u64 = kani::any();
        let bytes: u64 = kani::any();
        let remaining = released_count(current, bytes);
        assert_eq!(remaining, current.checked_sub(bytes).unwrap_or_default());
        assert!(remaining <= current);
        assert_eq!(remaining == 0, bytes >= current);
        kani::cover!(bytes > current);
        kani::cover!(remaining > 0);
    }

    #[kani::proof]
    #[kani::unwind(1)]
    fn counterexample_release_underflows() {
        assert_eq!(released_count(0, 1), u64::MAX);
    }
}
