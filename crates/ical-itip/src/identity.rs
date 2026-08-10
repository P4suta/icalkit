// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a message is about, and which version of it.
//!
//! Specification: RFC 5545 section 3.8.4.7 (`UID`), section 3.8.4.4 (`RECURRENCE-ID`),
//! section 3.2.13 (`RANGE`), section 3.8.7.4 (`SEQUENCE`), section 3.8.7.2 (`DTSTAMP`); RFC
//! 5546 section 2.1.4 and section 2.1.5 for what the last two decide between them.
//!
//! Identity is `UID` plus `RECURRENCE-ID` and both halves are attackable, so both are types
//! here rather than fields somewhere. The version is `SEQUENCE` plus `DTSTAMP`, and it is the
//! whole of this protocol's replay defense.
//!
//! # The repeated hour
//!
//! M2 left a question this module answers. `ical_tz::seam` walks a series on its own wall
//! clock projected onto UTC, so the two halves of the hour a zone repeats are **one cadence
//! key**: a `REPLY` naming `20261101T063000Z` and one naming `20261101T053000Z` in
//! `America/New_York` are two real instants and one key, and `ical_recur::OverrideSet` admits
//! both while `collisions` counts what was shadowed (ADR-0011 amendment 3).
//!
//! Bounding the damage is not the same as knowing which meeting a message is about. So an
//! instance identity here is a key **and a side**: [`FoldSide`] says which of a repeated wall
//! clock's two instants is meant, or says that nothing resolved it. Comparison is then
//! three-valued — [`InstanceMatch::Ambiguous`] is a real answer, not a `false` dressed up —
//! and the rule the gate applies is that **an ambiguous match is not a match**. A message
//! whose instance cannot be told from its neighbor is denied rather than applied to a guess,
//! because a guess cancels somebody else's meeting.
//!
//! Deriving a side needs a zone and this crate resolves none: [`FoldSide::from_resolution`]
//! takes the [`LocalResolution`] a caller already holds from `ical-tz`. A caller with no zone
//! gets [`FoldSide::Unresolved`] and the conservative answer that follows from it.

use ical_core::{Instant, RawText};
use ical_recur::OverrideRange;
use ical_tz::LocalResolution;

/// One component's `UID`, compared as RFC 5545 section 3.8.4.7 says to.
///
/// Octet-exact, and deliberately so. A `UID` is opaque text with no defined case folding and
/// no defined whitespace stripping, so two identifiers differing by either are two
/// identifiers. The cost lands on a producer that round-trips a `UID` through a system which
/// trims it: its message will look like it is about a different event and be refused. The
/// other direction — folding, so that two events look like one — is how a `CANCEL` for one
/// meeting cancels another, and it is not available at any price.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uid(RawText);

impl Uid {
    /// The identifier `bytes` spells.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self(RawText::from_bytes(bytes))
    }

    /// The identifier's octets.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Whether this identifier and `other` are the same one.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

/// Which of a repeated wall clock's two instants an instance identity means.
///
/// The value that closes M2's open question. It is a property of a *resolution*, not of a
/// file: nothing in a `RECURRENCE-ID` distinguishes the two halves of a fold, so a side is
/// something a zone answered and never something an octet said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FoldSide {
    /// The wall clock names one instant, which is every hour of the year but one.
    ///
    /// Also the answer for a floating or UTC series, where the projection onto the nominal
    /// timeline is the identity and no hour repeats.
    Once,
    /// The first of the two instants, under the offset in force before the transition.
    Earlier,
    /// The second, under the offset in force after it.
    Later,
    /// Nothing resolved this key: no zone was supplied, or the value named neither instant.
    ///
    /// The default, because a value nobody resolved must not read as one somebody did.
    #[default]
    Unresolved,
}

impl FoldSide {
    /// Which side `named` is of `resolution`, or [`FoldSide::Unresolved`] when it is neither.
    ///
    /// `named` is the real instant the `RECURRENCE-ID` states, present only when the value was
    /// written with a trailing `Z`. A value written as a local time names a wall clock and not
    /// an instant, so there is nothing to compare and the answer is unresolved — which is the
    /// honest report that the two halves of the fold are indistinguishable in that file.
    #[must_use]
    pub fn from_resolution(resolution: LocalResolution, named: Option<Instant>) -> Self {
        match resolution {
            LocalResolution::Unique { .. } => Self::Once,
            LocalResolution::Ambiguous { earlier, later } => match named {
                Some(instant) if instant == earlier.instant => Self::Earlier,
                Some(instant) if instant == later.instant => Self::Later,
                _ => Self::Unresolved,
            },
            // A gap names no instant and an undetermined source names none either. The `_`
            // arm is required because `LocalResolution` is `#[non_exhaustive]`, and it lands
            // in the closed direction: a state this crate has never heard of resolves nothing.
            _ => Self::Unresolved,
        }
    }

    /// Whether something resolved this side.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        !matches!(self, Self::Unresolved)
    }

    /// Whether this side says the wall clock repeats.
    #[must_use]
    pub const fn is_folded(self) -> bool {
        matches!(self, Self::Earlier | Self::Later)
    }
}

/// The clock a `RECURRENCE-ID` value was written in.
///
/// It matters for identity because two values in different clocks name the same instance only
/// if something placed them both, and the placing is what a fold makes ambiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum InstanceClock {
    /// Written with a trailing `Z`: a real instant on the UTC timeline.
    Utc,
    /// Written with a `TZID` parameter: a wall clock under a named zone.
    Zoned,
    /// Written with neither: a wall clock under whatever zone the reader is in.
    Floating,
}

/// Whether two identities are the same one.
///
/// Three answers rather than two, because "these might be the same and I cannot tell" is a
/// real state and reporting it as either `true` or `false` is a decision nobody made. The gate
/// treats [`InstanceMatch::Ambiguous`] as a denial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum InstanceMatch {
    /// The two name one instance.
    Same,
    /// The two name different instances.
    Different,
    /// Nothing here can tell them apart.
    Ambiguous,
}

impl InstanceMatch {
    /// Whether this is a match a decision may be taken on.
    ///
    /// `false` for [`InstanceMatch::Ambiguous`], which is the whole point: a caller writing
    /// `if identity.matches(..).is_same()` gets the safe reading without having to know that
    /// there are three answers.
    #[must_use]
    pub const fn is_same(self) -> bool {
        matches!(self, Self::Same)
    }
}

/// Which instance of a series a message addresses.
///
/// RFC 5546's instance identity, plus the side of a fold M2 left open and the clock the value
/// was written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceRef {
    /// The instant the value names, read in the clock it was written in.
    named: Instant,
    /// Which clock that was.
    clock: InstanceClock,
    /// Which half of a repeated wall clock, once something resolved it.
    side: FoldSide,
    /// How far forward the reference reaches, from RFC 5545 section 3.2.13's `RANGE`.
    ///
    /// `ical_recur::OverrideRange` rather than a second spelling of the same two values, for
    /// the reason DP-13 gives about reusing the change vocabulary.
    range: OverrideRange,
}

impl InstanceRef {
    /// A reference to the instance `named` in `clock`, reaching as far as `range`.
    ///
    /// The side starts [`FoldSide::Unresolved`]; a caller holding a zone attaches one with
    /// [`InstanceRef::with_side`].
    #[must_use]
    pub const fn new(named: Instant, clock: InstanceClock, range: OverrideRange) -> Self {
        Self {
            named,
            clock,
            side: FoldSide::Unresolved,
            range,
        }
    }

    /// The same reference with `side` resolved.
    #[must_use]
    pub const fn with_side(self, side: FoldSide) -> Self {
        Self { side, ..self }
    }

    /// The instant the value names, in the clock it was written in.
    #[must_use]
    pub const fn named(self) -> Instant {
        self.named
    }

    /// The clock the value was written in.
    #[must_use]
    pub const fn clock(self) -> InstanceClock {
        self.clock
    }

    /// Which half of a repeated wall clock this addresses.
    #[must_use]
    pub const fn side(self) -> FoldSide {
        self.side
    }

    /// How far forward this reference reaches.
    #[must_use]
    pub const fn range(self) -> OverrideRange {
        self.range
    }

    /// Whether this reference reaches every later instance.
    #[must_use]
    pub const fn is_this_and_future(self) -> bool {
        matches!(self.range, OverrideRange::ThisAndFuture)
    }

    /// Whether this reference and `other` address the same instance.
    ///
    /// Different instants are different instances and different ranges are different claims,
    /// so both are [`InstanceMatch::Different`] outright. What is left is one instant reached
    /// two ways, and there the fold decides: two resolved sides agree or they do not, and
    /// anything unresolved on either side is [`InstanceMatch::Ambiguous`] rather than a guess.
    ///
    /// Two values written in different clocks are also ambiguous unless both sides were
    /// resolved, because a wall clock and an instant are equal here only by the accident of
    /// their arithmetic agreeing.
    #[must_use]
    pub fn compare(self, other: Self) -> InstanceMatch {
        if self.named != other.named || self.range != other.range {
            return InstanceMatch::Different;
        }
        let resolved = self.side.is_resolved() && other.side.is_resolved();
        if !resolved {
            return InstanceMatch::Ambiguous;
        }
        if self.clock != other.clock {
            return InstanceMatch::Ambiguous;
        }
        if self.side == other.side {
            InstanceMatch::Same
        } else {
            InstanceMatch::Different
        }
    }
}

/// What a `SEQUENCE` property was.
///
/// Three states, because RFC 5546 section 3.2 reads an absent `SEQUENCE` as zero and zero is a
/// revision. A `SEQUENCE` that is present and not an integer is the absence of a revision, and
/// a message whose revision cannot be read cannot be held against the one a caller has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SequenceRead {
    /// No `SEQUENCE` property, which RFC 5546 section 3.2 reads as zero.
    #[default]
    Absent,
    /// A `SEQUENCE` this value.
    Value(u32),
    /// A `SEQUENCE` that was present and was not an integer.
    ///
    /// [`DiagnosticCode::SchedulingSequenceUnreadable`] is what travels beside it.
    ///
    /// [`DiagnosticCode::SchedulingSequenceUnreadable`]: ical_core::DiagnosticCode::SchedulingSequenceUnreadable
    Unreadable,
}

impl SequenceRead {
    /// The revision number, or `None` when there is not one.
    ///
    /// `Some(0)` for an absent property and `None` for an unreadable one, which is the whole
    /// reason this is not an `Option<u32>` at the trait.
    #[must_use]
    pub const fn value(self) -> Option<u32> {
        match self {
            Self::Absent => Some(0),
            Self::Value(sequence) => Some(sequence),
            Self::Unreadable => None,
        }
    }
}

/// Which version of a component a message is, from RFC 5546 sections 2.1.4 and 2.1.5.
///
/// `SEQUENCE` orders versions and `DTSTAMP` breaks ties, and that pair is the whole of this
/// protocol's replay defense. It is weak — nothing signs either number — so the rules below
/// are stated in the refusing direction and the tie is broken towards refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision {
    /// The `SEQUENCE`, with an absent property read as zero.
    sequence: u32,
    /// The `DTSTAMP`, absent when the component states none.
    dtstamp: Option<Instant>,
}

impl Revision {
    /// The revision `sequence` and `dtstamp` state.
    #[must_use]
    pub const fn new(sequence: u32, dtstamp: Option<Instant>) -> Self {
        Self { sequence, dtstamp }
    }

    /// The revision a component states, or `None` when its `SEQUENCE` could not be read.
    #[must_use]
    pub const fn read(sequence: SequenceRead, dtstamp: Option<Instant>) -> Option<Self> {
        match sequence.value() {
            Some(value) => Some(Self::new(value, dtstamp)),
            None => None,
        }
    }

    /// The `SEQUENCE`.
    #[must_use]
    pub const fn sequence(self) -> u32 {
        self.sequence
    }

    /// The `DTSTAMP`, absent when the component states none.
    #[must_use]
    pub const fn dtstamp(self) -> Option<Instant> {
        self.dtstamp
    }

    /// Whether this revision is strictly newer than `held`.
    ///
    /// A higher `SEQUENCE` is newer. An equal `SEQUENCE` is newer only when both sides carry a
    /// `DTSTAMP` and this one is later: a tie nothing can break is not a win, because the
    /// alternative lets a message with no `DTSTAMP` at all overwrite one that has one.
    #[must_use]
    pub fn supersedes(self, held: Self) -> bool {
        if self.sequence != held.sequence {
            return self.sequence > held.sequence;
        }
        match (self.dtstamp, held.dtstamp) {
            (Some(mine), Some(theirs)) => mine > theirs,
            _ => false,
        }
    }

    /// Whether `held` is strictly newer than this revision.
    ///
    /// The gate's own question, and not the negation of [`Revision::supersedes`]: two equal
    /// revisions supersede each other in neither direction and neither is stale, which is
    /// exactly the shape of a `REPLY` — it answers the invitation it was sent and does not
    /// claim to be a newer version of it.
    #[must_use]
    pub fn is_stale_against(self, held: Self) -> bool {
        held.supersedes(self)
    }
}

/// What one message, or one component, is about.
///
/// `UID` plus an optional `RECURRENCE-ID`. Absent means the whole series, which is a different
/// claim from any one instance and never a match for one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageIdentity {
    /// The component's `UID`.
    uid: Uid,
    /// Which instance, absent when the message is about the series.
    instance: Option<InstanceRef>,
}

impl MessageIdentity {
    /// The identity `uid` and `instance` state.
    #[must_use]
    pub const fn new(uid: Uid, instance: Option<InstanceRef>) -> Self {
        Self { uid, instance }
    }

    /// The `UID`.
    #[must_use]
    pub const fn uid(&self) -> &Uid {
        &self.uid
    }

    /// Which instance, absent when this is about the whole series.
    #[must_use]
    pub const fn instance(&self) -> Option<InstanceRef> {
        self.instance
    }

    /// Whether this identity and `other` name the same thing.
    ///
    /// A different `UID` is [`InstanceMatch::Different`] and never ambiguous: identifiers are
    /// compared as octets and octets either agree or they do not. Series against instance is
    /// also `Different` — a `CANCEL` of the series and a `CANCEL` of Tuesday are two messages.
    #[must_use]
    pub fn matches(&self, other: &Self) -> InstanceMatch {
        if !self.uid.matches(&other.uid) {
            return InstanceMatch::Different;
        }
        match (self.instance, other.instance) {
            (None, None) => InstanceMatch::Same,
            (Some(mine), Some(theirs)) => mine.compare(theirs),
            _ => InstanceMatch::Different,
        }
    }
}

#[cfg(test)]
mod tests {
    use ical_core::{Instant, UtcOffset};
    use ical_recur::OverrideRange;
    use ical_tz::{LocalResolution, Reading};

    use super::{
        FoldSide, InstanceClock, InstanceMatch, InstanceRef, MessageIdentity, Revision,
        SequenceRead, Uid,
    };

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    fn instance(seconds: i64, clock: InstanceClock) -> InstanceRef {
        InstanceRef::new(at(seconds), clock, OverrideRange::ThisOnly)
    }

    /// A `UID` is octets. Case and whitespace are differences, and the refusing direction is
    /// the one this crate takes.
    #[test]
    fn an_identifier_is_compared_as_octets_and_never_folded() {
        let uid = Uid::new(b"4f1b-9a@example.com");
        assert!(uid.matches(&Uid::new(b"4f1b-9a@example.com")));
        assert!(!uid.matches(&Uid::new(b"4F1B-9A@example.com")));
        assert!(!uid.matches(&Uid::new(b"4f1b-9a@example.com ")));
        assert_eq!(uid.as_bytes(), b"4f1b-9a@example.com");
    }

    /// The fold, from both halves. A `Z`-terminated `RECURRENCE-ID` picks its side; a wall
    /// clock cannot, and says so.
    #[test]
    fn a_side_is_derived_from_a_resolution_and_never_from_the_octets() {
        let earlier = Reading::new(
            at(1_762_000_200),
            UtcOffset::from_seconds(-14_400).unwrap(),
            true,
        );
        let later = Reading::new(
            at(1_762_003_800),
            UtcOffset::from_seconds(-18_000).unwrap(),
            false,
        );
        let fold = LocalResolution::Ambiguous { earlier, later };

        assert_eq!(
            FoldSide::from_resolution(fold, Some(earlier.instant)),
            FoldSide::Earlier
        );
        assert_eq!(
            FoldSide::from_resolution(fold, Some(later.instant)),
            FoldSide::Later
        );
        assert_eq!(
            FoldSide::from_resolution(fold, None),
            FoldSide::Unresolved,
            "a wall clock names both halves, so it names neither"
        );
        assert_eq!(
            FoldSide::from_resolution(LocalResolution::Unique { reading: earlier }, None),
            FoldSide::Once
        );
        assert_eq!(
            FoldSide::from_resolution(LocalResolution::Undetermined, Some(earlier.instant)),
            FoldSide::Unresolved
        );
        assert!(FoldSide::Later.is_folded() && !FoldSide::Once.is_folded());
        assert!(FoldSide::Once.is_resolved() && !FoldSide::Unresolved.is_resolved());
    }

    /// The question M2 left: two overrides on one cadence key. Resolved, they are told apart;
    /// unresolved, the answer is ambiguous and the gate above refuses rather than guesses.
    #[test]
    fn two_halves_of_a_fold_are_one_key_and_are_still_two_instances() {
        let key = 1_762_000_200;
        let unresolved = instance(key, InstanceClock::Utc);
        assert_eq!(
            unresolved.compare(unresolved),
            InstanceMatch::Ambiguous,
            "one key, and nothing said which meeting"
        );
        assert!(!InstanceMatch::Ambiguous.is_same());

        let first = unresolved.with_side(FoldSide::Earlier);
        let second = unresolved.with_side(FoldSide::Later);
        assert_eq!(first.compare(first), InstanceMatch::Same);
        assert_eq!(first.compare(second), InstanceMatch::Different);

        let elsewhere = instance(key + 3600, InstanceClock::Utc).with_side(FoldSide::Once);
        assert_eq!(first.compare(elsewhere), InstanceMatch::Different);
    }

    /// A range is part of the claim, and two clocks are not comparable without a resolution.
    #[test]
    fn a_range_and_a_clock_are_part_of_what_an_instance_reference_claims() {
        let one = instance(90, InstanceClock::Utc).with_side(FoldSide::Once);
        let onwards = InstanceRef::new(at(90), InstanceClock::Utc, OverrideRange::ThisAndFuture)
            .with_side(FoldSide::Once);
        assert_eq!(one.compare(onwards), InstanceMatch::Different);
        assert!(onwards.is_this_and_future() && !one.is_this_and_future());

        let zoned = instance(90, InstanceClock::Zoned).with_side(FoldSide::Once);
        assert_eq!(one.compare(zoned), InstanceMatch::Ambiguous);
        assert_eq!(one.named(), at(90));
        assert_eq!(one.range(), OverrideRange::ThisOnly);
    }

    /// RFC 5546 sections 2.1.4 and 2.1.5, in the direction that refuses.
    #[test]
    fn an_older_revision_never_overwrites_a_newer_one() {
        let held = Revision::new(2, Some(at(1_000)));
        assert!(Revision::new(1, Some(at(9_999))).is_stale_against(held));
        assert!(Revision::new(2, Some(at(999))).is_stale_against(held));
        assert!(!Revision::new(2, Some(at(1_000))).is_stale_against(held));
        assert!(Revision::new(3, None).supersedes(held));
        assert!(
            !Revision::new(2, None).supersedes(held),
            "a message with no tie-break does not win a tie"
        );
        assert!(
            !Revision::new(2, Some(at(9_999))).is_stale_against(Revision::new(2, None)),
            "and it does not lose one either, since neither is stale"
        );
    }

    /// An absent `SEQUENCE` is zero. An unreadable one is not a revision at all.
    #[test]
    fn an_absent_sequence_is_zero_and_an_unreadable_one_is_nothing() {
        assert_eq!(SequenceRead::Absent.value(), Some(0));
        assert_eq!(SequenceRead::Value(7).value(), Some(7));
        assert_eq!(SequenceRead::Unreadable.value(), None);
        assert_eq!(Revision::read(SequenceRead::Unreadable, Some(at(1))), None);
        assert_eq!(
            Revision::read(SequenceRead::Absent, None),
            Some(Revision::new(0, None))
        );
    }

    /// The series and one of its instances are two different things to send a message about.
    #[test]
    fn a_message_about_a_series_never_matches_one_about_an_instance() {
        let uid = Uid::new(b"4f1b-9a");
        let series = MessageIdentity::new(uid.clone(), None);
        let tuesday = MessageIdentity::new(
            uid.clone(),
            Some(instance(90, InstanceClock::Utc).with_side(FoldSide::Once)),
        );
        assert_eq!(series.matches(&series), InstanceMatch::Same);
        assert_eq!(series.matches(&tuesday), InstanceMatch::Different);
        assert_eq!(tuesday.matches(&tuesday), InstanceMatch::Same);
        assert_eq!(
            MessageIdentity::new(Uid::new(b"other"), None).matches(&series),
            InstanceMatch::Different
        );
        assert_eq!(series.uid(), &uid);
        assert_eq!(series.instance(), None);
    }
}
