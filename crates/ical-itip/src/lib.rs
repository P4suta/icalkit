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
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M3). The public surface is
//! designed and compiled; `docs/design/ical-itip-api.md` carries it.

#![no_std]

extern crate alloc;
