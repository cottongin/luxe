//! The Windows calls behind the pipe: who this user is, a descriptor naming them alone, who
//! connected, and every process's parent.
//!
//! No safe API does any of it, so the `unsafe` lives here and nowhere else in the endpoint.

use std::collections::HashMap;
use std::io;
use std::os::windows::io::AsRawHandle;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// A security descriptor whose only entry grants this user everything, freed on drop.
pub(super) struct OwnerOnly(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable once built and only read by `CreateNamedPipeW`.
#[allow(unsafe_code)]
unsafe impl Send for OwnerOnly {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for OwnerOnly {}

impl OwnerOnly {
    /// `D:P(A;;GA;;;<this user>)`: a protected list, so nothing is inherited, with one entry.
    #[allow(unsafe_code)]
    pub(super) fn new() -> io::Result<Self> {
        let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the descriptor it allocates is
        // owned by the returned value and freed in `drop`.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    /// A pipe instance at `address` carrying this descriptor.
    #[allow(unsafe_code)]
    pub(super) fn create(
        &self,
        options: &ServerOptions,
        address: &str,
    ) -> io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES pointing at a live descriptor this
        // value owns, and both outlive the call.
        unsafe {
            options.create_with_security_attributes_raw(
                address,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
        }
    }
}

impl Drop for OwnerOnly {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`, freed once.
        unsafe { LocalFree(self.0) };
    }
}

/// This process's user, as a SID string (`S-1-5-21-…`).
#[allow(unsafe_code)]
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle needs no closing; `token` is closed below on every path.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let user = token_user(token);
    // SAFETY: opened above and not used after this.
    unsafe { CloseHandle(token) };
    let user = user?;
    // SAFETY: the buffer holds a TOKEN_USER written by `GetTokenInformation`, and is u64-aligned.
    let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` points into `user`, alive here; the string it allocates is freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `text` is a NUL-terminated string from the call above, freed once after copying.
    let sid = unsafe {
        let length = (0..).take_while(|&at| *text.add(at) != 0).count();
        let copied = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        copied
    };
    Ok(sid)
}

/// The token's TOKEN_USER, in a buffer aligned for it.
#[allow(unsafe_code)]
fn token_user(token: HANDLE) -> io::Result<Vec<u64>> {
    let mut size = 0u32;
    // SAFETY: a size query, writing only `size`.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `buffer` holds at least `size` bytes.
    let read = unsafe {
        GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size)
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

/// The process on the other end of a connected pipe.
#[allow(unsafe_code)]
pub(super) fn client_process_id(pipe: &NamedPipeServer) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: the handle is the live, connected pipe, borrowed for the call.
    let found = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) };
    (found != 0).then_some(pid)
}

/// Every running process's parent, from one snapshot; empty when Windows will not take one.
#[allow(unsafe_code)]
pub(in crate::local_endpoint) fn process_parents() -> HashMap<u32, u32> {
    let mut parents = HashMap::new();
    // SAFETY: a snapshot of processes, closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return parents;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry` is sized as the API requires and reused for each process.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: opened above and not used after this.
    unsafe { CloseHandle(snapshot) };
    parents
}
