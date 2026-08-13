// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iTIP review and authorization workflow vocabulary.

pub use ical_itip::{
    Authorization as Review, AuthorizationDenied as Rejection, Commitment as AuthorizedChange,
    ItipMessage as Message, Party as Actor,
};
