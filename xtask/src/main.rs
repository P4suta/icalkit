// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Repository maintenance commands for the icalkit workspace.
//!
//! Run `cargo run -p xtask -- <task>` or use the matching `just` recipe.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The `Justfile` uses the same crate list for target builds.
const CORE_CRATES: &[&str] = &["icalkit"];

const PUBLIC_CRATES: &[&str] = &["icalkit"];

const PRIVATE_IMPLEMENTATION: &[&str] = &[];

const RETIRED_IMPLEMENTATION: &[&str] = &[
    "ical-core",
    "ical-dav",
    "ical-itip",
    "ical-query",
    "ical-recur",
    "ical-tz",
];

/// Each pair grants one core crate access to a specific external package.
const ALLOWED_EXTERNAL_DEPENDENCIES: &[(&str, &str)] = &[
    ("icalkit", "jiff"),
    ("icalkit", "xmlparser"),
    ("icalkit", "icu_calendar"),
];

/// Private tools can use `std` outside the production dependency graph.
const PRIVATE_TOOLS: &[&str] = &["icalkit-conformance"];

const MEMBER_ROOTS: [&str; 2] = ["crates", "gates"];

const JUSTFILE: &str = "Justfile";

const GRAMMAR_ROOT: &str = "crates/icalkit/src/internal/core/grammar";

const GRAMMAR_OWNER: &str = "icalkit";

/// Each member compiles the production sources for one layer in isolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayeringGate {
    member: &'static str,
    package: &'static str,
    root: &'static str,
    owner: &'static str,
}

const LAYERING_GATES: [LayeringGate; 2] = [
    LayeringGate {
        member: "gates/grammar-layering",
        package: "ical-grammar-layering",
        root: GRAMMAR_ROOT,
        owner: GRAMMAR_OWNER,
    },
    LayeringGate {
        member: "gates/xml-layering",
        package: "ical-xml-layering",
        root: XML_ROOT,
        owner: XML_OWNER,
    },
];

const XML_ROOT: &str = "crates/icalkit/src/internal/dav/xml";

const XML_OWNER: &str = "icalkit";

/// A wildcard inside the defining crate can hide a new variant of this enum.
const GUARDED_ENUM: &str = "Token";

const ROOT_MANIFEST: &str = "Cargo.toml";

const RELEASE_CONFIG: &str = "release-plz.toml";

const CHANGELOG_OWNER: &str = "icalkit";

const GOLDEN_LIST: &str = "docs/diagnostic-codes.md";

const DIAGNOSTIC_SOURCE: &str = "crates/icalkit/src/internal/core/grammar/report.rs";

const PUBLIC_API_DEFAULT: &str = "api/icalkit.default.txt";

const PUBLIC_API_NO_DEFAULT: &str = "api/icalkit.no-default.txt";
const PUBLIC_API_TOOLCHAIN: &str = "nightly-2026-07-02";
const CI_POLICY_ACTION: &str =
    "P4suta/project-template/.github/actions/ci-policy@ee6265bd944c53097191f97dcb7a6ac678794b43";

const GOLDEN_COLUMNS: [&str; 4] = ["code", "meaning", "channel", "milestone"];

mod candidate;
mod ci;
mod development;
#[cfg(test)]
mod hook_scope;
#[cfg(test)]
mod nextest_output;
mod package;
mod public_api;
use ci::{
    ALLOCATION_REJECTING_PROBE, ALLOCATION_REQUIRED_PROOFS, CANDIDATE_JOBS, CandidateRevision,
    CiJob, CiPhase, CiResults, JobState, ObservedJob, REJECTING_PROBE, REQUIRED_PROOFS,
};

fn ci_results(phase: CiPhase, arguments: &[String]) -> Option<CiResults> {
    let (pairs, remainder) = arguments.as_chunks::<2>();
    if !remainder.is_empty() || pairs.len() != phase.jobs().len() {
        return None;
    }
    let observations: Option<Vec<_>> = pairs
        .iter()
        .map(|pair| {
            Some(ObservedJob {
                job: CiJob::parse(pair.first()?.as_str())?,
                state: JobState::parse(pair.get(1)?.as_str())?,
            })
        })
        .collect();
    CiResults::from_observations(phase, &observations?)
}

fn candidate_revision(arguments: &[String]) -> io::Result<Vec<String>> {
    let [requested] = arguments else {
        return Ok(vec![
            "candidate-revision requires one complete commit ID".to_owned(),
        ]);
    };
    let Some(requested) = CandidateRevision::parse(requested) else {
        return Ok(vec![
            "candidate-revision requires a 40-digit hexadecimal commit ID".to_owned(),
        ]);
    };
    candidate_checkout(&workspace_root()?, requested)
}

fn candidate_checkout(workspace: &Path, requested: CandidateRevision) -> io::Result<Vec<String>> {
    candidate::inspect(workspace, requested, &std::env::temp_dir())
}

fn ci_summary(arguments: &[String]) -> Vec<String> {
    let Some((phase, results)) = arguments.split_first() else {
        return vec!["ci-summary needs a phase and its complete job inventory".to_owned()];
    };
    let Some(phase) = CiPhase::parse(phase) else {
        return vec!["ci-summary received an unknown phase".to_owned()];
    };
    match ci_results(phase, results) {
        Some(results) if results.succeeded() => Vec::new(),
        _ => vec!["CI jobs are missing, unexpected, malformed, or unsuccessful".to_owned()],
    }
}

/// The registry names the roadmap milestone for each diagnostic's emitter.
const MILESTONES: &[&str] = &["M0", "M1", "M2", "M3", "M4", "M5"];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("purity") => report("purity", collect_purity_violations()),
        Some("architecture") => report("architecture", collect_architecture_violations()),
        Some("codes") => report("codes", collect_codes_violations()),
        Some("public-api") => report("public-api", collect_public_api_violations()),
        Some("repository") => report("repository", collect_repository_violations()),
        Some("package-check") => report(
            "package-check",
            workspace_root().and_then(|root| {
                package::check(&root, &std::env::args().skip(2).collect::<Vec<_>>())
            }),
        ),
        Some("candidate-revision") => report(
            "candidate-revision",
            candidate_revision(&std::env::args().skip(2).collect::<Vec<_>>()),
        ),
        Some("ci-summary") => report(
            "ci-summary",
            Ok(ci_summary(&std::env::args().skip(2).collect::<Vec<_>>())),
        ),
        Some(task) => {
            eprintln!("xtask: unknown task `{task}`");
            print_usage();
            ExitCode::FAILURE
        },
        None => {
            print_usage();
            ExitCode::FAILURE
        },
    }
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -p xtask -- <purity|architecture|codes|public-api|repository|package-check [--allow-dirty]|candidate-revision COMMIT|ci-summary PHASE JOB RESULT ...>"
    );
}

const REQUIRED_REPOSITORY_FILES: &[&str] = &[
    "README.md",
    "docs/README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "SUPPORT.md",
    "LICENSE",
    "CODE_OF_CONDUCT.md",
    "docs/repository-policy.md",
    ".github/CODEOWNERS",
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".github/ISSUE_TEMPLATE/bug_report.yml",
    ".github/ISSUE_TEMPLATE/interoperability_disagreement.yml",
    ".github/ISSUE_TEMPLATE/feature_request.yml",
    ".github/ISSUE_TEMPLATE/documentation.md",
    ".github/ISSUE_TEMPLATE/config.yml",
    ".github/workflows/ci.yml",
    ".github/workflows/candidate.yml",
    ".github/workflows/codeql.yml",
    "Justfile",
    "lefthook.yml",
];

struct RepositoryDocuments<'a> {
    readme: &'a str,
    contributing: &'a str,
    security: &'a str,
    support: &'a str,
    license: &'a str,
    policy: &'a str,
    feature_request: &'a str,
    issue_config: &'a str,
    ci: &'a str,
    codeql: &'a str,
    codeowners: &'a str,
}

/// Remote hosting settings have a separate audit in `docs/repository-policy.md`.
fn collect_repository_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    Ok(collect_repository_violations_at(&workspace))
}

fn collect_repository_violations_at(workspace: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    let mut sources = Vec::new();

    for path in REQUIRED_REPOSITORY_FILES {
        match fs::read_to_string(workspace.join(path)) {
            Ok(source) => {
                if source.trim().is_empty() {
                    violations.push(format!("{path}: required repository file is empty"));
                }
                sources.push((*path, source));
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                violations.push(format!("{path}: required repository file is missing"));
                sources.push((*path, String::new()));
            },
            Err(error) => {
                violations.push(format!(
                    "{path}: required repository file could not be read: {error}"
                ));
                sources.push((*path, String::new()));
            },
        }
    }

    let source = |path: &str| {
        sources
            .iter()
            .find_map(|(candidate, source)| (*candidate == path).then_some(source.as_str()))
            .unwrap_or("")
    };
    violations.extend(repository_documentation_violations(&RepositoryDocuments {
        readme: source("docs/README.md"),
        contributing: source("CONTRIBUTING.md"),
        security: source("SECURITY.md"),
        support: source("SUPPORT.md"),
        license: source("LICENSE"),
        policy: source("docs/repository-policy.md"),
        feature_request: source(".github/ISSUE_TEMPLATE/feature_request.yml"),
        issue_config: source(".github/ISSUE_TEMPLATE/config.yml"),
        ci: source(".github/workflows/ci.yml"),
        codeql: source(".github/workflows/codeql.yml"),
        codeowners: source(".github/CODEOWNERS"),
    }));
    violations.extend(ci_source_violations(
        source(".github/workflows/ci.yml"),
        source(".github/workflows/candidate.yml"),
        source(JUSTFILE),
        source("lefthook.yml"),
    ));
    match development::collect(workspace) {
        Ok(findings) => violations.extend(findings),
        Err(error) => violations.push(format!(
            "development configuration could not be checked: {error}"
        )),
    }
    violations
}

fn repository_documentation_violations(documents: &RepositoryDocuments<'_>) -> Vec<String> {
    let mut violations = Vec::new();

    require_markers(
        "docs/README.md",
        documents.readme,
        &[
            "actions/workflows/ci.yml/badge.svg?branch=main",
            "actions/workflows/codeql.yml/badge.svg?branch=main",
            "[Contributing](../CONTRIBUTING.md)",
            "[Support](../SUPPORT.md)",
            "[Security policy](../SECURITY.md)",
            "[repository policy](repository-policy.md)",
        ],
        &mut violations,
    );
    forbid_markers(
        "CONTRIBUTING.md",
        documents.contributing,
        &[
            "`ical-core`",
            "`ical-recur`",
            "`ical-tz`",
            "`ical-itip`",
            "`ical-dav`",
        ],
        &mut violations,
    );
    forbid_markers(
        "SECURITY.md",
        documents.security,
        &["ical_dav::", "ical_itip::", "`ical-dav`", "`ical-itip`"],
        &mut violations,
    );
    require_markers(
        "SECURITY.md",
        documents.security,
        &["Until the first release", "security/advisories/new"],
        &mut violations,
    );
    require_markers(
        "SUPPORT.md",
        documents.support,
        &[
            "GitHub Discussions",
            "public GitHub issue",
            "security/advisories/new",
            "Do not include confidential",
        ],
        &mut violations,
    );
    require_markers(
        "LICENSE",
        documents.license,
        &[
            "MIT OR Apache-2.0",
            "LICENSES/MIT.txt",
            "LICENSES/Apache-2.0.txt",
        ],
        &mut violations,
    );
    require_markers(
        "docs/repository-policy.md",
        documents.policy,
        &[
            "`main`",
            "`required`",
            "squash",
            "draft pull request",
            "`0.0.0`",
            "explicit release decision",
            "optional non-provider pattern and validity extensions",
            "Renovate",
        ],
        &mut violations,
    );
    forbid_markers(
        "docs/repository-policy.md",
        documents.policy,
        &["Dependabot"],
        &mut violations,
    );
    violations.extend(repository_automation_violations(documents));

    violations
}

fn repository_automation_violations(documents: &RepositoryDocuments<'_>) -> Vec<String> {
    let mut violations = Vec::new();

    require_markers(
        ".github/ISSUE_TEMPLATE/feature_request.yml",
        documents.feature_request,
        &[
            "name: Design proposal",
            "labels: [\"enhancement\"]",
            "SECURITY.md",
        ],
        &mut violations,
    );
    require_markers(
        ".github/ISSUE_TEMPLATE/config.yml",
        documents.issue_config,
        &[
            "blank_issues_enabled: false",
            "https://github.com/P4suta/icalkit/discussions",
            "security/advisories/new",
        ],
        &mut violations,
    );
    require_markers(
        ".github/workflows/ci.yml",
        documents.ci,
        &["mise x -- just check", "ci-summary ordinary"],
        &mut violations,
    );
    require_markers(
        ".github/workflows/codeql.yml",
        documents.codeql,
        &["security-extended", "security-events: write"],
        &mut violations,
    );
    forbid_markers(
        ".github/workflows/ci.yml",
        documents.ci,
        &["Dependabot"],
        &mut violations,
    );
    require_markers(
        ".github/CODEOWNERS",
        documents.codeowners,
        &["/SECURITY.md @P4suta", "/docs/repository-policy.md @P4suta"],
        &mut violations,
    );

    violations
}

fn require_markers(path: &str, source: &str, markers: &[&str], violations: &mut Vec<String>) {
    for marker in markers {
        if !source.contains(marker) {
            violations.push(format!(
                "{path}: missing required repository marker `{marker}`"
            ));
        }
    }
}

fn forbid_markers(path: &str, source: &str, markers: &[&str], violations: &mut Vec<String>) {
    for marker in markers {
        if source.contains(marker) {
            violations.push(format!(
                "{path}: contains retired repository marker `{marker}`"
            ));
        }
    }
}

fn collect_public_api_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let profiles = [
        ("default", PUBLIC_API_DEFAULT, false),
        ("no-default", PUBLIC_API_NO_DEFAULT, true),
    ];
    let mut violations = Vec::new();
    for (profile, snapshot, no_default_features) in profiles {
        let expected = normalize_api(&fs::read_to_string(workspace.join(snapshot))?);
        let generated = generate_public_api(&workspace, no_default_features)?;
        violations.extend(snapshot_violations(profile, &expected, &generated));
    }
    Ok(violations)
}

fn generate_public_api(workspace: &Path, no_default_features: bool) -> io::Result<String> {
    let output = public_api::command(workspace, no_default_features)?.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "cargo-public-api failed for {} features: {}",
            if no_default_features {
                "no default"
            } else {
                "default"
            },
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8(output.stdout).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cargo-public-api emitted non-UTF-8 output: {error}"),
        )
    })?;
    Ok(normalize_api(&text))
}

fn normalize_api(text: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    format!("{}\n", unix.trim_end_matches(['\r', '\n']))
}

fn snapshot_violations(profile: &str, expected: &str, generated: &str) -> Vec<String> {
    if expected == generated {
        return Vec::new();
    }
    let mut expected_lines = expected.lines();
    let mut generated_lines = generated.lines();
    let mut line = 1_usize;
    loop {
        let expected_line = expected_lines.next();
        let generated_line = generated_lines.next();
        if expected_line != generated_line {
            return vec![format!(
                "api/icalkit.{profile}.txt: public API differs at line {line}: expected `{expected}`, generated `{generated}`",
                expected = expected_line.unwrap_or("<end of snapshot>"),
                generated = generated_line.unwrap_or("<end of generated API>"),
            )];
        }
        if expected_line.is_none() {
            return Vec::new();
        }
        line = line.saturating_add(1);
    }
}

fn collect_architecture_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let mut violations = release_config_violations(&workspace)?;
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let facade = fs::read_to_string(workspace.join("crates/icalkit/Cargo.toml"))?;
    let readme = fs::read_to_string(workspace.join("docs/README.md"))?;
    let architecture = fs::read_to_string(workspace.join("ARCHITECTURE.md"))?;
    let roadmap = fs::read_to_string(workspace.join("ROADMAP.md"))?;
    let ci_workflow = fs::read_to_string(workspace.join(".github/workflows/candidate.yml"))?;
    let internal_modules =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/mod.rs"))?;
    let scheduling_chapter =
        fs::read_to_string(workspace.join("crates/icalkit-conformance/src/itip.rs"))?;
    let example_path = workspace.join("crates/icalkit/examples/golden_path.rs");
    let request_writer =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/dav/write_request.rs"))?;
    let response_writer =
        fs::read_to_string(workspace.join("crates/icalkit/src/internal/dav/write_response.rs"))?;
    violations.extend(retired_crate_violations(&root, &facade));
    violations.extend(documentation_violations(&readme, &architecture));
    violations.extend(closure_status_violations(
        &roadmap,
        &internal_modules,
        &scheduling_chapter,
    ));
    violations.extend(xml_writer_violations(&request_writer, &response_writer));
    violations.extend(public_api_workflow_violations(&ci_workflow));
    match fs::read_to_string(&example_path) {
        Ok(example) => violations.extend(basic_example_violations(&example)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => violations.push(
            "crates/icalkit/examples/golden_path.rs: missing external-consumer compile fixture"
                .to_owned(),
        ),
        Err(error) => return Err(error),
    }
    let mut features = table_keys(&facade, "features");
    features.sort();
    let mut expected = vec![
        "default".to_owned(),
        "std".to_owned(),
        "system-tz".to_owned(),
    ];
    expected.sort();
    if features != expected {
        violations.push(format!(
            "crates/icalkit/Cargo.toml: features are {features:?}, expected only {expected:?} \
             (ADR 0013)"
        ));
    }
    if !declares(&facade, "default = [\"std\", \"system-tz\"]") {
        violations.push(
            "crates/icalkit/Cargo.toml: default must enable exactly std and system-tz (ADR 0013)"
                .to_owned(),
        );
    }
    Ok(violations)
}

/// The nightly toolchain must match the one that generated the API snapshots.
fn public_api_workflow_violations(workflow: &str) -> Vec<String> {
    const PINNED_NIGHTLY: &str = "rustup toolchain install nightly-2026-07-02 --profile minimal";

    let Some(job) = workflow_job(workflow, "public-api") else {
        return vec![".github/workflows/candidate.yml: missing public-api job".to_owned()];
    };
    let mut violations = mise_installation_violations(job);
    if !job.contains(PINNED_NIGHTLY) || !job.contains("RUSTUP_TOOLCHAIN: nightly-2026-07-02") {
        violations.push(".github/workflows/candidate.yml: public-api job does not install and use nightly-2026-07-02".to_owned());
    }
    violations
}

fn mise_installation_violations(workflow: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let mut lines = workflow.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        let Some(arguments) = line.trim().strip_prefix("install_args:") else {
            continue;
        };
        let arguments = arguments
            .split_once('#')
            .map_or(arguments, |(value, _)| value)
            .trim();
        let indentation = line.len().saturating_sub(line.trim_start().len());
        while lines
            .peek()
            .is_some_and(|(_, next)| next.trim().is_empty() || next.trim_start().starts_with('#'))
        {
            let _ = lines.next();
        }
        let continued = lines.peek().is_some_and(|(_, next)| {
            next.len().saturating_sub(next.trim_start().len()) > indentation
        });
        if arguments.is_empty()
            || continued
            || arguments.contains(['\'', '"', '|', '>'])
            || arguments.starts_with(['[', '{', '&', '*', '!'])
        {
            violations.push(format!(
                "mise install_args requires an unquoted, single-line tool list at line {}",
                index.saturating_add(1)
            ));
            continue;
        }
        let tools: Vec<_> = arguments.split_whitespace().collect();
        if tools.contains(&"actionlint") && !tools.contains(&"shellcheck") {
            violations.push(format!(
                "actionlint requires shellcheck in mise install_args at line {}",
                index.saturating_add(1)
            ));
        }
        if tools.iter().any(|tool| tool.starts_with("cargo:")) && !tools.contains(&"rust") {
            violations.push(format!(
                "Cargo tools require configured Rust in mise install_args at line {}",
                index.saturating_add(1)
            ));
        }
    }
    violations
}

fn workflow_job_names(source: &str) -> Vec<&str> {
    let Some((_, jobs)) = source.split_once("\njobs:\n") else {
        return Vec::new();
    };
    jobs.lines()
        .filter_map(|line| {
            let key = line.strip_prefix("  ")?;
            (!key.starts_with(' ')).then_some(key)?.strip_suffix(':')
        })
        .collect()
}

fn workflow_job<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let (_, jobs) = source.split_once("\njobs:\n")?;
    let marker = format!("  {name}:\n");
    let (_, body) = jobs.split_once(&marker)?;
    let end = body
        .split_inclusive('\n')
        .take_while(|line| {
            !line.starts_with("  ") || line.starts_with("    ") || line.trim().is_empty()
        })
        .map(str::len)
        .sum();
    Some(&body[..end])
}

fn ci_source_violations(ci: &str, candidate: &str, justfile: &str, hooks: &str) -> Vec<String> {
    let mut violations = workflow_phase_violations(ci, candidate);
    violations.extend(summary_source_violations(ci, candidate, justfile));
    violations.extend(recipe_phase_violations(justfile, hooks));
    violations.extend(public_api_workflow_violations(candidate));
    violations.extend(mise_installation_violations(ci));
    violations.extend(mise_installation_violations(candidate));
    violations.extend(platform_test_tool_violations(candidate));
    violations
}

fn platform_test_tool_violations(candidate: &str) -> Vec<String> {
    let Some(platform) = workflow_job(candidate, "platform") else {
        return Vec::new();
    };
    let steps = platform
        .split_once("    steps:\n")
        .map_or("", |(_, steps)| steps);
    let mut tools = Vec::new();
    let mut reached_tests = false;
    for step in steps.split("\n      - ") {
        if workflow_step_runs(step, "mise x -- just test-ci") {
            reached_tests = true;
            break;
        }
        let mise = step.lines().any(|line| {
            line.trim()
                .trim_start_matches("- ")
                .starts_with("uses: jdx/mise-action@")
        });
        if mise {
            tools.extend(
                step.lines()
                    .filter_map(|line| line.trim().strip_prefix("install_args:"))
                    .map(|arguments| {
                        arguments
                            .split_once('#')
                            .map_or(arguments, |(value, _)| value)
                    })
                    .flat_map(str::split_whitespace),
            );
        }
    }
    let mut violations: Vec<_> = [
        "rust",
        "just",
        "lefthook",
        "taplo",
        "typos",
        "cargo:cargo-nextest",
    ]
    .into_iter()
    .filter(|tool| !tools.contains(tool))
    .map(|tool| format!("candidate platform tests require {tool}"))
    .collect();
    if !reached_tests {
        violations.push("candidate platform tests require a test-ci step".to_owned());
    }
    violations
}

fn workflow_step_runs(step: &str, command: &str) -> bool {
    for line in step.lines() {
        let line = line.trim().trim_start_matches("- ");
        if let Some(run) = line.strip_prefix("run:") {
            let run = run.split_once('#').map_or(run, |(value, _)| value).trim();
            return run == command
                || (matches!(run, "|" | ">" | "|-" | ">-")
                    && step.lines().any(|line| line.trim() == command));
        }
    }
    false
}

fn workflow_phase_violations(ci: &str, candidate: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for marker in [
        "\n  workflow_dispatch:",
        "CANDIDATE_REVISION: ${{ inputs.revision }}",
    ] {
        if !candidate.contains(marker) {
            violations.push(format!("candidate CI is missing {marker}"));
        }
    }
    if workflow_job_names(ci) != ["ordinary", "required"] {
        violations.push("ordinary CI must contain only ordinary and required jobs".to_owned());
    }
    let mut candidate_jobs: Vec<_> = CANDIDATE_JOBS.iter().map(|job| job.label()).collect();
    candidate_jobs.push("candidate-required");
    if workflow_job_names(candidate) != candidate_jobs {
        violations.push("candidate CI job inventory differs from its complete summary".to_owned());
    }
    for (source, phase) in [(ci, "ordinary"), (candidate, "candidate")] {
        if source.contains("\n  push:")
            || source.contains("\n  schedule:")
            || (phase == "candidate"
                && (source.contains("\n  pull_request:") || source.contains("\n  merge_group:")))
        {
            violations.push(format!("{phase} CI has an unexpected automatic trigger"));
        }
    }
    for marker in [
        "\n  pull_request:",
        "\n  merge_group:",
        "\n  workflow_dispatch:",
    ] {
        if !ci.contains(marker) {
            violations.push(format!("ordinary CI is missing {marker}"));
        }
    }
    for (name, command) in [
        ("ordinary", "check"),
        ("ordinary", "package-check"),
        ("platform", "test-ci"),
        ("features", "feature-matrix"),
        ("portability", "no-std wasm"),
        ("public-api", "public-api"),
        ("msrv", "msrv"),
    ] {
        let body = workflow_job(candidate, name).unwrap_or("");
        for marker in [
            "ref: ${{ inputs.revision }}".to_owned(),
            "candidate-revision \"$CANDIDATE_REVISION\"".to_owned(),
            format!("run: mise x -- just {command}"),
        ] {
            if !body.contains(&marker) {
                violations.push(format!("candidate {name} is missing {marker}"));
            }
        }
        violations.extend(candidate_identity_violations(
            body,
            &format!("run: mise x -- just {command}"),
            name,
        ));
    }
    let package = workflow_job(candidate, "ordinary").unwrap_or("");
    if package.lines().any(|line| {
        line.trim()
            .trim_start_matches("- ")
            .strip_prefix("continue-on-error:")
            .is_some_and(|value| value.split('#').next().map(str::trim) != Some("false"))
    }) {
        violations.push("candidate ordinary checks must fail when a step fails".to_owned());
    }
    for marker in [
        "if: ${{ inputs.release_version != '' }}",
        "REF_PROTECTED: ${{ github.ref_protected }}",
        "test \"$GITHUB_REF\" = refs/heads/main",
        "test \"$REF_PROTECTED\" = true",
        "git merge-base --is-ancestor \"$CANDIDATE_REVISION\" refs/remotes/origin/main",
        "ICALKIT_RELEASE_VERSION: ${{ inputs.release_version }}",
    ] {
        if !package.lines().any(|line| line.trim() == marker) {
            violations.push(format!("release qualification is missing {marker}"));
        }
    }
    let platform = workflow_job(candidate, "platform").unwrap_or("");
    if !platform
        .lines()
        .any(|line| line.trim() == "os: [windows-2025, macos-15]")
    {
        violations.push("candidate platform tests must cover Windows and macOS".to_owned());
    }
    violations
}

fn candidate_identity_violations(body: &str, command: &str, job: &str) -> Vec<String> {
    const IDENTITY: &str = "candidate-revision \"$CANDIDATE_REVISION\"";
    const POST: &str = "      - name: Recheck candidate source\n        if: always()\n        run: mise x -- cargo run --quiet --locked -p xtask -- candidate-revision \"$CANDIDATE_REVISION\"";
    let bracketed = body
        .find(IDENTITY)
        .zip(body.rfind(IDENTITY))
        .zip(body.find(command))
        .is_some_and(|((before, after), check)| before < check && check < after);
    if body.matches(IDENTITY).count() == 2 && bracketed && body.trim_end().ends_with(POST) {
        Vec::new()
    } else {
        vec![format!(
            "candidate {job} must verify its source before and after its checks"
        )]
    }
}

fn summary_source_violations(ci: &str, candidate: &str, justfile: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let ordinary = workflow_job(ci, "ordinary").unwrap_or("");
    if !ordinary.contains("run: mise x -- just check") {
        violations.push("ordinary CI does not run the project ordinary command".to_owned());
    }
    let required = workflow_job(ci, "required").unwrap_or("");
    for marker in [
        "if: always()",
        "needs: [ordinary]",
        "ORDINARY_RESULT: ${{ needs.ordinary.result }}",
        "ci-summary ordinary ordinary \"$ORDINARY_RESULT\"",
    ] {
        if !required.contains(marker) {
            violations.push(format!("ordinary summary is missing {marker}"));
        }
    }
    let summary = workflow_job(candidate, "candidate-required").unwrap_or("");
    violations.extend(candidate_identity_violations(
        summary,
        "ci-summary candidate",
        "candidate-required",
    ));
    for marker in [
        "if: always()",
        "needs: [ordinary, platform, features, portability, public-api, msrv, proof]",
        "candidate-revision \"$CANDIDATE_REVISION\"",
        "ci-summary candidate",
    ] {
        if !summary.contains(marker) {
            violations.push(format!("candidate summary is missing {marker}"));
        }
    }
    for (job, variable) in [
        ("ordinary", "ORDINARY"),
        ("platform", "PLATFORM"),
        ("features", "FEATURES"),
        ("portability", "PORTABILITY"),
        ("public-api", "PUBLIC_API"),
        ("msrv", "MSRV"),
        ("proof", "PROOF"),
    ] {
        for marker in [
            format!("{variable}_RESULT: ${{{{ needs.{job}.result }}}}"),
            format!("{job} \"${variable}_RESULT\""),
        ] {
            if !summary.contains(&marker) {
                violations.push(format!("candidate summary is missing {marker}"));
            }
        }
    }
    violations.extend(proof_source_violations(candidate, justfile));
    violations
}

fn proof_source_violations(candidate: &str, justfile: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let proof = workflow_job(candidate, "proof").unwrap_or("");
    for marker in [
        "ref: ${{ inputs.revision }}",
        "candidate-revision \"$CANDIDATE_REVISION\"",
        "run: mise x -- just prove-ci",
    ] {
        if !proof.contains(marker) {
            violations.push(format!("candidate proof is missing {marker}"));
        }
    }
    violations.extend(candidate_identity_violations(
        proof,
        "run: mise x -- just prove-ci",
        "proof",
    ));
    let acquisition = proof
        .find(CI_POLICY_ACTION)
        .zip(proof.find("run: mise x -- just prove-ci"));
    if acquisition.is_none_or(|(setup, check)| setup >= check) {
        violations.push(
            "candidate proof must acquire the immutable shared policy helper before verification"
                .to_owned(),
        );
    }
    let commands: Vec<_> =
        justfile
            .split_once("\nprove-ci:\n")
            .map_or_else(Vec::new, |(_, body)| {
                body.lines()
                    .take_while(|line| {
                        line.starts_with(' ') || line.starts_with('#') || line.trim().is_empty()
                    })
                    .map(str::trim)
                    .collect()
            });
    for (source, inventory, counterexample) in [
        ("xtask/src/ci.rs", REQUIRED_PROOFS, REJECTING_PROBE),
        (
            "crates/icalkit-conformance/src/bin/allocation_gate/arithmetic.rs",
            ALLOCATION_REQUIRED_PROOFS,
            ALLOCATION_REJECTING_PROBE,
        ),
    ] {
        let command = format!(
            "ci-policy prove-source --source {source} --require-json '{inventory}' --counterexample {counterexample}"
        );
        if !commands.contains(&command.as_str()) {
            violations.push(format!("source proof recipe is missing {command}"));
        }
    }
    violations
}

fn recipe_phase_violations(justfile: &str, hooks: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let ordinary_recipe = justfile
        .lines()
        .find(|line| line.starts_with("check:"))
        .unwrap_or("");
    for command in [
        "fmt-check",
        "toml-check",
        "typos",
        "lint",
        "test-ci",
        "allocation",
        "doc",
        "purity",
        "architecture",
        "repository",
        "codes",
        "deny",
        "shear",
        "reuse",
        "actionlint",
        "zizmor",
    ] {
        if !ordinary_recipe
            .split_whitespace()
            .any(|word| word == command)
        {
            violations.push(format!("ordinary recipe is missing {command}"));
        }
    }
    for command in [
        "feature-matrix",
        "no-std",
        "wasm",
        "public-api",
        "msrv",
        "package-check",
    ] {
        if ordinary_recipe
            .split_whitespace()
            .any(|word| word == command)
        {
            violations.push(format!("ordinary recipe invokes candidate-only {command}"));
        }
        let full = justfile
            .lines()
            .find(|line| line.starts_with("ci:"))
            .unwrap_or("");
        if !full.split_whitespace().any(|word| word == command) {
            violations.push(format!("complete local qualification is missing {command}"));
        }
    }
    let push = hooks
        .split_once("\npre-push:\n")
        .and_then(|(_, after)| after.split_once("\nlocal-ci:\n"))
        .map_or("", |(push, _)| push);
    if !push.contains("run: mise exec -- just check") || push.contains("just ci") {
        violations.push("pre-push must run only the ordinary project gate".to_owned());
    }
    violations.extend(candidate_recipe_violations(justfile, hooks));
    violations
}

fn candidate_recipe_violations(justfile: &str, hooks: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let commit = hooks
        .split_once("\npre-commit:\n")
        .and_then(|(_, after)| after.split_once("\npre-push:\n"))
        .map_or("", |(commit, _)| commit);
    for candidate in [
        "ci",
        "public-api",
        "feature-matrix",
        "no-std",
        "wasm",
        "msrv",
        "package-check",
        "prove-ci",
    ] {
        if commit.contains(&format!("just {candidate}")) {
            violations.push(format!("pre-commit invokes candidate-only {candidate}"));
        }
    }
    let msrv: Vec<_> = justfile
        .split_once("\nmsrv:\n")
        .map_or_else(Vec::new, |(_, body)| {
            body.lines()
                .take_while(|line| {
                    line.starts_with(' ') || line.starts_with('#') || line.trim().is_empty()
                })
                .map(str::trim)
                .collect()
        });
    for path in [
        "crates/icalkit",
        "crates/icalkit-conformance",
        "gates/grammar-layering",
        "gates/xml-layering",
        "xtask",
    ] {
        let command = format!("cargo msrv verify --path {path}");
        if !msrv.contains(&command.as_str()) {
            violations.push(format!("candidate MSRV recipe is missing {path}"));
        }
    }
    violations
}

fn closure_status_violations(roadmap: &str, internal: &str, scheduling: &str) -> Vec<String> {
    const STATUS_HEADER: &str = "## Maintained implementation";
    const RETIRED_ROADMAP_PROSE: &[&str] = &[
        "still owed: a hostile input of 200,000",
        "which it currently is not",
        "no outside XML crate may be added",
        "remaining method-specific range behavior is still work",
        "One piece of internal debt is named rather than left",
    ];

    let mut violations = Vec::new();
    if !roadmap.contains(STATUS_HEADER) {
        violations.push(format!(
            "ROADMAP.md: missing maintained implementation marker `{STATUS_HEADER}`"
        ));
    }
    for stale in RETIRED_ROADMAP_PROSE {
        if roadmap.contains(stale) {
            violations.push(format!(
                "ROADMAP.md: completed work remains described as current debt: `{stale}`"
            ));
        }
    }
    if internal.contains("including units not yet reached") {
        violations.push(
            "crates/icalkit/src/internal/mod.rs: migrated query code is still described as \
             unconnected"
                .to_owned(),
        );
    }
    for stale in [
        "# What is still owed here",
        "# Where the three big clients disagree with the table",
        "The corpus records what was observed",
    ] {
        if scheduling.contains(stale) {
            violations.push(format!(
                "crates/icalkit-conformance/src/itip.rs: stale debt or unsupported capture claim \
                 remains: `{stale}`"
            ));
        }
    }
    violations
}

fn xml_writer_violations(request: &str, response: &str) -> Vec<String> {
    const SHARED_IMPORT: &str = "use crate::internal::dav::writer::XmlWriter;";
    const FORBIDDEN_HELPERS: &[&str] = &[
        "fn write_root_declarations(",
        "fn write_name(",
        "fn open_extension(",
        "fn close_extension(",
        "fn empty_extension(",
    ];

    let mut violations = Vec::new();
    for (path, source) in [
        ("crates/icalkit/src/internal/dav/write_request.rs", request),
        (
            "crates/icalkit/src/internal/dav/write_response.rs",
            response,
        ),
    ] {
        if !source.lines().any(|line| line.trim() == SHARED_IMPORT) {
            violations.push(format!(
                "{path}: DAV body encoder does not use the shared stack-balanced XmlWriter"
            ));
        }
        for helper in FORBIDDEN_HELPERS {
            if source.contains(helper) {
                violations.push(format!(
                    "{path}: duplicate XML structural helper {helper} bypasses XmlWriter"
                ));
            }
        }
    }
    violations
}

fn basic_example_violations(example: &str) -> Vec<String> {
    const INTERNAL_VOCABULARY: &[&str] = &[
        "Token",
        "Xml",
        "XML",
        "Limits",
        "Meter",
        "Sink",
        "DiagnosticSink",
        "RFC table",
    ];
    INTERNAL_VOCABULARY
        .iter()
        .filter(|word| example.contains(*word))
        .map(|word| {
            format!(
                "crates/icalkit/examples/golden_path.rs: basic example exposes internal vocabulary `{word}` (ADR 0014)"
            )
        })
        .collect()
}

fn documentation_violations(readme: &str, architecture: &str) -> Vec<String> {
    const GOLDEN_PATH: &[&str] = &[
        "## Strict parsing",
        "## Explicit normalization",
        "## Transactional editing",
        "## Recurrence across daylight saving time",
        "## iTIP scheduling",
        "## Calendar synchronization and server workflows",
    ];
    const RETIRED_PROSE: &[&str] = &[
        "one temporary path dependency",
        "unpublished scaffolding",
        "temporary compatibility harness",
        "remaining `ical-core` package",
    ];

    let mut violations = Vec::new();
    let mut previous = None;
    for heading in GOLDEN_PATH {
        let Some(at) = readme.find(heading) else {
            violations.push(format!(
                "docs/README.md: missing golden-path heading `{heading}` (ADR 0014)"
            ));
            continue;
        };
        if previous.is_some_and(|before| at <= before) {
            violations.push(format!(
                "docs/README.md: `{heading}` is out of golden-path order (ADR 0014)"
            ));
        }
        previous = Some(at);
    }
    for stale in RETIRED_PROSE {
        if architecture.contains(stale) || readme.contains(stale) {
            violations.push(format!(
                "docs/README.md/ARCHITECTURE.md: retired migration prose `{stale}` remains (ADR 0014)"
            ));
        }
    }
    violations
}

fn retired_crate_violations(root: &str, facade: &str) -> Vec<String> {
    let workspace_members = members(root);
    let dependencies = declared_dependencies(facade);
    let mut violations = Vec::new();
    for retired in RETIRED_IMPLEMENTATION {
        let old_member = format!("crates/{retired}");
        if workspace_members.iter().any(|member| member == &old_member) {
            violations.push(format!(
                "{ROOT_MANIFEST}: retired implementation package `{retired}` remains a \
                 workspace member (ADR 0013)"
            ));
        }
        if dependencies
            .iter()
            .any(|dependency| dependency.package == *retired)
        {
            violations.push(format!(
                "crates/icalkit/Cargo.toml: facade still depends on retired implementation \
                 package `{retired}` (ADR 0013)"
            ));
        }
    }
    violations
}

fn table_keys(document: &str, table: &str) -> Vec<String> {
    let wanted = format!("[{table}]");
    let mut inside = false;
    let mut keys = Vec::new();
    for line in document.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == wanted;
        } else if inside && let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                keys.push(key.to_owned());
            }
        }
    }
    keys
}

fn report(task: &str, outcome: io::Result<Vec<String>>) -> ExitCode {
    match outcome {
        Err(error) => {
            eprintln!("xtask: {task} could not run: {error}");
            ExitCode::FAILURE
        },
        Ok(violations) if violations.is_empty() => {
            println!("{task}: ok");
            ExitCode::SUCCESS
        },
        Ok(violations) => {
            for violation in &violations {
                eprintln!("{task}: {violation}");
            }
            eprintln!(
                "{task}: {count} violation(s). See docs/adr/",
                count = violations.len()
            );
            ExitCode::FAILURE
        },
    }
}

fn collect_purity_violations() -> io::Result<Vec<String>> {
    let workspace = workspace_root()?;
    let root = workspace.join("crates");
    let mut violations = Vec::new();

    for crate_name in CORE_CRATES {
        let dir = root.join(crate_name);
        let manifest = fs::read_to_string(dir.join("Cargo.toml"))?;
        violations.extend(manifest_violations(crate_name, &manifest));

        let lib = dir.join("src").join("lib.rs");
        if !lib.is_file() {
            violations.push(format!("{crate_name}: has no src/lib.rs"));
            continue;
        }
        let source = fs::read_to_string(&lib)?;
        if !declares(&source, "#![no_std]") {
            violations.push(format!(
                "{crate_name}: src/lib.rs does not declare `#![no_std]` (ADR 0004)"
            ));
        }
        if !declares(&source, "extern crate alloc;") {
            violations.push(format!(
                "{crate_name}: src/lib.rs does not declare `extern crate alloc;`; the core \
                 crates are `no_std` and `alloc`, and the declaration is the policy (ADR 0007)"
            ));
        }
    }

    violations.extend(recipe_violations(&fs::read_to_string(
        workspace.join(JUSTFILE),
    )?));

    let mut unregistered = Vec::new();
    for member_root in MEMBER_ROOTS {
        unregistered.extend(unregistered_core_crates(&workspace.join(member_root))?);
    }
    unregistered.sort();
    violations.extend(unregistered);

    for gate in LAYERING_GATES {
        violations.extend(layer_root_violations(gate, &workspace.join(gate.root))?);
        violations.extend(layering_member_violations(&workspace, gate)?);
    }
    violations.extend(guarded_match_violations(&workspace)?);
    violations.extend(release_config_violations(&workspace)?);
    Ok(violations)
}

fn layering_member_violations(workspace: &Path, gate: LayeringGate) -> io::Result<Vec<String>> {
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let directory = workspace.join(gate.member);
    let manifest_path = directory.join("Cargo.toml");
    let lib_path = directory.join("src").join("lib.rs");
    let member = if manifest_path.is_file() && lib_path.is_file() {
        Some((
            fs::read_to_string(&manifest_path)?,
            fs::read_to_string(&lib_path)?,
        ))
    } else {
        None
    };
    Ok(layering_violations(
        gate,
        &root,
        member
            .as_ref()
            .map(|(manifest, lib)| (manifest.as_str(), lib.as_str())),
    ))
}

fn layering_violations(
    gate: LayeringGate,
    root: &str,
    member: Option<(&str, &str)>,
) -> Vec<String> {
    let LayeringGate {
        member: directory,
        package,
        root: layer,
        ..
    } = gate;
    let mut violations = Vec::new();
    if !members(root).iter().any(|name| name == directory) {
        violations.push(format!(
            "{ROOT_MANIFEST}: does not register `{directory}`; that member is what makes \
             `{layer}` a layer rather than a directory, and it is nothing once it is not built \
             (ADR 0004)"
        ));
    }

    let Some((manifest, lib)) = member else {
        violations.push(format!(
            "{directory}: has no Cargo.toml and src/lib.rs; the compile half of the layering \
             guarantee over `{layer}` is this member and nothing else (ADR 0004)"
        ));
        return violations;
    };

    for declaration in [
        format!("name = \"{package}\""),
        "publish = false".to_owned(),
        "doc = false".to_owned(),
        "test = false".to_owned(),
    ] {
        if !declares(manifest, &declaration) {
            violations.push(format!(
                "{directory}/Cargo.toml: does not declare `{declaration}`; the member is a gate \
                 rather than a crate, and without both `[lib]` switches the layer's tests and \
                 documentation are counted twice (ADR 0004)"
            ));
        }
    }

    // Rust resolves the module path from the gate's `src/lib.rs`.
    let expected = format!("#[path = \"../../../{layer}/mod.rs\"]");
    if !declares(lib, &expected) {
        violations.push(format!(
            "{directory}/src/lib.rs: does not declare `{expected}`; a gate that compiles \
             anything other than the shipped sources proves something about a copy (ADR 0004)"
        ));
    }
    violations
}

fn guarded_match_violations(workspace: &Path) -> io::Result<Vec<String>> {
    let mut violations = Vec::new();
    let mut sources = Vec::new();
    collect_rust_sources(&workspace.join("crates"), &mut sources)?;
    sources.sort();
    for path in sources {
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(match_arm_violations(&label, &fs::read_to_string(&path)?));
    }
    Ok(violations)
}

fn collect_rust_sources(root: &Path, sources: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_sources(&path, sources)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_rust_source)
        {
            sources.push(path);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenMatch {
    arms: usize,
    guarded: bool,
    wildcards: Vec<usize>,
}

/// The scanner recognizes a match when its opening brace shares a line with `match`.
fn match_arm_violations(label: &str, source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let mut open: Vec<OpenMatch> = Vec::new();
    let mut depth = 0usize;

    for (number, code) in code_lines(source) {
        while open.last().is_some_and(|block| depth < block.arms) {
            if let Some(block) = open.pop() {
                violations.extend(closed_match_violations(label, &block));
            }
        }
        let line = code.trim();
        if let Some(block) = open.last_mut()
            && depth == block.arms
        {
            let pattern = line.split("=>").next().unwrap_or(line).trim();
            block.guarded |= names_path(pattern, &format!("{GUARDED_ENUM}::"));
            if is_wildcard(pattern) {
                block.wildcards.push(number);
            }
        }

        let inner = depth
            .saturating_add(code.matches('{').count())
            .saturating_sub(code.matches('}').count());
        if inner > depth && line.contains("match ") && line.ends_with('{') {
            open.push(OpenMatch {
                arms: inner,
                guarded: false,
                wildcards: Vec::new(),
            });
        }
        depth = inner;
    }

    while let Some(block) = open.pop() {
        violations.extend(closed_match_violations(label, &block));
    }
    violations.sort();
    violations
}

fn closed_match_violations(label: &str, block: &OpenMatch) -> Vec<String> {
    if !block.guarded {
        return Vec::new();
    }
    block
        .wildcards
        .iter()
        .map(|number| {
            format!(
                "{label}:{number}: a wildcard arm over `{GUARDED_ENUM}`. The attribute that makes \
                 adding a variant a minor release binds outside this workspace and means nothing \
                 inside it, so a `_` here is a variant whose payload is dropped with no compiler \
                 error rather than forward compatibility (ADR 0004)"
            )
        })
        .collect()
}

fn is_wildcard(pattern: &str) -> bool {
    pattern == "_"
        || pattern
            .strip_prefix("_ ")
            .is_some_and(|guard| guard.trim_start().starts_with("if "))
}

fn release_config_violations(workspace: &Path) -> io::Result<Vec<String>> {
    let root = fs::read_to_string(workspace.join(ROOT_MANIFEST))?;
    let mut published = Vec::new();
    let mut private_tools = Vec::new();
    let mut violations = Vec::new();
    for member in members(&root) {
        let manifest = fs::read_to_string(workspace.join(&member).join("Cargo.toml"))?;
        match package_name(&manifest) {
            Some(name) if declares(&manifest, "publish = false") => {
                if Path::new(&member).starts_with("crates") {
                    private_tools.push(name);
                }
            },
            Some(name) => published.push(name),
            None => violations.push(format!(
                "{member}/Cargo.toml: declares no package name, so the release configuration \
                 could not be read against it"
            )),
        }
    }
    violations.extend(core_list_violations(&published));
    violations.extend(private_crate_violations(&private_tools));
    let config = fs::read_to_string(workspace.join(RELEASE_CONFIG))?;
    violations.extend(release_violations(&published, &config));
    violations.extend(proposal_config_violations(&config));
    Ok(violations)
}

fn proposal_config_violations(config: &str) -> Vec<String> {
    let (workspace, package) = config.split_once("\n[[package]]\n").unwrap_or_default();
    let mut violations = Vec::new();
    for required in [
        "release = false",
        "allow_dirty = false",
        "publish = false",
        "git_tag_enable = false",
        "git_release_enable = false",
        "pr_draft = true",
    ] {
        if !declares(workspace, required) {
            violations.push(format!("{RELEASE_CONFIG}: proposals require `{required}`"));
        }
    }
    for key in package
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
    {
        if !["name", "release", "changelog_update", "changelog_path"].contains(&key) {
            violations.push(format!(
                "{RELEASE_CONFIG}: unexpected package setting `{key}`"
            ));
        }
    }
    if !declares(package, "release = true") {
        violations.push(format!("{RELEASE_CONFIG}: icalkit must receive proposals"));
    }
    violations
}

fn core_list_violations(published: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    for name in published {
        if !PUBLIC_CRATES.contains(&name.as_str()) {
            violations.push(format!(
                "{ROOT_MANIFEST}: publishes {name}, but ADR 0013 permits only the icalkit facade"
            ));
        }
    }
    for name in PUBLIC_CRATES {
        if !published.iter().any(|member| member == *name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: does not leave {name} as the sole future public crate (ADR 0013)"
            ));
        }
    }
    violations.sort();
    violations
}

fn private_crate_violations(private: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    let expected: Vec<&str> = PRIVATE_TOOLS
        .iter()
        .chain(PRIVATE_IMPLEMENTATION)
        .copied()
        .collect();

    for name in PRIVATE_TOOLS {
        if CORE_CRATES.contains(name) || PUBLIC_CRATES.contains(name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: `{name}` is both a private tool and a published crate; those \
                 release contracts are mutually exclusive"
            ));
        }
    }
    for name in &expected {
        if !private.iter().any(|member| member == *name) {
            violations.push(format!(
                "{ROOT_MANIFEST}: private member `{name}` is missing or publishable (ADR 0013)"
            ));
        }
    }

    for name in private {
        if !expected.contains(&name.as_str()) {
            violations.push(format!(
                "{ROOT_MANIFEST}: private crate `{name}` is unclassified; add it to the explicit \
                 private implementation or tool set"
            ));
        }
    }

    violations.sort();
    violations
}

fn release_violations(published: &[String], config: &str) -> Vec<String> {
    let released = release_packages(config);
    let included = quoted_values(array_value(config, "changelog_include").unwrap_or_default());
    let mut violations = Vec::new();

    for name in published {
        match released.iter().filter(|entry| *entry == name).count() {
            1 => {},
            0 => violations.push(format!(
                "{RELEASE_CONFIG}: `{name}` is a published member with no `[[package]]` block; \
                 the crates move as one version group, and a member outside it is a member with \
                 its own release story"
            )),
            count => violations.push(format!(
                "{RELEASE_CONFIG}: `{name}` has {count} `[[package]]` blocks; the last one wins \
                 silently, which is how a setting comes to be read by nobody"
            )),
        }
        let listed = included.iter().any(|entry| entry == name);
        if name == CHANGELOG_OWNER && listed {
            violations.push(format!(
                "{RELEASE_CONFIG}: `changelog_include` names `{CHANGELOG_OWNER}`, which is the \
                 crate carrying the changelog rather than a crate folded into it"
            ));
        } else if name != CHANGELOG_OWNER && !listed {
            violations.push(format!(
                "{RELEASE_CONFIG}: `changelog_include` does not name `{name}`, so that crate's \
                 history would be missing from the one changelog this stack has"
            ));
        }
    }

    for name in released.iter().chain(&included) {
        if !published.contains(name) {
            violations.push(format!(
                "{RELEASE_CONFIG}: names `{name}`, which this workspace does not publish; a \
                 release configuration describing a crate set that no longer exists is a release \
                 nobody can run"
            ));
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

fn release_packages(config: &str) -> Vec<String> {
    let mut packages = Vec::new();
    let mut inside = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[[package]]";
        } else if inside && let Some(name) = quoted_after(line, "name") {
            packages.push(name.to_owned());
            inside = false;
        }
    }
    packages
}

fn package_name(manifest: &str) -> Option<String> {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[package]";
        } else if inside && let Some(name) = quoted_after(line, "name") {
            return Some(name.to_owned());
        }
    }
    None
}

fn quoted_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = line.split_once('=')?;
    (name.trim() == key).then(|| value.trim().trim_matches('"'))
}

fn members(manifest: &str) -> Vec<String> {
    quoted_values(array_value(manifest, "members").unwrap_or_default())
}

fn array_value<'a>(document: &'a str, key: &str) -> Option<&'a str> {
    let at = document.find(&format!("{key} = ["))?;
    let rest = document.get(at..)?;
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    rest.get(open.saturating_add(1)..close)
}

fn quoted_values(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The core target builds and the dependency check must cover the same crates.
fn recipe_violations(justfile: &str) -> Vec<String> {
    let Some(named) = recipe_core_crates(justfile) else {
        return vec![format!(
            "{JUSTFILE}: no `core_crates := \"...\"` assignment was found, so the two copies of \
             the core list were compared against nothing (ADR 0004)"
        )];
    };

    let mut violations = Vec::new();
    for crate_name in CORE_CRATES {
        if !named.iter().any(|name| name == crate_name) {
            violations.push(format!(
                "{JUSTFILE}: `core_crates` does not name `{crate_name}`, which CORE_CRATES \
                 does; `just no-std` and `just wasm` would never compile it (ADR 0004)"
            ));
        }
    }
    for name in &named {
        if !CORE_CRATES.contains(&name.as_str()) {
            violations.push(format!(
                "{JUSTFILE}: `core_crates` names `{name}`, which CORE_CRATES does not, so no \
                 rule in this gate applies to it (ADR 0004)"
            ));
        }
    }
    violations
}

fn recipe_core_crates(justfile: &str) -> Option<Vec<String>> {
    let assignment = justfile
        .lines()
        .find(|line| line.trim_start().starts_with("core_crates"))?;
    let (_, value) = assignment.split_once(":=")?;
    let named = value.trim().strip_prefix('"')?.strip_suffix('"')?;

    let mut crates = Vec::new();
    let mut expecting = false;
    for token in named.split_whitespace() {
        if expecting {
            crates.push(token.to_owned());
        }
        expecting = token == "-p";
    }
    Some(crates)
}

/// The source scan covers literal paths in the layer's flat module tree.
fn layer_root_violations(gate: LayeringGate, root: &Path) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let directory = path.is_dir();
        let source = if directory || !is_rust_source(name) {
            String::new()
        } else {
            fs::read_to_string(&path)?
        };
        entries.push(LayerEntry {
            name: name.to_owned(),
            directory,
            source,
        });
    }
    Ok(layer_violations(gate, &entries))
}

fn is_rust_source(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LayerEntry {
    name: String,
    directory: bool,
    source: String,
}

fn layer_violations(gate: LayeringGate, entries: &[LayerEntry]) -> Vec<String> {
    let layer = gate.root;
    let mut violations = Vec::new();
    let mut sources = 0usize;
    let mut root = None;
    let mut beside = Vec::new();

    for entry in entries {
        let name = &entry.name;
        if entry.directory {
            violations.push(format!(
                "{layer}/{name}: a layer's tree is flat. A subdirectory changes the depth every \
                 path in this layer is stated in, and a check that quietly stops applying is \
                 worse than no check (ADR 0004)"
            ));
            continue;
        }
        if !is_rust_source(name) {
            continue;
        }
        sources = sources.saturating_add(1);
        violations.extend(file_path_violations(gate, name, &entry.source));
        if name == "mod.rs" {
            root = Some(entry.source.as_str());
        } else {
            beside.push(name.as_str());
        }
    }

    if sources == 0 {
        violations.push(format!(
            "{layer}: no source file was found, so a scan that matched nothing would have passed \
             this rule for free (ADR 0004)"
        ));
    } else {
        violations.extend(module_tree_violations(layer, root, &beside));
    }
    violations.sort();
    violations
}

/// The directory scan and the compiler must reach the same source files.
fn module_tree_violations(layer: &str, root: Option<&str>, beside: &[&str]) -> Vec<String> {
    let Some(source) = root else {
        return vec![format!(
            "{layer}: has no mod.rs, so the files beside it were compared against no module tree \
             at all (ADR 0004)"
        )];
    };

    let declared = declared_modules(source);
    let mut violations = Vec::new();
    for name in beside {
        let stem = name.strip_suffix(".rs").unwrap_or(name);
        if !declared.iter().any(|module| module == stem) {
            violations.push(format!(
                "{layer}/{name}: mod.rs declares no `mod {stem};`, so the file is part of this \
                 layer for one of the two gates that read it and invisible to the other \
                 (ADR 0004)"
            ));
        }
    }
    for module in &declared {
        if !beside.iter().any(|name| *name == format!("{module}.rs")) {
            violations.push(format!(
                "{layer}/mod.rs: declares `mod {module};` and there is no {module}.rs beside it; \
                 the module resolves somewhere this rule does not read (ADR 0004)"
            ));
        }
    }
    violations
}

fn declared_modules(source: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for (_, code) in code_lines(source) {
        let Some(declaration) = code.trim().strip_suffix(';') else {
            continue;
        };
        let named = declaration
            .trim_start_matches("pub(crate)")
            .trim_start_matches("pub")
            .trim_start();
        if let Some(name) = named.strip_prefix("mod ") {
            modules.push(name.trim().to_owned());
        }
    }
    modules
}

fn file_path_violations(gate: LayeringGate, name: &str, source: &str) -> Vec<String> {
    let LayeringGate {
        root: layer, owner, ..
    } = gate;
    let climb = if name == "mod.rs" {
        "super::"
    } else {
        "super::super::"
    };
    let prefix = format!("{owner}::");
    let mut violations = Vec::new();
    for (number, line) in code_lines(source) {
        let text = spaced(&line);
        let code = tightened(&text);
        for path in ["crate::", climb, prefix.as_str()] {
            if names_path(&code, path) {
                violations.push(format!(
                    "{layer}/{name}:{number}: `{path}` resolves above this layer's root; a path \
                     inside the layer names the layer or something under it, or the layer holds \
                     only by convention (ADR 0004)"
                ));
            }
        }
        if text.contains("extern crate") {
            violations.push(format!(
                "{layer}/{name}:{number}: `extern crate` gives this layer a second name for \
                 something above it — `extern crate self as {owner};` names the crate root in a \
                 spelling no path rule sees. `alloc` is declared by each root that compiles these \
                 sources, so nothing here needs one (ADR 0004)"
            ));
        }
        if text.replace(' ', "").contains("#[path") {
            violations.push(format!(
                "{layer}/{name}:{number}: `#[path]` maps a module of this layer onto a file \
                 outside the directory this rule reads, so the layer would hold code no rule here \
                 applies to (ADR 0004)"
            ));
        }
    }
    violations
}

fn spaced(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tightened(spaced: &str) -> String {
    spaced.replace(" ::", "::").replace(":: ", "::")
}

/// An identifier boundary distinguishes `Token` from names such as `SyncToken`.
fn names_path(code: &str, needle: &str) -> bool {
    code.match_indices(needle).any(|(at, _)| {
        code.get(..at)
            .and_then(|before| before.chars().next_back())
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
    })
}

/// Preserve source line numbers across multiline comments and literals for diagnostics.
fn code_lines(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut lines = Vec::new();
    let mut code = String::new();
    let mut number = 1usize;
    let mut at = 0usize;
    let mut comment = 0usize;
    let mut literal: Option<usize> = None;

    while let Some(&character) = chars.get(at) {
        if literal.is_some_and(|end| at >= end) {
            literal = None;
        }
        if character == '\n' {
            lines.push((number, std::mem::take(&mut code)));
            number = number.saturating_add(1);
            at = at.saturating_add(1);
        } else if literal.is_some() {
            at = at.saturating_add(1);
        } else if comment > 0 {
            let (step, depth) = block_comment_step(&chars, at, comment);
            comment = depth;
            at = at.saturating_add(step);
        } else if opens(&chars, at, '/') {
            at = at.saturating_add(1);
            while chars.get(at).is_some_and(|character| *character != '\n') {
                at = at.saturating_add(1);
            }
        } else if opens(&chars, at, '*') {
            comment = 1;
            at = at.saturating_add(2);
        } else if let Some(end) = literal_end(&chars, at) {
            literal = Some(end);
            at = at.saturating_add(1);
        } else {
            code.push(character);
            at = at.saturating_add(1);
        }
    }

    lines.push((number, code));
    lines
}

fn opens(chars: &[char], at: usize, second: char) -> bool {
    chars.get(at) == Some(&'/') && chars.get(at.saturating_add(1)) == Some(&second)
}

fn block_comment_step(chars: &[char], at: usize, depth: usize) -> (usize, usize) {
    if opens(chars, at, '*') {
        return (2, depth.saturating_add(1));
    }
    if chars.get(at) == Some(&'*') && chars.get(at.saturating_add(1)) == Some(&'/') {
        return (2, depth.saturating_sub(1));
    }
    (1, depth)
}

fn literal_end(chars: &[char], at: usize) -> Option<usize> {
    let mut index = at;
    if chars.get(index) == Some(&'b') {
        index = index.saturating_add(1);
    }
    let raw = chars.get(index) == Some(&'r');
    if raw {
        index = index.saturating_add(1);
    }
    let mut hashes = 0usize;
    while chars.get(index) == Some(&'#') {
        hashes = hashes.saturating_add(1);
        index = index.saturating_add(1);
    }
    match chars.get(index) {
        Some(&'"') => Some(string_end(chars, index.saturating_add(1), raw, hashes)),
        Some(&'\'') if !raw && hashes == 0 => character_end(chars, index),
        _ => None,
    }
}

fn string_end(chars: &[char], from: usize, raw: bool, hashes: usize) -> usize {
    let mut index = from;
    while let Some(&character) = chars.get(index) {
        if !raw && character == '\\' {
            index = index.saturating_add(2);
            continue;
        }
        index = index.saturating_add(1);
        if character == '"' && closed_by(chars, index, hashes) {
            return index.saturating_add(hashes);
        }
    }
    index
}

fn closed_by(chars: &[char], from: usize, hashes: usize) -> bool {
    (0..hashes).all(|offset| chars.get(from.saturating_add(offset)) == Some(&'#'))
}

fn character_end(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at.saturating_add(1)) == Some(&'\\') {
        let mut index = at.saturating_add(3);
        while let Some(&character) = chars.get(index) {
            index = index.saturating_add(1);
            if character == '\'' {
                return Some(index);
            }
        }
        return None;
    }
    (chars.get(at.saturating_add(2)) == Some(&'\'')).then(|| at.saturating_add(3))
}

fn manifest_violations(crate_name: &str, manifest: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for dependency in declared_dependencies(manifest) {
        let Declared { key, package, .. } = &dependency;
        if dependency.renamed {
            violations.push(format!(
                "{crate_name}: `{key}` renames the package it links to `{package}`; a rename \
                 exists only to make the linked name differ from the written one (ADR 0004)"
            ));
        }
        let is_core = CORE_CRATES.contains(&package.as_str());
        let is_recorded_boundary =
            ALLOWED_EXTERNAL_DEPENDENCIES.contains(&(crate_name, package.as_str()));
        if !is_core && !is_recorded_boundary {
            violations.push(format!(
                "{crate_name}: declares `{package}`; a core crate may depend only on other \
                 core crates or an explicitly recorded facade boundary (ADR 0013)"
            ));
        } else if is_core && !dependency.local {
            violations.push(format!(
                "{crate_name}: `{package}` does not resolve from inside this workspace; use \
                 `workspace = true` or a path under crates/ (ADR 0004)"
            ));
        }
    }
    violations
}

fn unregistered_core_crates(root: &Path) -> io::Result<Vec<String>> {
    let mut violations = Vec::new();
    for entry in fs::read_dir(root)? {
        let dir = entry?.path();
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if CORE_CRATES.contains(&name) {
            continue;
        }
        let lib = dir.join("src").join("lib.rs");
        if lib.is_file() && declares(&fs::read_to_string(&lib)?, "#![no_std]") {
            violations.push(format!(
                "{name}: declares `#![no_std]` but is absent from CORE_CRATES, so no rule in \
                 this gate applies to it (ADR 0004)"
            ));
        }
    }
    Ok(violations)
}

fn declares(source: &str, declaration: &str) -> bool {
    source.lines().any(|line| line.trim() == declaration)
}

fn workspace_root() -> io::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the xtask manifest directory has no parent",
            )
        })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Declared {
    key: String,
    package: String,
    renamed: bool,
    local: bool,
}

impl Declared {
    fn read(key: &str, spec: &str) -> Self {
        let package = inline_value(spec, "package");
        Self {
            key: key.to_owned(),
            package: package.unwrap_or(key).to_owned(),
            renamed: package.is_some_and(|package| package != key),
            local: inline_value(spec, "workspace") == Some("true")
                || inline_value(spec, "path").is_some(),
        }
    }
}

/// Read the workspace's manifest syntax, including package renames in dependency tables.
fn declared_dependencies(manifest: &str) -> Vec<Declared> {
    let mut declared = Vec::new();
    let mut subtable: Option<(String, Vec<String>)> = None;
    let mut inline_table: Option<(String, String)> = None;
    let mut inside_dependency_table = false;

    for line in manifest.lines() {
        let line = line.trim();
        let continuing_inline_table = inline_table.is_some();
        let mut completed_inline_table = false;
        if let Some((_, spec)) = inline_table.as_mut() {
            if !line.is_empty() && !line.starts_with('#') {
                spec.push(' ');
                spec.push_str(line);
            }
            if inline_table_is_complete(spec) {
                completed_inline_table = true;
            }
        }
        if completed_inline_table && let Some((key, spec)) = inline_table.take() {
            declared.push(Declared::read(&key, &spec));
        }
        if continuing_inline_table {
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            flush_subtable(&mut subtable, &mut declared);
            let header = header.trim();
            match dependency_subtable_name(header) {
                Some(name) => {
                    subtable = Some((name.to_owned(), Vec::new()));
                    inside_dependency_table = false;
                },
                None => inside_dependency_table = is_dependency_table(header),
            }
            continue;
        }

        if let Some((_, body)) = subtable.as_mut() {
            body.push(line.to_owned());
        } else if inside_dependency_table && let Some((key, spec)) = line.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !key.is_empty() {
                if spec.trim_start().starts_with('{') && !inline_table_is_complete(spec) {
                    inline_table = Some((key.to_owned(), spec.trim().to_owned()));
                } else {
                    declared.push(Declared::read(key, spec));
                }
            }
        }
    }

    if let Some((key, spec)) = inline_table {
        declared.push(Declared::read(&key, &spec));
    }
    flush_subtable(&mut subtable, &mut declared);
    declared.sort();
    declared
}

fn inline_table_is_complete(spec: &str) -> bool {
    let mut braces = 0usize;
    let mut quoted = false;
    let mut literal = false;
    let mut escaped = false;
    for character in spec.chars() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        if literal {
            if character == '\'' {
                literal = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '\'' => literal = true,
            '{' => braces = braces.saturating_add(1),
            '}' => braces = braces.saturating_sub(1),
            _ => {},
        }
    }
    braces == 0
}

fn flush_subtable(subtable: &mut Option<(String, Vec<String>)>, declared: &mut Vec<Declared>) {
    let Some((name, body)) = subtable.take() else {
        return;
    };
    let spec = format!("{{ {} }}", body.join(", "));
    declared.push(Declared::read(&name, &spec));
}

fn inline_value<'a>(spec: &'a str, key: &str) -> Option<&'a str> {
    let inner = spec.trim().strip_prefix('{')?.strip_suffix('}')?;
    inner.split(',').find_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"'))
    })
}

fn is_dependency_table(header: &str) -> bool {
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .any(|kind| header == *kind || header.ends_with(&format!(".{kind}")))
}

fn dependency_subtable_name(header: &str) -> Option<&str> {
    let (prefix, name) = header.rsplit_once('.')?;
    is_dependency_table(prefix).then(|| name.trim_matches('"'))
}

fn collect_codes_violations() -> io::Result<Vec<String>> {
    let root = workspace_root()?;
    let declarations = fs::read_to_string(root.join(DIAGNOSTIC_SOURCE))?;
    let golden = fs::read_to_string(root.join(GOLDEN_LIST))?;
    Ok(codes_violations(&declarations, &golden))
}

/// Compare meanings only after registry rows match variants in declaration order.
fn codes_violations(declarations: &str, golden: &str) -> Vec<String> {
    let variants = enum_variants(declarations, "DiagnosticCode");
    let arms = as_str_arms(declarations);
    let channels: Vec<String> = enum_variants(declarations, "Severity")
        .into_iter()
        .map(|(variant, _)| variant)
        .collect();

    let scanned = declaration_violations(&variants, &arms, &channels);
    if !scanned.is_empty() {
        return scanned;
    }

    let rows = match golden_rows(golden) {
        Ok(rows) => rows,
        Err(message) => return vec![message],
    };
    let codes = join_codes(&variants, &arms);
    let placement = placement_violations(&codes, &rows);
    if placement.is_empty() {
        content_violations(&codes, &rows, &channels)
    } else {
        placement
    }
}

fn declaration_violations(
    variants: &[(String, String)],
    arms: &[(String, String)],
    channels: &[String],
) -> Vec<String> {
    let mut violations = Vec::new();
    if variants.is_empty() {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: no `DiagnosticCode` variant was found; a scan that matches \
             nothing would pass every remaining check for free"
        ));
    }
    if channels.is_empty() {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: no `Severity` variant was found, so every channel a row \
             names would be unrecognizable"
        ));
    }
    if !names(variants).eq(names(arms)) {
        violations.push(format!(
            "{DIAGNOSTIC_SOURCE}: the `DiagnosticCode` variants and the `as_str` arms are not \
             the same names in the same order; the golden list is keyed on the arms, so a \
             variant with no arm has no key to be listed under (ADR 0009)"
        ));
    }
    violations
}

/// The caller verifies variant names and order before pairing them with their keys.
fn join_codes(variants: &[(String, String)], arms: &[(String, String)]) -> Vec<DeclaredCode> {
    variants
        .iter()
        .zip(arms)
        .map(|((_, meaning), (_, key))| DeclaredCode {
            key: key.clone(),
            meaning: meaning.clone(),
        })
        .collect()
}

fn placement_violations(codes: &[DeclaredCode], rows: &[Row]) -> Vec<String> {
    let mut violations = Vec::new();
    for code in codes {
        let key = &code.key;
        match rows.iter().filter(|row| row.code == *key).count() {
            0 => violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is declared in {DIAGNOSTIC_SOURCE} and has no row; a \
                 code with no row is a meaning nothing holds still (ADR 0009)"
            )),
            1 => {},
            count => violations.push(format!(
                "{GOLDEN_LIST}: `{key}` has {count} rows; the row for a code is that code's \
                 meaning, and two of them are two meanings (ADR 0009)"
            )),
        }
    }
    for row in rows {
        let key = &row.code;
        if !codes.iter().any(|code| code.key == *key) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` has a row and no `DiagnosticCode` declares it; a \
                 removed code is a break, and a typo in this column quietly retires a live one \
                 (ADR 0009)"
            ));
        }
    }
    if violations.is_empty() && !row_codes(rows).eq(code_keys(codes)) {
        violations.push(format!(
            "{GOLDEN_LIST}: the rows are not in declaration order; the two files stay in one \
             order so that adding a code is one line of diff in each (ADR 0009)"
        ));
    }
    violations
}

fn content_violations(codes: &[DeclaredCode], rows: &[Row], channels: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    for (code, row) in codes.iter().zip(rows) {
        let key = &row.code;
        if row.meaning != code.meaning {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is listed as \"{listed}\" and documented as \
                 \"{documented}\"; a meaning is frozen as hard as a name, so changing one is a \
                 rename or a deprecation (ADR 0009)",
                listed = row.meaning,
                documented = code.meaning
            ));
        }
        if !channels.contains(&row.channel) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` travels on `{channel}`, which is not a `Severity` \
                 {DIAGNOSTIC_SOURCE} declares (ADR 0009)",
                channel = row.channel
            ));
        }
        if !MILESTONES.contains(&row.milestone.as_str()) {
            violations.push(format!(
                "{GOLDEN_LIST}: `{key}` is owed by `{milestone}`, which ROADMAP.md does not \
                 name as a milestone",
                milestone = row.milestone
            ));
        }
    }
    violations.extend(unused_channel_violations(rows, channels));
    violations
}

fn unused_channel_violations(rows: &[Row], channels: &[String]) -> Vec<String> {
    channels
        .iter()
        .filter(|channel| !rows.iter().any(|row| row.channel == **channel))
        .map(|channel| {
            format!(
                "{GOLDEN_LIST}: no code travels on `Severity::{channel}`; a severity with \
                 nothing on it is unbuilt work, and the milestone column is where that is \
                 written down (ADR 0009)"
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DeclaredCode {
    key: String,
    meaning: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    code: String,
    meaning: String,
    channel: String,
    milestone: String,
}

impl Row {
    fn read(cells: &[String]) -> Result<Self, String> {
        let [code, meaning, channel, milestone] = cells else {
            return Err(format!(
                "{GOLDEN_LIST}: a row has {count} cells rather than {expected}: {cells:?}. A \
                 meaning holding a `|` reads as two cells, so this is refused rather than \
                 guessed at",
                count = cells.len(),
                expected = GOLDEN_COLUMNS.len()
            ));
        };
        Ok(Self {
            code: code.clone(),
            meaning: meaning.clone(),
            channel: channel.clone(),
            milestone: milestone.clone(),
        })
    }
}

fn golden_rows(document: &str) -> Result<Vec<Row>, String> {
    let lines = registry_lines(document)?;
    let mut rows = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut in_table = false;
    for line in lines {
        match table_cells(line) {
            None if in_table && !line.trim().is_empty() => {
                return Err(format!(
                    "{GOLDEN_LIST}: unexpected content in the registry block"
                ));
            },
            None if in_table => {},
            None => header = None,
            Some(cells) if in_table => rows.push(Row::read(&cells)?),
            Some(cells) if is_rule_row(&cells) => {
                check_columns(header.as_deref())?;
                in_table = true;
            },
            Some(cells) => header = Some(cells),
        }
    }

    if !in_table {
        return Err(format!(
            "{GOLDEN_LIST}: no `{columns}` table was found, so the declarations would have been \
             compared against nothing",
            columns = GOLDEN_COLUMNS.join(" | ")
        ));
    }
    if rows.is_empty() {
        return Err(format!("{GOLDEN_LIST}: the table has a header and no rows"));
    }
    Ok(rows)
}

#[derive(Clone, Copy)]
struct CodeFence {
    marker: u8,
    width: usize,
}

impl CodeFence {
    fn parse(line: &str) -> Option<(Self, &str)> {
        let trimmed = line.trim_start_matches(' ');
        if line.len().saturating_sub(trimmed.len()) > 3 {
            return None;
        }
        let marker = *trimmed.as_bytes().first()?;
        if !matches!(marker, b'`' | b'~') {
            return None;
        }
        let width = trimmed.bytes().take_while(|byte| *byte == marker).count();
        let info = trimmed.get(width..)?.trim_matches([' ', '\t']);
        if width < 3 || (marker == b'`' && info.contains('`')) {
            return None;
        }
        Some((Self { marker, width }, info))
    }

    fn closes(self, line: &str) -> bool {
        Self::parse(line).is_some_and(|(closing, info)| {
            closing.marker == self.marker && closing.width >= self.width && info.is_empty()
        })
    }
}

enum RegistryFence<'a> {
    Example(CodeFence),
    Registry(CodeFence, Vec<&'a str>),
}

fn registry_lines(document: &str) -> Result<Vec<&str>, String> {
    let mut open: Option<RegistryFence<'_>> = None;
    let mut registry = None;
    let mut html_comment = false;
    for line in document.lines() {
        if let Some(active) = open.as_mut() {
            let marker = match active {
                RegistryFence::Example(marker) | RegistryFence::Registry(marker, _) => *marker,
            };
            if marker.closes(line) {
                if let Some(RegistryFence::Registry(_, lines)) = open.take() {
                    registry = Some(lines);
                }
            } else if let RegistryFence::Registry(_, lines) = active {
                lines.push(line);
            }
            continue;
        }
        let trimmed = line.trim_start_matches(' ');
        if html_comment
            || (line.len().saturating_sub(trimmed.len()) <= 3 && trimmed.starts_with("<!--"))
        {
            html_comment = !line.contains("-->");
            continue;
        }
        if let Some((marker, info)) = CodeFence::parse(line) {
            open = Some(if info == "text" {
                if registry.is_some() {
                    return Err(format!(
                        "{GOLDEN_LIST}: multiple text blocks compete for the registry"
                    ));
                }
                RegistryFence::Registry(marker, Vec::new())
            } else {
                RegistryFence::Example(marker)
            });
        }
    }
    if matches!(open, Some(RegistryFence::Registry(..))) {
        return Err(format!(
            "{GOLDEN_LIST}: the registry text block has no closing fence"
        ));
    }
    registry.ok_or_else(|| format!("{GOLDEN_LIST}: no fenced text block contains the registry"))
}

fn check_columns(header: Option<&[String]>) -> Result<(), String> {
    let Some(cells) = header else {
        return Err(format!(
            "{GOLDEN_LIST}: a table rule has no header row above it"
        ));
    };
    if cells.iter().map(String::as_str).eq(GOLDEN_COLUMNS) {
        return Ok(());
    }
    Err(format!(
        "{GOLDEN_LIST}: the table's columns are `{found}`, not `{expected}`; the cells are read \
         by position, so renaming or reordering one redefines every row",
        found = cells.join(" | "),
        expected = GOLDEN_COLUMNS.join(" | ")
    ))
}

fn table_cells(line: &str) -> Option<Vec<String>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    Some(
        inner
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect(),
    )
}

fn is_rule_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            !cell.is_empty()
                && cell
                    .chars()
                    .all(|character| character == '-' || character == ':')
        })
}

/// Diagnostic and severity declarations use one fieldless variant per line.
fn enum_variants(source: &str, name: &str) -> Vec<(String, String)> {
    let header = format!("pub enum {name} {{");
    let mut variants = Vec::new();
    let mut meaning = Paragraph::default();
    let mut inside = false;

    for line in source.lines() {
        let line = line.trim();
        if !inside {
            inside = line == header;
        } else if line == "}" {
            break;
        } else if let Some(doc) = line.strip_prefix("///") {
            meaning.push(doc.trim());
        } else if let Some(variant) = variant_name(line) {
            variants.push((variant.to_owned(), meaning.take()));
        }
    }
    variants
}

fn variant_name(line: &str) -> Option<&str> {
    let name = line.strip_suffix(',')?;
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    plain.then_some(name)
}

fn as_str_arms(source: &str) -> Vec<(String, String)> {
    let mut arms = Vec::new();
    let mut inside = false;

    for line in source.lines() {
        if !inside {
            inside = line.contains("fn as_str(");
        } else if line == "}" || line.trim_start().starts_with("pub ") {
            break;
        } else if let Some(arm) = match_arm(line.trim()) {
            arms.push(arm);
        }
    }
    arms
}

fn match_arm(line: &str) -> Option<(String, String)> {
    let (variant, rest) = line.strip_prefix("Self::")?.split_once("=>")?;
    let quoted = rest.trim().strip_suffix(',')?;
    let key = quoted.strip_prefix('"')?.strip_suffix('"')?;
    Some((variant.trim().to_owned(), key.to_owned()))
}

/// ADR 0009 fixes the first documentation paragraph as the diagnostic's meaning.
#[derive(Debug, Default)]
struct Paragraph {
    text: Vec<String>,
    ended: bool,
}

impl Paragraph {
    fn push(&mut self, line: &str) {
        if line.is_empty() {
            self.ended = !self.text.is_empty();
        } else if !self.ended {
            self.text.push(line.to_owned());
        }
    }

    fn take(&mut self) -> String {
        self.ended = false;
        std::mem::take(&mut self.text).join(" ")
    }
}

fn names(entries: &[(String, String)]) -> impl Iterator<Item = &str> {
    entries.iter().map(|(name, _)| name.as_str())
}

fn code_keys(codes: &[DeclaredCode]) -> impl Iterator<Item = &str> {
    codes.iter().map(|code| code.key.as_str())
}

fn row_codes(rows: &[Row]) -> impl Iterator<Item = &str> {
    rows.iter().map(|row| row.code.as_str())
}

#[cfg(test)]
mod tests {
    use super::{
        LAYERING_GATES, LayerEntry, LayeringGate, RepositoryDocuments, as_str_arms,
        basic_example_violations, closure_status_violations, codes_violations,
        collect_architecture_violations, collect_codes_violations, collect_purity_violations,
        collect_repository_violations, core_list_violations, declared_dependencies, declares,
        dependency_subtable_name, documentation_violations, enum_variants, file_path_violations,
        golden_rows, is_dependency_table, layer_violations, layering_violations,
        manifest_violations, match_arm_violations, private_crate_violations,
        public_api_workflow_violations, recipe_violations, release_violations,
        repository_documentation_violations, retired_crate_violations, snapshot_violations,
        table_keys, xml_writer_violations,
    };

    const GATE: LayeringGate = LAYERING_GATES[0];

    fn file_paths(name: &str, source: &str) -> Vec<String> {
        file_path_violations(GATE, name, source)
    }

    fn layers(entries: &[LayerEntry]) -> Vec<String> {
        layer_violations(GATE, entries)
    }

    fn layering(root: &str, member: Option<(&str, &str)>) -> Vec<String> {
        layering_violations(GATE, root, member)
    }

    fn packages(manifest: &str) -> Vec<String> {
        declared_dependencies(manifest)
            .into_iter()
            .map(|dependency| dependency.package)
            .collect()
    }

    #[test]
    fn reads_dependencies_from_a_plain_table() {
        let manifest = r#"
[package]
name = "x"

[dependencies]
serde = "1"
log = { version = "0.4" }
"#;
        let linked = packages(manifest);
        assert!(linked.contains(&"serde".to_owned()));
        assert!(linked.contains(&"log".to_owned()));
        assert!(
            !linked.contains(&"name".to_owned()),
            "package keys are not dependencies"
        );
    }

    #[test]
    fn reads_dependencies_from_a_subtable_header() {
        let linked = packages(
            "[dependencies.serde]
version = \"1\"
",
        );
        assert_eq!(linked, vec!["serde".to_owned()]);
        assert!(
            !linked.contains(&"version".to_owned()),
            "keys inside a dependency subtable describe it, they are not further dependencies"
        );
    }

    #[test]
    fn an_inline_rename_is_read_as_the_package_it_links() {
        let manifest = r#"
[dependencies]
ical-dav = { package = "libm", version = "0.2" }
"#;
        assert_eq!(packages(manifest), vec!["libm".to_owned()]);

        let violations = manifest_violations("ical-tz", manifest);
        assert_eq!(violations.len(), 2, "the rename and the outside package");
        assert!(
            violations.iter().any(|line| line.contains("renames")),
            "a rename is itself a violation: {violations:?}"
        );
        assert!(
            violations.iter().any(|line| line.contains("`libm`")),
            "the linked package is named, not the key: {violations:?}"
        );
    }

    #[test]
    fn a_subtable_rename_is_read_as_the_package_it_links() {
        let manifest = r#"
[dependencies.ical-dav]
package = "libm"
version = "0.2"
"#;
        assert_eq!(packages(manifest), vec!["libm".to_owned()]);
        assert!(
            manifest_violations("ical-tz", manifest)
                .iter()
                .any(|line| line.contains("`libm`")),
            "a package line inside a subtable is the same rename as the inline one"
        );
    }

    #[test]
    fn recorded_facade_dependencies_are_no_violation() {
        let manifest = r#"
[package]
name = "icalkit"

[dependencies]
jiff = { version = "0.2.35", default-features = false }
xmlparser = { version = "0.13.6", default-features = false }
icu_calendar = { version = "2.4.0", default-features = false, features = ["compiled_data"] }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_formatted_multiline_path_dependency_is_still_local() {
        let manifest = r#"
[dependencies]
icalkit = { path = "../icalkit", version = "0.0.0", features = [
  "std",
] }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "taplo may wrap a local feature-bearing inline table without changing its provenance"
        );
    }

    #[test]
    fn only_the_facade_may_expose_the_jiff_boundary() {
        let manifest = r#"
[dependencies]
jiff = { version = "0.2.35", default-features = false, features = ["alloc"] }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "the unified facade owns the public civil-time boundary"
        );
        assert!(
            manifest_violations("icalkit-conformance", manifest)
                .iter()
                .any(|line| line.contains("jiff")),
            "an isolation tool must not acquire a second public time boundary"
        );
    }

    #[test]
    fn only_the_facade_may_own_the_private_xml_lexer() {
        let manifest = r#"
[dependencies]
xmlparser = { version = "0.13.6", default-features = false }
"#;
        assert_eq!(
            manifest_violations("icalkit", manifest),
            Vec::<String>::new(),
            "the facade owns the private lexer wrapper"
        );
        assert!(
            manifest_violations("icalkit-conformance", manifest)
                .iter()
                .any(|line| line.contains("xmlparser")),
            "an isolation tool must record its dependency independently"
        );
    }

    #[test]
    fn only_the_facade_may_own_calendar_conversion() {
        let manifest = r#"
[dependencies]
icu_calendar = { version = "2.4.0", default-features = false, features = ["compiled_data"] }
"#;
        assert!(
            manifest_violations("icalkit-conformance", manifest)
                .iter()
                .any(|line| line.contains("icu_calendar")),
        );
    }

    #[test]
    fn a_core_crate_taken_from_a_registry_is_a_violation() {
        let manifest = "[dependencies]
icalkit = \"0.1\"
";
        assert!(
            manifest_violations("icalkit", manifest)
                .iter()
                .any(|line| line.contains("does not resolve from inside this workspace")),
            "the name is ours; the source is not"
        );
    }

    #[test]
    fn treats_dev_build_and_target_dependencies_as_dependencies() {
        assert!(is_dependency_table("dev-dependencies"));
        assert!(is_dependency_table("build-dependencies"));
        assert!(is_dependency_table("target.'cfg(windows)'.dependencies"));
        assert!(!is_dependency_table("package"));
        assert!(!is_dependency_table("lints.clippy"));
    }

    #[test]
    fn names_a_dependency_subtable_only_under_a_dependency_table() {
        assert_eq!(
            dependency_subtable_name("dependencies.serde"),
            Some("serde")
        );
        assert_eq!(
            dependency_subtable_name("target.'cfg(windows)'.dependencies"),
            None,
            "this is a dependency table, not a single dependency"
        );
        assert_eq!(dependency_subtable_name("workspace.package"), None);
    }

    #[test]
    fn an_empty_manifest_declares_nothing() {
        assert_eq!(declared_dependencies(""), []);
        assert_eq!(
            declared_dependencies(
                "[dependencies]
"
            ),
            []
        );
    }

    #[test]
    fn only_dependencies_from_outside_the_core_are_violations() {
        let manifest = r#"
[dependencies]
icalkit = { path = "../icalkit" }

[dev-dependencies]
proptest = "1"
"#;
        let violations = manifest_violations("icalkit", manifest);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations
                .first()
                .is_some_and(|line| line.contains("proptest"))
        );
    }

    #[test]
    fn recognizes_a_crate_level_declaration() {
        assert!(declares(
            "//! docs

#![no_std]
",
            "#![no_std]"
        ));
        assert!(declares(
            "#![no_std]
extern crate alloc;
",
            "extern crate alloc;"
        ));
        assert!(!declares(
            "//! docs
",
            "#![no_std]"
        ));
        assert!(
            !declares(
                "//! This crate is `#![no_std]` throughout.
",
                "#![no_std]"
            ),
            "naming the attribute in prose is not declaring it"
        );
    }

    fn source(name: &str, body: &str) -> LayerEntry {
        LayerEntry {
            name: name.to_owned(),
            directory: false,
            source: body.to_owned(),
        }
    }

    #[test]
    fn the_two_copies_of_the_core_list_are_read_against_each_other() {
        let recipe = format!(
            "core_crates := \"{}\"\n",
            super::CORE_CRATES
                .iter()
                .map(|name| format!("-p {name}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(recipe_violations(&recipe), Vec::<String>::new());

        let missing = recipe.replace("-p icalkit", "");
        assert!(
            recipe_violations(&missing)
                .iter()
                .any(|line| line.contains("does not name `icalkit`")),
            "a core crate the recipe never compiles for a bare-metal target"
        );

        let extra = recipe.replace("core_crates := \"", "core_crates := \"-p ical-carddav ");
        assert!(
            recipe_violations(&extra)
                .iter()
                .any(|line| line.contains("names `ical-carddav`")),
            "a crate the recipe compiles and this gate states no rule about"
        );
    }

    #[test]
    fn a_recipe_with_no_core_list_is_reported_rather_than_compared_against_nothing() {
        assert!(
            recipe_violations("# a Justfile with no such assignment\n")
                .iter()
                .any(|line| line.contains("compared against nothing")),
            "the failure mode of a hand-rolled scan is matching nothing and passing"
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_grammar_layer_is_a_violation() {
        let violations = file_paths("token.rs", "use crate::tree::Component;\n");
        assert!(
            violations
                .iter()
                .any(|line| line.contains("token.rs:1") && line.contains("`crate::`")),
            "the file and the line are the whole point of reporting it: {violations:?}"
        );

        assert_eq!(
            file_paths("token.rs", "use super::Token;\n"),
            Vec::<String>::new(),
            "one `super::` beside the root names the root, which is inside the layer"
        );
        assert!(
            !file_paths("token.rs", "use super::super::Writer;\n").is_empty(),
            "two of them name the crate"
        );
        assert!(
            !file_paths("mod.rs", "use super::Writer;\n").is_empty(),
            "`mod.rs` is the root, so one `super::` there already names the crate"
        );
    }

    const LAYER_PROSE: &str = r##"
// use crate::tree::Component;
/// A doc link to [`Token`](crate::Token), which is how a reader reaches the item.
/*
   crate::Token, inside a block comment /* that nests */ and closes here.
*/
const QUOTE: u8 = b'"';
const LIFETIME: fn(&str) -> &str = |value: &str| value;
const MESSAGE: &str = "crate::Token";
const RAW: &[u8] = br"crate::Token ends with \";
const HASHED: &str = r#"crate::Token "quoted" inside"#;
"##;

    #[test]
    fn a_path_written_in_a_comment_or_a_literal_is_not_a_path() {
        assert_eq!(
            file_paths("lexer.rs", LAYER_PROSE),
            Vec::<String>::new(),
            "the rendered documentation is `ical-core`'s, and its links have to name items"
        );
        assert!(
            !file_paths(
                "lexer.rs",
                &format!("{LAYER_PROSE}use crate::tree::Tree;\n")
            )
            .is_empty(),
            "and the scan must still be reading code after all of that"
        );
    }

    #[test]
    fn a_subdirectory_under_the_grammar_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\n"),
            source("token.rs", ""),
            LayerEntry {
                name: "value".to_owned(),
                directory: true,
                source: String::new(),
            },
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("value") && line.contains("flat")),
            "a nested file is one `super::` deeper, and the rule is stated in that arithmetic"
        );
    }

    #[test]
    fn an_empty_layer_is_reported_rather_than_passing_for_free() {
        assert!(
            layers(&[])
                .iter()
                .any(|line| line.contains("no source file was found")),
            "a scan of nothing finds nothing, which is not the same as finding nothing wrong"
        );
        assert_eq!(
            layers(&[source("mod.rs", "mod token;\n"), source("token.rs", "")]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_file_the_module_root_does_not_declare_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\n"),
            source("token.rs", ""),
            source("sneak.rs", "pub(crate) const HIDDEN: u8 = 0;\n"),
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("sneak.rs") && line.contains("mod sneak;")),
            "an undeclared file is scanned by this rule and never compiled by the layering member"
        );
    }

    #[test]
    fn a_module_the_directory_does_not_hold_is_a_violation() {
        let entries = [
            source("mod.rs", "mod token;\nmod launder;\n"),
            source("token.rs", ""),
        ];
        assert!(
            layers(&entries)
                .iter()
                .any(|line| line.contains("mod launder;") && line.contains("launder.rs")),
            "a module with no file beside it resolves somewhere this rule does not read"
        );
    }

    #[test]
    fn a_path_attribute_inside_the_layer_is_a_violation() {
        assert!(
            file_paths("mod.rs", "#[path = \"../launder.rs\"]\nmod launder;\n")
                .iter()
                .any(|line| line.contains("`#[path]`")),
            "`#[path]` pulls a file into the layer that the directory listing never shows"
        );
    }

    #[test]
    fn an_extern_crate_declaration_inside_the_layer_is_a_violation() {
        let violations = file_paths(
            "token.rs",
            "extern crate self as icalkit;\nuse icalkit::Token as Laundered;\n",
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`extern crate`")),
            "the declaration is what makes the spelling below available: {violations:?}"
        );
        assert!(
            violations.iter().any(|line| line.contains("`icalkit::`")),
            "and the crate's own name is a path above the layer like any other: {violations:?}"
        );
    }

    #[test]
    fn whitespace_does_not_hide_a_path_that_climbs_out() {
        assert!(
            !file_paths("lexer.rs", "use crate ::Token as Laundered;\n").is_empty(),
            "the same import with one space in it, which only `cargo fmt` was catching"
        );
        assert!(
            !file_paths("token.rs", "use super :: super ::tree::Component;\n").is_empty(),
            "and the climbing spelling, which rustfmt would also have normalized"
        );
    }

    const EXHAUSTIVE_MATCH: &str = "\
fn take(token: Token<'_>) -> usize {
    match token {
        Token::Name(bytes) => bytes.len(),
        Token::Parameter {
            name,
            value,
            has_value,
        } => name.len().saturating_add(value.len()).saturating_add(usize::from(has_value)),
        Token::Value { bytes, .. } => bytes.len(),
        Token::EndOfLine { folds, .. } => folds.len(),
    }
}
";

    #[test]
    fn a_wildcard_arm_over_the_guarded_type_is_a_violation() {
        assert_eq!(
            match_arm_violations("parse.rs", EXHAUSTIVE_MATCH),
            Vec::<String>::new(),
            "naming every variant is the shape this rule exists to keep"
        );

        let swallowed = EXHAUSTIVE_MATCH.replace(
            "        Token::Value { bytes, .. } => bytes.len(),\n",
            "        _ => 0,\n",
        );
        assert!(
            match_arm_violations("parse.rs", &swallowed)
                .iter()
                .any(|line| line.contains("parse.rs:") && line.contains("a wildcard arm over")),
            "a match that omits a variant and adds `_` is the shape `unreachable_patterns` is \
             silent about, and the only shape that loses data"
        );

        let guarded = EXHAUSTIVE_MATCH.replace(
            "        Token::Value { bytes, .. } => bytes.len(),\n",
            "        _ if true => 0,\n",
        );
        assert!(
            !match_arm_violations("parse.rs", &guarded).is_empty(),
            "a guard does not make a catch-all name a variant"
        );
    }

    #[test]
    fn a_wildcard_arm_over_another_type_is_not_this_rule() {
        let source = "\
fn label(severity: Severity) -> &'static str {
    match severity {
        Severity::Note => \"note\",
        _ => \"else\",
    }
}
";
        assert_eq!(
            match_arm_violations("report.rs", source),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_type_whose_name_ends_in_the_guarded_one_is_a_different_type() {
        let source = "\
fn stale(token: SyncToken) -> bool {
    match token {
        SyncToken::Weak => true,
        _ => false,
    }
}
";
        assert_eq!(
            match_arm_violations("freshness.rs", source),
            Vec::<String>::new(),
            "`SyncToken::` ends in `Token::` and is a type this rule states nothing about"
        );
    }

    #[test]
    fn a_wildcard_beside_a_match_over_the_guarded_type_is_read_at_its_own_depth() {
        let source = "\
fn take(token: Token<'_>, kind: Kind) -> usize {
    match kind {
        Kind::Line => match token {
            Token::Name(bytes) => bytes.len(),
            Token::Parameter { .. } => 1,
            Token::Value { .. } => 2,
            Token::EndOfLine { .. } => 3,
        },
        _ => 0,
    }
}
";
        assert_eq!(
            match_arm_violations("mutate.rs", source),
            Vec::<String>::new(),
            "the outer match is over another type and the inner one names every variant"
        );
    }

    const LAYERING_ROOT: &str = "\
[workspace]
members = [
  \"crates/ical-core\",
  \"gates/grammar-layering\",
  \"xtask\",
]
";

    const LAYERING_MANIFEST: &str = "\
[package]
name = \"ical-grammar-layering\"
publish = false

[lib]
doc = false
test = false
";

    const LAYERING_LIB: &str = "\
#[path = \"../../../crates/icalkit/src/internal/core/grammar/mod.rs\"]
mod grammar;
";

    #[test]
    fn the_member_that_makes_the_layer_a_fact_is_held_by_string_equality() {
        let committed = Some((LAYERING_MANIFEST, LAYERING_LIB));
        assert_eq!(layering(LAYERING_ROOT, committed), Vec::<String>::new());

        let dropped = LAYERING_ROOT.replace("  \"gates/grammar-layering\",\n", "");
        assert!(
            layering(&dropped, committed)
                .iter()
                .any(|line| line.contains("does not register")),
            "a pull request that deleted this member used to pass every gate in the repository"
        );

        assert!(
            layering(LAYERING_ROOT, None)
                .iter()
                .any(|line| line.contains("has no Cargo.toml")),
            "and one that deleted the files rather than the member line, likewise"
        );

        let copied = LAYERING_LIB.replace("/grammar/", "/grammar-copy/");
        assert!(
            layering(LAYERING_ROOT, Some((LAYERING_MANIFEST, &copied)))
                .iter()
                .any(|line| line.contains("shipped sources")),
            "a gate compiling a copy proves something about the copy"
        );

        let counted_twice = LAYERING_MANIFEST.replace("test = false\n", "");
        assert!(
            layering(LAYERING_ROOT, Some((&counted_twice, LAYERING_LIB)))
                .iter()
                .any(|line| line.contains("test = false")),
            "without it the grammar's tests run twice and its coverage is counted twice"
        );
    }

    #[test]
    fn only_the_facade_carries_a_future_release_contract() {
        let published = ["icalkit".to_owned()];
        assert_eq!(
            core_list_violations(&published),
            Vec::<String>::new(),
            "the committed workspace"
        );

        let mut added = published.to_vec();
        added.push("ical-core".to_owned());
        assert!(
            core_list_violations(&added)
                .iter()
                .any(|line| line.contains("ical-core")),
            "an implementation crate must not regain a separate semver contract"
        );

        assert!(
            core_list_violations(&[])
                .iter()
                .any(|line| line.contains("sole future public crate")),
            "the facade is the one production release boundary"
        );
    }

    #[test]
    fn private_workspace_crates_are_explicit_and_never_published() {
        let private = ["icalkit-conformance".to_owned()];
        assert_eq!(
            private_crate_violations(&private),
            Vec::<String>::new(),
            "the committed private tool set"
        );

        assert!(
            private_crate_violations(&[])
                .iter()
                .any(|line| line.contains("`icalkit-conformance`") && line.contains("missing")),
            "a named isolation tool must remain a workspace member"
        );
        assert!(
            private_crate_violations(&["unclassified-helper".to_owned()])
                .iter()
                .any(|line| line.contains("`unclassified-helper`") && line.contains("unclassified")),
            "a private crate under crates/ must be admitted deliberately"
        );
    }

    #[test]
    fn feature_keys_are_read_only_from_the_requested_top_level_table() {
        let manifest = r#"
[package]
name = "facade"

[features]
default = [
    "std",
]
std = []
system-tz = ["std"]

[dependencies]
jiff = "0.2"
"#;
        assert_eq!(
            table_keys(manifest, "features"),
            ["default", "std", "system-tz"]
        );
    }

    #[test]
    fn a_retired_package_cannot_return_as_a_member_or_facade_dependency() {
        let root = r#"
[workspace]
members = [
  "crates/icalkit",
  "crates/ical-core",
  "crates/ical-dav",
  "crates/ical-itip",
  "crates/ical-query",
  "crates/ical-recur",
  "crates/ical-tz",
]
"#;
        let facade = r#"
[dependencies]
ical-core = { path = "../ical-core" }
ical-dav = { path = "../ical-dav" }
ical-itip = { path = "../ical-itip" }
ical-query = { path = "../ical-query" }
ical-recur = { path = "../ical-recur" }
ical-tz = { path = "../ical-tz" }
"#;
        let violations = retired_crate_violations(root, facade);
        assert_eq!(violations.len(), 12);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("workspace member"))
        );
        assert!(violations.iter().any(|line| line.contains("still depends")));

        assert_eq!(
            retired_crate_violations(
                "[workspace]\nmembers = [\"crates/icalkit\"]",
                "[dependencies]\njiff = \"0.2\"",
            ),
            Vec::<String>::new()
        );
    }

    const SAMPLE_RELEASE: &str = "\
[[package]]
name = \"icalkit\"
";

    fn published() -> Vec<String> {
        vec!["icalkit".to_owned()]
    }

    #[test]
    fn the_release_configuration_is_read_against_the_workspace_members() {
        assert_eq!(
            release_violations(&published(), SAMPLE_RELEASE),
            Vec::<String>::new()
        );

        let stale = format!("{SAMPLE_RELEASE}\n[[package]]\nname = \"ical-grammar\"\n");
        assert!(
            release_violations(&published(), &stale)
                .iter()
                .any(|line| line.contains("`ical-grammar`") && line.contains("does not publish")),
            "the collapse deleted the crate and this file named it for a whole landing"
        );

        let mut added = published();
        added.push("ical-query".to_owned());
        let violations = release_violations(&added, SAMPLE_RELEASE);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `[[package]]` block")),
            "a new crate is released outside the version group until it has one: {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`changelog_include` does not name")),
            "and its history is missing from the one changelog this stack has: {violations:?}"
        );
    }

    #[test]
    fn the_committed_tree_passes_every_leg_of_purity() {
        assert_eq!(collect_purity_violations().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn the_committed_tree_has_one_public_crate_and_two_features() {
        assert_eq!(
            collect_architecture_violations().unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn repository_documents_hold_public_entry_points_and_remote_policy_together() {
        let valid = RepositoryDocuments {
            readme: "actions/workflows/ci.yml/badge.svg?branch=main\n\
                     actions/workflows/codeql.yml/badge.svg?branch=main\n\
                     [Contributing](../CONTRIBUTING.md) [Support](../SUPPORT.md)\n\
                     [Security policy](../SECURITY.md)\n\
                     [repository policy](repository-policy.md)",
            contributing: "The private kernel stays no_std and sans-I/O.",
            security: "Until the first release, main receives fixes. security/advisories/new",
            support: "GitHub Discussions; public GitHub issue; security/advisories/new; \
                      Do not include confidential data.",
            license: "MIT OR Apache-2.0; LICENSES/MIT.txt; LICENSES/Apache-2.0.txt",
            policy: "`main`; `required`; squash; draft pull request; `0.0.0`; \
                     explicit release decision; optional non-provider pattern and validity \
                     extensions; Renovate",
            feature_request: "name: Design proposal\nlabels: [\"enhancement\"]\nSee SECURITY.md",
            issue_config: "blank_issues_enabled: false\n\
                           https://github.com/P4suta/icalkit/discussions\n\
                           security/advisories/new",
            ci: "mise x -- just check\nci-summary ordinary",
            codeql: "security-extended\nsecurity-events: write",
            codeowners: "/SECURITY.md @P4suta\n/docs/repository-policy.md @P4suta",
        };
        assert_eq!(
            repository_documentation_violations(&valid),
            Vec::<String>::new()
        );

        let root_relative = valid
            .readme
            .replace("../", "")
            .replace("(repository-policy.md)", "(docs/repository-policy.md)");
        let misplaced = RepositoryDocuments {
            readme: &root_relative,
            ..valid
        };
        let violations = repository_documentation_violations(&misplaced);
        for link in [
            "[Contributing](../CONTRIBUTING.md)",
            "[Support](../SUPPORT.md)",
            "[Security policy](../SECURITY.md)",
            "[repository policy](repository-policy.md)",
        ] {
            assert!(violations.iter().any(|violation| {
                violation.starts_with("docs/README.md:") && violation.contains(link)
            }));
        }
    }

    #[test]
    fn repository_documents_reject_missing_channels_and_retired_paths() {
        let incomplete = RepositoryDocuments {
            readme: "",
            contributing: "The `ical-core` crate is public.",
            security: "Use ical_dav::Revision and `ical-itip`.",
            support: "",
            license: "",
            policy: "",
            feature_request: "",
            issue_config: "",
            ci: "",
            codeql: "",
            codeowners: "",
        };
        let violations = repository_documentation_violations(&incomplete);
        assert!(violations.iter().any(|violation| {
            violation.starts_with("docs/README.md:") && violation.contains("badge.svg")
        }));
        assert!(violations.iter().any(|violation| {
            violation.contains("CONTRIBUTING.md") && violation.contains("ical-core")
        }));
        assert!(violations.iter().any(|violation| {
            violation.contains("SECURITY.md") && violation.contains("ical_dav::")
        }));
    }

    #[test]
    fn repository_policy_requires_the_ordinary_entrypoint() {
        let documents = RepositoryDocuments {
            readme: "",
            contributing: "",
            security: "",
            support: "",
            license: "",
            policy: "",
            feature_request: "",
            issue_config: "",
            ci: "unrelated CI command",
            codeql: "",
            codeowners: "",
        };

        assert!(
            repository_documentation_violations(&documents)
                .iter()
                .any(|violation| violation.contains("mise x -- just check"))
        );
    }

    #[test]
    fn the_committed_tree_carries_the_repository_policy() {
        assert_eq!(
            collect_repository_violations().unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn public_api_snapshots_install_the_pinned_nightly_they_run_on() {
        let valid = "\njobs:\n  public-api:\n    env:\n      RUSTUP_TOOLCHAIN: nightly-2026-07-02\n    steps:\n      - run: rustup toolchain install nightly-2026-07-02 --profile minimal\n\n  msrv:\n";
        assert_eq!(public_api_workflow_violations(valid), Vec::<String>::new());

        let stable_only = "\njobs:\n  public-api:\n    steps:\n      - uses: rust-toolchain # stable\n\n  msrv:\n";
        assert!(
            public_api_workflow_violations(stable_only)
                .iter()
                .any(|violation| violation.contains("nightly-2026-07-02"))
        );
    }

    #[test]
    fn public_api_cargo_tools_require_the_configured_rust_installation() {
        for tools in [
            "just cargo-binstall cargo:cargo-public-api",
            "just cargo-binstall cargo:cargo-public-api # rust",
        ] {
            let workflow = format!(
                "\njobs:\n  public-api:\n    env:\n      RUSTUP_TOOLCHAIN: nightly-2026-07-02\n    steps:\n      - run: rustup toolchain install nightly-2026-07-02 --profile minimal\n      - uses: jdx/mise-action\n        with:\n          install_args: {tools}\n\n  msrv:\n"
            );
            assert!(
                !public_api_workflow_violations(&workflow).is_empty(),
                "the dated nightly does not install the configured Rust dependency: {tools}"
            );
        }
    }

    #[test]
    fn every_cargo_tool_installation_requires_its_compiler() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        let broken = ci.replace("install_args: rust just", "install_args: just");
        assert!(
            super::ci_source_violations(&broken, &candidate, &justfile, &hooks)
                .iter()
                .any(|violation| violation.contains("Cargo tools require configured Rust"))
        );
    }

    #[test]
    fn actionlint_installations_require_the_shellcheck_helper() {
        for tools in ["actionlint", "actionlint # shellcheck"] {
            let workflow = format!("install_args: {tools}\n");
            assert!(
                super::mise_installation_violations(&workflow)
                    .iter()
                    .any(|violation| violation.contains("actionlint requires shellcheck")),
                "missing shellcheck admitted: {tools}"
            );
        }
        assert_eq!(
            super::mise_installation_violations("install_args: actionlint shellcheck\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mise_installation_guards_refuse_unsupported_yaml_scalars() {
        for arguments in [
            "'cargo:cargo-deny'",
            "\"cargo:cargo-deny\"",
            "|-\n  cargo:cargo-deny",
            ">-\n  cargo:cargo-deny",
            "'actionlint'",
            "\"actionlint\"",
            ">\n  actionlint",
            "rust\n  actionlint",
            "just\n\n  cargo:cargo-deny",
        ] {
            assert!(
                !super::mise_installation_violations(&format!("install_args: {arguments}\n"))
                    .is_empty(),
                "unsupported scalar hid an installation dependency: {arguments}"
            );
        }
        assert_eq!(
            super::mise_installation_violations(
                "install_args: rust cargo:cargo-deny actionlint shellcheck\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn repository_findings_survive_development_configuration_failures() {
        let fixture = CandidateFixture::init("");
        let root = super::workspace_root().unwrap();
        for path in super::REQUIRED_REPOSITORY_FILES.iter().copied().chain([
            "Justfile",
            "lefthook.yml",
            "mise.toml",
            "rust-toolchain.toml",
        ]) {
            let destination = fixture.0.join(path);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(root.join(path), destination).unwrap();
        }
        assert_eq!(
            super::collect_repository_violations_at(&fixture.0),
            Vec::<String>::new()
        );
        std::fs::remove_file(fixture.0.join("SECURITY.md")).unwrap();
        let path = fixture.0.join("mise.toml");
        let mise = std::fs::read_to_string(&path).unwrap();
        assert!(mise.lines().any(|line| line.starts_with("shellcheck =")));
        std::fs::write(
            path,
            mise.lines()
                .filter(|line| !line.starts_with("shellcheck ="))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let findings = super::collect_repository_violations_at(&fixture.0);
        assert!(findings.iter().any(|finding| finding.contains("SECURITY.md: required repository file is missing")));
        assert!(
            findings
                .iter()
                .any(|finding| finding.contains("shellcheck"))
        );
        std::fs::remove_file(fixture.0.join("Justfile")).unwrap();
        let findings = super::collect_repository_violations_at(&fixture.0);
        assert!(findings.iter().any(|finding| finding.contains("SECURITY.md: required repository file is missing")));
        assert!(
            findings
                .iter()
                .any(|finding| finding.contains("Justfile: required repository file is missing"))
        );
        std::fs::create_dir(fixture.0.join("SECURITY.md")).unwrap();
        let findings = super::collect_repository_violations_at(&fixture.0);
        assert!(findings.iter().any(|finding| {
            finding.contains("SECURITY.md: required repository file could not be read")
        }));
    }

    #[test]
    fn candidate_platform_tests_install_native_hook_helpers() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        let broken = candidate.replace("taplo typos cargo-binstall", "taplo cargo-binstall");
        assert!(
            super::ci_source_violations(&ci, &broken, &justfile, &hooks)
                .iter()
                .any(|violation| violation.contains("platform tests require typos"))
        );
    }

    #[test]
    fn candidate_platform_tools_must_precede_test_execution() {
        let candidate = include_str!("../../.github/workflows/candidate.yml");
        let platform = super::workflow_job(candidate, "platform").unwrap();
        let setup = platform
            .lines()
            .find(|line| line.trim().starts_with("install_args:"))
            .unwrap();
        let arguments = setup.trim().strip_prefix("install_args:").unwrap();
        for tool in [
            "rust",
            "just",
            "lefthook",
            "taplo",
            "typos",
            "cargo:cargo-nextest",
        ] {
            let remaining = arguments
                .split_whitespace()
                .filter(|held| *held != tool)
                .collect::<Vec<_>>()
                .join(" ");
            let without = platform.replace(setup, &format!("          install_args: {remaining}"));
            let step = format!(
                "      - uses: jdx/mise-action@fixture\n        with:\n          install_args: {tool}\n"
            );
            let later = format!("{without}{step}");
            assert!(
                !super::platform_test_tool_violations(&candidate.replace(platform, &later))
                    .is_empty(),
                "a later setup supplied {tool}"
            );
            let earlier = without.replace(
                "      - run: mise x -- just test-ci",
                &format!("{step}      - run: mise x -- just test-ci"),
            );
            assert_eq!(
                super::platform_test_tool_violations(&candidate.replace(platform, &earlier)),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn ordinary_ci_does_not_start_candidate_qualification() {
        let root = super::workspace_root().unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let ordinary = justfile
            .lines()
            .find(|line| line.starts_with("check:"))
            .unwrap();
        for required in [
            "test-ci",
            "allocation",
            "doc",
            "deny",
            "architecture",
            "repository",
        ] {
            assert!(
                ordinary.split_whitespace().any(|word| word == required),
                "missing {required}"
            );
        }
        for candidate in ["feature-matrix", "no-std", "wasm", "public-api", "msrv"] {
            assert!(
                !ordinary.split_whitespace().any(|word| word == candidate),
                "ordinary invokes {candidate}"
            );
        }
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        assert!(
            !ci.contains("\n  push:"),
            "ordinary CI duplicates main pushes"
        );
        assert!(
            !ci.contains("\n  ci-required:"),
            "obsolete summary duplicates required"
        );
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        let push = hooks
            .split_once("\npre-push:")
            .unwrap()
            .1
            .split_once("\nlocal-ci:")
            .unwrap()
            .0;
        assert!(push.contains("run: mise exec -- just check"));
        assert!(!push.contains("just ci"));
        let commit = hooks
            .split_once("\npre-commit:\n")
            .unwrap()
            .1
            .split_once("\npre-push:\n")
            .unwrap()
            .0;
        for candidate in [
            "ci",
            "public-api",
            "feature-matrix",
            "no-std",
            "wasm",
            "msrv",
            "prove-ci",
        ] {
            assert!(
                !commit.contains(&format!("just {candidate}")),
                "pre-commit invokes {candidate}"
            );
        }
    }

    #[test]
    fn ci_summary_requires_exact_successful_inventory() {
        for phase in [super::CiPhase::Ordinary, super::CiPhase::Candidate] {
            let name = match phase {
                super::CiPhase::Ordinary => "ordinary",
                super::CiPhase::Candidate => "candidate",
            };
            let mut arguments = vec![name.to_owned()];
            for job in phase.jobs() {
                arguments.extend([job.label().to_owned(), "success".to_owned()]);
            }
            assert_eq!(super::ci_summary(&arguments), Vec::<String>::new());
            for job in phase.jobs() {
                for state in ["failure", "cancelled", "skipped", "", "unknown"] {
                    let mut broken = vec![name.to_owned()];
                    for actual in phase.jobs() {
                        broken.extend([
                            actual.label().to_owned(),
                            if actual == job { state } else { "success" }.to_owned(),
                        ]);
                    }
                    assert!(
                        !super::ci_summary(&broken).is_empty(),
                        "accepted {}={state}",
                        job.label()
                    );
                }
            }
            let mut extra = arguments.clone();
            extra.extend(["extra".to_owned(), "success".to_owned()]);
            assert_ne!(super::ci_summary(&extra), Vec::<String>::new());
            arguments.pop();
            assert_ne!(super::ci_summary(&arguments), Vec::<String>::new());
            arguments.pop();
            assert_ne!(super::ci_summary(&arguments), Vec::<String>::new());
        }
        assert_ne!(super::ci_summary(&[]), Vec::<String>::new());
        assert_ne!(
            super::ci_summary(&["unknown".to_owned()]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn candidate_revision_requires_a_complete_matching_commit() {
        let first = "0123456789abcdef0123456789abcdef01234567";
        let second = "0123456789abcdef0123456789abcdef01234568";
        let revision = super::CandidateRevision::parse(first).unwrap();
        assert!(revision.matches(revision));
        assert!(!revision.matches(super::CandidateRevision::parse(second).unwrap()));
        for invalid in [
            "",
            "main",
            "0123456",
            "0123456789abcdef0123456789abcdef0123456g",
            "0123456789abcdef0123456789abcdef012345678",
        ] {
            assert!(
                super::CandidateRevision::parse(invalid).is_none(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn ci_decisions_compile_in_the_standalone_proof_wrapper() {
        let fixture = CandidateFixture::new();
        std::fs::write(fixture.0.join("production.rs"), include_str!("ci.rs")).unwrap();
        let wrapper = fixture.0.join("lib.rs");
        std::fs::write(&wrapper, b"#[path = \"production.rs\"]\nmod production;\n").unwrap();
        for edition in ["2015", "2024"] {
            let output = std::process::Command::new("rustc")
                .args([
                    "--crate-type",
                    "lib",
                    "--emit",
                    "metadata",
                    "--edition",
                    edition,
                ])
                .arg(&wrapper)
                .arg("-o")
                .arg(fixture.0.join(format!("proof-{edition}.rmeta")))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "standalone proof source failed in edition {edition}:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    struct CandidateFixture(std::path::PathBuf);

    fn isolate_fixture_git_environment(command: &mut std::process::Command) {
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
    }

    impl CandidateFixture {
        fn new() -> Self {
            Self::with_files(&[("source.txt", b"original\n")])
        }

        fn with_files(files: &[(&str, &[u8])]) -> Self {
            let fixture = Self::init("");
            for (name, contents) in files {
                let path = fixture.0.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, contents).unwrap();
            }
            fixture.git(&["add", "--all"]);
            let tree = fixture.git(&["write-tree"]);
            let commit = fixture.git(&[
                "-c",
                "user.name=CI fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                &tree,
                "-m",
                "test: candidate fixture",
            ]);
            fixture.git(&["update-ref", "HEAD", &commit]);
            fixture
        }

        fn init(configuration: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("icalkit-candidate-{}-{serial}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            let fixture = Self(path);
            fixture.git(&[
                "-c",
                "init.defaultObjectFormat=sha256",
                "init",
                "--quiet",
                "--object-format=sha1",
            ]);
            if !configuration.is_empty() {
                let path = fixture.0.join(".git/config");
                let mut contents = std::fs::read_to_string(&path).unwrap();
                contents.push_str(configuration);
                std::fs::write(path, contents).unwrap();
            }
            fixture.git(&["config", "--local", "core.fsmonitor", "false"]);
            fixture
        }

        fn git(&self, arguments: &[&str]) -> String {
            let mut command = std::process::Command::new("git");
            isolate_fixture_git_environment(&mut command);
            let output = command
                .args(arguments)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn revision(&self) -> super::CandidateRevision {
            super::CandidateRevision::parse(&self.git(&["rev-parse", "HEAD"])).unwrap()
        }
    }

    impl Drop for CandidateFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn candidate_fixture_turns_off_file_system_monitoring_before_index_creation() {
        let fixture = CandidateFixture::init("\n[core]\nfsmonitor = true\n");
        assert_eq!(
            fixture.git(&["config", "--bool", "core.fsmonitor"]),
            "false"
        );
        assert!(!fixture.0.join(".git/index").exists());
    }

    #[test]
    fn candidate_fixture_uses_the_sha1_commit_contract() {
        let fixture = CandidateFixture::init("");
        assert_eq!(fixture.git(&["hash-object", "--stdin"]).len(), 40);
    }

    #[test]
    fn candidate_checkout_rejects_modified_tracked_source() {
        let fixture = CandidateFixture::new();
        let revision = fixture.revision();
        assert_eq!(
            super::candidate_checkout(&fixture.0, revision).unwrap(),
            Vec::<String>::new()
        );
        std::fs::write(fixture.0.join("source.txt"), b"changed\n").unwrap();
        let refused = super::candidate_checkout(&fixture.0, revision).unwrap();
        assert!(
            refused
                .iter()
                .any(|message| message.contains("tracked source")),
            "{refused:?}"
        );
        fixture.git(&["add", "source.txt"]);
        assert_ne!(
            super::candidate_checkout(&fixture.0, revision).unwrap(),
            Vec::<String>::new()
        );
        std::fs::write(fixture.0.join("source.txt"), b"original\n").unwrap();
        assert_ne!(
            super::candidate_checkout(&fixture.0, revision).unwrap(),
            Vec::<String>::new(),
            "accepted staged changes when working-tree bytes equal HEAD"
        );
        fixture.git(&["add", "source.txt"]);
        assert_eq!(
            super::candidate_checkout(&fixture.0, revision).unwrap(),
            Vec::<String>::new()
        );
        let tree = fixture.git(&["write-tree"]);
        let parent = fixture.git(&["rev-parse", "HEAD"]);
        let changed = fixture.git(&[
            "-c",
            "user.name=CI fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "test: changed candidate identity",
        ]);
        fixture.git(&["update-ref", "HEAD", &changed]);
        let refused = super::candidate_checkout(&fixture.0, revision).unwrap();
        assert!(
            refused
                .iter()
                .any(|message| message.contains("commit differs")),
            "{refused:?}"
        );
    }

    fn candidate_flag_preserves_working_file_checks(flag: &str) {
        let fixture = CandidateFixture::new();
        let revision = fixture.revision();
        fixture.git(&["update-index", flag, "source.txt"]);
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        let cases: [Option<&[u8]>; 5] = [
            Some(b"original\n"),
            Some(b"changed\n"),
            Some(b""),
            None,
            Some(b"original\n"),
        ];
        for contents in cases {
            let path = fixture.0.join("source.txt");
            if let Some(contents) = contents {
                std::fs::write(path, contents).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
            let result = super::candidate_checkout(&fixture.0, revision).unwrap();
            assert_eq!(
                result.is_empty(),
                contents == Some(b"original\n".as_slice()),
                "{flag}, {contents:?}: {result:?}"
            );
            assert_eq!(
                std::fs::read(fixture.0.join(".git/index")).unwrap(),
                index,
                "changed the caller's index for {flag}",
            );
        }
    }

    #[test]
    fn candidate_checkout_reads_assumed_unchanged_files() {
        candidate_flag_preserves_working_file_checks("--assume-unchanged");
    }

    #[test]
    fn candidate_checkout_reads_skip_worktree_files() {
        candidate_flag_preserves_working_file_checks("--skip-worktree");
    }

    #[test]
    fn candidate_checkout_reads_content_despite_matching_cached_metadata() {
        let fixture = CandidateFixture::new();
        let revision = fixture.revision();
        fixture.git(&["config", "core.trustctime", "false"]);
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let set_modified = || {
            std::fs::File::options()
                .write(true)
                .open(fixture.0.join("source.txt"))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        };
        set_modified();
        fixture.git(&["add", "source.txt"]);
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        std::fs::write(fixture.0.join("source.txt"), b"modified\n").unwrap();
        set_modified();
        let result = super::candidate_checkout(&fixture.0, revision).unwrap();
        assert!(
            !result.is_empty(),
            "accepted content hidden by cached metadata"
        );
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn candidate_checkout_handles_flagged_paths_with_spaces_and_unicode() {
        let names = [
            "folder/with spaces.txt",
            "folder/\u{65e5}\u{672c}.txt",
            #[cfg(unix)]
            "folder/line\nbreak.txt",
            #[cfg(unix)]
            "folder/quote\"name.txt",
        ];
        let files: Vec<_> = names
            .iter()
            .map(|name| (*name, b"original\n".as_slice()))
            .collect();
        let fixture = CandidateFixture::with_files(&files);
        let revision = fixture.revision();
        for name in names {
            fixture.git(&["update-index", "--assume-unchanged", "--", name]);
            let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
            std::fs::write(fixture.0.join(name), b"changed\n").unwrap();
            let result = super::candidate_checkout(&fixture.0, revision).unwrap();
            assert!(!result.is_empty(), "accepted changed path {name:?}");
            assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
            std::fs::write(fixture.0.join(name), b"original\n").unwrap();
            assert_eq!(
                super::candidate_checkout(&fixture.0, revision).unwrap(),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn candidate_checkout_preserves_linked_worktree_support() {
        let fixture = CandidateFixture::new();
        let linked = CandidateFixture(fixture.0.join("linked"));
        fixture.git(&[
            "worktree",
            "add",
            "--detach",
            "--quiet",
            linked.0.to_str().unwrap(),
        ]);
        let revision = linked.revision();
        assert_eq!(
            super::candidate_checkout(&linked.0, revision).unwrap(),
            Vec::<String>::new()
        );
        std::fs::write(linked.0.join("source.txt"), b"changed\n").unwrap();
        assert_ne!(
            super::candidate_checkout(&linked.0, revision).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            super::candidate_checkout(&fixture.0, revision).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn candidate_checkout_cleans_scratch_storage_after_success_and_git_failure() {
        let fixture = CandidateFixture::with_files(&[
            ("source.txt", b"original\n"),
            (".gitattributes", b"source.txt filter=broken\n"),
        ]);
        let revision = fixture.revision();
        let scratch = fixture.0.join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        assert_eq!(
            super::candidate::inspect(&fixture.0, revision, &scratch).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);

        fixture.git(&["config", "filter.broken.required", "true"]);
        fixture.git(&[
            "config",
            "filter.broken.clean",
            "git rev-parse --verify refs/icalkit-filter-failure",
        ]);
        std::fs::write(fixture.0.join("source.txt"), b"changed\n").unwrap();
        assert!(super::candidate::inspect(&fixture.0, revision, &scratch).is_err());
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn candidate_checkout_resolves_relative_scratch_paths_before_changing_repository() {
        let fixture = CandidateFixture::new();
        assert_eq!(
            super::candidate::inspect(&fixture.0, fixture.revision(), std::path::Path::new("."))
                .unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn candidate_checkout_does_not_run_index_hooks_for_its_scratch_index() {
        let fixture = CandidateFixture::new();
        let hooks = fixture.0.join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("post-index-change");
        std::fs::write(&hook, "#!/bin/sh\nprintf 'called\\n' > hook-called\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        fixture.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
        fixture.git(&["read-tree", "HEAD"]);
        let marker = fixture.0.join("hook-called");
        assert!(marker.exists(), "the hook control did not execute");
        std::fs::remove_file(&marker).unwrap();
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        assert_eq!(
            super::candidate::inspect(&fixture.0, fixture.revision(), &fixture.0).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            !marker.exists(),
            "scratch index creation ran a repository hook"
        );
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn candidate_checkout_refuses_unavailable_scratch_storage() {
        let fixture = CandidateFixture::new();
        let revision = fixture.revision();
        let scratch = fixture.0.join("scratch");
        std::fs::write(&scratch, b"not a directory\n").unwrap();
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        assert!(super::candidate::inspect(&fixture.0, revision, &scratch).is_err());
        assert_eq!(std::fs::read(&scratch).unwrap(), b"not a directory\n");
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn candidate_checkout_cleans_scratch_storage_when_the_tree_cannot_be_read() {
        let fixture = CandidateFixture::new();
        let revision = fixture.revision();
        let scratch = fixture.0.join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let index = std::fs::read(fixture.0.join(".git/index")).unwrap();
        let tree = fixture.git(&["rev-parse", "HEAD^{tree}"]);
        let (prefix, suffix) = tree.split_at(2);
        std::fs::rename(
            fixture.0.join(".git/objects").join(prefix).join(suffix),
            fixture.0.join("hidden-tree"),
        )
        .unwrap();
        let error = super::candidate::inspect(&fixture.0, revision, &scratch).unwrap_err();
        assert_eq!(error.to_string(), "git could not read the candidate tree");
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        assert_eq!(std::fs::read(fixture.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn candidate_checkout_ignores_foreign_git_environment() {
        for (variable, suffix) in [
            ("GIT_INDEX_FILE", ".git/index"),
            ("GIT_DIR", ".git"),
            ("GIT_WORK_TREE", ""),
        ] {
            let foreign = CandidateFixture::new();
            std::fs::write(foreign.0.join("foreign.txt"), b"foreign\n").unwrap();
            foreign.git(&["add", "foreign.txt"]);
            let index = std::fs::read(foreign.0.join(".git/index")).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            isolate_fixture_git_environment(&mut command);
            let output = command
                .args([
                    "--exact",
                    "tests::candidate_checkout_rejects_modified_tracked_source",
                    "--nocapture",
                ])
                .env(variable, foreign.0.join(suffix))
                .env(
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                    foreign.0.join(".git/objects"),
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{variable}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            assert_eq!(
                std::fs::read(foreign.0.join(".git/index")).unwrap(),
                index,
                "changed the caller's index through {variable}",
            );
        }
    }

    #[test]
    fn candidate_inventory_requires_both_platforms_and_every_msrv() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        for removed in ["windows-2025, ", ", macos-15"] {
            let broken = candidate.replace(removed, "");
            assert_ne!(
                super::ci_source_violations(&ci, &broken, &justfile, &hooks),
                Vec::<String>::new()
            );
        }
        for path in [
            "crates/icalkit",
            "crates/icalkit-conformance",
            "gates/grammar-layering",
            "gates/xml-layering",
            "xtask",
        ] {
            let broken = justfile.replace(&format!("    cargo msrv verify --path {path}\n"), "");
            assert_ne!(
                super::ci_source_violations(&ci, &candidate, &broken, &hooks),
                Vec::<String>::new(),
                "missing MSRV {path}"
            );
        }
    }

    #[test]
    fn candidate_source_checks_bracket_commands_and_acquire_the_pinned_helper() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        let action = "P4suta/project-template/.github/actions/ci-policy@ee6265bd944c53097191f97dcb7a6ac678794b43";
        let proof = super::workflow_job(&candidate, "proof").unwrap();
        assert!(
            proof.contains(action),
            "missing immutable policy helper acquisition"
        );
        let broken = candidate.replace(
            action,
            "P4suta/project-template/.github/actions/ci-policy@main",
        );
        assert_ne!(
            super::ci_source_violations(&ci, &broken, &justfile, &hooks),
            Vec::<String>::new()
        );
        for job in super::CANDIDATE_JOBS
            .iter()
            .map(|job| job.label())
            .chain(["candidate-required"])
        {
            let body = super::workflow_job(&candidate, job).unwrap();
            let marker = "candidate-revision \"$CANDIDATE_REVISION\"";
            assert_eq!(
                body.matches(marker).count(),
                2,
                "missing post-check identity for {job}"
            );
            let last = body.rfind(marker).unwrap();
            let mut broken_body = body.to_owned();
            broken_body.replace_range(
                last..last.checked_add(marker.len()).unwrap(),
                "unrelated-command",
            );
            let broken = candidate.replacen(body, &broken_body, 1);
            assert_ne!(
                super::ci_source_violations(&ci, &broken, &justfile, &hooks),
                Vec::<String>::new(),
                "accepted missing post-check identity for {job}"
            );
        }
    }

    #[test]
    fn candidate_proofs_require_both_sources_inventories_and_counterexamples() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        for source in [
            "xtask/src/ci.rs",
            "crates/icalkit-conformance/src/bin/allocation_gate/arithmetic.rs",
        ] {
            let source_argument = format!("--source {source} ");
            let command = justfile
                .lines()
                .find(|line| line.contains(&source_argument))
                .expect("every production proof source needs its verification command");
            let (_, inventory) = command.split_once("--require-json ").unwrap();
            let (inventory, probe) = inventory.split_once(" --counterexample ").unwrap();
            for missing in [
                command.to_owned(),
                source_argument,
                format!("--require-json {inventory}"),
                format!("--counterexample {probe}"),
            ] {
                let broken = justfile.replacen(&missing, "", 1);
                assert_ne!(
                    super::ci_source_violations(&ci, &candidate, &broken, &hooks),
                    Vec::<String>::new(),
                    "accepted missing proof contract {missing}"
                );
            }
        }
    }

    #[test]
    fn actual_ci_sources_bind_complete_phases_and_candidate_checkouts() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        assert_eq!(
            super::ci_source_violations(&ci, &candidate, &justfile, &hooks),
            Vec::<String>::new()
        );
        for broken in [
            candidate.replace("  platform:\n", "  unexpected-platform:\n"),
            candidate.replace("ref: ${{ inputs.revision }}", "ref: main"),
            candidate.replace("ICALKIT_RELEASE_VERSION: ${{ inputs.release_version }}", ""),
            candidate.replace("git merge-base --is-ancestor", "git log"),
            candidate.replace(
                "candidate-revision \"$CANDIDATE_REVISION\"",
                "unrelated-command",
            ),
            candidate.replace(
                "needs: [ordinary, platform, features, portability, public-api, msrv, proof]",
                "needs: [ordinary]",
            ),
            candidate.replace(
                "PLATFORM_RESULT: ${{ needs.platform.result }}",
                "PLATFORM_RESULT: success",
            ),
            candidate.replace(
                "run: mise x -- just feature-matrix",
                "run: mise x -- just test-ci",
            ),
        ] {
            assert_ne!(
                super::ci_source_violations(&ci, &broken, &justfile, &hooks),
                Vec::<String>::new()
            );
        }
        for command in ["test-ci", "allocation", "doc", "deny", "repository"] {
            let ordinary = justfile
                .lines()
                .find(|line| line.starts_with("check:"))
                .unwrap();
            let broken =
                justfile.replacen(ordinary, &ordinary.replace(&format!(" {command}"), ""), 1);
            assert!(
                !super::ci_source_violations(&ci, &candidate, &broken, &hooks).is_empty(),
                "missing {command}"
            );
        }
    }

    #[test]
    fn candidate_requires_successful_packaged_tests_between_source_checks() {
        let root = super::workspace_root().unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let candidate =
            std::fs::read_to_string(root.join(".github/workflows/candidate.yml")).unwrap();
        let justfile = std::fs::read_to_string(root.join(super::JUSTFILE)).unwrap();
        let hooks = std::fs::read_to_string(root.join("lefthook.yml")).unwrap();
        let command = "      - run: mise x -- just package-check\n";
        let omitted = candidate.replace(command, "");
        let moved = omitted.replacen("\n  platform:\n", &format!("{command}\n  platform:\n"), 1);
        let nonfatal_step = candidate.replace(
            command,
            &format!("{command}        continue-on-error: true\n"),
        );
        let nonfatal_job = candidate.replacen(
            "  ordinary:\n",
            "  ordinary:\n    continue-on-error: true\n",
            1,
        );
        for unchecked in [omitted, moved, nonfatal_step, nonfatal_job] {
            assert!(
                !super::ci_source_violations(&ci, &unchecked, &justfile, &hooks).is_empty(),
                "accepted a candidate without mandatory package checks between source checks"
            );
        }
    }

    #[test]
    fn release_proposals_cannot_enable_publication() {
        let root = super::workspace_root().unwrap();
        let proposal = std::fs::read_to_string(root.join(super::RELEASE_CONFIG)).unwrap();
        let violations = super::proposal_config_violations;
        assert_eq!(violations(&proposal), Vec::<String>::new());
        for key in ["publish", "git_tag_enable", "git_release_enable"] {
            let enabled = format!("{key} = true");
            let workspace = proposal.replacen(&format!("{key} = false"), &enabled, 1);
            let overridden = format!("{proposal}\n{enabled}\n");
            assert!(!violations(&workspace).is_empty(), "accepted {enabled}");
            assert!(
                !violations(&overridden).is_empty(),
                "accepted package override for {key}"
            );
        }
    }

    #[test]
    fn dav_body_encoders_cannot_recreate_xml_structure_helpers() {
        let shared = "use crate::internal::dav::writer::XmlWriter;\n";
        assert_eq!(xml_writer_violations(shared, shared), Vec::<String>::new());

        let violations = xml_writer_violations(
            "fn write_name() {}\n",
            "use crate::internal::dav::writer::XmlWriter;\nfn open_extension() {}\n",
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("does not use"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("write_name"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("open_extension"))
        );
    }

    #[test]
    fn current_status_rejects_historical_debt_as_live_work() {
        let current_roadmap = "## Maintained implementation\n";
        assert_eq!(
            closure_status_violations(
                current_roadmap,
                "private modules are connected through workflows",
                "# Current evidence boundary"
            ),
            Vec::<String>::new()
        );

        let violations = closure_status_violations(
            "Two are still owed: a hostile input of 200,000",
            "including units not yet reached",
            "# What is still owed here",
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("missing maintained implementation marker"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("completed work"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unconnected"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unsupported capture claim"))
        );
    }

    #[test]
    fn public_guidance_follows_the_typestate_and_workflow_order() {
        let readme = "## Strict parsing\n## Explicit normalization\n## Transactional editing\n\
                      ## Recurrence across daylight saving time\n## iTIP scheduling\n\
                      ## Calendar synchronization and server workflows\n";
        assert_eq!(
            documentation_violations(readme, "private implementation modules"),
            Vec::<String>::new()
        );

        let stale = documentation_violations(
            "## Explicit normalization\n## Strict parsing\n",
            "one temporary path dependency",
        );
        assert!(
            stale
                .iter()
                .any(|violation| violation.contains("out of golden-path order"))
        );
        assert!(
            stale
                .iter()
                .any(|violation| violation.contains("retired migration prose"))
        );
    }

    #[test]
    fn the_basic_example_keeps_internal_vocabulary_out_of_the_golden_path() {
        assert_eq!(
            basic_example_violations("use icalkit::Calendar;"),
            Vec::<String>::new()
        );
        let violations = basic_example_violations(
            "use icalkit::Calendar; fn hidden(_: Limits, _: &mut Meter) {}",
        );
        assert_eq!(violations.len(), 2);
        assert!(violations.iter().any(|line| line.contains("`Limits`")));
        assert!(violations.iter().any(|line| line.contains("`Meter`")));
    }

    #[test]
    fn a_public_api_snapshot_is_exact_and_names_the_first_changed_line() {
        assert_eq!(
            snapshot_violations("default", "pub struct A;\n", "pub struct A;\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            snapshot_violations(
                "no-default",
                "pub struct A;\npub struct B;\n",
                "pub struct A;\npub struct C;\n",
            ),
            vec![
                "api/icalkit.no-default.txt: public API differs at line 2: expected `pub struct B;`, generated `pub struct C;`"
                    .to_owned()
            ]
        );
    }

    const SAMPLE_DECLARATIONS: &str = r#"
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The specification was violated. The input was kept anyway.
    Violation,
}

#[non_exhaustive]
pub enum DiagnosticCode {
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A `^` was followed by an octet RFC 6868 gives no meaning.
    ///
    /// A note rather than a violation: RFC 6868 section 2 requires the pair to be left as it
    /// is, so the octets are what they were.
    UndefinedCaretEscape,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingValueSeparator => "missing-value-separator",
            Self::UndefinedCaretEscape => "undefined-caret-escape",
        }
    }
}
"#;

    const SAMPLE_ADDED: &str = r#"
pub enum Severity {
    /// Something worth recording that the specification permits.
    Note,
    /// The specification was violated. The input was kept anyway.
    Violation,
}

#[non_exhaustive]
pub enum DiagnosticCode {
    /// A content line carried no `:`, so it has a name and no value.
    MissingValueSeparator,
    /// A `^` was followed by an octet RFC 6868 gives no meaning.
    ///
    /// A note rather than a violation: RFC 6868 section 2 requires the pair to be left as it
    /// is, so the octets are what they were.
    UndefinedCaretEscape,
    /// An `END` arrived with no `BEGIN` open.
    UnmatchedEnd,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingValueSeparator => "missing-value-separator",
            Self::UndefinedCaretEscape => "undefined-caret-escape",
            Self::UnmatchedEnd => "unmatched-end",
        }
    }
}
"#;

    const SAMPLE_GOLDEN: &str = "\
# Diagnostic codes

Prose, and a `|` in it, above the table.

```text
| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
```
";

    fn edited(document: &str, from: &str, to: &str) -> String {
        assert!(
            document.contains(from),
            "the sample does not carry `{from}`"
        );
        document.replace(from, to)
    }

    fn golden_with_added_row() -> String {
        format!(
            "{}| unmatched-end | An `END` arrived with no `BEGIN` open. | Violation \
             | M0 |\n```\n",
            SAMPLE_GOLDEN.strip_suffix("```\n").unwrap()
        )
    }

    #[test]
    fn registry_tables_require_a_closed_literal_block() {
        let unquoted = SAMPLE_GOLDEN.replace("```text\n", "").replace("```\n", "");
        assert!(golden_rows(&unquoted).is_err());
        let unclosed = SAMPLE_GOLDEN.strip_suffix("```\n").unwrap();
        assert!(golden_rows(unclosed).is_err());
    }

    #[test]
    fn registry_tables_ignore_prose_tables_outside_the_literal_block() {
        let source =
            format!("| unrelated | table |\n| --- | --- |\n| data | data |\n\n{SAMPLE_GOLDEN}");
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &source),
            Vec::<String>::new()
        );
    }

    #[test]
    fn registry_tables_reject_multiple_authoritative_blocks() {
        let source = format!("{SAMPLE_GOLDEN}\n{SAMPLE_GOLDEN}");
        assert!(golden_rows(&source).is_err());
    }

    #[test]
    fn registry_tables_ignore_html_comment_blocks() {
        for opening in ["<!--", " <!-- hidden", "   <!-- hidden"] {
            let hidden = format!("{opening}\n{SAMPLE_GOLDEN}-->\n");
            assert!(
                golden_rows(&hidden).is_err(),
                "a comment supplied the registry"
            );
            assert_eq!(
                codes_violations(SAMPLE_DECLARATIONS, &format!("{hidden}\n{SAMPLE_GOLDEN}")),
                Vec::<String>::new()
            );
        }
        let example = format!("````markdown\n<!--\n````\n{SAMPLE_GOLDEN}");
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &example),
            Vec::<String>::new()
        );
        let single = format!("<!-- one line -->\n{SAMPLE_GOLDEN}");
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &single),
            Vec::<String>::new()
        );
    }

    #[test]
    fn registry_tables_ignore_nested_and_quoted_fence_examples() {
        for (opening, closing) in [
            ("````markdown", "````"),
            ("~~~markdown", "~~~"),
            ("`````rust", "`````"),
        ] {
            let example = format!("{opening}\n{SAMPLE_GOLDEN}{closing}\n");
            assert!(
                golden_rows(&example).is_err(),
                "an example became the registry"
            );
            assert_eq!(
                codes_violations(SAMPLE_DECLARATIONS, &format!("{example}\n{SAMPLE_GOLDEN}")),
                Vec::<String>::new()
            );
        }
        let mut quoted = String::new();
        for line in SAMPLE_GOLDEN.lines() {
            quoted.push_str("> ");
            quoted.push_str(line);
            quoted.push('\n');
        }
        assert!(golden_rows(&quoted).is_err());
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &format!("{quoted}\n{SAMPLE_GOLDEN}")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn registry_tables_follow_fence_markers_and_minimum_closing_lengths() {
        for (opening, closing) in [("````text", "`````"), ("~~~text", "~~~~")] {
            let source = SAMPLE_GOLDEN.replace("```text", opening);
            let source = format!("{}{closing}\n", source.strip_suffix("```\n").unwrap());
            assert_eq!(
                codes_violations(SAMPLE_DECLARATIONS, &source),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn a_list_that_mirrors_the_declarations_is_clean() {
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, SAMPLE_GOLDEN),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_committed_list_matches_the_committed_declarations() {
        assert_eq!(collect_codes_violations().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn empty_inputs_are_reported_rather_than_passing_for_free() {
        let violations = codes_violations("", "");
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `DiagnosticCode` variant")),
            "a scan that matched nothing must say so: {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `Severity` variant")),
            "and must say so for the channel vocabulary too: {violations:?}"
        );
    }

    #[test]
    fn an_added_code_that_carries_a_row_passes_trivially() {
        assert_eq!(
            codes_violations(SAMPLE_ADDED, &golden_with_added_row()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_code_with_no_row_is_a_violation() {
        let violations = codes_violations(SAMPLE_ADDED, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`unmatched-end`") && line.contains("has no row")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_row_with_no_code_is_a_violation() {
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden_with_added_row());
        assert!(
            violations
                .iter()
                .any(|line| line.contains("no `DiagnosticCode` declares it")),
            "{violations:?}"
        );
    }

    #[test]
    fn an_edited_meaning_fails_from_whichever_side_it_was_edited() {
        let drifted = "so it has no value.";
        let listed = edited(SAMPLE_GOLDEN, "so it has a name and no value.", drifted);
        let documented = edited(
            SAMPLE_DECLARATIONS,
            "so it has a name and no value.",
            drifted,
        );
        for violations in [
            codes_violations(SAMPLE_DECLARATIONS, &listed),
            codes_violations(&documented, SAMPLE_GOLDEN),
        ] {
            assert!(
                violations
                    .iter()
                    .any(|line| line.contains("frozen as hard as a name")),
                "the two copies of a meaning are what freeze it: {violations:?}"
            );
        }
    }

    #[test]
    fn a_rename_carried_through_both_files_passes() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "\"undefined-caret-escape\"",
            "\"undefined-caret-escape-v2\"",
        );
        let golden = edited(
            SAMPLE_GOLDEN,
            "| undefined-caret-escape |",
            "| undefined-caret-escape-v2 |",
        );
        assert_eq!(
            codes_violations(&declarations, &golden),
            Vec::<String>::new(),
            "a rename is how a meaning is allowed to change (ADR 0009)"
        );
    }

    #[test]
    fn a_channel_that_is_not_a_severity_is_a_violation() {
        let golden = edited(SAMPLE_GOLDEN, "| Note | M0 |", "| Warning | M0 |");
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`Warning`") && line.contains("not a `Severity`")),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|line| line.contains("Severity::Note")),
            "retiring the last row on a severity leaves that severity with no emitter: \
             {violations:?}"
        );
    }

    #[test]
    fn a_severity_no_code_travels_on_is_a_violation() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "    Violation,\n}",
            "    Violation,\n    /// Cut short at a caller-stated bound.\n    LimitReached,\n}",
        );
        let violations = codes_violations(&declarations, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("Severity::LimitReached")),
            "this is the debt the milestone column exists to keep closed: {violations:?}"
        );
    }

    #[test]
    fn a_milestone_the_roadmap_does_not_name_is_a_violation() {
        let golden = edited(SAMPLE_GOLDEN, "| Violation | M0 |", "| Violation | later |");
        let violations = codes_violations(SAMPLE_DECLARATIONS, &golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("`later`") && line.contains("ROADMAP.md")),
            "{violations:?}"
        );
    }

    #[test]
    fn rows_out_of_declaration_order_are_a_violation() {
        let golden = "\
```text
| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
```
";
        let violations = codes_violations(SAMPLE_DECLARATIONS, golden);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("not in declaration order")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_wrapped_first_paragraph_is_one_meaning_and_the_rest_is_free_prose() {
        let source = "\
pub enum DiagnosticCode {
    /// A recurrence rule generated an instance whose date does not exist, so it was
    /// filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one.
    ///
    /// Reported so that the two answers stay different.
    NonexistentRecurrenceInstance,
}
";
        assert_eq!(
            enum_variants(source, "DiagnosticCode"),
            vec![(
                "NonexistentRecurrenceInstance".to_owned(),
                "A recurrence rule generated an instance whose date does not exist, so it was \
                 filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one."
                    .to_owned()
            )]
        );
    }

    #[test]
    fn the_arm_scan_stops_at_the_end_of_as_str() {
        let source = r#"
impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnmatchedEnd => "unmatched-end",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::UnmatchedEnd => "an END with no BEGIN",
        }
    }
}
"#;
        assert_eq!(
            as_str_arms(source),
            vec![("UnmatchedEnd".to_owned(), "unmatched-end".to_owned())],
            "only `as_str` produces golden-list keys; a later method's arms are prose"
        );
    }

    #[test]
    fn a_variant_whose_arm_is_missing_is_a_violation() {
        let declarations = edited(
            SAMPLE_DECLARATIONS,
            "            Self::UndefinedCaretEscape => \"undefined-caret-escape\",\n",
            "",
        );
        let violations = codes_violations(&declarations, SAMPLE_GOLDEN);
        assert!(
            violations
                .iter()
                .any(|line| line.contains("same names in the same order")),
            "a variant with no key cannot be listed under one: {violations:?}"
        );
    }

    #[test]
    fn a_table_that_cannot_be_read_is_an_error_rather_than_a_row_of_diagnostics() {
        let swapped = edited(
            SAMPLE_GOLDEN,
            "| code | meaning | channel | milestone |",
            "| code | meaning | milestone | channel |",
        );
        assert!(
            golden_rows(&swapped).is_err(),
            "the cells are read by position, so a column swap is not a readable table"
        );

        let piped = edited(
            SAMPLE_GOLDEN,
            "so it has a name and no value.",
            "a name | no value.",
        );
        assert!(golden_rows(&piped).is_err(), "a five-cell row is not a row");
        assert_eq!(
            codes_violations(SAMPLE_DECLARATIONS, &piped).len(),
            1,
            "a file that cannot be read is one message about the file, not one per row"
        );
    }
}
