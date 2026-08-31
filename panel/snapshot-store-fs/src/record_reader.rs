use std::io::{self, Read};

#[derive(Debug)]
pub(crate) enum BoundedRecordReadError {
    Io(io::Error),
    LimitExceeded { max_bytes: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecordCollectionLimitExceeded {
    pub(crate) max_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecordCollectionBudget {
    max_bytes: u64,
    consumed_bytes: u64,
}

impl RecordCollectionBudget {
    pub(crate) const fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            consumed_bytes: 0,
        }
    }

    pub(crate) fn consume(&mut self, bytes: u64) -> Result<(), RecordCollectionLimitExceeded> {
        self.consumed_bytes = self
            .consumed_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.max_bytes)
            .ok_or(RecordCollectionLimitExceeded {
                max_bytes: self.max_bytes,
            })?;
        Ok(())
    }
}

/// Reads the actual byte stream through a hard cap.
///
/// Filesystem metadata is only a capacity hint: it is never trusted as the
/// enforcement boundary because a concurrently modified file may grow after
/// its metadata is inspected.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BoundedRecordReader {
    max_bytes: u64,
}

impl BoundedRecordReader {
    pub(crate) const fn new(max_bytes: u64) -> Self {
        Self { max_bytes }
    }

    pub(crate) fn read(
        self,
        reader: impl Read,
        capacity_hint: u64,
    ) -> Result<Vec<u8>, BoundedRecordReadError> {
        let capacity = usize::try_from(capacity_hint.min(self.max_bytes)).unwrap_or(usize::MAX);
        let mut bytes = Vec::with_capacity(capacity);
        let mut limited = reader.take(self.max_bytes.saturating_add(1));
        limited
            .read_to_end(&mut bytes)
            .map_err(BoundedRecordReadError::Io)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > self.max_bytes {
            return Err(BoundedRecordReadError::LimitExceeded {
                max_bytes: self.max_bytes,
            });
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn actual_stream_size_is_enforced_independently_of_metadata_hint() {
        let reader = BoundedRecordReader::new(4);
        let error = reader.read(Cursor::new(b"12345"), 1).unwrap_err();
        assert!(matches!(
            error,
            BoundedRecordReadError::LimitExceeded { max_bytes: 4 }
        ));
    }

    #[test]
    fn exact_limit_and_short_streams_are_accepted() {
        let reader = BoundedRecordReader::new(4);
        assert_eq!(reader.read(Cursor::new(b"1234"), 99).unwrap(), b"1234");
        assert_eq!(reader.read(Cursor::new(b"12"), 0).unwrap(), b"12");
    }

    #[test]
    fn collection_budget_tracks_actual_bytes_and_rejects_overflow() {
        let mut budget = RecordCollectionBudget::new(5);
        budget.consume(2).unwrap();
        budget.consume(3).unwrap();
        assert_eq!(
            budget.consume(1),
            Err(RecordCollectionLimitExceeded { max_bytes: 5 })
        );

        let mut overflow = RecordCollectionBudget::new(u64::MAX);
        overflow.consume(u64::MAX).unwrap();
        assert!(overflow.consume(1).is_err());
    }
}
