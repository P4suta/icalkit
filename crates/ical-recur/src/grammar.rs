// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading RFC 5545 section 3.3.10's `RECUR` value out of the octets a file wrote.
//!
//! `ical-core` stops at preserved text for this value type on purpose — recurrence semantics
//! are the whole of this crate's subject — so the decoder is here and reaches a caller through
//! `Property::value::<RecurrenceRule>()` like every other value type in the workspace.
//!
//! # Two readings, one grammar
//!
//! [`DecodeValue`] has one channel: a value, or one [`DiagnosticCode`]. That is enough for a
//! *strict* reading and not enough for a lenient one, and this crate needs both.
//!
//! The strict reading is what this file implements. Anything that deviates from section
//! 3.3.10 — a missing `FREQ`, a `BYMONTHDAY=32`, a repeated part — is refused with the code
//! that names it, and `View::Malformed` hands the caller that code *next to the property's
//! untouched text*. Nothing is lost: `docs/adr/0001`'s guarantee is about the octets, and the
//! octets are still there and still written back.
//!
//! The lenient reading — drop the part that is out of range, report it to a sink, keep the
//! rule — needs a sink and a meter, which no `DecodeValue` signature carries. It is
//! [`parse_recur`], and it is built on the same [`parts`] walk and the same decoders rather
//! than on a second copy of the grammar. Two spellings of one grammar is the failure
//! `docs/adr/0008` names for the parser and it is no better here.
//!
//! # Order
//!
//! Section 3.3.10 says the rule parts "are not ordered in any particular sequence", and the two
//! readings honor that differently. [`parse_recur`] collects every pair before it decodes any of
//! them, so `BYDAY=MO;FREQ=WEEKLY` is the weekly rule it looks like. The strict reading requires
//! `FREQ` first, because it applies each part as it meets it and no builder exists before a
//! frequency does; that is stricter than the grammar, and a conforming value written in the
//! other order therefore reaches a caller as `View::Malformed` carrying its own octets rather
//! than as a rule. Only the strict reading has that restriction, and only the lenient one is
//! offered to a caller reading files it did not write.
//!
//! # Case
//!
//! Rule part names and enumerated values are compared without regard to case. RFC 5545's ABNF
//! writes them as quoted literals, and ABNF quoted literals are case-insensitive; producers
//! that write `freq=weekly` are conforming and are also rare enough that a reader written
//! against the common case would never notice.

use alloc::vec::Vec;
use core::num::{NonZeroI8, NonZeroU32};

use ical_core::{
    CivilDateTime, CivilTime, DateTimeValue, DecodeValue, Diagnostic, DiagnosticCode,
    DiagnosticSink, Location, Meter, Severity, UtcOffset, Weekday, report_diagnostic,
};

use crate::rule::{
    ByList, Freq, RecurrenceRule, RecurrenceRuleBuilder, RuleError, RuleLimit, RulePart,
    UntilClock, ValueKind, WeekdayNum,
};

/// One `name=value` pair of a `RECUR` value, with neither side interpreted.
///
/// Public so that the lenient reading can walk the same pairs the strict one does, and report
/// a diagnostic per pair instead of refusing the whole value at the first one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RulePartText<'a> {
    /// The part name, as written.
    name: &'a [u8],
    /// The part value, as written, `,`-separated list included.
    value: &'a [u8],
}

impl<'a> RulePartText<'a> {
    /// The part name, as written.
    #[must_use]
    pub const fn name(self) -> &'a [u8] {
        self.name
    }

    /// The part value, as written.
    #[must_use]
    pub const fn value(self) -> &'a [u8] {
        self.value
    }

    /// The rule part this names, `None` for `FREQ`, `UNTIL`, `COUNT`, `INTERVAL`, `WKST` and
    /// for a name section 3.3.10 does not define.
    ///
    /// Only the nine `BYxxx` rows of the expand/limit table have a [`RulePart`]; the other
    /// five parts are not rows of it and giving them one would put five cells in a table the
    /// RFC prints with sixty-three.
    #[must_use]
    pub fn by_part(self) -> Option<RulePart> {
        RulePart::ALL
            .into_iter()
            .find(|part| equals_ignoring_case(self.name, part.as_bytes()))
    }
}

/// Every `name=value` pair of `value_text`, in the order it wrote them.
///
/// A pair with no `=` yields an empty value rather than being skipped, so the caller decides
/// whether that is a violation; skipping here would make a malformed part indistinguishable
/// from an absent one.
pub fn parts(value_text: &[u8]) -> impl Iterator<Item = RulePartText<'_>> {
    value_text.split(|octet| *octet == b';').map(|pair| {
        let (name, value) = split_once(pair, b'=').unwrap_or((pair, &[]));
        RulePartText { name, value }
    })
}

/// Compare two byte strings ignoring ASCII case.
fn equals_ignoring_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(one, other)| one.eq_ignore_ascii_case(other))
}

/// Split `bytes` at the first `separator`, or `None` when there is none.
fn split_once(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|octet| *octet == separator)?;
    let head = bytes.get(..at)?;
    // `at` is a valid index, so `at + 1` is at most `bytes.len()` and the slice exists. The
    // checked add is written anyway: this file has no business being the one place a bound
    // holds by argument rather than by construction.
    let tail = bytes.get(at.checked_add(1)?..)?;
    Some((head, tail))
}

/// The name RFC 5545 section 3.3.10 gives the one rule part that is required.
///
/// A constant rather than a literal at each of the three places that compare against it,
/// because the two readings disagree about *where* `FREQ` may appear and must not be given the
/// chance to disagree about what it is called.
const FREQ_NAME: &[u8] = b"FREQ";

/// A rule part that carries one value and is not a row of the expand/limit table.
///
/// `FREQ` is deliberately not one of them, and the omission is the point: `FREQ` is what
/// *makes* a rule rather than what changes one, so both readings settle it before any of these
/// four is applied, and neither carries an arm here that nothing can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScalarPart {
    /// `UNTIL`.
    Until,
    /// `COUNT`.
    Count,
    /// `INTERVAL`.
    Interval,
    /// `WKST`.
    Wkst,
}

impl ScalarPart {
    /// Every scalar part, in the order RFC 5545 section 3.3.10's grammar lists them.
    const ALL: [Self; 4] = [Self::Until, Self::Count, Self::Interval, Self::Wkst];

    /// How many scalar parts there are.
    const COUNT: usize = Self::ALL.len();

    /// The name RFC 5545 section 3.3.10 writes.
    const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Until => b"UNTIL",
            Self::Count => b"COUNT",
            Self::Interval => b"INTERVAL",
            Self::Wkst => b"WKST",
        }
    }

    /// This part's own position among the four, for a fixed-size slot per name.
    const fn index(self) -> usize {
        match self {
            Self::Until => 0,
            Self::Count => 1,
            Self::Interval => 2,
            Self::Wkst => 3,
        }
    }

    /// This part's number in the flat numbering [`StrictDecode::seen`] keeps.
    ///
    /// Written as literals past [`FREQ_SLOT`] rather than as a sum: a sum is arithmetic, this
    /// workspace denies arithmetic that can overflow, and four literals beside the nine
    /// [`RulePart::index`] already supplies are easier to hold against each other than a
    /// checked add would be.
    const fn slot(self) -> u32 {
        match self {
            Self::Until => 17,
            Self::Count => 18,
            Self::Interval => 19,
            Self::Wkst => 20,
        }
    }
}

/// The scalar part `name` names, `None` for `FREQ`, a `BYxxx` part, or an undefined name.
fn scalar_part(name: &[u8]) -> Option<ScalarPart> {
    ScalarPart::ALL
        .into_iter()
        .find(|scalar| equals_ignoring_case(name, scalar.as_bytes()))
}

/// The frequency `bytes` names.
fn decode_freq(bytes: &[u8]) -> Option<Freq> {
    Freq::ALL
        .into_iter()
        .find(|freq| equals_ignoring_case(bytes, freq.as_bytes()))
}

/// The weekday `bytes` names, from section 3.3.10's two-letter `weekday` production.
fn decode_weekday(bytes: &[u8]) -> Option<Weekday> {
    Weekday::ALL
        .into_iter()
        .find(|day| equals_ignoring_case(bytes, day.as_bytes()))
}

/// An unsigned decimal, refusing a sign, an empty string and anything that does not fit.
fn decode_unsigned(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut total: u32 = 0;
    for octet in bytes {
        let digit = octet.checked_sub(b'0').filter(|value| *value <= 9)?;
        total = total.checked_mul(10)?.checked_add(u32::from(digit))?;
    }
    Some(total)
}

/// A decimal with an optional `+` or `-`, as section 3.3.10 writes an ordinal.
fn decode_signed(bytes: &[u8]) -> Option<i32> {
    let (negative, digits) = match bytes.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, bytes),
    };
    let magnitude = i32::try_from(decode_unsigned(digits)?).ok()?;
    if negative {
        magnitude.checked_neg()
    } else {
        Some(magnitude)
    }
}

/// A signed value within `±bound`, never zero — section 3.3.10's shape for every ordinal list.
///
/// Zero is refused for the same reason the RFC excludes it: `BYMONTHDAY=0` names no day and
/// `BYSETPOS=0` names no position, so admitting it would carry a value with no meaning into
/// an expansion that would have to invent one.
fn decode_ordinal(bytes: &[u8], bound: i32) -> Option<i32> {
    let value = decode_signed(bytes)?;
    (value != 0 && value.unsigned_abs() <= bound.unsigned_abs()).then_some(value)
}

/// An unsigned value within `0..=bound`.
fn decode_bounded(bytes: &[u8], bound: u32) -> Option<u32> {
    decode_unsigned(bytes).filter(|value| *value <= bound)
}

/// Read a `,`-separated list, refusing the whole list when any element is refused.
///
/// All or nothing per part, because a half-read `BYMONTHDAY` would silently answer a different
/// question than the file asked, and the property's own text is still there for the caller.
fn decode_list<T, F>(bytes: &[u8], mut element: F) -> Option<ByList<T>>
where
    F: FnMut(&[u8]) -> Option<T>,
{
    if bytes.is_empty() {
        return None;
    }
    let mut values: Vec<T> = Vec::new();
    for item in bytes.split(|octet| *octet == b',') {
        values.push(element(item)?);
    }
    Some(ByList::from(values))
}

/// Read one `BYDAY` entry: an optional ±ordinal followed by a two-letter weekday.
fn decode_weekday_num(bytes: &[u8]) -> Option<WeekdayNum> {
    let split = bytes.len().checked_sub(2)?;
    let (ordinal_text, day_text) = (bytes.get(..split)?, bytes.get(split..)?);
    let weekday = decode_weekday(day_text)?;
    if ordinal_text.is_empty() {
        return WeekdayNum::new(None, weekday);
    }
    let ordinal = decode_ordinal(ordinal_text, i32::from(WeekdayNum::MAX_ORDINAL))?;
    let ordinal = NonZeroI8::new(i8::try_from(ordinal).ok()?)?;
    WeekdayNum::new(Some(ordinal), weekday)
}

/// Read an `UNTIL`, saying which clock the file wrote it on.
///
/// A `DATE` and a floating `DATE-TIME` are both read *at UTC*, which is a decision and not a
/// resolution: this crate has no zone and may not acquire one. [`UntilClock`] records which of
/// the two happened so that nothing downstream compares an instant whose clock it never asked
/// about.
fn decode_until(bytes: &[u8]) -> Option<RuleLimit> {
    let written = DateTimeValue::decode_value(bytes).ok()?;
    let time = written.time().unwrap_or(CivilTime::MIDNIGHT);
    let at = CivilDateTime::new(written.date(), time).at_offset(UtcOffset::UTC)?;
    let (value_kind, clock) = match written {
        DateTimeValue::Date(_) => (ValueKind::Date, UntilClock::Floating),
        DateTimeValue::Local(_) => (ValueKind::DateTime, UntilClock::Floating),
        DateTimeValue::Utc(_) => (ValueKind::DateTime, UntilClock::Utc),
        // A `TZID` is a property parameter and a `RECUR` value has none, so this shape cannot
        // arrive from `decode_value`. Refusing rather than guessing keeps that true if the
        // set of shapes ever grows.
        DateTimeValue::Zoned { .. } => return None,
    };
    Some(RuleLimit::Until {
        at,
        value_kind,
        clock,
    })
}

/// The state one strict decode accumulates, so that the pair walk stays a loop over pairs.
///
/// A struct rather than thirteen locals because this crate's Clippy profile bounds a function
/// at a cognitive complexity of 15 and the walk would otherwise carry the whole rule in its
/// own frame.
#[derive(Debug)]
struct StrictDecode {
    /// The builder, complete except for the parts not yet seen.
    builder: Option<RecurrenceRuleBuilder>,
    /// Names already seen, to refuse a repeat.
    seen: Vec<u32>,
}

impl StrictDecode {
    /// Nothing read yet.
    fn new() -> Self {
        Self {
            builder: None,
            seen: Vec::new(),
        }
    }

    /// Record `name`, or say it was already there.
    fn note(&mut self, name: u32) -> Result<(), DiagnosticCode> {
        if self.seen.contains(&name) {
            return Err(DiagnosticCode::DuplicateRecurrenceRulePart);
        }
        self.seen.push(name);
        Ok(())
    }
}

/// `FREQ`'s number in the flat numbering [`StrictDecode::seen`] keeps.
///
/// Past the nine [`RulePart::index`] supplies and below the four [`ScalarPart::slot`] does, so
/// that one list holds all fourteen names and no two of them collide.
const FREQ_SLOT: u32 = 16;

/// Apply one already-recognized pair to the decode in progress.
///
/// Split from the walk so that neither exceeds this workspace's cognitive-complexity bound,
/// and split *by shape of value* rather than by part name so that the seven list parts share
/// one line each.
fn apply(state: &mut StrictDecode, part: RulePartText<'_>) -> Result<(), DiagnosticCode> {
    let malformed = DiagnosticCode::MalformedRecurrenceRule;
    let range = DiagnosticCode::RecurrenceRulePartOutOfRange;
    let text = part.value();
    if equals_ignoring_case(part.name(), FREQ_NAME) {
        state.note(FREQ_SLOT)?;
        let freq = decode_freq(text).ok_or(malformed)?;
        state.builder = Some(RecurrenceRuleBuilder::new(freq));
        return Ok(());
    }
    let builder = state.builder.take().ok_or(malformed)?;
    let updated = apply_to_builder(state, builder, part).map_err(|code| {
        // A part that named a legal thing badly and a part that named an illegal value are
        // different reports, and both leave the property's own octets untouched.
        if code == range { range } else { code }
    })?;
    state.builder = Some(updated);
    Ok(())
}

/// Apply one pair to a builder that already knows its frequency.
///
/// Reaching this function at all means `FREQ` came first, which the strict reading requires and
/// section 3.3.10 does not: its grammar says the rule parts "are not ordered in any particular
/// sequence". The requirement is a consequence of applying each pair as it arrives — a `BYDAY`
/// ordinal's meaning depends on the frequency through the table's two notes, so there is nothing
/// to apply a part *to* until one is known. [`parse_recur`] buys the RFC's order back by
/// collecting every pair before it decodes one; the price is holding thirteen slices, and the
/// strict reading declines to pay it because refusing is its whole job.
fn apply_to_builder(
    state: &mut StrictDecode,
    builder: RecurrenceRuleBuilder,
    part: RulePartText<'_>,
) -> Result<RecurrenceRuleBuilder, DiagnosticCode> {
    let text = part.value();
    if let Some(row) = part.by_part() {
        state.note(u32::try_from(row.index()).unwrap_or(u32::MAX))?;
        return apply_by_part(builder, row, text);
    }
    // `FREQ` cannot arrive here: `apply` answers it before a builder exists, and a repeat of it
    // is refused there as the duplicate it is.
    let scalar = scalar_part(part.name()).ok_or(DiagnosticCode::UnknownRecurrenceRulePart)?;
    state.note(scalar.slot())?;
    apply_scalar_part(builder, scalar, text)
}

/// Apply one of the four scalar parts, with the range RFC 5545 section 3.3.10 gives it.
///
/// Split out of the walk rather than inlined into it because both readings need it and neither
/// may own a second copy. A value whose *shape* is wrong is [`DiagnosticCode`]'s malformed and a
/// value whose *range* is wrong is its out-of-range; the strict reading refuses the whole value
/// either way, and [`parse_recur`] narrows both to one code because a part it dropped is a part
/// the rule survived.
fn apply_scalar_part(
    builder: RecurrenceRuleBuilder,
    scalar: ScalarPart,
    text: &[u8],
) -> Result<RecurrenceRuleBuilder, DiagnosticCode> {
    let malformed = DiagnosticCode::MalformedRecurrenceRule;
    let range = DiagnosticCode::RecurrenceRulePartOutOfRange;
    Ok(match scalar {
        ScalarPart::Until => builder.limit(decode_until(text).ok_or(malformed)?),
        ScalarPart::Count => {
            let count = decode_unsigned(text).ok_or(malformed)?;
            builder.limit(RuleLimit::Count(NonZeroU32::new(count).ok_or(range)?))
        },
        ScalarPart::Interval => {
            let interval = decode_unsigned(text).ok_or(malformed)?;
            builder.interval(NonZeroU32::new(interval).ok_or(range)?)
        },
        ScalarPart::Wkst => builder.wkst(decode_weekday(text).ok_or(malformed)?),
    })
}

/// Apply one of the nine `BYxxx` parts, each with the range RFC 5545 section 3.3.10 gives it.
///
/// The ranges are the specification and are written once, here, as a `match` over the same
/// nine-variant enum the expand/limit table is indexed by — so a part that gains a row there
/// and no arm here does not compile.
fn apply_by_part(
    builder: RecurrenceRuleBuilder,
    row: RulePart,
    text: &[u8],
) -> Result<RecurrenceRuleBuilder, DiagnosticCode> {
    let bad = DiagnosticCode::RecurrenceRulePartOutOfRange;
    let short = |value: i32| i8::try_from(value).ok();
    let narrow = |value: u32| u8::try_from(value).ok();
    let wide = |value: i32| i16::try_from(value).ok();
    Ok(match row {
        RulePart::Second => builder.by_second(
            decode_list(text, |item| decode_bounded(item, 60).and_then(narrow)).ok_or(bad)?,
        ),
        RulePart::Minute => builder.by_minute(
            decode_list(text, |item| decode_bounded(item, 59).and_then(narrow)).ok_or(bad)?,
        ),
        RulePart::Hour => builder.by_hour(
            decode_list(text, |item| decode_bounded(item, 23).and_then(narrow)).ok_or(bad)?,
        ),
        RulePart::Day => builder.by_day(decode_list(text, decode_weekday_num).ok_or(bad)?),
        RulePart::MonthDay => builder.by_month_day(
            decode_list(text, |item| decode_ordinal(item, 31).and_then(short)).ok_or(bad)?,
        ),
        RulePart::YearDay => builder.by_year_day(
            decode_list(text, |item| decode_ordinal(item, 366).and_then(wide)).ok_or(bad)?,
        ),
        RulePart::WeekNo => builder.by_week_no(
            decode_list(text, |item| decode_ordinal(item, 53).and_then(short)).ok_or(bad)?,
        ),
        RulePart::Month => builder.by_month(
            decode_list(text, |item| {
                decode_bounded(item, 12)
                    .filter(|month| *month != 0)
                    .and_then(narrow)
            })
            .ok_or(bad)?,
        ),
        RulePart::SetPos => builder.by_set_pos(
            decode_list(text, |item| decode_ordinal(item, 366).and_then(wide)).ok_or(bad)?,
        ),
    })
}

/// The diagnostic code a construction failure travels on.
const fn code_for(error: RuleError) -> DiagnosticCode {
    match error {
        RuleError::MissingFrequency | RuleError::UnknownFrequency => {
            DiagnosticCode::MalformedRecurrenceRule
        },
        RuleError::OrdinalOutOfRange => DiagnosticCode::RecurrenceRulePartOutOfRange,
        RuleError::BySetPosWithoutByRule => DiagnosticCode::BySetPosWithoutByRule,
    }
}

impl DecodeValue<'_> for RecurrenceRule {
    /// Read a `RECUR` value strictly: any deviation from RFC 5545 section 3.3.10 is refused.
    ///
    /// Refused, not discarded. The caller receives `View::Malformed` carrying this code beside
    /// the property with its octets intact, which is what `docs/adr/0001` asks of every value
    /// type. A caller that wants the surviving parts of a nearly-legal rule uses the lenient
    /// reading, which needs a sink this signature cannot carry.
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        let mut state = StrictDecode::new();
        for part in parts(bytes) {
            apply(&mut state, part)?;
        }
        let builder = state
            .builder
            .ok_or(DiagnosticCode::MalformedRecurrenceRule)?;
        builder.build().map_err(code_for)
    }
}

/// Which slot of RFC 5545 section 3.3.10's grammar a part name occupies.
///
/// Three arms because the grammar has three kinds of name: the one that makes a rule, the nine
/// that are rows of the expand/limit table, and the four that are neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// `FREQ`.
    Freq,
    /// One of the nine rows of the expand/limit table.
    Row(RulePart),
    /// One of the four scalar parts.
    Scalar(ScalarPart),
}

/// The slot `part` names, `None` for a name section 3.3.10 does not define.
fn slot_of(part: RulePartText<'_>) -> Option<Slot> {
    if equals_ignoring_case(part.name(), FREQ_NAME) {
        return Some(Slot::Freq);
    }
    if let Some(row) = part.by_part() {
        return Some(Slot::Row(row));
    }
    scalar_part(part.name()).map(Slot::Scalar)
}

/// The text every rule part was last written with, and how far into the value that was.
///
/// Collected before anything is decoded, which is what lets a repeated part resolve the way
/// [`DiagnosticCode::DuplicateRecurrenceRulePart`] says it does — the last occurrence wins. A
/// reading that applied parts as it met them could not give that answer for `FREQ`, because
/// `FREQ` constructs the builder the other twelve are applied to and [`RecurrenceRuleBuilder`]
/// offers no way to restate a frequency. Collecting first is also what makes section 3.3.10's
/// own sentence true here: the rule parts are not ordered in any particular sequence.
#[derive(Debug)]
struct LenientParts<'a> {
    /// `FREQ`.
    freq: Option<&'a [u8]>,
    /// One slot per row of the expand/limit table, indexed by [`RulePart::index`].
    rows: [Option<&'a [u8]>; RulePart::COUNT],
    /// One slot per scalar part, indexed by [`ScalarPart::index`], with the pair's position.
    ///
    /// The position is kept for one pair of names. `UNTIL` and `COUNT` are one field of a rule
    /// and section 3.3.10 forbids a value from stating both, but a value that states both
    /// anyway still has to resolve to something. Applying the scalar parts in the order the
    /// value wrote them makes the later one win, which is the answer the strict reading reaches
    /// by construction rather than an order this file chose for itself.
    scalars: [Option<(&'a [u8], u32)>; ScalarPart::COUNT],
}

impl<'a> LenientParts<'a> {
    /// A position past any a walk can assign, so an absent part sorts last and applies never.
    const UNWRITTEN: u32 = u32::MAX;

    /// Nothing collected yet.
    const fn new() -> Self {
        Self {
            freq: None,
            rows: [None; RulePart::COUNT],
            scalars: [None; ScalarPart::COUNT],
        }
    }

    /// Record `text` under `slot` as the `at`th pair, saying whether the slot was already full.
    ///
    /// `get_mut` rather than an index because the expression is then total on its face; the
    /// index comes from the enum each array is sized by, so the slot is always there.
    fn put(&mut self, slot: Slot, text: &'a [u8], at: u32) -> bool {
        match slot {
            Slot::Freq => self.freq.replace(text).is_some(),
            Slot::Row(row) => self
                .rows
                .get_mut(row.index())
                .is_some_and(|cell| cell.replace(text).is_some()),
            Slot::Scalar(scalar) => self
                .scalars
                .get_mut(scalar.index())
                .is_some_and(|cell| cell.replace((text, at)).is_some()),
        }
    }

    /// The text `row` was last written with.
    fn row_text(&self, row: RulePart) -> Option<&'a [u8]> {
        self.rows.get(row.index()).copied().flatten()
    }

    /// The text `scalar` was last written with.
    fn scalar_text(&self, scalar: ScalarPart) -> Option<&'a [u8]> {
        self.scalars
            .get(scalar.index())
            .copied()
            .flatten()
            .map(|(text, _)| text)
    }

    /// How far into the value `scalar` was last written, [`Self::UNWRITTEN`] when it was not.
    fn scalar_position(&self, scalar: ScalarPart) -> u32 {
        self.scalars
            .get(scalar.index())
            .copied()
            .flatten()
            .map_or(Self::UNWRITTEN, |(_, at)| at)
    }
}

/// Read a `RECUR` value leniently: drop the part that cannot be used, report it, keep the rule.
///
/// The counterpart of the [`DecodeValue`] implementation above, over the same [`parts`] walk and
/// the same decoders. Where the strict reading refuses the whole value at the first deviation,
/// this one drops the offending part, names it on `sink`, and hands back the rule the rest of
/// the value describes — which is what a caller rendering a calendar it did not write wants,
/// because one producer's `BYMONTHDAY=32` should not cost a series its `FREQ`.
///
/// A dropped part is *absent*, never nudged. `BYMONTHDAY=32` names no day of any month and
/// `INTERVAL=0` names no cadence; moving either to a nearby legal value would answer a question
/// the file did not ask. What survives is the rule with that part unwritten, so `INTERVAL=0`
/// leaves the 1 section 3.3.10 gives a rule that states no interval, and `COUNT=0` leaves a
/// series with no count — which is a longer series than the file asked for and still a smaller
/// claim than inventing a bound nobody wrote. Both are bounded downstream by the window and the
/// candidate budget a search is charged, which is where `docs/adr/0002` puts that job.
///
/// Four things are reported and survived: a part out of range or unreadable
/// ([`DiagnosticCode::RecurrenceRulePartOutOfRange`]), a part written twice
/// ([`DiagnosticCode::DuplicateRecurrenceRulePart`], the last occurrence winning), a name the
/// grammar does not define ([`DiagnosticCode::UnknownRecurrenceRulePart`], a note rather than a
/// violation because a later specification may define it), and `BYSETPOS` with no other `BYxxx`
/// part to select from ([`DiagnosticCode::BySetPosWithoutByRule`]). One thing is not: a value
/// with no usable `FREQ` describes no series, and there is nothing left to hand back.
///
/// `meter` is here for the refusals a sink is entitled to make, which [`report_diagnostic`]
/// counts outside it — a caller passing `IgnoreDiagnostics` loses which violations occurred and
/// never that they did. This reading charges no octets. They were charged by whoever read the
/// property; the work is linear in a value the reader already bounded; and [`RuleError`] carries
/// no way to say a budget was crossed, so charging one here would be a bound this signature
/// cannot report.
pub fn parse_recur<S: DiagnosticSink + ?Sized>(
    value_text: &[u8],
    meter: &mut Meter,
    sink: &mut S,
) -> Result<RecurrenceRule, RuleError> {
    let collected = collect_parts(value_text, meter, sink);
    let freq = resolve_freq(&collected, meter, sink)?;
    let builder = apply_collected(freq, &collected, meter, sink);
    let rule = finish(builder, meter, sink)?;
    report_forbidden_ordinals(&rule, meter, sink);
    Ok(rule)
}

/// Report a `BYDAY` ordinal under a frequency RFC 5545 section 3.3.10 forbids one under.
///
/// "The BYDAY rule part MUST NOT be specified with a numeric value when the FREQ rule part is
/// not set to MONTHLY or YEARLY." The entry itself is kept and the ordinal is ignored, which is
/// what the expansion does under all five of those frequencies: the entry still names its
/// weekday, and that is all the rule can be read to have meant. Reported rather than honored
/// silently, because the other readings are not equivalent — resolving `2TU` inside a week names
/// no day at all, and a rule that quietly expands to nothing is the silence `docs/adr/0009`
/// forbids. It travels on the code a part out of its stated range travels on, since an ordinal
/// is exactly a value outside the range this frequency gives `BYDAY`.
///
/// Asked of the built rule rather than of the text, so the answer does not depend on whether
/// `FREQ` was written before `BYDAY`.
fn report_forbidden_ordinals<S: DiagnosticSink + ?Sized>(
    rule: &RecurrenceRule,
    meter: &mut Meter,
    sink: &mut S,
) {
    if matches!(rule.freq(), Freq::Monthly | Freq::Yearly) {
        return;
    }
    if rule
        .by_day()
        .as_slice()
        .iter()
        .any(|entry| entry.ordinal().is_some())
    {
        report(
            sink,
            meter,
            DiagnosticCode::RecurrenceRulePartOutOfRange,
            Severity::Violation,
        );
    }
}

/// Offer `code` to `sink`, charging a refusal to `meter`.
///
/// Every diagnostic this reading emits is at [`Location::NOWHERE`], and that is a statement
/// rather than an omission. `value_text` is an unfolded value the caller assembled and not the
/// file it was read from, so a span into it addresses a buffer this crate was never handed; a
/// plausible-looking offset into the wrong buffer is worse than admitting there is none, which
/// is the answer `ical-core`'s own property accessors reached.
fn report<S: DiagnosticSink + ?Sized>(
    sink: &mut S,
    meter: &mut Meter,
    code: DiagnosticCode,
    severity: Severity,
) {
    report_diagnostic(
        sink,
        meter,
        Diagnostic::new(code, severity, Location::NOWHERE),
    );
}

/// Walk every pair once, keeping the text last written under each name.
fn collect_parts<'a, S: DiagnosticSink + ?Sized>(
    value_text: &'a [u8],
    meter: &mut Meter,
    sink: &mut S,
) -> LenientParts<'a> {
    let mut collected = LenientParts::new();
    for (position, part) in parts(value_text).enumerate() {
        let at = u32::try_from(position).unwrap_or(LenientParts::UNWRITTEN);
        let Some(slot) = slot_of(part) else {
            // Section 3.3.10's grammar is closed, but a name it does not define is
            // indistinguishable from one a later specification added, and the rest of the value
            // still describes a series this crate can expand.
            report(
                sink,
                meter,
                DiagnosticCode::UnknownRecurrenceRulePart,
                Severity::Note,
            );
            continue;
        };
        if collected.put(slot, part.value(), at) {
            report(
                sink,
                meter,
                DiagnosticCode::DuplicateRecurrenceRulePart,
                Severity::Violation,
            );
        }
    }
    collected
}

/// The frequency the collected `FREQ` names, or the one reason there is no rule at all.
fn resolve_freq<S: DiagnosticSink + ?Sized>(
    collected: &LenientParts<'_>,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<Freq, RuleError> {
    let Some(text) = collected.freq else {
        return Err(refuse(RuleError::MissingFrequency, meter, sink));
    };
    decode_freq(text).ok_or_else(|| refuse(RuleError::UnknownFrequency, meter, sink))
}

/// Report `error` on its own code and hand it back, so one refusal is stated once.
fn refuse<S: DiagnosticSink + ?Sized>(
    error: RuleError,
    meter: &mut Meter,
    sink: &mut S,
) -> RuleError {
    report(sink, meter, code_for(error), Severity::Violation);
    error
}

/// Apply every collected part to a builder at `freq`, dropping and reporting what it cannot use.
fn apply_collected<S: DiagnosticSink + ?Sized>(
    freq: Freq,
    collected: &LenientParts<'_>,
    meter: &mut Meter,
    sink: &mut S,
) -> RecurrenceRuleBuilder {
    let mut builder = RecurrenceRuleBuilder::new(freq);
    if collected.scalar_text(ScalarPart::Until).is_some()
        && collected.scalar_text(ScalarPart::Count).is_some()
    {
        // Section 3.3.10: "The UNTIL or COUNT rule parts are OPTIONAL, but they MUST NOT occur
        // in the same 'recur'." `RuleLimit` holds one bound, so the loop below silently lets the
        // later part win. Silently is the part that is not acceptable: the caller is being
        // handed a series whose end it can check but never asked about.
        report(
            sink,
            meter,
            DiagnosticCode::MutuallyExclusiveRuleParts,
            Severity::Violation,
        );
    }
    // The rows in the RFC's own order, which is the order the expand/limit table prints them.
    // Order is immaterial among the rows — each is a separate field of the rule — and stating
    // the RFC's anyway keeps this loop diffable against the same table the engine is driven by.
    for row in RulePart::ALL {
        if let Some(text) = collected.row_text(row) {
            let refused = keep_or_drop(&mut builder, |current| apply_by_part(current, row, text));
            report_dropped_part(refused, meter, sink);
        }
    }
    // The scalars in the order the *value* wrote them, because `UNTIL` and `COUNT` share one
    // field and a value that states both has to resolve the same way in both readings.
    let mut ordered = ScalarPart::ALL;
    ordered.sort_unstable_by_key(|entry| collected.scalar_position(*entry));
    for scalar in ordered {
        if let Some(text) = collected.scalar_text(scalar) {
            let refused = keep_or_drop(&mut builder, |current| {
                apply_scalar_part(current, scalar, text)
            });
            report_dropped_part(refused, meter, sink);
        }
    }
    builder
}

/// Apply one part, leaving `builder` as it was when the part cannot be read.
///
/// [`apply_by_part`] and [`apply_scalar_part`] consume their builder on the refusing path as
/// well as on the accepting one, which the strict reading never notices because it stops at the
/// first refusal. This reading keeps going, so it leaves a copy behind before it hands the
/// builder over and puts that copy back when the part is refused. One copy per rule part, and a
/// rule has at most thirteen of those however many times a producer repeated one — the walk
/// collected a slot per name before any of this ran, so the cost is a bounded multiple of the
/// value's own size rather than a function of how hostile it is.
fn keep_or_drop<F>(builder: &mut RecurrenceRuleBuilder, apply: F) -> Option<DiagnosticCode>
where
    F: FnOnce(RecurrenceRuleBuilder) -> Result<RecurrenceRuleBuilder, DiagnosticCode>,
{
    let kept = builder.clone();
    let offered = core::mem::replace(builder, kept);
    match apply(offered) {
        Ok(updated) => {
            *builder = updated;
            None
        },
        Err(code) => Some(code),
    }
}

/// Report a refused part, on the one code the golden list gives a part that was dropped.
///
/// [`DiagnosticCode::MalformedRecurrenceRule`] means "nothing usable could be read at all",
/// which is exactly what a dropped part is not — the rule is still here. The shared decoders
/// raise it for a value whose shape is wrong rather than its range because the strict reading,
/// which is refusing the whole value, is entitled to that distinction. This reading is not, so
/// every refusal of a single part arrives as [`DiagnosticCode::RecurrenceRulePartOutOfRange`].
fn report_dropped_part<S: DiagnosticSink + ?Sized>(
    refused: Option<DiagnosticCode>,
    meter: &mut Meter,
    sink: &mut S,
) {
    if let Some(code) = refused {
        // Narrowed one code rather than collapsed to one, so that a decoder taught to refuse a
        // part for some third reason reports that reason instead of this one.
        let dropped = if code == DiagnosticCode::MalformedRecurrenceRule {
            DiagnosticCode::RecurrenceRulePartOutOfRange
        } else {
            code
        };
        report(sink, meter, dropped, Severity::Violation);
    }
}

/// Build the rule, dropping `BYSETPOS` when that is what stands in the way.
///
/// The one condition [`RecurrenceRuleBuilder::build`] refuses is `BYSETPOS` with no other
/// `BYxxx` part to select from, and dropping the part is what keeps the rest of the rule — the
/// same answer this reading gives every other part it cannot use. The second build's own error
/// is propagated rather than assumed away: a condition added to `build` later reaches the caller
/// instead of being quietly un-refused here, and nothing in this file has to be right about what
/// `build` checks for that to hold.
fn finish<S: DiagnosticSink + ?Sized>(
    builder: RecurrenceRuleBuilder,
    meter: &mut Meter,
    sink: &mut S,
) -> Result<RecurrenceRule, RuleError> {
    match builder.clone().build() {
        Ok(rule) => Ok(rule),
        Err(error) => {
            report(sink, meter, code_for(error), Severity::Violation);
            builder.by_set_pos(ByList::empty()).build()
        },
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::NonZeroU32;

    use ical_core::{
        DecodeValue, Diagnostic, DiagnosticCode, IgnoreDiagnostics, Instant, Limits, Meter,
        Severity, Weekday,
    };

    use super::{RulePartText, parse_recur, parts};
    use crate::rule::{
        Freq, RecurrenceRule, RuleError, RuleLimit, RulePart, UntilClock, ValueKind,
    };

    /// Every `RECUR` value RFC 5545 prints, transcribed beside this crate's other fixtures.
    const RFC_VALUES: &[u8] = include_bytes!("../tests/fixtures/rfc5545-recur-values.txt");

    /// How many values that file holds, so a fixture that lost a line fails instead of passing.
    const RFC_VALUE_COUNT: usize = 44;

    fn decode(text: &[u8]) -> Result<RecurrenceRule, DiagnosticCode> {
        RecurrenceRule::decode_value(text)
    }

    /// Read `text` leniently, handing back the rule and every code it reported, in order.
    fn lenient(text: &[u8]) -> (Result<RecurrenceRule, RuleError>, Vec<DiagnosticCode>) {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut kept: Vec<Diagnostic> = Vec::new();
        let rule = parse_recur(text, &mut meter, &mut kept);
        (rule, kept.iter().map(|found| found.code()).collect())
    }

    /// The fixture's values, with its own comments and blank lines dropped.
    fn rfc_values() -> Vec<&'static [u8]> {
        RFC_VALUES
            .split(|octet| *octet == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .filter(|line| !line.is_empty() && !line.starts_with(b"#"))
            .collect()
    }

    #[test]
    fn the_example_rules_of_section_3_3_10_decode() {
        let rule = decode(b"FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1").unwrap();
        assert_eq!(rule.freq(), Freq::Monthly);
        assert_eq!(rule.by_day().len(), 5);
        assert_eq!(rule.by_set_pos().as_slice(), &[-1_i16]);
        assert!(rule.has_part(RulePart::Day));

        let counted = decode(b"FREQ=DAILY;COUNT=10;INTERVAL=2").unwrap();
        assert_eq!(
            counted.limit(),
            RuleLimit::Count(NonZeroU32::new(10).unwrap())
        );
        assert_eq!(counted.interval(), NonZeroU32::new(2).unwrap());
    }

    /// Case is not part of the grammar, so a lowercase rule is the same rule.
    #[test]
    fn part_names_and_enumerated_values_are_case_insensitive() {
        let upper = decode(b"FREQ=WEEKLY;BYDAY=SU;WKST=SU").unwrap();
        let lower = decode(b"freq=weekly;byday=su;wkst=su").unwrap();
        assert_eq!(upper, lower);
        assert_eq!(upper.wkst(), Weekday::Sunday);
    }

    /// The `UNTIL` clock is recorded rather than assumed, in both of its shapes.
    #[test]
    fn until_records_which_clock_it_was_written_on() {
        let utc = decode(b"FREQ=DAILY;UNTIL=19971224T000000Z").unwrap();
        assert_eq!(
            utc.limit(),
            RuleLimit::Until {
                at: Instant::from_unix_seconds(882_921_600),
                value_kind: ValueKind::DateTime,
                clock: UntilClock::Utc,
            }
        );

        let floating = decode(b"FREQ=DAILY;UNTIL=19971224T000000").unwrap();
        assert!(matches!(
            floating.limit(),
            RuleLimit::Until {
                clock: UntilClock::Floating,
                value_kind: ValueKind::DateTime,
                ..
            }
        ));

        let date = decode(b"FREQ=DAILY;UNTIL=19971224").unwrap();
        assert!(matches!(
            date.limit(),
            RuleLimit::Until {
                value_kind: ValueKind::Date,
                ..
            }
        ));
    }

    /// Each refusal names itself, so a caller can tell a broken rule from an out-of-range one.
    #[test]
    fn each_deviation_is_refused_under_its_own_code() {
        let cases: [(&[u8], DiagnosticCode); 6] = [
            (b"BYDAY=MO", DiagnosticCode::MalformedRecurrenceRule),
            (b"FREQ=FORTNIGHTLY", DiagnosticCode::MalformedRecurrenceRule),
            (
                b"FREQ=MONTHLY;BYMONTHDAY=32",
                DiagnosticCode::RecurrenceRulePartOutOfRange,
            ),
            (
                b"FREQ=MONTHLY;BYMONTHDAY=1;BYMONTHDAY=2",
                DiagnosticCode::DuplicateRecurrenceRulePart,
            ),
            (
                b"FREQ=MONTHLY;XYZZY=1",
                DiagnosticCode::UnknownRecurrenceRulePart,
            ),
            (
                b"FREQ=MONTHLY;BYSETPOS=-1",
                DiagnosticCode::BySetPosWithoutByRule,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                decode(text).err(),
                Some(expected),
                "{}",
                core::str::from_utf8(text).unwrap()
            );
        }
    }

    /// Zero is not a legal ordinal anywhere section 3.3.10 admits one.
    #[test]
    fn zero_is_refused_wherever_the_rfc_excludes_it() {
        for text in [
            &b"FREQ=MONTHLY;BYMONTHDAY=0"[..],
            b"FREQ=YEARLY;BYMONTH=0",
            b"FREQ=DAILY;INTERVAL=0",
            b"FREQ=DAILY;COUNT=0",
            b"FREQ=MONTHLY;BYDAY=0MO",
        ] {
            assert!(
                decode(text).is_err(),
                "{}",
                core::str::from_utf8(text).unwrap()
            );
        }
    }

    /// The pair walk is public because the lenient reading has to share it.
    #[test]
    fn the_pair_walk_keeps_every_pair_including_the_ones_it_cannot_read() {
        let walked: alloc::vec::Vec<RulePartText<'_>> =
            parts(b"FREQ=DAILY;XYZZY;BYHOUR=9").collect();
        assert_eq!(walked.len(), 3);
        assert_eq!(walked[1].name(), b"XYZZY");
        assert_eq!(walked[1].value(), b"");
        assert_eq!(walked[2].by_part(), Some(RulePart::Hour));
    }

    /// The four scalar parts moved into one table and refuse exactly what they refused before.
    #[test]
    fn the_scalar_parts_keep_their_own_codes_in_the_strict_reading() {
        let cases: [(&[u8], DiagnosticCode); 5] = [
            (
                b"FREQ=DAILY;UNTIL=NOTADATE",
                DiagnosticCode::MalformedRecurrenceRule,
            ),
            (
                b"FREQ=DAILY;COUNT=x",
                DiagnosticCode::MalformedRecurrenceRule,
            ),
            (
                b"FREQ=DAILY;COUNT=0",
                DiagnosticCode::RecurrenceRulePartOutOfRange,
            ),
            (
                b"FREQ=DAILY;INTERVAL=0",
                DiagnosticCode::RecurrenceRulePartOutOfRange,
            ),
            (
                b"FREQ=DAILY;WKST=XX",
                DiagnosticCode::MalformedRecurrenceRule,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                decode(text).err(),
                Some(expected),
                "{}",
                core::str::from_utf8(text).unwrap()
            );
        }
    }

    /// A `BYDAY` ordinal is reported wherever section 3.3.10 forbids one, and kept.
    ///
    /// "The BYDAY rule part MUST NOT be specified with a numeric value when the FREQ rule part
    /// is not set to MONTHLY or YEARLY." The rule survives with its weekday — the expansion
    /// ignores the ordinal — and the violation travels, because a rule read one way by this
    /// crate and another way by the file's author is exactly what a diagnostic is for. The two
    /// frequencies that permit an ordinal report nothing.
    #[test]
    fn a_weekday_ordinal_is_reported_under_every_frequency_that_forbids_one() {
        let forbidding: [&[u8]; 5] = [
            b"FREQ=SECONDLY;BYDAY=2TU",
            b"FREQ=MINUTELY;BYDAY=2TU",
            b"FREQ=HOURLY;BYDAY=2TU",
            b"FREQ=DAILY;BYDAY=2TU",
            b"FREQ=WEEKLY;BYDAY=2TU",
        ];
        for text in forbidding {
            let (rule, codes) = lenient(text);
            assert!(rule.is_ok(), "the weekday is kept");
            assert_eq!(
                codes,
                [DiagnosticCode::RecurrenceRulePartOutOfRange],
                "{}",
                core::str::from_utf8(text).unwrap()
            );
        }

        for text in [
            b"FREQ=MONTHLY;BYDAY=2TU".as_slice(),
            b"FREQ=YEARLY;BYDAY=2TU",
        ] {
            let (rule, codes) = lenient(text);
            assert!(rule.is_ok());
            assert!(
                codes.is_empty(),
                "an ordinal is what these two frequencies are for"
            );
        }

        let (_, plain) = lenient(b"FREQ=WEEKLY;BYDAY=TU");
        assert!(plain.is_empty(), "an ordinal is the whole of the complaint");
    }

    /// Every value the RFC prints reads clean, and both readings agree on all of them.
    ///
    /// The expected column is the RFC's own corpus rather than this decoder's output, which is
    /// the whole reason it was transcribed: a test whose expectation came from the
    /// implementation asserts only that the implementation is what it is.
    #[test]
    fn every_recur_value_the_rfc_prints_reads_clean_in_both_readings() {
        let values = rfc_values();
        assert_eq!(
            values.len(),
            RFC_VALUE_COUNT,
            "the fixture gained or lost a value"
        );
        for text in values {
            let printed = core::str::from_utf8(text).unwrap();
            let strict = decode(text);
            assert!(strict.is_ok(), "strict: {printed}");
            let (relaxed, codes) = lenient(text);
            assert!(codes.is_empty(), "lenient: {printed}: {codes:?}");
            assert_eq!(relaxed.ok(), strict.ok(), "{printed}");
        }
    }

    /// The same input, two answers, both asserted.
    ///
    /// This is the whole strict/lenient split in one table: the left column is what
    /// `View::Malformed` hands a caller beside the untouched octets, and the right is what a
    /// caller that would rather have the surviving rule gets instead.
    #[test]
    fn the_lenient_reading_keeps_the_rule_the_strict_reading_refuses() {
        // Constants rather than bindings so that the expected lists below are `'static` slices
        // and the table stays one expression.
        const RANGE: DiagnosticCode = DiagnosticCode::RecurrenceRulePartOutOfRange;
        const BROKEN: DiagnosticCode = DiagnosticCode::MalformedRecurrenceRule;
        const REPEAT: DiagnosticCode = DiagnosticCode::DuplicateRecurrenceRulePart;
        const UNKNOWN: DiagnosticCode = DiagnosticCode::UnknownRecurrenceRulePart;
        const ALONE: DiagnosticCode = DiagnosticCode::BySetPosWithoutByRule;

        // The value, the one code the strict reading refuses it with, and every code the
        // lenient reading reports while keeping a rule.
        let divergences: [(&[u8], DiagnosticCode, &[DiagnosticCode]); 8] = [
            (b"FREQ=MONTHLY;BYMONTHDAY=32", RANGE, &[RANGE]),
            (b"FREQ=MONTHLY;BYMONTHDAY=1;BYMONTHDAY=2", REPEAT, &[REPEAT]),
            (b"FREQ=MONTHLY;XYZZY=1", UNKNOWN, &[UNKNOWN]),
            (b"FREQ=MONTHLY;BYSETPOS=-1", ALONE, &[ALONE]),
            // Section 3.3.10 states that the rule parts are not ordered in any particular
            // sequence, so this value conforms and only the strict reading refuses it.
            (b"BYDAY=MO;FREQ=WEEKLY", BROKEN, &[]),
            (b"FREQ=DAILY;INTERVAL=0", RANGE, &[RANGE]),
            (b"FREQ=DAILY;COUNT=0", RANGE, &[RANGE]),
            // A value whose shape is wrong rather than whose range is: one dropped part either
            // way, so the lenient reading reports the one code a dropped part travels on.
            (b"FREQ=DAILY;UNTIL=NOTADATE", BROKEN, &[RANGE]),
        ];
        for (text, refused, reported) in divergences {
            let printed = core::str::from_utf8(text).unwrap();
            assert_eq!(decode(text).err(), Some(refused), "strict: {printed}");
            let (rule, codes) = lenient(text);
            assert!(rule.is_ok(), "lenient: {printed}");
            assert_eq!(codes.as_slice(), reported, "lenient: {printed}");
        }
    }

    /// A part this reading cannot use is absent, never moved to a nearby legal value.
    #[test]
    fn a_dropped_part_is_absent_rather_than_nudged_to_a_nearby_legal_value() {
        let (past_the_month, _) = lenient(b"FREQ=MONTHLY;BYMONTHDAY=32");
        let past_the_month = past_the_month.unwrap();
        assert!(
            past_the_month.by_month_day().is_empty(),
            "32 is dropped, not read as the 31st"
        );
        assert_eq!(past_the_month.freq(), Freq::Monthly);

        let (paced, _) = lenient(b"FREQ=DAILY;INTERVAL=0");
        assert_eq!(
            paced.unwrap().interval(),
            NonZeroU32::new(1).unwrap(),
            "a dropped INTERVAL is the interval a rule that states none has"
        );

        let (counted, _) = lenient(b"FREQ=DAILY;COUNT=0");
        assert_eq!(
            counted.unwrap().limit(),
            RuleLimit::Infinite,
            "a dropped COUNT is a rule that states no count, which the window still bounds"
        );

        let (positioned, codes) = lenient(b"FREQ=MONTHLY;BYDAY=MO;BYSETPOS=-1;BYSETPOS=0");
        let positioned = positioned.unwrap();
        assert!(
            positioned.by_set_pos().is_empty(),
            "the later BYSETPOS wins and zero names no position"
        );
        assert_eq!(positioned.by_day().len(), 1);
        assert_eq!(
            codes,
            alloc::vec![
                DiagnosticCode::DuplicateRecurrenceRulePart,
                DiagnosticCode::RecurrenceRulePartOutOfRange,
            ]
        );
    }

    /// `BYSETPOS` with nothing to select from costs the part and not the rule.
    #[test]
    fn by_set_pos_alone_is_dropped_and_the_rest_of_the_rule_survives_it() {
        let (rule, codes) = lenient(b"FREQ=MONTHLY;INTERVAL=2;BYSETPOS=-1");
        let rule = rule.unwrap();
        assert!(rule.by_set_pos().is_empty());
        assert_eq!(rule.interval(), NonZeroU32::new(2).unwrap());
        assert_eq!(codes, alloc::vec![DiagnosticCode::BySetPosWithoutByRule]);
    }

    /// A repeated part resolves to its last occurrence, which is what the golden list says.
    #[test]
    fn the_last_occurrence_of_a_repeated_part_wins_including_freq() {
        let (hours, codes) = lenient(b"FREQ=DAILY;BYHOUR=9;BYHOUR=10");
        assert_eq!(hours.unwrap().by_hour().as_slice(), &[10_u8]);
        assert_eq!(
            codes,
            alloc::vec![DiagnosticCode::DuplicateRecurrenceRulePart]
        );

        // The strict reading cannot give this answer at all: it builds from the first `FREQ` it
        // meets and has nowhere to put a second one.
        let (weekly, repeated) = lenient(b"FREQ=DAILY;FREQ=WEEKLY");
        assert_eq!(weekly.unwrap().freq(), Freq::Weekly);
        assert_eq!(
            repeated,
            alloc::vec![DiagnosticCode::DuplicateRecurrenceRulePart]
        );
        assert_eq!(
            decode(b"FREQ=DAILY;FREQ=WEEKLY").err(),
            Some(DiagnosticCode::DuplicateRecurrenceRulePart)
        );
    }

    /// `UNTIL` and `COUNT` are one field, the later text wins, and the lenient reading says so.
    ///
    /// RFC 5545 section 3.3.10 forbids a value from carrying both. `RuleLimit` holds one bound,
    /// so a rule cannot represent the violation and one part necessarily wins; what this asserts
    /// is that the two readings agree on *which*, and that the agreement is not reached in
    /// silence. The strict reading has no sink and reports nothing, which is the one asymmetry
    /// between them and is a property of its signature rather than of section 3.3.10.
    #[test]
    fn a_value_carrying_both_until_and_count_resolves_the_same_way_in_both_readings() {
        let orders: [&[u8]; 2] = [
            b"FREQ=DAILY;UNTIL=19971224T000000Z;COUNT=10",
            b"FREQ=DAILY;COUNT=10;UNTIL=19971224T000000Z",
        ];
        for text in orders {
            let printed = core::str::from_utf8(text).unwrap();
            let (rule, codes) = lenient(text);
            assert_eq!(
                codes,
                alloc::vec![DiagnosticCode::MutuallyExclusiveRuleParts],
                "{printed}"
            );
            assert_eq!(
                rule.ok().map(|kept| kept.limit()),
                decode(text).ok().map(|kept| kept.limit()),
                "{printed}"
            );
        }
        let (later, _) = lenient(b"FREQ=DAILY;COUNT=10;UNTIL=19971224T000000Z");
        assert!(
            matches!(later.unwrap().limit(), RuleLimit::Until { .. }),
            "the text written later is the limit that stands"
        );
    }

    /// The edges of the calendar are ranges here, and only ranges.
    ///
    /// This reading owns no calendar. `BYMONTHDAY=31` under `FREQ=MONTHLY` names a day February
    /// does not have, and section 3.3.10 says such an instance is ignored per period — the
    /// expansion's answer, not this one's. What belongs here is the range check, and the leap
    /// day, the month end and the year boundary all live in it.
    #[test]
    fn the_month_end_the_leap_day_and_the_year_boundary_are_ranges_and_not_special_cases() {
        let (last, _) = lenient(b"FREQ=MONTHLY;BYMONTHDAY=-1");
        assert_eq!(last.unwrap().by_month_day().as_slice(), &[-1_i8]);

        let (thirty_first, codes) = lenient(b"FREQ=MONTHLY;BYMONTHDAY=31");
        assert_eq!(thirty_first.unwrap().by_month_day().as_slice(), &[31_i8]);
        assert!(
            codes.is_empty(),
            "a day February lacks is still a day the rule may name"
        );

        let (leap, _) = lenient(b"FREQ=YEARLY;BYYEARDAY=366");
        assert_eq!(leap.unwrap().by_year_day().as_slice(), &[366_i16]);
        let (excess, refused) = lenient(b"FREQ=YEARLY;BYYEARDAY=367");
        assert!(excess.unwrap().by_year_day().is_empty());
        assert_eq!(
            refused,
            alloc::vec![DiagnosticCode::RecurrenceRulePartOutOfRange]
        );

        let (boundary, _) = lenient(b"FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=31");
        let boundary = boundary.unwrap();
        assert_eq!(boundary.by_month().as_slice(), &[12_u8]);
        assert_eq!(boundary.by_month_day().as_slice(), &[31_i8]);

        let (leap_day, _) = lenient(b"FREQ=YEARLY;UNTIL=20240229T000000Z");
        assert_eq!(
            leap_day.unwrap().limit(),
            RuleLimit::Until {
                at: Instant::from_unix_seconds(1_709_164_800),
                value_kind: ValueKind::DateTime,
                clock: UntilClock::Utc,
            }
        );
    }

    /// Case is not part of the grammar in the lenient reading either.
    #[test]
    fn a_lowercase_value_is_the_same_rule_as_an_uppercase_one() {
        let (lower, codes) = lenient(b"freq=weekly;byday=su;wkst=su;bysetpos=-1");
        let (upper, _) = lenient(b"FREQ=WEEKLY;BYDAY=SU;WKST=SU;BYSETPOS=-1");
        assert_eq!(lower.unwrap(), upper.unwrap());
        assert!(codes.is_empty());
    }

    /// A sink that keeps nothing still leaves a mark, which is what the meter is here for.
    #[test]
    fn a_sink_that_refuses_everything_still_counts_what_it_refused() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let rule = parse_recur(
            b"FREQ=DAILY;BYHOUR=99;BYMINUTE=99;XYZZY=1;BYHOUR=8",
            &mut meter,
            &mut IgnoreDiagnostics,
        )
        .unwrap();
        assert_eq!(
            rule.by_hour().as_slice(),
            &[8_u8],
            "the later BYHOUR wins and it is in range"
        );
        assert!(rule.by_minute().is_empty());
        assert_eq!(
            meter.diagnostics_dropped(),
            3,
            "one undefined name, one repeated part, one value out of range"
        );
        assert!(
            !meter.is_exhausted(),
            "this reading charges no octets, so it cannot exhaust a budget it never spends"
        );
    }

    /// An undefined name claims less than a violated range, and a sink can tell them apart.
    #[test]
    fn an_unknown_part_is_a_note_and_everything_else_is_a_violation() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut kept: Vec<Diagnostic> = Vec::new();
        let rule = parse_recur(b"FREQ=DAILY;XYZZY=1;BYHOUR=99", &mut meter, &mut kept).unwrap();
        assert_eq!(rule.freq(), Freq::Daily);
        let claims: Vec<(DiagnosticCode, Severity)> = kept
            .iter()
            .map(|found| (found.code(), found.severity()))
            .collect();
        assert_eq!(
            claims,
            alloc::vec![
                (DiagnosticCode::UnknownRecurrenceRulePart, Severity::Note),
                (
                    DiagnosticCode::RecurrenceRulePartOutOfRange,
                    Severity::Violation
                ),
            ]
        );
        assert_eq!(
            meter.diagnostics_dropped(),
            0,
            "a Vec accepts every one of them"
        );
    }

    /// The one thing this reading cannot survive, and what it says on the way out.
    #[test]
    fn a_value_with_no_usable_frequency_is_the_only_refusal_that_remains() {
        let (missing, reported) = lenient(b"BYDAY=MO;COUNT=3");
        assert_eq!(missing, Err(RuleError::MissingFrequency));
        assert_eq!(
            reported,
            alloc::vec![DiagnosticCode::MalformedRecurrenceRule]
        );

        let (strange, complaint) = lenient(b"FREQ=FORTNIGHTLY;COUNT=3");
        assert_eq!(strange, Err(RuleError::UnknownFrequency));
        assert_eq!(
            complaint,
            alloc::vec![DiagnosticCode::MalformedRecurrenceRule]
        );

        // An empty value is one pair whose name is empty, and the empty name defines no part.
        let (nothing, both) = lenient(b"");
        assert_eq!(nothing, Err(RuleError::MissingFrequency));
        assert_eq!(
            both,
            alloc::vec![
                DiagnosticCode::UnknownRecurrenceRulePart,
                DiagnosticCode::MalformedRecurrenceRule,
            ]
        );
    }
}
