//! Windows Job Object for ensuring child processes are killed when the parent exits.
//!
//! Creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assigns
//! the current process to it. Any child processes spawned after this are
//! automatically part of the job, and Windows kills them all when kronk exits.

use anyhow::Result;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Create a Job Object and assign the current process to it.
/// All child processes will be killed when this process exits.
/// Returns the handle (must be kept alive for the duration of the process).
pub fn setup_kill_on_exit() -> Result<JobObjectGuard> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() || job == INVALID_HANDLE_VALUE {
            anyhow::bail!("Failed to create Job Object");
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let result = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if result == 0 {
            CloseHandle(job);
            anyhow::bail!("Failed to set Job Object information");
        }

        let result = AssignProcessToJobObject(job, GetCurrentProcess());
        if result == 0 {
            CloseHandle(job);
            anyhow::bail!("Failed to assign process to Job Object");
        }

        Ok(JobObjectGuard(job))
    }
}

/// RAII guard that closes the Job Object handle on drop.
/// Dropping this will cause Windows to kill all child processes.
pub struct JobObjectGuard(HANDLE);

impl Drop for JobObjectGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// SAFETY: The handle is not accessed from multiple threads concurrently.
unsafe impl Send for JobObjectGuard {}
unsafe impl Sync for JobObjectGuard {}
