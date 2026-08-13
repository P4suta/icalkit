// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The WebDAV XML grammar, with no CalDAV vocabulary anywhere in it.
//!
//! # What this module is, and why it is a module rather than a crate
//!
//! `docs/adr/0004` deferred extracting a `webdav-core` "until a second DAV-shaped consumer
//! exists in this workspace to justify the extraction", and its own Consequences then recorded
//! that the deferral was costing more than it did, because what was being kept private had grown
//! from a small tag matcher into a namespace-resolving reader and writer. `docs/adr/0012`
//! settles it: the crate is **not** published, and the expensive half of the extraction — the
//! untangling — happens anyway. This directory is that untangling.
//!
//! Nothing here may name a CalDAV type. Not `ElementName`, not `Namespace`, not `DavError`, not
//! a single element of RFC 4791's vocabulary. What it holds is the part of the grammar that is
//! true of any `DAV:`-shaped body: how a name is spelled, how a prefix is bound and unbound,
//! which characters XML 1.0 admits, and what a reference resolves to. On the day a second
//! DAV-shaped consumer is accepted, moving this directory out is a file move and a manifest
//! rather than a redesign — and a published crate name cannot be withdrawn, while an unexported
//! module can.
//!
//! # Nothing here is exported
//!
//! Every item is `pub(crate)`, there is no `#[doc(hidden)] pub` escape, and `lib.rs` re-exports
//! none of it. That is the whole point: the harm ADR 0004's ordering bet guards against is
//! caused by *exporting* the grammar rather than by leaving it in place, so not exporting it
//! costs a gate instead of a crate name.
//!
//! # How the boundary is enforced
//!
//! Two rules, and they close different holes.
//!
//! `gates/xml-layering` compiles this directory in a root whose only dependency is `ical-core` —
//! the shared `Limits`, `Meter` and `LimitExceeded`, which are the model's and not CalDAV's. A
//! path from here into the CalDAV vocabulary above resolves in `ical-dav` and does not resolve
//! there, so `use crate::ElementName;` inside this layer is a compile error with a file and a
//! line rather than a convention somebody remembers.
//!
//! What that cannot catch is `crate::X` for an `X` the crate root re-exports *from this layer*,
//! because it would resolve in the gate too. So the second rule is textual and lives in `xtask
//! purity`: no path under this directory may resolve above it, the tree stays flat, `extern
//! crate` and `#[path]` are refused outright, and the files beside `mod.rs` are held equal to
//! the modules `mod.rs` declares.
//!
//! # What deliberately remains above this layer
//!
//! The `xmlparser` token stream is consumed in `ical-dav` because the wrapper is stated over
//! types that *are* CalDAV's — an `ElementName` row on every event, a `Namespace`
//! classification on every name, and `DavError` in every public signature. The element writer
//! stays there for the same reason. What lives here is vocabulary-independent wrapper logic:
//! namespace bindings, character/reference rules, and byte-oriented scans needed by the two
//! explicitly octet-preserving text modes.

// The layer is flat and every file beside this one is declared here. `xtask purity` holds the
// two sets equal in both directions, because a file the module root does not declare is
// invisible to the compile half of the gate and a module declared with no file beside it
// resolves somewhere no rule reads.
// Four modules and no re-export block. A consumer writes `crate::xml::scan::split_name`, which
// says which half of the layer it reached into; a flat re-export would also mean this file grows
// an unused import the day one of the two roots that compile these sources stops needing an
// item, and a gate that has to be edited to keep a lint quiet is a gate somebody edits.
pub(crate) mod bind;
pub(crate) mod chars;
pub(crate) mod fault;
pub(crate) mod scan;
