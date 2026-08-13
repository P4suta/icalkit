// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The one collection type, whose growth is a charged door rather than a public field.
//!
//! Every list in this crate is a [`Bounded`]: the responses of a multistatus, the `href`s of a
//! multiget, the properties of a `propstat`, the preconditions of an error body. A public
//! `Vec` field would be a way around the charge that the whole limits story rests on, and the
//! argument for that does not weaken below the top-level collection — a single response
//! carrying a hundred thousand properties crosses no bound the body has.
//!
//! The cap comes from the caller's `Limits` at construction and the dimension travels with it,
//! so a refusal names the number the caller can raise rather than saying that some bound
//! somewhere was crossed.

use alloc::vec::Vec;

use crate::internal::core::{LimitExceeded, Meter};

use crate::internal::dav::failure::DavError;

/// A list with a cap it was built with and a charge on every push.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bounded<T> {
    /// The items. Private: a handed-out `Vec` is a door with no check in front of it.
    items: Vec<T>,
    /// The most items this list may hold.
    cap: usize,
    /// The dimension a refusal names.
    dimension: LimitExceeded,
}

impl<T> Bounded<T> {
    /// An empty list that will refuse the item past `cap`, naming `dimension` when it does.
    #[must_use]
    pub const fn with_cap(cap: usize, dimension: LimitExceeded) -> Self {
        Self {
            items: Vec::new(),
            cap,
            dimension,
        }
    }

    /// Append `item`, charging the caller's ledger for what the list now retains.
    ///
    /// Two bounds cross here, which is the shape `docs/adr/0010` argues for everywhere: the
    /// cap bounds this one collection and the ledger bounds the run, so five thousand
    /// individually bounded bodies are bounded in aggregate too.
    ///
    /// The charge is what the collection itself retains — one item's own footprint. The octets
    /// an item points at are charged where they are read or built, which is the only place
    /// their length is known.
    pub fn push(&mut self, item: T, meter: &mut Meter) -> Result<(), DavError> {
        if self.items.len() >= self.cap {
            return Err(DavError::Limit(self.dimension));
        }
        meter.try_charge_bytes(u64::try_from(size_of::<T>()).unwrap_or(u64::MAX))?;
        self.items
            .try_reserve(1)
            .map_err(|_| DavError::Limit(LimitExceeded::Budget))?;
        self.items.push(item);
        Ok(())
    }

    /// The items, as a slice. Nothing hands out the `Vec`.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// How many items the list holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The most items the list may hold.
    #[must_use]
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// Whether the next push will be refused.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.items.len() >= self.cap
    }

    /// The dimension a refusal from this list names.
    #[must_use]
    pub const fn dimension(&self) -> LimitExceeded {
        self.dimension
    }

    /// The items, borrowed one at a time.
    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.items.iter()
    }
}

impl<'a, T> IntoIterator for &'a Bounded<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{LimitExceeded, Limits, Meter};

    use super::Bounded;
    use crate::internal::dav::failure::DavError;

    #[test]
    fn the_item_past_the_cap_is_refused_and_names_its_dimension() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut list: Bounded<u32> = Bounded::with_cap(2, LimitExceeded::Properties);
        list.push(1, &mut meter).unwrap();
        list.push(2, &mut meter).unwrap();
        assert_eq!(
            list.push(3, &mut meter),
            Err(DavError::Limit(LimitExceeded::Properties))
        );
        assert_eq!(list.as_slice(), [1, 2]);
        assert!(list.is_full());
    }

    #[test]
    fn a_push_charges_the_shared_ledger_so_many_lists_are_bounded_together() {
        // The budget admits three `u32`s across every list this ledger serves, and the caps
        // would admit sixteen. Aggregate is what binds, which is the point of one meter.
        let mut meter = Meter::with_budget(Limits::DEFAULT, 12);
        let mut first: Bounded<u32> = Bounded::with_cap(8, LimitExceeded::Responses);
        let mut second: Bounded<u32> = Bounded::with_cap(8, LimitExceeded::Responses);
        first.push(1, &mut meter).unwrap();
        first.push(2, &mut meter).unwrap();
        second.push(3, &mut meter).unwrap();
        assert_eq!(
            second.push(4, &mut meter),
            Err(DavError::Limit(LimitExceeded::Budget))
        );
    }
}
