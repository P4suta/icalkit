// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sans-I/O CalDAV client and server workflow vocabulary.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

/// One HTTP header without coupling the API to an HTTP implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    name: String,
    value: Vec<u8>,
}

impl Header {
    /// Build an owned header.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Header name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Header value octets.
    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

/// An owned sans-I/O HTTP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireRequest {
    method: String,
    uri: String,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl WireRequest {
    /// Build an owned request.
    #[must_use]
    pub fn new(
        method: impl Into<String>,
        uri: impl Into<String>,
        headers: Vec<Header>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method: method.into(),
            uri: uri.into(),
            headers,
            body,
        }
    }

    /// HTTP/WebDAV method.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Request URI.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Request headers.
    #[must_use]
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    /// Request body.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// An owned sans-I/O HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireResponse {
    status: u16,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl WireResponse {
    /// Build an owned response.
    #[must_use]
    pub fn new(status: u16, headers: Vec<Header>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// HTTP status code.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Response headers.
    #[must_use]
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    /// Response body.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// A client workflow factory.
#[derive(Clone, Debug, Default)]
pub struct Client {
    marker: (),
}

type ResponseDecoder<T> = Box<dyn FnOnce(WireResponse) -> Result<T, crate::Error>>;

impl Client {
    /// Create a sans-I/O client workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self { marker: () }
    }

    /// Start a typed operation without coupling it to an HTTP runtime.
    #[must_use]
    pub fn operation<T>(
        &self,
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Operation<T> {
        let () = self.marker;
        Operation::new(request, decoder)
    }
}

/// A typed client operation driven by requests and supplied responses.
pub struct Operation<T> {
    request: Option<WireRequest>,
    result: Option<Result<T, crate::Error>>,
    decoder: Option<ResponseDecoder<T>>,
}

impl<T> Operation<T> {
    /// Create an operation from its first request and response decoder.
    #[must_use]
    pub fn new(
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Self {
        Self {
            request: Some(request),
            result: None,
            decoder: Some(Box::new(decoder)),
        }
    }

    /// The request the caller should execute next.
    #[must_use]
    pub fn next_request(&self) -> Option<&WireRequest> {
        self.request.as_ref()
    }

    /// Supply the HTTP response to the current request.
    pub fn accept(&mut self, response: WireResponse) -> Result<(), crate::Error> {
        let decoder = self
            .decoder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-response"))?;
        self.request = None;
        self.result = Some(decoder(response));
        Ok(())
    }

    /// Finish after all required responses have been supplied.
    pub fn finish(mut self) -> Result<T, crate::Error> {
        self.result
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.operation-incomplete"))?
    }
}

impl<T> Debug for Operation<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("has_request", &self.request.is_some())
            .field("has_result", &self.result.is_some())
            .finish_non_exhaustive()
    }
}

/// A server workflow factory.
#[derive(Clone, Debug, Default)]
pub struct Server {
    marker: (),
}

type ServerResponder = Box<dyn FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error>>;

impl Server {
    /// Create a sans-I/O server workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self { marker: () }
    }

    /// Start a server operation whose application dependency is supplied explicitly.
    #[must_use]
    pub fn operation(
        &self,
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> ServerOperation {
        let () = self.marker;
        ServerOperation::new(need, responder)
    }
}

/// A need for application storage, ACL, or routing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerNeed {
    code: &'static str,
}

impl ServerNeed {
    /// Construct an application need identified by a stable code.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    /// The stable need code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

/// An application answer supplied to a server workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAnswer {
    body: Vec<u8>,
}

impl ServerAnswer {
    /// Construct an application answer body.
    #[must_use]
    pub fn new(body: Vec<u8>) -> Self {
        Self { body }
    }

    /// Application answer octets.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// A server operation separated from storage and ACL decisions.
pub struct ServerOperation {
    need: Option<ServerNeed>,
    response: Option<WireResponse>,
    responder: Option<ServerResponder>,
}

impl ServerOperation {
    /// Create an operation with one application need.
    #[must_use]
    pub fn new(
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> Self {
        Self {
            need: Some(need),
            response: None,
            responder: Some(Box::new(responder)),
        }
    }

    /// The storage, ACL, or routing fact needed next.
    #[must_use]
    pub const fn next_need(&self) -> Option<&ServerNeed> {
        self.need.as_ref()
    }

    /// Supply the application-owned answer.
    pub fn supply(&mut self, answer: ServerAnswer) -> Result<(), crate::Error> {
        let responder = self
            .responder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-answer"))?;
        self.need = None;
        self.response = Some(responder(answer)?);
        Ok(())
    }

    /// Finish after all application needs have been supplied.
    pub fn finish(mut self) -> Result<WireResponse, crate::Error> {
        self.response
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.server-operation-incomplete"))
    }
}

impl Debug for ServerOperation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerOperation")
            .field("has_need", &self.need.is_some())
            .field("has_response", &self.response.is_some())
            .finish_non_exhaustive()
    }
}

/// A projected query result that cannot be passed to persistence APIs as a [`Calendar`](crate::Calendar).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedCalendar {
    bytes: Box<[u8]>,
}

impl ProjectedCalendar {
    /// Serialized projected calendar data.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
