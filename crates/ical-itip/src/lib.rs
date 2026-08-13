// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scheduling (RFC 5546): what an incoming iTIP message would change, described rather than
//! applied.
//!
//! Specification: RFC 5546, "iCalendar Transport-Independent Interoperability Protocol
//! (iTIP)" <https://www.rfc-editor.org/rfc/rfc5546>, and RFC 6047 for the same messages
//! carried over email.
//!
//! iTIP is a state machine over a conversation, not a property of a file. An organizer
//! sends a `REQUEST`, attendees return a `REPLY`, an update or a `CANCEL` follows, and
//! `SEQUENCE` together with `DTSTAMP` decides which version wins. Answering "what does this
//! message do" needs state no `.ics` carries: who am I in this exchange, what did I last
//! see, and is the sender entitled to the change being asked for (see `docs/adr/0005`).
//!
//! So this crate takes the incoming message, the current state of the event, and the
//! identity of the party applying it, and returns a description of the transition. It
//! mutates nothing. Applying the description is the caller's decision, which is what a mail
//! client needs in order to show "this meeting was moved — accept?" before touching the
//! user's calendar, and what a server needs in order to record the same transition on its
//! own terms.
//!
//! Authorization is part of the semantics rather than a layer somebody adds later. An
//! attendee cannot move a meeting by replying to it, a `REPLY` from an address that is not
//! on the attendee list is a rejected message rather than a silently added participant, and
//! a stale `SEQUENCE` does not overwrite a newer one. Those are precisely the positions
//! where scheduling implementations have historically been exploited, and they are cheap to
//! get right only if the message and the identity arrive together.
//!
//! `ical-core` knows nothing of any of this, so a caller who only reads calendars never
//! compiles it. iMIP is a thin layer over this state machine: the MIME envelope and the
//! trust placed in the sending address change, the semantics do not.
//!
//! The transition is described in `ical-core`'s own change vocabulary rather than in a
//! private one, so applying it is the caller handing that description back to the model, and
//! the dependency runs one way and cannot invert (see `docs/adr/0005`). A change addresses a
//! property *occurrence* — the second `ATTENDEE`, not `ATTENDEE` — because a scheduling
//! message routinely changes one participant among many.
//!
//! # Status
//!
//! M3 is landed and tested end to end: the eight methods and their sender rules, RFC 5546
//! section 3's twenty-two constraint tables transcribed as data, the party and instance
//! identities including the fold side M2 left open, the message model with its bounds, the
//! occurrence-addressed transition, the octet diff, the authorization gate with its ordered
//! denials, both bridges to [`ical_core::Component`] — [`ScheduledView`] for reading and
//! [`ComponentTarget`] for writing — the zone-aware instance resolution, the reporting pass
//! behind every `scheduling-*` diagnostic code, and the two feature modules.
//!
//! `RANGE=THISANDFUTURE` is represented here, and the authorization gate can now judge one
//! organizer-authored anchor against a held master without weakening sender, revision or field
//! checks. This kernel still does not own a calendar container and therefore does not split one;
//! the single public `icalkit::scheduling` workflow materializes an authorized `REQUEST` as a
//! detached component and validates that its cadence key belongs to the master. Method-specific
//! range behavior beyond that workflow and a `COUNTER` that changes a time remain incomplete.
//! This crate is not RFC-5546-complete and nothing here entitles anyone to say it is. See
//! `ROADMAP.md` (M3) and `docs/design/ical-itip-api.md`.
//!
//! # Reading order
//!
//! [`method`] and [`table`] are RFC 5546 as data. [`identity`] and [`party`] are what a
//! message is about and who it is from. [`state`] is how a caller offers what it already
//! holds, and [`component`] is that trait answered for an [`ical_core::Component`].
//! [`message`] is the checked-and-charged message. [`instance`] is the one place a zone is
//! asked anything, and it runs before the gate. [`transition`] is what would change and
//! [`diff`] works it out. [`authorize`] decides whether it may happen, and its module
//! documentation is where the byte-boundary question is answered. [`target`] writes an
//! authorized transition back. [`report`] emits what the gate refuses to turn into a denial,
//! and it changes no authorization answer.

#![no_std]

extern crate alloc;

pub mod authorize;
pub mod component;
pub mod diff;
#[cfg(feature = "freebusy")]
pub mod freebusy;
pub mod identity;
#[cfg(feature = "imip")]
pub mod imip;
pub mod instance;
pub mod message;
pub mod method;
pub mod party;
pub mod report;
pub mod state;
pub mod table;
pub mod target;
pub mod transition;

pub use crate::authorize::{
    Authorization, AuthorizationDenied, Commitment, actor_role, apply_transition, attendee_index,
    evaluate_message,
};
pub use crate::component::ScheduledView;
pub use crate::diff::{attendee_occurrence_of, describe_message, describe_payload};
#[cfg(feature = "freebusy")]
pub use crate::freebusy::{
    BusyPeriod, FreeBusyError, FreeBusyKind, busy_periods, requested_window, window_of,
};
pub use crate::identity::{
    FoldSide, InstanceClock, InstanceMatch, InstanceRef, MessageIdentity, Revision, SequenceRead,
    Uid,
};
pub use crate::instance::{
    ResolvedInstance, check_exclusions_are_placeable, exclusions_are_placeable, resolve_instance,
};
pub use crate::message::{ItipMessage, MessageError};
pub use crate::method::{ActorRole, Method, SenderRule};
pub use crate::party::{ANSWERED_AT, Attendee, PartStat, Party, PartyId, Role};
pub use crate::report::inspect_message;
pub use crate::state::{PropertyOccurrence, ScheduledComponent};
pub use crate::table::{MethodRule, Presence, PriorState, Rule};
pub use crate::target::ComponentTarget;
pub use crate::transition::{
    ApplyReport, Changes, FieldRule, RejectedChange, ScheduleTarget, Transition, TransitionReason,
    WriteRejected, field_rule, is_time_property,
};
// The shared vocabulary is re-exported so that a caller names one crate for one concept, the
// way `ical-tz` re-exports the civil types. `ProposedChange` and `ParameterEdit` in particular:
// a transition is described in `ical-core`'s words, and a caller reading one should not have
// to know which crate the words came from.
pub use ical_core::{Limits, Meter, ParameterEdit, PropertyId, ProposedChange};
