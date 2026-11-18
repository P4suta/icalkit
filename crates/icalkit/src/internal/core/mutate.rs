// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Edits to direct properties of a component.
//! A value write discards the edited line's recorded folds.
//! Typed values supply their own parameter edits through [`EncodeValue::coupled_parameters`].
//! Other parameters keep their spelling and order.
//! [`Component::apply`] reads each replacement as a single content line.

use alloc::vec::Vec;

use crate::internal::core::{
    ContentLineReader, ContentLineSource, Limits, LineEnding, LineLayout, Token, encode_caret,
    parameter_is_representable, parameter_name_is_representable, parameter_value_accepts_delimiter,
    property_name_is_representable, quote_parameter_into,
};

use crate::internal::core::change::{ParameterEdit, ProposedChange};
use crate::internal::core::gregorian::DateTimeValue;
use crate::internal::core::ident::PropertyId;
use crate::internal::core::octets::RawText;
use crate::internal::core::parse::names_a_component_boundary;
use crate::internal::core::tree::{Boundary, Component, Item, Parameter, Property};
use crate::internal::core::view::{EncodeValue, MutationError, PropertyMut, ValueBuf};

/// Reject ASCII control characters in caller-supplied raw values.
/// RFC 5545 section 3.1 permits `HTAB` in input values.
fn refuse_control_characters(bytes: &[u8]) -> Result<(), MutationError> {
    if bytes.iter().any(u8::is_ascii_control) {
        return Err(MutationError::IllegalControlCharacter);
    }
    Ok(())
}

/// Reject `BEGIN` and `END` because a write could change the component's nesting.
fn refuse_component_boundary(name: &[u8]) -> Result<(), MutationError> {
    if names_a_component_boundary(name) {
        return Err(MutationError::ComponentBoundary);
    }
    Ok(())
}

/// Check the unfolded value size before copying each chunk.
fn refuse_oversized_value(
    written: usize,
    arriving: usize,
    limits: Limits,
) -> Result<(), MutationError> {
    let ceiling = limits.max_value_bytes();
    let held = u64::try_from(written).unwrap_or(u64::MAX);
    let added = u64::try_from(arriving).unwrap_or(u64::MAX);
    if held.saturating_add(added) > u64::from(ceiling) {
        return Err(MutationError::ValueTooLarge { limit: ceiling });
    }
    Ok(())
}

/// Check whether the caret-encoded value has a legal parameter spelling.
fn parameter_value_is_writable(value: &[u8]) -> bool {
    parameter_is_representable(encode_caret(value).as_ref())
}

/// Check each parameter name and value before applying any edits.
fn refuse_unwritable_edits(edits: &[ParameterEdit]) -> Result<(), MutationError> {
    for entry in edits {
        if !parameter_name_is_representable(entry.name()) {
            return Err(MutationError::NotRepresentable);
        }
        if entry
            .value()
            .is_some_and(|held| !parameter_value_is_writable(held))
        {
            return Err(MutationError::NotRepresentable);
        }
    }
    Ok(())
}

/// Encode the value under RFC 6868 before quoting its delimiters under RFC 5545 section 3.2.
fn spelled_parameter(name: &[u8], value: &[u8]) -> Parameter {
    let mut spelled = Vec::new();
    quote_parameter_into(encode_caret(value).as_ref(), &mut spelled);
    Parameter::new(RawText::from_bytes(name), RawText::from_vec(spelled))
}

/// Replace the name, all parameters, and the value in one property.
fn write_whole_line(property: &mut Property, line: &ParsedLine) {
    property.set_name(RawText::from_bytes(&line.name));
    let parameters = property.edit_parameters();
    parameters.clear();
    parameters.extend(line.parameters.iter().cloned());
    property.set_value_text(RawText::from_bytes(&line.value));
}

impl Parameter {
    /// Create a parameter from its name and decoded value.
    /// The writer encodes carets and quotes delimiters so the reader recovers the supplied value.
    ///
    /// # Errors
    ///
    /// Returns [`MutationError::NotRepresentable`] for an invalid name or a value with no legal spelling.
    pub fn create(name: &[u8], value: &[u8]) -> Result<Self, MutationError> {
        if !parameter_name_is_representable(name) || !parameter_value_is_writable(value) {
            return Err(MutationError::NotRepresentable);
        }
        Ok(spelled_parameter(name, value))
    }
}

impl Property {
    /// Create a property with canonical folds and a `CRLF` terminator.
    ///
    /// # Errors
    ///
    /// Returns [`MutationError::NotRepresentable`] for an invalid property name.
    /// Returns [`MutationError::ComponentBoundary`] for `BEGIN` or `END`.
    /// Returns [`MutationError::IllegalControlCharacter`] for an ASCII control character in the value.
    pub fn create(
        name: &[u8],
        parameters: Vec<Parameter>,
        value: &[u8],
    ) -> Result<Self, MutationError> {
        if !property_name_is_representable(name) {
            return Err(MutationError::NotRepresentable);
        }
        refuse_component_boundary(name)?;
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
    /// Create a component whose `BEGIN` and `END` lines use the supplied name and case.
    ///
    /// # Errors
    ///
    /// Returns [`MutationError::NotRepresentable`] for an invalid component name.
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

/// Replace all matching parameters at the first match's position, or append a new parameter.
fn assign_parameter(parameters: &mut Vec<Parameter>, name: &[u8], value: &[u8]) {
    let at = parameters.iter().position(|held| held.is_named(name));
    parameters.retain(|held| !held.is_named(name));
    let written = spelled_parameter(name, value);
    match at {
        Some(index) => parameters.insert(index, written),
        None => parameters.push(written),
    }
}

/// Refuse retained parameter quotes that would consume the emitted value separator.
fn refuse_unclosed_parameter(
    property: &Property,
    edits: &[ParameterEdit],
) -> Result<(), MutationError> {
    if property.parameters().iter().any(|parameter| {
        !parameter_value_accepts_delimiter(parameter.value().as_bytes())
            && !edits.iter().any(|edit| parameter.is_named(edit.name()))
    }) {
        return Err(MutationError::NotRepresentable);
    }
    Ok(())
}

/// Apply the checked edits in order.
fn apply_parameter_edits(parameters: &mut Vec<Parameter>, edits: &[ParameterEdit]) {
    for entry in edits {
        match entry.value() {
            Some(assigned) => assign_parameter(parameters, entry.name(), assigned),
            None => parameters.retain(|held| !held.is_named(entry.name())),
        }
    }
}

/// Owned parts of one replacement content line.
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
            // A replacement requires a colon even when its value is empty.
            if !has_separator {
                return Err(MutationError::MalformedReplacement);
            }
            line.ending = ending;
            Ok(true)
        },
    }
}

/// Read one named content line under the caller's grammar and value limits.
fn read_replacement_line(bytes: &[u8], limits: Limits) -> Result<ParsedLine, MutationError> {
    let mut reader = ContentLineReader::new(bytes, limits.grammar());
    let mut line = ParsedLine::default();
    let mut terminated = false;
    while let Some(token) = reader.next_token() {
        if terminated {
            // Tokens after the end of the first line: the octets describe more than one change, and this call names one property.
            return Err(MutationError::MalformedReplacement);
        }
        let piece = token.map_err(|_bound| MutationError::MalformedReplacement)?;
        terminated = take_token(&mut line, piece, limits)?;
    }
    // Empty octets end the loop before it starts, so "empty" needs no case of its own.
    // An empty name is the blank line's shape and is no more a replacement than nothing is.
    if !terminated || line.name.is_empty() {
        return Err(MutationError::MalformedReplacement);
    }
    Ok(line)
}

/// Check that the replacement names the property the caller selected.
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
    /// Write raw value bytes and discard the old fold positions.
    /// The name, parameters, and terminator keep their original spelling.
    ///
    /// # Errors
    ///
    /// Returns [`MutationError::IllegalControlCharacter`] for an ASCII control character.
    /// Returns [`MutationError::ComponentBoundary`] for `BEGIN` or `END`.
    /// Returns [`MutationError::NotRepresentable`] if an unclosed parameter quote hides the value separator.
    /// An error leaves the property intact.
    pub fn set_raw(&mut self, bytes: &[u8]) -> Result<(), MutationError> {
        refuse_component_boundary(self.property().name().as_bytes())?;
        refuse_control_characters(bytes)?;
        refuse_unclosed_parameter(self.property(), &[])?;
        self.property_mut()
            .set_value_text(RawText::from_bytes(bytes));
        Ok(())
    }
}

impl<T: EncodeValue> PropertyMut<'_, T> {
    /// Write the encoded value and the parameters its type supplies.
    ///
    /// # Errors
    ///
    /// Encoding errors and invalid parameters leave the property intact.
    /// An unclosed parameter quote that survives the edits returns [`MutationError::NotRepresentable`].
    /// The same boundary and control checks apply as in [`PropertyMut::set_raw`].
    pub fn set(&mut self, value: &T) -> Result<(), MutationError> {
        refuse_component_boundary(self.property().name().as_bytes())?;
        let mut encoded = ValueBuf::new();
        value.encode_value(&mut encoded)?;
        refuse_control_characters(encoded.as_bytes())?;

        let mut coupled: Vec<ParameterEdit> = Vec::new();
        value.coupled_parameters(&mut coupled);
        refuse_unwritable_edits(&coupled)?;
        refuse_unclosed_parameter(self.property(), &coupled)?;

        let property = self.property_mut();
        apply_parameter_edits(property.edit_parameters(), &coupled);
        property.set_value_text(encoded.into_raw_text());
        Ok(())
    }
}

impl Component {
    /// Borrow the first direct property with the given identity, or return `None` if absent.
    pub fn get_mut<T>(&mut self, id: &PropertyId) -> Option<PropertyMut<'_, T>> {
        self.property_with_id_mut(id).map(PropertyMut::new)
    }

    /// Borrow the first `DTSTART` property as a date-time value.
    /// A zoned value sets `TZID` and every other date-time form removes it.
    pub fn dtstart_mut(&mut self) -> Option<PropertyMut<'_, DateTimeValue<'_>>> {
        self.get_mut(&PropertyId::DTSTART)
    }

    /// Apply a change to the direct properties named by `id`.
    /// Replacement, parameter edits, and removal affect every matching occurrence.
    /// Addition inserts one property.
    ///
    /// # Errors
    ///
    /// Returns [`MutationError::MalformedReplacement`] unless the supplied line has the target name and parses as one property.
    /// Returns [`MutationError::ValueTooLarge`] if the replacement value exceeds the caller's limit.
    /// Returns [`MutationError::Absent`] for an edit that requires a missing target.
    /// Writing `BEGIN` or `END` returns [`MutationError::ComponentBoundary`].
    /// An invalid parameter edit or a surviving unclosed parameter quote returns [`MutationError::NotRepresentable`].
    pub fn apply(
        &mut self,
        id: &PropertyId,
        change: &ProposedChange,
        limits: Limits,
    ) -> Result<(), MutationError> {
        match change {
            ProposedChange::Replace(replacement) => {
                refuse_component_boundary(id.as_bytes())?;
                let line = read_named_line(id, replacement.as_bytes(), limits)?;
                self.overwrite(id, &line)
            },
            ProposedChange::Add(addition) => {
                refuse_component_boundary(id.as_bytes())?;
                let line = read_named_line(id, addition.as_bytes(), limits)?;
                self.insert_after_properties(line);
                Ok(())
            },
            ProposedChange::SetParameters(edits) => {
                refuse_component_boundary(id.as_bytes())?;
                refuse_unwritable_edits(edits)?;
                for property in self.properties_named(id) {
                    refuse_unclosed_parameter(property, edits)?;
                }
                let reached = self.for_each_property_with_id(id, |property| {
                    apply_parameter_edits(property.edit_parameters(), edits);
                });
                if reached {
                    Ok(())
                } else {
                    Err(MutationError::Absent)
                }
            },
            ProposedChange::Remove => self.remove_named(id),
        }
    }

    /// Apply a change to one direct property named by `id`, counting from zero.
    /// An RFC 5546 scheduling message can target one `ATTENDEE` in a repeated list.
    /// For [`ProposedChange::Add`], `index` must equal the number of existing occurrences.
    ///
    /// # Errors
    ///
    /// Returns the errors from [`Component::apply`].
    /// Returns [`MutationError::Absent`] for a missing occurrence or an incorrect append index.
    pub fn apply_to_occurrence(
        &mut self,
        id: &PropertyId,
        index: usize,
        change: &ProposedChange,
        limits: Limits,
    ) -> Result<(), MutationError> {
        match change {
            ProposedChange::Add(addition) => {
                refuse_component_boundary(id.as_bytes())?;
                if index != self.occurrence_count(id) {
                    return Err(MutationError::Absent);
                }
                let line = read_named_line(id, addition.as_bytes(), limits)?;
                self.insert_after_properties(line);
                Ok(())
            },
            ProposedChange::Replace(replacement) => {
                refuse_component_boundary(id.as_bytes())?;
                let line = read_named_line(id, replacement.as_bytes(), limits)?;
                let property = self.occurrence_mut(id, index)?;
                write_whole_line(property, &line);
                Ok(())
            },
            ProposedChange::SetParameters(edits) => {
                refuse_component_boundary(id.as_bytes())?;
                refuse_unwritable_edits(edits)?;
                let property = self.occurrence_mut(id, index)?;
                refuse_unclosed_parameter(property, edits)?;
                apply_parameter_edits(property.edit_parameters(), edits);
                Ok(())
            },
            ProposedChange::Remove => {
                let at = self
                    .occurrence_position(id, index)
                    .ok_or(MutationError::Absent)?;
                self.items_mut().remove(at);
                Ok(())
            },
        }
    }

    /// How many properties directly inside this component carry the identity `id`.
    fn occurrence_count(&self, id: &PropertyId) -> usize {
        self.properties().filter(|item| item.has_id(id)).count()
    }

    /// Find a direct property's position by name and zero-based occurrence.
    fn occurrence_position(&self, id: &PropertyId, index: usize) -> Option<usize> {
        self.items()
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .as_property()
                    .is_some_and(|property| property.has_id(id))
            })
            .map(|(at, _)| at)
            .nth(index)
    }

    /// The `index`th property named `id`, mutably.
    fn occurrence_mut(
        &mut self,
        id: &PropertyId,
        index: usize,
    ) -> Result<&mut Property, MutationError> {
        let at = self
            .occurrence_position(id, index)
            .ok_or(MutationError::Absent)?;
        self.items_mut()
            .get_mut(at)
            .and_then(Item::as_property_mut)
            .ok_or(MutationError::Absent)
    }

    /// Visit every direct property named by `id` and report whether any matched.
    fn for_each_property_with_id<F: FnMut(&mut Property)>(
        &mut self,
        id: &PropertyId,
        mut act: F,
    ) -> bool {
        let mut reached = false;
        for property in self
            .items_mut()
            .iter_mut()
            .filter_map(Item::as_property_mut)
        {
            if !property.has_id(id) {
                continue;
            }
            act(property);
            reached = true;
        }
        reached
    }

    /// The first property directly inside this component with the identity `id`, mutably.
    fn property_with_id_mut(&mut self, id: &PropertyId) -> Option<&mut Property> {
        self.items_mut()
            .iter_mut()
            .filter_map(Item::as_property_mut)
            .find(|property| property.has_id(id))
    }

    fn overwrite(&mut self, id: &PropertyId, line: &ParsedLine) -> Result<(), MutationError> {
        let reached = self.for_each_property_with_id(id, |property| {
            write_whole_line(property, line);
        });
        if reached {
            Ok(())
        } else {
            Err(MutationError::Absent)
        }
    }

    /// Insert after the last direct property, or before the first child if no property exists.
    /// Use the supplied terminator or `CRLF` when absent.
    fn insert_after_properties(&mut self, line: ParsedLine) {
        let layout = LineLayout::canonical(line.ending.unwrap_or(LineEnding::CANONICAL));
        let property = Property::new(
            RawText::from_vec(line.name),
            line.parameters,
            RawText::from_vec(line.value),
            layout,
        );
        let at = self
            .items()
            .iter()
            .rposition(|entry| entry.as_property().is_some())
            .map_or(0, |index| index.saturating_add(1));
        self.terminate_line_above(at);
        self.items_mut().insert(at, Item::Property(property));
    }

    /// Add `CRLF` to an unterminated predecessor before inserting a property.
    /// The predecessor can be the component's `BEGIN` line.
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

    /// Remove all direct properties named by `id`.
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

    use crate::internal::core::{FoldPoint, Limits, LineEnding, LineLayout};

    use super::{ParsedLine, read_replacement_line};
    use crate::internal::core::change::{ParameterEdit, ProposedChange};
    use crate::internal::core::ident::PropertyId;
    use crate::internal::core::octets::RawText;
    use crate::internal::core::tree::{Boundary, Component, Item, Parameter, Property};
    use crate::internal::core::view::{
        EncodeValue, MutationError, PropertyMut, TextValue, ValueBuf,
    };

    /// A property folded once, so that a write discarding this line's layout is observable and a neighbor keeping its own is observable beside it.
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

    /// Model a typed value that sets `VALUE` and removes `TZID`.
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

    #[test]
    fn a_value_longer_than_any_line_is_stored_whole() {
        let mut replacement: Vec<u8> = Vec::from(&b"DESCRIPTION:"[..]);
        replacement.extend(core::iter::repeat_n(b'x', 65_536));
        replacement.extend_from_slice(b"\r\n");

        let line = read_replacement_line(&replacement, Limits::DEFAULT)
            .expect("a value has no ceiling but the caller's");
        assert_eq!(line.value.len(), 65_536);
    }

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

    #[test]
    fn a_parameter_edit_cannot_extend_an_unclosed_quoted_parameter() {
        use crate::internal::core::Document;

        let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X", b"a:b")]);
        for prefix in ["", "P:keep\r\n"] {
            let source = alloc::format!("BEGIN:C\r\n{prefix}P;Q=\"\r\nEND:C\r\n");
            for occurrence in [false, true] {
                let mut document =
                    Document::parse(source.as_bytes(), Limits::DEFAULT, &mut vec![]).unwrap();
                let component = document.items_mut()[0].as_component_mut().unwrap();
                let id = PropertyId::from_name(b"P");
                let position = usize::from(!prefix.is_empty());
                let property = component.items()[position].as_property().unwrap();
                assert_eq!(property.value_text().as_bytes(), b"");
                let result = if occurrence {
                    component.apply_to_occurrence(&id, position, &change, Limits::DEFAULT)
                } else {
                    component.apply(&id, &change, Limits::DEFAULT)
                };
                let written = document.to_bytes();
                let reopened = Document::parse(&written, Limits::DEFAULT, &mut vec![]).unwrap();
                let property = reopened.items()[0].as_component().unwrap().items()[position]
                    .as_property()
                    .unwrap();
                assert_eq!(
                    property.value_text().as_bytes(),
                    b"",
                    "input {} output {}",
                    source.as_bytes().escape_ascii(),
                    written.escape_ascii(),
                );
                assert_eq!(result, Err(MutationError::NotRepresentable));
                assert_eq!(written, source.as_bytes());
            }
        }
    }

    #[test]
    fn canonical_layout_does_not_make_unclosed_parameters_writable() {
        use crate::internal::core::Document;

        for source in [b"P;Q=\"\r\n".as_slice(), b"P;Q=ok,\"\r\n"] {
            let document = Document::parse(source, Limits::DEFAULT, &mut vec![]).unwrap();
            let bad = document.items()[0].as_property().unwrap().parameters()[0].clone();
            for position in 0..=2 {
                let mut parameters = vec![
                    Parameter::create(b"A", b"first").unwrap(),
                    Parameter::create(b"B", b"last").unwrap(),
                ];
                parameters.insert(position, bad.clone());
                let original = Property::create(b"P", parameters, b"before").unwrap();
                assert!(original.layout().has_separator());
                for raw in [false, true] {
                    let mut property = original.clone();
                    let mut guard = PropertyMut::new(&mut property);
                    let result = if raw {
                        guard.set_raw(b"after")
                    } else {
                        guard.set(&TextValue::from_bytes(b"after"))
                    };
                    assert_eq!(result, Err(MutationError::NotRepresentable));
                    assert_eq!(property, original);
                }
                let mut component = event(vec![Item::Property(original.clone())]);
                let change =
                    ProposedChange::SetParameters(vec![ParameterEdit::set(b"X", b"added")]);
                assert_eq!(
                    component.apply(&PropertyId::from_name(b"P"), &change, Limits::DEFAULT),
                    Err(MutationError::NotRepresentable)
                );
                assert_eq!(component.items()[0].as_property(), Some(&original));
                for edit in [
                    ParameterEdit::remove(b"q"),
                    ParameterEdit::set(b"q", b"closed"),
                ] {
                    let mut repaired = component.clone();
                    repaired
                        .apply(
                            &PropertyId::from_name(b"P"),
                            &ProposedChange::SetParameters(vec![edit]),
                            Limits::DEFAULT,
                        )
                        .unwrap();
                    repaired
                        .get_mut::<TextValue<'_>>(&PropertyId::from_name(b"P"))
                        .unwrap()
                        .set_raw(b"after")
                        .unwrap();
                    let written = Document::new(vec![Item::Component(repaired)]).to_bytes();
                    let reopened = Document::parse(&written, Limits::DEFAULT, &mut vec![]).unwrap();
                    let property = reopened.items()[0].as_component().unwrap().items()[0]
                        .as_property()
                        .unwrap();
                    assert_eq!(property.value_text().as_bytes(), b"after");
                }
            }
        }
    }

    #[test]
    fn a_value_write_requires_a_reachable_value_separator() {
        use crate::internal::core::Document;

        let source = b"BEGIN:C\r\nP;Q=ok,\"\r\nEND:C\r\n";
        for raw in [false, true] {
            let mut document = Document::parse(source, Limits::DEFAULT, &mut vec![]).unwrap();
            let property = document.items_mut()[0]
                .as_component_mut()
                .unwrap()
                .items_mut()[0]
                .as_property_mut()
                .unwrap();
            let mut guard = PropertyMut::new(property);
            let result = if raw {
                guard.set_raw(b"text")
            } else {
                guard.set(&TextValue::from_bytes(b"text"))
            };
            assert_eq!(result, Err(MutationError::NotRepresentable));
            assert_eq!(document.to_bytes(), source);
        }
    }

    #[test]
    fn a_parameter_edit_can_repair_a_colonless_header() {
        use crate::internal::core::Document;

        for (original, edit, expected) in [
            ("Q=\"", ParameterEdit::remove(b"q"), "P:"),
            ("Q=\"", ParameterEdit::set(b"q", b"ok"), "P;q=ok:"),
            (
                "Q=a\"b",
                ParameterEdit::set(b"X", b"a:b"),
                "P;Q=a\"b;X=\"a:b\":",
            ),
            (
                "Q=\"a,b\",c",
                ParameterEdit::set(b"X", b"ok"),
                "P;Q=\"a,b\",c;X=ok:",
            ),
        ] {
            let source = alloc::format!("BEGIN:C\r\nP;{original}\r\nEND:C\r\n");
            let mut document =
                Document::parse(source.as_bytes(), Limits::DEFAULT, &mut vec![]).unwrap();
            let component = document.items_mut()[0].as_component_mut().unwrap();
            component
                .apply(
                    &PropertyId::from_name(b"P"),
                    &ProposedChange::SetParameters(vec![edit]),
                    Limits::DEFAULT,
                )
                .unwrap();
            let expected = alloc::format!("BEGIN:C\r\n{expected}\r\nEND:C\r\n");
            assert_eq!(document.to_bytes(), expected.as_bytes());
            let reopened =
                Document::parse(expected.as_bytes(), Limits::DEFAULT, &mut vec![]).unwrap();
            let property = reopened.items()[0].as_component().unwrap().items()[0]
                .as_property()
                .unwrap();
            assert_eq!(property.value_text().as_bytes(), b"");
        }
    }

    /// RFC 5545 section 3.2 permits comma, colon and semicolon inside a quoted parameter value.
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

    #[test]
    fn a_parameter_edit_the_grammar_cannot_write_at_all_is_refused_and_writes_nothing() {
        let refused: [ParameterEdit; 6] = [
            // The injection `set_raw` refuses, arriving on the parameter channel instead.
            // RFC 6868 spells the `LF` and has no pair for the `CR`, so the value still has none.
            ParameterEdit::set(b"X-STATE", b"busy\r\nATTENDEE:mailto:eve@example.test"),
            ParameterEdit::set(b"X-STATE", b"carriage\rreturn"),
            ParameterEdit::set(b"X-STATE", b"bell\x07"),
            ParameterEdit::set(b"X-A:B", b"busy"),
            ParameterEdit::set(b"", b"busy"),
            ParameterEdit::remove(b"X;A"),
        ];
        for attempt in refused {
            // A preserved layout rather than `decorated`'s canonical one, so that "the layout survived" is an observation and not a tautology.
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

    /// An addition goes after the properties and before the subcomponents, and nothing already in the component moves relative to anything else.
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

    #[test]
    fn an_occurrence_addressed_write_reaches_one_line_of_a_repeated_name() {
        let attendee = |address: &[u8]| {
            Item::Property(decorated(
                b"ATTENDEE",
                &[(&b"PARTSTAT"[..], &b"NEEDS-ACTION"[..])],
                address,
            ))
        };
        let mut component = event(vec![
            attendee(b"mailto:ann@example.com"),
            attendee(b"mailto:bo@example.com"),
            attendee(b"mailto:cy@example.com"),
        ]);

        let answered =
            ProposedChange::SetParameters(vec![ParameterEdit::set(b"PARTSTAT", b"ACCEPTED")]);
        assert_eq!(
            component.apply_to_occurrence(&PropertyId::ATTENDEE, 1, &answered, Limits::DEFAULT),
            Ok(())
        );

        let answers: Vec<&[u8]> = component
            .properties()
            .map(|property| property.parameters()[0].value().as_bytes())
            .collect();
        assert_eq!(
            answers,
            vec![&b"NEEDS-ACTION"[..], &b"ACCEPTED"[..], &b"NEEDS-ACTION"[..]],
            "one attendee answered and the other two did not"
        );

        assert_eq!(
            component.apply_to_occurrence(&PropertyId::ATTENDEE, 3, &answered, Limits::DEFAULT),
            Err(MutationError::Absent),
            "there is no fourth attendee to answer for"
        );
    }

    #[test]
    fn an_occurrence_addressed_addition_must_name_the_append_position() {
        let mut component = event(vec![Item::Property(folded(
            b"ATTENDEE",
            b"mailto:ann@example.com",
        ))]);
        let added = ProposedChange::Add(RawText::from_bytes(b"ATTENDEE:mailto:bo@example.com"));
        assert_eq!(
            component.apply_to_occurrence(&PropertyId::ATTENDEE, 0, &added, Limits::DEFAULT),
            Err(MutationError::Absent),
            "occurrence zero is taken, so an addition there would renumber the one that is"
        );
        assert_eq!(
            component.apply_to_occurrence(&PropertyId::ATTENDEE, 1, &added, Limits::DEFAULT),
            Ok(())
        );
        assert_eq!(component.properties().count(), 2);

        // And a removal takes exactly the one it names, not every namesake.
        assert_eq!(
            component.apply_to_occurrence(
                &PropertyId::ATTENDEE,
                0,
                &ProposedChange::Remove,
                Limits::DEFAULT
            ),
            Ok(())
        );
        let left: Vec<&[u8]> = component
            .properties()
            .map(|property| property.value_text().as_bytes())
            .collect();
        assert_eq!(left, vec![&b"mailto:bo@example.com"[..]]);
    }

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

    /// The injection the construction door used to be open to.
    /// `Property::new` was public and unchecked, so the octets a scoped write refuses arrived through the tree builder instead and serialized as two content lines.
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
            Parameter::create(b"CN", b"bell\x07"),
            Err(MutationError::NotRepresentable),
            "a control character has no spelling under either grammar"
        );
    }

    #[test]
    fn construction_refuses_the_two_names_that_are_component_boundaries() {
        for name in [&b"BEGIN"[..], b"END", b"begin", b"EnD"] {
            assert_eq!(
                Property::create(name, Vec::new(), b"VEVENT"),
                Err(MutationError::ComponentBoundary),
                "{name:?}"
            );
        }
        assert!(
            Property::create(b"X-BEGIN", Vec::new(), b"VEVENT").is_ok(),
            "the refusal is the name and not a prefix of it"
        );
    }

    /// Recovery can keep an unmatched `END` as a property.
    #[test]
    fn a_write_to_a_property_that_is_a_component_boundary_is_refused() {
        let mut property = Property::new(
            RawText::from_bytes(b"END"),
            Vec::new(),
            RawText::from_bytes(b"VTODO"),
            LineLayout::preserved(Vec::new(), Some(LineEnding::CrLf), true),
        );
        let mut guard: PropertyMut<'_, TextValue<'_>> = PropertyMut::new(&mut property);
        assert_eq!(
            guard.set_raw(b"VEVENT"),
            Err(MutationError::ComponentBoundary)
        );
        assert_eq!(property.value_text().as_bytes(), b"VTODO");

        let mut component = event(vec![Item::Property(property)]);
        let identity = PropertyId::from_name(b"END");
        let changes = [
            ProposedChange::Replace(RawText::from_bytes(b"END:VEVENT\r\n")),
            ProposedChange::Add(RawText::from_bytes(b"END:VEVENT\r\n")),
            ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-A", b"1")]),
        ];
        for change in changes {
            assert_eq!(
                component.apply(&identity, &change, Limits::DEFAULT),
                Err(MutationError::ComponentBoundary),
                "{change:?}"
            );
        }
        assert_eq!(
            component.apply(&identity, &ProposedChange::Remove, Limits::DEFAULT),
            Ok(()),
            "removing the line is not authoring one, so a removal is not refused"
        );
    }

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
        let document = crate::internal::core::tree::Document::new(vec![Item::Component(event)]);
        assert_eq!(
            document.to_bytes(),
            b"BEGIN:VEVENT\r\nATTENDEE;CN=\"Doe, John\":mailto:j@example.test\r\nEND:VEVENT\r\n"
        );
    }

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
