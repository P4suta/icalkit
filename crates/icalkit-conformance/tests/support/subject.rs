// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, Command, ExitStatus, Stdio};

use serde_json::Value;

#[derive(Debug)]
pub(super) enum SubjectError {
    Spawn(io::Error),
    MissingInput,
    MissingOutput,
    Encode {
        request: usize,
        source: serde_json::Error,
    },
    Send {
        request: usize,
        source: io::Error,
    },
    Flush {
        request: usize,
        source: io::Error,
    },
    Read {
        request: usize,
        source: io::Error,
    },
    MissingResponse {
        request: usize,
    },
    Decode {
        request: usize,
        source: serde_json::Error,
    },
    ExtraResponse,
    Wait(io::Error),
    Exit(ExitStatus),
}

impl fmt::Display for SubjectError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(_) => output.write_str("could not spawn conformance subject"),
            Self::MissingInput => output.write_str("subject stdin was not piped"),
            Self::MissingOutput => output.write_str("subject stdout was not piped"),
            Self::Encode { request, .. } => write!(output, "could not encode request {request}"),
            Self::Send { request, .. } => write!(output, "could not send request {request}"),
            Self::Flush { request, .. } => write!(output, "could not flush request {request}"),
            Self::Read { request, .. } => write!(output, "could not read response {request}"),
            Self::MissingResponse { request } => write!(output, "missing response {request}"),
            Self::Decode { request, .. } => write!(output, "could not decode response {request}"),
            Self::ExtraResponse => output.write_str("subject returned an extra response"),
            Self::Wait(_) => output.write_str("could not wait for conformance subject"),
            Self::Exit(status) => write!(output, "conformance subject failed: {status}"),
        }
    }
}

impl Error for SubjectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spawn(source) | Self::Wait(source) => Some(source),
            Self::Encode { source, .. } | Self::Decode { source, .. } => Some(source),
            Self::Send { source, .. } | Self::Flush { source, .. } | Self::Read { source, .. } => {
                Some(source)
            },
            Self::MissingInput
            | Self::MissingOutput
            | Self::MissingResponse { .. }
            | Self::ExtraResponse
            | Self::Exit(_) => None,
        }
    }
}

pub(super) fn run(requests: &[Value]) -> Result<Vec<Value>, SubjectError> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_icalkit-conformance"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(SubjectError::Spawn)?;
    collect(&mut child, requests)
}

fn collect(child: &mut Child, requests: &[Value]) -> Result<Vec<Value>, SubjectError> {
    let mut guard = SubjectGuard {
        child,
        status: None,
    };
    let output = guard
        .child
        .stdout
        .take()
        .ok_or(SubjectError::MissingOutput)?;
    let input = guard.child.stdin.take().ok_or(SubjectError::MissingInput)?;
    let responses = exchange(input, BufReader::new(output), requests)?;
    let status = guard.wait()?;
    if !status.success() {
        return Err(SubjectError::Exit(status));
    }
    Ok(responses)
}

struct SubjectGuard<'a> {
    child: &'a mut Child,
    status: Option<ExitStatus>,
}

impl SubjectGuard<'_> {
    fn wait(&mut self) -> Result<ExitStatus, SubjectError> {
        let status = self.child.wait().map_err(SubjectError::Wait)?;
        self.status = Some(status);
        Ok(status)
    }
}

impl Drop for SubjectGuard<'_> {
    fn drop(&mut self) {
        if self.status.is_some() {
            return;
        }
        let stop = self.child.kill();
        if let Err(error) = self.child.wait() {
            let _ = writeln!(
                io::stderr().lock(),
                "could not reap conformance subject: {error}; stop result: {stop:?}"
            );
        }
    }
}

fn exchange(
    mut input: impl Write,
    mut output: impl BufRead,
    requests: &[Value],
) -> Result<Vec<Value>, SubjectError> {
    let mut responses = Vec::with_capacity(requests.len());
    let mut line = String::new();
    for (request, value) in requests.iter().enumerate() {
        serde_json::to_writer(&mut input, value)
            .map_err(|source| SubjectError::Encode { request, source })?;
        input
            .write_all(b"\n")
            .map_err(|source| SubjectError::Send { request, source })?;
        input
            .flush()
            .map_err(|source| SubjectError::Flush { request, source })?;
        line.clear();
        if output
            .read_line(&mut line)
            .map_err(|source| SubjectError::Read { request, source })?
            == 0
        {
            return Err(SubjectError::MissingResponse { request });
        }
        responses.push(
            serde_json::from_str(&line)
                .map_err(|source| SubjectError::Decode { request, source })?,
        );
    }
    drop(input);
    if !output
        .fill_buf()
        .map_err(|source| SubjectError::Read {
            request: requests.len(),
            source,
        })?
        .is_empty()
    {
        return Err(SubjectError::ExtraResponse);
    }
    Ok(responses)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{Cursor, Read};
    use std::rc::Rc;

    use serde_json::json;

    use super::*;

    const CAPACITY: usize = 32;

    #[derive(Default)]
    struct PipeState {
        input: Vec<u8>,
        response: Option<Vec<u8>>,
        output_blocked: bool,
        buffered_input: usize,
        closed: bool,
    }

    struct PipeInput(Rc<RefCell<PipeState>>);

    impl Write for PipeInput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut state = self.0.borrow_mut();
            if state.output_blocked {
                let available = CAPACITY.saturating_sub(state.buffered_input);
                if available == 0 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let count = available.min(bytes.len());
                state.input.extend_from_slice(&bytes[..count]);
                state.buffered_input = state.buffered_input.saturating_add(count);
                return Ok(count);
            }
            for (index, byte) in bytes.iter().enumerate() {
                state.input.push(*byte);
                if *byte == b'\n' {
                    let value: Value = serde_json::from_slice(&state.input)?;
                    let mut response = serde_json::to_vec(&json!({
                        "id": value["id"],
                        "padding": "R".repeat(128),
                    }))?;
                    response.push(b'\n');
                    state.response = Some(response);
                    state.output_blocked = true;
                    state.input.clear();
                    return Ok(index.saturating_add(1));
                }
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for PipeInput {
        fn drop(&mut self) {
            self.0.borrow_mut().closed = true;
        }
    }

    struct PipeOutput {
        state: Rc<RefCell<PipeState>>,
        response: Cursor<Vec<u8>>,
    }

    impl Read for PipeOutput {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let count = bytes.len().min(available.len());
            bytes[..count].copy_from_slice(&available[..count]);
            self.consume(count);
            Ok(count)
        }
    }

    impl BufRead for PipeOutput {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.response.fill_buf()?.is_empty() {
                let mut state = self.state.borrow_mut();
                if let Some(response) = state.response.take() {
                    self.response = Cursor::new(response);
                } else if !state.closed {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
            }
            let bytes = self.response.fill_buf()?;
            Ok(&bytes[..CAPACITY.min(bytes.len())])
        }

        fn consume(&mut self, count: usize) {
            self.response.consume(count);
            if self.response.fill_buf().is_ok_and(<[u8]>::is_empty) {
                let mut state = self.state.borrow_mut();
                state.output_blocked = false;
                state.buffered_input = 0;
            }
        }
    }

    #[test]
    fn bounded_pipes_consume_each_response_before_the_next_request() -> Result<(), Box<dyn Error>> {
        let state = Rc::new(RefCell::new(PipeState::default()));
        let responses = exchange(
            PipeInput(Rc::clone(&state)),
            PipeOutput {
                state,
                response: Cursor::new(Vec::new()),
            },
            &[
                json!({"id": "first", "padding": "I".repeat(128)}),
                json!({"id": "second", "padding": "I".repeat(128)}),
            ],
        )?;
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "first");
        assert_eq!(responses[1]["id"], "second");
        Ok(())
    }

    fn waiting_subject() -> io::Result<Child> {
        Command::new(env!("CARGO_BIN_EXE_icalkit-conformance"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
    }

    fn observe_and_clean(child: &mut Child) -> io::Result<Option<ExitStatus>> {
        let observed = child.try_wait()?;
        if observed.is_none() {
            let _ = child.kill();
            child.wait()?;
        }
        Ok(observed)
    }

    #[test]
    fn scope_exit_stops_and_waits_for_an_unfinished_subject() -> Result<(), Box<dyn Error>> {
        let mut child = waiting_subject()?;
        {
            let _guard = SubjectGuard {
                child: &mut child,
                status: None,
            };
        }
        let waited = child.stdin.is_none();
        let observed = observe_and_clean(&mut child)?;
        assert!(waited, "scope exit did not close input through wait");
        assert!(
            observed.is_some_and(|status| !status.success()),
            "scope exit must stop the subject before waiting: {observed:?}"
        );
        Ok(())
    }

    #[test]
    fn missing_output_preserves_the_error_and_reaps_the_subject() -> Result<(), Box<dyn Error>> {
        let mut child = waiting_subject()?;
        drop(child.stdout.take());
        let error = collect(&mut child, &[]).err();
        let waited = child.stdin.is_none();
        let observed = observe_and_clean(&mut child)?;
        assert!(matches!(error, Some(SubjectError::MissingOutput)));
        assert!(waited, "missing output did not close input through wait");
        assert!(
            observed.is_some_and(|status| !status.success()),
            "missing output must stop the subject before waiting: {observed:?}"
        );
        Ok(())
    }

    #[test]
    fn unwinding_stops_and_waits_for_an_unfinished_subject() -> Result<(), Box<dyn Error>> {
        let mut child = waiting_subject()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = SubjectGuard {
                child: &mut child,
                status: None,
            };
            std::panic::resume_unwind(Box::new(()));
        }));
        let waited = child.stdin.is_none();
        let observed = observe_and_clean(&mut child)?;
        assert!(result.is_err());
        assert!(waited, "unwinding did not close input through wait");
        assert!(
            observed.is_some_and(|status| !status.success()),
            "unwinding must stop the subject before waiting: {observed:?}"
        );
        Ok(())
    }

    #[test]
    fn missing_response_identifies_the_request() {
        let result = exchange(Vec::new(), Cursor::new(b"{}\n"), &[json!({}), json!({})]);
        assert!(matches!(
            result,
            Err(SubjectError::MissingResponse { request: 1 })
        ));
    }

    #[test]
    fn extra_response_is_rejected_after_input_closes() {
        let result = exchange(Vec::new(), Cursor::new(b"{}\n{}\n"), &[json!({})]);
        assert!(matches!(result, Err(SubjectError::ExtraResponse)));
    }

    #[test]
    fn a_final_response_without_a_newline_keeps_its_value() -> Result<(), SubjectError> {
        let values = exchange(Vec::new(), Cursor::new(b"{\"id\":\"last\"}"), &[json!({})])?;
        assert_eq!(values, vec![json!({"id": "last"})]);
        Ok(())
    }

    #[test]
    fn malformed_response_preserves_its_cause_without_request_contents()
    -> Result<(), Box<dyn Error>> {
        let error = exchange(
            Vec::new(),
            Cursor::new(b"invalid\n"),
            &[json!({"secret": "private-input"})],
        )
        .err()
        .ok_or("malformed response was accepted")?;
        assert!(matches!(error, SubjectError::Decode { request: 0, .. }));
        assert!(error.source().is_some());
        assert_eq!(error.to_string(), "could not decode response 0");
        assert!(!format!("{error:?}").contains("private-input"));
        Ok(())
    }

    struct BrokenWriter {
        newline: bool,
    }

    impl Write for BrokenWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.newline || bytes == b"\n" {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_request_encoding_preserves_the_io_cause() -> Result<(), Box<dyn Error>> {
        let error = exchange(
            BrokenWriter { newline: false },
            Cursor::new(b"{}\n"),
            &[json!({})],
        )
        .err()
        .ok_or("broken writer was accepted")?;
        assert!(error.source().is_some());
        assert!(
            matches!(error, SubjectError::Encode { request: 0, source } if source.io_error_kind() == Some(io::ErrorKind::BrokenPipe))
        );
        Ok(())
    }

    #[test]
    fn failed_request_terminator_preserves_the_io_cause() -> Result<(), Box<dyn Error>> {
        let error = exchange(
            BrokenWriter { newline: true },
            Cursor::new(b"{}\n"),
            &[json!({})],
        )
        .err()
        .ok_or("broken terminator was accepted")?;
        assert!(error.source().is_some());
        assert!(
            matches!(error, SubjectError::Send { request: 0, source } if source.kind() == io::ErrorKind::BrokenPipe)
        );
        Ok(())
    }

    struct BrokenFlush;

    impl Write for BrokenFlush {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }

    #[test]
    fn request_flush_failure_prevents_reading_a_response() -> Result<(), Box<dyn Error>> {
        let error = exchange(BrokenFlush, Cursor::new(b"{}\n"), &[json!({})])
            .err()
            .ok_or("failed flush was accepted")?;
        assert!(error.source().is_some());
        assert!(
            matches!(error, SubjectError::Flush { request: 0, source } if source.kind() == io::ErrorKind::PermissionDenied)
        );
        Ok(())
    }

    struct BrokenReader;

    impl Read for BrokenReader {
        fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::ConnectionReset.into())
        }
    }

    #[test]
    fn response_read_failure_preserves_the_io_cause() -> Result<(), Box<dyn Error>> {
        let error = exchange(Vec::new(), BufReader::new(BrokenReader), &[json!({})])
            .err()
            .ok_or("broken reader was accepted")?;
        assert!(error.source().is_some());
        assert!(
            matches!(error, SubjectError::Read { request: 0, source } if source.kind() == io::ErrorKind::ConnectionReset)
        );
        Ok(())
    }

    #[test]
    fn an_empty_native_exchange_closes_input_and_waits() -> Result<(), SubjectError> {
        assert_eq!(run(&[])?, Vec::<Value>::new());
        Ok(())
    }

    #[test]
    fn a_failed_native_subject_is_rejected_after_waiting() -> Result<(), Box<dyn Error>> {
        let mut child = waiting_subject()?;
        child.kill()?;
        let error = collect(&mut child, &[]).err();
        let status = observe_and_clean(&mut child)?;
        assert!(matches!(error, Some(SubjectError::Exit(status)) if !status.success()));
        assert!(status.is_some());
        Ok(())
    }
}
