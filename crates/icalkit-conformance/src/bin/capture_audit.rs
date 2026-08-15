// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Audit one raw producer capture without copying it into the workspace.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const CAPTURE_SCHEMA: &str = "icalkit-capture/1";
const REPORT_SCHEMA: &str = "icalkit-capture-audit/1";
const GAP_SCENARIO: &str = "dst-gap-daily-series-v1";
const MAX_ARTIFACT_OCTETS: u64 = 16_777_216;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    schema: String,
    client: String,
    surface: String,
    version: String,
    account_type: AccountType,
    observed_on: String,
    ics: PathBuf,
    rendering_evidence: PathBuf,
    scenario: Scenario,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AccountType {
    Consumer,
    Organization,
}

impl AccountType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Consumer => "consumer",
            Self::Organization => "organization",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    id: String,
    outcome: GapOutcome,
    rendered_local: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum GapOutcome {
    Skipped,
    OffsetBefore,
    GapEnd,
    OffsetAfter,
    Other,
}

impl GapOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::OffsetBefore => "offset-before",
            Self::GapEnd => "gap-end",
            Self::OffsetAfter => "offset-after",
            Self::Other => "other",
        }
    }
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    status: &'static str,
    client: String,
    surface: String,
    version: String,
    account_type: &'static str,
    observed_on: String,
    scenario: &'static str,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rendered_local: Option<String>,
    source_sha256: String,
    evidence_sha256: String,
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(report) => match write_report(&report) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "capture-audit: {error}");
                ExitCode::FAILURE
            },
        },
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "capture-audit: {error}");
            ExitCode::FAILURE
        },
    }
}

fn run(arguments: impl Iterator<Item = OsString>) -> Result<Report, String> {
    let arguments: Vec<OsString> = arguments.collect();
    let [metadata] = arguments.as_slice() else {
        return Err("usage: capture-audit <outside-workspace-capture.json>".to_owned());
    };
    let metadata = fs::canonicalize(metadata)
        .map_err(|_| "capture metadata is not a readable file".to_owned())?;
    let workspace = workspace_root()?;
    if metadata.starts_with(&workspace) {
        return Err("raw capture bundle must remain outside the workspace".to_owned());
    }
    let bundle = metadata
        .parent()
        .ok_or_else(|| "capture metadata has no bundle directory".to_owned())?;
    let encoded =
        fs::read(&metadata).map_err(|_| "capture metadata is not a readable file".to_owned())?;
    let capture: Capture = serde_json::from_slice(&encoded)
        .map_err(|error| format!("invalid capture metadata at {error}"))?;
    validate_metadata(&capture)?;

    let ics_path = artifact_path(bundle, &capture.ics, "ics")?;
    let evidence_path = artifact_path(bundle, &capture.rendering_evidence, "rendering_evidence")?;
    let ics = read_artifact(&ics_path, "ICS export")?;
    let evidence = read_artifact(&evidence_path, "rendering evidence")?;
    validate_calendar(&ics)?;
    validate_image(&evidence)?;

    Ok(Report {
        schema: REPORT_SCHEMA,
        status: "ready-for-reduction",
        client: capture.client,
        surface: capture.surface,
        version: capture.version,
        account_type: capture.account_type.as_str(),
        observed_on: capture.observed_on,
        scenario: GAP_SCENARIO,
        outcome: capture.scenario.outcome.as_str(),
        rendered_local: capture.scenario.rendered_local,
        source_sha256: sha256(&ics),
        evidence_sha256: sha256(&evidence),
    })
}

fn write_report(report: &Report) -> Result<(), String> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, report).map_err(|error| error.to_string())?;
    output.write_all(b"\n").map_err(|error| error.to_string())
}

fn workspace_root() -> Result<PathBuf, String> {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = package
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot locate the workspace root".to_owned())?;
    fs::canonicalize(root).map_err(|_| "cannot locate the workspace root".to_owned())
}

fn validate_metadata(capture: &Capture) -> Result<(), String> {
    if capture.schema != CAPTURE_SCHEMA {
        return Err(format!(
            "capture schema must be {CAPTURE_SCHEMA}, not {}",
            capture.schema
        ));
    }
    let expected_surface = match capture.client.as_str() {
        "Google Calendar" => "Google Calendar Web",
        "Microsoft 365" => "Outlook Web",
        "Apple Calendar" => "Apple Calendar",
        _ => {
            return Err(
                "client must be Google Calendar, Microsoft 365, or Apple Calendar".to_owned(),
            );
        },
    };
    if capture.surface != expected_surface {
        return Err(format!(
            "{} captures must name surface {expected_surface}",
            capture.client
        ));
    }
    if capture.version.trim().is_empty() {
        return Err("version must identify the observed producer surface".to_owned());
    }
    capture
        .observed_on
        .parse::<jiff::civil::Date>()
        .map_err(|_| "observed_on must be a valid YYYY-MM-DD date".to_owned())?;
    if capture.scenario.id != GAP_SCENARIO {
        return Err(format!("scenario id must be {GAP_SCENARIO}"));
    }
    validate_outcome(&capture.scenario)
}

fn validate_outcome(scenario: &Scenario) -> Result<(), String> {
    let rendered = scenario.rendered_local.as_deref();
    match scenario.outcome {
        GapOutcome::Skipped if rendered.is_none() => Ok(()),
        GapOutcome::Skipped => Err("skipped must not carry rendered_local".to_owned()),
        GapOutcome::OffsetBefore if rendered == Some("2027-03-14T03:30:00") => Ok(()),
        GapOutcome::OffsetBefore => {
            Err("offset-before must render as 2027-03-14T03:30:00".to_owned())
        },
        GapOutcome::GapEnd if rendered == Some("2027-03-14T03:00:00") => Ok(()),
        GapOutcome::GapEnd => Err("gap-end must render as 2027-03-14T03:00:00".to_owned()),
        GapOutcome::OffsetAfter if rendered == Some("2027-03-14T01:30:00") => Ok(()),
        GapOutcome::OffsetAfter => {
            Err("offset-after must render as 2027-03-14T01:30:00".to_owned())
        },
        GapOutcome::Other
            if rendered.is_some_and(|value| !value.trim().is_empty())
                && scenario
                    .notes
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()) =>
        {
            Ok(())
        },
        GapOutcome::Other => Err("other must carry rendered_local and non-empty notes".to_owned()),
    }
}

fn artifact_path(bundle: &Path, relative: &Path, field: &str) -> Result<PathBuf, String> {
    if !matches!(
        relative.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(format!(
            "{field} must name one file inside the capture bundle"
        ));
    }
    let path = fs::canonicalize(bundle.join(relative))
        .map_err(|_| format!("{field} is not a readable file"))?;
    if path.parent() != Some(bundle) {
        return Err(format!("{field} must remain inside the capture bundle"));
    }
    Ok(path)
}

fn read_artifact(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|_| format!("{label} is not a readable file"))?;
    if !metadata.is_file() {
        return Err(format!("{label} is not a regular file"));
    }
    if metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_OCTETS {
        return Err(format!(
            "{label} must contain 1..={MAX_ARTIFACT_OCTETS} octets"
        ));
    }
    fs::read(path).map_err(|_| format!("{label} is not a readable file"))
}

fn validate_calendar(bytes: &[u8]) -> Result<(), String> {
    let lines = unfolded_lines(bytes);
    for required in [
        "BEGIN:VCALENDAR",
        "BEGIN:VEVENT",
        "END:VEVENT",
        "END:VCALENDAR",
    ] {
        if !lines.iter().any(|line| line.eq_ignore_ascii_case(required)) {
            return Err(format!("ICS export is missing {required}"));
        }
    }
    let has_marker = lines.iter().any(|line| {
        property(line, "SUMMARY").is_some_and(|(_, value)| value == "ICALKIT GAP TEST")
    });
    if !has_marker {
        return Err("ICS export is missing the ICALKIT GAP TEST summary".to_owned());
    }
    let starts_correctly = lines.iter().any(|line| {
        property(line, "DTSTART").is_some_and(|(head, value)| {
            head.to_ascii_uppercase().contains("TZID=") && value == "20270313T023000"
        })
    });
    if !starts_correctly {
        return Err("ICS export must start the zoned series at 2027-03-13T02:30:00".to_owned());
    }
    let ends_correctly = lines.iter().any(|line| {
        property(line, "DTEND").is_some_and(|(head, value)| {
            head.to_ascii_uppercase().contains("TZID=") && value == "20270313T024500"
        })
    });
    if !ends_correctly {
        return Err("ICS export must end the first occurrence at 02:45:00".to_owned());
    }
    let recurs_daily = lines.iter().any(|line| {
        property(line, "RRULE").is_some_and(|(_, value)| daily_rule_crosses_gap(value))
    });
    if !recurs_daily {
        return Err(
            "ICS export must contain a bounded daily recurrence across the spring-forward gap"
                .to_owned(),
        );
    }
    Ok(())
}

fn daily_rule_crosses_gap(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    let mut daily = false;
    let mut crosses_gap = false;
    for part in upper.split(';') {
        if part == "FREQ=DAILY" {
            daily = true;
        } else if part == "COUNT=3" {
            crosses_gap = true;
        } else if let Some(until) = part.strip_prefix("UNTIL=") {
            crosses_gap = until.get(..8).is_some_and(|date| {
                date.bytes().all(|byte| byte.is_ascii_digit()) && date >= "20270315"
            });
        }
    }
    daily && crosses_gap
}

fn unfolded_lines(bytes: &[u8]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for physical in bytes.split(|byte| *byte == b'\n') {
        let physical = physical.strip_suffix(b"\r").unwrap_or(physical);
        let text = String::from_utf8_lossy(physical);
        let remainder = text.strip_prefix(' ').or_else(|| text.strip_prefix('\t'));
        if let Some(remainder) = remainder {
            if let Some(previous) = lines.last_mut() {
                previous.push_str(remainder);
            }
        } else if !text.is_empty() {
            lines.push(text.into_owned());
        }
    }
    lines
}

fn property<'a>(line: &'a str, wanted: &str) -> Option<(&'a str, &'a str)> {
    let (head, value) = line.split_once(':')?;
    let name = head.split(';').next()?;
    name.eq_ignore_ascii_case(wanted).then_some((head, value))
}

fn validate_image(bytes: &[u8]) -> Result<(), String> {
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    let jpeg = bytes.starts_with(b"\xff\xd8\xff");
    if png || jpeg {
        Ok(())
    } else {
        Err("rendering evidence must be a PNG or JPEG image".to_owned())
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{Capture, sha256, validate_metadata};

    const EXAMPLE: &str = include_str!("../../corpus/capture.v1.example.json");

    #[test]
    fn the_digest_is_sha256_not_a_process_local_fingerprint() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_committed_capture_template_matches_the_versioned_protocol()
    -> Result<(), Box<dyn std::error::Error>> {
        let capture: Capture = serde_json::from_str(EXAMPLE)?;
        validate_metadata(&capture).map_err(std::io::Error::other)?;
        Ok(())
    }
}
