// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Explicit, versioned compatibility processing outside the strict kernel.

use alloc::vec::Vec;
use core::fmt::{self, Display, Formatter};

use ical_core::Meter;

use crate::{Calendar, Error, ResourcePolicy};

/// Lossless imported octets that have not been promoted to a validated calendar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    bytes: Vec<u8>,
    policy: ResourcePolicy,
}

impl Import {
    /// Retain all input octets under secure resource defaults.
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        let mut meter = Meter::new(policy.limits);
        Self::read_with_policy(bytes, policy, &mut meter)
    }

    pub(crate) fn read_with_policy(
        bytes: &[u8],
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<Self, Error> {
        meter
            .try_charge_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .map_err(|_| Error::single("icalkit.import.resource-limit"))?;
        Ok(Self {
            bytes: bytes.to_vec(),
            policy,
        })
    }

    /// The original or explicitly normalized octets.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Strictly validate and promote these octets.
    pub fn validate(&self) -> Result<Calendar, Error> {
        Calendar::parse(&self.bytes)
    }

    /// Apply one versioned normalization profile, preserving this import unchanged.
    pub fn normalize<P: NormalizationProfile>(&self, _profile: P) -> Result<Normalization, Error> {
        let (bytes, changes) = match P::KIND {
            sealed::RFC_REPAIR_V1 => normalize_line_endings(&self.bytes),
            _ => (self.bytes.clone(), Vec::new()),
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > self.policy.max_input_bytes() {
            return Err(Error::single("icalkit.normalize.resource-limit"));
        }
        Ok(Normalization {
            output: Self {
                bytes,
                policy: self.policy,
            },
            changes,
        })
    }
}

mod sealed {
    pub(super) const RFC_REPAIR_V1: u8 = 1;
    pub(super) const COMMON_CLIENTS_V1: u8 = 2;

    pub(super) trait Sealed {
        const KIND: u8;
    }
}

/// A closed set of versioned normalization profiles.
#[allow(private_bounds)]
pub trait NormalizationProfile: sealed::Sealed + Copy {}

/// Conservative RFC line-ending repairs, version 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RfcRepairV1;

impl sealed::Sealed for RfcRepairV1 {
    const KIND: u8 = sealed::RFC_REPAIR_V1;
}

impl NormalizationProfile for RfcRepairV1 {}

/// Corpus-backed compatibility repairs for common clients, version 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommonClientsV1;

impl sealed::Sealed for CommonClientsV1 {
    const KIND: u8 = sealed::COMMON_CLIENTS_V1;
}

impl NormalizationProfile for CommonClientsV1 {}

/// The immutable result of one explicit normalization pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Normalization {
    output: Import,
    changes: Vec<Change>,
}

impl Normalization {
    /// The normalized octets, still unvalidated.
    #[must_use]
    pub const fn output(&self) -> &Import {
        &self.output
    }

    /// Every change made by this profile, in input order.
    #[must_use]
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
}

/// A stable machine-readable normalization change identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChangeCode(&'static str);

impl ChangeCode {
    /// The stable string spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Display for ChangeCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// One reported normalization change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    code: ChangeCode,
    offset: u64,
}

impl Change {
    /// The stable kind of change.
    #[must_use]
    pub const fn code(self) -> ChangeCode {
        self.code
    }

    /// The input offset where the repaired spelling began.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }
}

fn normalize_line_endings(input: &[u8]) -> (Vec<u8>, Vec<Change>) {
    let mut output = Vec::with_capacity(input.len());
    let mut changes = Vec::new();
    let mut cursor = 0_usize;
    while cursor < input.len() {
        match input[cursor] {
            b'\r' if input.get(cursor.saturating_add(1)) == Some(&b'\n') => {
                output.extend_from_slice(b"\r\n");
                cursor = cursor.saturating_add(2);
            },
            b'\r' | b'\n' => {
                output.extend_from_slice(b"\r\n");
                changes.push(Change {
                    code: ChangeCode("icalkit.normalize.line-ending"),
                    offset: u64::try_from(cursor).unwrap_or(u64::MAX),
                });
                cursor = cursor.saturating_add(1);
            },
            octet => {
                output.push(octet);
                cursor = cursor.saturating_add(1);
            },
        }
    }
    (output, changes)
}
