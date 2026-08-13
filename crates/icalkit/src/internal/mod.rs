// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private implementation layers with no independent semver contract.

// The allocation-aware, sans-I/O foundation. The unpublished conformance helper compiles this
// module tree as shared source, while this remains the only production implementation.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod core;

#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod dav;

// The migrated source still contains its former public surface, including units not yet reached
// by a facade workflow. This private ancestor makes those items unreachable outside icalkit.
// Keep the exemption on this one migration boundary until each unit is connected or removed.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod query;

#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod recur;

#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod tz;

// The unpublished conformance helper compiles this module tree as shared source. Production
// code reaches it only through the public scheduling workflow.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub mod itip;
