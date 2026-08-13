// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private JSONL subject for the versioned icalkit conformance protocol.

use std::io::{self, Write as _};
use std::process::ExitCode;

use icalkit::Calendar;
use icalkit::interop::{CommonClientsV1, Import, RfcRepairV1};
use serde::{Deserialize, Serialize};

const PROTOCOL: &str = "icalkit-conformance/1";

#[derive(Deserialize)]
struct Request {
    protocol: String,
    id: String,
    operation: Operation,
    input_hex: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Operation {
    StrictParse,
    NormalizeRfcRepairV1,
    NormalizeCommonClientsV1,
}

#[derive(Serialize)]
struct Response {
    protocol: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    issues: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    changes: Vec<ReportedChange>,
}

#[derive(Serialize)]
struct ReportedChange {
    code: String,
    offset: u64,
}

impl Response {
    fn error(id: Option<String>, outcome: &'static str, code: &str) -> Self {
        Self {
            protocol: PROTOCOL,
            id,
            outcome,
            output_hex: None,
            code: Some(code.to_owned()),
            issues: Vec::new(),
            changes: Vec::new(),
        }
    }
}

fn main() -> ExitCode {
    match serve(io::stdin().lock(), io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "icalkit-conformance: {error}");
            ExitCode::FAILURE
        },
    }
}

fn serve(input: impl io::BufRead, mut output: impl io::Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let response = answer_line(&line);
        serde_json::to_writer(&mut output, &response).map_err(io::Error::other)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

fn answer_line(line: &str) -> Response {
    let Ok(request) = serde_json::from_str::<Request>(line) else {
        return Response::error(
            recover_id(line),
            "protocol-error",
            "icalkit.conformance.invalid-json",
        );
    };
    if request.protocol != PROTOCOL {
        return Response::error(
            Some(request.id),
            "protocol-error",
            "icalkit.conformance.protocol-version",
        );
    }
    let Some(bytes) = decode_hex(&request.input_hex) else {
        return Response::error(
            Some(request.id),
            "protocol-error",
            "icalkit.conformance.input-hex",
        );
    };
    answer(request.id, request.operation, &bytes)
}

fn recover_id(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

fn answer(id: String, operation: Operation, bytes: &[u8]) -> Response {
    match operation {
        Operation::StrictParse => strict_parse(id, bytes),
        Operation::NormalizeRfcRepairV1 => normalize_rfc(id, bytes),
        Operation::NormalizeCommonClientsV1 => normalize_common_clients(id, bytes),
    }
}

fn strict_parse(id: String, bytes: &[u8]) -> Response {
    match Calendar::parse(bytes) {
        Ok(calendar) => Response {
            protocol: PROTOCOL,
            id: Some(id),
            outcome: "accepted",
            output_hex: Some(encode_hex(&calendar.to_bytes())),
            code: None,
            issues: calendar
                .issues()
                .iter()
                .map(|issue| issue.code().as_str().to_owned())
                .collect(),
            changes: Vec::new(),
        },
        Err(error) => Response {
            protocol: PROTOCOL,
            id: Some(id),
            outcome: "rejected",
            output_hex: None,
            code: Some(error.code().as_str().to_owned()),
            issues: error
                .issues()
                .iter()
                .map(|issue| issue.code().as_str().to_owned())
                .collect(),
            changes: Vec::new(),
        },
    }
}

fn normalize_rfc(id: String, bytes: &[u8]) -> Response {
    let normalized = Import::read(bytes).and_then(|import| import.normalize(RfcRepairV1));
    normalization_response(id, normalized)
}

fn normalize_common_clients(id: String, bytes: &[u8]) -> Response {
    let normalized = Import::read(bytes).and_then(|import| import.normalize(CommonClientsV1));
    normalization_response(id, normalized)
}

fn normalization_response(
    id: String,
    normalized: Result<icalkit::interop::Normalization, icalkit::Error>,
) -> Response {
    match normalized {
        Ok(normalized) => Response {
            protocol: PROTOCOL,
            id: Some(id),
            outcome: "normalized",
            output_hex: Some(encode_hex(normalized.output().as_bytes())),
            code: None,
            issues: Vec::new(),
            changes: normalized
                .changes()
                .iter()
                .map(|change| ReportedChange {
                    code: change.code().as_str().to_owned(),
                    offset: change.offset(),
                })
                .collect(),
        },
        Err(error) => Response {
            protocol: PROTOCOL,
            id: Some(id),
            outcome: "rejected",
            output_hex: None,
            code: Some(error.code().as_str().to_owned()),
            issues: error
                .issues()
                .iter()
                .map(|issue| issue.code().as_str().to_owned())
                .collect(),
            changes: Vec::new(),
        },
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(*byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(*byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(encoded: &str) -> Option<Vec<u8>> {
    let pairs = encoded.as_bytes().chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    for pair in pairs {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes.push(high.checked_mul(16)?.checked_add(low)?);
    }
    Some(bytes)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte
            .checked_sub(b'a')
            .and_then(|digit| digit.checked_add(10)),
        b'A'..=b'F' => byte
            .checked_sub(b'A')
            .and_then(|digit| digit.checked_add(10)),
        _ => None,
    }
}
