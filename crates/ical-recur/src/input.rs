// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a caller offers a search: the rule, the additions, the exclusions, and the overrides.
//!
//! An override is a **diff**, never a replacement component. RFC 5545 section 3.8.4.4 places
//! no restriction on which properties a `RANGE=THISANDFUTURE` override may change, so an
//! organizer moving a recurring meeting to a new room without moving its time is ordinary
//! input — and a scalar time delta, which is the shape most implementations reach for, cannot
//! represent it. That is data loss on a file half the clients in the world can produce, and it
//! is why [`PropertyDiff`] exists (`docs/adr/0002`).
//!
//! Anchors compose. A series edited "this and following" twice — a March anchor changing
//! `LOCATION`, a June anchor changing `SUMMARY` and restating nothing else — must not revert
//! the March change in July, so [`OverrideSet::anchors_before`] yields *every* anchor at or
//! before a key rather than the nearest one. Omission means no opinion, never revert-to-base.
//!
//! Everything here is borrowed and `Copy`. A caller holding a parsed document already owns
//! these slices, and a search that copied them would be hiding an allocation inside a call
//! advertised as a linear merge. Sortedness is required rather than repaired for the same
//! reason: a constructor that quietly sorted would hide both the allocation and an
//! `O(n log n)`.
//!
//! Each list is charged to the caller's meter as it is admitted. `docs/adr/0010`'s own
//! argument — bounded per call, unbounded in aggregate — applies to these three lists exactly
//! as it applies to a parse, and a bound nobody charges is decoration.

use ical_core::{Instant, LimitExceeded, Meter, Property, PropertyId};

use crate::rule::{RecurrenceRule, RuleLimit, ValueKind};

/// One property an override states something about.
///
/// Two arms because a diff can say two things, and RFC 5545 has syntax for only one of them.
/// [`PropertyChange::Set`] carries the property the override wrote. [`PropertyChange::Removed`]
/// carries a name and nothing else, for a caller that determined the override dropped a
/// property the base component had — a determination this crate does not make and does not
/// forbid.
#[derive(Clone, Copy, Debug)]
pub enum PropertyChange<'a> {
    /// The override states this property.
    Set(&'a Property),
    /// The override removed the property with this name.
    Removed(&'a [u8]),
}

impl<'a> PropertyChange<'a> {
    /// The name of the property this change concerns.
    #[must_use]
    pub fn name(self) -> &'a [u8] {
        match self {
            Self::Set(property) => property.name().as_bytes(),
            Self::Removed(name) => name,
        }
    }

    /// Whether this change concerns the property `id` names.
    #[must_use]
    pub fn has_id(self, id: &PropertyId) -> bool {
        id.matches(self.name())
    }
}

/// What one override changed, and nothing about what it left alone.
///
/// Borrowed rather than owned so that composing three anchors costs three slice walks and no
/// allocation at all. The order of `changes` is the order the caller supplied; a later change
/// with the same name wins, which is the same rule anchor composition uses one level up.
#[derive(Clone, Copy, Debug)]
pub struct PropertyDiff<'a> {
    /// The changes, in the order the caller stated them.
    changes: &'a [PropertyChange<'a>],
}

impl<'a> PropertyDiff<'a> {
    /// A diff over `changes`.
    #[must_use]
    pub const fn new(changes: &'a [PropertyChange<'a>]) -> Self {
        Self { changes }
    }

    /// A diff that changes nothing, which is what a pure time shift is.
    #[must_use]
    pub const fn empty() -> Self {
        Self { changes: &[] }
    }

    /// The changes, in the order the caller stated them.
    #[must_use]
    pub const fn changes(self) -> &'a [PropertyChange<'a>] {
        self.changes
    }

    /// The last change this diff states about `id`, if it states one.
    ///
    /// Last rather than first, so that a caller restating a property within one diff gets the
    /// same "later wins" answer it gets from a later anchor.
    #[must_use]
    pub fn get(self, id: &PropertyId) -> Option<&'a PropertyChange<'a>> {
        self.changes.iter().rev().find(|change| change.has_id(id))
    }
}

/// How far forward an override reaches, from RFC 5545 section 3.2.13's `RANGE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OverrideRange {
    /// The override modifies the one instance its `RECURRENCE-ID` names.
    ThisOnly,
    /// The override modifies that instance and every later one.
    ThisAndFuture,
}

/// One `RECURRENCE-ID` override: which instance, how far it reaches, and what it changed.
#[derive(Clone, Copy, Debug)]
pub struct Override<'a> {
    /// The cadence key this override addresses — the instant the base rule generated.
    recurrence_id: Instant,
    /// Whether it reaches later instances too.
    range: OverrideRange,
    /// Where the instance moved to, absent when the override moved nothing.
    moved_to: Option<Instant>,
    /// What it changed.
    diff: PropertyDiff<'a>,
}

impl<'a> Override<'a> {
    /// An override addressing `recurrence_id`.
    #[must_use]
    pub const fn new(
        recurrence_id: Instant,
        range: OverrideRange,
        moved_to: Option<Instant>,
        diff: PropertyDiff<'a>,
    ) -> Self {
        Self {
            recurrence_id,
            range,
            moved_to,
            diff,
        }
    }

    /// The cadence key this override addresses.
    #[must_use]
    pub const fn recurrence_id(self) -> Instant {
        self.recurrence_id
    }

    /// How far forward it reaches.
    #[must_use]
    pub const fn range(self) -> OverrideRange {
        self.range
    }

    /// Where the instance moved to, absent when it did not move.
    #[must_use]
    pub const fn moved_to(self) -> Option<Instant> {
        self.moved_to
    }

    /// What it changed.
    #[must_use]
    pub const fn diff(self) -> PropertyDiff<'a> {
        self.diff
    }

    /// Whether this override reaches later instances.
    #[must_use]
    pub const fn is_anchor(self) -> bool {
        matches!(self.range, OverrideRange::ThisAndFuture)
    }

    /// The time shift this override implies, derived rather than stored.
    ///
    /// `moved_to − recurrence_id`, checked. The shift is a *consequence* of the diff and never
    /// the diff itself: an override that changes only `LOCATION` has no shift, and one that
    /// changes only the time has a shift and an empty diff. Storing the number instead of
    /// deriving it is exactly the scalar-delta design `docs/adr/0002` overruled.
    #[must_use]
    pub fn shift_seconds(self) -> Option<i64> {
        self.recurrence_id
            .checked_seconds_until(self.moved_to?)
            .filter(|shift| *shift != 0)
    }
}

/// Which caller-supplied list an input complaint is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputList {
    /// The `RDATE` instants.
    Rdate,
    /// The `EXDATE` instants.
    Exdate,
    /// The `RECURRENCE-ID` overrides.
    Override,
}

/// Why a search could not be assembled from what the caller offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum InputError {
    /// A list was not in ascending order, so the merge's advertised linear cost is not true.
    NotAscending(InputList),
    /// A list named one instant twice, which gives two entries one undefined precedence.
    Duplicated(InputList),
    /// A list was longer than the caller's own policy admits.
    TooMany(InputList, LimitExceeded),
}

impl InputError {
    /// Which list the complaint is about.
    #[must_use]
    pub const fn list(self) -> InputList {
        match self {
            Self::NotAscending(list) | Self::Duplicated(list) | Self::TooMany(list, _) => list,
        }
    }
}

/// The `RECURRENCE-ID` overrides for one series, exact matches and anchors in one slice.
///
/// One slice rather than two. Two parallel slices would let a caller desynchronize them with
/// nothing able to check it, and the same override is an exact match for its own key and an
/// anchor for every later one, so splitting them would also duplicate half the entries.
#[derive(Clone, Copy, Debug)]
pub struct OverrideSet<'a> {
    /// The overrides, strictly ascending by `RECURRENCE-ID`.
    entries: &'a [Override<'a>],
}

impl<'a> OverrideSet<'a> {
    /// The overrides for a series that has none.
    #[must_use]
    pub const fn empty() -> Self {
        Self { entries: &[] }
    }

    /// A set over `entries`, charged to `meter`.
    ///
    /// Strictly ascending `RECURRENCE-ID`s are required: two overrides claiming one instant
    /// have no defined precedence, and guessing one silently is the failure this crate exists
    /// to prevent.
    pub fn new(entries: &'a [Override<'a>], meter: &mut Meter) -> Result<Self, InputError> {
        let count = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        meter
            .try_charge_override_entries(count)
            .map_err(|breach| InputError::TooMany(InputList::Override, breach))?;
        let keys = entries.iter().map(|entry| entry.recurrence_id());
        check_strictly_ascending(keys, InputList::Override)?;
        Ok(Self { entries })
    }

    /// The overrides, strictly ascending by `RECURRENCE-ID`.
    #[must_use]
    pub const fn entries(self) -> &'a [Override<'a>] {
        self.entries
    }

    /// The override addressing exactly `key`, if there is one.
    #[must_use]
    pub fn exact_match(self, key: Instant) -> Option<&'a Override<'a>> {
        self.entries
            .binary_search_by(|entry| entry.recurrence_id().cmp(&key))
            .ok()
            .and_then(|position| self.entries.get(position))
    }

    /// Every `RANGE=THISANDFUTURE` anchor at or before `key`, oldest first.
    ///
    /// Every, not the nearest. A series edited "this and following" twice has two anchors in
    /// force, and the later edit is under no obligation to restate what the earlier changed.
    #[must_use]
    pub const fn anchors_before(self, key: Instant) -> AppliedDiffs<'a> {
        AppliedDiffs {
            entries: self.entries,
            position: 0,
            key,
        }
    }
}

/// The anchors in force at one cadence key, oldest first.
///
/// `Clone` but deliberately not `Copy`: an iterator that copies on use is an iterator a caller
/// can advance twice by accident, and half the point of this type is that a caller walks the
/// composed anchors exactly once per occurrence.
#[derive(Clone, Debug)]
pub struct AppliedDiffs<'a> {
    /// The whole override table.
    entries: &'a [Override<'a>],
    /// How far through it this iterator has walked.
    position: usize,
    /// The key the anchors are being collected for.
    key: Instant,
}

impl<'a> Iterator for AppliedDiffs<'a> {
    type Item = &'a Override<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(entry) = self.entries.get(self.position) {
            self.position = self.position.saturating_add(1);
            if entry.recurrence_id() > self.key {
                // The table is ascending, so the first key past the mark ends the walk.
                return None;
            }
            if entry.is_anchor() {
                return Some(entry);
            }
        }
        None
    }
}

impl core::iter::FusedIterator for AppliedDiffs<'_> {}

/// Everything one component says about which occurrences it has.
#[derive(Clone, Copy, Debug)]
pub struct RecurrenceInput<'a> {
    /// The first instance, and the anchor every `BYxxx` default is taken from.
    dtstart: Instant,
    /// Whether `DTSTART` was written as a `DATE` or a `DATE-TIME`.
    dtstart_kind: ValueKind,
    /// The rule, absent for a component that is only `RDATE`s.
    rule: Option<&'a RecurrenceRule>,
    /// Instants `RDATE` adds, ascending.
    additions: &'a [Instant],
    /// Instants `EXDATE` removes, ascending.
    exclusions: &'a [Instant],
    /// The `RECURRENCE-ID` overrides.
    overrides: OverrideSet<'a>,
}

impl<'a> RecurrenceInput<'a> {
    /// Assemble an input, checking each list and charging it to `meter`.
    ///
    /// Six arguments rather than a builder because none of them has a default: a component
    /// either has an `RDATE` list or has an empty one, and there is no third answer for a
    /// builder to express.
    pub fn new(
        dtstart: Instant,
        dtstart_kind: ValueKind,
        rule: Option<&'a RecurrenceRule>,
        additions: &'a [Instant],
        exclusions: &'a [Instant],
        overrides: OverrideSet<'a>,
        meter: &mut Meter,
    ) -> Result<Self, InputError> {
        charge_instants(additions, InputList::Rdate, meter)?;
        charge_instants(exclusions, InputList::Exdate, meter)?;
        check_strictly_ascending(additions.iter().copied(), InputList::Rdate)?;
        check_strictly_ascending(exclusions.iter().copied(), InputList::Exdate)?;
        Ok(Self {
            dtstart,
            dtstart_kind,
            rule,
            additions,
            exclusions,
            overrides,
        })
    }

    /// The first instance.
    #[must_use]
    pub const fn dtstart(self) -> Instant {
        self.dtstart
    }

    /// Whether `DTSTART` was written as a `DATE` or a `DATE-TIME`.
    #[must_use]
    pub const fn dtstart_kind(self) -> ValueKind {
        self.dtstart_kind
    }

    /// The rule, absent for a component that is only `RDATE`s.
    #[must_use]
    pub const fn rule(self) -> Option<&'a RecurrenceRule> {
        self.rule
    }

    /// The instants `RDATE` adds.
    #[must_use]
    pub const fn rdates(self) -> &'a [Instant] {
        self.additions
    }

    /// The instants `EXDATE` removes.
    #[must_use]
    pub const fn exdates(self) -> &'a [Instant] {
        self.exclusions
    }

    /// The `RECURRENCE-ID` overrides.
    #[must_use]
    pub const fn overrides(self) -> OverrideSet<'a> {
        self.overrides
    }

    /// Whether `UNTIL` and `DTSTART` agree about `DATE` versus `DATE-TIME`.
    ///
    /// A predicate rather than a constructor check. Disagreement violates RFC 5545 section
    /// 3.3.10 and is emitted by half the clients in the corpus — Google has shipped a floating
    /// `UNTIL` against a zoned `DTSTART` — and refusing the component over it would discard a
    /// file, which `docs/adr/0001` forbids. A rule with no `UNTIL` agrees vacuously.
    #[must_use]
    pub fn until_value_type_agrees(self) -> bool {
        match self.rule.map(RecurrenceRule::limit) {
            Some(RuleLimit::Until { value_kind, .. }) => value_kind == self.dtstart_kind,
            _ => true,
        }
    }
}

/// Charge one caller-supplied instant list against the dimension its name owns.
fn charge_instants(
    instants: &[Instant],
    list: InputList,
    meter: &mut Meter,
) -> Result<(), InputError> {
    let count = u32::try_from(instants.len()).unwrap_or(u32::MAX);
    let charged = match list {
        InputList::Rdate => meter.try_charge_rdate_entries(count),
        InputList::Exdate => meter.try_charge_exdate_entries(count),
        InputList::Override => meter.try_charge_override_entries(count),
    };
    charged.map_err(|breach| InputError::TooMany(list, breach))
}

/// Whether `keys` ascends with no repeats, saying which failure it was.
///
/// One walk answering both questions, because "not sorted" and "sorted with a duplicate" are
/// detected by the same comparison and reporting them from two passes would let the two
/// disagree about a list that is both.
fn check_strictly_ascending<I>(keys: I, list: InputList) -> Result<(), InputError>
where
    I: IntoIterator<Item = Instant>,
{
    let mut previous: Option<Instant> = None;
    for key in keys {
        if let Some(earlier) = previous {
            if key == earlier {
                return Err(InputError::Duplicated(list));
            }
            if key < earlier {
                return Err(InputError::NotAscending(list));
            }
        }
        previous = Some(key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ical_core::{Instant, Limits, Meter};

    use super::{
        InputError, InputList, Override, OverrideRange, OverrideSet, PropertyDiff, RecurrenceInput,
    };
    use crate::rule::ValueKind;

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    fn anchor(seconds: i64, range: OverrideRange) -> Override<'static> {
        Override::new(at(seconds), range, None, PropertyDiff::empty())
    }

    #[test]
    fn an_unsorted_or_repeated_list_is_refused_and_says_which() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let descending = [at(20), at(10)];
        assert_eq!(
            RecurrenceInput::new(
                at(0),
                ValueKind::DateTime,
                None,
                &descending,
                &[],
                OverrideSet::empty(),
                &mut meter,
            )
            .map(|_| ()),
            Err(InputError::NotAscending(InputList::Rdate))
        );

        let repeated = [at(10), at(10)];
        let refused = RecurrenceInput::new(
            at(0),
            ValueKind::DateTime,
            None,
            &[],
            &repeated,
            OverrideSet::empty(),
            &mut meter,
        )
        .map(|_| ());
        assert_eq!(refused, Err(InputError::Duplicated(InputList::Exdate)));
    }

    #[test]
    fn a_list_longer_than_the_policy_admits_is_refused_under_its_own_dimension() {
        let limits = Limits::DEFAULT.with_rdate_entries(1);
        let mut meter = Meter::new(limits);
        let two = [at(10), at(20)];
        let refused = RecurrenceInput::new(
            at(0),
            ValueKind::DateTime,
            None,
            &two,
            &[],
            OverrideSet::empty(),
            &mut meter,
        )
        .map(|_| ());
        assert_eq!(
            refused.map_err(InputError::list),
            Err(InputList::Rdate),
            "the dimension that ran out is the one the list is named for"
        );
    }

    /// Every anchor at or before the key, and no anchor after it.
    #[test]
    fn anchors_compose_oldest_first_rather_than_nearest_only() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let entries = [
            anchor(100, OverrideRange::ThisAndFuture),
            anchor(200, OverrideRange::ThisOnly),
            anchor(300, OverrideRange::ThisAndFuture),
            anchor(400, OverrideRange::ThisAndFuture),
        ];
        let set = OverrideSet::new(&entries, &mut meter).unwrap();
        let keys: alloc::vec::Vec<i64> = set
            .anchors_before(at(300))
            .map(|entry| entry.recurrence_id().unix_seconds())
            .collect();
        assert_eq!(keys, alloc::vec![100, 300]);
        assert!(set.exact_match(at(200)).is_some());
        assert!(set.exact_match(at(250)).is_none());
    }

    #[test]
    fn a_shift_is_derived_from_the_move_and_absent_when_nothing_moved() {
        let moved = Override::new(
            at(100),
            OverrideRange::ThisAndFuture,
            Some(at(400)),
            PropertyDiff::empty(),
        );
        assert_eq!(moved.shift_seconds(), Some(300));
        let relocated = anchor(100, OverrideRange::ThisAndFuture);
        assert_eq!(
            relocated.shift_seconds(),
            None,
            "an override that changes only a property moves nothing"
        );
    }
}
