//! Conservative classification of activation rejection codes.
//!
//! These codes are reserved for rejection before activation commits by the
//! GatewayPort contract. Storage, transport, timeout, and future error codes
//! provide no such evidence and must retain the claim for reconciliation.

use panel_errors::{ErrorCode, PanelError};

pub(super) fn confirmed_before_commit(error: &PanelError) -> bool {
    matches!(
        error.code.as_str(),
        ErrorCode::INVALID_ARGUMENT
            | ErrorCode::VALIDATION_FAILED
            | ErrorCode::CONFLICT
            | ErrorCode::NOT_FOUND
            | ErrorCode::PRECONDITION_FAILED
            | ErrorCode::UNSUPPORTED_CAPABILITY
            | ErrorCode::UNAUTHENTICATED
            | ErrorCode::PERMISSION_DENIED
    )
}
