use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::InitError;

pub(super) fn commit(staging: TempDir, destination: &Path) -> Result<PathBuf, InitError> {
    prepare_permissions(staging.path()).map_err(|source| InitError::CreateDestination {
        path: destination.to_path_buf(),
        source,
    })?;

    match rename_no_replace(staging.path(), destination) {
        Ok(()) => {
            let _ = staging.keep();

            Ok(destination.to_path_buf())
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            Err(InitError::DestinationExists(destination.to_path_buf()))
        }
        Err(source) => Err(InitError::CreateDestination {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn prepare_permissions(staging: &Path) -> io::Result<()> {
    let parent = staging.parent().unwrap_or_else(|| Path::new("."));
    let probe_root = tempfile::Builder::new()
        .prefix(".cargo-upwell-mode-")
        .tempdir_in(parent)?;
    let probe = probe_root.path().join("directory");

    std::fs::create_dir(&probe)?;
    let permissions = std::fs::metadata(&probe)?.permissions();
    std::fs::set_permissions(staging, permissions)
}

#[cfg(not(unix))]
fn prepare_permissions(_staging: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;

    // SAFETY: both C strings are NUL-terminated and remain alive for the syscall.
    #[allow(unsafe_code)]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };

    syscall_result(result)
}

#[cfg(target_os = "macos")]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;

    // SAFETY: both C strings are NUL-terminated and remain alive for the syscall.
    #[allow(unsafe_code)]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };

    syscall_result(result)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn syscall_result(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();

    // SAFETY: both buffers are NUL-terminated and remain alive for the call. No replacement flag
    // is supplied, so Windows fails when the destination already exists.
    #[allow(unsafe_code)]
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };

    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace directory publication is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests;
