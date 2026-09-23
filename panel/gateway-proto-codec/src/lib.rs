#![forbid(unsafe_code)]

//! Shared Proto ↔ engine-neutral IR conversion for transport adapters.
//! No server, client, engine or runtime dependency belongs in this crate.
//! Conversion modules are private; only the snapshot and hash facade is public.

use panel_errors::{PanelError, Result};

mod hash;
mod policies;
mod routing;
mod snapshot;
#[cfg(test)]
mod tests;
mod upstream;

pub use hash::{decode_hash, encode_hash};
pub use snapshot::{decode_snapshot, encode_snapshot};

fn optional_string(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn status_code(value: u32) -> Result<u16> {
    let value = u16::try_from(value)
        .map_err(|_| PanelError::invalid_argument("HTTP status exceeds 65535"))?;
    if !(100..=599).contains(&value) {
        return Err(PanelError::invalid_argument(format!(
            "invalid HTTP status {value}"
        )));
    }
    Ok(value)
}

fn domain_error(error: panel_domain::DomainError) -> PanelError {
    PanelError::invalid_argument(error.to_string())
}
