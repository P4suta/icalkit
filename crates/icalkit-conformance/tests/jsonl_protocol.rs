// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The process boundary is the conformance contract; no Rust library API is required.

use std::error::Error;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const PROTOCOL: &str = "icalkit-conformance/1";
const VALID: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit protocol tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn run(requests: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_icalkit-conformance"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("subject stdin was not piped"))?;
        for request in requests {
            serde_json::to_writer(&mut *stdin, request)?;
            stdin.write_all(b"\n")?;
        }
    }
    let output = child.wait_with_output()?;
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout)?;
    text.lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[test]
fn strict_and_normalization_questions_are_versioned_jsonl() -> Result<(), Box<dyn Error>> {
    let bare_lf: Vec<u8> = VALID
        .iter()
        .copied()
        .filter(|byte| *byte != b'\r')
        .collect();
    let answers = run(&[
        json!({
            "protocol": PROTOCOL,
            "id": "strict-valid",
            "operation": "strict-parse",
            "input_hex": hex(VALID),
        }),
        json!({
            "protocol": PROTOCOL,
            "id": "strict-invalid",
            "operation": "strict-parse",
            "input_hex": hex(b"not a calendar"),
        }),
        json!({
            "protocol": PROTOCOL,
            "id": "normalize",
            "operation": "normalize-rfc-repair-v1",
            "input_hex": hex(&bare_lf),
        }),
    ])?;

    assert_eq!(answers.len(), 3);
    assert_eq!(answers[0]["protocol"], PROTOCOL);
    assert_eq!(answers[0]["id"], "strict-valid");
    assert_eq!(answers[0]["outcome"], "accepted");
    assert_eq!(answers[0]["output_hex"], hex(VALID));
    assert_eq!(answers[1]["id"], "strict-invalid");
    assert_eq!(answers[1]["outcome"], "rejected");
    assert_eq!(answers[1]["code"], "icalkit.validation.failed");
    assert_eq!(answers[2]["id"], "normalize");
    assert_eq!(answers[2]["outcome"], "normalized");
    assert_eq!(answers[2]["output_hex"], hex(VALID));
    assert!(
        answers[2]["changes"]
            .as_array()
            .is_some_and(|changes| !changes.is_empty())
    );
    assert_eq!(
        answers[2]["changes"][0]["code"],
        "icalkit.normalize.line-ending"
    );
    Ok(())
}

#[test]
fn a_protocol_mismatch_is_a_correlated_answer_not_a_process_crash() -> Result<(), Box<dyn Error>> {
    let answers = run(&[json!({
        "protocol": "icalkit-conformance/999",
        "id": "future",
        "operation": "strict-parse",
        "input_hex": "",
    })])?;

    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["protocol"], PROTOCOL);
    assert_eq!(answers[0]["id"], "future");
    assert_eq!(answers[0]["outcome"], "protocol-error");
    assert_eq!(answers[0]["code"], "icalkit.conformance.protocol-version");
    Ok(())
}
