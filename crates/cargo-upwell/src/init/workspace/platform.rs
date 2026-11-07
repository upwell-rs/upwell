use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::path::Path;

use super::super::InitError;

pub(super) enum ExchangeError {
    Conflict,
    Unsupported(String),
    Io(std::io::Error),
}

pub(super) fn ensure_supported(project: &Path, manifest: &Path) -> Result<(), InitError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let _ = (project, manifest);
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(InitError::UnsupportedWorkspacePublication {
            project: project.to_path_buf(),
            manifest: manifest.to_path_buf(),
            reason: String::from("atomic manifest exchange is not implemented for this platform"),
        })
    }
}

pub(super) fn open_manifest(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workspace manifest must not be a symlink",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workspace manifest must be a regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe workspace manifest publication is unavailable on this platform",
        ))
    }
}

pub(super) fn open_published(path: &Path) -> std::io::Result<File> {
    open_manifest(path)
}

pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "directory durability is unavailable on this platform",
        ))
    }
}

pub(super) fn exchange(
    candidate_file: &File,
    manifest: &Path,
    candidate: &Path,
) -> Result<(), ExchangeError> {
    if !path_names_file(candidate_file, candidate).map_err(ExchangeError::Io)? {
        return Err(ExchangeError::Conflict);
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let manifest = std::ffi::CString::new(manifest.as_os_str().as_bytes())
            .map_err(|error| ExchangeError::Io(std::io::Error::other(error)))?;
        let candidate = std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .map_err(|error| ExchangeError::Io(std::io::Error::other(error)))?;
        // SAFETY: both paths are valid nul-terminated strings and the syscall borrows them only.
        #[allow(unsafe_code)]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                manifest.as_ptr(),
                libc::AT_FDCWD,
                candidate.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)
            ) {
                return Err(ExchangeError::Unsupported(error.to_string()));
            }
            return Err(ExchangeError::Io(error));
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let manifest = std::ffi::CString::new(manifest.as_os_str().as_bytes())
            .map_err(|error| ExchangeError::Io(std::io::Error::other(error)))?;
        let candidate_c = std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .map_err(|error| ExchangeError::Io(std::io::Error::other(error)))?;
        // SAFETY: both paths are valid nul-terminated strings and the call borrows them only.
        #[allow(unsafe_code)]
        let result =
            unsafe { libc::renamex_np(manifest.as_ptr(), candidate_c.as_ptr(), libc::RENAME_SWAP) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) {
                return Err(ExchangeError::Unsupported(error.to_string()));
            }
            return Err(ExchangeError::Io(error));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (candidate_file, manifest, candidate);
        Err(ExchangeError::Unsupported(String::from(
            "atomic manifest exchange is not implemented for this platform",
        )))
    }
}

pub(super) fn verify_publication(
    active: &mut File,
    candidate: &File,
    manifest: &Path,
    displaced: &Path,
    original: &[u8],
    expected: &[u8],
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let published = open_published(manifest)?;
        if !same_file(candidate, &published)? {
            return Err(std::io::Error::other(
                "published manifest no longer names the candidate",
            ));
        }
        let mut bytes = Vec::new();
        (&published).read_to_end(&mut bytes)?;
        if bytes != expected {
            return Err(std::io::Error::other(
                "published manifest bytes did not verify",
            ));
        }
        let displaced_file = open_manifest(displaced)?;
        if !same_file(active, &displaced_file)? {
            return Err(std::io::Error::other(
                "displaced recovery no longer names the original manifest",
            ));
        }
        use std::io::Seek as _;
        active.rewind()?;
        let mut displaced_bytes = Vec::new();
        active.read_to_end(&mut displaced_bytes)?;
        if displaced_bytes != original {
            return Err(std::io::Error::other(
                "displaced manifest changed during publication",
            ));
        }
        verify_metadata(active, &published)?;
        if !path_names_file(candidate, manifest)? || !path_names_file(active, displaced)? {
            return Err(std::io::Error::other(
                "publication paths changed during verification",
            ));
        }

        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (active, candidate, manifest, displaced, original, expected);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe workspace manifest publication is unavailable on this platform",
        ))
    }
}

pub(super) fn path_names_file(file: &File, path: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        let current = open_manifest(path)?;

        same_file(file, &current)
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        Ok(false)
    }
}

#[cfg(unix)]
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let left = left.metadata()?;
    let right = right.metadata()?;

    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[allow(unsafe_code)]
pub(super) fn copy_metadata(source: &File, destination: &File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd as _;
        // SAFETY: both descriptors remain valid for the duration of the metadata-only copy.
        if unsafe {
            libc::fcopyfile(
                source.as_raw_fd(),
                destination.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_METADATA,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        use xattr::FileExt as _;
        let metadata = source.metadata()?;
        // SAFETY: destination owns a live fd; uid/gid originate from source metadata.
        if unsafe { libc::fchown(destination.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EPERM) {
                return Err(error);
            }
            let actual = destination.metadata()?;
            if actual.uid() != metadata.uid() || actual.gid() != metadata.gid() {
                return Err(error);
            }
        }
        destination.set_permissions(std::fs::Permissions::from_mode(metadata.mode()))?;
        for name in source.list_xattr()? {
            let value = source.get_xattr(&name)?.ok_or_else(|| {
                std::io::Error::other("extended attribute disappeared during metadata copy")
            })?;
            destination.set_xattr(&name, &value)?;
        }
    }
    #[cfg(not(unix))]
    let _ = (source, destination);
    Ok(())
}

pub(super) fn protect_recovery(source: &File, recovery: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = source;
        recovery.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = (source, recovery);
        Ok(())
    }
}

pub(super) fn verify_metadata(source: &File, destination: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        use std::os::darwin::fs::MetadataExt as _;
        use std::os::unix::fs::MetadataExt as _;
        let source_metadata = source.metadata()?;
        let destination_metadata = destination.metadata()?;
        let basic_mismatch = source_metadata.mode() != destination_metadata.mode()
            || source_metadata.uid() != destination_metadata.uid()
            || source_metadata.gid() != destination_metadata.gid();
        #[cfg(target_os = "macos")]
        let basic_mismatch =
            basic_mismatch || source_metadata.st_flags() != destination_metadata.st_flags();

        if basic_mismatch {
            return Err(std::io::Error::other(
                "manifest ownership or mode was not preserved",
            ));
        }
        let source_xattrs = xattrs(source)?;
        let destination_xattrs = xattrs(destination)?;
        if source_xattrs != destination_xattrs {
            return Err(std::io::Error::other(
                "manifest extended attributes were not preserved",
            ));
        }
        #[cfg(target_os = "macos")]
        if acl(source)? != acl(destination)? {
            return Err(std::io::Error::other(
                "manifest access control list was not preserved",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = (source, destination);
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn acl(file: &File) -> std::io::Result<Vec<u8>> {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::os::fd::AsRawFd as _;

    unsafe extern "C" {
        fn acl_get_fd(fd: c_int) -> *mut c_void;
        fn acl_to_text(acl: *mut c_void, len: *mut isize) -> *mut c_char;
        fn acl_free(object: *mut c_void) -> c_int;
    }

    // SAFETY: the descriptor remains live and each returned allocation is freed exactly once.
    unsafe {
        let value = acl_get_fd(file.as_raw_fd());
        if value.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(Vec::new());
            }

            return Err(error);
        }

        let text = acl_to_text(value, std::ptr::null_mut());
        if text.is_null() {
            let error = std::io::Error::last_os_error();
            let _ = acl_free(value);
            return Err(error);
        }

        let bytes = CStr::from_ptr(text).to_bytes().to_vec();
        let _ = acl_free(text.cast());
        let _ = acl_free(value);

        Ok(bytes)
    }
}

#[cfg(unix)]
fn xattrs(file: &File) -> std::io::Result<std::collections::BTreeMap<std::ffi::OsString, Vec<u8>>> {
    use xattr::FileExt as _;
    let mut attributes = std::collections::BTreeMap::new();
    for name in file.list_xattr()? {
        let value = file.get_xattr(&name)?.ok_or_else(|| {
            std::io::Error::other("extended attribute disappeared during verification")
        })?;
        attributes.insert(name, value);
    }
    Ok(attributes)
}
