// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private implementation layers with no independent semver contract.

// The migrated source still contains its former public surface, including units not yet reached
// by a facade workflow. This private ancestor makes those items unreachable outside icalkit.
// Keep the exemption on this one migration boundary until each unit is connected or removed.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub(crate) mod query;

#[allow(dead_code, unused_imports, unreachable_pub)]
pub(crate) mod recur;

#[allow(dead_code, unused_imports, unreachable_pub)]
pub(crate) mod tz;

// The former crate remains temporarily as a shared-source conformance harness. Its source of
// truth is this module; the facade may not depend back on that compatibility package.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub(crate) mod itip;
