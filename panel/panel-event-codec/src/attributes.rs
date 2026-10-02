//! The single mapping between an envelope and CloudEvents context attributes.

use chrono::{DateTime, SecondsFormat, Utc};
use panel_errors::{PanelError, Result};
use panel_events::{
    parse_event_source, parse_qualified_event_type, Actor, AggregateRef, EventEnvelope,
    EventEnvelopeParts, EventId, EventPayload, IdempotencyKey, Principal, PrincipalKind, RequestId,
    TraceContext, CLOUDEVENTS_SPEC_VERSION,
};
use std::collections::BTreeMap;

pub(crate) const SPECVERSION: &str = "specversion";
pub(crate) const ID: &str = "id";
pub(crate) const SOURCE: &str = "source";
pub(crate) const TYPE: &str = "type";
pub(crate) const SUBJECT: &str = "subject";
pub(crate) const TIME: &str = "time";
pub(crate) const DATACONTENTTYPE: &str = "datacontenttype";
pub(crate) const DATASCHEMA: &str = "dataschema";
pub(crate) const CORRELATIONID: &str = "correlationid";
pub(crate) const CAUSATIONID: &str = "causationid";
pub(crate) const AUTHTYPE: &str = "authtype";
pub(crate) const AUTHID: &str = "authid";
pub(crate) const IDEMPOTENCYKEY: &str = "idempotencykey";
pub(crate) const TRACEPARENT: &str = "traceparent";
pub(crate) const TRACESTATE: &str = "tracestate";

/// CloudEvents abstract type of an attribute this crate writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    String,
    UriReference,
    Uri,
    Timestamp,
}

/// One context attribute in its canonical string encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attribute {
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
    pub(crate) value: String,
}

pub(crate) fn format_time(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| {
            PanelError::invalid_argument(format!("event time is not RFC 3339: {error}"))
        })
}

/// Every context attribute of `envelope`, required attributes first.
pub(crate) fn context_attributes(envelope: &EventEnvelope) -> Vec<Attribute> {
    let mut attributes = vec![
        Attribute {
            name: SPECVERSION,
            kind: Kind::String,
            value: CLOUDEVENTS_SPEC_VERSION.to_owned(),
        },
        Attribute {
            name: ID,
            kind: Kind::String,
            value: envelope.event_id().to_string(),
        },
        Attribute {
            name: SOURCE,
            kind: Kind::UriReference,
            value: envelope.source(),
        },
        Attribute {
            name: TYPE,
            kind: Kind::String,
            value: envelope.qualified_type(),
        },
        Attribute {
            name: SUBJECT,
            kind: Kind::String,
            value: envelope.aggregate().subject(),
        },
        Attribute {
            name: TIME,
            kind: Kind::Timestamp,
            value: format_time(envelope.occurred_at()),
        },
        Attribute {
            name: DATACONTENTTYPE,
            kind: Kind::String,
            value: envelope.payload().content_type().to_owned(),
        },
    ];
    if let Some(schema) = envelope.payload().schema() {
        attributes.push(Attribute {
            name: DATASCHEMA,
            kind: Kind::Uri,
            value: schema.to_owned(),
        });
    }
    attributes.push(Attribute {
        name: CORRELATIONID,
        kind: Kind::String,
        value: envelope.correlation_id().as_str().to_owned(),
    });
    attributes.push(Attribute {
        name: CAUSATIONID,
        kind: Kind::String,
        value: envelope.causation_id().as_str().to_owned(),
    });
    attributes.push(Attribute {
        name: AUTHTYPE,
        kind: Kind::String,
        value: envelope.principal().kind().as_str().to_owned(),
    });
    if let Some(id) = envelope.principal().id() {
        attributes.push(Attribute {
            name: AUTHID,
            kind: Kind::String,
            value: id.as_str().to_owned(),
        });
    }
    if let Some(key) = envelope.idempotency_key() {
        attributes.push(Attribute {
            name: IDEMPOTENCYKEY,
            kind: Kind::String,
            value: key.as_str().to_owned(),
        });
    }
    if let Some(trace) = envelope.trace_context() {
        attributes.push(Attribute {
            name: TRACEPARENT,
            kind: Kind::String,
            value: trace.traceparent().to_owned(),
        });
        if let Some(tracestate) = trace.tracestate() {
            attributes.push(Attribute {
                name: TRACESTATE,
                kind: Kind::String,
                value: tracestate.to_owned(),
            });
        }
    }
    attributes
}

/// Context attributes read from any representation, in canonical strings.
#[derive(Debug, Default)]
pub(crate) struct ReadAttributes(BTreeMap<String, String>);

impl ReadAttributes {
    /// Records one attribute. Names follow the CloudEvents naming rule and
    /// each attribute may appear at most once.
    pub(crate) fn insert(&mut self, name: &str, value: String) -> Result<()> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            return Err(PanelError::invalid_argument(format!(
                "CloudEvents attribute name {name:?} is not lowercase alphanumeric"
            )));
        }
        if self.0.insert(name.to_owned(), value).is_some() {
            return Err(PanelError::invalid_argument(format!(
                "CloudEvents attribute {name} appears more than once"
            )));
        }
        Ok(())
    }

    fn take(&mut self, name: &str) -> Option<String> {
        self.0.remove(name)
    }

    fn require(&mut self, name: &str) -> Result<String> {
        self.take(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                PanelError::invalid_argument(format!("CloudEvent has no {name} attribute"))
            })
    }

    pub(crate) fn content_type(&self) -> Option<&str> {
        self.0.get(DATACONTENTTYPE).map(String::as_str)
    }

    pub(crate) fn schema(&self) -> Option<&str> {
        self.0.get(DATASCHEMA).map(String::as_str)
    }

    /// Builds the envelope. `implied_content_type` applies when the
    /// representation defines a default for an absent `datacontenttype`.
    /// Unknown extension attributes are ignored, as recommended for consumers.
    pub(crate) fn into_envelope(
        mut self,
        data: Vec<u8>,
        implied_content_type: Option<&str>,
    ) -> Result<EventEnvelope> {
        let specversion = self.require(SPECVERSION)?;
        if specversion != CLOUDEVENTS_SPEC_VERSION {
            return Err(PanelError::invalid_argument(format!(
                "unsupported CloudEvents specversion {specversion}"
            )));
        }
        let (event_type, event_version) = parse_qualified_event_type(&self.require(TYPE)?)?;
        let content_type = match self.take(DATACONTENTTYPE) {
            Some(content_type) => content_type,
            None => implied_content_type
                .map(str::to_owned)
                .ok_or_else(|| PanelError::invalid_argument("CloudEvent has no datacontenttype"))?,
        };
        let mut payload = EventPayload::new(content_type, data)?;
        if let Some(schema) = self.take(DATASCHEMA) {
            payload = payload.with_schema(schema)?;
        }
        let principal = Principal::new(
            PrincipalKind::new(self.require(AUTHTYPE)?)?,
            self.take(AUTHID).map(Actor::new).transpose()?,
        );
        let tracestate = self.take(TRACESTATE);
        Ok(EventEnvelope::from_parts(EventEnvelopeParts {
            event_id: EventId::parse(&self.require(ID)?)?,
            event_type,
            event_version,
            occurred_at: parse_time(&self.require(TIME)?)?,
            producer: parse_event_source(&self.require(SOURCE)?)?,
            aggregate: AggregateRef::parse_subject(&self.require(SUBJECT)?)?,
            correlation_id: RequestId::new(self.require(CORRELATIONID)?)?,
            causation_id: RequestId::new(self.require(CAUSATIONID)?)?,
            principal,
            idempotency_key: self
                .take(IDEMPOTENCYKEY)
                .map(IdempotencyKey::new)
                .transpose()?,
            // Trace Context requires receivers to ignore invalid values
            // rather than reject the carrying message.
            trace_context: self
                .take(TRACEPARENT)
                .and_then(|traceparent| TraceContext::parse(&traceparent, tracestate.as_deref())),
            payload,
        }))
    }
}
