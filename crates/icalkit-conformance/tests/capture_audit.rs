// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Raw producer captures cross one deliberately narrow, non-writing intake boundary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Producer//EN\r\n\
BEGIN:VEVENT\r\n\
UID:icalkit-gap-test@example.invalid\r\n\
DTSTART;TZID=America/New_York:20270313T023000\r\n\
DTEND;TZID=America/New_York:20270313T024500\r\n\
RRULE:FREQ=DAILY;COUNT=3\r\n\
SUMMARY:ICALKIT GAP TEST\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrendered-grid";

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn outside_workspace(label: &str) -> io::Result<Self> {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "icalkit-capture-audit-{}-{serial}-{label}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        Ok(Self(directory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_bundle(
    directory: &Path,
    outcome: &str,
    rendered_local: Option<&str>,
) -> io::Result<PathBuf> {
    fs::write(directory.join("export.ics"), CALENDAR)?;
    fs::write(directory.join("render.png"), PNG)?;
    let rendered_local =
        rendered_local.map_or_else(|| "null".to_owned(), |value| format!("\"{value}\""));
    let metadata = format!(
        r#"{{
  "schema": "icalkit-capture/1",
  "client": "Google Calendar",
  "surface": "Google Calendar Web",
  "version": "web",
  "account_type": "consumer",
  "observed_on": "2026-08-15",
  "ics": "export.ics",
  "rendering_evidence": "render.png",
  "scenario": {{
    "id": "dst-gap-daily-series-v1",
    "outcome": "{outcome}",
    "rendered_local": {rendered_local}
  }}
}}"#
    );
    let path = directory.join("capture.json");
    fs::write(&path, metadata)?;
    Ok(path)
}

fn audit(metadata: &Path) -> io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_capture-audit"))
        .arg(metadata)
        .output()
}

#[test]
fn an_isolated_gap_capture_is_audited_without_copying_or_disclosing_it()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TemporaryDirectory::outside_workspace("valid")?;
    let metadata = write_bundle(
        directory.path(),
        "offset-before",
        Some("2027-03-14T03:30:00"),
    )?;

    let output = audit(&metadata)?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["schema"], "icalkit-capture-audit/1");
    assert_eq!(report["status"], "ready-for-reduction");
    assert_eq!(report["client"], "Google Calendar");
    assert_eq!(report["scenario"], "dst-gap-daily-series-v1");
    assert_eq!(report["outcome"], "offset-before");
    for field in ["source_sha256", "evidence_sha256"] {
        let digest = report[field]
            .as_str()
            .ok_or_else(|| io::Error::other(format!("{field} is not a string")))?;
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "{field} is lowercase hexadecimal"
        );
    }
    let rendered = String::from_utf8(output.stdout)?;
    assert!(!rendered.contains("BEGIN:VCALENDAR"));
    assert!(!rendered.contains(&directory.path().display().to_string()));
    assert!(directory.path().join("export.ics").is_file());
    assert!(directory.path().join("render.png").is_file());
    Ok(())
}

#[test]
fn an_outlook_web_organization_capture_uses_the_same_scenario_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TemporaryDirectory::outside_workspace("outlook")?;
    let metadata = write_bundle(directory.path(), "skipped", None)?;
    let google = fs::read_to_string(&metadata)?;
    let microsoft = google
        .replace("Google Calendar", "Microsoft 365")
        .replace("Microsoft 365 Web", "Outlook Web")
        .replace("\"consumer\"", "\"organization\"");
    fs::write(&metadata, microsoft)?;
    let outlook_calendar = String::from_utf8(CALENDAR.to_vec())?
        .replace("TZID=America/New_York", "TZID=Eastern Standard Time")
        .replace(
            "RRULE:FREQ=DAILY;COUNT=3",
            "RRULE:UNTIL=20270315T063000Z;FREQ=DAILY",
        )
        .replace(
            "SUMMARY:ICALKIT GAP TEST",
            "SUMMARY;LANGUAGE=en-US:ICALKIT GAP TEST",
        );
    fs::write(directory.path().join("export.ics"), outlook_calendar)?;

    let output = audit(&metadata)?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["client"], "Microsoft 365");
    assert_eq!(report["surface"], "Outlook Web");
    assert_eq!(report["account_type"], "organization");
    assert_eq!(report["outcome"], "skipped");
    assert!(report.get("rendered_local").is_none());
    Ok(())
}

#[test]
fn a_daily_rule_that_ends_before_the_gap_is_not_the_capture_scenario()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TemporaryDirectory::outside_workspace("ends-early")?;
    let metadata = write_bundle(directory.path(), "skipped", None)?;
    let ends_early = String::from_utf8(CALENDAR.to_vec())?.replace(
        "RRULE:FREQ=DAILY;COUNT=3",
        "RRULE:FREQ=DAILY;UNTIL=20270313T073000Z",
    );
    fs::write(directory.path().join("export.ics"), ends_early)?;

    let output = audit(&metadata)?;

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("spring-forward gap"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn a_gap_outcome_must_match_the_producer_rendering() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TemporaryDirectory::outside_workspace("contradiction")?;
    let metadata = write_bundle(
        directory.path(),
        "offset-before",
        Some("2027-03-14T01:30:00"),
    )?;

    let output = audit(&metadata)?;

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("offset-before"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn a_raw_bundle_inside_the_workspace_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = root.join("target").join(format!(
        "capture-audit-inside-workspace-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory)?;
    let metadata = write_bundle(&directory, "skipped", None)?;

    let output = audit(&metadata)?;

    let _ = fs::remove_dir_all(&directory);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the workspace"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    Ok(())
}
