// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The WebDAV XML layer, compiled with no CalDAV vocabulary in scope. A path from it into the
//! protocol above it resolves in `ical-dav` and does not resolve here, which is what makes the
//! layer a fact rather than a promise (ADR 0004, ADR 0012).

// The layer names `alloc` and `alloc` is not in a std crate's extern prelude, so the declaration
// is needed here even though nothing in this file uses it directly. Deliberately not
// `#![no_std]`: that attribute is what `xtask purity` reads to decide a directory holds a core
// crate, and this member is not one.
extern crate alloc;

use alloc::vec::Vec;

use ical_core::{LimitExceeded, Limits, Meter};

#[path = "../../../crates/ical-dav/src/xml/mod.rs"]
mod xml;

/// Name every item the layer offers, so that this member compiles the whole layer.
///
/// Two things make this necessary rather than decorative, and both are about what a gate is
/// worth when nobody watches it fail.
///
/// The layer's items are `pub(crate)` — that is the point of `docs/adr/0012`, which keeps the
/// grammar unexported so the deferred extraction stays a file move — so there is nothing here to
/// re-export the way `gates/grammar-layering` re-exports `ical-core`'s grammar, and every item
/// the consumers in `ical-dav` use is dead code in *this* root. Silencing that with an attribute
/// is refused by this workspace and would be the wrong fix anyway: a gate that compiles with
/// half the layer deleted proves nothing about the half that is gone. Naming each item here
/// makes deleting one a compile error in the gate rather than a lint nobody reads.
///
/// It is also a smoke test of the only thing this member can assert: that the layer *builds* in
/// a root that has never heard of CalDAV. Behavior is asserted by the layer's own tests, which
/// run inside `ical-dav` — `[lib] test = false` on this member is what stops them running twice
/// and being counted twice.
#[must_use]
pub fn roll_call() -> usize {
    let mut named = 0_usize;
    for marker in [
        xml::scan::BYTE_ORDER_MARK,
        xml::scan::CDATA_CLOSE,
        xml::scan::CDATA_OPEN,
        xml::scan::COMMENT_CLOSE,
        xml::scan::COMMENT_OPEN,
        xml::scan::DECLARATION_OPEN,
        xml::scan::NO_NAMESPACE,
        xml::scan::XML_PREFIX,
        xml::scan::XML_URI,
        xml::scan::XMLNS,
        xml::scan::XMLNS_COLON,
    ] {
        named = named.saturating_add(marker.len());
    }
    for answered in [
        xml::scan::is_space(b' '),
        xml::scan::is_name_end(b'>'),
        xml::scan::is_attribute_name_end(b'='),
        xml::scan::is_name_forbidden(b'<'),
        xml::chars::escape_for(b'&', false).is_some(),
        xml::scan::declared_prefix(xml::scan::XMLNS).is_some(),
        xml::scan::find(b"ab", b"b").is_some(),
        xml::scan::split_name(b"D:href").is_ok(),
        xml::scan::check_encoding(b" encoding=\"utf-8\"").is_ok(),
        xml::chars::check_chars(b"ok").is_ok(),
    ] {
        named = named.saturating_add(usize::from(answered));
    }
    named.saturating_add(buffered()).saturating_add(bound())
}

/// The two doors that write into a buffer, and the fault type they refuse through.
fn buffered() -> usize {
    let mut out: Vec<u8> = Vec::new();
    let resolved: usize = xml::chars::push_reference(b"&amp;", 0, &mut out).unwrap_or_default();
    if xml::chars::normalize_attribute(b"a\tb", &mut out).is_err()
        || xml::chars::push(&mut out, b"c").is_err()
    {
        return resolved;
    }
    resolved
        .saturating_add(out.len())
        .saturating_add(refusals())
}

/// The layer's own refusal vocabulary, named so that deleting a class is a compile error here.
///
/// Four syntax classes and not the ten `ical-dav`'s public `SyntaxError` carries, because these
/// are the four this layer raises; the six the tokenizer's state machine raises stay above it.
/// Naming them from a root that has never heard of CalDAV is the assertion that the layer
/// classifies for itself rather than borrowing a protocol's vocabulary to do it.
fn refusals() -> usize {
    [
        xml::fault::XmlFault::Limit(LimitExceeded::Text),
        xml::fault::XmlFault::Syntax(xml::fault::XmlSyntax::UndefinedEntity),
        xml::fault::XmlFault::Syntax(xml::fault::XmlSyntax::Encoding),
        xml::fault::XmlFault::Syntax(xml::fault::XmlSyntax::Malformed),
        xml::fault::XmlFault::Syntax(xml::fault::XmlSyntax::ForbiddenCharacter),
    ]
    .iter()
    .filter(|fault| matches!(**fault, xml::fault::XmlFault::Syntax(_)))
    .count()
}

/// The namespace binding stack, which is the layer's one piece of state.
fn bound() -> usize {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut stack = xml::bind::PrefixStack::new();
    if stack.bind(b"D", b"DAV:", &mut meter).is_err() {
        return 0;
    }
    let held = usize::from(stack.uri_for(b"D").is_some())
        .saturating_add(usize::from(stack.declared_here(1, b"D")))
        .saturating_add(usize::from(stack.uri_for(b"C").is_none()));
    stack.unbind(1, &mut meter);
    held
}
