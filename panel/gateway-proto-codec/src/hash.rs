//! Hash wire format shared by snapshots and mutation receipts.

use panel_contracts::common::v1 as common;
use panel_domain::ContentHash;
use panel_errors::{PanelError, Result};

pub fn decode_hash(value: Option<common::ContentHash>) -> Result<ContentHash> {
    let value = value.ok_or_else(|| PanelError::invalid_argument("content hash is required"))?;
    if value.algorithm != "sha256" {
        return Err(PanelError::invalid_argument(format!(
            "unsupported content hash algorithm {}",
            value.algorithm
        )));
    }
    ContentHash::from_hex(value.value)
        .map_err(|error| PanelError::invalid_argument(error.to_string()))
}

pub fn encode_hash(value: &ContentHash) -> common::ContentHash {
    common::ContentHash {
        algorithm: "sha256".into(),
        value: value.as_str().into(),
    }
}
