//! Job — a Windows Job Object holding a command and everything it starts, so a timeout or a cancel
//! can end them all.
//!
//! Windows has no process group to signal. `taskkill /T` walks parent links, and a shell killed
//! first, as `kill_on_drop` kills it, leaves its children unlinked and running. A process started
//! inside a job stays in it at any depth, so ending the job ends the tree whatever died first.

use std::os::windows::io::RawHandle;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
};

/// A job holding one process and what it starts. Dropping it closes the handle and leaves them
/// running, as a command that finished leaves a background process it started; [`Job::terminate`]
/// ends them.
pub(crate) struct Job(HANDLE);

// SAFETY: a job handle names a kernel object, and every call on it is thread-safe.
#[allow(unsafe_code)]
unsafe impl Send for Job {}
// SAFETY: as above; `terminate` and `drop` take no state but the handle.
#[allow(unsafe_code)]
unsafe impl Sync for Job {}

impl Job {
    /// A new job holding `process`, or `None` when Windows refuses one, as it can for a process
    /// already in a job that forbids nesting.
    #[allow(unsafe_code)]
    pub(crate) fn holding(process: RawHandle) -> Option<Self> {
        // SAFETY: `process` is a live process handle lent for this call; the job is created
        // unnamed with default security, so nothing else can open it, and is closed here if the
        // process cannot join it.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            if AssignProcessToJobObject(job, process as HANDLE) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Self(job))
        }
    }

    /// Ends every process in the job.
    #[allow(unsafe_code)]
    pub(crate) fn terminate(&self) {
        // SAFETY: `self.0` is a live job handle this value owns until it drops.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: closed once, here, by its only owner.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
