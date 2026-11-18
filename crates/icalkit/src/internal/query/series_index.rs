// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sibling components grouped by their decoded series identity.

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::internal::core::codec::decode_single_scalar_text;
use crate::internal::core::{Component, ComponentKind, Item, LimitExceeded, Meter, PropertyId};
use crate::internal::query::Undecided;

#[derive(Debug)]
pub(super) struct SiblingScope<'a> {
    items: &'a [Item],
    index: Option<SeriesIndex<'a>>,
}

impl<'a> SiblingScope<'a> {
    pub(super) const fn new(items: &'a [Item]) -> Self {
        Self { items, index: None }
    }

    pub(super) fn index(&mut self, meter: &mut Meter) -> Result<&SeriesIndex<'a>, LimitExceeded> {
        match &mut self.index {
            Some(index) => Ok(index),
            slot @ None => Ok(slot.insert(SeriesIndex::new(self.items, meter)?)),
        }
    }
}

#[derive(Debug)]
pub(super) struct SeriesIndex<'a> {
    members: Vec<Member<'a>>,
}

#[derive(Debug)]
pub(super) struct Member<'a> {
    group: (ComponentKind, Role),
    uid: Option<Cow<'a, str>>,
    position: usize,
    pub(super) component: &'a Component,
}

impl Member<'_> {
    fn width(&self) -> Option<usize> {
        self.uid.as_deref().map(str::len)
    }

    fn bucket(&self) -> ((ComponentKind, Role), Option<usize>) {
        (self.group, self.width())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Role {
    Master,
    Override,
}

pub(super) fn is_override(component: &Component) -> bool {
    component
        .properties_named(&PropertyId::RECURRENCE_ID)
        .next()
        .is_some()
}

impl<'a> SeriesIndex<'a> {
    pub(super) fn new(items: &'a [Item], meter: &mut Meter) -> Result<Self, LimitExceeded> {
        meter.try_charge(octets(items.len()))?;
        let mut members = Vec::new();
        for (position, component) in items.iter().enumerate().filter_map(|(position, item)| {
            item.as_component().map(|component| (position, component))
        }) {
            let Some(kind) = component.kind() else {
                continue;
            };
            let uid = decoded_uid(component, meter)?;
            meter.try_charge(octets(core::mem::size_of::<Member<'_>>()))?;
            meter.admit(|| members.try_reserve(1).map_err(|_| LimitExceeded::Budget))?;
            members.push(Member {
                group: (
                    kind,
                    if is_override(component) {
                        Role::Override
                    } else {
                        Role::Master
                    },
                ),
                uid,
                position,
                component,
            });
        }
        meter
            .try_charge(octets(members.len()).saturating_mul(comparison_cost(members.len(), 1)))?;
        members.sort_unstable_by_key(Member::bucket);
        for bucket in members.chunk_by_mut(|left, right| left.bucket() == right.bucket()) {
            if let [first, _, ..] = bucket {
                let width = first.width().unwrap_or(1);
                meter.try_charge(
                    octets(bucket.len()).saturating_mul(comparison_cost(bucket.len(), width)),
                )?;
                bucket.sort_unstable_by(|left, right| {
                    (&left.uid, left.position).cmp(&(&right.uid, right.position))
                });
            }
        }
        Ok(Self { members })
    }

    pub(super) fn master(
        &self,
        candidate: &Component,
        meter: &mut Meter,
    ) -> Result<Result<Option<&'a Component>, Undecided>, LimitExceeded> {
        Ok(self
            .matching(candidate, Role::Master, meter)?
            .map(|group| group.first().map(|member| member.component)))
    }

    pub(super) fn related(
        &self,
        master: &Component,
        meter: &mut Meter,
    ) -> Result<Result<&[Member<'a>], Undecided>, LimitExceeded> {
        if is_override(master) {
            return Ok(Ok(&[]));
        }
        self.matching(master, Role::Override, meter)
    }

    fn matching(
        &self,
        component: &Component,
        role: Role,
        meter: &mut Meter,
    ) -> Result<Result<&[Member<'a>], Undecided>, LimitExceeded> {
        let Some(kind) = component.kind() else {
            return Ok(Ok(&[]));
        };
        let key = (kind, role);
        let group = equal_members(&self.members, 1, meter, |member| member.group.cmp(&key))?;
        let Some(first) = group.first() else {
            return Ok(Ok(&[]));
        };
        if first.uid.is_none() {
            return Ok(Err(Undecided::ValueUnreadable));
        }
        let Some(uid) = decoded_uid(component, meter)? else {
            return Ok(Err(Undecided::ValueUnreadable));
        };
        let bucket = equal_members(group, 1, meter, |member| {
            member.width().cmp(&Some(uid.len()))
        })?;
        Ok(Ok(equal_members(bucket, uid.len(), meter, |member| {
            member.uid.as_deref().cmp(&Some(uid.as_ref()))
        })?))
    }
}

fn equal_members<'index, 'doc>(
    members: &'index [Member<'doc>],
    width: usize,
    meter: &mut Meter,
    compare: impl Fn(&Member<'doc>) -> Ordering,
) -> Result<&'index [Member<'doc>], LimitExceeded> {
    if members.is_empty() {
        return Ok(&[]);
    }
    meter.try_charge(2_u64.saturating_mul(comparison_cost(members.len(), width)))?;
    let start = members.partition_point(|member| compare(member).is_lt());
    let end = members.partition_point(|member| compare(member).is_le());
    Ok(members.get(start..end).unwrap_or(&[]))
}

pub(crate) fn decoded_uid<'a>(
    component: &'a Component,
    meter: &mut Meter,
) -> Result<Option<Cow<'a, str>>, LimitExceeded> {
    meter.try_charge(octets(component.items().len()))?;
    if let Some(uid) = component.properties_named(&PropertyId::UID).next() {
        meter.try_charge(octets(uid.value_text().len()))?;
        for parameter in uid.parameters() {
            meter.try_charge(
                octets(parameter.name().len()).saturating_add(octets(parameter.value().len())),
            )?;
        }
    }
    Ok(decode_single_scalar_text(component, &PropertyId::UID))
}

fn comparison_cost(count: usize, width: usize) -> u64 {
    u64::from(count.checked_ilog2().unwrap_or(0))
        .saturating_add(1)
        .saturating_mul(octets(width.max(1)))
}

fn octets(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}
