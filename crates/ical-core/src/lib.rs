// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iCalendar (RFC 5545): the object model, the typed views over it, and serialization.
//!
//! Specification: RFC 5545, "Internet Calendaring and Scheduling Core Object Specification
//! (iCalendar)" <https://www.rfc-editor.org/rfc/rfc5545>.
//!
//! An `.ics` file is a tree of components — `VCALENDAR` wrapping `VEVENT`, `VTODO`,
//! `VJOURNAL`, `VFREEBUSY`, `VTIMEZONE` — built out of content lines, each a property name,
//! its parameters, and a value, folded at octet boundaries and escaped by rules that are
//! close enough to other formats to invite mistakes. Reading those content lines is the
//! grammar layer's job — a private module whose every item is re-exported here unchanged, so
//! `ical_core::Token` is the one spelling of that type. What this crate adds is the
//! tree, the typed views, scoped mutation, and serialization. It expands no recurrence,
//! resolves no `TZID`, and attaches no meaning to `METHOD`; those live in the crates above
//! it.
//!
//! It also owns the vocabulary the crates around it share, because they do not all depend on
//! each other and a type two siblings speak has to sit at their common root: the `Limits`
//! policy and its `Meter`, the civil-time primitives, and the change vocabulary `ical-itip`
//! reuses (see `docs/adr/0010` and `docs/adr/0011`).
//!
//! The model preserves everything it read (see `docs/adr/0001`). Vendor properties,
//! parameters on properties that are otherwise understood, components with no type here,
//! and the original text of values that are not interpreted all stay in position and in
//! order, and serialization writes them back byte for byte. Typed access is a *view* over
//! that preserved text, never the storage behind it: reading `DTSTART` parses on demand and
//! leaves what the writer wrote intact, which also settles cases where a value cannot be
//! reproduced from its parsed form. Discarding the parts a parser does not recognize is how
//! one client silently destroys another client's data, and it is the failure this crate
//! exists to make structurally impossible.
//!
//! Calendars in the wild violate the specification constantly, so a violation is a
//! diagnostic attached to the item it concerns rather than an error that throws the file
//! away. A caller that wants strictness reads the diagnostics; a caller that wants to show
//! the user their meeting still can.
//!
//! Input is hostile in the ordinary case, not the exotic one — an `.ics` arrives as a mail
//! attachment or over CalDAV from a server the user does not control. Nothing here is sized
//! from a length found in the input without checking it against the caller's limits and the
//! bytes actually present, and octets are charged against the caller's budget as they are
//! appended rather than counted once at the end (see `docs/adr/0007`).
//!
//! Parsing is staged, and every stage is public. The token layer is the parser and the
//! document tree is one consumer of the same path, so a caller with 64 KB of RAM reads a
//! calendar it can never hold, and the two cannot drift into separate grammars (see
//! `docs/adr/0008`).
//!
//! # Status
//!
//! The content line grammar is part of this crate rather than a crate below it, as of D-0003:
//! `src/grammar/` is a private module tree whose items are re-exported here, `ical_grammar` is
//! not a crate any more, and `gates/grammar-layering` plus the second rule of `just purity` are
//! what keep the layer a layer.
//!
//! Every item `docs/design/ical-core-api.md` commits to exists and is tested. The foundation —
//! the octet storage and its one decode point, the property identity, the civil-time and value
//! types, the tree nodes with the recorded line syntax, the change vocabulary, the typed view
//! shape and its mutation guard, the output sink — now carries the behavior built on it:
//! [`Document::parse`] and [`Document::from_tokens`] read a tree out of the public token path,
//! [`Document::serialize`] writes one back, the civil arithmetic is checked in every direction,
//! every value type of RFC 5545 section 3.3 decodes and all but `GEO` and `FLOAT` encode, the
//! typed accessors read through [`View`], and a write is either [`PropertyMut`] or one of
//! [`Component`]'s two described-change doors — [`Component::apply`], addressed to a property
//! identity and answering for every occurrence of it, and [`Component::apply_to_occurrence`],
//! addressed to one, which is the address a scheduling message needs. All three discard the
//! preserved text of the line they touch and of no other.
//!
//! Section 3.6 is read as well as stored. [`ComponentKind::cardinality`] states how often each
//! of the nine components may carry a name, [`Component::audit`] reports what section 3.6 says
//! about one component's own properties, and the seventeen accessors beside it reach the
//! properties that reading names. The audit is advisory and never a stage of parsing or
//! writing: a component this crate has no definition for allows everything, because "no
//! schema" and "the schema forbids it" are different answers and only the second is reported.
//!
//! Construction is a door rather than an opening. `Property::new`, `Parameter::new` and
//! `Boundary::new` are crate-private; the public [`Property::create`], [`Parameter::create`]
//! and [`Component::create`] refuse the octets section 3.1 cannot write back, so a `SUMMARY`
//! taken from a web form can no longer arrive as a second `ATTENDEE` through the tree.
//!
//! The round trip is asserted end to end rather than per unit: `crates/ical-core/tests` parses
//! and serializes folds in every position, all three terminators, a value that is not UTF-8, a
//! fold that splits a codepoint, an unterminated quoted parameter, and a file whose every
//! structural rule is broken, and compares octets. The reader also reports what it sees now
//! rather than only what it refuses: a physical line over 75 octets, a control character in a
//! name, a parameter name or a value, a parameter with no value, an unterminated quoted
//! parameter, and an RFC 6868 pair nothing defines.
//!
//! `RRULE` is the deliberate hole. [`PropertyId::RRULE`] exists, its value stays preserved
//! text, and [`ValueType::Recur`] names it; the section 3.3.10 grammar is `ical-recur`'s.

#![no_std]

extern crate alloc;

#[path = "../../icalkit/src/internal/core/access.rs"]
pub(crate) mod access;
#[path = "../../icalkit/src/internal/core/arith.rs"]
pub(crate) mod arith;
#[path = "../../icalkit/src/internal/core/change.rs"]
pub(crate) mod change;
#[path = "../../icalkit/src/internal/core/codec.rs"]
pub(crate) mod codec;
#[path = "../../icalkit/src/internal/core/emit.rs"]
pub(crate) mod emit;
#[path = "../../icalkit/src/internal/core/grammar/mod.rs"]
pub(crate) mod grammar;
#[path = "../../icalkit/src/internal/core/gregorian.rs"]
pub(crate) mod gregorian;
#[path = "../../icalkit/src/internal/core/ident.rs"]
pub(crate) mod ident;
#[path = "../../icalkit/src/internal/core/mutate.rs"]
pub(crate) mod mutate;
#[path = "../../icalkit/src/internal/core/octets.rs"]
pub(crate) mod octets;
#[path = "../../icalkit/src/internal/core/output.rs"]
pub(crate) mod output;
#[path = "../../icalkit/src/internal/core/parse.rs"]
pub(crate) mod parse;
#[path = "../../icalkit/src/internal/core/schema.rs"]
pub(crate) mod schema;
#[path = "../../icalkit/src/internal/core/tree.rs"]
pub(crate) mod tree;
#[path = "../../icalkit/src/internal/core/view.rs"]
pub(crate) mod view;

// Stable crate-shaped root for source shared with `icalkit::internal::core`.
pub(crate) mod internal {
    #[allow(unused_imports)]
    pub(crate) mod core {
        pub(crate) use crate::*;
        pub(crate) use crate::{
            access, arith, change, codec, emit, grammar, gregorian, ident, mutate, octets, output,
            parse, schema, tree, view,
        };
    }
}

// The grammar is a private module and every item of it is re-exported here unchanged, so that
// `ical_core::Token` is the only spelling there is: no caller writes `ical_core::grammar::`
// and no caller writes `ical_grammar::`, because there is no such crate. A glob rather than a
// list, because the layer is meant to be invisible from outside and a list is a second place
// to forget an item.
pub use crate::grammar::*;

pub use crate::change::{ParameterEdit, ProposedChange};
pub use crate::gregorian::{
    CivilDate, CivilDateTime, CivilTime, DateTimeValue, Duration, MonthAddOutcome, UtcOffset,
    Weekday,
};
pub use crate::ident::PropertyId;
pub use crate::octets::{RawText, TextError};
pub use crate::output::Writer;
// `schema` is re-exported wholesale rather than item by item, for the reason the grammar
// globs two of its own: the component readings arrive with the milestone that writes them, and
// a glob keeps this file from becoming a place two separate pieces of work both have to edit.
pub use crate::schema::*;
pub use crate::tree::{
    Boundary, Component, Document, Item, Parameter, ParametersNamed, PropertiesNamed, Property,
};
pub use crate::view::{
    BinaryValue, DecodeValue, EncodeValue, Geo, MutationError, Period, PropertyMut, TextValue,
    UriValue, ValueBuf, ValueType, View,
};
