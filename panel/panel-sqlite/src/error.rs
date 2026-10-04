use panel_errors::PanelError;
use sqlx::error::ErrorKind;

/// Maps a database failure onto the stable error model.
///
/// Constraint violations are conflicts or invalid arguments, a file another
/// connection holds or a full disk are retryable storage errors, and a
/// damaged file is corrupt state. The driver error is kept as the source
/// and never serialized.
pub fn storage_error(error: sqlx::Error) -> PanelError {
    let mapped = match &error {
        sqlx::Error::Database(database) => match database.kind() {
            ErrorKind::UniqueViolation => {
                PanelError::conflict("database uniqueness constraint violated")
            }
            ErrorKind::ForeignKeyViolation
            | ErrorKind::NotNullViolation
            | ErrorKind::CheckViolation => {
                PanelError::invalid_argument("database integrity constraint violated")
            }
            _ => match primary_code(database.code().as_deref()) {
                // SQLITE_BUSY, SQLITE_LOCKED
                Some(5 | 6) => PanelError::storage_unavailable("database is busy"),
                // SQLITE_FULL
                Some(13) => PanelError::storage_unavailable("database disk is full"),
                // SQLITE_READONLY, SQLITE_IOERR, SQLITE_CANTOPEN, SQLITE_PROTOCOL
                Some(8 | 10 | 14 | 15) => {
                    PanelError::storage_unavailable("database is unavailable")
                }
                // SQLITE_CORRUPT, SQLITE_NOTADB
                Some(11 | 26) => PanelError::corrupt_state("database file is damaged"),
                // SQLITE_CONSTRAINT
                Some(19) => PanelError::invalid_argument("database integrity constraint violated"),
                _ => PanelError::internal("database statement failed"),
            },
        },
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
            PanelError::storage_unavailable("database is unavailable")
        }
        sqlx::Error::RowNotFound => PanelError::not_found("database row not found"),
        _ => PanelError::internal("database operation failed"),
    };
    mapped.with_source(error)
}

/// The primary result code of an extended SQLite result code.
fn primary_code(code: Option<&str>) -> Option<i32> {
    code?.parse::<i32>().ok().map(|code| code & 0xff)
}
