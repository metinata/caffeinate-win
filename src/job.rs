//! Ending the utility when caffeinate is killed.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// A job object that caffeinate and the processes it starts belong to.
///
/// On macOS, caffeinate runs the utility in its own process, so whatever ends
/// caffeinate ends the utility. On Windows the utility is a child process, so
/// the job is set to end all its processes when its last handle is closed. Only
/// caffeinate holds a handle, and the kernel closes it when caffeinate is
/// killed. When caffeinate exits by itself, dropping the job lifts that first:
/// whatever is still running then (a GUI utility after Ctrl+C, or a server the
/// utility started) keeps running.
pub struct Job(HANDLE);

impl Job {
    /// Puts caffeinate in a new job. Processes it starts from now on join it too.
    pub fn enter() -> io::Result<Self> {
        // SAFETY: no security attributes and no name.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        // Processes may still leave the job on purpose (CREATE_BREAKAWAY_FROM_JOB).
        job.set_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK)?;
        // SAFETY: `job.0` is a job handle; GetCurrentProcess returns a pseudo
        // handle with full access to this process.
        if unsafe { AssignProcessToJobObject(job.0, GetCurrentProcess()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn set_limits(&self, flags: JOB_OBJECT_LIMIT) -> io::Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = flags;
        let size = u32::try_from(size_of_val(&info)).expect("size fits in u32");
        // SAFETY: `info` is a JOBOBJECT_EXTENDED_LIMIT_INFORMATION of `size` bytes.
        let ok = unsafe {
            SetInformationJobObject(
                self.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // Closing the last handle while the job still ends its processes would
        // end caffeinate itself, so it stays open (until exit) if that fails.
        if self.set_limits(JOB_OBJECT_LIMIT_BREAKAWAY_OK).is_ok() {
            // SAFETY: `self.0` is a job handle that only `self` owns.
            unsafe { CloseHandle(self.0) };
        }
    }
}
