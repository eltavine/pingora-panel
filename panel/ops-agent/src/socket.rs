use panel_errors::{PanelError, Result};
use std::{
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::Path,
};
use tokio::net::UnixListener;

/// Read and write for the agent and its socket's group, nothing for others.
const SOCKET_MODE: u32 = 0o660;

/// Binds the agent's socket, replacing one a previous run left behind, and
/// gives it to `group`.
pub(crate) fn bind(path: &Path, group: Option<u32>) -> Result<UnixListener> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(path).map_err(|error| unusable(path, &error))?;
        }
        Ok(_) => {
            return Err(PanelError::precondition_failed(format!(
                "{} exists and is not a socket",
                path.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(unusable(path, &error)),
    }
    let listener = UnixListener::bind(path).map_err(|error| unusable(path, &error))?;
    if let Some(group) = group {
        std::os::unix::fs::chown(path, None, Some(group)).map_err(|error| {
            PanelError::precondition_failed(format!(
                "cannot give {} to group {group}; the agent must be a member: {error}",
                path.display()
            ))
        })?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE))
        .map_err(|error| unusable(path, &error))?;
    Ok(listener)
}

fn unusable(path: &Path, error: &io::Error) -> PanelError {
    PanelError::precondition_failed(format!("cannot serve on {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[tokio::test]
    async fn sockets_are_private_to_their_group_and_replace_stale_ones() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.sock");
        let group = std::fs::metadata(directory.path()).unwrap().gid();

        drop(bind(&path, Some(group)).unwrap());
        let listener = bind(&path, Some(group)).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.mode() & 0o777, SOCKET_MODE);
        assert_eq!(metadata.gid(), group);
        drop(listener);

        let file = directory.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(
            bind(&file, None).is_err(),
            "a regular file is never replaced"
        );
    }
}
