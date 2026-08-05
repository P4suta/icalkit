# ADR-0005: scheduling semantics live apart from the data model

- Status: accepted
- Date: 2026-08-05

## Context

iTIP (RFC 5546) is a state machine over messages: an organizer publishes a `REQUEST`,
attendees return a `REPLY`, the organizer sends a `CANCEL` or a partial update, sequence
numbers arbitrate which version wins, and the whole exchange has rules about who is
permitted to change what.

Those rules are not properties of an `.ics` file. They are properties of a *conversation*
between parties, and they need state the file does not carry: who am I in this exchange,
what did I last see, and is this message authorized to make the change it is asking for.

Folding scheduling into the calendar model means every caller who just wants to read an
`.ics` file drags it along, and it means the model acquires a notion of identity that
parsing has no business having.

## Decision

`ical-core` knows about components and properties. It does not know what a `METHOD` means,
who the organizer is relative to the current user, or whether a `SEQUENCE` bump should be
accepted.

`ical-itip` is a separate crate that takes an incoming message, the current state of the
event, and the identity of the party applying it, and returns what changes — as a
description of the transition, not as a mutated object. Applying it is the caller's
decision.

Authorization is part of the semantics, not an afterthought: an attendee cannot change an
event's time by replying, and a `REPLY` from an address that is not on the attendee list is
a rejected message rather than a silent participant addition. Those are exactly the
positions where scheduling implementations have historically been exploited.

## Consequences

A caller who only reads calendars never compiles the scheduling crate.

The transition being a value rather than a mutation means it can be shown to a user before
being applied, which is what a mail client actually needs when it displays "this meeting
was moved — accept?".

iMIP (RFC 6047), which carries the same messages over email, becomes a thin layer over the
same state machine rather than a second implementation of it.
