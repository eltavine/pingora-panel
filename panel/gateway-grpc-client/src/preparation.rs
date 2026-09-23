//! Treat malformed success receipts as uncertain after a prepare may have committed.

use super::{hash, response_error};
use panel_application::PreparedDeployment;
use panel_contracts::gateway::v1::PrepareResponse;
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result};

pub(super) fn decode(response: PrepareResponse) -> Result<PreparedDeployment> {
    response_error(response.error)?;
    let receipt = || {
        PreparedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.content_hash)?,
            response.prepare_token,
        )
    };
    receipt().map_err(|error: PanelError| {
        PanelError::commit_outcome_unknown(
            "gateway acknowledged prepare but its receipt is invalid",
        )
        .with_source(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_contracts::common::v1 as common;
    use panel_errors::ErrorCode;

    #[test]
    fn malformed_acknowledgements_preserve_prepare_uncertainty() {
        for response in [
            PrepareResponse::default(),
            PrepareResponse {
                content_hash: Some(common::ContentHash {
                    algorithm: "sha256".into(),
                    value: "a".repeat(64),
                }),
                ..Default::default()
            },
            PrepareResponse {
                prepare_token: "token".into(),
                content_hash: Some(common::ContentHash {
                    algorithm: "sha256".into(),
                    value: "invalid".into(),
                }),
                ..Default::default()
            },
        ] {
            let error = decode(response).unwrap_err();
            assert_eq!(error.code.as_str(), ErrorCode::COMMIT_OUTCOME_UNKNOWN);
        }
    }

    #[test]
    fn explicit_gateway_rejection_keeps_its_code() {
        let error = decode(PrepareResponse {
            error: Some(common::Error {
                code: ErrorCode::VALIDATION_FAILED.into(),
                message: "invalid snapshot".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::VALIDATION_FAILED);
    }
}
