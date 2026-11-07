use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
#[cfg(test)]
use windows_sys::Win32::Security::{PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, GetFileInformationByHandleEx,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::PWSTR;

const SECURITY_INFORMATION: u32 = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;

struct LocalDescriptor(PSECURITY_DESCRIPTOR);

impl LocalDescriptor {
    fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

impl Drop for LocalDescriptor {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: this wrapper is constructed only from LocalAlloc-returning APIs.
        unsafe { LocalFree(self.0) };
    }
}

struct OwnedDirectory(HANDLE);

// SAFETY: a Windows directory handle is a kernel object reference that may be used and closed
// from any thread, and `OwnedDirectory` has no other state.
#[allow(unsafe_code)]
unsafe impl Send for OwnedDirectory {}

impl Drop for OwnedDirectory {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: this wrapper is constructed only from successful CreateFileW calls.
        unsafe { CloseHandle(self.0) };
    }
}

/// Keeps every checked path component open without FILE_SHARE_DELETE. Stored by the
/// `Dir` handle after `ensure`, so an attacker cannot replace an ancestor afterwards.
pub(super) struct PrivateDirectoryGuard {
    _components: Vec<OwnedDirectory>,
}

pub(super) fn ensure_private_directory(path: &Path) -> io::Result<PrivateDirectoryGuard> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private application directory must not contain '..'",
        ));
    }

    let user_sid = current_user_sid_string()?;
    let private_sddl = private_sddl(&user_sid);
    let private_descriptor = descriptor_from_sddl(&private_sddl)?;
    let security_attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: private_descriptor.as_ptr(),
        bInheritHandle: 0,
    };
    let mut current = PathBuf::new();
    // Omitting FILE_SHARE_DELETE while these handles are alive prevents a checked
    // component from being renamed or replaced during the remainder of the walk.
    let mut held_components = Vec::new();

    for component in path.components() {
        match component {
            // A drive/UNC prefix alone is syntax, not a directory (`C:` means a
            // per-drive working directory). Open its rooted form on RootDir below.
            Component::Prefix(_) => {
                current.push(component.as_os_str());
                continue;
            }
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => unreachable!("parent components rejected before walking"),
            Component::Normal(_) => current.push(component.as_os_str()),
        }

        let handle = match open_directory(&current) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&current, &security_attributes)?;
                open_directory(&current)?
            }
            Err(error) => return Err(error),
        };
        held_components.push(handle);
    }

    verify_private_acl(path, &private_sddl)?;

    Ok(PrivateDirectoryGuard {
        _components: held_components,
    })
}

fn permission_denied(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn private_sddl(user_sid: &str) -> String {
    format!("O:{user_sid}D:P(A;OICI;FA;;;{user_sid})(A;OICI;FA;;;SY)")
}

#[allow(unsafe_code)]
fn create_private_directory(
    path: &Path,
    security_attributes: &SECURITY_ATTRIBUTES,
) -> io::Result<()> {
    let encoded_path = wide_null(path.as_os_str());
    // SAFETY: path is NUL-terminated and security_attributes points to a descriptor
    // that remains alive for this call. The ACL is therefore installed atomically with
    // directory creation; there is no inherited-permissions window.
    if unsafe { CreateDirectoryW(encoded_path.as_ptr(), security_attributes) } != 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
        return Ok(());
    }

    Err(error)
}

#[allow(unsafe_code)]
fn open_directory(path: &Path) -> io::Result<OwnedDirectory> {
    let encoded_path = wide_null(path.as_os_str());
    // SAFETY: path is NUL-terminated. The returned owned handle is closed by its
    // wrapper. OPEN_REPARSE_POINT ensures we inspect the component itself, not a target.
    let handle = unsafe {
        CreateFileW(
            encoded_path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let handle = OwnedDirectory(handle);
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: handle is valid and attributes is correctly sized writable storage.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileAttributeTagInfo,
            ptr::addr_of_mut!(attributes).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(permission_denied(format!(
            "refusing reparse point in application directory: {}",
            path.display()
        )));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!(
                "application directory path is not a directory: {}",
                path.display()
            ),
        ));
    }

    Ok(handle)
}

#[allow(unsafe_code)]
fn descriptor_from_sddl(sddl: &str) -> io::Result<LocalDescriptor> {
    let encoded_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();

    // SAFETY: input is NUL-terminated and the output pointer is valid writable storage.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            encoded_sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if descriptor.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an empty security descriptor",
        ));
    }

    Ok(LocalDescriptor(descriptor))
}

#[cfg(test)]
fn apply_dacl(path: &Path, sddl: &str) -> io::Result<()> {
    apply_security(
        path,
        sddl,
        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
    )
}

#[cfg(test)]
#[allow(unsafe_code)]
fn apply_security(path: &Path, sddl: &str, security_information: u32) -> io::Result<()> {
    let descriptor = descriptor_from_sddl(sddl)?;
    let encoded_path = wide_null(path.as_os_str());

    // SAFETY: both arguments remain valid for the duration of this call.
    let result = unsafe {
        SetFileSecurityW(
            encoded_path.as_ptr(),
            security_information,
            descriptor.as_ptr(),
        )
    };

    if result == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[allow(unsafe_code)]
fn verify_private_acl(path: &Path, expected_sddl: &str) -> io::Result<()> {
    let encoded_path = wide_null(path.as_os_str());
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();

    // SAFETY: the path is NUL-terminated and the output pointer is valid. We request
    // one self-contained descriptor instead of borrowing owner/ACL interior pointers.
    let status = unsafe {
        GetNamedSecurityInfoW(
            encoded_path.as_ptr(),
            SE_FILE_OBJECT,
            SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };

    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }

    if descriptor.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an empty security descriptor",
        ));
    }
    let descriptor = LocalDescriptor(descriptor);
    let actual = descriptor_to_sddl(descriptor.as_ptr())?;
    let expected_descriptor = descriptor_from_sddl(expected_sddl)?;
    let expected = descriptor_to_sddl(expected_descriptor.as_ptr())?;

    if actual != expected {
        return Err(permission_denied(format!(
            "application directory owner or private ACL does not match the current user: {}",
            path.display()
        )));
    }

    Ok(())
}

#[allow(unsafe_code)]
fn descriptor_to_sddl(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<String> {
    let mut encoded: PWSTR = ptr::null_mut();
    let mut length = 0u32;
    // SAFETY: descriptor is owned by a live LocalDescriptor and both outputs point to
    // writable storage. Windows returns a LocalAlloc UTF-16 string and its length.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            SECURITY_INFORMATION,
            &mut encoded,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if encoded.is_null() || length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an empty SDDL string",
        ));
    }

    // SAFETY: the call above succeeded, so `encoded` points to `length` initialized UTF-16 units
    // (including the trailing NUL) that stay allocated until the `LocalFree` below. The reported
    // length includes the trailing NUL for this API.
    let units = unsafe { std::slice::from_raw_parts(encoded, length as usize) };
    let units = units.strip_suffix(&[0]).unwrap_or(units);
    let result = String::from_utf16(units)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Windows returned invalid SDDL"));
    // SAFETY: encoded is the LocalAlloc allocation returned above.
    unsafe { LocalFree(encoded.cast()) };

    result
}

#[allow(unsafe_code)]
fn current_user_sid_string() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: token points to writable storage; the pseudo process handle is valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let result = token_user_sid_string(token);

    // SAFETY: token is an owned handle returned by OpenProcessToken.
    unsafe { CloseHandle(token) };

    result
}

#[allow(unsafe_code)]
fn token_user_sid_string(token: HANDLE) -> io::Result<String> {
    let mut required = 0u32;
    // SAFETY: a null buffer of length zero is the documented size query, `required` is writable
    // storage, and the call is expected to fail with ERROR_INSUFFICIENT_BUFFER.
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
    }
    if required == 0 {
        return Err(io::Error::last_os_error());
    }

    if required < size_of::<TOKEN_USER>() as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an undersized token-user buffer length",
        ));
    }

    // TOKEN_USER contains pointer-aligned fields. Allocate in machine words rather
    // than bytes so casting the initialized prefix is correctly aligned on all targets.
    let words = (required as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![MaybeUninit::<usize>::uninit(); words];
    // SAFETY: buffer is pointer-aligned and has at least `required` writable bytes.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: successful TokenUser initialized the aligned TOKEN_USER prefix. The SID
    // pointer refers into `buffer`, which remains alive throughout sid_to_string.
    let user = unsafe { buffer.as_ptr().cast::<TOKEN_USER>().read() };
    sid_to_string(user.User.Sid)
}

#[allow(unsafe_code)]
fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut value: PWSTR = ptr::null_mut();
    // SAFETY: sid is supplied by a successful Windows security API.
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: a successful ConvertSidToStringSidW returns a NUL-terminated string, so the scan stops
    // at the terminator without leaving the allocation.
    let len = unsafe {
        let mut len = 0;
        while *value.add(len) != 0 {
            len += 1;
        }
        len
    };
    // SAFETY: value points to a NUL-terminated allocation of `len` UTF-16 units.
    let string = String::from_utf16(unsafe { std::slice::from_raw_parts(value, len) })
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Windows returned invalid SID"));
    // SAFETY: value is the LocalAlloc allocation returned above.
    unsafe { LocalFree(value.cast()) };

    string
}

#[cfg(test)]
pub(super) fn apply_world_access_for_test(path: &Path) -> io::Result<()> {
    apply_dacl(path, "D:P(A;OICI;FA;;;WD)")
}
