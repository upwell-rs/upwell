#![cfg(unix)]

use std::path::{Path, PathBuf};

use tokio::net::{
    UnixListener,
    unix::{OwnedReadHalf, OwnedWriteHalf},
};
use tracing::{debug, instrument, trace};

use crate::{
    error::Result,
    frame::PeerInfo,
    transport::Transport,
    transports::stream::{StreamConfig, StreamConnection, StreamResponder},
};

/// Unix socket transport. Removes the socket file on drop.
pub struct UnixTransport {
    listener: UnixListener,
    path: PathBuf,
    config: StreamConfig,
}

/// An accepted Unix socket connection.
pub type UnixConnection = StreamConnection<OwnedReadHalf, OwnedWriteHalf>;

/// Responds to one inbound call on a Unix socket connection.
pub type UnixResponder = StreamResponder<OwnedWriteHalf>;

impl UnixTransport {
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        Self::bind_with_config(path, StreamConfig::default())
    }

    pub fn bind_with_config(path: impl Into<PathBuf>, config: StreamConfig) -> Result<Self> {
        let path = path.into();
        Self::ensure_path(&path)?;

        let listener = UnixListener::bind(&path)?;

        if let Err(error) = set_private_permissions(&path, 0o600) {
            let _ = std::fs::remove_file(&path);

            return Err(error.into());
        }

        debug!(path = %path.display(), "Unix transport bound");

        Ok(Self {
            listener,
            path,
            config,
        })
    }

    fn ensure_path(path: &Path) -> Result<()> {
        if !path.exists() {
            Self::create_dirs(path)?
        }

        Ok(())
    }

    fn create_dirs(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            ensure_private_parent(parent)?;
        }

        Ok(())
    }
}

fn ensure_private_parent(path: &Path) -> std::io::Result<()> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    const UNSAFE_WRITE: u32 = 0o022;
    const STICKY: u32 = 0o1000;

    // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
    #[allow(unsafe_code)]
    let effective_uid = unsafe { libc::geteuid() };
    let mut current = PathBuf::new();

    for component in path.components() {
        current.push(component);

        let (metadata, created) = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => (metadata, false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match DirBuilder::new()
                    .recursive(false)
                    .mode(0o700)
                    .create(&current)
                {
                    Ok(()) => (std::fs::symlink_metadata(&current)?, true),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        (std::fs::symlink_metadata(&current)?, false)
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let target = current == path;

        if metadata.file_type().is_symlink() && (target || metadata.uid() != 0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing symlinked Unix socket directory: {}",
                    current.display()
                ),
            ));
        }

        let followed = std::fs::metadata(&current)?;

        if !followed.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("Unix socket path is not a directory: {}", current.display()),
            ));
        }

        if target && followed.uid() != effective_uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Unix socket directory is not owned by this user: {}",
                    current.display()
                ),
            ));
        }

        if target && !created && followed.mode() & UNSAFE_WRITE != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Unix socket directory is writable by its group or other users: {}",
                    current.display()
                ),
            ));
        }

        if !target && followed.mode() & UNSAFE_WRITE != 0 && followed.mode() & STICKY == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Unix socket directory has an unsafe writable ancestor: {}",
                    current.display()
                ),
            ));
        }
    }

    set_private_permissions(path, 0o700)
}

fn set_private_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, Permissions::from_mode(mode))
}

impl Drop for UnixTransport {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        debug!(path = %self.path.display(), "Unix socket removed");
    }
}

impl Transport for UnixTransport {
    type Connection = UnixConnection;

    #[instrument(level = "debug", skip_all)]
    async fn accept(&mut self) -> Result<UnixConnection> {
        trace!("waiting for Unix connection");

        let (stream, _) = self.listener.accept().await?;

        debug!("Unix connection accepted");

        let (read, write) = stream.into_split();
        let peer = PeerInfo { addr: None };

        Ok(StreamConnection::with_config(
            read,
            write,
            peer,
            self.config,
        ))
    }
}

#[cfg(test)]
mod tests;
