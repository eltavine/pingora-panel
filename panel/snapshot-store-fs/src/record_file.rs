use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

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
    path: &Path,
) -> Result<Option<OpenedRecordFile>, RecordFileOpenError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }

    #[cfg(not(unix))]
    {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RecordFileOpenError::Io(error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RecordFileOpenError::NotRegular);
        }
    }

    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
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
