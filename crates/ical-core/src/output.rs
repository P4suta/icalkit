// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where serialized octets go.
//!
//! `core` has no `io::Write`, and `core::fmt::Write` takes `&str` — which is exactly what
//! storage here is not, since a preserved value may hold octets that are not valid UTF-8. So
//! the sink is this crate's own trait, and it is octet-shaped for the same reason everything
//! below the typed view is.
//!
//! The error is associated rather than fixed so that a caller writing into a growable buffer
//! does not pay for an error type it cannot produce.

use alloc::vec::Vec;
use core::convert::Infallible;

/// A sink for serialized octets.
///
/// Object-safe, so `&mut dyn Writer<Error = E>` is a legal argument.
pub trait Writer {
    /// What can go wrong while writing. [`Infallible`] for a sink that cannot fail.
    type Error;

    /// Write every octet of `bytes`, or fail.
    ///
    /// Partial writes are not a state this trait represents: a serializer that had to resume
    /// mid-property would need to know where in a fold it stopped, and a caller that wants
    /// that owns the buffering instead.
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

impl Writer for Vec<u8> {
    type Error = Infallible;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

impl<W: Writer + ?Sized> Writer for &mut W {
    type Error = W::Error;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        (**self).write_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::Writer;

    /// Accepts a `dyn` sink, which is the shape a caller with two kinds of buffer needs.
    fn write_through(sink: &mut dyn Writer<Error = core::convert::Infallible>) {
        sink.write_bytes(b"BEGIN:VCALENDAR\r\n").unwrap();
    }

    #[test]
    fn a_growable_buffer_is_a_sink_that_cannot_fail() {
        let mut out: Vec<u8> = Vec::new();
        write_through(&mut out);
        assert_eq!(out, b"BEGIN:VCALENDAR\r\n");
    }

    #[test]
    fn a_mutable_reference_forwards_to_what_it_points_at() {
        let mut out: Vec<u8> = Vec::new();
        let borrowed = &mut out;
        Writer::write_bytes(&mut { borrowed }, b"END:VCALENDAR\r\n").unwrap();
        assert_eq!(out, b"END:VCALENDAR\r\n");
    }
}
