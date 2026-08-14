// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private iTIP scheduling kernel.
//!
//! The unpublished conformance helper also compiles these files to exercise the low-level
//! adversarial corpus. The private ancestor prevents the kernel API from becoming a second
//! production path.

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

pub use crate::internal::core::{Limits, Meter, ParameterEdit, PropertyId, ProposedChange};
pub(crate) use authorize::evaluate_new_instance_payload;
pub use authorize::{
    Authorization, AuthorizationDenied, Commitment, actor_role, apply_transition, attendee_index,
    evaluate_initial_payload, evaluate_message,
};
pub use component::ScheduledView;
pub use diff::{attendee_occurrence_of, describe_message, describe_payload};
pub use freebusy::{
    BusyPeriod, FreeBusyError, FreeBusyKind, busy_periods, requested_window, window_of,
};
pub use identity::{
    FoldSide, InstanceClock, InstanceMatch, InstanceRef, MessageIdentity, Revision, SequenceRead,
    Uid,
};
pub use imip::MediaTypeParams;
pub use instance::{
    ResolvedInstance, check_exclusions_are_placeable, exclusions_are_placeable, resolve_instance,
};
pub use message::{ItipMessage, MessageError};
pub use method::{ActorRole, Method, SenderRule};
pub use party::{ANSWERED_AT, Attendee, PartStat, Party, PartyId, Role};
pub use report::inspect_message;
pub use state::{PropertyOccurrence, ScheduledComponent, property_value};
pub use table::{MethodRule, Presence, PriorState, Rule};
pub use target::ComponentTarget;
pub use transition::{
    ApplyReport, Changes, FieldRule, RejectedChange, ScheduleTarget, Transition, TransitionReason,
    WriteRejected, field_rule, is_time_property,
};
