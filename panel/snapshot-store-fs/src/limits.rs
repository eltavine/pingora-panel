use std::{
    error::Error,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
};

/// Default maximum encoded size of one snapshot record.
pub const DEFAULT_MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;

/// Default maximum aggregate encoded size of prepared snapshot records.
pub const DEFAULT_MAX_PREPARED_RECORD_BYTES: u64 = 256 * 1024 * 1024;

/// Default maximum number of directory entries inspected for prepared records.
pub const DEFAULT_MAX_PREPARED_DIRECTORY_ENTRIES: usize = 4096;

/// A limit set whose ceilings contradict each other.
///
/// The aggregate prepared budget must admit at least one maximum-sized record.
/// Otherwise every prepare that fits the per-record ceiling still overruns the
/// aggregate one, and the store rejects work it advertises as acceptable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SnapshotStoreLimitsError {
    max_record_bytes: u64,
    max_prepared_record_bytes: u64,
}

impl SnapshotStoreLimitsError {
    /// Returns the per-record ceiling that the aggregate budget cannot admit.
    pub const fn max_record_bytes(self) -> u64 {
        self.max_record_bytes
    }

    /// Returns the aggregate prepared budget that is too small.
    pub const fn max_prepared_record_bytes(self) -> u64 {
        self.max_prepared_record_bytes
    }
}

impl fmt::Display for SnapshotStoreLimitsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the {} byte aggregate prepared budget cannot hold one {} byte record",
            self.max_prepared_record_bytes, self.max_record_bytes
        )
    }
}

impl Error for SnapshotStoreLimitsError {}

/// Immutable resource policy shared by snapshot reads and writes.
///
/// Every ceiling is non-zero by type. The ordering invariant between the
/// per-record and aggregate prepared ceilings is checked by [`Self::try_new`]
/// and [`Self::validate`], and by every fallible store constructor. The
/// infallible constructors accept whatever they are handed, which is why they
/// exist alongside fallible counterparts rather than being changed in place.
///
/// Existing constructors use [`Default`]; deployments with a different capacity
/// envelope can inject this value without changing codecs or persistence logic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SnapshotStoreLimits {
    max_record_bytes: NonZeroU64,
    max_prepared_record_bytes: NonZeroU64,
    max_prepared_directory_entries: NonZeroUsize,
}

impl SnapshotStoreLimits {
    /// Creates one snapshot-store resource policy without checking its ordering.
    ///
    /// Prefer [`Self::try_new`]. This constructor cannot fail, so it accepts an
    /// aggregate prepared budget smaller than one maximum-sized record; callers
    /// that keep it must run [`Self::validate`] before composing a store.
    pub const fn new(
        max_record_bytes: NonZeroU64,
        max_prepared_record_bytes: NonZeroU64,
        max_prepared_directory_entries: NonZeroUsize,
    ) -> Self {
        Self {
            max_record_bytes,
            max_prepared_record_bytes,
            max_prepared_directory_entries,
        }
    }

    /// Creates one snapshot-store resource policy, rejecting contradictory ceilings.
    pub const fn try_new(
        max_record_bytes: NonZeroU64,
        max_prepared_record_bytes: NonZeroU64,
        max_prepared_directory_entries: NonZeroUsize,
    ) -> Result<Self, SnapshotStoreLimitsError> {
        let limits = Self::new(
            max_record_bytes,
            max_prepared_record_bytes,
            max_prepared_directory_entries,
        );
        match limits.validate() {
            Ok(()) => Ok(limits),
            Err(error) => Err(error),
        }
    }

    /// Reports whether the aggregate prepared budget admits one whole record.
    pub const fn validate(self) -> Result<(), SnapshotStoreLimitsError> {
        if self.max_prepared_record_bytes.get() < self.max_record_bytes.get() {
            return Err(SnapshotStoreLimitsError {
                max_record_bytes: self.max_record_bytes.get(),
                max_prepared_record_bytes: self.max_prepared_record_bytes.get(),
            });
        }
        Ok(())
    }

    /// Returns the encoded byte ceiling for one active or prepared record.
    pub const fn max_record_bytes(self) -> u64 {
        self.max_record_bytes.get()
    }

    /// Returns the aggregate encoded byte ceiling for prepared records.
    pub const fn max_prepared_record_bytes(self) -> u64 {
        self.max_prepared_record_bytes.get()
    }

    /// Returns the maximum number of prepared-directory entries inspected.
    pub const fn max_prepared_directory_entries(self) -> usize {
        self.max_prepared_directory_entries.get()
    }
}

impl Default for SnapshotStoreLimits {
    fn default() -> Self {
        Self::try_new(
            NonZeroU64::new(DEFAULT_MAX_RECORD_BYTES)
                .expect("the default record byte limit is non-zero"),
            NonZeroU64::new(DEFAULT_MAX_PREPARED_RECORD_BYTES)
                .expect("the default prepared byte limit is non-zero"),
            NonZeroUsize::new(DEFAULT_MAX_PREPARED_DIRECTORY_ENTRIES)
                .expect("the default prepared entry limit is non-zero"),
        )
        .expect("the default aggregate budget holds one maximum-sized record")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("test byte limits are non-zero")
    }

    fn entries(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).expect("test entry limits are non-zero")
    }

    #[test]
    fn defaults_are_non_zero_and_internally_ordered() {
        let limits = SnapshotStoreLimits::default();
        assert!(limits.max_record_bytes() > 0);
        assert!(limits.max_prepared_record_bytes() >= limits.max_record_bytes());
        assert!(limits.max_prepared_directory_entries() > 0);
        assert_eq!(limits.validate(), Ok(()));
    }

    #[test]
    fn an_aggregate_budget_equal_to_one_record_is_accepted() {
        let limits = SnapshotStoreLimits::try_new(bytes(4096), bytes(4096), entries(8))
            .expect("an aggregate budget of exactly one record is usable");
        assert_eq!(limits.max_prepared_record_bytes(), 4096);
    }

    #[test]
    fn an_aggregate_budget_below_one_record_is_rejected() {
        let error = SnapshotStoreLimits::try_new(bytes(4096), bytes(4095), entries(8))
            .expect_err("an aggregate budget below one record is unusable");
        assert_eq!(error.max_record_bytes(), 4096);
        assert_eq!(error.max_prepared_record_bytes(), 4095);
        assert_eq!(
            error.to_string(),
            "the 4095 byte aggregate prepared budget cannot hold one 4096 byte record"
        );
    }

    #[test]
    fn the_unchecked_constructor_still_reports_a_contradictory_limit_set() {
        let limits = SnapshotStoreLimits::new(bytes(4096), bytes(1), entries(8));
        assert_eq!(
            limits.validate(),
            Err(SnapshotStoreLimitsError {
                max_record_bytes: 4096,
                max_prepared_record_bytes: 1,
            })
        );
    }
}
