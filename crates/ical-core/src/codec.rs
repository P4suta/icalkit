// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading a typed value out of preserved octets, and writing one back into them.
//!
//! Specification: RFC 5545 section 3.3.
//!
//! Both directions live together on purpose. `DecodeValue` and `EncodeValue` for one value
//! type are a pair, and the parameters a written value implies are the transition table
//! `docs/adr/0001` requires be checkable for completeness against the set of typed accessors
//! — which it cannot be if the two halves are written in different places at different times.
//!
//! Three rules shape everything below.
//!
//! A decoder answers with a [`DiagnosticCode`] and never with an error, because a value this
//! crate cannot read is still a value it must write back: the octets stay where they are and
//! the caller is handed the reason next to them. Nothing here reaches for the property, and
//! nothing here caches, so there is no second place for an answer to live.
//!
//! A decoder is exact or it refuses. Every form is accepted at its full written length and no
//! prefix of one is taken for the whole, because a `DTSTART` read as the first eight octets of
//! something longer is a wrong answer that looks like a right one. Where a written form
//! carries a distinction the target type has nowhere to keep — the `Z` of a UTC `DATE-TIME`
//! against a bare [`CivilDateTime`] — the narrow type refuses and the type that can hold the
//! distinction, [`DateTimeValue`], is the one that reads it. That refusal is what "no lossy
//! decode" means in practice.
//!
//! An encoder exists only where the value determines its own text. [`Geo`] therefore decodes
//! and does not encode: `37.386013` is not recoverable from the nearest `f64`, so the stored
//! text is authoritative and the pair of floats is derived from it. A bare [`f64`] is that same
//! number arriving alone and is read the same way. [`BinaryValue`] and [`UriValue`] are the
//! other side of the rule and are writable for the same reason: each holds the text it will
//! write, so writing one spends nothing the producer chose. The date-time family is
//! writable only through [`DateTimeValue`] for the same kind of reason — the form decides the
//! `VALUE` and `TZID` parameters, and a bare [`CivilDate`] would be a second way to say
//! something that must be said once.
//!
//! Arithmetic here reads attacker-supplied digits, so every accumulation is checked and a run
//! of digits too long to fit is a malformed value rather than a wrapped one.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};
use core::str;

use ical_grammar::{DiagnosticCode, TEXT_ESCAPES};

use crate::change::ParameterEdit;
use crate::gregorian::{CivilDate, CivilDateTime, CivilTime, DateTimeValue, Duration, UtcOffset};
use crate::octets::TextError;
use crate::tree::Property;
use crate::view::{
    BinaryValue, DecodeValue, EncodeValue, Geo, MutationError, Period, TextValue, UriValue,
    ValueBuf, ValueType,
};

/// The written length of RFC 5545 section 3.3.4's `DATE`.
const DATE_LEN: usize = 8;

/// The written length of section 3.3.12's `TIME`, without the optional `Z`.
const TIME_LEN: usize = 6;

/// The written length of section 3.3.5's floating `DATE-TIME`.
const DATE_TIME_LEN: usize = 15;

/// The written length of section 3.3.5's UTC `DATE-TIME`, which is the floating form plus `Z`.
const UTC_DATE_TIME_LEN: usize = 16;

/// Seconds in a day, used only to reconcile a span whose two fields disagree in sign.
const SECONDS_PER_DAY: i64 = 86_400;

/// Days in a week, used only to fold section 3.3.6's `dur-week` into days.
const DAYS_PER_WEEK: u64 = 7;

// ---------------------------------------------------------------------------------------
// Shared octet reading
// ---------------------------------------------------------------------------------------

/// The decimal value of `bytes`, or `None` when it is empty, holds a non-digit, or does not
/// fit.
///
/// Not fitting is a refusal rather than a saturation: a `SEQUENCE` of two hundred digits is a
/// malformed value, and answering with the largest representable one would be an invention.
fn decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    for &octet in bytes {
        if !octet.is_ascii_digit() {
            return None;
        }
        // Checked immediately above, so the difference is `0..=9` and the wrapping form
        // cannot wrap. The checked accumulation below is what refuses a run too long to fit.
        let digit = u64::from(octet.wrapping_sub(b'0'));
        total = total.checked_mul(10)?.checked_add(digit)?;
    }
    Some(total)
}

/// The decimal value of the octets from `start` up to `end`, `None` when they are not there.
///
/// Bounds are asked of the slice rather than asserted, so a value cut short refuses instead
/// of reading past its end.
fn field(bytes: &[u8], start: usize, end: usize) -> Option<u64> {
    decimal(bytes.get(start..end)?)
}

/// Split the optional leading sign RFC 5545 writes before a number.
///
/// A missing sign means positive, which is what sections 3.3.6 and 3.3.8 both say.
fn split_sign(bytes: &[u8]) -> (bool, &[u8]) {
    match bytes.split_first() {
        Some((&b'-', tail)) => (true, tail),
        Some((&b'+', tail)) => (false, tail),
        _ => (false, bytes),
    }
}

/// The octets after `designator`, or `None` when it is not the one leading `bytes`.
fn after(bytes: &[u8], designator: u8) -> Option<&[u8]> {
    match bytes.split_first() {
        Some((&first, tail)) if first == designator => Some(tail),
        _ => None,
    }
}

/// Split the leading run of digits from `bytes`, together with what follows it.
fn split_digits(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let end = bytes
        .iter()
        .position(|octet| !octet.is_ascii_digit())
        .unwrap_or(bytes.len());
    let (digits, rest) = bytes.split_at(end);
    Some((decimal(digits)?, rest))
}

/// Read one `1*DIGIT` term closed by `designator`, or `None` when the next term is a
/// different one.
///
/// Returning `None` rather than an error is what lets an optional term be tried and skipped:
/// section 3.3.6's terms are ordered and each is optional, so "this is not an `H` term" and
/// "this is not a duration" have to be the same answer at this level and are separated by the
/// caller, which knows whether anything at all was read.
fn take_term(bytes: &[u8], designator: u8) -> Option<(u64, &[u8])> {
    let (count, rest) = split_digits(bytes)?;
    Some((count, after(rest, designator)?))
}

/// `total` seconds as hours, minutes and seconds.
///
/// The divisors are nonzero constants, so `None` cannot happen; it is spelled anyway because
/// unchecked division is not worth an exception in a module that reads hostile digits, and
/// the one caller reports it as a value with no RFC 5545 form.
fn split_clock(total: u64) -> Option<(u64, u64, u64)> {
    let hours = total.checked_div(3_600)?;
    let minutes = total.checked_rem(3_600)?.checked_div(60)?;
    let seconds = total.checked_rem(60)?;
    Some((hours, minutes, seconds))
}

/// Whether `octet` is a control character RFC 5545 section 3.1 excludes from a value.
///
/// `HTAB` is deliberately not one of them: section 3.1's `CONTROL` production is
/// `%x00-08 / %x0A-1F / %x7F`, and a tab inside a `DESCRIPTION` is ordinary in real exports.
const fn is_forbidden_control(octet: u8) -> bool {
    matches!(octet, 0x00..=0x08 | 0x0A..=0x1F | 0x7F)
}

/// Write formatted text into `out`.
///
/// [`ValueBuf`] cannot fail to accept octets, but [`core::fmt::Write`] says it might, so the
/// impossible arm is named rather than unwrapped. A caller sees it as a value with no RFC
/// 5545 form, which is the honest reading of "the buffer refused the text".
fn write_formatted(out: &mut ValueBuf, arguments: fmt::Arguments<'_>) -> Result<(), MutationError> {
    out.write_fmt(arguments)
        .map_err(|_error| MutationError::NotRepresentable)
}

// ---------------------------------------------------------------------------------------
// Dates and times, section 3.3.4, section 3.3.5 and section 3.3.12
// ---------------------------------------------------------------------------------------

/// Read section 3.3.4's `date-fullyear date-month date-mday`.
fn decode_date(bytes: &[u8]) -> Option<CivilDate> {
    if bytes.len() != DATE_LEN {
        return None;
    }
    let year = u16::try_from(field(bytes, 0, 4)?).ok()?;
    let month = u8::try_from(field(bytes, 4, 6)?).ok()?;
    let day = u8::try_from(field(bytes, 6, 8)?).ok()?;
    CivilDate::from_ymd(year, month, day)
}

/// Read section 3.3.12's `time-hour time-minute time-second`, without the `Z`.
///
/// The UTC form is refused here and read by [`DateTimeValue`] instead. A `Z` is a statement
/// about the zone, [`CivilTime`] has nowhere to keep one, and accepting it would make the
/// decoded value silently weaker than the text it came from.
fn decode_time(bytes: &[u8]) -> Option<CivilTime> {
    if bytes.len() != TIME_LEN {
        return None;
    }
    let hour = u8::try_from(field(bytes, 0, 2)?).ok()?;
    let minute = u8::try_from(field(bytes, 2, 4)?).ok()?;
    let second = u8::try_from(field(bytes, 4, 6)?).ok()?;
    CivilTime::from_hms(hour, minute, second)
}

/// Read section 3.3.5's `date "T" time` in its floating form.
fn decode_date_time(bytes: &[u8]) -> Option<CivilDateTime> {
    if bytes.len() != DATE_TIME_LEN {
        return None;
    }
    let date = decode_date(bytes.get(..DATE_LEN)?)?;
    let time = decode_time(after(bytes.get(DATE_LEN..)?, b'T')?)?;
    Some(CivilDateTime::new(date, time))
}

/// Read a property value that is a date or a date-time, in whichever of the three forms it
/// was written.
///
/// The three are told apart by length before anything is parsed, so a truncated date-time is
/// never mistaken for a date that happens to be a prefix of it.
fn decode_date_time_value(bytes: &[u8]) -> Option<DateTimeValue<'_>> {
    match bytes.len() {
        DATE_LEN => decode_date(bytes).map(DateTimeValue::Date),
        DATE_TIME_LEN => decode_date_time(bytes).map(DateTimeValue::Local),
        UTC_DATE_TIME_LEN if bytes.last() == Some(&b'Z') => {
            decode_date_time(bytes.get(..DATE_TIME_LEN)?).map(DateTimeValue::Utc)
        },
        _ => None,
    }
}

/// Write section 3.3.4's `DATE`.
fn encode_date(out: &mut ValueBuf, date: CivilDate) -> Result<(), MutationError> {
    write_formatted(
        out,
        format_args!("{:04}{:02}{:02}", date.year(), date.month(), date.day()),
    )
}

/// Write section 3.3.12's `TIME`, without a `Z`; the caller adds one for the UTC form.
fn encode_time(out: &mut ValueBuf, time: CivilTime) -> Result<(), MutationError> {
    write_formatted(
        out,
        format_args!("{:02}{:02}{:02}", time.hour(), time.minute(), time.second()),
    )
}

impl DecodeValue<'_> for CivilDate {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_date(bytes).ok_or(DiagnosticCode::MalformedDate)
    }
}

impl DecodeValue<'_> for CivilTime {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_time(bytes).ok_or(DiagnosticCode::MalformedTime)
    }
}

impl DecodeValue<'_> for CivilDateTime {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_date_time(bytes).ok_or(DiagnosticCode::MalformedDateTime)
    }
}

/// The zone a property's `TZID` parameter names, or `None` when it names none.
///
/// The first occurrence, and the octets with a section 3.2 `DQUOTE` pair removed, which is the
/// form a zone source is handed. A second `TZID` on one line is a defect this level cannot
/// report — a decoder answers with a value or with one code — and both stay in storage and
/// reachable through [`Property::parameters_named`].
///
/// An empty `TZID` names no zone, so it is not one: `TZID=:` reads as a floating date-time and
/// the parameter is still written back exactly as it arrived.
fn zone_of(property: &Property) -> Option<&[u8]> {
    let named = property.parameters_named(b"TZID").next()?;
    let tzid = named.unquoted();
    (!tzid.is_empty()).then_some(tzid)
}

impl<'a> DecodeValue<'a> for DateTimeValue<'a> {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
        decode_date_time_value(bytes).ok_or(DiagnosticCode::MalformedDateTime)
    }

    fn decode_property(property: &'a Property) -> Result<Self, DiagnosticCode> {
        let written = Self::decode_value(property.value_text().as_bytes())?;
        // Only a floating date-time can become a zoned one. A `TZID` beside a `DATE` or beside
        // a value ending in `Z` is what section 3.2.19 forbids, and the value's own octets are
        // the stronger statement of the two: `Z` says UTC outright, and a date has no clock for
        // a zone to move. The stray parameter is neither obeyed nor removed — it is still in
        // storage, still written back, and still there for a caller to see.
        let Self::Local(stamp) = written else {
            return Ok(written);
        };
        match zone_of(property) {
            Some(tzid) => Ok(Self::Zoned { stamp, tzid }),
            None => Ok(written),
        }
    }
}

impl EncodeValue for DateTimeValue<'_> {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        // The zone is refused here rather than where the parameter is written, so a value whose
        // zone no line could name leaves the property exactly as it was: `set` encodes before
        // it touches anything (`docs/adr/0001`).
        if !names_a_writable_zone(*self) {
            return Err(MutationError::NotRepresentable);
        }
        match *self {
            Self::Date(date) => encode_date(out, date),
            // A zoned date-time is written as the floating octets it is: section 3.3.5 gives
            // the zone no spelling inside the value, which is why it has to be a parameter and
            // why the two have to be written together.
            Self::Local(stamp) | Self::Zoned { stamp, .. } => {
                encode_date(out, stamp.date())?;
                out.push_octet(b'T');
                encode_time(out, stamp.time())
            },
            Self::Utc(stamp) => {
                encode_date(out, stamp.date())?;
                out.push_octet(b'T');
                encode_time(out, stamp.time())?;
                out.push_octet(b'Z');
                Ok(())
            },
        }
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `VALUE` and `TZID` are a function of which of the four shapes this is, and of nothing
        // the caller wrote before. A date carries no time and therefore no zone; a UTC
        // date-time carries its zone in the `Z`; a floating one asserts the absence of one; a
        // zoned one names it. Only the date needs a `VALUE` at all, `DATE-TIME` being the
        // default for every property that takes one.
        match *self {
            Self::Date(_) => {
                out.push(ParameterEdit::set(b"VALUE", ValueType::Date.as_bytes()));
                out.push(ParameterEdit::remove(b"TZID"));
            },
            Self::Local(_) | Self::Utc(_) => {
                out.push(ParameterEdit::remove(b"VALUE"));
                out.push(ParameterEdit::remove(b"TZID"));
            },
            Self::Zoned { tzid, .. } => {
                out.push(ParameterEdit::remove(b"VALUE"));
                out.push(ParameterEdit::set(b"TZID", tzid));
            },
        }
    }
}

// ---------------------------------------------------------------------------------------
// Durations, section 3.3.6
// ---------------------------------------------------------------------------------------

/// Read section 3.3.6's `dur-time`: an ordered `H`, `M`, `S` run with at least one term.
///
/// The terms are tried in the order the grammar writes them and each one consumes only what
/// it matched, so `PT1S1H` fails at the leftover rather than being reordered into something
/// the producer did not write.
fn duration_time(bytes: &[u8]) -> Option<u64> {
    let mut rest = bytes;
    let mut seconds: u64 = 0;
    let mut saw_term = false;
    for (designator, scale) in [(b'H', 3_600_u64), (b'M', 60), (b'S', 1)] {
        if let Some((count, tail)) = take_term(rest, designator) {
            seconds = seconds.checked_add(count.checked_mul(scale)?)?;
            rest = tail;
            saw_term = true;
        }
    }
    if !saw_term || !rest.is_empty() {
        return None;
    }
    Some(seconds)
}

/// Read what follows the `P` of section 3.3.6, as whole days and seconds beyond them.
///
/// A week is folded into days because [`Duration`] has no week field; the two fields it does
/// have are kept apart, so a producer that wrote `P1D` and one that wrote `PT24H` each get
/// their own spelling back when the value is written again.
fn duration_body(bytes: &[u8]) -> Option<(u64, u64)> {
    if let Some((weeks, rest)) = take_term(bytes, b'W') {
        if !rest.is_empty() {
            return None;
        }
        return Some((weeks.checked_mul(DAYS_PER_WEEK)?, 0));
    }
    let (days, rest, dated) = match take_term(bytes, b'D') {
        Some((days, rest)) => (days, rest, true),
        None => (0, bytes, false),
    };
    match after(rest, b'T') {
        Some(time) => Some((days, duration_time(time)?)),
        // A `P` with no term at all is not a duration, so the day term has to have been read
        // for an empty remainder to mean anything.
        None if dated && rest.is_empty() => Some((days, 0)),
        None => None,
    }
}

/// Read section 3.3.6's `dur-value`.
fn decode_duration(bytes: &[u8]) -> Option<Duration> {
    let (negative, unsigned) = split_sign(bytes);
    let (days, seconds) = duration_body(after(unsigned, b'P')?)?;
    let days = i64::try_from(days).ok()?;
    let seconds = i64::try_from(seconds).ok()?;
    if negative {
        // A magnitude that has no negative counterpart is a value this type cannot hold, and
        // refusing it is the only answer that does not invent a different span.
        return Some(Duration::new(days.checked_neg()?, seconds.checked_neg()?));
    }
    Some(Duration::new(days, seconds))
}

/// The same span with both fields agreeing in sign.
///
/// Section 3.3.6 writes one sign for the whole value and has nowhere to put a second, so a
/// span of one day less thirty seconds has to be reconciled before it can be written. `None`
/// when combining the two overflows, which a caller sees as a value with no RFC 5545 form
/// rather than as a wrapped one.
fn single_signed(value: Duration) -> Option<(i64, i64)> {
    let days = value.days();
    let seconds = value.seconds();
    if (days >= 0) == (seconds >= 0) {
        return Some((days, seconds));
    }
    let total = days.checked_mul(SECONDS_PER_DAY)?.checked_add(seconds)?;
    Some((
        total.checked_div(SECONDS_PER_DAY)?,
        total.checked_rem(SECONDS_PER_DAY)?,
    ))
}

/// Write section 3.3.6's `dur-time` for `total` seconds, always as all three terms.
///
/// One shape rather than the shortest one, because every branch that could be omitted is a
/// branch that could be wrong, and `T0H0M0S` is as legal as `T0S`.
fn encode_duration_time(out: &mut ValueBuf, total: u64) -> Result<(), MutationError> {
    let (hours, minutes, seconds) = split_clock(total).ok_or(MutationError::NotRepresentable)?;
    write_formatted(out, format_args!("T{hours}H{minutes}M{seconds}S"))
}

impl DecodeValue<'_> for Duration {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_duration(bytes).ok_or(DiagnosticCode::MalformedDuration)
    }
}

impl EncodeValue for Duration {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        let (days, seconds) = single_signed(*self).ok_or(MutationError::NotRepresentable)?;
        // A magnitude with no positive counterpart is a span section 3.3.6 cannot write: the
        // grammar carries the sign outside the number, so the text would state a count this
        // crate's own reader refuses at the `i64` it reads terms into. `single_signed` makes
        // the same refusal one branch earlier for a span whose two halves cannot be reconciled,
        // and this is the other end of it — an encoder exists only where the value determines
        // its own text, and no text determines this one.
        if days == i64::MIN || seconds == i64::MIN {
            return Err(MutationError::NotRepresentable);
        }
        if days < 0 || seconds < 0 {
            out.push_octet(b'-');
        }
        out.push_octet(b'P');
        if days != 0 {
            write_formatted(out, format_args!("{}D", days.unsigned_abs()))?;
        }
        // A `P` alone is not a duration, so a span of nothing is written as its time half.
        if seconds != 0 || days == 0 {
            encode_duration_time(out, seconds.unsigned_abs())?;
        }
        Ok(())
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `DURATION` is the default value type of every property that accepts one, `TRIGGER`
        // included — and `TRIGGER` is the property that can also be written as a date-time,
        // so a stale `VALUE=DATE-TIME` beside a duration is a real pairing to undo.
        out.push(ParameterEdit::remove(b"VALUE"));
    }
}

// ---------------------------------------------------------------------------------------
// Periods, section 3.3.9
// ---------------------------------------------------------------------------------------

/// Read one bound of section 3.3.9's `period`, which is a `DATE-TIME` and never a `DATE`.
///
/// A bound comes out of the octets floating or in UTC and never zoned, because section 3.3.9
/// gives the value no place to spell a zone. The only thing that can supply one is the
/// property, in [`DecodeValue::decode_property`] below.
fn decode_period_bound(bytes: &[u8]) -> Option<DateTimeValue<'static>> {
    match decode_date_time_value(bytes)? {
        DateTimeValue::Local(stamp) => Some(DateTimeValue::Local(stamp)),
        DateTimeValue::Utc(stamp) => Some(DateTimeValue::Utc(stamp)),
        // Section 3.3.9's ABNF has a `date-time` at each end and no `date`, so a bound with no
        // clock is refused rather than read as the midnight nobody wrote. A zoned bound is
        // refused because no run of octets is one.
        DateTimeValue::Date(_) | DateTimeValue::Zoned { .. } => None,
    }
}

/// Whether `span` is the positive duration section 3.3.9 requires of a `period-start`.
///
/// Reconciled first, because a span whose two fields disagree in sign is positive or negative
/// as a whole rather than field by field.
fn is_positive(span: Duration) -> bool {
    match single_signed(span) {
        Some((days, seconds)) => days > 0 || seconds > 0,
        // A span too large to reconcile has no single sign, and section 3.3.6 writes one sign
        // for the whole value.
        None => false,
    }
}

/// Whether `bound` is one section 3.3.9 can write: a date-time, and never a date.
const fn is_period_bound(bound: DateTimeValue<'_>) -> bool {
    bound.time().is_some() && names_a_writable_zone(bound)
}

/// Whether the zone this value states is one a `TZID` parameter could name it by.
///
/// The read side has already ruled on the empty one: `zone_of` says "an empty `TZID` names no
/// zone, so it is not one: `TZID=:` reads as a floating date-time". A write that emitted
/// `TZID=` for a zoned value would therefore state a zone the very next read answers `Local`
/// to — the zone the caller named, gone, with nothing returned and nothing reported. The two
/// sides get one rule, so what the reader will not read back as a zone is not a zone this
/// crate writes.
const fn names_a_writable_zone(value: DateTimeValue<'_>) -> bool {
    match value {
        DateTimeValue::Zoned { tzid, .. } => !tzid.is_empty(),
        DateTimeValue::Date(_) | DateTimeValue::Local(_) | DateTimeValue::Utc(_) => true,
    }
}

/// Whether the two bounds of an explicit period name two different zones.
///
/// A bound naming none contradicts nothing: a UTC end beside a zoned start is what a producer
/// writes when only one of the two is a wall clock, and it is written back as it arrived.
fn zones_disagree(start: DateTimeValue<'_>, end: DateTimeValue<'_>) -> bool {
    match (start.tzid(), end.tzid()) {
        (Some(first), Some(second)) => first != second,
        _ => false,
    }
}

/// Read section 3.3.9's `period-explicit / period-start`.
///
/// What follows the `/` is tried as a date-time and then as a duration, and the two cannot be
/// confused: a `dur-value` begins with a sign or a `P` and a `date-time` begins with a digit,
/// so at most one of them can match and neither half is read twice.
///
/// Whether the end precedes the start is not checked. That is a claim about time rather than
/// about the grammar — comparing a floating bound against a UTC one needs a zone this crate
/// does not resolve (`docs/adr/0003`) — and the text is written back either way.
fn decode_period(bytes: &[u8]) -> Option<Period<'static>> {
    let separator = bytes.iter().position(|&octet| octet == b'/')?;
    let start = decode_period_bound(bytes.get(..separator)?)?;
    let rest = after(bytes.get(separator..)?, b'/')?;
    if let Some(end) = decode_period_bound(rest) {
        return Some(Period::Explicit { start, end });
    }
    let duration = decode_duration(rest)?;
    if !is_positive(duration) {
        // Section 3.3.9 writes `period-start` as a start and a *positive* duration. A span
        // that runs backwards names a period whose end precedes its start, and one of no
        // length names a period that is not one.
        return None;
    }
    Some(Period::Starting { start, duration })
}

/// The same bound read under the zone `tzid` names, where it is a bound that can carry one.
///
/// Only a floating bound becomes a zoned one, exactly as for a bare date-time: a bound written
/// with a `Z` says UTC outright and section 3.2.19 forbids the `TZID` beside it. The value's
/// own octets are the stronger of the two statements, and the stray parameter is neither
/// obeyed nor removed.
const fn under_zone<'a>(bound: DateTimeValue<'a>, tzid: &'a [u8]) -> DateTimeValue<'a> {
    match bound {
        DateTimeValue::Local(stamp) => DateTimeValue::Zoned { stamp, tzid },
        held => held,
    }
}

impl<'a> DecodeValue<'a> for Period<'a> {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
        decode_period(bytes).ok_or(DiagnosticCode::MalformedPeriod)
    }

    fn decode_property(property: &'a Property) -> Result<Self, DiagnosticCode> {
        let written = Self::decode_value(property.value_text().as_bytes())?;
        let Some(tzid) = zone_of(property) else {
            return Ok(written);
        };
        // One line carries one `TZID`, and it is a statement about the value rather than about
        // the octets on one side of the `/`. So it reaches both bounds: a period that took the
        // zone at the start and left the end floating would be two halves in two zones, which
        // is not something a content line can say.
        Ok(match written {
            Self::Explicit { start, end } => Self::Explicit {
                start: under_zone(start, tzid),
                end: under_zone(end, tzid),
            },
            Self::Starting { start, duration } => Self::Starting {
                start: under_zone(start, tzid),
                duration,
            },
        })
    }
}

impl EncodeValue for Period<'_> {
    /// Write back the form the value holds: an explicit period as `start/end`, a starting one
    /// as `start/duration`.
    ///
    /// Which of the two it is belongs to the value and not to this encoder, because section
    /// 3.3.9's two productions say different things — one names an end and the other names a
    /// length — and a producer that wrote one gets it back rather than the other.
    ///
    /// Every refusal is made before an octet is written, so a value with no RFC 5545 form
    /// leaves the buffer as empty as it found it.
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        match *self {
            Self::Explicit { start, end } => {
                // Two bounds naming two zones have one line and one `TZID` to be written on,
                // so writing them would keep one zone and drop the other silently — which is
                // the loss this crate is arranged against, arriving through a write.
                if !is_period_bound(start) || !is_period_bound(end) || zones_disagree(start, end) {
                    return Err(MutationError::NotRepresentable);
                }
                start.encode_value(out)?;
                out.push_octet(b'/');
                end.encode_value(out)
            },
            Self::Starting { start, duration } => {
                if !is_period_bound(start) || !is_positive(duration) {
                    return Err(MutationError::NotRepresentable);
                }
                start.encode_value(out)?;
                out.push_octet(b'/');
                duration.encode_value(out)
            },
        }
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `PERIOD` is stated outright rather than removed. It is the default value type of
        // `FREEBUSY` and not of `RDATE`, which takes three, so a period that said nothing
        // about `VALUE` would read back as a date-time on the property where it matters. Which
        // property this is is not a question a value type may ask, so the parameter that makes
        // the answer the same everywhere is the one to write.
        out.push(ParameterEdit::set(b"VALUE", ValueType::Period.as_bytes()));
        // The zone is whichever bound names one; the encoder has already refused the value
        // where the two name different ones, so there is at most one answer here.
        let zone = match *self {
            Self::Explicit { start, end } => match start.tzid() {
                Some(tzid) => Some(tzid),
                None => end.tzid(),
            },
            Self::Starting { start, .. } => start.tzid(),
        };
        match zone {
            Some(tzid) => out.push(ParameterEdit::set(b"TZID", tzid)),
            None => out.push(ParameterEdit::remove(b"TZID")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// UTC offsets, section 3.3.14
// ---------------------------------------------------------------------------------------

/// Read section 3.3.14's `("+" / "-") time-hour time-minute [time-second]`.
///
/// The sign is mandatory, unlike section 3.3.8's, and `-0000` is the one spelling the section
/// names as forbidden: it would say "unknown offset", which is a claim RFC 5545 reserves and
/// [`UtcOffset`] cannot make.
fn decode_utc_offset(bytes: &[u8]) -> Option<UtcOffset> {
    let (negative, rest) = match bytes.split_first() {
        Some((&b'-', tail)) => (true, tail),
        Some((&b'+', tail)) => (false, tail),
        _ => return None,
    };
    let second = match rest.len() {
        4 => 0,
        6 => field(rest, 4, 6)?,
        _ => return None,
    };
    let hour = field(rest, 0, 2)?;
    let minute = field(rest, 2, 4)?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let total = hour
        .checked_mul(3_600)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;
    let magnitude = i32::try_from(total).ok()?;
    if negative && magnitude == 0 {
        return None;
    }
    let signed = if negative {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    UtcOffset::from_seconds(signed)
}

impl DecodeValue<'_> for UtcOffset {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_utc_offset(bytes).ok_or(DiagnosticCode::MalformedUtcOffset)
    }
}

impl EncodeValue for UtcOffset {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        let total = self.seconds();
        out.push_octet(if total < 0 { b'-' } else { b'+' });
        let (hours, minutes, seconds) =
            split_clock(u64::from(total.unsigned_abs())).ok_or(MutationError::NotRepresentable)?;
        // The seconds term is written only when there is one. It is optional in the grammar,
        // every real producer omits it, and a zero written where none was expected is the
        // kind of difference that shows up as a spurious diff in a caller's version control.
        if seconds == 0 {
            return write_formatted(out, format_args!("{hours:02}{minutes:02}"));
        }
        write_formatted(out, format_args!("{hours:02}{minutes:02}{seconds:02}"))
    }

    fn coupled_parameters(&self, _out: &mut Vec<ParameterEdit>) {
        // Nothing. An offset has one written form, so no parameter is a function of its
        // shape, and `UTC-OFFSET` is the default value type of both properties that take one.
    }
}

// ---------------------------------------------------------------------------------------
// Floats and the geographic pair, section 3.3.7 and section 3.8.1.6
// ---------------------------------------------------------------------------------------

/// Whether `bytes` is section 3.3.7's `FLOAT` exactly.
///
/// Checked before anything parses it, because the standard library's float reader accepts
/// spellings this format does not have — `1e5`, `.5`, `inf`, `NaN` — and accepting them would
/// mean a value RFC 5545 calls malformed arriving as a number.
fn is_float_text(bytes: &[u8]) -> bool {
    let (_, unsigned) = split_sign(bytes);
    let Some(dot) = unsigned.iter().position(|&octet| octet == b'.') else {
        return all_digits(unsigned);
    };
    let (whole, after_dot) = unsigned.split_at(dot);
    all_digits(whole) && after(after_dot, b'.').is_some_and(all_digits)
}

/// Whether `bytes` is a non-empty run of ASCII digits.
fn all_digits(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

/// Read one section 3.3.7 `FLOAT`.
///
/// The syntax check is not the whole of the refusal. `1` followed by three hundred and nine
/// zeros is section 3.3.7's `FLOAT` exactly — every octet a digit — and the nearest `f64` to it
/// is infinity, so a guard written to keep `inf` out by refusing the spellings that name it
/// lets `inf` in through a spelling that names a number. A magnitude the target type cannot
/// hold is a malformed value and not the largest representable one, which is the answer
/// [`decimal`] already gives a `SEQUENCE` of two hundred digits rather than saturating it.
fn decode_float(bytes: &[u8]) -> Option<f64> {
    if !is_float_text(bytes) {
        return None;
    }
    // Every octet was checked to be a sign, a digit or a dot, so the text conversion cannot
    // fail; it is asked rather than assumed because this module has no unchecked step.
    let value = str::from_utf8(bytes).ok()?.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

impl DecodeValue<'_> for f64 {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_float(bytes).ok_or(DiagnosticCode::MalformedFloat)
    }
}

// There is deliberately no `EncodeValue for f64`, for the reason there is none for `Geo`
// below: this is that value arriving alone rather than in a pair, and `37.386013` is no more
// recoverable from the nearest `f64` for being the only number on the line.

/// Read section 3.8.1.6's `FLOAT ";" FLOAT`.
///
/// The range of the pair is not checked. A latitude past the pole is a claim about the world
/// rather than about the grammar, the diagnostic this decoder reports is about the grammar,
/// and the text is written back either way. A magnitude past what an `f64` holds is the other
/// thing, and it is reported as the code the half that failed earns:
/// [`DiagnosticCode::MalformedFloat`] where both halves are section 3.3.7's syntax and one of
/// them names no number this crate can hold, and [`DiagnosticCode::MalformedGeo`] where the
/// pair is not a pair at all.
fn decode_geo(bytes: &[u8]) -> Result<Geo, DiagnosticCode> {
    let separator = bytes
        .iter()
        .position(|&octet| octet == b';')
        .ok_or(DiagnosticCode::MalformedGeo)?;
    let written = (
        bytes.get(..separator).ok_or(DiagnosticCode::MalformedGeo)?,
        after(
            bytes.get(separator..).ok_or(DiagnosticCode::MalformedGeo)?,
            b';',
        )
        .ok_or(DiagnosticCode::MalformedGeo)?,
    );
    if !is_float_text(written.0) || !is_float_text(written.1) {
        return Err(DiagnosticCode::MalformedGeo);
    }
    let latitude = decode_float(written.0).ok_or(DiagnosticCode::MalformedFloat)?;
    let longitude = decode_float(written.1).ok_or(DiagnosticCode::MalformedFloat)?;
    Ok(Geo::new(latitude, longitude))
}

impl DecodeValue<'_> for Geo {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_geo(bytes)
    }
}

// There is deliberately no `EncodeValue for Geo`. The shortest round-trip formatting of the
// nearest `f64` to `37.386013` is not necessarily `37.386013`, so writing through this type
// would rewrite text nobody asked to change. Readable and not writable is the honest shape.

// ---------------------------------------------------------------------------------------
// Integers and booleans, section 3.3.8 and section 3.3.2
// ---------------------------------------------------------------------------------------

/// Read section 3.3.8's `["+" / "-"] 1*DIGIT` within the range the section states.
fn decode_integer(bytes: &[u8]) -> Option<i32> {
    let (negative, unsigned) = split_sign(bytes);
    let magnitude = decimal(unsigned)?;
    if negative {
        // The most negative integer has no positive counterpart, so the negation happens in
        // the wider type and the range check is the conversion back.
        let signed = i64::try_from(magnitude).ok()?.checked_neg()?;
        return i32::try_from(signed).ok();
    }
    i32::try_from(magnitude).ok()
}

impl DecodeValue<'_> for i32 {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_integer(bytes).ok_or(DiagnosticCode::MalformedInteger)
    }
}

impl EncodeValue for i32 {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        write_formatted(out, format_args!("{self}"))
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `INTEGER` is the default value type of every property that takes one, so what a
        // written integer implies is the absence of a `VALUE`, not a particular one.
        out.push(ParameterEdit::remove(b"VALUE"));
    }
}

impl DecodeValue<'_> for bool {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        if bytes.eq_ignore_ascii_case(b"TRUE") {
            return Ok(true);
        }
        if bytes.eq_ignore_ascii_case(b"FALSE") {
            return Ok(false);
        }
        Err(DiagnosticCode::MalformedBoolean)
    }
}

impl EncodeValue for bool {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        out.push_bytes(if *self { b"TRUE" } else { b"FALSE" });
        Ok(())
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // As for an integer: `BOOLEAN` is the default wherever one is accepted.
        out.push(ParameterEdit::remove(b"VALUE"));
    }
}

// ---------------------------------------------------------------------------------------
// Text, section 3.3.11
// ---------------------------------------------------------------------------------------

/// What one escape spelling stands for, or `None` when section 3.3.11 gives it no meaning.
fn substitution_for(spelling: u8) -> Option<u8> {
    TEXT_ESCAPES
        .iter()
        .find(|&&(written, _)| written == spelling)
        .map(|&(_, stands_for)| stands_for)
}

/// Append what the octet after a backslash stands for.
fn push_escaped(out: &mut String, character: char) {
    if let Some(octet) = u8::try_from(character).ok().and_then(substitution_for) {
        out.push(char::from(octet));
        return;
    }
    // Section 3.3.11 gives no meaning to any other escape. Both octets are kept rather than
    // the backslash dropped, because a decode is the one place a caller stops looking at the
    // storage, and it must not be the place a byte the storage still holds disappears.
    out.push('\\');
    out.push(character);
}

/// Resolve section 3.3.11's escapes, borrowing when there is nothing to resolve.
///
/// The scan for a backslash is what keeps the common case free of allocation: a `SUMMARY`
/// with no escape in it hands back the same octets the property is holding, and nothing is
/// copied on the way.
fn unescape(text: &str) -> Cow<'_, str> {
    if !text.as_bytes().contains(&b'\\') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for character in text.chars() {
        if pending {
            push_escaped(&mut out, character);
            pending = false;
        } else if character == '\\' {
            pending = true;
        } else {
            out.push(character);
        }
    }
    if pending {
        // A value ending in a lone backslash escapes nothing. Section 3.3.11 defines no such
        // value; the octet is written back rather than swallowed, for the reason above.
        out.push('\\');
    }
    Cow::Owned(out)
}

impl<'a> TextValue<'a> {
    /// Resolve section 3.3.11's escapes, after asking whether the octets are text at all.
    ///
    /// Validation comes first and unescaping second, and the order is the whole defense
    /// against a multi-byte lead octet whose trail was eaten by an escape. Every substitution
    /// the section defines is ASCII, so none can satisfy a UTF-8 continuation requirement:
    /// an orphaned lead octet fails here deterministically instead of being completed by
    /// accident into a codepoint nobody wrote. Reversing the two would make the attack work.
    ///
    /// The result borrows when there was no escape to resolve, so the ordinary value costs
    /// nothing, and the escaped one allocates exactly once. Neither touches the storage: a
    /// failure leaves the octets where they are, still written back, and the caller is told
    /// where they stopped being text.
    pub fn decode(self) -> Result<Cow<'a, str>, TextError> {
        let text = str::from_utf8(self.as_bytes()).map_err(TextError::from)?;
        Ok(unescape(text))
    }
}

impl<'a> DecodeValue<'a> for TextValue<'a> {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
        // Infallible on purpose. A `TEXT` value is whatever octets arrived, and the two
        // questions that can fail — is it UTF-8, what do its escapes stand for — are asked by
        // `decode`, where a caller has said it wants text rather than a view.
        Ok(Self::from_bytes(bytes))
    }
}

impl EncodeValue for TextValue<'_> {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        let bytes = self.as_bytes();
        // A `TextValue` holds the value's octets as they will be written, escapes included,
        // so the write is a copy. What it is not allowed to be is an injection: a value
        // carrying a terminator would smuggle a whole second content line into the component,
        // and a `SUMMARY` taken from a web form becoming a second `ATTENDEE` is a real
        // attack. Refused rather than escaped, because escaping here would silently change
        // what the caller asked to store.
        if bytes.iter().copied().any(is_forbidden_control) {
            return Err(MutationError::IllegalControlCharacter);
        }
        out.push_bytes(bytes);
        Ok(())
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `TEXT` is the default value type of every property this view is written through.
        out.push(ParameterEdit::remove(b"VALUE"));
    }
}

// ---------------------------------------------------------------------------------------
// Inline binary, section 3.3.1
// ---------------------------------------------------------------------------------------

/// How many characters section 3.3.1 writes one quantum as.
const QUANTUM_LEN: usize = 4;

/// The sextet `octet` stands for, or `None` when section 3.3.1's `b-char` does not include it.
const fn sextet(octet: u8) -> Option<u8> {
    // Each arm has established its own range before subtracting, so none of the wrapping forms
    // below can wrap; they are spelled that way because a bare `-` is an unchecked operation
    // and this module has none.
    match octet {
        b'A'..=b'Z' => Some(octet.wrapping_sub(b'A')),
        b'a'..=b'z' => Some(octet.wrapping_sub(b'a').wrapping_add(26)),
        b'0'..=b'9' => Some(octet.wrapping_sub(b'0').wrapping_add(52)),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Whether `quantum` is the last four characters of a `binary`: four characters of the
/// alphabet, or section 3.3.1's `b-end` — two or three of them and the `=` that pads them out.
fn is_end_quantum(quantum: &[u8]) -> bool {
    let padding = quantum
        .iter()
        .rev()
        .take_while(|&&octet| octet == b'=')
        .count();
    if padding > 2 {
        return false;
    }
    let filled = quantum.len().saturating_sub(padding);
    quantum
        .get(..filled)
        .is_some_and(|held| held.iter().all(|&octet| sextet(octet).is_some()))
}

/// Whether `bytes` is section 3.3.1's `binary` exactly: whole quanta, padded on the last one
/// and nowhere else.
///
/// The padding is where a base 64 reader that accepts what it is given goes wrong. Section
/// 3.3.1 writes `*(4b-char) [b-end]`, so a length that is not a multiple of four is a value
/// that lost characters somewhere, and taking it for a shorter one would hand a caller octets
/// no producer wrote.
fn is_base64_text(bytes: &[u8]) -> bool {
    let Some(end) = bytes.len().checked_sub(QUANTUM_LEN) else {
        // `*(4b-char)` matches nothing at all, so a value with no characters in it is an empty
        // binary; anything shorter than one quantum is a quantum that did not arrive whole.
        return bytes.is_empty();
    };
    let Some((body, last)) = bytes.split_at_checked(end) else {
        // `end` is a length this slice has, so this cannot happen; it is spelled rather than
        // unwrapped because a malformed value is the honest reading of an impossible split.
        return false;
    };
    // The divisor is a nonzero constant, so the remainder is always there; it is asked for
    // rather than taken because this module has no unchecked arithmetic.
    body.len().checked_rem(QUANTUM_LEN) == Some(0)
        && body.iter().all(|&octet| sextet(octet).is_some())
        && is_end_quantum(last)
}

/// The three octets one quantum stands for, and how many of them its padding leaves.
///
/// The padding contributes six zero bits per `=` and stands for no octet of its own, which is
/// what the count is for: four characters stand for three octets, three for two, and two for
/// one. Where the padding may appear is [`is_base64_text`]'s question and is settled before
/// this runs.
fn quantum_octets(quantum: &[u8]) -> Option<([u8; 3], usize)> {
    let mut packed: u32 = 0;
    let mut filled = 0_usize;
    for &octet in quantum {
        let value = if octet == b'=' { 0 } else { sextet(octet)? };
        packed = packed.checked_mul(64)?.checked_add(u32::from(value))?;
        if octet != b'=' {
            filled = filled.checked_add(1)?;
        }
    }
    // Divided rather than shifted, for the reason the accumulation above is checked: the
    // quantum is under three octets wide, so each of the three is in range by construction and
    // the conversion is asked anyway.
    let stands_for = [
        u8::try_from(packed.checked_div(65_536)?).ok()?,
        u8::try_from(packed.checked_div(256)?.checked_rem(256)?).ok()?,
        u8::try_from(packed.checked_rem(256)?).ok()?,
    ];
    Some((stands_for, filled.checked_mul(6)?.checked_div(8)?))
}

/// How many octets a base 64 text of `written` characters can stand for.
const fn decoded_len(written: usize) -> usize {
    // Three per quantum, which is exact except for a padded final one — where it is one or two
    // too many and never too few, so the reservation is made once and never grown into.
    match written.checked_div(QUANTUM_LEN) {
        // A text long enough to overflow this cannot be resident, and asking for no room up
        // front is the harmless answer: the vector grows on its own.
        Some(groups) => match groups.checked_mul(3) {
            Some(room) => room,
            None => 0,
        },
        None => 0,
    }
}

impl BinaryValue<'_> {
    /// Read the octets this base 64 text stands for.
    ///
    /// A step a caller asks for and never one storage takes, exactly as [`TextValue::decode`]
    /// is. The text is checked again here rather than assumed, because a view can be built
    /// over any octets and this is the one place where the answer would be wrong rather than
    /// absent.
    ///
    /// Section 3.3.1 fixes the alphabet and says nothing about what a producer left in the
    /// bits past the last full octet of a padded quantum. Those bits are dropped rather than
    /// refused — RFC 4648 leaves them to the reader — and the text that carried them is
    /// written back untouched, which is why two texts can decode alike and still each come
    /// back as itself.
    pub fn decode(self) -> Result<Vec<u8>, DiagnosticCode> {
        let text = self.as_bytes();
        if !is_base64_text(text) {
            return Err(DiagnosticCode::MalformedBinary);
        }
        let mut out = Vec::with_capacity(decoded_len(text.len()));
        for quantum in text.chunks_exact(QUANTUM_LEN) {
            let (stands_for, filled) =
                quantum_octets(quantum).ok_or(DiagnosticCode::MalformedBinary)?;
            out.extend_from_slice(
                stands_for
                    .get(..filled)
                    .ok_or(DiagnosticCode::MalformedBinary)?,
            );
        }
        Ok(out)
    }
}

impl<'a> DecodeValue<'a> for BinaryValue<'a> {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
        if !is_base64_text(bytes) {
            return Err(DiagnosticCode::MalformedBinary);
        }
        Ok(Self::from_bytes(bytes))
    }
}

impl EncodeValue for BinaryValue<'_> {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        let bytes = self.as_bytes();
        // The text is what is written, and never a re-encoding of what it decodes to: the bits
        // past the last full octet of a padded quantum are not read, so two texts stand for
        // the same octets and writing through the decoded form would rewrite one of them into
        // the other. That is `Geo`'s rule; this type can still be written because what it
        // holds is already the text.
        //
        // What is refused is text that is not section 3.3.1's, which is also why no control
        // character check is needed here: the alphabet has no octet a content line ends on.
        if !is_base64_text(bytes) {
            return Err(MutationError::NotRepresentable);
        }
        out.push_bytes(bytes);
        Ok(())
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // Section 3.3.1 requires both, and requires them together: inline octets are
        // unreadable without the encoding that says how to read them. `ATTACH` is the property
        // that takes either this or a URI, so these two are exactly the pairing a URI written
        // over a binary value has to undo — which is what `UriValue` states below.
        out.push(ParameterEdit::set(b"VALUE", ValueType::Binary.as_bytes()));
        out.push(ParameterEdit::set(b"ENCODING", b"BASE64"));
    }
}

// ---------------------------------------------------------------------------------------
// Identifiers and addresses, section 3.3.13 and section 3.3.3
// ---------------------------------------------------------------------------------------

/// Whether `octet` is one RFC 3986 section 2 gives a URI a place for.
///
/// The union of `unreserved`, `gen-delims`, `sub-delims` and the `%` that opens an escape.
/// Everything else is excluded, a space and every octet past ASCII included: RFC 5545 section
/// 3.3.13 says a value of this type is a URI, and a URI carrying either is one that was meant
/// to be percent-encoded and was not.
const fn is_uri_octet(octet: u8) -> bool {
    matches!(
        octet,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b':'
            | b'/'
            | b'?'
            | b'#'
            | b'['
            | b']'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b'%'
    )
}

/// Whether `bytes` is RFC 3986 section 3.1's `scheme`.
fn is_scheme(bytes: &[u8]) -> bool {
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && rest
            .iter()
            .all(|&octet| octet.is_ascii_alphanumeric() || matches!(octet, b'+' | b'-' | b'.'))
}

/// The octets before the first `:`, or `None` when there is none.
fn scheme_of(bytes: &[u8]) -> Option<&[u8]> {
    let separator = bytes.iter().position(|&octet| octet == b':')?;
    bytes.get(..separator)
}

/// Whether every `%` in `bytes` opens RFC 3986 section 2.1's `pct-encoded`.
fn percents_are_escapes(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .enumerate()
        .filter(|&(_, &octet)| octet == b'%')
        .all(|(at, _)| {
            let after_percent = bytes.get(at.saturating_add(1)..at.saturating_add(3));
            matches!(
                after_percent,
                Some([high, low]) if high.is_ascii_hexdigit() && low.is_ascii_hexdigit()
            )
        })
}

/// Whether `bytes` is a URI RFC 3986 section 3 defines, which is what section 3.3.13 writes.
///
/// A scheme is what makes it one, and it is checked rather than assumed: a `CAL-ADDRESS` with
/// a bare mail address where a `mailto:` belongs is the most common malformed `ATTENDEE` in
/// the corpus, and reading it as a URI would let a scheduling reply match nothing at all.
/// Nothing here is normalized — the scheme's case, the percent-encoding and the path's case
/// are all a caller's to compare — because rewriting one is how a `mailto:` stops matching the
/// `ATTENDEE` it was written for.
fn is_uri_text(bytes: &[u8]) -> bool {
    scheme_of(bytes).is_some_and(is_scheme)
        && bytes.iter().copied().all(is_uri_octet)
        && percents_are_escapes(bytes)
}

impl<'a> DecodeValue<'a> for UriValue<'a> {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode> {
        if !is_uri_text(bytes) {
            return Err(DiagnosticCode::MalformedUri);
        }
        Ok(Self::from_bytes(bytes))
    }
}

impl EncodeValue for UriValue<'_> {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        let bytes = self.as_bytes();
        // Written as it stands, for `BinaryValue`'s reason: this type holds the text, so
        // there is nothing here to reproduce and nothing to spend. A value that is not a URI
        // is refused rather than written, and the syntax excludes every octet a content line
        // ends on, so this write has no injection to check for either.
        if !is_uri_text(bytes) {
            return Err(MutationError::NotRepresentable);
        }
        out.push_bytes(bytes);
        Ok(())
    }

    fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
        // `URI` is the default value type of every property that takes one, and so is section
        // 3.3.3's `CAL-ADDRESS` on the two properties that take that — so what a written URI
        // implies is the absence of a `VALUE` rather than a particular one. The `ENCODING`
        // goes with it: `ATTACH` is the property that takes either this or inline octets, and
        // a `BASE64` left beside a URI says the address is a value it is not.
        out.push(ParameterEdit::remove(b"VALUE"));
        out.push(ParameterEdit::remove(b"ENCODING"));
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::vec::Vec;

    use ical_grammar::{Diagnostic, DiagnosticCode, LineEnding, LineLayout, TEXT_ESCAPES};

    use super::{decode_date_time_value, decode_duration, decode_geo, decode_utc_offset};
    use crate::change::ParameterEdit;
    use crate::gregorian::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Duration, UtcOffset,
    };
    use crate::octets::RawText;
    use crate::tree::{Parameter, Property};
    use crate::view::{
        BinaryValue, DecodeValue, EncodeValue, Geo, MutationError, Period, PropertyMut, TextValue,
        UriValue, ValueBuf, View,
    };

    /// The octets a value encodes to, for the round-trip tables below.
    fn written<T: EncodeValue>(value: &T) -> Vec<u8> {
        let mut buffer = ValueBuf::new();
        value.encode_value(&mut buffer).unwrap();
        buffer.into_vec()
    }

    /// Assert that `value` writes exactly `expected` and nothing more.
    fn assert_written<T: EncodeValue>(value: &T, expected: &[u8]) {
        assert_eq!(written(value).as_slice(), expected);
    }

    /// The parameters a written value states about itself.
    fn coupled<T: EncodeValue>(value: &T) -> Vec<ParameterEdit> {
        let mut edits = Vec::new();
        value.coupled_parameters(&mut edits);
        edits
    }

    /// The statement a written value makes about `VALUE`, `None` when it makes none.
    fn value_statement<T: EncodeValue>(value: &T) -> Option<ParameterEdit> {
        coupled(value)
            .into_iter()
            .find(|edit| edit.name().eq_ignore_ascii_case(b"VALUE"))
    }

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    /// The same date, under the name the zoned tests read better with.
    fn date_of(year: u16, month: u8, day: u8) -> CivilDate {
        date(year, month, day)
    }

    /// A `DTSTART` carrying the given parameters, as a producer wrote it.
    fn decorated(written: &[(&[u8], &[u8])], value: &[u8]) -> Property {
        let parameters = written
            .iter()
            .map(|(spelling, assigned)| {
                Parameter::new(RawText::from_bytes(spelling), RawText::from_bytes(assigned))
            })
            .collect();
        Property::new(
            RawText::from_bytes(b"DTSTART"),
            parameters,
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, second).unwrap(),
        )
    }

    /// A view holds the octets as written. Resolving the escapes is a step a caller asks for,
    /// and the storage is unaffected by whether anyone ever does.
    #[test]
    fn a_text_view_keeps_its_escapes_until_something_resolves_them() {
        let stored = RawText::from_bytes(b"a\\nb");
        let value = TextValue::from_bytes(stored.as_bytes());
        assert_eq!(value.as_bytes(), b"a\\nb");
    }

    /// Every substitution is ASCII, which is what lets validation run before unescaping and
    /// makes an orphaned lead octet fail deterministically instead of being completed.
    #[test]
    fn no_escape_substitution_can_complete_a_multi_byte_sequence() {
        for (written_as, stands_for) in TEXT_ESCAPES {
            assert!(written_as.is_ascii(), "the escape spelling is ASCII");
            assert!(stands_for.is_ascii(), "what it stands for is ASCII");
            assert!(
                !(0x80..=0xBF).contains(&stands_for),
                "a substitution that is a UTF-8 continuation octet would repair broken text"
            );
        }
    }

    /// Empty input. Every decoder has to answer with its own code rather than with a default
    /// value, because a property that arrived with no value at all is exactly the shape a
    /// colonless content line degrades to.
    #[test]
    fn an_empty_value_is_malformed_in_every_codec_that_can_say_so() {
        assert_eq!(
            CivilDate::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedDate
        );
        assert_eq!(
            CivilTime::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedTime
        );
        assert_eq!(
            CivilDateTime::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedDateTime
        );
        assert_eq!(
            DateTimeValue::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedDateTime
        );
        assert_eq!(
            Duration::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedDuration
        );
        assert_eq!(
            UtcOffset::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedUtcOffset
        );
        assert_eq!(
            Geo::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedGeo
        );
        assert_eq!(
            i32::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedInteger
        );
        assert_eq!(
            bool::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedBoolean
        );
        assert_eq!(
            f64::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedFloat
        );
        assert_eq!(
            Period::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedPeriod
        );
        assert_eq!(
            UriValue::decode_value(b"").unwrap_err(),
            DiagnosticCode::MalformedUri
        );
    }

    /// An empty `TEXT` is a value and not a failure: a `DESCRIPTION:` with nothing after the
    /// colon is legal, common, and must not decode to an error.
    #[test]
    fn an_empty_text_value_is_a_value() {
        let value = TextValue::decode_value(b"").unwrap();
        assert_eq!(value.as_bytes(), b"");
        assert!(matches!(value.decode().unwrap(), Cow::Borrowed("")));
    }

    /// Input that stops before its terminator. Each of these is a prefix of something legal,
    /// which is the case a decoder that reads greedily gets wrong by answering.
    #[test]
    fn a_value_cut_short_is_refused_rather_than_half_read() {
        assert!(CivilDate::decode_value(b"2026081").is_err());
        assert!(CivilDateTime::decode_value(b"20260815T1200").is_err());
        assert!(DateTimeValue::decode_value(b"20260815T120000").is_ok());
        assert!(
            DateTimeValue::decode_value(b"20260815T12000").is_err(),
            "a date-time one octet short is not the date that starts it"
        );
        assert!(Duration::decode_value(b"P1DT").is_err());
        assert!(Duration::decode_value(b"P").is_err());
        assert!(UtcOffset::decode_value(b"+12").is_err());
        assert!(Geo::decode_value(b"37.386013;").is_err());
        assert!(Geo::decode_value(b"37.;-122.0").is_err());
        assert!(Period::decode_value(b"20260815T090000Z/").is_err());
        assert!(
            BinaryValue::decode_value(b"QUJ").is_err(),
            "a quantum one character short is a quantum that lost one, not a shorter value"
        );
    }

    /// The longest legal form of each value this unit reads, at its full written length.
    #[test]
    fn the_longest_written_form_of_each_value_decodes() {
        assert_eq!(
            DateTimeValue::decode_value(b"20260815T235960Z").unwrap(),
            DateTimeValue::Utc(stamp(2026, 8, 15, 23, 59, 60)),
        );
        assert_eq!(
            UtcOffset::decode_value(b"-235959").unwrap(),
            UtcOffset::from_seconds(-86_399).unwrap(),
        );
        assert_eq!(i32::decode_value(b"-2147483648").unwrap(), i32::MIN);
        assert_eq!(i32::decode_value(b"+2147483647").unwrap(), i32::MAX);
        assert_eq!(
            Duration::decode_value(b"-P4294967295DT23H59M59S").unwrap(),
            Duration::new(-4_294_967_295, -86_399),
        );
    }

    /// The three forms of a date-time value are told apart before anything is parsed, and the
    /// `Z` is read only by the type that can hold what it says.
    #[test]
    fn the_three_date_time_forms_map_to_the_three_variants() {
        let cases: [(&[u8], Option<DateTimeValue>); 6] = [
            (b"20260815", Some(DateTimeValue::Date(date(2026, 8, 15)))),
            (
                b"20260815T120000",
                Some(DateTimeValue::Local(stamp(2026, 8, 15, 12, 0, 0))),
            ),
            (
                b"20260815T120000Z",
                Some(DateTimeValue::Utc(stamp(2026, 8, 15, 12, 0, 0))),
            ),
            (b"20260815T120000z", None),
            (b"20260230", None),
            (b"20260815 120000", None),
        ];
        for (input, expected) in cases {
            assert_eq!(decode_date_time_value(input), expected, "{input:?}");
        }
    }

    /// A `Z` is a statement about a zone, and the two narrow types have nowhere to keep one,
    /// so they refuse rather than decoding to something weaker than the text they were given.
    #[test]
    fn a_zone_marker_is_refused_by_the_types_that_cannot_hold_it() {
        assert!(CivilTime::decode_value(b"120000Z").is_err());
        assert!(CivilDateTime::decode_value(b"20260815T120000Z").is_err());
        assert!(CivilTime::decode_value(b"120000").is_ok());
    }

    /// The forms section 3.3.6 defines, and the ones it does not.
    #[test]
    fn a_duration_reads_the_forms_the_section_defines_and_no_others() {
        let cases: [(&[u8], Option<Duration>); 10] = [
            (b"P1D", Some(Duration::new(1, 0))),
            (b"PT24H", Some(Duration::new(0, 86_400))),
            (b"P1W", Some(Duration::new(7, 0))),
            (b"P15DT5H0M20S", Some(Duration::new(15, 18_020))),
            (b"-PT10M", Some(Duration::new(0, -600))),
            (b"+P1D", Some(Duration::new(1, 0))),
            (b"PT0S", Some(Duration::new(0, 0))),
            (b"P1Y", None),
            (b"P1M", None),
            (b"PT1S1H", None),
        ];
        for (input, expected) in cases {
            assert_eq!(decode_duration(input), expected, "{input:?}");
        }
    }

    /// The two fields stay apart, so a span of one day and the equivalent span of hours each
    /// keep the spelling they were built with rather than collapsing into one canonical form.
    #[test]
    fn a_duration_keeps_the_split_it_was_built_with() {
        assert_written(&Duration::new(1, 0), b"P1D");
        assert_written(&Duration::new(0, 86_400), b"PT24H0M0S");
        assert_written(&Duration::ZERO, b"PT0H0M0S");
        assert_written(&Duration::new(-1, -30), b"-P1DT0H0M30S");
        assert_written(&Duration::new(15, 18_020), b"P15DT5H0M20S");
        // Two fields that disagree in sign have no two-signed form to be written as, so they
        // are reconciled into the one sign the section allows.
        assert_written(&Duration::new(1, -30), b"PT23H59M30S");
    }

    /// Writing then reading is the identity on this type, which is the fixed point every
    /// mutation round trip through a duration rests on.
    #[test]
    fn a_written_duration_reads_back_as_the_same_span() {
        for span in [
            Duration::ZERO,
            Duration::new(1, 0),
            Duration::new(0, 86_400),
            Duration::new(15, 18_020),
            Duration::new(-1, -30),
        ] {
            let octets = written(&span);
            assert_eq!(Duration::decode_value(&octets).unwrap(), span, "{octets:?}");
        }
    }

    /// The offset spellings, including the one the section names as forbidden.
    #[test]
    fn an_offset_needs_its_sign_and_may_not_be_negative_zero() {
        let cases: [(&[u8], Option<i32>); 8] = [
            (b"+0900", Some(32_400)),
            (b"-0500", Some(-18_000)),
            (b"+053000", Some(19_800)),
            (b"+0000", Some(0)),
            (b"-0000", None),
            (b"-000000", None),
            (b"0900", None),
            (b"+2400", None),
        ];
        for (input, expected) in cases {
            let decoded = decode_utc_offset(input).map(UtcOffset::seconds);
            assert_eq!(decoded, expected, "{input:?}");
        }
    }

    /// The seconds term is written back only where there is one, so an offset that arrived as
    /// four digits does not come back as six.
    #[test]
    fn an_offset_writes_the_seconds_term_only_when_it_has_one() {
        assert_written(&UtcOffset::from_seconds(32_400).unwrap(), b"+0900");
        assert_written(&UtcOffset::from_seconds(-18_000).unwrap(), b"-0500");
        assert_written(&UtcOffset::from_seconds(19_800).unwrap(), b"+0530");
        assert_written(&UtcOffset::from_seconds(19_845).unwrap(), b"+053045");
        assert_written(&UtcOffset::UTC, b"+0000");
        // The one spelling that may not be produced, and cannot be: a negative sign is only
        // written for a magnitude that is not zero.
        assert_written(&UtcOffset::from_seconds(0).unwrap(), b"+0000");
    }

    /// The float grammar is narrower than the standard library's reader, and the difference
    /// is exactly the set of spellings RFC 5545 does not have.
    #[test]
    fn a_geographic_pair_takes_the_float_grammar_and_not_the_language_one() {
        assert_eq!(
            decode_geo(b"37.386013;-122.082932"),
            Ok(Geo::new(37.386_013, -122.082_932))
        );
        assert_eq!(decode_geo(b"37;-122"), Ok(Geo::new(37.0, -122.0)));
        assert_eq!(decode_geo(b"+0.0;-0.5"), Ok(Geo::new(0.0, -0.5)));

        // Every spelling below parses as a number in this language and is not one in this
        // format, which is the whole reason the grammar is checked before anything reads it.
        let rejected: [&[u8]; 7] = [
            b"1e5;-122.0",
            b".5;-122.0",
            b"5.;-122.0",
            b"inf;-122.0",
            b"NaN;-122.0",
            b"37.386013",
            b"37.386013;-122.0;0",
        ];
        for input in rejected {
            assert_eq!(
                decode_geo(input),
                Err(DiagnosticCode::MalformedGeo),
                "{input:?}"
            );
        }

        // Section 3.3.7's syntax exactly, and no `f64` holds it. The pair is well formed and
        // the number is not, so the code names the half that failed.
        let mut past_the_range = b"1".to_vec();
        past_the_range.extend(core::iter::repeat_n(b'0', 309));
        past_the_range.extend_from_slice(b";-122.0");
        assert_eq!(
            decode_geo(&past_the_range),
            Err(DiagnosticCode::MalformedFloat)
        );
    }

    /// The text a `GEO` arrived as is the text that is written back, which is why this type
    /// decodes and does not encode. The pair below is the evidence: the shortest formatting
    /// of the nearest `f64` is not the spelling the producer used.
    #[test]
    fn a_geographic_pair_is_readable_and_not_writable() {
        let stored = RawText::from_bytes(b"37.3860130;-122.0829320");
        let pair = Geo::decode_value(stored.as_bytes()).unwrap();
        assert_eq!(pair, Geo::new(37.386_013, -122.082_932));
        assert_eq!(
            stored.as_bytes(),
            b"37.3860130;-122.0829320",
            "the trailing zeros are the producer's, and reading the pair does not spend them"
        );
    }

    /// An integer wider than the range section 3.3.8 states is malformed rather than clamped.
    #[test]
    fn an_integer_out_of_range_is_malformed_rather_than_saturated() {
        assert_eq!(
            i32::decode_value(b"2147483648").unwrap_err(),
            DiagnosticCode::MalformedInteger
        );
        assert_eq!(
            i32::decode_value(b"-2147483649").unwrap_err(),
            DiagnosticCode::MalformedInteger
        );
        assert!(i32::decode_value(b"1 ").is_err());
        assert!(i32::decode_value(b"-").is_err());
        assert_written(&-42_i32, b"-42");
        assert_eq!(i32::decode_value(&written(&i32::MIN)).unwrap(), i32::MIN);
    }

    /// Section 3.3.2 names two values and says nothing about their casing, and real producers
    /// have written both spellings.
    #[test]
    fn a_boolean_is_two_words_in_any_casing() {
        assert!(bool::decode_value(b"TRUE").unwrap());
        assert!(bool::decode_value(b"true").unwrap());
        assert!(!bool::decode_value(b"False").unwrap());
        assert_eq!(
            bool::decode_value(b"YES").unwrap_err(),
            DiagnosticCode::MalformedBoolean
        );
        assert_written(&true, b"TRUE");
        assert_written(&false, b"FALSE");
    }

    /// Nothing to unescape means nothing to allocate, which is the common case for every
    /// `UID`, `SUMMARY` and `LOCATION` a calendar carries.
    #[test]
    fn text_with_no_escape_borrows_rather_than_copying() {
        let value = TextValue::from_bytes(b"standup");
        let decoded = value.decode().unwrap();
        assert!(matches!(decoded, Cow::Borrowed("standup")));
    }

    /// The substitutions, the undefined escape, and the value that ends on a lone backslash.
    #[test]
    fn escapes_resolve_to_what_the_section_says_and_nothing_else_is_dropped() {
        let cases: [(&[u8], &str); 6] = [
            (b"a\\nb", "a\nb"),
            (b"a\\Nb", "a\nb"),
            (b"a\\,b\\;c", "a,b;c"),
            (b"a\\\\b", "a\\b"),
            (b"a\\qb", "a\\qb"),
            (b"trailing\\", "trailing\\"),
        ];
        for (input, expected) in cases {
            let decoded = TextValue::from_bytes(input).decode().unwrap();
            assert_eq!(decoded, expected, "{input:?}");
        }
    }

    /// The attack validate-then-unescape exists to stop. The lead octet of a two-octet
    /// sequence is followed by an escape whose substitution is ASCII; unescaping first would
    /// hand the lead octet a neighbor and manufacture a codepoint nobody wrote.
    #[test]
    fn text_is_validated_before_it_is_unescaped() {
        let attack = TextValue::from_bytes(b"\xc3\\n");
        let error = attack.decode().unwrap_err();
        assert_eq!(error.valid_up_to(), 0);

        // A CP1252 export, which is in the corpus and must survive as octets either way.
        let legacy = TextValue::from_bytes(b"\xe9t\xe9");
        assert!(legacy.decode().is_err());
        assert_eq!(legacy.as_bytes(), b"\xe9t\xe9", "the octets are untouched");
    }

    /// A failure to read is a diagnostic code and not an error: the value is still there, and
    /// the caller is handed the reason next to the octets that produced it.
    #[test]
    fn a_value_that_cannot_be_read_is_a_diagnostic_and_not_a_lost_property() {
        let stored = RawText::from_bytes(b"20261301T990000");
        let outcome = DateTimeValue::decode_value(stored.as_bytes());
        assert_eq!(outcome, Err(DiagnosticCode::MalformedDateTime));
        assert_eq!(
            stored.as_bytes(),
            b"20261301T990000",
            "a malformed value keeps every octet it arrived with"
        );
    }

    /// A date-time read beside a `TZID` is a zoned value and not a floating one, which is the
    /// whole reason the shape exists: read as floating, written back through the same type, it
    /// would have carried `ParameterEdit::remove(b"TZID")` and dropped the zone it came with.
    #[test]
    fn a_date_time_read_beside_a_zone_carries_it() {
        let zoned = decorated(&[(b"TZID", b"\"Europe/Paris\"")], b"20260815T090000");
        assert_eq!(
            DateTimeValue::decode_property(&zoned),
            Ok(DateTimeValue::Zoned {
                stamp: stamp(2026, 8, 15, 9, 0, 0),
                tzid: b"Europe/Paris",
            }),
            "the DQUOTE pair comes off, because that is the form a zone source is handed"
        );

        let floating = decorated(&[], b"20260815T090000");
        assert_eq!(
            DateTimeValue::decode_property(&floating),
            Ok(DateTimeValue::Local(stamp(2026, 8, 15, 9, 0, 0)))
        );

        // A zone with no name names nothing, so the value stays what its own octets say and the
        // parameter is still written back exactly as it arrived.
        let nameless = decorated(&[(b"TZID", b"")], b"20260815T090000");
        assert_eq!(
            DateTimeValue::decode_property(&nameless),
            Ok(DateTimeValue::Local(stamp(2026, 8, 15, 9, 0, 0)))
        );
    }

    /// Section 3.2.19 forbids a `TZID` beside a UTC value or a date, and the value's own octets
    /// are the stronger of the two statements. The stray parameter is neither obeyed nor
    /// removed.
    #[test]
    fn a_zone_beside_a_value_that_cannot_have_one_does_not_change_the_value() {
        let utc = decorated(&[(b"TZID", b"Europe/Paris")], b"20260815T090000Z");
        assert_eq!(
            DateTimeValue::decode_property(&utc),
            Ok(DateTimeValue::Utc(stamp(2026, 8, 15, 9, 0, 0)))
        );
        assert_eq!(
            utc.parameters_named(b"TZID").count(),
            1,
            "the parameter the reading ignored is still there to be written back"
        );

        let date = decorated(&[(b"TZID", b"Europe/Paris")], b"20260815");
        assert_eq!(
            DateTimeValue::decode_property(&date),
            Ok(DateTimeValue::Date(date_of(2026, 8, 15)))
        );
    }

    /// The transition table `docs/adr/0001` requires: what a written date-time says about the
    /// two parameters its shape decides, for every shape it has.
    #[test]
    fn a_written_zoned_date_time_states_the_zone_it_names() {
        let value = DateTimeValue::Zoned {
            stamp: stamp(2026, 8, 15, 9, 0, 0),
            tzid: b"Europe/Paris",
        };
        assert_written(&value, b"20260815T090000");
        assert_eq!(
            coupled(&value),
            [
                ParameterEdit::remove(b"VALUE"),
                ParameterEdit::set(b"TZID", b"Europe/Paris"),
            ],
        );
    }

    /// Moving a zoned date-time keeps the zone it named, which is the cycle the shape exists
    /// for: read the value, change the clock, write it back, and the `TZID` is still there.
    ///
    /// The zone is copied out before the guard is taken, and the compiler is what says so: the
    /// value borrows the property's parameter octets and the guard borrows the property, so a
    /// caller writing a zone back into the property it came from has to say where those octets
    /// live. That is the same borrow `docs/adr/0001` spends on the guard, arriving one step
    /// earlier.
    #[test]
    fn a_zoned_date_time_is_moved_without_losing_the_zone_it_named() {
        let mut property = decorated(&[(b"TZID", b"Europe/Paris")], b"20260815T090000");
        let zone: Vec<u8> = DateTimeValue::decode_property(&property)
            .unwrap()
            .tzid()
            .unwrap()
            .to_vec();
        let moved = DateTimeValue::Zoned {
            stamp: stamp(2026, 8, 15, 10, 0, 0),
            tzid: &zone,
        };

        let mut guard: PropertyMut<'_, DateTimeValue<'_>> = PropertyMut::new(&mut property);
        guard.set(&moved).unwrap();

        assert_eq!(property.value_text().as_bytes(), b"20260815T100000");
        assert_eq!(
            property
                .parameters_named(b"TZID")
                .map(|held| held.value().as_bytes())
                .collect::<Vec<_>>(),
            [b"Europe/Paris"],
            "the zone it was read with is the zone it was written with"
        );
    }

    /// Converting a zoned date-time to a date has to say `VALUE=DATE` and drop the stale
    /// `TZID`, or it leaves behind the invalid pairing a value-only write would produce.
    #[test]
    fn a_written_date_time_states_the_parameters_its_shape_implies() {
        assert_eq!(
            coupled(&DateTimeValue::Date(date(2026, 8, 15))),
            [
                ParameterEdit::set(b"VALUE", b"DATE"),
                ParameterEdit::remove(b"TZID"),
            ],
        );
        for timed in [
            DateTimeValue::Local(stamp(2026, 8, 15, 12, 0, 0)),
            DateTimeValue::Utc(stamp(2026, 8, 15, 12, 0, 0)),
        ] {
            assert_eq!(
                coupled(&timed),
                [
                    ParameterEdit::remove(b"VALUE"),
                    ParameterEdit::remove(b"TZID"),
                ],
                "a timed value carries no zone of its own and needs no VALUE",
            );
        }
    }

    /// Every writable type states something about `VALUE`, so no type added later can be the
    /// one that silently says nothing.
    #[test]
    fn every_writable_type_states_what_it_implies_about_the_value_parameter() {
        assert_eq!(coupled(&Duration::ZERO), [ParameterEdit::remove(b"VALUE")]);
        assert_eq!(coupled(&7_i32), [ParameterEdit::remove(b"VALUE")]);
        assert_eq!(coupled(&true), [ParameterEdit::remove(b"VALUE")]);
        assert_eq!(
            coupled(&TextValue::from_bytes(b"hi")),
            [ParameterEdit::remove(b"VALUE")]
        );
        assert!(
            coupled(&UtcOffset::UTC).is_empty(),
            "an offset has one form, so nothing about it is a function of its shape"
        );
    }

    /// What every date-time writes is what it reads back, which is the fixed point the
    /// mutation round trip rests on.
    #[test]
    fn a_written_date_time_reads_back_as_the_same_value() {
        for value in [
            DateTimeValue::Date(date(2026, 8, 15)),
            DateTimeValue::Local(stamp(2026, 8, 15, 12, 0, 0)),
            DateTimeValue::Utc(stamp(1, 1, 1, 0, 0, 0)),
            DateTimeValue::Utc(stamp(9999, 12, 31, 23, 59, 60)),
        ] {
            let octets = written(&value);
            assert_eq!(
                DateTimeValue::decode_value(&octets).unwrap(),
                value,
                "{octets:?}"
            );
        }
    }

    /// The injection a write has to refuse rather than escape. A terminator inside a value
    /// would be a whole second content line once the property is serialized.
    #[test]
    fn a_text_write_refuses_the_control_characters_a_content_line_would_end_on() {
        let attack = TextValue::from_bytes(b"hi\r\nATTENDEE:mailto:eve@example.test");
        let mut buffer = ValueBuf::new();
        assert_eq!(
            attack.encode_value(&mut buffer),
            Err(MutationError::IllegalControlCharacter)
        );
        assert!(buffer.is_empty(), "a refused write leaves nothing behind");

        // A tab is not one of them: section 3.1's CONTROL production excludes HTAB, and a
        // tab inside a DESCRIPTION is ordinary in real exports.
        assert_written(&TextValue::from_bytes(b"a\tb"), b"a\tb");
        // The escapes go through untouched: a text view holds what will be written, and an
        // encoder that unescaped here would be undoing the read half's work.
        assert_written(&TextValue::from_bytes(b"a\\,b\\nc"), b"a\\,b\\nc");
    }

    /// Section 3.3.7's grammar at the type that is one number rather than a pair, and the same
    /// distance from the standard library's reader that `GEO` is checked against.
    ///
    /// Compared as bit patterns, because the claim is that the exact double the text names
    /// came back and not one near it.
    #[test]
    fn a_float_reads_the_spellings_the_section_has_and_no_others() {
        let cases: [(&[u8], Option<u64>); 9] = [
            (b"37.386013", Some(37.386_013_f64.to_bits())),
            (b"-122.082932", Some((-122.082_932_f64).to_bits())),
            (b"+1.5", Some(1.5_f64.to_bits())),
            (b"42", Some(42.0_f64.to_bits())),
            (b"1e5", None),
            (b".5", None),
            (b"5.", None),
            (b"inf", None),
            (b"NaN", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                f64::decode_value(input).ok().map(f64::to_bits),
                expected,
                "{input:?}"
            );
        }
    }

    /// The alphabet section 3.3.1 fixes, the padding it defines, and the shapes it has no
    /// production for. The second column is the octets the text stands for.
    #[test]
    fn a_binary_value_reads_the_quanta_the_section_defines() {
        let cases: [(&[u8], Option<&[u8]>); 11] = [
            (b"", Some(b"")),
            (b"QQ==", Some(b"A")),
            (b"QUI=", Some(b"AB")),
            (b"QUJD", Some(b"ABC")),
            (b"/+++", Some(b"\xff\xef\xbe")),
            (b"QQ", None),
            (b"QQ=", None),
            (b"Q===", None),
            (b"QQ==QQ==", None),
            (b"QQ-=", None),
            (b"QQ =", None),
        ];
        for (input, expected) in cases {
            let read = BinaryValue::decode_value(input).ok();
            assert_eq!(read.is_some(), expected.is_some(), "{input:?}");
            assert_eq!(
                read.and_then(|value| value.decode().ok()).as_deref(),
                expected,
                "{input:?}"
            );
        }
    }

    /// An empty `BINARY` is a value, for the reason an empty `TEXT` is: section 3.3.1's
    /// `*(4b-char)` matches nothing at all, and an `ATTACH` with nothing after its colon is a
    /// property that must come back the way it arrived.
    #[test]
    fn an_empty_binary_value_is_a_value_and_stands_for_no_octets() {
        let value = BinaryValue::decode_value(b"").unwrap();
        assert_eq!(value.as_bytes(), b"");
        assert!(value.decode().unwrap().is_empty());
        assert_written(&value, b"");
    }

    /// Two texts that stand for the same octets, each written back as itself. The bits past
    /// the last full octet of a padded quantum are not read, so a value written through its
    /// decoded form would rewrite one of these into the other — which is `GEO`'s rule, at a
    /// type that can still be written because what it holds is already the text.
    #[test]
    fn a_binary_value_is_written_as_the_text_it_holds_and_never_as_a_re_encoding() {
        let padded = BinaryValue::decode_value(b"QQ==").unwrap();
        let spare = BinaryValue::decode_value(b"QR==").unwrap();
        assert_eq!(padded.decode().unwrap(), spare.decode().unwrap());
        assert_written(&padded, b"QQ==");
        assert_written(&spare, b"QR==");

        // Octets that are not section 3.3.1's are refused rather than stored as a value they
        // are not, and the refusal comes before anything is written.
        let mut buffer = ValueBuf::new();
        assert_eq!(
            BinaryValue::from_bytes(b"not base 64!").encode_value(&mut buffer),
            Err(MutationError::NotRepresentable)
        );
        assert!(buffer.is_empty());
    }

    /// A URI is read as written and written as read: the scheme's case, the percent-encoding
    /// and the path's case are all things a normalizer would rewrite, and rewriting one is how
    /// a `mailto:` stops matching the `ATTENDEE` a scheduling reply names.
    #[test]
    fn a_uri_needs_a_scheme_and_octets_the_syntax_has_a_place_for() {
        let cases: [(&[u8], bool); 13] = [
            (b"mailto:jane@example.test", true),
            (b"MailTo:Jane.Doe@Example.Test", true),
            (b"http://example.test/calendars/a%20b.ics", true),
            (b"ftp://example.test/pub/my.ics", true),
            (b"data:text/plain;base64,QQ==", true),
            (b"mailto:", true),
            (b"", false),
            (b":no-scheme", false),
            (b"example.test/no-scheme", false),
            (b"9tel:0000", false),
            (b"mailto:jane doe@example.test", false),
            (b"http://example.test/50%", false),
            (b"http://example.test/caf\xc3\xa9", false),
        ];
        for (input, readable) in cases {
            let read = UriValue::decode_value(input);
            assert_eq!(read.is_ok(), readable, "{input:?}");
            if let Ok(value) = read {
                assert_written(&value, input);
            }
        }
    }

    /// The two productions section 3.3.9 defines, and the shapes it does not have.
    #[test]
    fn a_period_reads_the_two_forms_the_section_defines() {
        let explicit = Period::Explicit {
            start: DateTimeValue::Utc(stamp(1997, 1, 1, 18, 0, 0)),
            end: DateTimeValue::Utc(stamp(1997, 1, 2, 7, 0, 0)),
        };
        let starting = Period::Starting {
            start: DateTimeValue::Utc(stamp(1997, 1, 1, 18, 0, 0)),
            duration: Duration::new(0, 19_800),
        };
        let cases: [(&[u8], Option<Period<'_>>); 10] = [
            (b"19970101T180000Z/19970102T070000Z", Some(explicit)),
            (b"19970101T180000Z/PT5H30M", Some(starting)),
            (
                b"20260815T090000/20260815T100000",
                Some(Period::Explicit {
                    start: DateTimeValue::Local(stamp(2026, 8, 15, 9, 0, 0)),
                    end: DateTimeValue::Local(stamp(2026, 8, 15, 10, 0, 0)),
                }),
            ),
            // A `DATE` at either end is a form the section's ABNF does not have.
            (b"20260815/20260816", None),
            // A span that runs backwards, and one of no length: section 3.3.9 writes a start
            // and a positive duration.
            (b"19970101T180000Z/-PT1H", None),
            (b"19970101T180000Z/PT0S", None),
            (b"19970101T180000Z", None),
            (b"/PT1H", None),
            (b"19970101T180000Z/19970102T070000Z/PT1H", None),
            (b"19970101T180000Z/19970102T0700", None),
        ];
        for (input, expected) in cases {
            assert_eq!(Period::decode_value(input).ok(), expected, "{input:?}");
        }
    }

    /// One `TZID` on the line is a statement about the value and not about the octets on one
    /// side of the `/`, so it reaches both bounds. A period that took the zone at the start
    /// and left the end floating would be two halves in two zones, which is not something one
    /// content line can say.
    #[test]
    fn a_period_read_beside_a_zone_carries_it_at_both_ends() {
        let zoned = decorated(
            &[(b"TZID", b"\"Europe/Paris\"")],
            b"20260815T090000/20260815T100000",
        );
        assert_eq!(
            Period::decode_property(&zoned),
            Ok(Period::Explicit {
                start: DateTimeValue::Zoned {
                    stamp: stamp(2026, 8, 15, 9, 0, 0),
                    tzid: b"Europe/Paris",
                },
                end: DateTimeValue::Zoned {
                    stamp: stamp(2026, 8, 15, 10, 0, 0),
                    tzid: b"Europe/Paris",
                },
            })
        );

        let starting = decorated(&[(b"TZID", b"Europe/Paris")], b"20260815T090000/PT1H");
        assert_eq!(
            Period::decode_property(&starting).map(Period::start),
            Ok(DateTimeValue::Zoned {
                stamp: stamp(2026, 8, 15, 9, 0, 0),
                tzid: b"Europe/Paris",
            })
        );

        // A bound written with a `Z` says UTC outright, and section 3.2.19 forbids the `TZID`
        // beside it: the octets are the stronger statement, and the parameter is neither
        // obeyed nor removed.
        let mixed = decorated(
            &[(b"TZID", b"Europe/Paris")],
            b"20260815T090000/20260815T100000Z",
        );
        assert_eq!(
            Period::decode_property(&mixed),
            Ok(Period::Explicit {
                start: DateTimeValue::Zoned {
                    stamp: stamp(2026, 8, 15, 9, 0, 0),
                    tzid: b"Europe/Paris",
                },
                end: DateTimeValue::Utc(stamp(2026, 8, 15, 10, 0, 0)),
            })
        );
        assert_eq!(
            mixed.parameters_named(b"TZID").count(),
            1,
            "the parameter the end ignored is still there to be written back"
        );
    }

    /// Text this crate did not author, written back through the value it was read as. A period
    /// keeps the form it arrived in — an end stays an end and a length stays a length — and
    /// every form whose text its value determines comes back octet for octet.
    ///
    /// A duration is the one part that does not: `PT5H30M` and `PT5H30M0S` are the same span,
    /// and which spelling comes back is `Duration`'s answer rather than this type's.
    #[test]
    fn a_period_is_written_back_in_the_form_it_was_read_in() {
        let cases: [(&[u8], &[u8]); 6] = [
            (
                b"19970101T180000Z/19970102T070000Z",
                b"19970101T180000Z/19970102T070000Z",
            ),
            (
                b"20260815T090000/20260815T100000",
                b"20260815T090000/20260815T100000",
            ),
            (b"19970101T180000Z/P1D", b"19970101T180000Z/P1D"),
            (b"19970101T180000Z/PT5H30M", b"19970101T180000Z/PT5H30M0S"),
            (b"19970101T180000Z/P1W", b"19970101T180000Z/P7D"),
            (b"20260815T090000/PT1H", b"20260815T090000/PT1H0M0S"),
        ];
        for (input, expected) in cases {
            let value = Period::decode_value(input).unwrap();
            assert_written(&value, expected);
        }

        // A zoned period writes the floating octets it is, which are the octets it was read
        // from: section 3.3.9 gives the zone no spelling inside the value.
        let zoned = decorated(&[(b"TZID", b"Europe/Paris")], b"20260815T090000/PT1H0M0S");
        let value = Period::decode_property(&zoned).unwrap();
        assert_written(&value, b"20260815T090000/PT1H0M0S");
    }

    /// What a period cannot be written as. Each is refused before an octet is written, so a
    /// value with no RFC 5545 form leaves the buffer as empty as it found it.
    #[test]
    fn a_period_with_no_form_to_be_written_in_is_refused_rather_than_written_wrong() {
        let midday = DateTimeValue::Local(stamp(2026, 8, 15, 12, 0, 0));
        let refused: [Period<'_>; 4] = [
            // A bound with no clock, which section 3.3.9's ABNF has no place for.
            Period::Explicit {
                start: DateTimeValue::Date(date(2026, 8, 15)),
                end: midday,
            },
            // Two bounds naming two zones, which one line and one `TZID` cannot say.
            Period::Explicit {
                start: DateTimeValue::Zoned {
                    stamp: stamp(2026, 8, 15, 9, 0, 0),
                    tzid: b"Europe/Paris",
                },
                end: DateTimeValue::Zoned {
                    stamp: stamp(2026, 8, 15, 10, 0, 0),
                    tzid: b"Asia/Tokyo",
                },
            },
            Period::Starting {
                start: midday,
                duration: Duration::new(0, -3_600),
            },
            Period::Starting {
                start: midday,
                duration: Duration::ZERO,
            },
        ];
        for value in refused {
            let mut buffer = ValueBuf::new();
            assert_eq!(
                value.encode_value(&mut buffer),
                Err(MutationError::NotRepresentable),
                "{value:?}"
            );
            assert!(buffer.is_empty(), "a refused write leaves nothing behind");
        }
    }

    /// The transition table for the three value types this unit writes. A binary value states
    /// the encoding that makes it readable; a URI undoes exactly that pairing, because `ATTACH`
    /// is the property that takes either; a period names its own value type, because `RDATE`
    /// takes three and a value may not ask which property it is being written to.
    #[test]
    fn a_written_value_states_the_pairing_its_own_shape_implies() {
        assert_eq!(
            coupled(&BinaryValue::from_bytes(b"QUJD")),
            [
                ParameterEdit::set(b"VALUE", b"BINARY"),
                ParameterEdit::set(b"ENCODING", b"BASE64"),
            ],
        );
        assert_eq!(
            coupled(&UriValue::from_bytes(b"http://example.test/my.ics")),
            [
                ParameterEdit::remove(b"VALUE"),
                ParameterEdit::remove(b"ENCODING"),
            ],
        );

        let floating = Period::decode_value(b"20260815T090000/20260815T100000").unwrap();
        assert_eq!(
            coupled(&floating),
            [
                ParameterEdit::set(b"VALUE", b"PERIOD"),
                ParameterEdit::remove(b"TZID"),
            ],
        );

        let zoned = decorated(&[(b"TZID", b"\"Europe/Paris\"")], b"20260815T090000/PT1H");
        let carried = Period::decode_property(&zoned).unwrap();
        assert_eq!(
            coupled(&carried),
            [
                ParameterEdit::set(b"VALUE", b"PERIOD"),
                ParameterEdit::set(b"TZID", b"Europe/Paris"),
            ],
            "a period read under a zone is written back under it",
        );
    }

    /// The completeness half of `docs/adr/0001`'s transition table: every value type this file
    /// can write appears below with the statement its shape makes about `VALUE`, so a type
    /// added without one is a row missing rather than a judgment nobody wrote down.
    #[test]
    fn the_transition_table_is_complete_across_every_type_this_file_writes() {
        let period = Period::decode_value(b"20260815T090000/PT1H").unwrap();
        let stated: [(&str, Option<ParameterEdit>); 9] = [
            ("Duration", value_statement(&Duration::ZERO)),
            ("i32", value_statement(&7_i32)),
            ("bool", value_statement(&true)),
            ("TextValue", value_statement(&TextValue::from_bytes(b"hi"))),
            (
                "DateTimeValue",
                value_statement(&DateTimeValue::Date(date(2026, 8, 15))),
            ),
            (
                "BinaryValue",
                value_statement(&BinaryValue::from_bytes(b"QUJD")),
            ),
            (
                "UriValue",
                value_statement(&UriValue::from_bytes(b"mailto:j@example.test")),
            ),
            ("Period", value_statement(&period)),
            // The one type that states nothing, in the table rather than left out of it: an
            // offset has one written form and `UTC-OFFSET` is the default value type of both
            // properties that take one, so no parameter is a function of its shape. What this
            // check asserts is that every implementor was considered, not that every one
            // speaks.
            ("UtcOffset", value_statement(&UtcOffset::UTC)),
        ];
        for (named, statement) in stated {
            assert_eq!(statement.is_none(), named == "UtcOffset", "{named}");
        }
    }

    /// A value type never asks what the property is called: the two accessor levels are named
    /// on different axes, and section 3.3 asks what a value is and never what it is named. The
    /// parameters are the half a decoder may read, which is what the zoned readings above do.
    #[test]
    fn a_decoder_reads_the_parameters_and_never_the_property_name() {
        let property = decorated(&[(b"ENCODING", b"BASE64"), (b"VALUE", b"BINARY")], b"QUJD");
        let view: View<'_, BinaryValue<'_>> = property.value();
        assert_eq!(view.value().map(BinaryValue::as_bytes), Some(&b"QUJD"[..]));
        assert_eq!(
            property.name().as_bytes(),
            b"DTSTART",
            "the line is named something else entirely, which the decoder neither read nor \
             was misled by"
        );
    }

    /// A value this unit cannot read is a diagnostic beside the octets and never an error that
    /// costs the property: the text is still there, still reachable, and still written back.
    #[test]
    fn a_value_this_unit_cannot_read_is_a_diagnostic_and_not_a_lost_property() {
        let inline = decorated(&[], b"not base 64!");
        let view: View<'_, BinaryValue<'_>> = inline.value();
        assert_eq!(
            view.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::MalformedBinary)
        );
        assert_eq!(
            inline.value_text().as_bytes(),
            b"not base 64!",
            "a malformed value keeps every octet it arrived with"
        );

        let backwards = decorated(&[], b"19970101T180000Z/-PT1H");
        let span: View<'_, Period<'_>> = backwards.value();
        assert_eq!(
            span.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::MalformedPeriod)
        );

        // The address every corpus has an `ATTENDEE` carrying, with the `mailto:` left off.
        let bare = decorated(&[], b"jane@example.test");
        let address: View<'_, UriValue<'_>> = bare.value();
        assert_eq!(
            address.diagnostic().map(Diagnostic::code),
            Some(DiagnosticCode::MalformedUri)
        );
        assert!(address.is_present(), "malformed is present, not absent");
        assert!(address.source().is_some());
    }
}
