use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::ffi::CStr;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::fd::IntoRawFd;
#[cfg(unix)]
use std::{
    ffi::CString,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
};

#[derive(Debug)]
pub(crate) enum StateDirectoryOpenError {
    Io(io::Error),
    NotDirectory,
}

/// Capability-style handle anchoring all record operations to one directory.
///
/// Logical paths are retained only for diagnostics. On Unix, child lookup,
/// creation, deletion, and rename all use the held descriptor and `*at` APIs,
/// so renaming or replacing the configured path cannot redirect an operation.
#[derive(Debug)]
pub(crate) struct StateDirectoryHandle {
    logical_path: PathBuf,
    #[cfg(unix)]
    file: File,
}

impl StateDirectoryHandle {
    pub(crate) fn open_root(
        path: &Path,
        create: bool,
    ) -> Result<Option<Self>, StateDirectoryOpenError> {
        if create {
            fs::create_dir_all(path).map_err(StateDirectoryOpenError::Io)?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut options = OpenOptions::new();
            options.read(true).custom_flags(
                libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            );
            let file = match options.open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound && !create => {
                    return Ok(None);
                }
                Err(error) => return Err(classify_directory_open_error(error)),
            };
            if !file
                .metadata()
                .map_err(StateDirectoryOpenError::Io)?
                .is_dir()
            {
                return Err(StateDirectoryOpenError::NotDirectory);
            }
            file.set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(StateDirectoryOpenError::Io)?;
            Ok(Some(Self {
                logical_path: path.to_path_buf(),
                file,
            }))
        }

        #[cfg(not(unix))]
        {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound && !create => {
                    return Ok(None);
                }
                Err(error) => return Err(StateDirectoryOpenError::Io(error)),
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StateDirectoryOpenError::NotDirectory);
            }
            Ok(Some(Self {
                logical_path: path.to_path_buf(),
            }))
        }
    }

    pub(crate) fn open_child_directory(
        &self,
        name: &OsStr,
        create: bool,
    ) -> Result<Option<Self>, StateDirectoryOpenError> {
        validate_name(name).map_err(StateDirectoryOpenError::Io)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if create {
                let name = c_name(name).map_err(StateDirectoryOpenError::Io)?;
                // SAFETY: both the directory descriptor and NUL-terminated name
                // remain valid for the duration of this call.
                let result = unsafe {
                    libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o700 as libc::mode_t)
                };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(StateDirectoryOpenError::Io(error));
                    }
                }
            }

            let file = match self.open_at(
                name,
                libc::O_RDONLY
                    | libc::O_CLOEXEC
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK,
                0,
            ) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound && !create => {
                    return Ok(None);
                }
                Err(error) => return Err(classify_directory_open_error(error)),
            };
            if !file
                .metadata()
                .map_err(StateDirectoryOpenError::Io)?
                .is_dir()
            {
                return Err(StateDirectoryOpenError::NotDirectory);
            }
            file.set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(StateDirectoryOpenError::Io)?;
            Ok(Some(Self {
                logical_path: self.logical_path.join(name),
                file,
            }))
        }

        #[cfg(not(unix))]
        Self::open_root(&self.logical_path.join(name), create)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.logical_path
    }

    pub(crate) fn path_for(&self, name: &OsStr) -> PathBuf {
        self.logical_path.join(name)
    }

    pub(crate) fn read_entry_names(&self, max_entries: usize) -> io::Result<Vec<OsString>> {
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            self.read_entry_names_from_descriptor(max_entries)
        }
        #[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
        {
            let anchored_path = PathBuf::from(format!("/dev/fd/{}", self.file.as_raw_fd()));
            read_entry_names_from_path(&anchored_path, max_entries)
        }
        #[cfg(not(unix))]
        {
            read_entry_names_from_path(&self.logical_path, max_entries)
        }
    }

    pub(crate) fn open_readonly_record(&self, name: &OsStr) -> io::Result<Option<File>> {
        validate_name(name)?;
        #[cfg(unix)]
        {
            match self.open_at(
                name,
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0,
            ) {
                Ok(file) => Ok(Some(file)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
        #[cfg(not(unix))]
        {
            let path = self.path_for(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record is not a regular file",
                ));
            }
            OpenOptions::new().read(true).open(path).map(Some)
        }
    }

    pub(crate) fn create_new_file(&self, name: &OsStr) -> io::Result<File> {
        validate_name(name)?;
        #[cfg(unix)]
        {
            self.open_at(
                name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        }
        #[cfg(not(unix))]
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path_for(name))
    }

    #[cfg(unix)]
    pub(crate) fn open_lock_file(&self, name: &OsStr) -> io::Result<File> {
        validate_name(name)?;
        self.open_at(
            name,
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    }

    pub(crate) fn rename_file(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        validate_name(source)?;
        validate_name(destination)?;
        #[cfg(unix)]
        {
            let source = c_name(source)?;
            let destination = c_name(destination)?;
            // SAFETY: both descriptors are owned by `self` and both C strings
            // remain alive through the syscall.
            let result = unsafe {
                libc::renameat(
                    self.file.as_raw_fd(),
                    source.as_ptr(),
                    self.file.as_raw_fd(),
                    destination.as_ptr(),
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(unix))]
        fs::rename(self.path_for(source), self.path_for(destination))
    }

    pub(crate) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        validate_name(name)?;
        #[cfg(unix)]
        {
            let name = c_name(name)?;
            // SAFETY: the descriptor and C string are valid for this syscall.
            let result = unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(unix))]
        fs::remove_file(self.path_for(name))
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.file.sync_all()
        }
        #[cfg(not(unix))]
        {
            File::open(&self.logical_path)?.sync_all()
        }
    }

    #[cfg(unix)]
    fn open_at(&self, name: &OsStr, flags: libc::c_int, mode: libc::mode_t) -> io::Result<File> {
        let name = c_name(name)?;
        // SAFETY: `self.file` owns a live descriptor and `name` is a valid,
        // NUL-terminated path component. A successful descriptor is transferred
        // exactly once into `File`.
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags,
                mode as libc::c_uint,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `descriptor` was just returned by `openat` and is uniquely owned.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn read_entry_names_from_descriptor(&self, max_entries: usize) -> io::Result<Vec<OsString>> {
        let descriptor = self
            .open_at(
                OsStr::new("."),
                libc::O_RDONLY
                    | libc::O_CLOEXEC
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK,
                0,
            )?
            .into_raw_fd();
        // SAFETY: ownership of the newly opened descriptor transfers to the
        // directory stream on success. No other File retains this descriptor.
        let stream = unsafe { libc::fdopendir(descriptor) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir did not take ownership when it returned null.
            let _ = unsafe { libc::close(descriptor) };
            return Err(error);
        }
        let stream = OwnedDirectoryStream(stream);
        let mut names = Vec::with_capacity(max_entries.min(64));
        while names.len() < max_entries {
            clear_errno();
            // SAFETY: the stream remains owned and open for this call. readdir's
            // returned entry is consumed before the next call mutates its buffer.
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                let error = current_errno();
                return if error == 0 {
                    Ok(names)
                } else {
                    Err(io::Error::from_raw_os_error(error))
                };
            }
            // SAFETY: POSIX requires d_name to be NUL-terminated for a successful
            // readdir call and the entry remains valid until the next call.
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            names.push(OsStr::from_bytes(bytes).to_os_string());
        }
        Ok(names)
    }
}

#[cfg(any(
    not(unix),
    all(unix, not(target_os = "linux"), not(target_vendor = "apple"))
))]
fn read_entry_names_from_path(path: &Path, max_entries: usize) -> io::Result<Vec<OsString>> {
    fs::read_dir(path)?
        .take(max_entries)
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct OwnedDirectoryStream(*mut libc::DIR);

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl Drop for OwnedDirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns the stream returned by fdopendir.
        let _ = unsafe { libc::closedir(self.0) };
    }
}

#[cfg(target_os = "linux")]
fn clear_errno() {
    // SAFETY: errno is thread-local and the pointer is valid for this thread.
    unsafe { *libc::__errno_location() = 0 };
}

#[cfg(target_vendor = "apple")]
fn clear_errno() {
    // SAFETY: errno is thread-local and the pointer is valid for this thread.
    unsafe { *libc::__error() = 0 };
}

#[cfg(target_os = "linux")]
fn current_errno() -> libc::c_int {
    // SAFETY: errno is thread-local and the pointer is valid for this thread.
    unsafe { *libc::__errno_location() }
}

#[cfg(target_vendor = "apple")]
fn current_errno() -> libc::c_int {
    // SAFETY: errno is thread-local and the pointer is valid for this thread.
    unsafe { *libc::__error() }
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    let mut components = path.components();
    if matches!(components.next(), Some(Component::Normal(component)) if component == name)
        && components.next().is_none()
    {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "state directory operation requires one path component",
    ))
}

#[cfg(unix)]
fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "state directory entry contains a NUL byte",
        )
    })
}

#[cfg(unix)]
fn classify_directory_open_error(error: io::Error) -> StateDirectoryOpenError {
    if matches!(
        error.raw_os_error(),
        Some(libc::ELOOP) | Some(libc::ENOTDIR)
    ) {
        StateDirectoryOpenError::NotDirectory
    } else {
        StateDirectoryOpenError::Io(error)
    }
}
