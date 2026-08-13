// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private, allocation-aware iCalendar kernel.

pub(crate) mod access;
pub(crate) mod arith;
pub(crate) mod change;
pub(crate) mod codec;
pub(crate) mod emit;
pub(crate) mod grammar;
pub(crate) mod gregorian;
pub(crate) mod ident;
pub(crate) mod mutate;
pub(crate) mod octets;
pub(crate) mod output;
pub(crate) mod parse;
pub(crate) mod schema;
pub(crate) mod tree;
pub(crate) mod view;

pub use self::change::{ParameterEdit, ProposedChange};
pub use self::grammar::*;
pub use self::gregorian::{
    CivilDate, CivilDateTime, CivilTime, DateTimeValue, Duration, MonthAddOutcome, UtcOffset,
    Weekday,
};
pub use self::ident::PropertyId;
pub use self::octets::{RawText, TextError};
pub use self::output::Writer;
pub use self::schema::*;
pub use self::tree::{
    Boundary, Component, Document, Item, Parameter, ParametersNamed, PropertiesNamed, Property,
};
pub use self::view::{
    BinaryValue, DecodeValue, EncodeValue, Geo, MutationError, Period, PropertyMut, TextValue,
    UriValue, ValueBuf, ValueType, View,
};
