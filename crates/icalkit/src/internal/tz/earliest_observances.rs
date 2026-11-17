// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stage the earliest admissible observances and one overflow witness.

use alloc::collections::BinaryHeap;

use crate::internal::core::Meter;
use crate::internal::tz::model::{Observance, OrderedObservance};

pub(super) struct EarliestObservances {
    entries: BinaryHeap<OrderedObservance>,
    capacity: usize,
}

impl EarliestObservances {
    pub(super) fn new(meter: &Meter) -> Self {
        let remaining = meter
            .limits()
            .max_vtimezone_observances()
            .saturating_sub(meter.vtimezone_observances());
        Self {
            entries: BinaryHeap::new(),
            capacity: usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .saturating_add(1),
        }
    }

    pub(super) fn push(&mut self, observance: Observance) {
        let offered = OrderedObservance::new(observance);
        if self.entries.len() < self.capacity {
            self.entries.push(offered);
        } else if let Some(mut latest) = self.entries.peek_mut()
            && offered < *latest
        {
            *latest = offered;
        }
    }

    pub(super) const fn capacity(&self) -> usize {
        self.capacity
    }

    pub(super) fn into_observances(self) -> impl Iterator<Item = Observance> {
        self.entries
            .into_sorted_vec()
            .into_iter()
            .map(OrderedObservance::into_observance)
    }
}

impl Extend<Observance> for EarliestObservances {
    fn extend<T: IntoIterator<Item = Observance>>(&mut self, observances: T) {
        for observance in observances {
            self.push(observance);
        }
    }
}
