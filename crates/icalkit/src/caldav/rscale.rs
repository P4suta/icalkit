// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::str;

use super::{
    Client, ClientStep, DecodeContext, Discovery, ElementName, Header, Meter, Operation, PropName,
    PropValue, Revision, ServerAnswerKind, ServerNeed, ServerOperation, ServerStep, Status,
    WireRequest, WireResponse, XmlEvent, XmlPull, XmlReader, XmlWriter, propfind_request,
    read_multistatus_metered, validate_uri,
};
use crate::internal::core::{ContentLineReader, Document, ValueType, is_iana_token};
use crate::internal::recur::{is_gregorian_scale, parts};
use crate::{Calendar, Error, ResourcePolicy};

fn invalid() -> Error {
    Error::single("icalkit.caldav.rscale-advertisement-invalid")
}

fn too_large() -> Error {
    Error::single("icalkit.caldav.rscale-advertisement-too-large")
}

impl Client {
    /// Query the selected calendar collection or calendar home for RFC 7529 support.
    /// `None` means no `200 OK` advertisement, while an empty list permits any calendar name.
    pub fn supported_rscales(&self, uri: &str) -> Result<Operation<Option<Vec<String>>>, Error> {
        let policy = self.policy;
        let request = propfind_request(uri, &[ElementName::SupportedRscaleSet], policy)?;
        Ok(Operation::with_policy(
            request,
            move |response| read_advertisement(&response, policy),
            policy,
        ))
    }

    /// Probe the associated collection or home before conditionally storing `RSCALE` rules.
    /// The caller supplies the capability location associated with the target resource.
    pub fn conditional_write_with_rscales(
        &self,
        capability_uri: &str,
        revision: &Revision,
        calendar: &Calendar,
    ) -> Result<Operation<Revision>, Error> {
        validate_uri(capability_uri)?;
        let mut prepared = self.prepare_conditional_write(revision, calendar)?;
        let required = required_scales(calendar, self.policy)?;
        if required.is_empty() {
            return Ok(prepared);
        }
        let request = prepared.request.take().ok_or_else(invalid)?;
        let decoder = prepared.decoder.take().ok_or_else(invalid)?;
        let policy = self.policy;
        let probe = propfind_request(capability_uri, &[ElementName::SupportedRscaleSet], policy)?;
        Ok(Operation::from_decoder(
            probe,
            Box::new(move |response| {
                let advertised = read_advertisement(&response, policy)?
                    .ok_or_else(|| Error::single("icalkit.caldav.rscale-advertisement-required"))?;
                if !advertised.is_empty()
                    && !required.iter().all(|name| {
                        advertised
                            .iter()
                            .any(|supported| names_match(name, supported))
                    })
                {
                    return Err(Error::single("icalkit.caldav.rscale-unsupported"));
                }
                Ok(ClientStep::Request(request, decoder))
            }),
            policy,
        ))
    }
}

impl Discovery {
    /// Configure a protected RFC 7529 advertisement for the served discovery resource.
    /// An empty list advertises support without enumerating calendar names.
    pub fn with_supported_rscales(mut self, names: &[&str]) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        if names.len() > usize::try_from(policy.limits.max_xml_elements()).unwrap_or(usize::MAX) {
            return Err(too_large());
        }
        let mut meter = Meter::new(policy.limits);
        let mut validated = Vec::new();
        for name in names {
            validated.push(scale_name(name.as_bytes(), policy, &mut meter)?);
        }
        self.rscales = Some(validated);
        Ok(self)
    }

    /// The recurrence scale advertisement supplied with these service properties.
    #[must_use]
    pub fn advertised_rscales(&self) -> Option<&[String]> {
        self.rscales.as_deref()
    }
}

fn names_match(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
        || (is_gregorian_scale(left.as_bytes()) && is_gregorian_scale(right.as_bytes()))
}

pub(super) fn required_scales(
    calendar: &Calendar,
    policy: ResourcePolicy,
) -> Result<Vec<String>, Error> {
    let mut meter = Meter::new(policy.limits);
    scales_in_document(&calendar.document, policy, &mut meter)
}

fn scales_in_document(
    document: &Document,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Vec<String>, Error> {
    let mut pending: Vec<_> = document.components().collect();
    let mut names: Vec<String> = Vec::new();
    while let Some(component) = pending.pop() {
        if component.kind().is_none() {
            continue;
        }
        for property in component
            .properties()
            .filter(|property| property.is_named(b"RRULE"))
        {
            if !matches!(
                property.declared_value_type(),
                None | Some(Ok(ValueType::Recur))
            ) {
                continue;
            }
            let text = property.value_text();
            for part in
                parts(text.as_bytes()).filter(|part| part.name().eq_ignore_ascii_case(b"RSCALE"))
            {
                let name = scale_name(part.value(), policy, meter)?;
                if !names.iter().any(|held| names_match(held, &name)) {
                    names.push(name);
                }
            }
        }
        pending.extend(component.components());
    }
    Ok(names)
}

pub(super) fn unsupported_scale_body(bytes: &[u8], policy: ResourcePolicy) -> Result<bool, Error> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > policy.max_input_bytes() {
        return Err(too_large());
    }
    let mut meter = Meter::new(policy.limits);
    let mut notes = Vec::new();
    let mut reader = ContentLineReader::new(bytes, policy.limits.grammar());
    let document =
        Document::from_tokens(&mut reader, &mut meter, &mut notes).map_err(|_| invalid())?;
    let names = scales_in_document(&document, policy, &mut meter)?;
    Ok(names
        .iter()
        .any(|name| !is_gregorian_scale(name.as_bytes())))
}

pub(super) fn unsupported_put_operation(
    request: &WireRequest,
    policy: ResourcePolicy,
) -> ServerOperation {
    let need = ServerNeed::request("caldav.authorize", &request.method, &request.uri);
    ServerOperation::from_responder(
        need,
        Box::new(move |answer| {
            let ServerAnswerKind::Authorized(authorized) = answer.kind else {
                return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
            };
            let response = if authorized {
                unsupported_response(policy)?
            } else {
                WireResponse::new(403, Vec::new(), Vec::new())
            };
            Ok(ServerStep::Done(response))
        }),
        policy,
    )
}

fn unsupported_response(policy: ResourcePolicy) -> Result<WireResponse, Error> {
    let mut body = Vec::new();
    let mut meter = Meter::new(policy.limits);
    let mut writer = XmlWriter::new(&mut body);
    writer
        .open(ElementName::Error, &mut meter)
        .and_then(|()| writer.empty(ElementName::SupportedRscale, &mut meter))
        .and_then(|()| writer.close(&mut meter))
        .and_then(|()| writer.finish(&mut meter))
        .map_err(|_| too_large())?;
    Ok(WireResponse::new(
        403,
        alloc::vec![Header::new(
            "Content-Type",
            b"application/xml; charset=utf-8".to_vec()
        ),],
        body,
    ))
}

fn scale_name(bytes: &[u8], policy: ResourcePolicy, meter: &mut Meter) -> Result<String, Error> {
    if bytes.len() > usize::try_from(policy.limits.max_xml_text_bytes()).unwrap_or(usize::MAX) {
        return Err(too_large());
    }
    meter
        .try_charge_bytes(u64::try_from(bytes.len()).map_err(|_| too_large())?)
        .map_err(|_| too_large())?;
    if !is_iana_token(bytes) {
        return Err(invalid());
    }
    str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| invalid())
}

fn read_advertisement(
    response: &WireResponse,
    policy: ResourcePolicy,
) -> Result<Option<Vec<String>>, Error> {
    let mut meter = Meter::new(policy.limits);
    let body = read_multistatus_metered(response, policy, &mut meter)?;
    let [resource] = body.responses() else {
        return Err(invalid());
    };
    let mut found = None;
    for group in resource
        .propstats()
        .iter()
        .filter(|group| group.status == Status::OK)
    {
        for property in group.props() {
            if property.name != PropName::Known(ElementName::SupportedRscaleSet) {
                continue;
            }
            if found.is_some() {
                return Err(invalid());
            }
            found = Some(read_value(&property.value, policy, &mut meter)?);
        }
    }
    Ok(found)
}

fn read_value(
    value: &PropValue,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Vec<String>, Error> {
    let fragment = match value {
        PropValue::Empty => return Ok(Vec::new()),
        PropValue::Markup(fragment) => fragment.as_ref(),
        _ => return Err(invalid()),
    };
    let mut envelope = Vec::new();
    for bytes in [
        b"<C:supported-rscale-set xmlns:C=\"urn:ietf:params:xml:ns:caldav\">".as_slice(),
        fragment,
        b"</C:supported-rscale-set>",
    ] {
        meter
            .try_charge_bytes(u64::try_from(bytes.len()).map_err(|_| too_large())?)
            .map_err(|_| too_large())?;
        envelope.extend_from_slice(bytes);
    }
    let mut notes = Vec::new();
    let mut context = DecodeContext::new(policy.limits, meter, &mut notes);
    let mut events = XmlReader::new(&envelope);
    let mut parser = ScaleList::new();
    while let Some(event) = events.next_event(&mut context).map_err(|_| invalid())? {
        parser.event(event, policy, context.meter)?;
    }
    if parser.stage != ScaleStage::Finished {
        return Err(invalid());
    }
    Ok(parser.names)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScaleStage {
    Before,
    Container,
    Name,
    Finished,
}

struct ScaleList {
    stage: ScaleStage,
    names: Vec<String>,
    name: Vec<u8>,
}

impl ScaleList {
    fn new() -> Self {
        Self {
            stage: ScaleStage::Before,
            names: Vec::new(),
            name: Vec::new(),
        }
    }

    fn event(
        &mut self,
        event: XmlEvent<'_>,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<(), Error> {
        match event {
            XmlEvent::Start {
                known: Some(ElementName::SupportedRscaleSet),
                depth: 1,
                ..
            } if self.stage == ScaleStage::Before => self.stage = ScaleStage::Container,
            XmlEvent::Start {
                known: Some(ElementName::SupportedRscale),
                depth: 2,
                ..
            } if self.stage == ScaleStage::Container => {
                self.name.clear();
                self.stage = ScaleStage::Name;
            },
            XmlEvent::End {
                known: Some(ElementName::SupportedRscale),
                depth: 2,
                ..
            } if self.stage == ScaleStage::Name => {
                let bytes = self.name.trim_ascii();
                self.names.push(scale_name(bytes, policy, meter)?);
                self.stage = ScaleStage::Container;
            },
            XmlEvent::End {
                known: Some(ElementName::SupportedRscaleSet),
                depth: 1,
                ..
            } if self.stage == ScaleStage::Container => self.stage = ScaleStage::Finished,
            XmlEvent::Text(text) if self.stage == ScaleStage::Name => {
                let bytes = text.run.as_bytes();
                let length = self
                    .name
                    .len()
                    .checked_add(bytes.len())
                    .ok_or_else(too_large)?;
                if length
                    > usize::try_from(policy.limits.max_xml_text_bytes()).unwrap_or(usize::MAX)
                {
                    return Err(too_large());
                }
                meter
                    .try_charge_bytes(u64::try_from(bytes.len()).map_err(|_| too_large())?)
                    .map_err(|_| too_large())?;
                self.name.extend_from_slice(bytes);
            },
            XmlEvent::Text(text) if text.run.as_bytes().iter().all(u8::is_ascii_whitespace) => {},
            _ => return Err(invalid()),
        }
        Ok(())
    }
}

pub(super) fn advertisement_value(
    names: Option<&[String]>,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Option<PropValue>, Error> {
    let Some(names) = names else {
        return Ok(None);
    };
    if names.is_empty() {
        return Ok(Some(PropValue::Empty));
    }
    let mut fragment = Vec::new();
    let mut writer = XmlWriter::fragment(&mut fragment);
    for name in names {
        writer
            .element_text(ElementName::SupportedRscale, name.as_bytes(), meter)
            .map_err(|_| too_large())?;
    }
    writer.finish(meter).map_err(|_| too_large())?;
    if fragment.len() > usize::try_from(policy.limits.max_xml_text_bytes()).unwrap_or(usize::MAX) {
        return Err(too_large());
    }
    Ok(Some(PropValue::Markup(fragment.into_boxed_slice())))
}
