use core::{error::Error, fmt};

macro_rules! declare_lifecycle_enum {
    (
        $(#[$enum_attribute:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_attribute:meta])*
                $variant:ident
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_attribute])*
        pub enum $name {
            $(
                $(#[$variant_attribute])*
                $variant,
            )+
        }

        impl $name {
            /// Every value known to this version of the domain crate.
            ///
            /// Consumers must still handle unknown future values because the
            /// enum is non-exhaustive; this list exists for compatibility tests
            /// and capability discovery.
            pub const KNOWN: &'static [Self] = &[
                $(Self::$variant,)+
            ];
        }
    };
}

declare_lifecycle_enum! {
    /// Stable lifecycle state for one immutable configuration revision.
    ///
    /// The enum is non-exhaustive so adding a future state does not force downstream
    /// consumers to update exhaustive matches in lockstep.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    #[non_exhaustive]
    pub enum RevisionStatus {
        /// Editable revision which has not passed validation.
        Draft,
        /// Revision which passed validation and awaits an approval decision.
        Validated,
        /// Revision blocked until an independent approval is resolved.
        AwaitingApproval,
        /// Revision authorized to begin gateway preparation.
        Approved,
        /// Revision currently being prepared by the gateway.
        Preparing,
        /// Revision with a durable gateway preparation receipt.
        Prepared,
        /// Revision currently attempting compare-and-swap activation.
        Activating,
        /// Revision whose activation result must be reconciled with the gateway.
        Reconciling,
        /// Revision confirmed as the gateway's active configuration.
        Active,
        /// Formerly active revision replaced by a later revision.
        Superseded,
        /// Revision denied or expired during approval.
        Rejected,
        /// Revision which cannot continue after validation, preparation, or reconciliation.
        Failed,
    }
}

impl RevisionStatus {
    /// Returns true when no transition may leave this state.
    pub const fn is_terminal(self) -> bool {
        match self {
            Self::Draft
            | Self::Validated
            | Self::AwaitingApproval
            | Self::Approved
            | Self::Preparing
            | Self::Prepared
            | Self::Activating
            | Self::Reconciling
            | Self::Active => false,
            Self::Superseded | Self::Rejected | Self::Failed => true,
        }
    }
}

declare_lifecycle_enum! {
    /// A domain fact which requests one legal lifecycle transition.
    ///
    /// Failure details, actors, timestamps, and receipts belong to the surrounding
    /// aggregate or event envelope. Keeping them out of this enum makes transition
    /// legality independent from storage and transport schemas.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    #[non_exhaustive]
    pub enum RevisionTransition {
        /// Validation completed successfully.
        ValidationSucceeded,
        /// Validation found a terminal failure.
        ValidationFailed,
        /// Policy requires an independent approval.
        ApprovalRequired,
        /// Policy permits preparation without an approval.
        ApprovalNotRequired,
        /// An independent approver authorized the revision.
        ApprovalGranted,
        /// An approver rejected the revision.
        ApprovalRejected,
        /// The pending approval expired.
        ApprovalExpired,
        /// Gateway preparation began.
        PreparationStarted,
        /// Gateway preparation produced a durable receipt.
        PreparationSucceeded,
        /// Gateway preparation failed or timed out.
        PreparationFailed,
        /// Compare-and-swap activation began.
        ActivationStarted,
        /// Activation returned a receipt confirming the revision.
        ActivationSucceeded,
        /// Activation ended without a trustworthy outcome.
        ActivationOutcomeUnknown,
        /// Reconciliation confirmed that the gateway runs this revision.
        GatewayConfirmedRevision,
        /// Reconciliation confirmed that the gateway retained the previous revision.
        GatewayConfirmedPreviousRevision,
        /// A later revision replaced this active revision.
        LaterRevisionActivated,
    }
}

/// One validated revision lifecycle value.
///
/// The private field prevents constructing a state through an invalid sequence.
/// The rehydrate constructor is intentionally explicit for persistence adapters
/// which already validated their versioned representation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RevisionState {
    status: RevisionStatus,
}

impl RevisionState {
    /// Starts a newly created revision in Draft.
    pub const fn new() -> Self {
        Self {
            status: RevisionStatus::Draft,
        }
    }

    /// Restores a previously validated status at an adapter boundary.
    pub const fn rehydrate(status: RevisionStatus) -> Self {
        Self { status }
    }

    /// Returns the current lifecycle status.
    pub const fn status(self) -> RevisionStatus {
        self.status
    }

    /// Checks transition legality without mutating or consuming external state.
    pub const fn allows(self, transition: RevisionTransition) -> bool {
        self.transition_target(transition).is_some()
    }

    /// Returns the target status for a legal transition without applying it.
    ///
    /// Presentation and transport layers can use this to advertise capabilities
    /// without duplicating the lifecycle table or constructing speculative state.
    pub const fn transition_target(self, transition: RevisionTransition) -> Option<RevisionStatus> {
        next_status(self.status, transition)
    }

    /// Iterates over every transition currently legal from this state.
    ///
    /// The iterator borrows no external state and allocates nothing. Its order is
    /// the stable capability-discovery order exposed by [`RevisionTransition::KNOWN`].
    pub fn allowed_transitions(self) -> impl Iterator<Item = RevisionTransition> {
        RevisionTransition::KNOWN
            .iter()
            .copied()
            .filter(move |transition| self.allows(*transition))
    }

    /// Returns the next immutable state or a typed rejection.
    ///
    /// Because the current value is copied and publication happens only after a
    /// successful return, rejected transitions cannot leave partial state.
    pub const fn transition(
        self,
        transition: RevisionTransition,
    ) -> Result<Self, RevisionTransitionError> {
        match self.transition_with_outcome(transition) {
            Ok(outcome) => Ok(outcome.next_state()),
            Err(error) => Err(error),
        }
    }

    /// Applies one transition and returns its immutable before/event/after fact.
    ///
    /// Consumers can persist or publish the returned outcome without separately
    /// reconstructing the previous state. The original value remains unchanged.
    pub const fn transition_with_outcome(
        self,
        transition: RevisionTransition,
    ) -> Result<RevisionTransitionOutcome, RevisionTransitionError> {
        match next_status(self.status, transition) {
            Some(status) => Ok(RevisionTransitionOutcome::new(
                self.status,
                transition,
                Self { status },
            )),
            None => Err(RevisionTransitionError::new(self.status, transition)),
        }
    }
}

impl Default for RevisionState {
    fn default() -> Self {
        Self::new()
    }
}

const fn next_status(
    status: RevisionStatus,
    transition: RevisionTransition,
) -> Option<RevisionStatus> {
    // Matching the event exhaustively keeps future enum additions from silently
    // becoming transitions which are rejected from every state.
    match transition {
        RevisionTransition::ValidationSucceeded => match status {
            RevisionStatus::Draft => Some(RevisionStatus::Validated),
            _ => None,
        },
        RevisionTransition::ValidationFailed => match status {
            RevisionStatus::Draft => Some(RevisionStatus::Failed),
            _ => None,
        },
        RevisionTransition::ApprovalRequired => match status {
            RevisionStatus::Validated => Some(RevisionStatus::AwaitingApproval),
            _ => None,
        },
        RevisionTransition::ApprovalNotRequired => match status {
            RevisionStatus::Validated => Some(RevisionStatus::Approved),
            _ => None,
        },
        RevisionTransition::ApprovalGranted => match status {
            RevisionStatus::AwaitingApproval => Some(RevisionStatus::Approved),
            _ => None,
        },
        RevisionTransition::ApprovalRejected | RevisionTransition::ApprovalExpired => {
            match status {
                RevisionStatus::AwaitingApproval => Some(RevisionStatus::Rejected),
                _ => None,
            }
        }
        RevisionTransition::PreparationStarted => match status {
            RevisionStatus::Approved => Some(RevisionStatus::Preparing),
            _ => None,
        },
        RevisionTransition::PreparationSucceeded => match status {
            RevisionStatus::Preparing => Some(RevisionStatus::Prepared),
            _ => None,
        },
        RevisionTransition::PreparationFailed => match status {
            RevisionStatus::Preparing => Some(RevisionStatus::Failed),
            _ => None,
        },
        RevisionTransition::ActivationStarted => match status {
            RevisionStatus::Prepared => Some(RevisionStatus::Activating),
            _ => None,
        },
        RevisionTransition::ActivationSucceeded => match status {
            RevisionStatus::Activating => Some(RevisionStatus::Active),
            _ => None,
        },
        RevisionTransition::ActivationOutcomeUnknown => match status {
            RevisionStatus::Activating => Some(RevisionStatus::Reconciling),
            _ => None,
        },
        RevisionTransition::GatewayConfirmedRevision => match status {
            RevisionStatus::Reconciling => Some(RevisionStatus::Active),
            _ => None,
        },
        RevisionTransition::GatewayConfirmedPreviousRevision => match status {
            RevisionStatus::Reconciling => Some(RevisionStatus::Failed),
            _ => None,
        },
        RevisionTransition::LaterRevisionActivated => match status {
            RevisionStatus::Active => Some(RevisionStatus::Superseded),
            _ => None,
        },
    }
}

/// Immutable evidence of one successfully applied lifecycle transition.
///
/// This value is intentionally storage- and transport-neutral. Adapters may
/// attach identifiers, actors, timestamps, or receipts in their own envelopes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct RevisionTransitionOutcome {
    from: RevisionStatus,
    transition: RevisionTransition,
    next: RevisionState,
}

impl RevisionTransitionOutcome {
    const fn new(
        from: RevisionStatus,
        transition: RevisionTransition,
        next: RevisionState,
    ) -> Self {
        Self {
            from,
            transition,
            next,
        }
    }

    /// Returns the lifecycle state before the transition.
    pub const fn from_status(self) -> RevisionStatus {
        self.from
    }

    /// Returns the transition which was successfully applied.
    pub const fn applied_transition(self) -> RevisionTransition {
        self.transition
    }

    /// Returns the lifecycle state after the transition.
    pub const fn to_status(self) -> RevisionStatus {
        self.next.status()
    }

    /// Returns the next immutable revision state.
    pub const fn next_state(self) -> RevisionState {
        self.next
    }
}

/// Typed evidence that a transition is invalid from the current state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RevisionTransitionError {
    from: RevisionStatus,
    transition: RevisionTransition,
}

impl RevisionTransitionError {
    const fn new(from: RevisionStatus, transition: RevisionTransition) -> Self {
        Self { from, transition }
    }

    /// Returns the lifecycle state from which the transition was attempted.
    pub const fn from_status(self) -> RevisionStatus {
        self.from
    }

    /// Returns the rejected transition.
    pub const fn attempted_transition(self) -> RevisionTransition {
        self.transition
    }
}

impl fmt::Display for RevisionTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "revision transition {:?} is invalid from {:?}",
            self.transition, self.from
        )
    }
}

impl Error for RevisionTransitionError {}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGAL: [(RevisionStatus, RevisionTransition, RevisionStatus); 16] = [
        (
            RevisionStatus::Draft,
            RevisionTransition::ValidationSucceeded,
            RevisionStatus::Validated,
        ),
        (
            RevisionStatus::Draft,
            RevisionTransition::ValidationFailed,
            RevisionStatus::Failed,
        ),
        (
            RevisionStatus::Validated,
            RevisionTransition::ApprovalRequired,
            RevisionStatus::AwaitingApproval,
        ),
        (
            RevisionStatus::Validated,
            RevisionTransition::ApprovalNotRequired,
            RevisionStatus::Approved,
        ),
        (
            RevisionStatus::AwaitingApproval,
            RevisionTransition::ApprovalGranted,
            RevisionStatus::Approved,
        ),
        (
            RevisionStatus::AwaitingApproval,
            RevisionTransition::ApprovalRejected,
            RevisionStatus::Rejected,
        ),
        (
            RevisionStatus::AwaitingApproval,
            RevisionTransition::ApprovalExpired,
            RevisionStatus::Rejected,
        ),
        (
            RevisionStatus::Approved,
            RevisionTransition::PreparationStarted,
            RevisionStatus::Preparing,
        ),
        (
            RevisionStatus::Preparing,
            RevisionTransition::PreparationSucceeded,
            RevisionStatus::Prepared,
        ),
        (
            RevisionStatus::Preparing,
            RevisionTransition::PreparationFailed,
            RevisionStatus::Failed,
        ),
        (
            RevisionStatus::Prepared,
            RevisionTransition::ActivationStarted,
            RevisionStatus::Activating,
        ),
        (
            RevisionStatus::Activating,
            RevisionTransition::ActivationSucceeded,
            RevisionStatus::Active,
        ),
        (
            RevisionStatus::Activating,
            RevisionTransition::ActivationOutcomeUnknown,
            RevisionStatus::Reconciling,
        ),
        (
            RevisionStatus::Reconciling,
            RevisionTransition::GatewayConfirmedRevision,
            RevisionStatus::Active,
        ),
        (
            RevisionStatus::Reconciling,
            RevisionTransition::GatewayConfirmedPreviousRevision,
            RevisionStatus::Failed,
        ),
        (
            RevisionStatus::Active,
            RevisionTransition::LaterRevisionActivated,
            RevisionStatus::Superseded,
        ),
    ];

    #[test]
    fn every_declared_transition_reaches_its_expected_state() {
        for (from, transition, expected) in LEGAL {
            let current = RevisionState::rehydrate(from);
            assert!(current.allows(transition));
            assert_eq!(current.transition_target(transition), Some(expected));
            let outcome = current.transition_with_outcome(transition).unwrap();
            assert_eq!(outcome.from_status(), from);
            assert_eq!(outcome.applied_transition(), transition);
            assert_eq!(outcome.to_status(), expected);
            assert_eq!(outcome.next_state().status(), expected);
            assert_eq!(
                current.transition(transition).unwrap(),
                outcome.next_state()
            );
            assert_eq!(current.status(), from);
        }
    }

    #[test]
    fn every_transition_has_a_declared_legal_source() {
        for transition in RevisionTransition::KNOWN.iter().copied() {
            assert!(
                LEGAL.iter().any(|(_, declared, _)| *declared == transition),
                "{transition:?} has no declared legal source"
            );
        }
    }

    #[test]
    fn every_other_state_and_transition_pair_is_rejected_with_typed_context() {
        for status in RevisionStatus::KNOWN.iter().copied() {
            for transition in RevisionTransition::KNOWN.iter().copied() {
                let expected = LEGAL
                    .iter()
                    .find(|(from, event, _)| *from == status && *event == transition);
                if expected.is_some() {
                    continue;
                }

                let current = RevisionState::rehydrate(status);
                assert!(!current.allows(transition));
                let error = current.transition_with_outcome(transition).unwrap_err();
                assert_eq!(error.from_status(), status);
                assert_eq!(error.attempted_transition(), transition);
                assert_eq!(current.transition(transition).unwrap_err(), error);
                assert_eq!(current.status(), status);
            }
        }
    }

    #[test]
    fn terminal_statuses_have_no_outgoing_transitions() {
        for status in RevisionStatus::KNOWN.iter().copied() {
            let has_outgoing = RevisionTransition::KNOWN
                .iter()
                .copied()
                .any(|transition| RevisionState::rehydrate(status).allows(transition));
            assert_eq!(status.is_terminal(), !has_outgoing);
        }
    }

    #[test]
    fn discovery_projects_the_transition_table_without_duplicates() {
        for status in RevisionStatus::KNOWN.iter().copied() {
            let expected = LEGAL
                .iter()
                .filter_map(|(from, transition, _)| (*from == status).then_some(*transition));
            let discovered = RevisionState::rehydrate(status).allowed_transitions();

            assert!(discovered.eq(expected));
        }
    }
}
