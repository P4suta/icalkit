// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private iTIP scheduling kernel.
//!
//! Files in this module are also compiled by the temporary `ical-itip` conformance harness.
//! Keeping the former crate-shaped root here lets the facade absorb the implementation without
//! copying it or exposing the old API as a second public path.

mod authorize;
mod component;
mod diff;
mod freebusy;
mod identity;
mod imip;
mod instance;
mod message;
mod method;
mod party;
mod report;
mod state;
mod table;
mod target;
mod transition;

pub(crate) use crate::internal::core::{Limits, Meter, ParameterEdit, PropertyId, ProposedChange};
pub(crate) use authorize::{
    Authorization, AuthorizationDenied, Commitment, actor_role, apply_transition, attendee_index,
    evaluate_message,
};
pub(crate) use component::ScheduledView;
pub(crate) use diff::{attendee_occurrence_of, describe_message, describe_payload};
pub(crate) use freebusy::{
    BusyPeriod, FreeBusyError, FreeBusyKind, busy_periods, requested_window, window_of,
};
pub(crate) use identity::{
    FoldSide, InstanceClock, InstanceMatch, InstanceRef, MessageIdentity, Revision, SequenceRead,
    Uid,
};
pub(crate) use instance::{
    ResolvedInstance, check_exclusions_are_placeable, exclusions_are_placeable, resolve_instance,
};
pub(crate) use message::{ItipMessage, MessageError};
pub(crate) use method::{ActorRole, Method, SenderRule};
pub(crate) use party::{ANSWERED_AT, Attendee, PartStat, Party, PartyId, Role};
pub(crate) use report::inspect_message;
pub(crate) use state::{PropertyOccurrence, ScheduledComponent};
pub(crate) use table::{MethodRule, Presence, PriorState, Rule};
pub(crate) use target::ComponentTarget;
pub(crate) use transition::{
    ApplyReport, Changes, FieldRule, RejectedChange, ScheduleTarget, Transition, TransitionReason,
    WriteRejected, field_rule, is_time_property,
};
