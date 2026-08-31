use crate::state_directory::StateDirectoryHandle;
use std::{ffi::OsStr, fs::File, io};

pub(crate) struct OpenedRecordFile {
    pub(crate) file: File,
    pub(crate) length_hint: u64,
}

#[derive(Debug)]
pub(crate) enum RecordFileOpenError {
    Io(io::Error),
    NotRegular,
}

/// Opens a record and validates the object referenced by the resulting handle.
///
/// On Unix, `O_NOFOLLOW` closes the check/open race for the final path component,
/// while `O_NONBLOCK` prevents a swapped FIFO or device from stalling the worker
/// before descriptor metadata can be validated.
pub(crate) fn open_regular_record(
    directory: &StateDirectoryHandle,
    name: &OsStr,
) -> Result<Option<OpenedRecordFile>, RecordFileOpenError> {
    let file = match directory.open_readonly_record(name) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(None),
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(RecordFileOpenError::NotRegular);
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Err(RecordFileOpenError::NotRegular);
        }
        Err(error) => return Err(RecordFileOpenError::Io(error)),
    };
    let metadata = file.metadata().map_err(RecordFileOpenError::Io)?;
    if !metadata.is_file() {
        return Err(RecordFileOpenError::NotRegular);
    }

    Ok(Some(OpenedRecordFile {
        file,
        length_hint: metadata.len(),
    }))
}
