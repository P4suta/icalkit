// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where encoded octets go.
//!
//! An encoder here writes into a caller-supplied sink rather than returning a buffer, for the
//! reason `docs/adr/0004` gives about the whole layer: a multistatus answering a
//! forty-thousand-resource collection is not something either side can be assumed able to
//! hold, and a `Vec<u8>` return type decides that for everyone.
//!
//! Two implementations ship. [`ByteSink`] over `Vec<u8>` goes through `try_reserve`, so an
//! encoder discovers that the allocator refused rather than aborting the process on it — the
//! posture `docs/adr/0007` requires of every allocation in this workspace. [`SliceSink`]
//! writes into a caller-owned buffer and allocates nothing at all, which is the shape a
//! device with 64 KB of RAM has.

use alloc::vec::Vec;

use crate::failure::SinkFull;

/// A push-only destination for encoded octets.
///
/// Object-safe, so `&mut dyn ByteSink` is a legal argument and an encoder does not spread a
/// generic parameter through the caller's own types.
pub trait ByteSink {
    /// Append `bytes`, or report that there is no room for them.
    ///
    /// All or nothing: a sink that cannot take every octet takes none of them, because a
    /// partially written element is not a document anyone can finish or discard cleanly.
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull>;
}

impl ByteSink for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull> {
        // `try_reserve` rather than `extend_from_slice`: an encoder asked to write more than
        // the allocator will give must report it, and the infallible path aborts the process
        // instead. A server encoding an answer it cannot afford has a `507` to send, and it
        // cannot send one from inside an abort.
        self.try_reserve(bytes.len()).map_err(|_| SinkFull)?;
        self.extend_from_slice(bytes);
        Ok(())
    }
}

impl<S: ByteSink + ?Sized> ByteSink for &mut S {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull> {
        (**self).write(bytes)
    }
}

/// A sink over a caller-owned buffer, which allocates nothing.
///
/// The written prefix is readable through [`SliceSink::written`] while the sink is alive and
/// the buffer is the caller's again once it is dropped.
#[derive(Debug)]
pub struct SliceSink<'a> {
    /// The caller's buffer.
    buffer: &'a mut [u8],
    /// How many octets of it are live.
    filled: usize,
}

impl<'a> SliceSink<'a> {
    /// A sink writing into `buffer` from its start.
    #[must_use]
    pub const fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, filled: 0 }
    }

    /// The octets written so far.
    #[must_use]
    pub fn written(&self) -> &[u8] {
        self.buffer.get(..self.filled).unwrap_or(&[])
    }

    /// How many octets have been written.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.filled
    }

    /// Whether nothing has been written yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// How much room is left.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buffer.len().saturating_sub(self.filled)
    }
}

impl ByteSink for SliceSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull> {
        let end = self.filled.checked_add(bytes.len()).ok_or(SinkFull)?;
        let room = self.buffer.get_mut(self.filled..end).ok_or(SinkFull)?;
        room.copy_from_slice(bytes);
        self.filled = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{ByteSink, SliceSink};
    use crate::failure::SinkFull;

    #[test]
    fn a_slice_sink_refuses_rather_than_writing_a_prefix() {
        let mut buffer = [0_u8; 4];
        let mut sink = SliceSink::new(&mut buffer);
        sink.write(b"ab").unwrap();
        assert_eq!(sink.write(b"cde"), Err(SinkFull));
        assert_eq!(sink.written(), b"ab");
        assert_eq!(sink.remaining(), 2);
    }

    #[test]
    fn a_vec_sink_appends() {
        let mut out: Vec<u8> = Vec::new();
        out.write(b"<D:multistatus").unwrap();
        out.write(b"/>").unwrap();
        assert_eq!(out, b"<D:multistatus/>");
    }

    #[test]
    fn a_dyn_sink_is_a_legal_argument() {
        fn emit(into: &mut dyn ByteSink) -> Result<(), SinkFull> {
            into.write(b"ok")
        }
        let mut out: Vec<u8> = Vec::new();
        emit(&mut out).unwrap();
        assert_eq!(out, b"ok");
    }
}
