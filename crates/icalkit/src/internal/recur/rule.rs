// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `RECUR` value type of RFC 5545 section 3.3.10, as a type rather than as text.
//!
//! Every invariant this file can establish is established once, at construction, and never
//! revisited by anything downstream. `INTERVAL` is non-zero because [`NonZeroU32`] is the
//! type. A `BYDAY` ordinal is within ±53 and never zero, because [`WeekdayNum::new`] refuses
//! the first and [`NonZeroI8`] forbids the second. `BYSETPOS` is empty unless another `BYxxx`
//! part is present, because [`RecurrenceRuleBuilder::build`] refuses the pair. An expansion
//! engine that had to re-check any of those would be a second opinion about the same
//! question, and the two would eventually disagree.
//!
//! What is *not* established here is anything a period would answer. `BYMONTHDAY=31` is a
//! legal rule that names no day of February, and RFC 5545 section 3.3.10 says such an
//! instance is ignored rather than moved; that is the expansion's answer per candidate, not a
//! construction error, and `docs/adr/0011` makes the distinction binding. Values that are out
//! of range outright — `BYMONTHDAY=32`, which names no day of any month — are dropped by the
//! decoder with a diagnostic and never reach a builder at all, because `docs/adr/0001` forbids
//! discarding a component over one bad part.

use alloc::vec::Vec;
use core::error::Error;
use core::fmt::{self, Display, Formatter};
use core::num::{NonZeroI8, NonZeroU32};

use crate::internal::core::{Instant, Weekday};

use crate::internal::recur::table::PartsPresent;

/// How often the base cadence repeats, from RFC 5545 section 3.3.10's `freq` production.
///
/// Not `#[non_exhaustive]`: section 3.3.10 closes the set at seven, and marking it open would
/// tax every downstream `match` forever for a variant that cannot arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Freq {
    /// `SECONDLY`.
    Secondly,
    /// `MINUTELY`.
    Minutely,
    /// `HOURLY`.
    Hourly,
    /// `DAILY`.
    Daily,
    /// `WEEKLY`.
    Weekly,
    /// `MONTHLY`.
    Monthly,
    /// `YEARLY`.
    Yearly,
}

impl Freq {
    /// Every frequency, in the column order RFC 5545 section 3.3.10's table prints.
    pub const ALL: [Self; 7] = [
        Self::Secondly,
        Self::Minutely,
        Self::Hourly,
        Self::Daily,
        Self::Weekly,
        Self::Monthly,
        Self::Yearly,
    ];

    /// How many frequencies there are, which is the width of the expand/limit table.
    pub const COUNT: usize = Self::ALL.len();

    /// The name RFC 5545 section 3.3.10 writes.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Secondly => b"SECONDLY",
            Self::Minutely => b"MINUTELY",
            Self::Hourly => b"HOURLY",
            Self::Daily => b"DAILY",
            Self::Weekly => b"WEEKLY",
            Self::Monthly => b"MONTHLY",
            Self::Yearly => b"YEARLY",
        }
    }

    /// This frequency's column in the expand/limit table.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Secondly => 0,
            Self::Minutely => 1,
            Self::Hourly => 2,
            Self::Daily => 3,
            Self::Weekly => 4,
            Self::Monthly => 5,
            Self::Yearly => 6,
        }
    }
}

/// One `BYxxx` rule part, named so the expand/limit table can be indexed by it.
///
/// `BYSETPOS` is a row of that table and is nonetheless not a `BYxxx` part in the sense the
/// other eight are: it selects from the candidate set the others produced, after every one of
/// them has run. It is listed here because the RFC lists it, and the engine that applies the
/// other eight must not apply this one in the same pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RulePart {
    /// `BYMONTH`.
    Month,
    /// `BYWEEKNO`.
    WeekNo,
    /// `BYYEARDAY`.
    YearDay,
    /// `BYMONTHDAY`.
    MonthDay,
    /// `BYDAY`.
    Day,
    /// `BYHOUR`.
    Hour,
    /// `BYMINUTE`.
    Minute,
    /// `BYSECOND`.
    Second,
    /// `BYSETPOS`.
    SetPos,
}

impl RulePart {
    /// Every rule part, in the row order RFC 5545 section 3.3.10's table prints.
    pub const ALL: [Self; 9] = [
        Self::Month,
        Self::WeekNo,
        Self::YearDay,
        Self::MonthDay,
        Self::Day,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::SetPos,
    ];

    /// How many rule parts there are, which is the height of the expand/limit table.
    pub const COUNT: usize = Self::ALL.len();

    /// The name RFC 5545 section 3.3.10 writes.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Month => b"BYMONTH",
            Self::WeekNo => b"BYWEEKNO",
            Self::YearDay => b"BYYEARDAY",
            Self::MonthDay => b"BYMONTHDAY",
            Self::Day => b"BYDAY",
            Self::Hour => b"BYHOUR",
            Self::Minute => b"BYMINUTE",
            Self::Second => b"BYSECOND",
            Self::SetPos => b"BYSETPOS",
        }
    }

    /// This part's bit in a [`PartsPresent`] set.
    ///
    /// Written as literals rather than as `1 << index()`: a shift is arithmetic, this
    /// workspace denies arithmetic that can overflow, and a checked shift in a `const fn`
    /// buys nothing a nine-arm match does not already give.
    #[must_use]
    pub const fn bit(self) -> u16 {
        match self {
            Self::Month => 1,
            Self::WeekNo => 2,
            Self::YearDay => 4,
            Self::MonthDay => 8,
            Self::Day => 16,
            Self::Hour => 32,
            Self::Minute => 64,
            Self::Second => 128,
            Self::SetPos => 256,
        }
    }

    /// This part's row in the expand/limit table.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Month => 0,
            Self::WeekNo => 1,
            Self::YearDay => 2,
            Self::MonthDay => 3,
            Self::Day => 4,
            Self::Hour => 5,
            Self::Minute => 6,
            Self::Second => 7,
            Self::SetPos => 8,
        }
    }
}

/// Whether an instant is written as a `DATE` or as a `DATE-TIME`.
///
/// Deliberately narrower than `crate::internal::core::ValueType`, which has fourteen variants. Only these
/// two can be the value type of a `DTSTART` or an `UNTIL`, and a two-variant enum makes the
/// other twelve unrepresentable at the one place RFC 5545 section 3.3.10 requires the two to
/// agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValueKind {
    /// RFC 5545 section 3.3.4, a date with no clock.
    Date,
    /// RFC 5545 section 3.3.5, a date and a time.
    DateTime,
}

/// One entry of `BYDAY`: a weekday, optionally counted from one end of a scope.
///
/// `-1MO` is the last Monday, `2MO` is the second. Which scope it counts within is not a
/// property of this value — RFC 5545 section 3.3.10's Note 1 and Note 2 decide that from the
/// frequency and from which other parts are present, and `crate::internal::recur::table` resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WeekdayNum {
    /// The ordinal, within ±53 and never zero.
    ordinal: Option<NonZeroI8>,
    /// The weekday itself.
    weekday: Weekday,
}

impl WeekdayNum {
    /// The largest ordinal RFC 5545 section 3.3.10 admits, in either direction.
    ///
    /// A year has at most 53 occurrences of a weekday, so 53 is the ceiling the widest scope
    /// can justify; the narrower scopes justify less and are checked per candidate rather than
    /// here, because the scope is not known at construction.
    pub const MAX_ORDINAL: i8 = 53;

    /// A `BYDAY` entry, or `None` when the ordinal is outside ±53.
    #[must_use]
    pub fn new(ordinal: Option<NonZeroI8>, weekday: Weekday) -> Option<Self> {
        let too_far = ordinal
            .is_some_and(|count| count.get().unsigned_abs() > Self::MAX_ORDINAL.unsigned_abs());
        if too_far {
            return None;
        }
        Some(Self { ordinal, weekday })
    }

    /// The ordinal, absent when the entry names every occurrence of the weekday.
    #[must_use]
    pub const fn ordinal(self) -> Option<NonZeroI8> {
        self.ordinal
    }

    /// The weekday.
    #[must_use]
    pub const fn weekday(self) -> Weekday {
        self.weekday
    }
}

/// The values one `BYxxx` part carries, in the order the file wrote them.
///
/// Order is preserved rather than sorted. RFC 5545 section 3.3.10 does not require a sorted
/// list and `docs/adr/0001` requires the original text to survive a round trip, so sorting
/// here would be an opinion this crate is not entitled to have; the expansion sorts the
/// candidates it produces, which is a different question.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByList<T>(Vec<T>);

impl<T> ByList<T> {
    /// An empty list, which is what "the part is absent" means everywhere in this crate.
    ///
    /// Absent and empty are deliberately the same state. RFC 5545 section 3.3.10's grammar
    /// gives no way to write a present-but-empty part, so a second state would be one nothing
    /// can produce and everything would have to match on.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// The values, in the order they were written.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    /// Whether the part is absent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many values the part carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<T: Clone> ByList<T> {
    /// A list holding a copy of `values`.
    #[must_use]
    pub fn from_slice(values: &[T]) -> Self {
        Self(values.to_vec())
    }
}

impl<T> From<Vec<T>> for ByList<T> {
    fn from(values: Vec<T>) -> Self {
        Self(values)
    }
}

/// Which clock an `UNTIL` was written on, and therefore which clock it compares in.
///
/// RFC 5545 section 3.3.10 requires `UNTIL` to be a UTC date-time whenever `DTSTART` is a
/// date-time with a `TZID`, and a date whenever `DTSTART` is a date. Real files violate that
/// constantly — Google has emitted a floating `UNTIL` against a zoned `DTSTART` — so the
/// violation cannot be refused and the comparison still has to happen somewhere.
///
/// This field is what stops that comparison from happening in an unnamed clock, which is where
/// every off-by-one-day bug in this area lives. [`UntilClock::Utc`] means the file wrote a
/// trailing `Z` and the instant beside it is that UTC instant. [`UntilClock::Floating`] means
/// it did not, and the instant beside it is the wall-clock reading *interpreted at UTC* —
/// correct only if the caller resolved `DTSTART` the same way, which is exactly the
/// normalization `docs/adr/0003` makes the caller's step.
///
/// M2 settled what that normalization is, and one sentence above is narrower than the answer.
/// The timeline this crate walks for a **zoned** series is the series' own wall clock projected
/// onto UTC, not the UTC timeline: `ical-tz` projects `DTSTART`, `UNTIL`, every `RDATE`,
/// `EXDATE` and `RECURRENCE-ID` onto it before the search, and reads each cadence key back
/// through the zone afterwards, which is what keeps a daily 09:00 series at 09:00 across a
/// daylight saving transition. For a floating series and for a UTC series that projection is
/// the identity and [`UntilClock::Utc`]'s reading above holds verbatim. For a zoned one it is
/// not: the instant beside the variant is the projection, and the variant records that the
/// *file* wrote `Z`. A `Z`-terminated `UNTIL` handed over unprojected cuts a zoned series off
/// an hour early or late for half the year. `crate::internal::tz::seam` states the contract in full, and a
/// floating `UNTIL` against a zoned `DTSTART` now travels on
/// `DiagnosticCode::RecurrenceUntilNotUtc` rather than only being named by this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UntilClock {
    /// The value carried a trailing `Z`, so it names a UTC instant outright.
    Utc,
    /// The value carried no `Z`, so it is a wall clock read at UTC for want of a zone.
    Floating,
}

/// Where the series stops, from RFC 5545 section 3.3.10's `COUNT` and `UNTIL`.
///
/// Three states rather than two `Option`s, because `COUNT` and `UNTIL` are mutually exclusive
/// in the grammar and a pair of options would make the illegal fourth combination
/// representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleLimit {
    /// Neither `COUNT` nor `UNTIL`: the series has no end.
    Infinite,
    /// `COUNT`, counted over occurrences the rule emitted, starting at `DTSTART`.
    Count(NonZeroU32),
    /// `UNTIL`, an inclusive bound on the cadence key.
    Until {
        /// The bound, already resolved to the timeline by the caller.
        at: Instant,
        /// Whether the file wrote it as a `DATE` or as a `DATE-TIME`.
        ///
        /// Kept beside the instant because RFC 5545 section 3.3.10 requires it to agree with
        /// `DTSTART`'s and real files disagree constantly.
        value_kind: ValueKind,
        /// Which clock `at` was read on.
        clock: UntilClock,
    },
}

/// Why no rule could be built at all.
///
/// This is the error channel of `docs/adr/0009`, and it is narrow on purpose: it names only
/// the conditions under which no rule exists. Everything survivable — a part out of range, a
/// part repeated, a part nothing defines — is a diagnostic on the property and the component
/// still parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RuleError {
    /// The value carried no `FREQ`, which RFC 5545 section 3.3.10 requires of every rule.
    MissingFrequency,
    /// The value's `FREQ` named none of the seven frequencies.
    UnknownFrequency,
    /// A `BYDAY` ordinal was outside ±53.
    OrdinalOutOfRange,
    /// `BYSETPOS` was given with no other `BYxxx` part for it to select from.
    BySetPosWithoutByRule,
}

impl Display for RuleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let reason = match *self {
            Self::MissingFrequency => "the recurrence rule carried no FREQ",
            Self::UnknownFrequency => "the recurrence rule's FREQ named no known frequency",
            Self::OrdinalOutOfRange => "a BYDAY ordinal was outside the range +/-53",
            Self::BySetPosWithoutByRule => "BYSETPOS was given with no other BYxxx part",
        };
        formatter.write_str(reason)
    }
}

impl Error for RuleError {}

/// One recurrence rule, with every invariant its construction can settle already settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurrenceRule {
    /// The base cadence.
    freq: Freq,
    /// How many periods one step of the cadence covers.
    interval: NonZeroU32,
    /// Where the series stops.
    limit: RuleLimit,
    /// The day a week starts on, which `BYWEEKNO` and `FREQ=WEEKLY` count from.
    wkst: Weekday,
    /// `BYSECOND`, each within 0..=60.
    by_second: ByList<u8>,
    /// `BYMINUTE`, each within 0..=59.
    by_minute: ByList<u8>,
    /// `BYHOUR`, each within 0..=23.
    by_hour: ByList<u8>,
    /// `BYDAY`.
    by_day: ByList<WeekdayNum>,
    /// `BYMONTHDAY`, each within ±1..=31.
    by_month_day: ByList<i8>,
    /// `BYYEARDAY`, each within ±1..=366.
    by_year_day: ByList<i16>,
    /// `BYWEEKNO`, each within ±1..=53.
    by_week_no: ByList<i8>,
    /// `BYMONTH`, each within 1..=12.
    by_month: ByList<u8>,
    /// `BYSETPOS`, each within ±1..=366, and never present alone.
    by_set_pos: ByList<i16>,
}

impl RecurrenceRule {
    /// The `INTERVAL` a rule that states none has, which RFC 5545 section 3.3.10 fixes at 1.
    pub const DEFAULT_INTERVAL: NonZeroU32 = NonZeroU32::MIN;

    /// The `WKST` a rule that states none has, which RFC 5545 section 3.3.10 fixes at Monday.
    pub const DEFAULT_WKST: Weekday = Weekday::Monday;

    /// The base cadence.
    #[must_use]
    pub const fn freq(&self) -> Freq {
        self.freq
    }

    /// How many periods one step of the cadence covers.
    #[must_use]
    pub const fn interval(&self) -> NonZeroU32 {
        self.interval
    }

    /// Where the series stops.
    #[must_use]
    pub const fn limit(&self) -> RuleLimit {
        self.limit
    }

    /// The day a week starts on.
    #[must_use]
    pub const fn wkst(&self) -> Weekday {
        self.wkst
    }

    /// `BYSECOND`.
    #[must_use]
    pub const fn by_second(&self) -> &ByList<u8> {
        &self.by_second
    }

    /// `BYMINUTE`.
    #[must_use]
    pub const fn by_minute(&self) -> &ByList<u8> {
        &self.by_minute
    }

    /// `BYHOUR`.
    #[must_use]
    pub const fn by_hour(&self) -> &ByList<u8> {
        &self.by_hour
    }

    /// `BYDAY`.
    #[must_use]
    pub const fn by_day(&self) -> &ByList<WeekdayNum> {
        &self.by_day
    }

    /// `BYMONTHDAY`.
    #[must_use]
    pub const fn by_month_day(&self) -> &ByList<i8> {
        &self.by_month_day
    }

    /// `BYYEARDAY`.
    #[must_use]
    pub const fn by_year_day(&self) -> &ByList<i16> {
        &self.by_year_day
    }

    /// `BYWEEKNO`.
    #[must_use]
    pub const fn by_week_no(&self) -> &ByList<i8> {
        &self.by_week_no
    }

    /// `BYMONTH`.
    #[must_use]
    pub const fn by_month(&self) -> &ByList<u8> {
        &self.by_month
    }

    /// `BYSETPOS`.
    #[must_use]
    pub const fn by_set_pos(&self) -> &ByList<i16> {
        &self.by_set_pos
    }

    /// The same rule, ending where `limit` says instead of where this one does.
    ///
    /// The one edit a parsed rule needs and the one the seam with `ical-tz` cannot do without.
    /// `parse_recur` reads `UNTIL=20260310T120000Z` as the real UTC instant the file wrote, and
    /// a zoned series is walked on the timeline `crate::internal::tz::seam` describes, so the bound has to
    /// be projected onto that timeline before it is compared against a cadence key. Until this
    /// existed the only way to substitute the projection was to rebuild the rule through
    /// [`RecurrenceRuleBuilder`] and copy every `BYxxx` list across by hand, which is a
    /// correction no caller performs by accident and several perform wrongly.
    ///
    /// Clones the lists rather than mutating in place, because a `RecurrenceRule` read from a
    /// file is what the file said and a caller may want to keep both readings.
    #[must_use]
    pub fn with_limit(&self, limit: RuleLimit) -> Self {
        Self {
            limit,
            ..self.clone()
        }
    }

    /// Whether `part` carries at least one value.
    #[must_use]
    pub fn has_part(&self, part: RulePart) -> bool {
        match part {
            RulePart::Month => !self.by_month.is_empty(),
            RulePart::WeekNo => !self.by_week_no.is_empty(),
            RulePart::YearDay => !self.by_year_day.is_empty(),
            RulePart::MonthDay => !self.by_month_day.is_empty(),
            RulePart::Day => !self.by_day.is_empty(),
            RulePart::Hour => !self.by_hour.is_empty(),
            RulePart::Minute => !self.by_minute.is_empty(),
            RulePart::Second => !self.by_second.is_empty(),
            RulePart::SetPos => !self.by_set_pos.is_empty(),
        }
    }

    /// Which rule parts this rule carries, for the expand/limit table's two `BYDAY` notes.
    #[must_use]
    pub fn parts_present(&self) -> PartsPresent {
        RulePart::ALL
            .into_iter()
            .filter(|part| self.has_part(*part))
            .fold(PartsPresent::NONE, PartsPresent::with)
    }

    /// Whether any `BYxxx` part other than `BYSETPOS` carries a value.
    ///
    /// The predicate `BYSETPOS` is legal under, and the reason it is stated once here rather
    /// than open-coded: "another part is present" is a claim about eight rows of the table,
    /// and eight is enough for a hand-written condition to lose one.
    #[must_use]
    pub fn has_selectable_part(&self) -> bool {
        RulePart::ALL
            .iter()
            .filter(|part| **part != RulePart::SetPos)
            .any(|part| self.has_part(*part))
    }
}

/// Assembles a [`RecurrenceRule`], defaulting everything RFC 5545 section 3.3.10 defaults.
///
/// A builder rather than a constructor because a rule has thirteen fields of which twelve have
/// a specified default, and a twelve-argument constructor would exceed this workspace's own
/// argument bound while telling a reader nothing.
#[derive(Clone, Debug)]
pub struct RecurrenceRuleBuilder {
    /// The rule as assembled so far, complete at every point except for its checks.
    rule: RecurrenceRule,
}

impl RecurrenceRuleBuilder {
    /// A rule at `freq`, with every other part at RFC 5545 section 3.3.10's default.
    #[must_use]
    pub fn new(freq: Freq) -> Self {
        Self {
            rule: RecurrenceRule {
                freq,
                interval: RecurrenceRule::DEFAULT_INTERVAL,
                limit: RuleLimit::Infinite,
                wkst: RecurrenceRule::DEFAULT_WKST,
                by_second: ByList::empty(),
                by_minute: ByList::empty(),
                by_hour: ByList::empty(),
                by_day: ByList::empty(),
                by_month_day: ByList::empty(),
                by_year_day: ByList::empty(),
                by_week_no: ByList::empty(),
                by_month: ByList::empty(),
                by_set_pos: ByList::empty(),
            },
        }
    }

    /// Set `INTERVAL`.
    #[must_use]
    pub fn interval(mut self, interval: NonZeroU32) -> Self {
        self.rule.interval = interval;
        self
    }

    /// Set `COUNT` or `UNTIL`, or neither.
    #[must_use]
    pub fn limit(mut self, limit: RuleLimit) -> Self {
        self.rule.limit = limit;
        self
    }

    /// Set `WKST`.
    #[must_use]
    pub fn wkst(mut self, wkst: Weekday) -> Self {
        self.rule.wkst = wkst;
        self
    }

    /// Set `BYSECOND`.
    #[must_use]
    pub fn by_second(mut self, values: ByList<u8>) -> Self {
        self.rule.by_second = values;
        self
    }

    /// Set `BYMINUTE`.
    #[must_use]
    pub fn by_minute(mut self, values: ByList<u8>) -> Self {
        self.rule.by_minute = values;
        self
    }

    /// Set `BYHOUR`.
    #[must_use]
    pub fn by_hour(mut self, values: ByList<u8>) -> Self {
        self.rule.by_hour = values;
        self
    }

    /// Set `BYDAY`.
    #[must_use]
    pub fn by_day(mut self, values: ByList<WeekdayNum>) -> Self {
        self.rule.by_day = values;
        self
    }

    /// Set `BYMONTHDAY`.
    #[must_use]
    pub fn by_month_day(mut self, values: ByList<i8>) -> Self {
        self.rule.by_month_day = values;
        self
    }

    /// Set `BYYEARDAY`.
    #[must_use]
    pub fn by_year_day(mut self, values: ByList<i16>) -> Self {
        self.rule.by_year_day = values;
        self
    }

    /// Set `BYWEEKNO`.
    #[must_use]
    pub fn by_week_no(mut self, values: ByList<i8>) -> Self {
        self.rule.by_week_no = values;
        self
    }

    /// Set `BYMONTH`.
    #[must_use]
    pub fn by_month(mut self, values: ByList<u8>) -> Self {
        self.rule.by_month = values;
        self
    }

    /// Set `BYSETPOS`.
    #[must_use]
    pub fn by_set_pos(mut self, values: ByList<i16>) -> Self {
        self.rule.by_set_pos = values;
        self
    }

    /// Check what a whole rule can be checked for, and hand back the rule.
    ///
    /// One condition, because one is all that is not already a type invariant: RFC 5545
    /// section 3.3.10 forbids `BYSETPOS` unless another `BYxxx` part is present, and no field
    /// type can express a relation between two fields.
    pub fn build(self) -> Result<RecurrenceRule, RuleError> {
        if !self.rule.by_set_pos.is_empty() && !self.rule.has_selectable_part() {
            return Err(RuleError::BySetPosWithoutByRule);
        }
        Ok(self.rule)
    }
}

#[cfg(test)]
mod tests {
    use core::num::{NonZeroI8, NonZeroU32};

    use crate::internal::core::Weekday;

    use super::{
        ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleError, RulePart, WeekdayNum,
    };

    #[test]
    fn a_default_rule_carries_the_defaults_the_rfc_states() {
        let rule = RecurrenceRuleBuilder::new(Freq::Daily).build().unwrap();
        assert_eq!(rule.interval(), NonZeroU32::new(1).unwrap());
        assert_eq!(rule.wkst(), Weekday::Monday);
        assert!(!rule.has_selectable_part());
    }

    #[test]
    fn by_set_pos_alone_is_the_one_condition_build_refuses() {
        let alone = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_set_pos(ByList::from_slice(&[-1_i16]))
            .build();
        assert_eq!(alone, Err(RuleError::BySetPosWithoutByRule));

        let monday = WeekdayNum::new(None, Weekday::Monday).unwrap();
        let accompanied = RecurrenceRuleBuilder::new(Freq::Monthly)
            .by_day(ByList::from_slice(&[monday]))
            .by_set_pos(ByList::from_slice(&[-1_i16]))
            .build();
        assert!(accompanied.is_ok());
    }

    #[test]
    fn an_ordinal_past_fifty_three_has_no_by_day_entry() {
        let past = NonZeroI8::new(54).unwrap();
        assert_eq!(WeekdayNum::new(Some(past), Weekday::Monday), None);
        let negative = NonZeroI8::new(-54).unwrap();
        assert_eq!(WeekdayNum::new(Some(negative), Weekday::Monday), None);
        let last = NonZeroI8::new(-53).unwrap();
        assert!(WeekdayNum::new(Some(last), Weekday::Monday).is_some());
    }

    #[test]
    fn the_notes_read_the_four_flags_the_rule_reports() {
        let rule = RecurrenceRuleBuilder::new(Freq::Yearly)
            .by_month(ByList::from_slice(&[5_u8]))
            .build()
            .unwrap();
        let present = rule.parts_present();
        assert!(present.has(RulePart::Month));
        assert!(!present.has(RulePart::WeekNo));
        assert!(rule.has_part(RulePart::Month));
        assert!(!rule.has_part(RulePart::Day));
    }

    #[test]
    fn the_table_indices_are_the_order_the_rfc_prints() {
        for (position, freq) in Freq::ALL.into_iter().enumerate() {
            assert_eq!(freq.index(), position);
        }
        for (position, part) in RulePart::ALL.into_iter().enumerate() {
            assert_eq!(part.index(), position);
        }
        assert_eq!(RecurrenceRule::DEFAULT_INTERVAL.get(), 1);
    }
}
