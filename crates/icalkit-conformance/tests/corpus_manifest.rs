// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Provenance is data: an unverified client-shaped fixture cannot become a captured export.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;

use serde::Deserialize;

const SCHEMA: &str = "icalkit-corpus/1";
const MANIFEST: &str = include_str!("../corpus/manifest.v1.jsonl");

#[derive(Deserialize)]
struct Entry {
    schema: String,
    id: String,
    fixture: String,
    provenance: Provenance,
    #[serde(default)]
    normalization_profile: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Provenance {
    Synthetic {
        shaped_like: String,
    },
    Captured {
        client: String,
        version: String,
        observed_on: String,
        anonymized: bool,
        reduction: String,
    },
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
                version,
                observed_on,
                anonymized,
                reduction,
            } => {
                assert!(
                    matches!(
                        client.as_str(),
                        "Google Calendar" | "Microsoft 365" | "Apple Calendar"
                    ),
                    "{}",
                    entry.id
                );
                assert!(!version.trim().is_empty(), "{}", entry.id);
                assert_eq!(observed_on.len(), 10, "{}", entry.id);
                assert!(anonymized, "{}", entry.id);
                assert!(!reduction.trim().is_empty(), "{}", entry.id);
            },
        }
    }
    assert!(
        rows >= 3,
        "the manifest must cover the existing client shapes"
    );
    Ok(())
}
