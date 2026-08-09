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
//! text is authoritative and the pair of floats is derived from it. The date-time family is
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
use crate::view::{DecodeValue, EncodeValue, Geo, MutationError, TextValue, ValueBuf, ValueType};

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
fn decode_date_time_value(bytes: &[u8]) -> Option<DateTimeValue> {
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

impl DecodeValue<'_> for DateTimeValue {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_date_time_value(bytes).ok_or(DiagnosticCode::MalformedDateTime)
    }
}

impl EncodeValue for DateTimeValue {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
        match *self {
            Self::Date(date) => encode_date(out, date),
            Self::Local(stamp) => {
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
        // `VALUE` and `TZID` are a function of which of the three forms this is, and of
        // nothing the caller wrote before. A date carries no time and therefore no zone; a
        // UTC date-time carries its zone in the `Z`; a floating one asserts the absence of
        // one. All three drop `TZID`, and only the date needs a `VALUE` at all, `DATE-TIME`
        // being the default for every property that takes one.
        match *self {
            Self::Date(_) => {
                out.push(ParameterEdit::set(b"VALUE", ValueType::Date.as_bytes()));
                out.push(ParameterEdit::remove(b"TZID"));
            },
            Self::Local(_) | Self::Utc(_) => {
                out.push(ParameterEdit::remove(b"VALUE"));
                out.push(ParameterEdit::remove(b"TZID"));
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
fn decode_float(bytes: &[u8]) -> Option<f64> {
    if !is_float_text(bytes) {
        return None;
    }
    // Every octet was checked to be a sign, a digit or a dot, so the text conversion cannot
    // fail; it is asked rather than assumed because this module has no unchecked step.
    str::from_utf8(bytes).ok()?.parse::<f64>().ok()
}

/// Read section 3.8.1.6's `FLOAT ";" FLOAT`.
///
/// The range of the pair is not checked. A latitude past the pole is a claim about the world
/// rather than about the grammar, the diagnostic this decoder reports is about the grammar,
/// and the text is written back either way.
fn decode_geo(bytes: &[u8]) -> Option<Geo> {
    let separator = bytes.iter().position(|&octet| octet == b';')?;
    let latitude = decode_float(bytes.get(..separator)?)?;
    let longitude = decode_float(after(bytes.get(separator..)?, b';')?)?;
    Some(Geo::new(latitude, longitude))
}

impl DecodeValue<'_> for Geo {
    fn decode_value(bytes: &[u8]) -> Result<Self, DiagnosticCode> {
        decode_geo(bytes).ok_or(DiagnosticCode::MalformedGeo)
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

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::vec::Vec;

    use ical_grammar::{DiagnosticCode, TEXT_ESCAPES};

    use super::{decode_date_time_value, decode_duration, decode_geo, decode_utc_offset};
    use crate::change::ParameterEdit;
    use crate::gregorian::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Duration, UtcOffset,
    };
    use crate::octets::RawText;
    use crate::view::{DecodeValue, EncodeValue, Geo, MutationError, TextValue, ValueBuf};

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

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
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
            Some(Geo::new(37.386_013, -122.082_932))
        );
        assert_eq!(decode_geo(b"37;-122"), Some(Geo::new(37.0, -122.0)));
        assert_eq!(decode_geo(b"+0.0;-0.5"), Some(Geo::new(0.0, -0.5)));

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
            assert_eq!(decode_geo(input), None, "{input:?}");
        }
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
}
