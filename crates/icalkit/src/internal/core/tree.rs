// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The document: one ordered heterogeneous tree, and nothing else.
//!
//! There is no known/unknown split and no keyed map behind any of these types. A content line
//! this crate cannot make sense of degrades to a [`Property`] rather than to a third kind of
//! node, which is what makes "never discards the file" mechanical instead of promised: a line
//! with no `:`, a `BEGIN` carrying parameters, an `END` with nothing open — each is stored as
//! an ordinary property, reported as a diagnostic, and written back octet for octet
//! (`docs/adr/0001`).
//!
//! [`Property`] has no typed-value field and never will. "Typed access is a view" is enforced
//! by the absence of a second place to keep the answer, not by contributors remembering not
//! to add one.
//!
//! Two fields carry the round-trip claim. [`Property::layout`] records the syntax of the line
//! the property arrived on, because unfolding into a fresh buffer destroys the only record of
//! where the producer folded, and a canonical refold would rewrite every file in the corpus
//! on its first save. [`Component::end`] is optional, because a component whose `END` never
//! arrived serializes without one, and an `END:vevent` that disagreed in case with its
//! `BEGIN:VEVENT` serializes back in the case it was written.

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{self, Debug, Formatter};
use core::hash::{Hash, Hasher};
use core::{mem, slice};

use crate::internal::core::{LineEnding, LineLayout};

use crate::internal::core::ident::PropertyId;
use crate::internal::core::octets::RawText;

/// A `BEGIN` or `END` line, kept in the spelling it arrived in.
///
/// The keyword is stored beside the component name because a producer that wrote `begin` gets
/// `begin` back: this crate compares names case-insensitively and rewrites nothing.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Boundary {
    /// The `BEGIN` or `END` keyword, as written.
    keyword: RawText,
    /// The component name, as written.
    name: RawText,
    /// The syntax of the line the boundary arrived on.
    layout: LineLayout,
}

impl Boundary {
    /// A boundary line with the given spelling and syntax.
    ///
    /// Crate-private, for the reason [`Property::new`] is: the octets are stored as they are,
    /// and a caller that could hand over `VEVENT\r\nATTENDEE:mailto:eve@example.test` as a
    /// component name would have written two content lines through a constructor. The public
    /// door is [`Component::create`](crate::internal::core::Component::create), which refuses what section 3.1
    /// cannot write back.
    #[must_use]
    pub(crate) fn new(keyword: RawText, name: RawText, layout: LineLayout) -> Self {
        Self {
            keyword,
            name,
            layout,
        }
    }

    /// The `BEGIN` or `END` keyword, as written.
    #[must_use]
    pub fn keyword(&self) -> &RawText {
        &self.keyword
    }

    /// The component name, as written.
    #[must_use]
    pub fn name(&self) -> &RawText {
        &self.name
    }

    /// The syntax of the line this boundary arrived on.
    #[must_use]
    pub fn layout(&self) -> &LineLayout {
        &self.layout
    }

    /// Give this line the terminator RFC 5545 section 3.1 requires, if it carried none.
    ///
    /// Crate-private for the same reason the property setters below are: the only caller is
    /// the insertion in `mutate`, which is where a line stops being the last one.
    pub(crate) fn terminate_line(&mut self, ending: LineEnding) -> bool {
        self.layout.terminate_with(ending)
    }
}

/// One property parameter, name and value, both as octets.
///
/// The value keeps any surrounding `DQUOTE`. RFC 5545 section 3.2 lets a producer quote a
/// value that did not need quoting, and stripping the quotes at parse time would write back a
/// line the producer did not send.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Parameter {
    /// The parameter name, as written.
    name: RawText,
    /// The parameter value, quotes included, as written.
    value: RawText,
    /// Whether an `=` and a value were present at all.
    has_value: bool,
}

impl Parameter {
    /// A parameter with a value, stored in the section 3.2 spelling it arrived in.
    ///
    /// Crate-private, for the reason [`Property::new`] is. The public door is
    /// [`Parameter::create`](crate::internal::core::Parameter::create), which chooses the spelling rather than
    /// trusting one.
    #[must_use]
    pub(crate) fn new(name: RawText, value: RawText) -> Self {
        Self {
            name,
            value,
            has_value: true,
        }
    }

    /// A parameter that arrived with no `=`, which RFC 5545 does not allow and producers do.
    ///
    /// Crate-private because it is a shape only a reader observes: a caller that wants a
    /// parameter writes one with a value, and this one exists to write back what was read.
    #[must_use]
    pub(crate) fn without_value(name: RawText) -> Self {
        Self {
            name,
            value: RawText::default(),
            has_value: false,
        }
    }

    /// The parameter name, as written.
    #[must_use]
    pub fn name(&self) -> &RawText {
        &self.name
    }

    /// The parameter value, quotes included, as written.
    #[must_use]
    pub fn value(&self) -> &RawText {
        &self.value
    }

    /// Whether an `=` and a value were present at all.
    #[must_use]
    pub fn has_value(&self) -> bool {
        self.has_value
    }

    /// Whether this parameter is named `name`, compared as RFC 5545 compares a name.
    #[must_use]
    pub fn is_named(&self, name: &[u8]) -> bool {
        self.name.eq_name(name)
    }
}

/// Every parameter of one property carrying a given name, in order.
///
/// An iterator rather than a single answer, because RFC 5545 puts no repeat limit on most
/// parameters and a singular lookup would silently keep the first and drop the rest.
#[derive(Clone, Debug)]
pub struct ParametersNamed<'a> {
    /// The parameters not yet examined.
    remaining: slice::Iter<'a, Parameter>,
    /// The name being looked for.
    wanted: &'a [u8],
}

impl<'a> Iterator for ParametersNamed<'a> {
    type Item = &'a Parameter;

    fn next(&mut self) -> Option<Self::Item> {
        self.remaining.find(|entry| entry.is_named(self.wanted))
    }
}

/// One content line: a name, its parameters in order, and its value as octets.
///
/// Also the resting place for every line this crate could not make sense of. A blank line is
/// a property with an empty name and no separator; a `BEGIN` carrying parameters is a
/// property, because a [`Boundary`] has nowhere to keep them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Property {
    /// The property name, as written.
    name: RawText,
    /// The parameters, in the order they were written.
    parameters: Vec<Parameter>,
    /// The value's octets, unfolded, unescaped by nothing, exactly as read.
    value_text: RawText,
    /// The syntax of the line this property arrived on.
    layout: LineLayout,
}

impl Property {
    /// A property with the given name, parameters, value and line syntax, stored unchecked.
    ///
    /// Crate-private, and that is the other half of what makes
    /// [`PropertyMut::set_raw`](crate::internal::core::PropertyMut::set_raw)'s refusal true rather than
    /// customary. The scoped-write door was closed by making the setters below crate-private;
    /// this is the tree-building door, and it was open for as long as this constructor was
    /// public. `Property::new(b"SUMMARY", [], b"a\r\nATTENDEE:mailto:eve@example.test", ..)`
    /// pushed through [`Component::items_mut`] serializes as two content lines, which is the
    /// same injection arriving through construction rather than through a write.
    ///
    /// It stays unchecked because the reader needs it: octets that came out of a file are kept
    /// whatever they hold, control characters included, and a constructor that refused them
    /// would be a parser that discarded the file. The public door is
    /// [`Property::create`](crate::internal::core::Property::create), which is for octets that were never read
    /// from anywhere and therefore have no producer's spelling to preserve (`docs/adr/0001`).
    #[must_use]
    pub(crate) fn new(
        name: RawText,
        parameters: Vec<Parameter>,
        value_text: RawText,
        layout: LineLayout,
    ) -> Self {
        Self {
            name,
            parameters,
            value_text,
            layout,
        }
    }

    /// The property name, as written.
    #[must_use]
    pub fn name(&self) -> &RawText {
        &self.name
    }

    /// The parameters, in the order they were written.
    #[must_use]
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    /// Every parameter named `name`, in order.
    #[must_use]
    pub fn parameters_named<'a>(&'a self, name: &'a [u8]) -> ParametersNamed<'a> {
        ParametersNamed {
            remaining: self.parameters.iter(),
            wanted: name,
        }
    }

    /// The value's octets, exactly as read.
    #[must_use]
    pub fn value_text(&self) -> &RawText {
        &self.value_text
    }

    /// The syntax of the line this property arrived on.
    #[must_use]
    pub fn layout(&self) -> &LineLayout {
        &self.layout
    }

    /// Whether this property is named `name`, compared as RFC 5545 compares a name.
    #[must_use]
    pub fn is_named(&self, name: &[u8]) -> bool {
        self.name.eq_name(name)
    }

    /// Whether this property has the identity `id`.
    #[must_use]
    pub fn has_id(&self, id: &PropertyId) -> bool {
        id.matches(self.name.as_bytes())
    }

    /// Replace the value's octets, discarding this line's recorded fold layout.
    ///
    /// The layout goes because the folds were positions into text that no longer exists.
    /// Nothing outside this property is touched, which is what lets every other line in the
    /// component still serialize octet for octet.
    ///
    /// Crate-private, and that is the whole of what makes
    /// [`PropertyMut::set_raw`](crate::internal::core::PropertyMut::set_raw)'s refusal true rather than
    /// customary. The refusal is documented as "the one place this crate rejects caller input
    /// outright", which is a claim about the *only* door and not about one of several: a
    /// setter beside it that checks nothing lets a `SUMMARY` taken from a web form carry its
    /// own `CRLF` into the file, and the octets come back on the next read as a second
    /// property nobody added. A check repeated here would close that one door and leave
    /// [`Property::edit_parameters`] open, since a handed-out `&mut Vec` cannot be checked at
    /// all once it is handed out. Privacy closes all three at once.
    pub(crate) fn set_value_text(&mut self, value_text: RawText) {
        self.value_text = value_text;
        self.layout.mark_refolded();
    }

    /// Replace the property name, discarding this line's recorded fold layout.
    ///
    /// Crate-private, for the reason [`Property::set_value_text`] gives: a name carrying a `:`
    /// or a `;` splits one line into two exactly as a value carrying a terminator does.
    pub(crate) fn set_name(&mut self, name: RawText) {
        self.name = name;
        self.layout.mark_refolded();
    }

    /// Take the parameters for editing, discarding this line's recorded fold layout.
    ///
    /// The layout goes for the whole line rather than for the parameters alone, because the
    /// parameters are part of the line the folds were positions into.
    ///
    /// Crate-private, for the reason [`Property::set_value_text`] gives, and more sharply: a
    /// `&mut Vec` handed to a caller is a door no check can stand in front of, because the
    /// caller writes through it after the check would have run. The public way to change
    /// parameters is [`Component::apply`](crate::internal::core::Component::apply) with
    /// [`ProposedChange::SetParameters`](crate::internal::core::ProposedChange::SetParameters), which refuses
    /// what section 3.2 has no way to write.
    pub(crate) fn edit_parameters(&mut self) -> &mut Vec<Parameter> {
        self.layout.mark_refolded();
        &mut self.parameters
    }

    /// Give this line the terminator RFC 5545 section 3.1 requires, if it carried none.
    pub(crate) fn terminate_line(&mut self, ending: LineEnding) -> bool {
        self.layout.terminate_with(ending)
    }
}

/// One entry in a component: a property, or a nested component.
///
/// Concrete, non-generic, never `dyn`, and closed at two variants. A third variant for
/// "something we did not understand" is exactly what this crate refuses to have, because
/// anything reachable only through it is something a later pass can forget to write back.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    /// A content line.
    Property(Property),
    /// A `BEGIN`/`END` pair and everything between them.
    Component(Component),
}

impl Item {
    /// The property, if this entry is one.
    #[must_use]
    pub const fn as_property(&self) -> Option<&Property> {
        match self {
            Self::Property(property) => Some(property),
            Self::Component(_) => None,
        }
    }

    /// The component, if this entry is one.
    #[must_use]
    pub const fn as_component(&self) -> Option<&Component> {
        match self {
            Self::Component(component) => Some(component),
            Self::Property(_) => None,
        }
    }

    /// The property, mutably, if this entry is one.
    pub const fn as_property_mut(&mut self) -> Option<&mut Property> {
        match self {
            Self::Property(property) => Some(property),
            Self::Component(_) => None,
        }
    }

    /// The component, mutably, if this entry is one.
    pub const fn as_component_mut(&mut self) -> Option<&mut Component> {
        match self {
            Self::Component(component) => Some(component),
            Self::Property(_) => None,
        }
    }
}

/// Every property of one component carrying a given identity, in order.
///
/// Nested components are skipped: a `DTSTART` inside a `VALARM` belongs to the alarm, and a
/// lookup that walked into it would answer questions about a component the caller did not
/// ask about.
#[derive(Clone, Debug)]
pub struct PropertiesNamed<'a> {
    /// The entries not yet examined.
    remaining: slice::Iter<'a, Item>,
    /// The identity being looked for.
    wanted: &'a PropertyId,
}

impl<'a> Iterator for PropertiesNamed<'a> {
    type Item = &'a Property;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.remaining.next()?;
            if let Some(property) = entry.as_property() {
                if property.has_id(self.wanted) {
                    return Some(property);
                }
            }
        }
    }
}

/// A `BEGIN`/`END` pair and the ordered entries between them.
///
/// One heterogeneous sequence, not a properties list beside a components list: the interleaved
/// order is what a producer wrote and what serialization has to reproduce.
///
/// Nothing about this type is derived, and the reason is the same one [`Component::drop`]
/// gives. Every derived traversal of a tree whose entries hold more of the tree recurses, one
/// stack frame per level, and the depth is `Limits::max_component_depth` — a `u16` a caller
/// raises through a public builder — so sixteen thousand `BEGIN` lines parse cleanly and take
/// the process down the first time anything clones, compares, orders, hashes or prints them.
/// A stack overflow is an abort and not an unwind: no `catch_unwind` sees it, and a server that
/// parsed an untrusted attachment loses the process rather than the request. Each of the six is
/// therefore written over an explicit stack, and [`Item`] and [`Document`] may keep their
/// derives because what those delegate to no longer recurses.
pub struct Component {
    /// The `BEGIN` line.
    begin: Boundary,
    /// The entries, in order.
    items: Vec<Item>,
    /// The `END` line, absent when it never arrived.
    end: Option<Boundary>,
}

impl Component {
    /// A component with the given boundaries and entries.
    #[must_use]
    pub fn new(begin: Boundary, items: Vec<Item>, end: Option<Boundary>) -> Self {
        Self { begin, items, end }
    }

    /// The `BEGIN` line.
    #[must_use]
    pub fn begin(&self) -> &Boundary {
        &self.begin
    }

    /// The `BEGIN` line, for the one edit that reaches it.
    ///
    /// Crate-private and deliberately not a general handle on the boundary: the only caller is
    /// the insertion in `mutate`, which has to terminate whatever line will sit above the
    /// property it adds — and for a component with no properties yet, that line is this one.
    pub(crate) fn begin_mut(&mut self) -> &mut Boundary {
        &mut self.begin
    }

    /// The `END` line, absent when it never arrived.
    #[must_use]
    pub fn end(&self) -> Option<&Boundary> {
        self.end.as_ref()
    }

    /// Install the `END` line, once one arrives.
    pub fn set_end(&mut self, end: Option<Boundary>) {
        self.end = end;
    }

    /// The component name, as the `BEGIN` line wrote it.
    #[must_use]
    pub fn name(&self) -> &RawText {
        self.begin.name()
    }

    /// Whether this component is named `name`, compared as RFC 5545 compares a name.
    #[must_use]
    pub fn is_named(&self, name: &[u8]) -> bool {
        self.begin.name().eq_name(name)
    }

    /// The entries, in order.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The entries, in order, for editing.
    ///
    /// Reordering or removing entries through this is the caller's decision and the caller's
    /// consequence; nothing in this crate does it on the caller's behalf.
    pub fn items_mut(&mut self) -> &mut Vec<Item> {
        &mut self.items
    }

    /// The properties directly inside this component, in order.
    pub fn properties(&self) -> impl Iterator<Item = &Property> {
        self.items.iter().filter_map(Item::as_property)
    }

    /// Every property directly inside this component with the identity `id`, in order.
    ///
    /// The route every property name is reachable through, whether or not the specification
    /// limits how often it may appear. The singular accessors are a convenience layered over
    /// this, never the only way to a value.
    #[must_use]
    pub fn properties_named<'a>(&'a self, id: &'a PropertyId) -> PropertiesNamed<'a> {
        PropertiesNamed {
            remaining: self.items.iter(),
            wanted: id,
        }
    }

    /// The components directly inside this component, in order.
    pub fn components(&self) -> impl Iterator<Item = &Self> {
        self.items.iter().filter_map(Item::as_component)
    }

    /// The components directly inside this component, in order, mutably.
    pub fn components_mut(&mut self) -> impl Iterator<Item = &mut Self> {
        self.items.iter_mut().filter_map(Item::as_component_mut)
    }
}

/// One step of a walk over a component and everything nested inside it.
///
/// Four of the six hand-written traits are one line each over this, because each of them asks
/// the same question about the same sequence: two components are equal when they walk alike,
/// ordered by the first step that differs, hashed as the steps they produce, and shown as the
/// lines they write. A sequence of steps determines the tree it came from — the `Begin` and
/// `End` steps carry the nesting — so answering over it is answering over the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Step<'a> {
    /// A component's `BEGIN` line. Every step until the matching `End` is inside it.
    Begin(&'a Boundary),
    /// One content line.
    Line(&'a Property),
    /// A component's `END` line, `None` when it never arrived.
    End(Option<&'a Boundary>),
}

/// A component whose `BEGIN` has been walked past and whose entries are still being walked.
#[derive(Debug)]
struct OpenFrame<'a> {
    /// The entries not yet reached.
    rest: slice::Iter<'a, Item>,
    /// The `END` line owed once they run out.
    end: Option<&'a Boundary>,
}

/// The steps of one component, innermost nesting held on an explicit stack.
#[derive(Debug)]
struct Walk<'a> {
    /// The components entered and not yet left, outermost first.
    open: Vec<OpenFrame<'a>>,
    /// The `BEGIN` line owed before anything else.
    opening: Option<&'a Boundary>,
}

impl<'a> Walk<'a> {
    /// The steps of `component`, its own two boundaries included.
    fn over(component: &'a Component) -> Self {
        let open = alloc::vec![OpenFrame {
            rest: component.items.iter(),
            end: component.end.as_ref(),
        }];
        Self {
            open,
            opening: Some(&component.begin),
        }
    }
}

impl<'a> Iterator for Walk<'a> {
    type Item = Step<'a>;

    fn next(&mut self) -> Option<Step<'a>> {
        if let Some(begin) = self.opening.take() {
            return Some(Step::Begin(begin));
        }
        // Bound before the match so that the borrow of the stack ends before an arm pushes to
        // it. The entry itself borrows the tree rather than this walk, so it outlives the
        // statement.
        let next = self.open.last_mut()?.rest.next();
        match next {
            Some(Item::Property(property)) => Some(Step::Line(property)),
            Some(Item::Component(nested)) => {
                self.open.push(OpenFrame {
                    rest: nested.items.iter(),
                    end: nested.end.as_ref(),
                });
                Some(Step::Begin(&nested.begin))
            },
            None => self.open.pop().map(|frame| Step::End(frame.end)),
        }
    }
}

/// One component under construction while the component it copies is being walked.
#[derive(Debug)]
struct CloneFrame<'a> {
    /// The entries of the source not yet copied.
    rest: slice::Iter<'a, Item>,
    /// The `BEGIN` line, already copied.
    begin: Boundary,
    /// The `END` line, already copied.
    end: Option<Boundary>,
    /// The entries copied so far.
    items: Vec<Item>,
}

impl<'a> CloneFrame<'a> {
    /// A frame that will copy `source`, with its boundaries taken and no entry yet.
    fn of(source: &'a Component) -> Self {
        Self {
            rest: source.items.iter(),
            begin: source.begin.clone(),
            end: source.end.clone(),
            items: Vec::with_capacity(source.items.len()),
        }
    }

    /// The copy, once every entry of the source has been reached.
    fn finish(self) -> Component {
        Component {
            begin: self.begin,
            items: self.items,
            end: self.end,
        }
    }
}

/// Copy a component and everything nested inside it, one flat worklist deep.
///
/// A nested component is copied into a frame of its own rather than through [`Item`]'s derived
/// clone, which is what keeps the two from recursing into each other.
fn clone_component(source: &Component) -> Component {
    let mut open: Vec<CloneFrame<'_>> = Vec::new();
    let mut current = CloneFrame::of(source);
    loop {
        let next = current.rest.next();
        match next {
            Some(Item::Property(property)) => current.items.push(Item::Property(property.clone())),
            Some(Item::Component(nested)) => {
                open.push(current);
                current = CloneFrame::of(nested);
            },
            None => {
                let finished = current.finish();
                let Some(parent) = open.pop() else {
                    return finished;
                };
                current = parent;
                current.items.push(Item::Component(finished));
            },
        }
    }
}

impl Clone for Component {
    fn clone(&self) -> Self {
        clone_component(self)
    }
}

impl PartialEq for Component {
    fn eq(&self, other: &Self) -> bool {
        Walk::over(self).eq(Walk::over(other))
    }
}

impl Eq for Component {}

impl Ord for Component {
    /// Ordered by the first step the two walks disagree on, and by which walk ran out first.
    ///
    /// A total order consistent with equality, which is all `Ord` promises and all a `BTreeMap`
    /// key needs. It is not the order the derive produced — that one compared the `BEGIN` line,
    /// then the entries, then the `END` line, and a component's `END` therefore sorted before
    /// everything nested inside it. Nothing observes either order but the comparison itself.
    fn cmp(&self, other: &Self) -> Ordering {
        Walk::over(self).cmp(Walk::over(other))
    }
}

impl PartialOrd for Component {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for Component {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for step in Walk::over(self) {
            step.hash(state);
        }
    }
}

impl Debug for Component {
    /// Shown as the flat sequence of lines it writes rather than as a nested structure.
    ///
    /// A nested rendering is the one thing this type cannot afford: `{:#?}` on a tree deeper
    /// than the stack aborts the process, and a `Debug` that is only safe for shallow values is
    /// a footgun in exactly the case somebody reaches for it, which is a file that went wrong.
    /// Every octet is still there, in order, with the nesting readable off the `Begin` and
    /// `End` steps.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(Walk::over(self)).finish()
    }
}

impl Drop for Component {
    /// Dismantle the nesting iteratively rather than letting the derived drop recurse.
    ///
    /// A derived `Drop` walks a component tree by recursion, one stack frame per level, and
    /// the depth of that tree is `Limits::max_component_depth` — a `u16` a caller sets through
    /// a public builder. Sixteen thousand `BEGIN` lines therefore parse cleanly under a raised
    /// policy and take the process down when the tree leaves scope. A stack overflow is an
    /// abort and not an unwind: no `catch_unwind` sees it, and a server that parsed an
    /// untrusted attachment loses the process rather than the request.
    ///
    /// So the entries are moved into one flat worklist and dropped from there. A nested
    /// component's own entries are taken out of it *before* it is dropped, so the drop that
    /// runs on it finds nothing left to walk and the recursion is two frames deep whatever the
    /// nesting was. The worklist is the vector the entries already lived in, taken rather than
    /// allocated, so an ordinary component costs this nothing.
    fn drop(&mut self) {
        let mut pending = mem::take(&mut self.items);
        while let Some(entry) = pending.pop() {
            if let Item::Component(mut nested) = entry {
                pending.append(&mut nested.items);
            }
        }
    }
}

/// The whole of one `.ics` stream: an ordered sequence of entries.
///
/// A document is a sequence rather than a single `VCALENDAR` because a stream may carry more
/// than one, and because a file that begins with junk before its first `BEGIN` still has to
/// come back out the way it went in.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Document {
    /// The entries, in order.
    items: Vec<Item>,
}

impl Document {
    /// A document over the given entries.
    #[must_use]
    pub fn new(items: Vec<Item>) -> Self {
        Self { items }
    }

    /// The entries, in order.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The entries, in order, for editing.
    pub fn items_mut(&mut self) -> &mut Vec<Item> {
        &mut self.items
    }

    /// The top-level components, in order. Usually the `VCALENDAR`s.
    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.items.iter().filter_map(Item::as_component)
    }

    /// The top-level components, in order, mutably.
    pub fn components_mut(&mut self) -> impl Iterator<Item = &mut Component> {
        self.items.iter_mut().filter_map(Item::as_component_mut)
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cmp::Ordering;
    use core::hash::{Hash, Hasher};

    use crate::internal::core::{FoldPoint, LineEnding, LineLayout};

    use super::{Boundary, Component, Document, Item, Parameter, Property};
    use crate::internal::core::ident::PropertyId;
    use crate::internal::core::octets::RawText;

    /// FNV-1a, because `core` ships no hasher and these tests need one that answers
    /// differently for octets that differ rather than only for octets that are longer.
    #[derive(Debug)]
    struct Fnv {
        /// The accumulated state.
        state: u64,
    }

    impl Default for Fnv {
        fn default() -> Self {
            Self {
                state: 0xcbf2_9ce4_8422_2325,
            }
        }
    }

    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.state
        }

        fn write(&mut self, bytes: &[u8]) {
            for &octet in bytes {
                self.state ^= u64::from(octet);
                self.state = self.state.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }

    /// What `value` hashes to under [`Fnv`].
    fn hashed<T: Hash>(value: &T) -> u64 {
        let mut hasher = Fnv::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    /// A property with no parameters, on a line this crate would have authored.
    fn line(name: &[u8], value: &[u8]) -> Property {
        Property::new(
            RawText::from_bytes(name),
            Vec::new(),
            RawText::from_bytes(value),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    /// A component named `X` wrapping `items`, closed.
    fn wrapper(items: Vec<Item>) -> Component {
        Component::new(
            boundary(b"BEGIN", b"X"),
            items,
            Some(boundary(b"END", b"X")),
        )
    }

    /// A tree `depth` components deep with one property at the bottom.
    ///
    /// Built from the inside out, because a helper that nested by recursion would abort in the
    /// test rather than in the code under test.
    fn nested(depth: usize) -> Component {
        let mut innermost = wrapper(vec![Item::Property(folded_summary())]);
        for _ in 1..depth {
            innermost = wrapper(vec![Item::Component(innermost)]);
        }
        innermost
    }

    /// `SUMMARY;X-A=1;X-A=2:hello`, folded once, as a producer might have written it.
    fn folded_summary() -> Property {
        let layout = LineLayout::preserved(
            vec![FoldPoint {
                offset: 20,
                tab: false,
                newline: LineEnding::CrLf,
            }],
            Some(LineEnding::CrLf),
            true,
        );
        Property::new(
            RawText::from_bytes(b"SUMMARY"),
            vec![
                Parameter::new(RawText::from_bytes(b"X-A"), RawText::from_bytes(b"1")),
                Parameter::new(RawText::from_bytes(b"X-A"), RawText::from_bytes(b"2")),
            ],
            RawText::from_bytes(b"hello"),
            layout,
        )
    }

    fn boundary(keyword: &[u8], name: &[u8]) -> Boundary {
        Boundary::new(
            RawText::from_bytes(keyword),
            RawText::from_bytes(name),
            LineLayout::canonical(LineEnding::CANONICAL),
        )
    }

    #[test]
    fn a_repeated_parameter_is_reachable_more_than_once() {
        let property = folded_summary();
        let found: Vec<&[u8]> = property
            .parameters_named(b"x-a")
            .map(|entry| entry.value().as_bytes())
            .collect();
        assert_eq!(found, vec![&b"1"[..], &b"2"[..]]);
    }

    #[test]
    fn a_write_discards_this_lines_layout_and_no_other_state() {
        let mut property = folded_summary();
        assert_eq!(property.layout().folds().len(), 1);

        property.set_value_text(RawText::from_bytes(b"goodbye"));
        assert!(property.layout().is_refolded());
        assert!(property.layout().folds().is_empty());
        assert_eq!(
            property.layout().ending(),
            Some(LineEnding::CrLf),
            "the terminator the producer used is not a fold and does not go"
        );
        assert_eq!(
            property.parameters().len(),
            2,
            "the parameters are untouched"
        );
        assert_eq!(property.name().as_bytes(), b"SUMMARY");
    }

    #[test]
    fn a_component_whose_end_never_arrived_is_still_a_component() {
        let unclosed = Component::new(
            boundary(b"BEGIN", b"VEVENT"),
            vec![Item::Property(folded_summary())],
            None,
        );
        assert!(unclosed.end().is_none());
        assert!(
            unclosed.is_named(b"vevent"),
            "names compare case-insensitively"
        );
        assert_eq!(unclosed.items().len(), 1);
    }

    #[test]
    fn an_end_that_disagreed_in_case_keeps_the_case_it_was_written_in() {
        let mismatched = Component::new(
            boundary(b"BEGIN", b"VEVENT"),
            Vec::new(),
            Some(boundary(b"end", b"vevent")),
        );
        assert_eq!(
            mismatched.end().map(|line| line.keyword().as_bytes()),
            Some(&b"end"[..])
        );
        assert_eq!(mismatched.name().as_bytes(), b"VEVENT");
    }

    #[test]
    fn a_lookup_stops_at_the_components_own_entries() {
        let alarm = Component::new(
            boundary(b"BEGIN", b"VALARM"),
            vec![Item::Property(folded_summary())],
            Some(boundary(b"END", b"VALARM")),
        );
        let event = Component::new(
            boundary(b"BEGIN", b"VEVENT"),
            vec![Item::Component(alarm)],
            Some(boundary(b"END", b"VEVENT")),
        );
        assert_eq!(
            event.properties_named(&PropertyId::SUMMARY).count(),
            0,
            "the alarm's SUMMARY is the alarm's"
        );
        assert_eq!(event.components().count(), 1);
    }

    /// A component is shown as the lines it writes, flat, with the nesting readable off the
    /// boundary steps. Pinned on a small tree, because the reason for the shape is a large one.
    #[test]
    fn a_component_is_shown_as_the_sequence_of_lines_it_writes() {
        let shown = format!("{:?}", wrapper(vec![Item::Property(line(b"UID", b"1"))]));
        assert!(shown.starts_with("[Begin("), "{shown}");
        assert!(shown.ends_with(']'), "{shown}");
        assert!(shown.contains("Line(Property"), "{shown}");
        assert!(shown.contains("End(Some("), "{shown}");

        let unclosed = Component::new(boundary(b"BEGIN", b"X"), Vec::new(), None);
        assert!(format!("{unclosed:?}").contains("End(None)"));
    }

    /// Equality, ordering and hashing are answers about the whole tree and not about its top
    /// line, which is what a walk that stopped at the boundaries would have got wrong.
    #[test]
    fn two_components_differing_only_deep_inside_are_told_apart() {
        /// A `UID` of the given value, two components down.
        fn buried(value: &[u8]) -> Component {
            let inner = wrapper(vec![Item::Property(line(b"UID", value))]);
            wrapper(vec![Item::Component(inner)])
        }

        let one = buried(b"1@example.test");
        let again = buried(b"1@example.test");
        let other = buried(b"2@example.test");

        assert_eq!(one, again);
        assert_eq!(one.cmp(&again), Ordering::Equal);
        assert_eq!(hashed(&one), hashed(&again));

        assert_ne!(one, other);
        assert_eq!(one.cmp(&other), Ordering::Less, "1 sorts before 2");
        assert_ne!(hashed(&one), hashed(&other));

        assert!(
            one < other,
            "the order is antisymmetric, not merely unequal"
        );
        assert!(other > one);

        // A component with nothing in it is neither, and the comparison still answers rather
        // than stopping at the two boundary lines they share.
        let empty = wrapper(Vec::new());
        assert_ne!(empty, one);
        assert_ne!(empty.cmp(&one), Ordering::Equal);
    }

    /// The traversals that recursed until this file stopped deriving them.
    ///
    /// `Limits::max_component_depth` is a `u16` a caller raises through a public builder, so a
    /// tree this deep is one the reader will build when it is asked to. Every derived traversal
    /// of it overflows the stack, which is an abort rather than a panic: no `catch_unwind` sees
    /// it, no sibling test in the same binary survives it, and this test asserts the whole of
    /// its claim by returning at all.
    #[test]
    fn every_traversal_of_a_tree_deeper_than_the_stack_returns() {
        const DEPTH: usize = 20_000;

        let deep = nested(DEPTH);
        let copy = deep.clone();
        assert_eq!(copy, deep);
        assert_eq!(copy.cmp(&deep), Ordering::Equal);
        assert_eq!(hashed(&copy), hashed(&deep));
        assert!(!format!("{deep:?}").is_empty());

        // Through the derives that delegate to those, which is how a caller reaches them.
        let document = Document::new(vec![Item::Component(deep)]);
        assert_eq!(document.clone(), document);
        assert_eq!(document.items().len(), 1);
        drop(document);
    }

    #[test]
    fn a_document_is_a_sequence_and_not_a_single_calendar() {
        let mut document = Document::new(vec![
            Item::Property(folded_summary()),
            Item::Component(Component::new(
                boundary(b"BEGIN", b"VCALENDAR"),
                Vec::new(),
                Some(boundary(b"END", b"VCALENDAR")),
            )),
        ]);
        assert_eq!(document.items().len(), 2);
        assert_eq!(document.components().count(), 1);
        assert_eq!(document.components_mut().count(), 1);
        assert!(Document::default().items().is_empty());
    }
}
