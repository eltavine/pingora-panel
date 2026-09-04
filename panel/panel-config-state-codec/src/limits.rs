use core::fmt;
use std::num::NonZeroUsize;

/// Default maximum encoded size of one revision codec document.
pub const DEFAULT_MAX_REVISION_DOCUMENT_BYTES: usize = 64 * 1024;

/// Immutable resource policy for revision-state and transition documents.
///
/// The policy is representation-neutral and can be injected at API, storage,
/// or messaging boundaries. Existing decode functions use [`Default`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RevisionCodecLimits {
    max_document_bytes: NonZeroUsize,
}

impl RevisionCodecLimits {
    /// Creates a validated document resource policy.
    pub const fn new(max_document_bytes: NonZeroUsize) -> Self {
        Self { max_document_bytes }
    }

    /// Returns the maximum accepted encoded document size.
    pub const fn max_document_bytes(self) -> usize {
        self.max_document_bytes.get()
    }
}

impl Default for RevisionCodecLimits {
    fn default() -> Self {
        Self::new(
            NonZeroUsize::new(DEFAULT_MAX_REVISION_DOCUMENT_BYTES)
                .expect("the default revision document limit is non-zero"),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocumentLimitExceeded {
    pub(crate) actual_bytes: usize,
    pub(crate) max_bytes: usize,
}

impl fmt::Display for DocumentLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "revision document uses {} bytes but the limit is {}",
            self.actual_bytes, self.max_bytes
        )
    }
}

pub(crate) fn enforce_document_limit(
    encoded: &[u8],
    limits: RevisionCodecLimits,
) -> Result<(), DocumentLimitExceeded> {
    let max_bytes = limits.max_document_bytes();
    if encoded.len() > max_bytes {
        return Err(DocumentLimitExceeded {
            actual_bytes: encoded.len(),
            max_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_limit_is_accepted_and_excess_is_rejected() {
        let limits = RevisionCodecLimits::new(NonZeroUsize::new(3).unwrap());
        assert!(enforce_document_limit(b"123", limits).is_ok());
        assert_eq!(
            enforce_document_limit(b"1234", limits).unwrap_err(),
            DocumentLimitExceeded {
                actual_bytes: 4,
                max_bytes: 3,
            }
        );
    }
}
