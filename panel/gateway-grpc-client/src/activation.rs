//! Receipt decoding must preserve uncertainty after an acknowledged mutation.

use super::{hash, response_error};
use panel_application::ActivatedDeployment;
use panel_contracts::gateway::v1::ActivateResponse;
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result};

pub(super) fn decode(response: ActivateResponse) -> Result<ActivatedDeployment> {
    response_error(response.error)?;
    let receipt = || {
        Ok(ActivatedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.active_hash)?,
            response
                .previous_active_hash
                .map(|value| hash(Some(value)))
                .transpose()?,
        ))
    };
    receipt().map_err(|error: PanelError| {
        PanelError::commit_outcome_unknown(
            "gateway acknowledged activation but its receipt is invalid",
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
    fn malformed_acknowledgements_never_become_precommit_rejections() {
        for (active_hash, previous_active_hash) in [
            (None, None),
            (
                Some(common::ContentHash {
                    algorithm: "unknown".into(),
                    value: "a".repeat(64),
                }),
                None,
            ),
            (
                Some(common::ContentHash {
                    algorithm: "sha256".into(),
                    value: "invalid".into(),
                }),
                None,
            ),
            (
                Some(common::ContentHash {
                    algorithm: "sha256".into(),
                    value: "a".repeat(64),
                }),
                Some(common::ContentHash {
                    algorithm: "sha256".into(),
                    value: "invalid".into(),
                }),
            ),
        ] {
            let error = decode(ActivateResponse {
                active_hash,
                previous_active_hash,
                ..Default::default()
            })
            .unwrap_err();
            assert_eq!(error.code.as_str(), ErrorCode::COMMIT_OUTCOME_UNKNOWN);
        }
    }

    #[test]
    fn explicit_gateway_rejection_keeps_its_code() {
        let error = decode(ActivateResponse {
            error: Some(common::Error {
                code: ErrorCode::CONFLICT.into(),
                message: "CAS mismatch".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::CONFLICT);
    }
}
