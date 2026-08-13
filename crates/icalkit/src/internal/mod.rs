// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private implementation layers with no independent semver contract.

// The migrated source still contains its former public surface, including units not yet reached
// by a facade workflow. This private ancestor makes those items unreachable outside icalkit.
// Keep the exemption on this one migration boundary until each unit is connected or removed.
#[allow(dead_code, unused_imports, unreachable_pub)]
pub(crate) mod query;
