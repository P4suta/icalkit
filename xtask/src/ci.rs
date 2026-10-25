// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

const COMMIT_BYTES: usize = 40;

pub(super) const REQUIRED_PROOFS: &str = r#"["production::proofs::candidate_identity_is_complete_and_exact","production::proofs::phase_inventory_is_complete_and_ordered","production::proofs::summaries_require_every_job_to_succeed"]"#;
pub(super) const REJECTING_PROBE: &str =
    "production::proofs::counterexample_skipped_candidate_is_successful";
pub(super) const ALLOCATION_REQUIRED_PROOFS: &str =
    r#"["production::proofs::released_count_is_saturating"]"#;
pub(super) const ALLOCATION_REJECTING_PROBE: &str =
    "production::proofs::counterexample_release_underflows";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CiJob {
    Ordinary,
    Platform,
    Features,
    Portability,
    PublicApi,
    Msrv,
    Proof,
}

pub(super) const CANDIDATE_JOBS: [CiJob; 7] = [
    CiJob::Ordinary,
    CiJob::Platform,
    CiJob::Features,
    CiJob::Portability,
    CiJob::PublicApi,
    CiJob::Msrv,
    CiJob::Proof,
];

impl CiJob {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "ordinary" => Some(Self::Ordinary),
            "platform" => Some(Self::Platform),
            "features" => Some(Self::Features),
            "portability" => Some(Self::Portability),
            "public-api" => Some(Self::PublicApi),
            "msrv" => Some(Self::Msrv),
            "proof" => Some(Self::Proof),
            _ => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Platform => "platform",
            Self::Features => "features",
            Self::Portability => "portability",
            Self::PublicApi => "public-api",
            Self::Msrv => "msrv",
            Self::Proof => "proof",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CiPhase {
    Ordinary,
    Candidate,
}

impl CiPhase {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "ordinary" => Some(Self::Ordinary),
            "candidate" => Some(Self::Candidate),
            _ => None,
        }
    }

    pub(super) fn jobs(self) -> &'static [CiJob] {
        match self {
            Self::Ordinary => &[CiJob::Ordinary],
            Self::Candidate => &CANDIDATE_JOBS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JobState {
    Success,
    Failure,
    Cancelled,
    Skipped,
}

impl JobState {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "failure" => Some(Self::Failure),
            "cancelled" => Some(Self::Cancelled),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ObservedJob {
    pub(super) job: CiJob,
    pub(super) state: JobState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CiResults {
    Ordinary(JobState),
    Candidate([JobState; CANDIDATE_JOBS.len()]),
}

impl CiResults {
    pub(super) fn from_observations(phase: CiPhase, observed: &[ObservedJob]) -> Option<Self> {
        if !observed
            .iter()
            .map(|entry| entry.job)
            .eq(phase.jobs().iter().copied())
        {
            return None;
        }
        match (phase, observed) {
            (CiPhase::Ordinary, [job]) => Some(Self::Ordinary(job.state)),
            (
                CiPhase::Candidate,
                [
                    ordinary,
                    platform,
                    features,
                    portability,
                    public_api,
                    msrv,
                    proof,
                ],
            ) => Some(Self::Candidate([
                ordinary.state,
                platform.state,
                features.state,
                portability.state,
                public_api.state,
                msrv.state,
                proof.state,
            ])),
            _ => None,
        }
    }

    pub(super) fn succeeded(self) -> bool {
        match self {
            Self::Ordinary(state) => state == JobState::Success,
            Self::Candidate(states) => states.iter().all(|state| *state == JobState::Success),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CandidateRevision([u8; COMMIT_BYTES]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TrackedSource {
    Clean,
    Changed,
}

impl TrackedSource {
    pub(super) fn from_differences(worktree_changed: bool, index_changed: bool) -> Self {
        if worktree_changed || index_changed {
            Self::Changed
        } else {
            Self::Clean
        }
    }
}

impl CandidateRevision {
    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::from_bytes(value.as_bytes().try_into().ok()?)
    }

    fn from_bytes(bytes: [u8; COMMIT_BYTES]) -> Option<Self> {
        bytes
            .iter()
            .all(u8::is_ascii_hexdigit)
            .then_some(Self(bytes))
    }

    pub(super) fn matches(self, actual: Self) -> bool {
        self == actual
    }

    pub(super) fn qualifies(self, actual: Self, source: TrackedSource) -> bool {
        self.matches(actual) && source == TrackedSource::Clean
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn state(value: u8) -> JobState {
        match value & 3 {
            0 => JobState::Success,
            1 => JobState::Failure,
            2 => JobState::Cancelled,
            _ => JobState::Skipped,
        }
    }

    fn job(value: u8) -> CiJob {
        match value % 7 {
            0 => CiJob::Ordinary,
            1 => CiJob::Platform,
            2 => CiJob::Features,
            3 => CiJob::Portability,
            4 => CiJob::PublicApi,
            5 => CiJob::Msrv,
            _ => CiJob::Proof,
        }
    }

    #[kani::proof]
    #[kani::unwind(41)]
    fn candidate_identity_is_complete_and_exact() {
        let first: [u8; COMMIT_BYTES] = kani::any();
        let second: [u8; COMMIT_BYTES] = kani::any();
        let parsed = CandidateRevision::from_bytes(first);
        let valid = first
            .iter()
            .all(|byte| matches!(*byte, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'));
        assert_eq!(parsed.is_some(), valid);
        let accepted = parsed
            .zip(CandidateRevision::from_bytes(second))
            .is_some_and(|(a, b)| a.matches(b));
        assert_eq!(accepted, valid && first == second);
        let worktree_changed: bool = kani::any();
        let index_changed: bool = kani::any();
        let source = TrackedSource::from_differences(worktree_changed, index_changed);
        let qualified = parsed
            .zip(CandidateRevision::from_bytes(second))
            .is_some_and(|(a, b)| a.qualifies(b, source));
        assert_eq!(
            qualified,
            valid && first == second && !worktree_changed && !index_changed
        );
        kani::cover!(qualified);
        kani::cover!(!qualified);
    }

    #[kani::proof]
    #[kani::unwind(9)]
    fn phase_inventory_is_complete_and_ordered() {
        let selectors: [u8; 8] = kani::any();
        let statuses: [u8; 8] = kani::any();
        let count: usize = kani::any();
        kani::assume(count <= 8);
        let phase = if kani::any() {
            CiPhase::Ordinary
        } else {
            CiPhase::Candidate
        };
        let entries = std::array::from_fn::<_, 8, _>(|index| ObservedJob {
            job: job(selectors[index]),
            state: state(statuses[index]),
        });
        let expected = phase.jobs();
        let ordered = count == expected.len()
            && entries
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.job == *expected);
        let accepted = CiResults::from_observations(phase, &entries[..count]).is_some();
        assert_eq!(accepted, ordered);
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn summaries_require_every_job_to_succeed() {
        let selectors: [u8; CANDIDATE_JOBS.len()] = kani::any();
        let states = selectors.map(state);
        let accepted = CiResults::Candidate(states).succeeded();
        assert_eq!(accepted, selectors.iter().all(|value| value & 3 == 0));
        let ordinary = state(kani::any());
        assert_eq!(
            CiResults::Ordinary(ordinary).succeeded(),
            ordinary == JobState::Success
        );
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn counterexample_skipped_candidate_is_successful() {
        assert!(CiResults::Candidate([JobState::Skipped; CANDIDATE_JOBS.len()]).succeeded());
    }
}
