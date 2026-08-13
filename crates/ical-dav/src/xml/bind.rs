// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The namespace binding stack: which URI a prefix names, at this point in this document.
//!
//! A prefix is a per-document choice its author made and nothing else. `DAV:` may be bound to
//! `D:`, to `d:`, to `ns0:`, to no prefix at all through a default declaration, or to a different
//! prefix on every element of one body, and the servers people actually run disagree about
//! which. Resolving that is this file's whole job, and it does it without knowing a single URI
//! by name: what it answers with is the octets the document bound, and classifying those is the
//! vocabulary's business one layer up.
//!
//! # Scope is a suffix
//!
//! Bindings live in declaration order in one vector, and an element's scope ends by truncating
//! the tail it declared. That is what makes shadowing free — an inner `xmlns:D=` simply sits
//! later in the vector than the outer one, and [`PrefixStack::uri_for`] searches from the end —
//! and it is why a binding count rather than a nested structure is what an open element carries.
//!
//! # It is a metered dimension of its own
//!
//! One element can carry a thousand declarations at depth one, so neither a depth bound nor an
//! element count reaches them. `Limits::max_prefix_bindings` is the bound and `Meter` is where it
//! is charged, which is `docs/adr/0010`'s dimension that was predicted to be missing and was.

use alloc::vec::Vec;

use ical_core::{LimitExceeded, Meter};

use super::fault::XmlFault;
use super::scan::{XML_PREFIX, XML_URI};

/// A prefix bound to a URI for as long as the element that declared it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding<'a> {
    /// The prefix, empty for a default declaration.
    prefix: &'a [u8],
    /// The URI it is bound to, exactly as the document spelled it.
    uri: &'a [u8],
}

/// Every namespace binding live at one point in one document, innermost last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrefixStack<'a> {
    /// The bindings, in declaration order, so a scope ends by truncating the tail.
    bindings: Vec<Binding<'a>>,
}

impl<'a> PrefixStack<'a> {
    /// An empty stack, which is what a document begins with.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Bind `prefix` to `uri` for as long as the element declaring it stays open.
    ///
    /// Charged before the vector grows, so a body that declares more bindings than the caller
    /// admits is refused at the bound rather than after the memory is resident.
    pub(crate) fn bind(
        &mut self,
        prefix: &'a [u8],
        uri: &'a [u8],
        meter: &mut Meter,
    ) -> Result<(), XmlFault> {
        meter.try_bind_prefix()?;
        self.bindings
            .try_reserve(1)
            .map_err(|_| LimitExceeded::Budget)?;
        self.bindings.push(Binding { prefix, uri });
        Ok(())
    }

    /// Release the `count` bindings the element that is closing declared.
    pub(crate) fn unbind(&mut self, count: u16, meter: &mut Meter) {
        for _ in 0..count {
            if self.bindings.pop().is_some() {
                meter.unbind_prefix();
            }
        }
    }

    /// Whether the element currently being read has already declared `prefix` itself.
    ///
    /// Only its own declarations: shadowing an outer binding is what a prefix rebound
    /// mid-document does, and it is ordinary rather than an error. XML Namespaces 1.0 section
    /// 6.3 makes the *repeat on one element* the collision.
    #[must_use]
    pub(crate) fn declared_here(&self, declared: u16, prefix: &[u8]) -> bool {
        let held = usize::from(declared);
        let first = self.bindings.len().saturating_sub(held);
        self.bindings
            .get(first..)
            .unwrap_or(&[])
            .iter()
            .any(|binding| binding.prefix == prefix)
    }

    /// The URI a prefix is bound to here, or `None` when nothing binds it.
    ///
    /// `xml` is bound to its reserved URI without a declaration, as XML Namespaces 1.0 section 3
    /// requires, because RFC 4918 bodies carry `xml:lang` and a reader that demanded a
    /// declaration for it would refuse bodies the specification writes itself.
    #[must_use]
    pub(crate) fn uri_for(&self, prefix: &[u8]) -> Option<&'a [u8]> {
        if prefix == XML_PREFIX {
            return Some(XML_URI);
        }
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.prefix == prefix)
            .map(|binding| binding.uri)
    }
}

#[cfg(test)]
mod tests {
    use ical_core::{Limits, Meter};

    use super::PrefixStack;

    #[test]
    fn an_inner_declaration_shadows_an_outer_one_and_the_outer_one_comes_back() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut stack = PrefixStack::new();
        stack.bind(b"D", b"DAV:", &mut meter).unwrap();
        assert_eq!(stack.uri_for(b"D"), Some(b"DAV:".as_slice()));

        stack
            .bind(b"D", b"http://evil.example/not-dav", &mut meter)
            .unwrap();
        assert_eq!(
            stack.uri_for(b"D"),
            Some(b"http://evil.example/not-dav".as_slice()),
            "the innermost binding is the one in force"
        );

        stack.unbind(1, &mut meter);
        assert_eq!(stack.uri_for(b"D"), Some(b"DAV:".as_slice()));
    }

    #[test]
    fn the_xml_prefix_is_bound_without_a_declaration() {
        let stack = PrefixStack::new();
        assert_eq!(
            stack.uri_for(b"xml"),
            Some(b"http://www.w3.org/XML/1998/namespace".as_slice())
        );
        assert_eq!(stack.uri_for(b"D"), None);
    }

    #[test]
    fn one_elements_own_repeat_is_seen_and_an_outer_shadow_is_not() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut stack = PrefixStack::new();
        stack.bind(b"D", b"DAV:", &mut meter).unwrap();
        assert!(
            !stack.declared_here(0, b"D"),
            "the outer one is not this one"
        );

        stack.bind(b"C", b"urn:x", &mut meter).unwrap();
        assert!(stack.declared_here(1, b"C"));
        assert!(!stack.declared_here(1, b"D"));
    }

    #[test]
    fn a_body_declaring_more_bindings_than_the_caller_admits_is_refused() {
        let limits = Limits::DEFAULT.with_max_prefix_bindings(1);
        let mut meter = Meter::new(limits);
        let mut stack = PrefixStack::new();
        stack.bind(b"D", b"DAV:", &mut meter).unwrap();
        assert!(stack.bind(b"C", b"urn:x", &mut meter).is_err());
        assert_eq!(stack.uri_for(b"C"), None, "a refused charge binds nothing");
    }
}
