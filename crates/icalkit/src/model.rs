// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validated, read-only views over the lossless calendar CST.

use core::str;

use ical_core::{Component, DateTimeValue, DecodeValue as _, Item, Property};

use crate::time::IcalDateTime;

/// A validated iCalendar name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name<'a>(&'a str);

impl<'a> Name<'a> {
    /// Validate a component or property name.
    #[must_use]
    pub fn new(name: &'a str) -> Option<Self> {
        (!name.is_empty()
            && name
                .bytes()
                .all(|octet| octet.is_ascii_alphanumeric() || octet == b'-'))
        .then_some(Self(name))
    }

    /// The spelling retained by the calendar.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

/// A read-only reference to any validated component.
#[derive(Clone, Copy, Debug)]
pub struct ComponentRef<'a> {
    component: &'a Component,
}

impl<'a> ComponentRef<'a> {
    pub(crate) const fn new(component: &'a Component) -> Self {
        Self { component }
    }

    /// This component's validated name.
    #[must_use]
    pub fn name(self) -> Name<'a> {
        name_of(self.component.name().as_bytes())
    }

    /// Its direct properties, in source order.
    pub fn properties(self) -> impl Iterator<Item = PropertyRef<'a>> {
        self.component.properties().map(PropertyRef::new)
    }
}

/// A read-only reference to any validated property.
#[derive(Clone, Copy, Debug)]
pub struct PropertyRef<'a> {
    property: &'a Property,
}

impl<'a> PropertyRef<'a> {
    pub(crate) const fn new(property: &'a Property) -> Self {
        Self { property }
    }

    /// This property's validated name.
    #[must_use]
    pub fn name(self) -> Name<'a> {
        name_of(self.property.name().as_bytes())
    }

    /// The unfolded value octets exactly as retained.
    #[must_use]
    pub fn value(self) -> &'a [u8] {
        self.property.value_text().as_bytes()
    }
}

/// A validated VEVENT view.
#[derive(Clone, Copy, Debug)]
pub struct EventRef<'a> {
    component: &'a Component,
}

impl<'a> EventRef<'a> {
    pub(crate) const fn new(component: &'a Component) -> Self {
        Self { component }
    }

    /// The required UID, validated during calendar promotion.
    #[must_use]
    pub fn uid(self) -> &'a str {
        let bytes = self
            .component
            .properties()
            .find(|property| property.is_named(b"UID"))
            .map_or(&[][..], |property| property.value_text().as_bytes());
        str::from_utf8(bytes).unwrap_or_default()
    }

    /// Find one direct property by case-insensitive name.
    #[must_use]
    pub fn property(self, wanted: &str) -> Option<PropertyRef<'a>> {
        self.component
            .properties()
            .find(|property| property.is_named(wanted.as_bytes()))
            .map(PropertyRef::new)
    }

    /// The optional DTSTART, whose value was validated during calendar promotion.
    #[must_use]
    pub fn dtstart(self) -> Option<IcalDateTime> {
        let property = self
            .component
            .properties()
            .find(|property| property.is_named(b"DTSTART"))?;
        let value = DateTimeValue::decode_property(property).ok()?;
        crate::time::from_core_date_time(value)
    }

    /// View this event through the generic component API.
    #[must_use]
    pub const fn as_component(self) -> ComponentRef<'a> {
        ComponentRef::new(self.component)
    }

    /// Direct nested components, in source order.
    pub fn components(self) -> impl Iterator<Item = ComponentRef<'a>> {
        self.component
            .items()
            .iter()
            .filter_map(Item::as_component)
            .map(ComponentRef::new)
    }
}

fn name_of(bytes: &[u8]) -> Name<'_> {
    let text = str::from_utf8(bytes).unwrap_or_default();
    Name::new(text).unwrap_or(Name(""))
}
