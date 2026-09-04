use panel_config_domain::{RevisionStatus, RevisionTransition};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(crate) struct RevisionDocumentHeader {
    pub(crate) schema: String,
    pub(crate) version: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) enum WireRevisionStatusV1 {
    #[serde(rename = "draft")]
    Draft,
    #[serde(rename = "validated")]
    Validated,
    #[serde(rename = "awaiting_approval")]
    AwaitingApproval,
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "prepared")]
    Prepared,
    #[serde(rename = "activating")]
    Activating,
    #[serde(rename = "reconciling")]
    Reconciling,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "superseded")]
    Superseded,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "failed")]
    Failed,
}

impl TryFrom<RevisionStatus> for WireRevisionStatusV1 {
    type Error = RevisionStatus;

    fn try_from(status: RevisionStatus) -> Result<Self, Self::Error> {
        match status {
            RevisionStatus::Draft => Ok(Self::Draft),
            RevisionStatus::Validated => Ok(Self::Validated),
            RevisionStatus::AwaitingApproval => Ok(Self::AwaitingApproval),
            RevisionStatus::Approved => Ok(Self::Approved),
            RevisionStatus::Preparing => Ok(Self::Preparing),
            RevisionStatus::Prepared => Ok(Self::Prepared),
            RevisionStatus::Activating => Ok(Self::Activating),
            RevisionStatus::Reconciling => Ok(Self::Reconciling),
            RevisionStatus::Active => Ok(Self::Active),
            RevisionStatus::Superseded => Ok(Self::Superseded),
            RevisionStatus::Rejected => Ok(Self::Rejected),
            RevisionStatus::Failed => Ok(Self::Failed),
            _ => Err(status),
        }
    }
}

impl From<WireRevisionStatusV1> for RevisionStatus {
    fn from(status: WireRevisionStatusV1) -> Self {
        match status {
            WireRevisionStatusV1::Draft => Self::Draft,
            WireRevisionStatusV1::Validated => Self::Validated,
            WireRevisionStatusV1::AwaitingApproval => Self::AwaitingApproval,
            WireRevisionStatusV1::Approved => Self::Approved,
            WireRevisionStatusV1::Preparing => Self::Preparing,
            WireRevisionStatusV1::Prepared => Self::Prepared,
            WireRevisionStatusV1::Activating => Self::Activating,
            WireRevisionStatusV1::Reconciling => Self::Reconciling,
            WireRevisionStatusV1::Active => Self::Active,
            WireRevisionStatusV1::Superseded => Self::Superseded,
            WireRevisionStatusV1::Rejected => Self::Rejected,
            WireRevisionStatusV1::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) enum WireRevisionTransitionV1 {
    #[serde(rename = "validation_succeeded")]
    ValidationSucceeded,
    #[serde(rename = "validation_failed")]
    ValidationFailed,
    #[serde(rename = "approval_required")]
    ApprovalRequired,
    #[serde(rename = "approval_not_required")]
    ApprovalNotRequired,
    #[serde(rename = "approval_granted")]
    ApprovalGranted,
    #[serde(rename = "approval_rejected")]
    ApprovalRejected,
    #[serde(rename = "approval_expired")]
    ApprovalExpired,
    #[serde(rename = "preparation_started")]
    PreparationStarted,
    #[serde(rename = "preparation_succeeded")]
    PreparationSucceeded,
    #[serde(rename = "preparation_failed")]
    PreparationFailed,
    #[serde(rename = "activation_started")]
    ActivationStarted,
    #[serde(rename = "activation_succeeded")]
    ActivationSucceeded,
    #[serde(rename = "activation_outcome_unknown")]
    ActivationOutcomeUnknown,
    #[serde(rename = "gateway_confirmed_revision")]
    GatewayConfirmedRevision,
    #[serde(rename = "gateway_confirmed_previous_revision")]
    GatewayConfirmedPreviousRevision,
    #[serde(rename = "later_revision_activated")]
    LaterRevisionActivated,
}

impl TryFrom<RevisionTransition> for WireRevisionTransitionV1 {
    type Error = RevisionTransition;

    fn try_from(transition: RevisionTransition) -> Result<Self, Self::Error> {
        match transition {
            RevisionTransition::ValidationSucceeded => Ok(Self::ValidationSucceeded),
            RevisionTransition::ValidationFailed => Ok(Self::ValidationFailed),
            RevisionTransition::ApprovalRequired => Ok(Self::ApprovalRequired),
            RevisionTransition::ApprovalNotRequired => Ok(Self::ApprovalNotRequired),
            RevisionTransition::ApprovalGranted => Ok(Self::ApprovalGranted),
            RevisionTransition::ApprovalRejected => Ok(Self::ApprovalRejected),
            RevisionTransition::ApprovalExpired => Ok(Self::ApprovalExpired),
            RevisionTransition::PreparationStarted => Ok(Self::PreparationStarted),
            RevisionTransition::PreparationSucceeded => Ok(Self::PreparationSucceeded),
            RevisionTransition::PreparationFailed => Ok(Self::PreparationFailed),
            RevisionTransition::ActivationStarted => Ok(Self::ActivationStarted),
            RevisionTransition::ActivationSucceeded => Ok(Self::ActivationSucceeded),
            RevisionTransition::ActivationOutcomeUnknown => Ok(Self::ActivationOutcomeUnknown),
            RevisionTransition::GatewayConfirmedRevision => Ok(Self::GatewayConfirmedRevision),
            RevisionTransition::GatewayConfirmedPreviousRevision => {
                Ok(Self::GatewayConfirmedPreviousRevision)
            }
            RevisionTransition::LaterRevisionActivated => Ok(Self::LaterRevisionActivated),
            _ => Err(transition),
        }
    }
}

impl From<WireRevisionTransitionV1> for RevisionTransition {
    fn from(transition: WireRevisionTransitionV1) -> Self {
        match transition {
            WireRevisionTransitionV1::ValidationSucceeded => Self::ValidationSucceeded,
            WireRevisionTransitionV1::ValidationFailed => Self::ValidationFailed,
            WireRevisionTransitionV1::ApprovalRequired => Self::ApprovalRequired,
            WireRevisionTransitionV1::ApprovalNotRequired => Self::ApprovalNotRequired,
            WireRevisionTransitionV1::ApprovalGranted => Self::ApprovalGranted,
            WireRevisionTransitionV1::ApprovalRejected => Self::ApprovalRejected,
            WireRevisionTransitionV1::ApprovalExpired => Self::ApprovalExpired,
            WireRevisionTransitionV1::PreparationStarted => Self::PreparationStarted,
            WireRevisionTransitionV1::PreparationSucceeded => Self::PreparationSucceeded,
            WireRevisionTransitionV1::PreparationFailed => Self::PreparationFailed,
            WireRevisionTransitionV1::ActivationStarted => Self::ActivationStarted,
            WireRevisionTransitionV1::ActivationSucceeded => Self::ActivationSucceeded,
            WireRevisionTransitionV1::ActivationOutcomeUnknown => Self::ActivationOutcomeUnknown,
            WireRevisionTransitionV1::GatewayConfirmedRevision => Self::GatewayConfirmedRevision,
            WireRevisionTransitionV1::GatewayConfirmedPreviousRevision => {
                Self::GatewayConfirmedPreviousRevision
            }
            WireRevisionTransitionV1::LaterRevisionActivated => Self::LaterRevisionActivated,
        }
    }
}
