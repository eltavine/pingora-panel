use crate::{error::broker_error, JetStreamSettings};
use async_nats::jetstream::{
    context::GetStreamErrorKind,
    stream::{Config, DiscardPolicy, RetentionPolicy, StorageType},
    Context, ErrorCode,
};
use panel_errors::{PanelError, Result};
use panel_events::MAX_EVENT_BYTES;

/// Whether provisioning created, changed or kept a stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StreamChange {
    Created,
    Updated,
    Unchanged,
}

/// Creates or reconciles the event and dead-letter streams.
///
/// Mutable settings are updated in place. A stream whose storage or
/// retention differs is never recreated implicitly, because that would
/// delete its messages; provisioning fails instead.
pub async fn ensure_streams(
    context: &Context,
    settings: &JetStreamSettings,
) -> Result<Vec<(String, StreamChange)>> {
    let desired = [
        Config {
            name: settings.events_stream(),
            subjects: settings.event_subjects(),
            max_age: settings.events_max_age(),
            ..stream_defaults(settings)
        },
        Config {
            name: settings.dead_letter_stream(),
            subjects: settings.dead_letter_subjects(),
            max_age: settings.dead_letter_max_age(),
            ..stream_defaults(settings)
        },
    ];
    let mut changes = Vec::with_capacity(desired.len());
    for config in desired {
        let name = config.name.clone();
        let change = ensure_stream(context, config).await?;
        if change != StreamChange::Unchanged {
            tracing::info!(event = "jetstream_stream_provisioned", stream = %name, change = ?change);
        }
        changes.push((name, change));
    }
    Ok(changes)
}

fn stream_defaults(settings: &JetStreamSettings) -> Config {
    Config {
        retention: RetentionPolicy::Limits,
        storage: StorageType::File,
        discard: DiscardPolicy::Old,
        num_replicas: 1,
        duplicate_window: settings.duplicate_window(),
        max_message_size: i32::try_from(MAX_EVENT_BYTES * 2).unwrap_or(i32::MAX),
        allow_direct: true,
        ..Config::default()
    }
}

async fn ensure_stream(context: &Context, desired: Config) -> Result<StreamChange> {
    let existing = match context.get_stream(&desired.name).await {
        Ok(mut stream) => stream
            .info()
            .await
            .map_err(|error| broker_error("stream lookup", error))?
            .config
            .clone(),
        Err(error) => {
            let missing = matches!(
                error.kind(),
                GetStreamErrorKind::JetStream(source)
                    if source.error_code() == ErrorCode::STREAM_NOT_FOUND
            );
            if !missing {
                return Err(broker_error("stream lookup", error));
            }
            context
                .create_stream(desired)
                .await
                .map_err(|error| broker_error("stream creation", error))?;
            return Ok(StreamChange::Created);
        }
    };
    if existing.storage != desired.storage || existing.retention != desired.retention {
        return Err(PanelError::precondition_failed(format!(
            "stream {} exists with different storage or retention; migrate it explicitly",
            desired.name
        )));
    }
    let unchanged = existing.subjects == desired.subjects
        && existing.max_age == desired.max_age
        && existing.duplicate_window == desired.duplicate_window
        && existing.discard == desired.discard
        && existing.max_message_size == desired.max_message_size
        && existing.allow_direct == desired.allow_direct;
    if unchanged {
        return Ok(StreamChange::Unchanged);
    }
    context
        .update_stream(desired)
        .await
        .map_err(|error| broker_error("stream update", error))?;
    Ok(StreamChange::Updated)
}
