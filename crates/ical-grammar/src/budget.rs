// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One policy, one running ledger, threaded through every hostile-input entry point.
//!
//! A limit is two things. [`Limits`] is the caller's immutable policy — the thresholds,
//! cheap to copy, identical for every call. [`Meter`] is the caller's mutable ledger, a
//! running count of work already done under that policy. Both travel together because a
//! threshold checked per call is bounded per call and unbounded in aggregate: five thousand
//! individually bounded searches driven by a fan-out loop above this workspace add up to
//! whatever the attacker chose the fan-out to be (`docs/adr/0010`).
//!
//! The ledger is passed as `&mut` precisely so its lifetime is the caller's choice and not
//! the call's. [`Meter`] is neither `Copy` nor `Default` nor `Clone` for the same reason:
//! minting a fresh one inside a loop is how a budget silently stops binding, and none of
//! those three traits leaves a mark at the call site. Nothing here can force reuse — a
//! caller who writes `Meter::new(..)` inside its own loop reproduces the attack exactly —
//! so this makes the mistake visible rather than impossible.
//!
//! These types live at the bottom of the stack rather than in `ical-core` because the crates
//! that name them do not all depend on each other, and because the running count of refused
//! diagnostics has to live outside the sink, which is defined here.

use crate::failure::{LimitExceeded, ParseError};

/// The bounds that apply to the content-line grammar alone.
///
/// Separate from the rest of [`Limits`] because a caller of the token layer has no component
/// tree, no recurrence and no XML, and should not have to state bounds for them to read a
/// line.
///
/// Every field is a ceiling, which is what the type is for, so none of them says so again in
/// its own name. The accessors do, because `max_header_bytes()` is read at a call site where
/// the type has already been forgotten.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrammarLimits {
    /// The most octets one content line's name and parameters may occupy together.
    header_bytes: u32,
    /// The most parameters one content line may carry.
    parameters: u32,
    /// The most continuation lines one content line may be folded across.
    folds_per_line: u32,
}

impl GrammarLimits {
    /// Bounds a phone rendering one month can afford.
    pub const DEFAULT: Self = Self {
        header_bytes: 4096,
        parameters: 64,
        folds_per_line: 16_384,
    };

    /// Bounds a server indexing a decade can afford.
    pub const GENEROUS: Self = Self {
        header_bytes: 65_536,
        parameters: 1024,
        folds_per_line: 1_048_576,
    };

    /// The most octets one content line's name and parameters may occupy together.
    ///
    /// A header has a ceiling because it is reassembled across folds through a scratch
    /// buffer. A value has no such ceiling here, because a value is delivered in chunks that
    /// the reader never buffers.
    #[must_use]
    pub const fn max_header_bytes(self) -> u32 {
        self.header_bytes
    }

    /// The most parameters one content line may carry.
    #[must_use]
    pub const fn max_parameters(self) -> u32 {
        self.parameters
    }

    /// The most continuation lines one content line may be folded across.
    ///
    /// A value has no octet ceiling at this layer because its chunks are never buffered, but
    /// each fold *is* retained: the reader records where the producer folded so the writer can
    /// put the fold back, and a caller that never states this bound has a line of nothing but
    /// continuations charging its memory rather than its budget. A hundred thousand octets of
    /// `LF SP` is one item, one octet of value, and no header at all, so no other bound here
    /// sees it.
    ///
    /// The default is the width the specification asks for divided into the octets a value is
    /// allowed to carry, with headroom: a one mebibyte value folded at section 3.1's
    /// seventy-five octets needs about fourteen thousand continuations, so sixteen thousand
    /// three hundred and eighty-four accepts every legitimate line and refuses a line whose
    /// continuations outnumber its content.
    #[must_use]
    pub const fn max_folds_per_line(self) -> u32 {
        self.folds_per_line
    }

    /// The same policy with a different header bound.
    #[must_use]
    pub const fn with_max_header_bytes(self, bytes: u32) -> Self {
        Self {
            header_bytes: bytes,
            ..self
        }
    }

    /// The same policy with a different parameter-count bound.
    #[must_use]
    pub const fn with_max_parameters(self, count: u32) -> Self {
        Self {
            parameters: count,
            ..self
        }
    }

    /// The same policy with a different per-line fold bound.
    #[must_use]
    pub const fn with_max_folds_per_line(self, folds: u32) -> Self {
        Self {
            folds_per_line: folds,
            ..self
        }
    }
}

impl Default for GrammarLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The caller's immutable policy for every bound this workspace observes.
///
/// `Copy` with private fields and `with_*` builders, so adding a field is not a breaking
/// change and no caller can construct a policy that skips a bound. Every accessor takes
/// `self` by value: the type is small enough that a reference would be the slower option and
/// the workspace's own Clippy profile rejects it.
///
/// The numbers below are provisional. A budget right for a phone rendering one month is
/// wrong for a server indexing a decade, and `docs/adr/0007` is explicit that the real
/// numbers want corpus measurement before they are asserted anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Limits {
    /// The bounds the content-line grammar observes on its own.
    grammar: GrammarLimits,
    /// The octet budget for one parse, charged as octets are appended.
    max_input_bytes: u64,
    /// The most octets one property value may occupy.
    max_value_bytes: u32,
    /// The deepest components may nest.
    max_component_depth: u16,
    /// The most properties and components one document may hold together.
    max_items: u32,
    /// The most recurrence candidates that may be generated per period.
    candidates_per_period: u32,
    /// The most `RECURRENCE-ID` overrides one series may carry.
    override_entries: u32,
    /// The most observances one `VTIMEZONE` may define.
    max_vtimezone_observances: u32,
    /// The most `VTIMEZONE` components one calendar may define.
    max_vtimezone_components: u32,
    /// The most components one scheduling payload may carry.
    max_payload_components: u32,
    /// The most attendees one component may carry.
    max_attendees: u32,
    /// The deepest XML elements may nest.
    max_xml_depth: u16,
    /// The most XML elements one request or response may hold.
    max_xml_elements: u32,
    /// The octet budget for one XML request or response body.
    max_response_bytes: u64,
    /// The most octets one `href` may occupy.
    max_href_bytes: u32,
}

impl Limits {
    /// The policy a caller gets by saying nothing.
    pub const DEFAULT: Self = Self {
        grammar: GrammarLimits::DEFAULT,
        max_input_bytes: 16 * 1024 * 1024,
        max_value_bytes: 1024 * 1024,
        max_component_depth: 32,
        max_items: 100_000,
        candidates_per_period: 65_536,
        override_entries: 4096,
        max_vtimezone_observances: 4096,
        max_vtimezone_components: 256,
        max_payload_components: 1024,
        max_attendees: 4096,
        max_xml_depth: 64,
        max_xml_elements: 100_000,
        max_response_bytes: 64 * 1024 * 1024,
        max_href_bytes: 4096,
    };

    /// The policy for a server that has memory and expects large calendars.
    ///
    /// A named second policy rather than a builder chain, because two crates each invented
    /// one separately and a name they can share is worth more than the flexibility.
    pub const GENEROUS: Self = Self {
        grammar: GrammarLimits::GENEROUS,
        max_input_bytes: 512 * 1024 * 1024,
        max_value_bytes: 64 * 1024 * 1024,
        max_component_depth: 64,
        max_items: 5_000_000,
        candidates_per_period: 1_048_576,
        override_entries: 65_536,
        max_vtimezone_observances: 65_536,
        max_vtimezone_components: 4096,
        max_payload_components: 65_536,
        max_attendees: 65_536,
        max_xml_depth: 128,
        max_xml_elements: 5_000_000,
        max_response_bytes: 1024 * 1024 * 1024,
        max_href_bytes: 65_536,
    };

    /// The bounds the content-line grammar observes on its own.
    #[must_use]
    pub const fn grammar(self) -> GrammarLimits {
        self.grammar
    }

    /// The octet budget for one parse.
    #[must_use]
    pub const fn max_input_bytes(self) -> u64 {
        self.max_input_bytes
    }

    /// The most octets one property value may occupy.
    #[must_use]
    pub const fn max_value_bytes(self) -> u32 {
        self.max_value_bytes
    }

    /// The deepest components may nest.
    #[must_use]
    pub const fn max_component_depth(self) -> u16 {
        self.max_component_depth
    }

    /// The most properties and components one document may hold together.
    #[must_use]
    pub const fn max_items(self) -> u32 {
        self.max_items
    }

    /// The most recurrence candidates that may be generated per period.
    ///
    /// Counted over candidates *generated*, not instances emitted. A fine `BYxxx`
    /// combination under a negative `BYSETPOS` does the work either way, and counting the
    /// output would leave that work unbounded.
    #[must_use]
    pub const fn candidates_per_period(self) -> u32 {
        self.candidates_per_period
    }

    /// The most `RECURRENCE-ID` overrides one series may carry.
    #[must_use]
    pub const fn override_entries(self) -> u32 {
        self.override_entries
    }

    /// The most observances one `VTIMEZONE` may define.
    #[must_use]
    pub const fn max_vtimezone_observances(self) -> u32 {
        self.max_vtimezone_observances
    }

    /// The most `VTIMEZONE` components one calendar may define.
    #[must_use]
    pub const fn max_vtimezone_components(self) -> u32 {
        self.max_vtimezone_components
    }

    /// The most components one scheduling payload may carry.
    #[must_use]
    pub const fn max_payload_components(self) -> u32 {
        self.max_payload_components
    }

    /// The most attendees one component may carry.
    #[must_use]
    pub const fn max_attendees(self) -> u32 {
        self.max_attendees
    }

    /// The deepest XML elements may nest.
    #[must_use]
    pub const fn max_xml_depth(self) -> u16 {
        self.max_xml_depth
    }

    /// The most XML elements one request or response may hold.
    #[must_use]
    pub const fn max_xml_elements(self) -> u32 {
        self.max_xml_elements
    }

    /// The octet budget for one XML request or response body.
    #[must_use]
    pub const fn max_response_bytes(self) -> u64 {
        self.max_response_bytes
    }

    /// The most octets one `href` may occupy.
    #[must_use]
    pub const fn max_href_bytes(self) -> u32 {
        self.max_href_bytes
    }

    /// The same policy with different grammar bounds.
    #[must_use]
    pub const fn with_grammar(self, grammar: GrammarLimits) -> Self {
        Self { grammar, ..self }
    }

    /// The same policy with a different input budget.
    #[must_use]
    pub const fn with_max_input_bytes(self, bytes: u64) -> Self {
        Self {
            max_input_bytes: bytes,
            ..self
        }
    }

    /// The same policy with a different per-value bound.
    #[must_use]
    pub const fn with_max_value_bytes(self, bytes: u32) -> Self {
        Self {
            max_value_bytes: bytes,
            ..self
        }
    }

    /// The same policy with a different component nesting bound.
    #[must_use]
    pub const fn with_max_component_depth(self, depth: u16) -> Self {
        Self {
            max_component_depth: depth,
            ..self
        }
    }

    /// The same policy with a different item-count bound.
    #[must_use]
    pub const fn with_max_items(self, items: u32) -> Self {
        Self {
            max_items: items,
            ..self
        }
    }

    /// The same policy with a different per-period candidate budget.
    #[must_use]
    pub const fn with_candidates_per_period(self, candidates: u32) -> Self {
        Self {
            candidates_per_period: candidates,
            ..self
        }
    }

    /// The same policy with a different override-entry bound.
    #[must_use]
    pub const fn with_override_entries(self, entries: u32) -> Self {
        Self {
            override_entries: entries,
            ..self
        }
    }

    /// The same policy with a different observance bound.
    #[must_use]
    pub const fn with_max_vtimezone_observances(self, observances: u32) -> Self {
        Self {
            max_vtimezone_observances: observances,
            ..self
        }
    }

    /// The same policy with a different `VTIMEZONE` component bound.
    #[must_use]
    pub const fn with_max_vtimezone_components(self, components: u32) -> Self {
        Self {
            max_vtimezone_components: components,
            ..self
        }
    }

    /// The same policy with a different payload-component bound.
    #[must_use]
    pub const fn with_max_payload_components(self, components: u32) -> Self {
        Self {
            max_payload_components: components,
            ..self
        }
    }

    /// The same policy with a different attendee bound.
    #[must_use]
    pub const fn with_max_attendees(self, attendees: u32) -> Self {
        Self {
            max_attendees: attendees,
            ..self
        }
    }

    /// The same policy with a different XML nesting bound.
    #[must_use]
    pub const fn with_max_xml_depth(self, depth: u16) -> Self {
        Self {
            max_xml_depth: depth,
            ..self
        }
    }

    /// The same policy with a different XML element-count bound.
    #[must_use]
    pub const fn with_max_xml_elements(self, elements: u32) -> Self {
        Self {
            max_xml_elements: elements,
            ..self
        }
    }

    /// The same policy with a different response budget.
    #[must_use]
    pub const fn with_max_response_bytes(self, bytes: u64) -> Self {
        Self {
            max_response_bytes: bytes,
            ..self
        }
    }

    /// The same policy with a different `href` bound.
    #[must_use]
    pub const fn with_max_href_bytes(self, bytes: u32) -> Self {
        Self {
            max_href_bytes: bytes,
            ..self
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The caller's running ledger of work already done under a [`Limits`] policy.
///
/// Exhaustion latches: once the budget is crossed, every later charge fails, so a caller
/// that ignores one `false` cannot spend its way back into a clean answer.
///
/// Three shapes over one accounting, rather than three accountings. [`Meter::charge`]
/// returns `bool` because a recurrence search needs a budget breach to be a reported outcome
/// it can keep iterating past; [`Meter::try_charge`] is the identical charge for callers
/// whose surrounding code is already `Result`-shaped; and the `charge_*` methods convert the
/// same breach into a [`ParseError`], because a document a reader cannot finish is not one
/// it can hand back.
#[derive(Debug)]
pub struct Meter {
    /// The policy this ledger is kept under.
    limits: Limits,
    /// The octet ceiling for this ledger, which may be tighter than the policy's.
    budget: u64,
    /// Octets charged so far, saturating.
    spent: u64,
    /// Items charged so far.
    items: u32,
    /// Components currently open.
    depth: u16,
    /// XML elements currently open.
    element_depth: u16,
    /// XML elements charged so far.
    elements: u32,
    /// Diagnostics a sink refused, saturating.
    dropped: u32,
    /// Whether the octet budget has been crossed.
    exhausted: bool,
}

impl Meter {
    /// A ledger over `limits`, budgeted at that policy's input bound.
    #[must_use]
    pub const fn new(limits: Limits) -> Self {
        Self::with_budget(limits, limits.max_input_bytes)
    }

    /// A ledger over `limits` with an octet budget of the caller's own choosing.
    ///
    /// The budget is separate from the policy so that one policy can be spread across many
    /// calls with a shared ceiling — the fan-out case this whole type exists for.
    #[must_use]
    pub const fn with_budget(limits: Limits, budget: u64) -> Self {
        Self {
            limits,
            budget,
            spent: 0,
            items: 0,
            depth: 0,
            element_depth: 0,
            elements: 0,
            dropped: 0,
            exhausted: false,
        }
    }

    /// Charge `units` octets. `false` once the budget is crossed, and `false` forever after.
    pub fn charge(&mut self, units: u64) -> bool {
        if self.exhausted {
            return false;
        }
        // Saturating rather than checked: at `u64::MAX` spent octets the answer is the same
        // refusal either way, and a wrap would report a clean ledger.
        self.spent = self.spent.saturating_add(units);
        if self.spent > self.budget {
            self.exhausted = true;
            return false;
        }
        true
    }

    /// Charge `units` octets, as a `Result`.
    pub fn try_charge(&mut self, units: u64) -> Result<(), LimitExceeded> {
        if self.charge(units) {
            Ok(())
        } else {
            Err(LimitExceeded::Budget)
        }
    }

    /// Charge `count` octets, as the failure a reader cannot continue past.
    pub fn charge_bytes(&mut self, count: u64) -> Result<(), ParseError> {
        if self.charge(count) {
            Ok(())
        } else {
            Err(ParseError::InputTooLarge { limit: self.budget })
        }
    }

    /// Charge `count` octets against the shared ledger, as a `Result` a protocol reader can
    /// carry.
    pub fn try_charge_bytes(&mut self, count: u64) -> Result<(), LimitExceeded> {
        self.try_charge(count)
    }

    /// Charge one item — one property or one component.
    pub fn charge_item(&mut self) -> Result<(), ParseError> {
        let next = self.items.saturating_add(1);
        if next > self.limits.max_items {
            return Err(ParseError::TooManyItems {
                limit: self.limits.max_items,
            });
        }
        self.items = next;
        Ok(())
    }

    /// Charge one XML element against the shared ledger.
    pub fn try_charge_element(&mut self) -> Result<(), LimitExceeded> {
        let next = self.elements.saturating_add(1);
        if next > self.limits.max_xml_elements {
            return Err(LimitExceeded::Elements);
        }
        self.elements = next;
        Ok(())
    }

    /// Open one component. Paired with [`Meter::leave`].
    ///
    /// The depth is not restored on refusal, because a reader that has been told it is too
    /// deep has nothing left to close.
    pub fn enter(&mut self) -> Result<(), ParseError> {
        let next = self.depth.saturating_add(1);
        if next > self.limits.max_component_depth {
            return Err(ParseError::TooDeep {
                limit: self.limits.max_component_depth,
            });
        }
        self.depth = next;
        Ok(())
    }

    /// Close one component.
    ///
    /// Saturating at zero rather than refusing, because an unmatched `END` is a diagnostic
    /// about the calendar and must not become an error about the ledger.
    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Open one XML element. Paired with [`Meter::leave_element`].
    pub fn try_enter_element(&mut self) -> Result<(), LimitExceeded> {
        let next = self.element_depth.saturating_add(1);
        if next > self.limits.max_xml_depth {
            return Err(LimitExceeded::Depth);
        }
        self.element_depth = next;
        Ok(())
    }

    /// Close one XML element.
    pub fn leave_element(&mut self) {
        self.element_depth = self.element_depth.saturating_sub(1);
    }

    /// Record that a sink refused a diagnostic.
    ///
    /// Called through [`report_diagnostic`](crate::report_diagnostic) rather than directly.
    /// The count lives here because a sink that keeps nothing cannot also remember how much
    /// it did not keep, and a caller that loses *which* violations occurred must not also
    /// lose *that* they did.
    pub fn note_dropped_diagnostic(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    /// Diagnostics a sink refused. A nonzero count is not a clean parse.
    #[must_use]
    pub const fn diagnostics_dropped(&self) -> u32 {
        self.dropped
    }

    /// Whether the octet budget has been crossed.
    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// Octets charged so far.
    #[must_use]
    pub const fn spent(&self) -> u64 {
        self.spent
    }

    /// The octet ceiling for this ledger.
    #[must_use]
    pub const fn budget(&self) -> u64 {
        self.budget
    }

    /// Items charged so far.
    #[must_use]
    pub const fn items(&self) -> u32 {
        self.items
    }

    /// Components currently open.
    #[must_use]
    pub const fn depth(&self) -> u16 {
        self.depth
    }

    /// The policy this ledger is kept under.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }
}

#[cfg(test)]
mod tests {
    use super::{GrammarLimits, Limits, Meter};
    use crate::failure::{LimitExceeded, ParseError};

    #[test]
    fn a_builder_changes_one_bound_and_leaves_the_rest() {
        let tightened = Limits::DEFAULT.with_max_input_bytes(4096);
        assert_eq!(tightened.max_input_bytes(), 4096);
        assert_eq!(tightened.max_items(), Limits::DEFAULT.max_items());
        assert_eq!(tightened.grammar(), GrammarLimits::DEFAULT);
    }

    #[test]
    fn exhaustion_latches_so_a_budget_that_binds_keeps_binding() {
        let mut meter = Meter::with_budget(Limits::DEFAULT, 10);
        assert!(meter.charge(10));
        assert!(!meter.charge(1));
        assert!(meter.is_exhausted());
        assert!(!meter.charge(0), "a charge after exhaustion never succeeds");
    }

    #[test]
    fn one_accounting_is_reported_in_three_shapes() {
        let mut meter = Meter::with_budget(Limits::DEFAULT, 4);
        assert_eq!(meter.try_charge(8), Err(LimitExceeded::Budget));
        assert_eq!(
            meter.charge_bytes(1),
            Err(ParseError::InputTooLarge { limit: 4 })
        );
        assert!(!meter.charge(1));
        assert_eq!(meter.spent(), 8, "one ledger, whichever shape charged it");
    }

    #[test]
    fn depth_is_refused_before_it_is_recorded() {
        let limits = Limits::DEFAULT.with_max_component_depth(1);
        let mut meter = Meter::new(limits);
        assert_eq!(meter.enter(), Ok(()));
        assert_eq!(meter.enter(), Err(ParseError::TooDeep { limit: 1 }));
        meter.leave();
        meter.leave();
        assert_eq!(meter.depth(), 0, "closing more than was opened saturates");
    }

    #[test]
    fn an_item_bound_counts_properties_and_components_together() {
        let mut meter = Meter::new(Limits::DEFAULT.with_max_items(2));
        assert_eq!(meter.charge_item(), Ok(()));
        assert_eq!(meter.charge_item(), Ok(()));
        assert_eq!(
            meter.charge_item(),
            Err(ParseError::TooManyItems { limit: 2 })
        );
        assert_eq!(meter.items(), 2);
    }
}
