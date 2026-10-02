use panel_errors::PanelError;

/// Maps a database failure onto the stable error model.
///
/// SQLSTATE classes decide the code: privilege violations are permission
/// errors, integrity violations are conflicts or invalid arguments, and
/// connection, resource and serialization failures are retryable storage
/// errors. The driver error is kept as the source and never serialized.
pub fn storage_error(error: sqlx::Error) -> PanelError {
    let code = match &error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    };
    let mapped = match code.as_deref() {
        Some("42501") => PanelError::permission_denied("database privilege denied"),
        Some(code) if code.starts_with("28") => {
            PanelError::unauthenticated("database authentication failed")
        }
        Some("23505") => PanelError::conflict("database uniqueness constraint violated"),
        Some(code) if code.starts_with("23") => {
            PanelError::invalid_argument("database integrity constraint violated")
        }
        Some("40001" | "40P01") => {
            PanelError::storage_unavailable("database transaction must be retried")
        }
        Some("57014") => PanelError::deadline_exceeded("database statement timed out"),
        Some(code)
            if code.starts_with("08") || code.starts_with("53") || code.starts_with("57") =>
        {
            PanelError::storage_unavailable("database is unavailable")
        }
        Some(_) => PanelError::internal("database statement failed"),
        None => match &error {
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
                PanelError::storage_unavailable("database is unavailable")
            }
            sqlx::Error::RowNotFound => PanelError::not_found("database row not found"),
            _ => PanelError::internal("database operation failed"),
        },
    };
    mapped.with_source(error)
}
