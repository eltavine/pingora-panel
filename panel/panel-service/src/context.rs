//! The context every internal call carries, mapped onto
//! `common.v1.RequestContext` the same way by each client and server.

use panel_context::{
    CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope, SiteAccess,
    SiteScope, TraceContext,
};
use panel_contracts::{common::v1 as common, PROTOCOL_VERSION};
use panel_errors::{PanelError, Result};

/// The context an internal query carries for `scope`.
pub fn request_context(scope: &RequestScope) -> common::RequestContext {
    common::RequestContext {
        request_id: scope.request_id().as_str().into(),
        correlation_id: scope.correlation_id().as_str().into(),
        schema_version: PROTOCOL_VERSION.into(),
        site_scope: scope.site_scope().map(encode_site_scope),
        ..common::RequestContext::default()
    }
}

/// The context an internal command carries.
pub fn command_context(context: &CommandContext) -> common::RequestContext {
    common::RequestContext {
        request_id: context.request_id().as_str().into(),
        correlation_id: context.correlation_id().as_str().into(),
        actor: context.actor().into(),
        deadline: context.deadline().as_str().into(),
        idempotency_key: context.idempotency_key().as_str().into(),
        schema_version: PROTOCOL_VERSION.into(),
        site_scope: context.site_scope().map(encode_site_scope),
    }
}

fn required(value: Option<common::RequestContext>) -> Result<common::RequestContext> {
    value.ok_or_else(|| PanelError::invalid_argument("request context is required"))
}

/// The correlation identity a context names, its request's when it names
/// none.
fn correlation(value: &common::RequestContext, request_id: &RequestId) -> Result<RequestId> {
    if value.correlation_id.is_empty() {
        Ok(request_id.clone())
    } else {
        RequestId::new(value.correlation_id.clone())
    }
}

/// A query's scope; `trace` comes from transport metadata.
pub fn decode_scope(
    value: Option<common::RequestContext>,
    trace: Option<TraceContext>,
) -> Result<RequestScope> {
    let value = required(value)?;
    let request_id = RequestId::new(value.request_id.clone())?;
    let correlation_id = correlation(&value, &request_id)?;
    Ok(RequestScope::new(request_id)
        .with_correlation_id(correlation_id)
        .with_trace_context(trace)
        .with_site_scope(value.site_scope.map(decode_site_scope)))
}

/// A command's context; `trace` comes from transport metadata.
pub fn decode_command(
    value: Option<common::RequestContext>,
    trace: Option<TraceContext>,
) -> Result<CommandContext> {
    let value = required(value)?;
    let request_id = RequestId::new(value.request_id.clone())?;
    let correlation_id = correlation(&value, &request_id)?;
    Ok(CommandContext::new(
        request_id,
        correlation_id,
        value.actor,
        RequestDeadline::new(value.deadline)?,
        IdempotencyKey::new(value.idempotency_key)?,
    )?
    .with_trace_context(trace)
    .with_site_scope(value.site_scope.map(decode_site_scope)))
}

fn encode_site_scope(scope: &SiteScope) -> common::SiteScope {
    common::SiteScope {
        unrestricted: scope.unrestricted.clone(),
        limited: scope
            .limited
            .iter()
            .map(|access| common::SiteAccess {
                permission: access.permission.clone(),
                groups: access.groups.clone(),
                sites: access.sites.clone(),
            })
            .collect(),
    }
}

fn decode_site_scope(scope: common::SiteScope) -> SiteScope {
    SiteScope {
        unrestricted: scope.unrestricted,
        limited: scope
            .limited
            .into_iter()
            .map(|access| SiteAccess {
                permission: access.permission,
                groups: access.groups,
                sites: access.sites,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_scopes_round_trip_with_their_site_scope() {
        let scope = SiteScope {
            unrestricted: vec!["config.read".into()],
            limited: vec![SiteAccess {
                permission: "config.write".into(),
                groups: vec!["shop".into()],
                sites: Vec::new(),
            }],
        };
        let command = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("flow-1").unwrap(),
            "alice",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
        .with_site_scope(Some(scope));
        assert_eq!(
            decode_command(Some(command_context(&command)), None).unwrap(),
            command
        );
        // Trace context travels in transport metadata, beside the context.
        let trace = TraceContext::parse(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            None,
        );
        let traced = command.with_trace_context(trace.clone());
        assert_eq!(
            decode_command(Some(command_context(&traced)), trace.clone()).unwrap(),
            traced
        );
        assert_eq!(
            decode_scope(Some(request_context(&traced.scope())), trace).unwrap(),
            traced.scope()
        );
        assert!(decode_command(None, None).is_err());
        assert!(decode_command(Some(common::RequestContext::default()), None).is_err());
    }
}
