//! The only successful abort response is a confirmed removal.

use super::response_error;
use panel_application::AbortOutcome;
use panel_contracts::gateway::v1::AbortResponse;
use panel_errors::{PanelError, Result};

pub(super) fn decode(response: AbortResponse) -> Result<AbortOutcome> {
    response_error(response.error)?;
    if !response.aborted {
        return Err(PanelError::commit_outcome_unknown(
            "gateway acknowledged abort without confirming removal",
        ));
    }
    Ok(AbortOutcome::new(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_contracts::common::v1 as common;
    use panel_errors::ErrorCode;

    #[test]
    fn malformed_acknowledgement_is_uncertain() {
        let error = decode(AbortResponse::default()).unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::COMMIT_OUTCOME_UNKNOWN);
    }

    #[test]
    fn explicit_gateway_rejection_keeps_its_code() {
        let error = decode(AbortResponse {
            error: Some(common::Error {
                code: ErrorCode::NOT_FOUND.into(),
                message: "prepare token was not found".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::NOT_FOUND);
    }
}
