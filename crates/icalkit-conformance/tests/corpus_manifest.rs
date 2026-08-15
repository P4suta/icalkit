// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Provenance is data: an unverified client-shaped fixture cannot become a captured export.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Component, Path};

use serde::Deserialize;

const SCHEMA: &str = "icalkit-corpus/1";
const MANIFEST: &str = include_str!("../corpus/manifest.v1.jsonl");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    schema: String,
    id: String,
    fixture: String,
    provenance: Provenance,
    #[serde(default)]
    normalization_profile: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Provenance {
    Synthetic {
        shaped_like: String,
    },
    Captured {
        client: String,
        surface: String,
        version: String,
        account_type: AccountType,
        observed_on: String,
        anonymized: bool,
        reduction: String,
        evidence: Box<CaptureEvidence>,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AccountType {
    Consumer,
    Organization,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum CaptureEvidence {
    Export {
        source_sha256: String,
    },
    DstGapRendering {
        source_sha256: String,
        scenario: String,
        outcome: GapOutcome,
        rendered_local: Option<String>,
        rendering: String,
        rendering_sha256: String,
        #[serde(default)]
        notes: Option<String>,
    },
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

#[test]
fn every_manifest_row_is_versioned_unique_present_and_honest() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut ids = BTreeSet::new();
    let mut rows = 0_usize;
    for line in MANIFEST.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Entry = serde_json::from_str(line)?;
        rows = rows.saturating_add(1);
        assert_eq!(entry.schema, SCHEMA, "{}", entry.id);
        assert!(ids.insert(entry.id.clone()), "duplicate id: {}", entry.id);
        assert!(
            safe_relative_path(&entry.fixture),
            "fixture escapes the corpus: {}",
            entry.fixture
        );
        assert!(
            root.join(&entry.fixture).is_file(),
            "missing fixture: {}",
            entry.fixture
        );
        match entry.provenance {
            Provenance::Synthetic { shaped_like } => {
                assert!(!shaped_like.trim().is_empty(), "{}", entry.id);
                assert!(
                    entry.normalization_profile.is_none(),
                    "synthetic evidence cannot justify a compatibility repair: {}",
                    entry.id
                );
            },
            Provenance::Captured {
                client,
                surface,
                version,
                account_type,
                observed_on,
                anonymized,
                reduction,
                evidence,
            } => {
                let Some(expected_surface) = expected_surface(&client) else {
                    return Err(
                        std::io::Error::other(format!("unsupported client: {}", entry.id)).into(),
                    );
                };
                assert_eq!(surface, expected_surface, "{}", entry.id);
                assert!(
                    matches!(
                        account_type,
                        AccountType::Consumer | AccountType::Organization
                    ),
                    "{}",
                    entry.id
                );
                assert!(!version.trim().is_empty(), "{}", entry.id);
                assert!(
                    observed_on.parse::<jiff::civil::Date>().is_ok(),
                    "{}",
                    entry.id
                );
                assert!(anonymized, "{}", entry.id);
                assert!(!reduction.trim().is_empty(), "{}", entry.id);
                validate_evidence(root, &entry.id, *evidence).map_err(std::io::Error::other)?;
            },
        }
    }
    assert!(
        rows >= 3,
        "the manifest must cover the existing client shapes"
    );
    Ok(())
}

fn expected_surface(client: &str) -> Option<&'static str> {
    match client {
        "Google Calendar" => Some("Google Calendar Web"),
        "Microsoft 365" => Some("Outlook Web"),
        "Apple Calendar" => Some("Apple Calendar"),
        _ => None,
    }
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_evidence(root: &Path, id: &str, evidence: CaptureEvidence) -> Result<(), String> {
    match evidence {
        CaptureEvidence::Export { source_sha256 } => {
            if valid_sha256(&source_sha256) {
                Ok(())
            } else {
                Err(format!("{id}: source_sha256 is not lowercase SHA-256"))
            }
        },
        CaptureEvidence::DstGapRendering {
            source_sha256,
            scenario,
            outcome,
            rendered_local,
            rendering,
            rendering_sha256,
            notes,
        } => {
            if !valid_sha256(&source_sha256) {
                return Err(format!("{id}: source_sha256 is not lowercase SHA-256"));
            }
            if !valid_sha256(&rendering_sha256) {
                return Err(format!("{id}: rendering_sha256 is not lowercase SHA-256"));
            }
            if scenario != "dst-gap-daily-series-v1" {
                return Err(format!("{id}: unknown DST gap scenario {scenario}"));
            }
            validate_gap_outcome(id, outcome, rendered_local.as_deref(), notes.as_deref())?;
            if !safe_relative_path(&rendering) || !root.join(&rendering).is_file() {
                return Err(format!("{id}: missing rendering evidence {rendering}"));
            }
            Ok(())
        },
    }
}

fn validate_gap_outcome(
    id: &str,
    outcome: GapOutcome,
    rendered_local: Option<&str>,
    notes: Option<&str>,
) -> Result<(), String> {
    let valid = match outcome {
        GapOutcome::Skipped => rendered_local.is_none(),
        GapOutcome::OffsetBefore => rendered_local == Some("2027-03-14T03:30:00"),
        GapOutcome::GapEnd => rendered_local == Some("2027-03-14T03:00:00"),
        GapOutcome::OffsetAfter => rendered_local == Some("2027-03-14T01:30:00"),
        GapOutcome::Other => {
            rendered_local.is_some_and(|value| !value.trim().is_empty())
                && notes.is_some_and(|value| !value.trim().is_empty())
        },
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{id}: gap outcome and rendered_local contradict each other"
        ))
    }
}

#[test]
fn captured_rows_cannot_omit_the_audited_evidence() {
    let row = r#"{
        "schema":"icalkit-corpus/1",
        "id":"captured-without-evidence",
        "fixture":"tests/fixtures/break_clients/apple_structured_location.ics",
        "provenance":{
            "kind":"captured",
            "client":"Google Calendar",
            "surface":"Google Calendar Web",
            "version":"web",
            "account_type":"consumer",
            "observed_on":"2026-08-15",
            "anonymized":true,
            "reduction":"one synthetic event"
        }
    }"#;
    assert!(
        serde_json::from_str::<Entry>(row).is_err(),
        "a capture with no hash or claim is not evidence"
    );
}

#[test]
fn a_gap_bucket_must_match_the_clock_the_producer_rendered()
-> Result<(), Box<dyn std::error::Error>> {
    let evidence: CaptureEvidence = serde_json::from_str(
        r#"{
            "kind":"dst-gap-rendering",
            "source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "scenario":"dst-gap-daily-series-v1",
            "outcome":"offset-before",
            "rendered_local":"2027-03-14T01:30:00",
            "rendering":"corpus/evidence/example.png",
            "rendering_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        }"#,
    )?;
    let result = validate_evidence(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "contradictory-gap",
        evidence,
    );
    let Err(error) = result else {
        return Err(std::io::Error::other("01:30 is the offset-after bucket").into());
    };
    assert!(error.contains("contradict"));
    Ok(())
}

#[test]
fn source_hashes_are_canonical_lowercase_sha256() {
    assert!(valid_sha256(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ));
    assert!(!valid_sha256(
        "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
    ));
    assert!(!valid_sha256("0123456789abcdef"));
}
