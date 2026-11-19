// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The namespace binding stack: which URI a prefix names, at this point in this document.
//!
//! A prefix is a per-document choice its author made and nothing else.
//! `DAV:` may be bound to `D:`, to `d:`, to `ns0:`, to no prefix at all through a default declaration, or to a different prefix on every element of one body, and the servers people actually run disagree about which.
//! The stack stores the caller's classification of each accepted URI until its scope ends.
//!
//! # Scope is a suffix
//!
//! Bindings live in declaration order in one vector, and an element's scope ends by truncating the tail it declared.
//! That is what makes shadowing free — an inner `xmlns:D=` simply sits later in the vector than the outer one, and [`PrefixStack::uri_for`] searches from the end —
//! and it is why a binding count rather than a nested structure is what an open element carries.
//!
//! # It is a metered dimension of its own
//!
//! One element can carry a thousand declarations at depth one, so neither a depth bound nor an element count reaches them.
//! `Limits::max_prefix_bindings` is the bound and `Meter` is where it is charged, which is `docs/adr/0010`'s dimension that was predicted to be missing and was.

use alloc::vec::Vec;

use ical_core::{LimitExceeded, Meter};

use super::chars::AttributeValue;
use super::fault::{XmlFault, XmlSyntax};
use super::scan::{XML_PREFIX, XML_URI, XMLNS_URI};

/// A prefix bound to a URI for as long as the element that declared it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding<'a, T> {
    /// The prefix, empty for a default declaration.
    prefix: &'a [u8],
    uri: T,
}

/// Every namespace binding live at one point in one document, innermost last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrefixStack<'a, T> {
    /// The bindings, in declaration order, so a scope ends by truncating the tail.
    bindings: Vec<Binding<'a, T>>,
}

impl<'a, T: Copy + From<AttributeValue<'a>>> PrefixStack<'a, T> {
    pub(crate) fn declarations(
        &self,
        local: Option<u16>,
    ) -> impl DoubleEndedIterator<Item = (&'a [u8], T)> + '_ {
        let first = local.map_or(0, |count| {
            self.bindings.len().saturating_sub(usize::from(count))
        });
        self.bindings
            .get(first..)
            .unwrap_or(&[])
            .iter()
            .map(|binding| (binding.prefix, binding.uri))
    }

    /// An empty stack, which is what a document begins with.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Bind `prefix` to `uri` for as long as the element declaring it stays open.
    ///
    /// Charged before the vector grows, so a body that declares more bindings than the caller admits is refused at the bound rather than after the memory is resident.
    pub(crate) fn bind(
        &mut self,
        prefix: &'a [u8],
        uri: AttributeValue<'a>,
        meter: &mut Meter,
    ) -> Result<(), XmlFault> {
        meter.admit(|| Ok(()))?;
        let xml_namespace = uri.is(XML_URI);
        if prefix == b"xmlns"
            || uri.is(XMLNS_URI)
            || (prefix == XML_PREFIX) != xml_namespace
            || (!prefix.is_empty() && uri.len() == 0)
        {
            return Err(XmlSyntax::Malformed.into());
        }
        meter.try_bind_prefix()?;
        if let Err(error) = meter.admit(|| {
            self.bindings
                .try_reserve(1)
                .map_err(|_| LimitExceeded::Budget)
        }) {
            meter.unbind_prefix();
            return Err(error.into());
        }
        self.bindings.push(Binding {
            prefix,
            uri: T::from(uri),
        });
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
    /// Only its own declarations: shadowing an outer binding is what a prefix rebound mid-document does, and it is ordinary rather than an error.
    /// XML Namespaces 1.0 section 6.3 makes the *repeat on one element* the collision.
    #[must_use]
    pub(crate) fn declared_here(&self, declared: u16, prefix: &[u8]) -> bool {
        self.declarations(Some(declared))
            .any(|(held, _)| held == prefix)
    }

    /// The URI a prefix is bound to here, or `None` when nothing binds it.
    ///
    /// `xml` is bound to its reserved URI without a declaration, as XML Namespaces 1.0 section 3 requires, because RFC 4918 bodies carry `xml:lang` and a reader that demanded a declaration for it would refuse bodies the specification writes itself.
    #[must_use]
    pub(crate) fn uri_for(&self, prefix: &[u8]) -> Option<T> {
        if prefix == XML_PREFIX {
            return Some(T::from(AttributeValue::literal(XML_URI)));
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
    use ical_core::{LimitExceeded, Limits, Meter};

    use super::{AttributeValue, XmlFault};

    type PrefixStack<'a> = super::PrefixStack<'a, AttributeValue<'a>>;

    #[test]
    fn scope_lookup_reuses_the_value_classified_at_declaration() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone, Copy)]
        struct Classified(usize);
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        impl From<AttributeValue<'_>> for Classified {
            fn from(value: AttributeValue<'_>) -> Self {
                CALLS.fetch_add(1, Ordering::Relaxed);
                Self(value.len())
            }
        }

        let mut meter = Meter::new(Limits::DEFAULT);
        let mut stack = super::PrefixStack::<Classified>::new();
        stack
            .bind(
                b"d",
                AttributeValue::parse(b"D&#65;V:").unwrap(),
                &mut meter,
            )
            .unwrap();
        assert_eq!(stack.uri_for(b"d").unwrap().0, 4);
        assert_eq!(stack.declarations(None).next().unwrap().1.0, 4);
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exhaustion_precedes_binding_validation() {
        let limits = Limits::DEFAULT.with_max_prefix_bindings(0);
        let mut meter = Meter::new(limits);
        let mut stack = PrefixStack::new();
        assert_eq!(
            stack.bind(b"d", AttributeValue::literal(b"DAV:"), &mut meter),
            Err(XmlFault::Limit(LimitExceeded::PrefixBindings))
        );
        assert_eq!(
            stack.bind(b"xml", AttributeValue::literal(b"urn:wrong"), &mut meter),
            Err(XmlFault::Limit(LimitExceeded::Exhausted))
        );
    }

    #[test]
    fn an_inner_declaration_shadows_an_outer_one_and_the_outer_one_comes_back() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut stack = PrefixStack::new();
        stack
            .bind(b"D", AttributeValue::literal(b"DAV:"), &mut meter)
            .unwrap();
        assert_eq!(stack.uri_for(b"D"), Some(AttributeValue::literal(b"DAV:")));

        stack
            .bind(
                b"D",
                AttributeValue::literal(b"http://evil.example/not-dav"),
                &mut meter,
            )
            .unwrap();
        assert_eq!(
            stack.uri_for(b"D"),
            Some(AttributeValue::literal(b"http://evil.example/not-dav")),
            "the innermost binding is the one in force"
        );

        stack.unbind(1, &mut meter);
        assert_eq!(stack.uri_for(b"D"), Some(AttributeValue::literal(b"DAV:")));
    }

    #[test]
    fn the_xml_prefix_is_bound_without_a_declaration() {
        let stack = PrefixStack::new();
        assert_eq!(
            stack.uri_for(b"xml"),
            Some(AttributeValue::literal(
                b"http://www.w3.org/XML/1998/namespace"
            ))
        );
        assert_eq!(stack.uri_for(b"D"), None);
    }

    #[test]
    fn one_elements_own_repeat_is_seen_and_an_outer_shadow_is_not() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut stack = PrefixStack::new();
        stack
            .bind(b"D", AttributeValue::literal(b"DAV:"), &mut meter)
            .unwrap();
        assert!(
            !stack.declared_here(0, b"D"),
            "the outer one is not this one"
        );

        stack
            .bind(b"C", AttributeValue::literal(b"urn:x"), &mut meter)
            .unwrap();
        assert!(stack.declared_here(1, b"C"));
        assert!(!stack.declared_here(1, b"D"));
    }

    #[test]
    fn a_body_declaring_more_bindings_than_the_caller_admits_is_refused() {
        let limits = Limits::DEFAULT.with_max_prefix_bindings(1);
        let mut meter = Meter::new(limits);
        let mut stack = PrefixStack::new();
        stack
            .bind(b"D", AttributeValue::literal(b"DAV:"), &mut meter)
            .unwrap();
        assert!(
            stack
                .bind(b"C", AttributeValue::literal(b"urn:x"), &mut meter)
                .is_err()
        );
        assert_eq!(stack.uri_for(b"C"), None, "a refused charge binds nothing");
    }
}
