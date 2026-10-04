//! Times as the API's queries carry them.

use crate::error::ApiError;
use chrono::DateTime;
use panel_errors::PanelError;
use std::time::SystemTime;

/// An RFC 3339 time a query names as `name`; none when it is empty.
pub(crate) fn parse_time(name: &str, value: Option<&str>) -> Result<Option<SystemTime>, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(SystemTime::from)
                .map_err(|_| {
                    ApiError::new(PanelError::invalid_argument(format!(
                        "{name} is not an RFC 3339 time"
                    )))
                })
        })
        .transpose()
}
