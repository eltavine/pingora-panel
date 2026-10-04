//! Logging settings on the wire (ADR 0025).

use panel_contracts::gateway::v1 as wire;
use panel_errors::{PanelError, Result};
use panel_ir::{AccessLog, AccessLogFormat, LogFiles, LoggingPolicy};

pub(super) fn decode_access_log(value: Option<wire::AccessLog>) -> Result<AccessLog> {
    let Some(value) = value else {
        return Ok(AccessLog::default());
    };
    let format = match wire::AccessLogFormat::try_from(value.format) {
        Ok(wire::AccessLogFormat::Unspecified) => None,
        Ok(wire::AccessLogFormat::Json) => Some(AccessLogFormat::Json),
        Ok(wire::AccessLogFormat::Combined) => Some(AccessLogFormat::Combined),
        Err(_) => {
            return Err(PanelError::invalid_argument(format!(
                "unknown access log format {}",
                value.format
            )))
        }
    };
    Ok(AccessLog {
        enabled: value.enabled,
        format,
        fields: value.fields.into_iter().collect(),
    })
}

pub(super) fn encode_access_log(value: &AccessLog) -> Option<wire::AccessLog> {
    let format = match value.format {
        None => wire::AccessLogFormat::Unspecified,
        Some(AccessLogFormat::Json) => wire::AccessLogFormat::Json,
        Some(AccessLogFormat::Combined) => wire::AccessLogFormat::Combined,
    };
    (!value.is_unset()).then(|| wire::AccessLog {
        enabled: value.enabled,
        format: format.into(),
        fields: value
            .fields
            .iter()
            .map(|(name, template)| (name.clone(), template.clone()))
            .collect(),
    })
}

pub(super) fn decode_logging(value: Option<wire::LoggingPolicy>) -> Result<LoggingPolicy> {
    let Some(value) = value else {
        return Ok(LoggingPolicy::default());
    };
    Ok(LoggingPolicy {
        access: decode_access_log(value.access)?,
        files: value
            .files
            .map_or_else(LogFiles::default, |files| LogFiles {
                max_size_bytes: files.max_size_bytes,
                rotate_daily: files.rotate_daily,
                keep_days: files.keep_days,
                max_files: files.max_files,
            }),
        redact_query: value
            .redact_query
            .map(|query| query.keys.into_iter().collect()),
        redact_headers: value.redact_headers.into_iter().collect(),
    })
}

pub(super) fn encode_logging(value: &LoggingPolicy) -> Option<wire::LoggingPolicy> {
    (!value.is_default()).then(|| wire::LoggingPolicy {
        access: encode_access_log(&value.access),
        files: (!value.files.is_default()).then_some(wire::LogFiles {
            max_size_bytes: value.files.max_size_bytes,
            rotate_daily: value.files.rotate_daily,
            keep_days: value.files.keep_days,
            max_files: value.files.max_files,
        }),
        redact_query: value.redact_query.as_ref().map(|keys| wire::RedactedQuery {
            keys: keys.iter().cloned().collect(),
        }),
        redact_headers: value.redact_headers.iter().cloned().collect(),
    })
}
