use panel_errors::{PanelError, Result};
use panel_ir::RuntimeSnapshot;
use std::num::NonZeroUsize;

pub const DEFAULT_MAX_OUTSTANDING_PREPARES: usize = 64;
pub const DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_TOTAL_PREPARED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedSnapshotUsage {
    pub outstanding: usize,
    pub total_bytes: usize,
}

/// Admission port for outstanding prepared snapshots.
///
/// Custom policies may inspect snapshot content or apply tenant-aware accounting
/// without changing the durable engine.
pub trait PreparedSnapshotAdmissionPolicy: Send + Sync {
    /// Upper bound passed down to stores so efficient adapters can reject excess
    /// records before loading their contents.
    fn restoration_limit(&self) -> usize;

    fn admit(&self, usage: PreparedSnapshotUsage, candidate: &RuntimeSnapshot) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedSnapshotBudget {
    max_outstanding: NonZeroUsize,
    max_snapshot_bytes: NonZeroUsize,
    max_total_bytes: NonZeroUsize,
}

impl PreparedSnapshotBudget {
    pub fn new(max_outstanding: usize) -> Result<Self> {
        Self::with_limits(
            max_outstanding,
            DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES,
            DEFAULT_MAX_TOTAL_PREPARED_BYTES,
        )
    }

    pub fn with_limits(
        max_outstanding: usize,
        max_snapshot_bytes: usize,
        max_total_bytes: usize,
    ) -> Result<Self> {
        let positive = |value, name| {
            NonZeroUsize::new(value)
                .ok_or_else(|| PanelError::invalid_argument(format!("{name} must be non-zero")))
        };
        let max_outstanding = positive(max_outstanding, "prepared snapshot count limit")?;
        let max_snapshot_bytes = positive(max_snapshot_bytes, "prepared snapshot size limit")?;
        let max_total_bytes = positive(max_total_bytes, "total prepared snapshot size limit")?;
        if max_snapshot_bytes > max_total_bytes {
            return Err(PanelError::invalid_argument(
                "prepared snapshot size limit must not exceed its total size limit",
            ));
        }
        Ok(Self {
            max_outstanding,
            max_snapshot_bytes,
            max_total_bytes,
        })
    }

    pub fn max_outstanding(self) -> usize {
        self.max_outstanding.get()
    }

    pub fn max_snapshot_bytes(self) -> usize {
        self.max_snapshot_bytes.get()
    }

    pub fn max_total_bytes(self) -> usize {
        self.max_total_bytes.get()
    }
}

impl Default for PreparedSnapshotBudget {
    fn default() -> Self {
        Self {
            max_outstanding: NonZeroUsize::new(DEFAULT_MAX_OUTSTANDING_PREPARES)
                .expect("default prepared budget is non-zero"),
            max_snapshot_bytes: NonZeroUsize::new(DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES)
                .expect("default snapshot size limit is non-zero"),
            max_total_bytes: NonZeroUsize::new(DEFAULT_MAX_TOTAL_PREPARED_BYTES)
                .expect("default total size limit is non-zero"),
        }
    }
}

impl PreparedSnapshotAdmissionPolicy for PreparedSnapshotBudget {
    fn restoration_limit(&self) -> usize {
        self.max_outstanding()
    }

    fn admit(&self, usage: PreparedSnapshotUsage, candidate: &RuntimeSnapshot) -> Result<()> {
        if usage.outstanding >= self.max_outstanding() {
            return Err(PanelError::resource_exhausted(format!(
                "outstanding prepared snapshot limit {} has been reached",
                self.max_outstanding()
            )));
        }
        let candidate_bytes = candidate.canonical_bytes().len();
        if candidate_bytes > self.max_snapshot_bytes() {
            return Err(PanelError::resource_exhausted(format!(
                "prepared snapshot exceeds the {} byte limit",
                self.max_snapshot_bytes()
            )));
        }
        if usage
            .total_bytes
            .checked_add(candidate_bytes)
            .is_none_or(|total| total > self.max_total_bytes())
        {
            return Err(PanelError::resource_exhausted(format!(
                "prepared snapshots exceed the {} byte aggregate limit",
                self.max_total_bytes()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::RevisionId;

    #[test]
    fn budget_rejects_zero_and_enforces_its_limit() {
        assert_eq!(
            PreparedSnapshotBudget::new(0).unwrap_err().code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
        let policy = PreparedSnapshotBudget::new(1).unwrap();
        let candidate = RuntimeSnapshot::empty(RevisionId::new(1));
        assert!(policy
            .admit(
                PreparedSnapshotUsage {
                    outstanding: 0,
                    total_bytes: 0,
                },
                &candidate,
            )
            .is_ok());
        assert_eq!(
            policy
                .admit(
                    PreparedSnapshotUsage {
                        outstanding: 1,
                        total_bytes: 0,
                    },
                    &candidate,
                )
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );

        let tiny = PreparedSnapshotBudget::with_limits(1, 1, 1).unwrap();
        assert_eq!(
            tiny.admit(
                PreparedSnapshotUsage {
                    outstanding: 0,
                    total_bytes: 0,
                },
                &candidate,
            )
            .unwrap_err()
            .code
            .as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );

        let candidate_bytes = candidate.canonical_bytes().len();
        let aggregate =
            PreparedSnapshotBudget::with_limits(2, candidate_bytes, candidate_bytes).unwrap();
        assert_eq!(
            aggregate
                .admit(
                    PreparedSnapshotUsage {
                        outstanding: 1,
                        total_bytes: candidate_bytes,
                    },
                    &candidate,
                )
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );
    }
}
