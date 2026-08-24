use std::ffi::c_void;
use std::io;
use std::mem::{align_of, size_of};
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::ptr;

use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use widestring::U16CString;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

use super::LocalSocketStream;
use crate::PlatformError;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) fn current_user_sid() -> Result<String, PlatformError> {
    current_process_user_sid().map_err(|error| {
        PlatformError::new(format!(
            "could not read the current Windows user SID: {error}"
        ))
    })
}

fn current_process_user_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    token_user_sid(token.0)
}

fn current_thread_user_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    token_user_sid(token.0)
}

fn token_user_sid(token: HANDLE) -> io::Result<String> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
    }
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    // `TOKEN_USER` contains a pointer and must not be read from a
    // byte-aligned `Vec<u8>`. Word storage supplies pointer alignment
    // while still reserving the variable-sized SID bytes Windows
    // writes after the structure.
    let words = (size as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0_usize; words];
    debug_assert_eq!(storage.as_ptr().align_offset(align_of::<TOKEN_USER>()), 0);
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            storage.as_mut_ptr().cast::<c_void>(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let token_user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    sid_string(token_user.User.Sid)
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut string_sid = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut string_sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let allocation = LocalAllocation(string_sid.cast());
    let mut length = 0;
    while unsafe { *string_sid.add(length) } != 0 {
        length += 1;
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(string_sid, length) })
        .map_err(io::Error::other);
    drop(allocation);
    sid
}

pub(super) fn authenticate_server(
    stream: &LocalSocketStream,
    expected_sid: &str,
) -> io::Result<()> {
    let LocalSocketStream::NamedPipe(stream) = stream;
    let mut owner = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            stream.as_handle().as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    if descriptor.is_null() || owner.is_null() {
        if !descriptor.is_null() {
            unsafe {
                LocalFree(descriptor.cast());
            }
        }
        return Err(io::Error::other("the named pipe has no owner SID"));
    }
    let descriptor = LocalAllocation(descriptor.cast());
    let owner_sid = sid_string(owner)?;
    drop(descriptor);
    require_sid(&owner_sid, expected_sid, "pipe owner")
}

pub(super) fn authenticate_client(
    stream: &LocalSocketStream,
    expected_sid: &str,
) -> io::Result<()> {
    let LocalSocketStream::NamedPipe(stream) = stream;
    // The guard binds the token lookup to this connected pipe instance;
    // unlike a peer PID, the impersonation token cannot be redirected
    // by process exit and PID reuse between two system calls.
    let _impersonation = stream.inner().impersonate_client()?;
    let client_sid = current_thread_user_sid()?;
    require_sid(&client_sid, expected_sid, "pipe client")
}

pub(super) fn bytes_available(stream: &LocalSocketStream) -> io::Result<usize> {
    let LocalSocketStream::NamedPipe(stream) = stream;
    let mut available = 0;
    if unsafe {
        PeekNamedPipe(
            stream.as_handle().as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut available,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(available as usize)
}

fn require_sid(actual: &str, expected: &str, peer: &str) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the single-instance {peer} belongs to another user"),
        ))
    }
}

pub(super) fn security_descriptor(user_sid: &str) -> io::Result<SecurityDescriptor> {
    let sddl = U16CString::from_str(format!("O:{user_sid}D:P(A;;GA;;;{user_sid})"))
        .map_err(io::Error::other)?;
    SecurityDescriptor::deserialize(&sddl)
}
