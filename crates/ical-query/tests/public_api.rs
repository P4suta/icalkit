// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! External-consumer compile fixture for the query workflow surface.

#[allow(unused_imports)]
use ical_query::{
    BusyAnswer, Expansion, Instance, InstanceSpan, Placement, SearchBounds, Series, SeriesClock,
    Unplaced, ZONE_SLACK_SECONDS, expand, free_busy, limit_freebusy_set, limit_recurrence_set,
    matches, overlaps, select, without_values,
};

#[test]
fn workflow_types_have_one_reachable_root_path() {
    assert_eq!(ZONE_SLACK_SECONDS, 86_399);
    assert!(core::any::type_name::<Instance>().starts_with("ical_query::"));
    assert!(core::any::type_name::<Expansion>().starts_with("ical_query::"));
    assert!(core::any::type_name::<BusyAnswer>().starts_with("ical_query::"));
}
