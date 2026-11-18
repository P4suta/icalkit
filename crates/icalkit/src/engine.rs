// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::boxed::Box;
use core::cell::Cell;
use core::fmt::{self, Debug, Formatter};

use alloc::vec::Vec;

use crate::internal::core::{
    CivilDateTime, Diagnostic, DiagnosticCode, Instant, Location, Meter, Severity,
};
use crate::internal::tz::{AnswerBasis, OffsetAnswer, ZoneAnswer, ZoneSource};

use crate::caldav::Query;
use crate::calendar::parse_calendar;
use crate::interop::Import;
use crate::time::{ZoneAdapter, ZoneDatabase};
use crate::{Calendar, Error, Issue, ResourcePolicy};

/// Shared configuration and adapters for calendar workflows.
pub struct Engine {
    pub(crate) policy: ResourcePolicy,
    zones: Option<Box<dyn ZoneDatabase>>,
}

impl Engine {
    /// Begin configuring an engine.
    #[must_use]
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    /// Start a bounded session whose ledger spans every operation made through it.
    #[must_use]
    pub fn session(&self) -> Session<'_> {
        Session {
            engine: self,
            meter: Meter::new(self.policy.limits),
            recurrence_diagnostics: Vec::new(),
            workflow_issues: Vec::new(),
        }
    }

    /// The configured zone database, when one was supplied or installed by the default adapter.
    #[must_use]
    pub fn zone_database(&self) -> Option<&dyn ZoneDatabase> {
        self.zones.as_deref()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Debug for Engine {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("policy", &self.policy)
            .field("has_zone_database", &self.zones.is_some())
            .finish()
    }
}

/// Builder for an [`Engine`].
pub struct EngineBuilder {
    policy: ResourcePolicy,
    zones: Option<Box<dyn ZoneDatabase>>,
}

impl EngineBuilder {
    fn new() -> Self {
        #[cfg(feature = "system-tz")]
        let zones = Some(crate::time::default_zone_database());
        #[cfg(not(feature = "system-tz"))]
        let zones = None;
        Self {
            policy: ResourcePolicy::secure(),
            zones,
        }
    }

    /// Set the resource policy used by new sessions.
    #[must_use]
    pub const fn resource_policy(mut self, policy: ResourcePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Replace the default zone database with an application implementation.
    #[must_use]
    pub fn zone_database(mut self, zones: impl ZoneDatabase + 'static) -> Self {
        self.zones = Some(Box::new(zones));
        self
    }

    /// Finish building the engine.
    #[must_use]
    pub fn build(self) -> Engine {
        Engine {
            policy: self.policy,
            zones: self.zones,
        }
    }
}

impl Debug for EngineBuilder {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineBuilder")
            .field("policy", &self.policy)
            .field("has_zone_database", &self.zones.is_some())
            .finish()
    }
}

/// One aggregate resource-budget scope.
#[derive(Debug)]
pub struct Session<'a> {
    pub(crate) engine: &'a Engine,
    pub(crate) meter: Meter,
    pub(crate) recurrence_diagnostics: Vec<Diagnostic>,
    workflow_issues: Vec<Issue>,
}

impl Session<'_> {
    /// Notes from this session's zone-dependent workflows, including failed operations.
    ///
    /// Unasserted provider coverage is reported once per session without a boundary date.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.workflow_issues
    }

    pub(crate) fn with_zone_source<R>(
        &mut self,
        run: impl FnOnce(&dyn ZoneSource, &mut Meter, &mut Vec<Diagnostic>) -> R,
    ) -> R {
        let source = CoverageObserver {
            source: ZoneAdapter::new(self.engine.zone_database()),
            unasserted: Cell::new(false),
        };
        let result = self
            .meter
            .with_live_scope(|meter| run(&source, meter, &mut self.recurrence_diagnostics));
        let code = DiagnosticCode::TimeZoneCoverageUnasserted;
        if source.unasserted.get()
            && !self
                .workflow_issues
                .iter()
                .any(|issue| issue.code().as_str() == code.as_str())
        {
            self.workflow_issues
                .push(Issue::from_diagnostic(Diagnostic::new(
                    code,
                    Severity::Note,
                    Location::NOWHERE,
                )));
        }
        result
    }

    /// Strictly parse and validate one calendar.
    pub fn parse(&mut self, bytes: &[u8]) -> Result<Calendar, Error> {
        parse_calendar(bytes, self.engine.policy, &mut self.meter)
    }

    /// Losslessly retain an input under this session's aggregate budget.
    pub fn import(&mut self, bytes: &[u8]) -> Result<Import, Error> {
        Import::read_with_policy(bytes, self.engine.policy, &mut self.meter)
    }

    /// Promote an import only when strict validation succeeds.
    pub fn validate(&mut self, import: &Import) -> Result<Calendar, Error> {
        self.parse(import.as_bytes())
    }

    /// Strictly read one CalDAV calendar-query under this session's aggregate budget.
    pub fn query(&mut self, bytes: &[u8]) -> Result<Query, Error> {
        Query::parse_with_policy(bytes, self.engine.policy, &mut self.meter)
    }
}

struct CoverageObserver<'a> {
    source: ZoneAdapter<'a>,
    unasserted: Cell<bool>,
}

impl CoverageObserver<'_> {
    fn observe(&self, basis: AnswerBasis) {
        if matches!(basis, AnswerBasis::CoverageUnasserted) {
            self.unasserted.set(true);
        }
    }
}

impl ZoneSource for CoverageObserver<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        let answer = self.source.resolve(tzid, local)?;
        self.observe(answer.basis);
        Some(answer)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        let answer = self.source.offset_at(tzid, instant)?;
        self.observe(answer.basis);
        Some(answer)
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.source.recognizes(tzid)
    }
}
