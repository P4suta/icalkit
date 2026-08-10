// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scoped mutation: what a write may touch, and what it refuses.
//!
//! Specification: not RFC 5545. This is the mutation boundary of `docs/adr/0001`.
//!
//! A write names one property and reaches nothing else. Every path here goes through
//! [`PropertyMut::property_mut`] and the three setters on [`Property`], each of which discards
//! the preserved text and recorded fold layout of one line and of nothing else — which is what
//! lets every other line in the component still serialize octet for octet after an edit. No
//! path here regenerates a component, and none reorders what is already in one.
//!
//! The unit a write scopes is the whole property rather than its value alone. RFC 5545 makes
//! `VALUE` and `TZID` a function of what a date-time is, so writing a date where a zoned
//! date-time was has to emit `VALUE=DATE` and drop the stale `TZID`; a value-only write would
//! leave behind a pairing the specification does not allow. Which parameters those are is the
//! written value's own answer, through [`EncodeValue::coupled_parameters`], and never a
//! judgment made property by property here. Parameters outside that list — `RANGE`, `FBTYPE`,
//! every `X-` parameter — belong to the caller and survive a write untouched.
//!
//! This is also the one place the crate refuses caller input outright rather than diagnosing
//! it. Octets read out of a file are kept whatever they hold; octets a caller hands to a write
//! are checked, because a value carrying a terminator could carry a whole second content line
//! after it, and a `SUMMARY` taken from a web form arriving as a second `ATTENDEE` is an
//! injection that has happened rather than one that could. The refusal is a write-side check,
//! so nothing that was ever read travels through it and the round-trip guarantee costs nothing.
//!
//! [`Component::apply`] is the other half, and it refuses less. A replacement line is octets
//! off the wire like any other, so it is read through the same [`ContentLineReader`] a file
//! goes through — never a second grammar — and a violation inside it stays a diagnostic the
//! reader reports rather than a refusal here. The injection is closed structurally instead: a
//! terminator inside a replacement ends the line, and a second line is not a replacement.

use alloc::vec::Vec;

use ical_grammar::{
    ContentLineReader, ContentLineSource, Limits, LineEnding, LineLayout, Token,
    parameter_is_representable, parameter_name_is_representable, property_name_is_representable,
    quote_parameter_into,
};

use crate::change::{ParameterEdit, ProposedChange};
use crate::gregorian::DateTimeValue;
use crate::ident::PropertyId;
use crate::octets::RawText;
use crate::tree::{Boundary, Component, Item, Parameter, Property};
use crate::view::{EncodeValue, MutationError, PropertyMut, ValueBuf};

/// Refuse octets carrying an ASCII control character.
///
/// Stated over every ASCII control character, which is stricter than RFC 5545 section 3.1 —
/// that grammar excludes them from a value except for `HTAB`. The extra octet goes too: a
/// caller writing a tab into a calendar value gains nothing a space does not give it, and a
/// rule a reviewer can check in one line is worth more here than an exception list, which is
/// one more shape for a terminator to arrive in.
fn refuse_control_characters(bytes: &[u8]) -> Result<(), MutationError> {
    if bytes.iter().any(u8::is_ascii_control) {
        return Err(MutationError::IllegalControlCharacter);
    }
    Ok(())
}

/// Refuse a value that would cross the caller's per-value bound.
///
/// Checked before each arriving chunk rather than once over the finished value, so a
/// replacement folded across a great many continuation lines is refused at the octet that
/// crosses the bound rather than after all of it is resident (`docs/adr/0007`).
fn refuse_oversized_value(
    written: usize,
    arriving: usize,
    limits: Limits,
) -> Result<(), MutationError> {
    let ceiling = limits.max_value_bytes();
    // Saturating on both conversions: a length that does not fit a `u64` is past any bound a
    // caller can stipulate, and the refusal is the same answer either way.
    let held = u64::try_from(written).unwrap_or(u64::MAX);
    let added = u64::try_from(arriving).unwrap_or(u64::MAX);
    if held.saturating_add(added) > u64::from(ceiling) {
        return Err(MutationError::ValueTooLarge { limit: ceiling });
    }
    Ok(())
}

/// Refuse an edit RFC 5545 section 3.2 has no way to write back as the edit that was made.
///
/// A [`ParameterEdit`] carries a *value*, not the octets of a line, so writing one means
/// choosing its section 3.2 spelling — and three shapes have no spelling at all. A `DQUOTE`
/// is excluded from `QSAFE-CHAR` and section 3.2 defines no escape that would return it, so a
/// quoted value carrying one reads back as something shorter with the rest of the line
/// attached. A control character ends the physical line outright, which is how a parameter
/// assignment becomes a second `ATTENDEE` — the same injection
/// [`PropertyMut::set_raw`] exists to refuse, arriving through the channel `ical-itip` applies
/// an off-the-wire transition through. And a name carrying a delimiter is a name the reader
/// hands back in pieces.
///
/// The other shapes are written rather than refused: `:` `;` and `,` are excluded from
/// `SAFE-CHAR` and included in `QSAFE-CHAR`, so [`quote_parameter_into`] puts them inside a
/// `DQUOTE` pair and the value survives. Refusing those would refuse
/// `CN="Doe, John"`, which every client in the corpus writes.
fn refuse_unwritable_edits(edits: &[ParameterEdit]) -> Result<(), MutationError> {
    for entry in edits {
        if !parameter_name_is_representable(entry.name()) {
            return Err(MutationError::NotRepresentable);
        }
        if entry
            .value()
            .is_some_and(|held| !parameter_is_representable(held))
        {
            return Err(MutationError::NotRepresentable);
        }
    }
    Ok(())
}

/// One parameter in the section 3.2 spelling its value requires, the refusals already made.
///
/// The value is stored quoted where `SAFE-CHAR` excludes what it carries, because that is the
/// form the tree stores for a value that was read: a parameter keeps the quotes its producer
/// wrote, so a parameter this crate writes has to carry the quotes its value needs. Storing the
/// bare octets instead would put a `:` on the wire unquoted and the next read would end the
/// header at it.
fn spelled_parameter(name: &[u8], value: &[u8]) -> Parameter {
    let mut spelled = Vec::new();
    quote_parameter_into(value, &mut spelled);
    Parameter::new(RawText::from_bytes(name), RawText::from_vec(spelled))
}

impl Parameter {
    /// A parameter a caller is assembling, refused where section 3.2 has no way to write it.
    ///
    /// # Errors
    ///
    /// [`MutationError::NotRepresentable`] for a name the reader would hand back in pieces, and
    /// for a value carrying a `DQUOTE` or a control character — the two shapes `QSAFE-CHAR`
    /// excludes and section 3.2 defines no escape for.
    pub fn create(name: &[u8], value: &[u8]) -> Result<Self, MutationError> {
        if !parameter_name_is_representable(name) || !parameter_is_representable(value) {
            return Err(MutationError::NotRepresentable);
        }
        Ok(spelled_parameter(name, value))
    }
}

impl Property {
    /// A property a caller is assembling, refused where RFC 5545 has no way to write it back.
    ///
    /// This is the tree-building door, and it is the same door
    /// [`PropertyMut::set_raw`](crate::PropertyMut::set_raw) is: octets a caller hands over have
    /// no producer whose spelling to preserve, so refusing them costs the round-trip guarantee
    /// nothing (`docs/adr/0001`). Without it, a `SUMMARY` taken from a web form could carry its
    /// own `CRLF` into a component through [`Component::items_mut`] and come back on the next
    /// read as a second `ATTENDEE` nobody added — the injection the scoped-write refusal exists
    /// to stop, arriving through construction instead.
    ///
    /// The line is given a canonical layout: this is a line the crate authors, so it is folded
    /// the way the crate folds and terminated the way section 3.1 requires.
    ///
    /// # Errors
    ///
    /// [`MutationError::NotRepresentable`] for a name that would not read back whole — the
    /// empty one included — and [`MutationError::IllegalControlCharacter`] for a value carrying
    /// one, which is refused rather than escaped because escaping would silently store
    /// something other than what the caller asked to store.
    pub fn create(
        name: &[u8],
        parameters: Vec<Parameter>,
        value: &[u8],
    ) -> Result<Self, MutationError> {
        if !property_name_is_representable(name) {
            return Err(MutationError::NotRepresentable);
        }
        refuse_control_characters(value)?;
        Ok(Self::new(
            RawText::from_bytes(name),
            parameters,
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        ))
    }
}

impl Component {
    /// A component a caller is assembling, with the two boundary lines section 3.6 requires.
    ///
    /// The `END` is written from the same octets as the `BEGIN`, so a component this crate
    /// authored is closed by the name it was opened with and in the case it was opened in.
    ///
    /// [`Component::new`] stays open beside this because nothing it takes can be fabricated: a
    /// [`Boundary`] has no public constructor, so the only ones in existence came out of a file
    /// or out of this call, and rearranging those is a caller's business rather than an
    /// injection.
    ///
    /// # Errors
    ///
    /// [`MutationError::NotRepresentable`] for a name that would not read back whole.
    pub fn create(name: &[u8], items: Vec<Item>) -> Result<Self, MutationError> {
        if !property_name_is_representable(name) {
            return Err(MutationError::NotRepresentable);
        }
        let edge = |keyword: &[u8]| {
            Boundary::new(
                RawText::from_bytes(keyword),
                RawText::from_bytes(name),
                LineLayout::canonical(LineEnding::CANONICAL),
            )
        };
        Ok(Self::new(edge(b"BEGIN"), items, Some(edge(b"END"))))
    }
}

/// Assign `value` to the parameter `name`, keeping the position the line already gave it.
///
/// In place rather than remove-and-append, because where a parameter sits on the line is
/// something its producer chose and an assignment is about the value. Any further parameter of
/// the same name goes: one that stayed would still be on the line and still be read, so an
/// assignment that left it there would not be one.
///
/// The value is stored in the form section 3.2 writes it in, `DQUOTE`s included, which is what
/// [`spelled_parameter`] does and why both write doors go through it.
fn assign_parameter(parameters: &mut Vec<Parameter>, name: &[u8], value: &[u8]) {
    let at = parameters.iter().position(|held| held.is_named(name));
    parameters.retain(|held| !held.is_named(name));
    let written = spelled_parameter(name, value);
    match at {
        // Every parameter before the first match survived the retain, so the recorded index
        // is still a position this vector has.
        Some(index) => parameters.insert(index, written),
        None => parameters.push(written),
    }
}

/// Apply one ordered list of parameter edits, leaving every other parameter where it was.
///
/// The caller has already run [`refuse_unwritable_edits`] over the whole list, which is what
/// makes this total: an edit list is applied entirely or not at all, and a property is never
/// left carrying the first three edits of five.
fn apply_parameter_edits(parameters: &mut Vec<Parameter>, edits: &[ParameterEdit]) {
    for entry in edits {
        match entry.value() {
            Some(assigned) => assign_parameter(parameters, entry.name(), assigned),
            None => parameters.retain(|held| !held.is_named(entry.name())),
        }
    }
}

/// The parts of one replacement content line, taken out of the token stream.
///
/// A structure rather than four bindings threaded through the loop, because the token layer
/// hands its payloads back one at a time and each borrows the reader for exactly as long as it
/// is looked at; the only way to hold two of them is to have owned them first.
#[derive(Debug, Default)]
struct ParsedLine {
    /// The property name, as the replacement spelled it.
    name: Vec<u8>,
    /// The parameters, in the order the replacement wrote them.
    parameters: Vec<Parameter>,
    /// The value's octets, reassembled across the replacement's own folds.
    value: Vec<u8>,
    /// The terminator the replacement carried, `None` when it carried none.
    ending: Option<LineEnding>,
}

/// One parameter of a replacement line, in the shape the tree stores.
fn read_parameter(name: &[u8], value: &[u8], has_value: bool) -> Parameter {
    if has_value {
        Parameter::new(RawText::from_bytes(name), RawText::from_bytes(value))
    } else {
        Parameter::without_value(RawText::from_bytes(name))
    }
}

/// Take one token into `line`, answering whether it ended the line.
fn take_token(
    line: &mut ParsedLine,
    token: Token<'_>,
    limits: Limits,
) -> Result<bool, MutationError> {
    match token {
        Token::Name(name) => {
            line.name.extend_from_slice(name);
            Ok(false)
        },
        Token::Parameter {
            name,
            value,
            has_value,
        } => {
            line.parameters.push(read_parameter(name, value, has_value));
            Ok(false)
        },
        Token::Value { bytes, .. } => {
            refuse_oversized_value(line.value.len(), bytes.len(), limits)?;
            line.value.extend_from_slice(bytes);
            Ok(false)
        },
        Token::EndOfLine {
            ending,
            has_separator,
            ..
        } => {
            // A line with no `:` has a name and no value. That shape is preserved when it is
            // read and is not one this crate authors: a write asserts a separator, so storing
            // this replacement would store something other than what the caller handed over.
            if !has_separator {
                return Err(MutationError::MalformedReplacement);
            }
            line.ending = ending;
            Ok(true)
        },
        // `Token` is `#[non_exhaustive]`, and a variant added after this code was written is a
        // piece of a content line this crate has nowhere to put. Keeping the rest without it
        // would write back less than the caller handed over, which is the one outcome the
        // whole crate is arranged against.
        _ => Err(MutationError::MalformedReplacement),
    }
}

/// Read exactly one content line out of `bytes`, or say the replacement was not one.
///
/// Through the same reader a file goes through, built from the caller's own grammar bounds.
/// A second grammar for replacement lines is how one corpus case comes to be accepted on one
/// path and refused on the other (`docs/adr/0008`).
fn read_replacement_line(bytes: &[u8], limits: Limits) -> Result<ParsedLine, MutationError> {
    let mut reader = ContentLineReader::new(bytes, limits.grammar());
    let mut line = ParsedLine::default();
    let mut terminated = false;
    while let Some(token) = reader.next_token() {
        if terminated {
            // Tokens after the end of the first line: the octets describe more than one
            // change, and this call names one property.
            return Err(MutationError::MalformedReplacement);
        }
        // A reader failure is a bound it could not read past rather than a judgment about the
        // calendar. Either way there is no line here to write, and a replacement this crate
        // could not read is refused.
        let piece = token.map_err(|_bound| MutationError::MalformedReplacement)?;
        terminated = take_token(&mut line, piece, limits)?;
    }
    // Empty octets end the loop before it starts, so "empty" needs no case of its own. An
    // empty name is the blank line's shape and is no more a replacement than nothing is.
    if !terminated || line.name.is_empty() {
        return Err(MutationError::MalformedReplacement);
    }
    Ok(line)
}

/// Read a replacement line and check that it names the property the change is addressed to.
///
/// A line naming something else would not replace the property the caller named. It would
/// rename it, leaving the component without the property the change was about and with a
/// second one of whatever the line said — reaching a property this call does not name, which
/// is the one thing a scoped write may not do. So it is refused rather than performed.
fn read_named_line(
    id: &PropertyId,
    bytes: &[u8],
    limits: Limits,
) -> Result<ParsedLine, MutationError> {
    let line = read_replacement_line(bytes, limits)?;
    if !id.matches(&line.name) {
        return Err(MutationError::MalformedReplacement);
    }
    Ok(line)
}

impl<T> PropertyMut<'_, T> {
    /// Write `bytes` as this property's value, refusing any ASCII control character.
    ///
    /// The value's text and this line's recorded fold layout go; the property's name, its
    /// parameters, its terminator, and every other line in the component stay exactly as they
    /// were. A refused write changes nothing at all, because the refusal comes first.
    pub fn set_raw(&mut self, bytes: &[u8]) -> Result<(), MutationError> {
        refuse_control_characters(bytes)?;
        self.property_mut()
            .set_value_text(RawText::from_bytes(bytes));
        Ok(())
    }
}

impl<T: EncodeValue> PropertyMut<'_, T> {
    /// Write `value` as this property's value, and the parameters its shape implies.
    ///
    /// The coupled parameters are applied before the value so that the property is never
    /// observable in a state where the two disagree, and both come after the encoding and the
    /// refusal, so that a value which cannot be written leaves the property as it was.
    pub fn set(&mut self, value: &T) -> Result<(), MutationError> {
        let mut encoded = ValueBuf::new();
        value.encode_value(&mut encoded)?;
        refuse_control_characters(encoded.as_bytes())?;

        let mut coupled: Vec<ParameterEdit> = Vec::new();
        value.coupled_parameters(&mut coupled);
        // A coupled parameter is the value's own answer rather than the caller's, so this
        // refusal is a claim about a value type rather than about untrusted octets — a `TZID`
        // carrying a `DQUOTE` is a zone identifier no line could name. It is checked here
        // anyway, and before anything is written, so that a value type added later cannot make
        // a property half written by being wrong about its own parameters.
        refuse_unwritable_edits(&coupled)?;

        let property = self.property_mut();
        apply_parameter_edits(property.edit_parameters(), &coupled);
        property.set_value_text(encoded.into_raw_text());
        Ok(())
    }
}

impl Component {
    /// A guard over the property with the identity `id`, or `None` when there is none.
    ///
    /// The guard borrows this whole component for as long as it lives, so reaching a second
    /// property means visibly widening a signature rather than remembering not to.
    ///
    /// The first occurrence, when a name the specification allows once arrives twice. The
    /// alternative would be answering `None`, which says "there is no such property" about a
    /// property that is there — collapsing the two states the read side spends a whole enum
    /// to keep apart. A caller that needs to know reads first: the singular accessor reports
    /// [`DiagnosticCode::DuplicateProperty`](ical_grammar::DiagnosticCode::DuplicateProperty)
    /// and every occurrence stays reachable through the general lookup.
    pub fn get_mut<T>(&mut self, id: &PropertyId) -> Option<PropertyMut<'_, T>> {
        self.property_with_id_mut(id).map(PropertyMut::new)
    }

    /// A guard over `DTSTART`, RFC 5545 section 3.8.2.4.
    ///
    /// The value written through it states its own zone: writing a
    /// [`DateTimeValue::Zoned`](crate::DateTimeValue::Zoned) assigns the `TZID` the value
    /// names, and writing any of the other three drops the one that was there, because none of
    /// them has a zone to keep.
    pub fn dtstart_mut(&mut self) -> Option<PropertyMut<'_, DateTimeValue<'_>>> {
        self.get_mut(&PropertyId::DTSTART)
    }

    /// Apply one described change to the property `id` names.
    ///
    /// `Limits` is here because a replacement line is octets off the wire like any other, and
    /// it is read through the same content-line reader a file is. A replacement that is empty,
    /// that this crate cannot read, that names another property, or that is more than one line
    /// is [`MutationError::MalformedReplacement`]; a change to a property that is not in this
    /// component is [`MutationError::Absent`]; and a value past the caller's per-value bound is
    /// [`MutationError::ValueTooLarge`], refused rather than truncated.
    pub fn apply(
        &mut self,
        id: &PropertyId,
        change: &ProposedChange,
        limits: Limits,
    ) -> Result<(), MutationError> {
        match change {
            ProposedChange::Replace(replacement) => {
                let line = read_named_line(id, replacement.as_bytes(), limits)?;
                self.overwrite(id, line)
            },
            ProposedChange::Add(addition) => {
                let line = read_named_line(id, addition.as_bytes(), limits)?;
                self.insert_after_properties(line);
                Ok(())
            },
            ProposedChange::SetParameters(edits) => {
                // Refused before the property is reached, so a refused change leaves the
                // component exactly as it was — layout included, since asking a property for
                // its parameters is itself what discards the recorded folds.
                refuse_unwritable_edits(edits)?;
                let property = self.property_with_id_mut(id).ok_or(MutationError::Absent)?;
                // The value's text is untouched, which is the whole reason this variant
                // exists. The line's layout goes anyway, because the parameters were part of
                // the line the folds were positions into.
                apply_parameter_edits(property.edit_parameters(), edits);
                Ok(())
            },
            ProposedChange::Remove => self.remove_named(id),
        }
    }

    /// The first property directly inside this component with the identity `id`, mutably.
    ///
    /// Nested components are skipped, as they are on the reading side: a `DTSTART` inside a
    /// `VALARM` belongs to the alarm, and writing through it would edit a component the caller
    /// did not name.
    fn property_with_id_mut(&mut self, id: &PropertyId) -> Option<&mut Property> {
        self.items_mut()
            .iter_mut()
            .filter_map(Item::as_property_mut)
            .find(|property| property.has_id(id))
    }

    /// Write `line` over the property `id` names — name, parameters and value together.
    fn overwrite(&mut self, id: &PropertyId, line: ParsedLine) -> Result<(), MutationError> {
        let property = self.property_with_id_mut(id).ok_or(MutationError::Absent)?;
        property.set_name(RawText::from_vec(line.name));
        let parameters = property.edit_parameters();
        // Every parameter goes, including ones the replacement did not mention: a replacement
        // states a whole content line, so a parameter it left out is one it does not have.
        // The narrower edit that keeps them is `SetParameters`.
        parameters.clear();
        parameters.extend(line.parameters);
        property.set_value_text(RawText::from_vec(line.value));
        Ok(())
    }

    /// Insert `line` as a new property, after the last property already in this component.
    ///
    /// After the properties and ahead of any nested component, because RFC 5545 section 3.6
    /// writes a component's properties before its subcomponents, and a line appended past a
    /// `VALARM` reads as that alarm's neighbor. Nothing already here changes position relative
    /// to anything else, which is the sense in which an insertion is not a reordering.
    ///
    /// The layout is a canonical one: this is a line the crate is authoring, so it is folded
    /// the way the crate folds and any fold the caller wrote is a position into text that has
    /// been read and rebuilt since. The terminator the caller wrote is kept, and a caller who
    /// wrote none gets the one RFC 5545 section 3.1 requires — a line with no terminator at
    /// all would run into whatever follows it.
    ///
    /// The line it is inserted *after* needs the same thing, for the same reason, and that is
    /// the one octet an addition writes outside the line it added. See
    /// [`Component::terminate_line_above`].
    fn insert_after_properties(&mut self, line: ParsedLine) {
        let layout = LineLayout::canonical(line.ending.unwrap_or(LineEnding::CANONICAL));
        let property = Property::new(
            RawText::from_vec(line.name),
            line.parameters,
            RawText::from_vec(line.value),
            layout,
        );
        // The last property's index counted from the end; one past it is where a property
        // goes, and `0` when this component holds no property yet.
        let at = self
            .items()
            .iter()
            .rposition(|entry| entry.as_property().is_some())
            .map_or(0, |index| index.saturating_add(1));
        self.terminate_line_above(at);
        self.items_mut().insert(at, Item::Property(property));
    }

    /// Give the line that will sit above position `at` the terminator section 3.1 requires.
    ///
    /// A final line often arrives with no terminator, and this crate writes it back with none,
    /// because appending one would add an octet the file did not have. That reasoning holds
    /// for exactly as long as the line is last. Two content lines with nothing between them
    /// are one content line: written unchanged, the property above would swallow the addition,
    /// the addition would not exist on the next read, and the property above would come back
    /// with the addition's octets glued to its value. So the terminator is written at the
    /// moment the insertion creates the need for it, and never at any other moment.
    ///
    /// This is the one octet a scoped write puts outside the property it names, and it is not
    /// the rewrite `docs/adr/0001` forbids: the line above keeps its name, its parameters, its
    /// value and its position, and what it gains is the delimiter that makes it a line rather
    /// than the first half of one.
    ///
    /// The entry above an insertion is always a property, because `at` is one past the last
    /// property this component holds; when there is no property at all, `at` is zero and the
    /// line above is this component's own `BEGIN`.
    fn terminate_line_above(&mut self, at: usize) {
        let ending = LineEnding::CANONICAL;
        let above = at
            .checked_sub(1)
            .and_then(|index| self.items_mut().get_mut(index))
            .and_then(Item::as_property_mut);
        match above {
            Some(property) => property.terminate_line(ending),
            None => self.begin_mut().terminate_line(ending),
        };
    }

    /// Remove every property directly inside this component with the identity `id`.
    ///
    /// Every one rather than the first. A caller that asked for a property to be gone and got
    /// one of two copies removed has no way to see that it half happened, and the identity is
    /// what the change names. Order among what is left is untouched.
    fn remove_named(&mut self, id: &PropertyId) -> Result<(), MutationError> {
        let before = self.items().len();
        self.items_mut().retain(|entry| {
            !entry
                .as_property()
                .is_some_and(|property| property.has_id(id))
        });
        if self.items().len() == before {
            return Err(MutationError::Absent);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use ical_grammar::{FoldPoint, Limits, LineEnding, LineLayout};

    use super::{ParsedLine, read_replacement_line};
    use crate::change::{ParameterEdit, ProposedChange};
    use crate::ident::PropertyId;
    use crate::octets::RawText;
    use crate::tree::{Boundary, Component, Item, Parameter, Property};
    use crate::view::{EncodeValue, MutationError, PropertyMut, TextValue, ValueBuf};

    /// A property folded once, so that a write discarding this line's layout is observable and
    /// a neighbor keeping its own is observable beside it.
    fn folded(name: &[u8], value: &[u8]) -> Property {
        let layout = LineLayout::preserved(
            vec![FoldPoint {
                offset: 40,
                tab: false,
                newline: LineEnding::CrLf,
            }],
            Some(LineEnding::CrLf),
            true,
        );
        Property::new(
            RawText::from_bytes(name),
            Vec::new(),
            RawText::from_bytes(value),
            layout,
        )
    }

    /// A property with parameters, in the order they were written.
    fn decorated(name: &[u8], written: &[(&[u8], &[u8])], value: &[u8]) -> Property {
        let parameters = written
            .iter()
            .map(|(spelling, assigned)| {
                Parameter::new(RawText::from_bytes(spelling), RawText::from_bytes(assigned))
            })
            .collect();
        Property::new(
            RawText::from_bytes(name),
            parameters,
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    fn event(items: Vec<Item>) -> Component {
        let edge = |keyword: &[u8]| {
            Boundary::new(
                RawText::from_bytes(keyword),
                RawText::from_bytes(b"VEVENT"),
                LineLayout::canonical(LineEnding::CANONICAL),
            )
        };
        Component::new(edge(b"BEGIN"), items, Some(edge(b"END")))
    }

    /// Every parameter of a property as `(name, value)`, for comparing whole lists at once.
    fn parameters_of(property: &Property) -> Vec<(&[u8], &[u8])> {
        property
            .parameters()
            .iter()
            .map(|held| (held.name().as_bytes(), held.value().as_bytes()))
            .collect()
    }

    /// A value whose shape decides two parameters, standing in for the date-time transition
    /// table: it assigns a `VALUE` and it removes a `TZID`, and it says nothing about anything
    /// else, because anything else belongs to whoever wrote it.
    #[derive(Debug)]
    struct Stamp {
        /// The octets this value writes.
        text: &'static [u8],
    }

    impl EncodeValue for Stamp {
        fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError> {
            out.push_bytes(self.text);
            Ok(())
        }

        fn coupled_parameters(&self, out: &mut Vec<ParameterEdit>) {
            out.push(ParameterEdit::set(b"VALUE", b"DATE"));
            out.push(ParameterEdit::remove(b"TZID"));
        }
    }

    /// The refusal is stated over control characters and made before anything is written, so
    /// the injection cannot arrive escaped and cannot arrive half applied either.
    #[test]
    fn a_write_refuses_every_control_character_rather_than_escaping_one() {
        let refused: [&[u8]; 5] = [
            b"hi\r\nATTENDEE:mailto:eve@example.test",
            b"hi\n",
            b"hi\r",
            b"a\x00b",
            b"a\tb",
        ];
        for attempt in refused {
            let mut property = folded(b"SUMMARY", b"standup");
            let mut guard: PropertyMut<'_, TextValue<'_>> = PropertyMut::new(&mut property);
            assert_eq!(
                guard.set_raw(attempt),
                Err(MutationError::IllegalControlCharacter)
            );
            assert_eq!(property.value_text().as_bytes(), b"standup");
            assert!(
                !property.layout().is_refolded(),
                "a refused write leaves even the layout alone"
            );
        }
    }

    /// A write discards the preserved layout of the property it names, and of nothing else.
    /// The neighbor here stands for every vendor property beside an edited `DTSTART`.
    #[test]
    fn a_write_discards_one_lines_layout_and_leaves_its_neighbor_alone() {
        let mut items = [
            Item::Property(folded(b"SUMMARY", b"standup")),
            Item::Property(folded(b"X-VENDOR", b"kept")),
        ];

        let edited = items
            .first_mut()
            .and_then(Item::as_property_mut)
            .expect("the first entry is a property");
        let mut guard: PropertyMut<'_, TextValue<'_>> = PropertyMut::new(edited);
        assert_eq!(guard.set_raw(b"retro"), Ok(()));

        let written = items[0].as_property().expect("still a property");
        assert_eq!(written.value_text().as_bytes(), b"retro");
        assert!(written.layout().is_refolded());
        assert_eq!(
            written.layout().ending(),
            Some(LineEnding::CrLf),
            "the terminator is not a fold and does not go"
        );

        let untouched = items[1].as_property().expect("still a property");
        assert_eq!(untouched.layout().folds().len(), 1);
        assert!(!untouched.layout().is_refolded());
    }

    /// The parameters a value's shape decides are emitted from the value; the ones it does not
    /// decide are the caller's and stay where the caller put them.
    #[test]
    fn a_written_value_states_its_own_parameters_and_carries_no_other_over() {
        let mut property = decorated(
            b"DTSTART",
            &[
                (b"VALUE", b"DATE-TIME"),
                (b"TZID", b"Europe/Paris"),
                (b"X-VENDOR", b"kept"),
            ],
            b"20260815T090000",
        );

        let mut guard: PropertyMut<'_, Stamp> = PropertyMut::new(&mut property);
        assert_eq!(guard.set(&Stamp { text: b"20260815" }), Ok(()));

        assert_eq!(
            parameters_of(&property),
            vec![
                (&b"VALUE"[..], &b"DATE"[..]),
                (&b"X-VENDOR"[..], &b"kept"[..])
            ],
            "the assignment kept its place and the stale zone went"
        );
        assert_eq!(property.value_text().as_bytes(), b"20260815");
        assert_eq!(property.name().as_bytes(), b"DTSTART");
    }

    /// A coupled parameter that was not on the line arrives at the end, and one the value does
    /// not name is never invented.
    #[test]
    fn a_coupled_parameter_that_was_absent_is_appended_rather_than_reordering_the_line() {
        let mut property = decorated(b"DTSTART", &[(b"X-VENDOR", b"kept")], b"20260815T090000");
        let mut guard: PropertyMut<'_, Stamp> = PropertyMut::new(&mut property);
        assert_eq!(guard.set(&Stamp { text: b"20260815" }), Ok(()));
        assert_eq!(
            parameters_of(&property),
            vec![
                (&b"X-VENDOR"[..], &b"kept"[..]),
                (&b"VALUE"[..], &b"DATE"[..])
            ]
        );
    }

    /// A guard is reached by identity, which RFC 5545 compares without case, and absence is
    /// absence rather than a guard over nothing.
    #[test]
    fn a_guard_is_reached_by_identity_and_not_by_spelling() {
        let mut component = event(vec![
            Item::Property(folded(b"X-VENDOR", b"kept")),
            Item::Property(folded(b"dtstart", b"20260815T090000Z")),
        ]);
        assert!(component.dtstart_mut().is_some());
        assert!(
            component
                .get_mut::<TextValue<'_>>(&PropertyId::SUMMARY)
                .is_none()
        );
    }

    /// Everything a replacement can be that is not one content line naming this property. Each
    /// one leaves the component exactly as it was, layout included.
    #[test]
    fn a_replacement_that_is_not_one_content_line_is_refused() {
        let refused: [&[u8]; 6] = [
            b"",
            b"SUMMARY:one\r\nSUMMARY:two\r\n",
            b"SUMMARY\r\n",
            b":standup\r\n",
            b"\r\n",
            b"X-OTHER:standup\r\n",
        ];
        for attempt in refused {
            let mut component = event(vec![Item::Property(folded(b"SUMMARY", b"standup"))]);
            let change = ProposedChange::Replace(RawText::from_bytes(attempt));
            assert_eq!(
                component.apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT),
                Err(MutationError::MalformedReplacement)
            );

            let kept = component.items()[0]
                .as_property()
                .expect("still a property");
            assert_eq!(kept.value_text().as_bytes(), b"standup");
            assert!(
                !kept.layout().is_refolded(),
                "a refused change writes nothing"
            );
        }
    }

    /// The last line of an input carries no terminator, and a replacement is an input. It is
    /// still exactly one content line, and the line it replaces keeps its own terminator.
    #[test]
    fn a_replacement_that_carried_no_terminator_is_still_one_line() {
        let mut component = event(vec![Item::Property(folded(b"SUMMARY", b"standup"))]);
        let change = ProposedChange::Replace(RawText::from_bytes(b"summary;X-A=1:retro"));
        assert_eq!(
            component.apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT),
            Ok(())
        );

        let written = component.items()[0]
            .as_property()
            .expect("still a property");
        assert_eq!(
            written.name().as_bytes(),
            b"summary",
            "the spelling replaced too"
        );
        assert_eq!(parameters_of(written), vec![(&b"X-A"[..], &b"1"[..])]);
        assert_eq!(written.value_text().as_bytes(), b"retro");
        assert!(written.layout().is_refolded());
        assert_eq!(
            written.layout().ending(),
            Some(LineEnding::CrLf),
            "the replaced line's terminator is not part of what a replacement replaces"
        );
    }

    /// A folded replacement is unfolded by the same reader a file goes through, so a value
    /// arriving in chunks is one value by the time it is stored.
    #[test]
    fn a_replacement_folded_by_its_author_is_read_as_one_value() {
        let folded_line = b"DESCRIPTION:standup and re\r\n view\r\n";
        let line =
            read_replacement_line(folded_line, Limits::DEFAULT).expect("one folded content line");
        assert_eq!(line.name, b"DESCRIPTION");
        assert_eq!(
            line.value, b"standup and review",
            "a fold is removed on the way in, wherever the producer put it"
        );
        assert_eq!(line.ending, Some(LineEnding::CrLf));
    }

    /// The bound is the caller's and the refusal is exact: the longest value that fits is
    /// written, and the one octet past it is refused rather than truncated.
    #[test]
    fn a_value_past_the_callers_bound_is_refused_at_the_octet_that_crosses_it() {
        let limits = Limits::DEFAULT.with_max_value_bytes(8);
        let cases: [(&[u8], Result<(), MutationError>); 2] = [
            (b"SUMMARY:12345678\r\n", Ok(())),
            (
                b"SUMMARY:123456789\r\n",
                Err(MutationError::ValueTooLarge { limit: 8 }),
            ),
        ];
        for (attempt, expected) in cases {
            let mut component = event(vec![Item::Property(folded(b"SUMMARY", b"standup"))]);
            let change = ProposedChange::Replace(RawText::from_bytes(attempt));
            assert_eq!(
                component.apply(&PropertyId::SUMMARY, &change, limits),
                expected
            );
        }
    }

    /// The longest thing this unit sees under the default policy. The grammar bounds a header
    /// and deliberately does not bound a value, so the only ceiling a value of this size meets
    /// is the caller's own — and under the default policy it clears that too, and is stored
    /// whole rather than in pieces.
    #[test]
    fn a_value_longer_than_any_line_is_stored_whole() {
        let mut replacement: Vec<u8> = Vec::from(&b"DESCRIPTION:"[..]);
        replacement.extend(core::iter::repeat_n(b'x', 65_536));
        replacement.extend_from_slice(b"\r\n");

        let line = read_replacement_line(&replacement, Limits::DEFAULT)
            .expect("a value has no ceiling but the caller's");
        assert_eq!(line.value.len(), 65_536);
    }

    /// A replacement carrying a violation is read, diagnosed by the reader, and kept — not
    /// refused. The write-side refusal is `set_raw`'s and is stated over the caller's own
    /// octets; a control character that reaches the tree through a content line cannot have
    /// been a terminator, because a terminator would have ended the line.
    #[test]
    fn a_replacement_carrying_a_violation_is_kept_rather_than_refused() {
        let mut component = event(vec![Item::Property(folded(b"SUMMARY", b"standup"))]);
        let change = ProposedChange::Replace(RawText::from_bytes(b"SUMMARY:re\x0bview\n"));
        assert_eq!(
            component.apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT),
            Ok(())
        );

        let written = component.items()[0]
            .as_property()
            .expect("still a property");
        assert_eq!(
            written.value_text().as_bytes(),
            b"re\x0bview",
            "the octets are kept as they arrived, for the reader to diagnose"
        );

        let mut guard: PropertyMut<'_, TextValue<'_>> = PropertyMut::new(
            component.items_mut()[0]
                .as_property_mut()
                .expect("still a property"),
        );
        assert_eq!(
            guard.set_raw(b"re\x0bview"),
            Err(MutationError::IllegalControlCharacter),
            "the same octets handed to a write directly are refused"
        );
    }

    /// The variant that earns its place: parameters change and the value's preserved text does
    /// not, while the line's layout goes because the parameters were part of that line.
    #[test]
    fn setting_parameters_leaves_the_values_text_exactly_as_it_was() {
        let mut component = event(vec![Item::Property(decorated(
            b"RECURRENCE-ID",
            &[(b"TZID", b"Europe/Paris")],
            b"20260815T090000",
        ))]);
        let change = ProposedChange::SetParameters(vec![
            ParameterEdit::set(b"RANGE", b"THISANDFUTURE"),
            ParameterEdit::remove(b"TZID"),
        ]);
        assert_eq!(
            component.apply(&PropertyId::RECURRENCE_ID, &change, Limits::DEFAULT),
            Ok(())
        );

        let written = component.items()[0]
            .as_property()
            .expect("still a property");
        assert_eq!(
            parameters_of(written),
            vec![(&b"RANGE"[..], &b"THISANDFUTURE"[..])]
        );
        assert_eq!(written.value_text().as_bytes(), b"20260815T090000");
    }

    /// RFC 5545 section 3.2 excludes `:` `;` and `,` from `SAFE-CHAR` and includes them in
    /// `QSAFE-CHAR`, so a value carrying one is written inside a `DQUOTE` pair. Unquoted, the
    /// `:` would end the header and the value's own text would move to the parameter side —
    /// which is the one thing `SetParameters` exists to promise it will not do.
    #[test]
    fn a_parameter_value_the_grammar_cannot_write_bare_is_written_quoted() {
        let cases: [(&[u8], &[u8]); 5] = [
            (b"a:b", b"\"a:b\""),
            (b"a;b", b"\"a;b\""),
            (b"Doe, John", b"\"Doe, John\""),
            (b"THISANDFUTURE", b"THISANDFUTURE"),
            (b"W. Europe Standard Time", b"W. Europe Standard Time"),
        ];
        for (assigned, spelled) in cases {
            let mut component = event(vec![Item::Property(decorated(
                b"SUMMARY",
                &[(b"X-STATE", b"old")],
                b"Lunch",
            ))]);
            let change =
                ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", assigned)]);
            assert_eq!(
                component.apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT),
                Ok(())
            );
            let written = component.items()[0]
                .as_property()
                .expect("still a property");
            assert_eq!(parameters_of(written), vec![(&b"X-STATE"[..], spelled)]);
            assert_eq!(
                written.value_text().as_bytes(),
                b"Lunch",
                "the value's text is what this variant promised not to touch"
            );
        }
    }

    /// The shapes section 3.2 has no spelling for. Each leaves the component exactly as it
    /// was, because the refusal runs over the whole list before the property is reached.
    #[test]
    fn a_parameter_edit_the_grammar_cannot_write_at_all_is_refused_and_writes_nothing() {
        let refused: [ParameterEdit; 6] = [
            // The injection `set_raw` refuses, arriving on the parameter channel instead.
            ParameterEdit::set(b"X-STATE", b"busy\r\nATTENDEE:mailto:eve@example.test"),
            // `QSAFE-CHAR` excludes `DQUOTE` and section 3.2 defines no escape for it.
            ParameterEdit::set(b"X-STATE", b"say \"hi\""),
            ParameterEdit::set(b"X-STATE", b"bell\x07"),
            ParameterEdit::set(b"X-A:B", b"busy"),
            ParameterEdit::set(b"", b"busy"),
            ParameterEdit::remove(b"X;A"),
        ];
        for attempt in refused {
            // A preserved layout rather than `decorated`'s canonical one, so that "the layout
            // survived" is an observation and not a tautology.
            let mut component = event(vec![Item::Property(Property::new(
                RawText::from_bytes(b"SUMMARY"),
                vec![Parameter::new(
                    RawText::from_bytes(b"X-STATE"),
                    RawText::from_bytes(b"old"),
                )],
                RawText::from_bytes(b"Lunch"),
                LineLayout::preserved(Vec::new(), Some(LineEnding::CrLf), true),
            ))]);
            let change = ProposedChange::SetParameters(vec![
                ParameterEdit::set(b"X-OK", b"kept"),
                attempt.clone(),
            ]);
            assert_eq!(
                component.apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT),
                Err(MutationError::NotRepresentable),
                "{attempt:?}"
            );
            let kept = component.items()[0]
                .as_property()
                .expect("still a property");
            assert_eq!(
                parameters_of(kept),
                vec![(&b"X-STATE"[..], &b"old"[..])],
                "the edit before the refused one was not applied either"
            );
            assert!(
                !kept.layout().is_refolded(),
                "a refused change does not even discard the layout"
            );
        }
    }

    /// An addition after a line that carried no terminator gives that line the terminator
    /// section 3.1 requires, because a line with something written after it is no longer last.
    #[test]
    fn an_addition_terminates_the_line_it_is_written_after() {
        let unterminated = Property::new(
            RawText::from_bytes(b"SUMMARY"),
            Vec::new(),
            RawText::from_bytes(b"Lunch"),
            LineLayout::preserved(Vec::new(), None, true),
        );
        let mut component = event(vec![Item::Property(unterminated)]);
        let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
        assert_eq!(
            component.apply(&PropertyId::COMMENT, &change, Limits::DEFAULT),
            Ok(())
        );

        let above = component.items()[0]
            .as_property()
            .expect("still a property");
        assert_eq!(
            above.layout().ending(),
            Some(LineEnding::CrLf),
            "the line above stopped being last and gained the octets that make it a line"
        );
        assert_eq!(
            above.value_text().as_bytes(),
            b"Lunch",
            "and nothing else about it moved"
        );
        assert!(
            !above.layout().is_refolded(),
            "a terminator is not a fold, and the recorded folds are not a write's to discard"
        );
    }

    /// A terminator already there is left alone, bare `LF` included: which one a producer
    /// wrote is a diagnostic about the file rather than something an addition corrects.
    #[test]
    fn an_addition_leaves_the_terminator_the_line_above_already_had() {
        let mut component = event(vec![Item::Property(Property::new(
            RawText::from_bytes(b"SUMMARY"),
            Vec::new(),
            RawText::from_bytes(b"Lunch"),
            LineLayout::preserved(Vec::new(), Some(LineEnding::Lf), true),
        ))]);
        let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
        assert_eq!(
            component.apply(&PropertyId::COMMENT, &change, Limits::DEFAULT),
            Ok(())
        );
        assert_eq!(
            component.items()[0]
                .as_property()
                .expect("still a property")
                .layout()
                .ending(),
            Some(LineEnding::Lf)
        );
    }

    /// With no property to sit after, the addition sits after the `BEGIN`, and that is the
    /// line the same rule applies to.
    #[test]
    fn an_addition_into_a_component_with_no_properties_terminates_the_begin_line() {
        let opening = Boundary::new(
            RawText::from_bytes(b"BEGIN"),
            RawText::from_bytes(b"VEVENT"),
            LineLayout::preserved(Vec::new(), None, true),
        );
        let mut component = Component::new(opening, Vec::new(), None);
        let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
        assert_eq!(
            component.apply(&PropertyId::COMMENT, &change, Limits::DEFAULT),
            Ok(())
        );
        assert_eq!(
            component.begin().layout().ending(),
            Some(LineEnding::CrLf),
            "the BEGIN line stopped being last too"
        );
    }

    /// An addition goes after the properties and before the subcomponents, and nothing already
    /// in the component moves relative to anything else.
    #[test]
    fn an_addition_lands_among_the_properties_and_reorders_nothing() {
        let alarm = Component::new(
            Boundary::new(
                RawText::from_bytes(b"BEGIN"),
                RawText::from_bytes(b"VALARM"),
                LineLayout::canonical(LineEnding::CANONICAL),
            ),
            Vec::new(),
            None,
        );
        let mut component = event(vec![
            Item::Property(folded(b"UID", b"1@example.test")),
            Item::Property(folded(b"SUMMARY", b"standup")),
            Item::Component(alarm),
        ]);

        let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
        assert_eq!(
            component.apply(&PropertyId::COMMENT, &change, Limits::DEFAULT),
            Ok(())
        );

        let order: Vec<&[u8]> = component
            .items()
            .iter()
            .map(|entry| match entry {
                Item::Property(property) => property.name().as_bytes(),
                Item::Component(nested) => nested.name().as_bytes(),
            })
            .collect();
        assert_eq!(
            order,
            vec![
                &b"UID"[..],
                &b"SUMMARY"[..],
                &b"COMMENT"[..],
                &b"VALARM"[..]
            ]
        );
        let added = component.items()[2].as_property().expect("the addition");
        assert_eq!(added.value_text().as_bytes(), b"added");
        assert!(
            added.layout().is_refolded(),
            "a line this crate wrote is folded canonically"
        );
    }

    /// A removal names an identity, so every property carrying it goes; a removal of what was
    /// never there is absence rather than a silent success.
    #[test]
    fn a_removal_takes_every_occurrence_and_reports_finding_none() {
        let mut component = event(vec![
            Item::Property(folded(b"CATEGORIES", b"one")),
            Item::Property(folded(b"SUMMARY", b"standup")),
            Item::Property(folded(b"categories", b"two")),
        ]);
        assert_eq!(
            component.apply(
                &PropertyId::CATEGORIES,
                &ProposedChange::Remove,
                Limits::DEFAULT
            ),
            Ok(())
        );

        let order: Vec<&[u8]> = component
            .properties()
            .map(|property| property.name().as_bytes())
            .collect();
        assert_eq!(order, vec![&b"SUMMARY"[..]]);
        assert_eq!(
            component.apply(
                &PropertyId::CATEGORIES,
                &ProposedChange::Remove,
                Limits::DEFAULT
            ),
            Err(MutationError::Absent)
        );
    }

    /// A change addressed to a property the component does not carry is absence, whichever
    /// change it was — and nothing is added on the way to finding out.
    #[test]
    fn a_change_to_a_property_that_is_not_here_is_absent() {
        let mut component = event(vec![Item::Property(folded(b"SUMMARY", b"standup"))]);
        let replace = ProposedChange::Replace(RawText::from_bytes(b"DTSTART:20260815\r\n"));
        assert_eq!(
            component.apply(&PropertyId::DTSTART, &replace, Limits::DEFAULT),
            Err(MutationError::Absent)
        );

        let parameters = ProposedChange::SetParameters(vec![ParameterEdit::remove(b"TZID")]);
        assert_eq!(
            component.apply(&PropertyId::DTSTART, &parameters, Limits::DEFAULT),
            Err(MutationError::Absent)
        );
        assert_eq!(component.items().len(), 1);
    }

    /// The injection the construction door used to be open to. `Property::new` was public and
    /// unchecked, so the octets a scoped write refuses arrived through the tree builder
    /// instead and serialized as two content lines.
    #[test]
    fn construction_refuses_the_octets_a_write_refuses() {
        let refused: [&[u8]; 4] = [
            b"a\r\nATTENDEE:mailto:eve@example.test",
            b"a\n",
            b"a\r",
            b"a\x00b",
        ];
        for attempt in refused {
            assert_eq!(
                Property::create(b"SUMMARY", Vec::new(), attempt),
                Err(MutationError::IllegalControlCharacter),
                "{attempt:?}"
            );
        }
    }

    /// A name the reader would hand back in pieces is a name this crate declines to author,
    /// whether it names a property or a component.
    #[test]
    fn construction_refuses_a_name_that_would_not_read_back_whole() {
        let refused: [&[u8]; 4] = [b"", b"SUMMARY:x", b"SUMMARY;X-A=1", b"SUM\r\nATTENDEE"];
        for attempt in refused {
            assert_eq!(
                Property::create(attempt, Vec::new(), b"standup"),
                Err(MutationError::NotRepresentable),
                "{attempt:?}"
            );
            assert_eq!(
                Component::create(attempt, Vec::new()),
                Err(MutationError::NotRepresentable),
                "{attempt:?}"
            );
        }
        assert_eq!(
            Parameter::create(b"X-A;B", b"1"),
            Err(MutationError::NotRepresentable)
        );
        assert_eq!(
            Parameter::create(b"CN", b"say \"hi\""),
            Err(MutationError::NotRepresentable),
            "a DQUOTE has no section 3.2 spelling, so a parameter carrying one is refused"
        );
    }

    /// What construction accepts is written back as the caller asked, quoting only what the
    /// grammar forces and closing a component with the name it opened.
    #[test]
    fn what_construction_accepts_serializes_as_the_caller_stated_it() {
        let attendee = Property::create(
            b"ATTENDEE",
            vec![Parameter::create(b"CN", b"Doe, John").expect("a quotable value")],
            b"mailto:j@example.test",
        )
        .expect("a well-formed property");
        assert_eq!(
            parameters_of(&attendee),
            vec![(&b"CN"[..], &b"\"Doe, John\""[..])]
        );

        let event = Component::create(b"VEVENT", vec![Item::Property(attendee)])
            .expect("a well-formed component");
        let document = crate::tree::Document::new(vec![Item::Component(event)]);
        assert_eq!(
            document.to_bytes(),
            b"BEGIN:VEVENT\r\nATTENDEE;CN=\"Doe, John\":mailto:j@example.test\r\nEND:VEVENT\r\n"
        );
    }

    /// The parsed shape a replacement is written from, so that a change to the reader shows up
    /// here rather than only through a component.
    #[test]
    fn a_parsed_line_carries_the_whole_content_line_and_nothing_else() {
        let line: ParsedLine =
            read_replacement_line(b"SUMMARY;X-A=1;X-B:standup\r\n", Limits::DEFAULT)
                .expect("one content line");
        assert_eq!(line.name, b"SUMMARY");
        assert_eq!(line.parameters.len(), 2);
        assert!(
            !line.parameters[1].has_value(),
            "a parameter with no `=` is preserved as one, not repaired"
        );
        assert_eq!(line.value, b"standup");
    }
}
