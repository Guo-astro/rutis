//! Windows: a Job Object around each process the harness starts, so
//! everything it starts can be listed and ended with it (#232).
//!
//! A host goes into its job before it runs: it is created suspended
//! (`CREATE_SUSPENDED`), assigned, then its one thread is resumed. std's
//! `Command` does not give the thread's handle, so the thread is found with
//! a Toolhelp snapshot of the process's threads (a process created
//! suspended has exactly one, and its id cannot be reused while the
//! harness holds the process's handle). That keeps std's pipes and
//! environment handling; calling `CreateProcessW` directly would mean
//! redoing them, and `NtResumeProcess` is not a documented API.
//!
//! rutis-host puts the runtimes it starts in a job of its own; jobs nest
//! (Windows 8 and later), so they are in this one too, and listed with it.

use std::io;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

/// A job that ends its processes when its last handle closes: when the
/// scenario that holds it ends, or this test process does, however it does.
pub(crate) struct Job(HANDLE);

// SAFETY: a job handle may be used from any thread.
unsafe impl Send for Job {}
// SAFETY: as above; every call on it is a thread-safe Win32 call.
unsafe impl Sync for Job {}

impl Job {
    pub(crate) fn new() -> io::Result<Job> {
        // SAFETY: plain Win32 calls; the handle is owned by the `Job`.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            match set {
                0 => Err(io::Error::last_os_error()),
                _ => Ok(job),
            }
        }
    }

    /// Put the process (a live handle) in the job.
    pub(crate) fn assign(&self, process: RawHandle) -> io::Result<()> {
        // SAFETY: both handles are live.
        match unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }

    /// End every process in the job.
    pub(crate) fn terminate(&self) {
        // SAFETY: the handle is live.
        unsafe { TerminateJobObject(self.0, 1) };
    }

    /// The ids of the processes in the job that have not ended.
    pub(crate) fn processes(&self) -> io::Result<Vec<u32>> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two u32 counts, then the ids as
        // usize, from offset 8 on every target.
        let header = 8 / std::mem::size_of::<usize>();
        let mut capacity = 64;
        loop {
            let mut buffer = vec![0usize; header + capacity];
            // SAFETY: the buffer is as long as the size given, and aligned
            // for the structure.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * std::mem::size_of::<usize>()) as u32,
                    std::ptr::null_mut(),
                )
            };
            // SAFETY: the buffer holds at least the two counts.
            let (assigned, listed) = unsafe {
                let counts = buffer.as_ptr().cast::<u32>();
                (*counts as usize, *counts.add(1) as usize)
            };
            if ok != 0 && listed >= assigned {
                return Ok(buffer[header..header + listed]
                    .iter()
                    .map(|&pid| pid as u32)
                    .collect());
            }
            if ok == 0 && io::Error::last_os_error().raw_os_error() != Some(234) {
                // Not ERROR_MORE_DATA.
                return Err(io::Error::last_os_error());
            }
            capacity = capacity.max(assigned) * 2;
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this `Job`; closing the last one
        // ends the job's processes (KILL_ON_JOB_CLOSE).
        unsafe { CloseHandle(self.0) };
    }
}

/// Resume the thread of `pid`, a process created suspended.
pub(crate) fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: plain Win32 calls on handles closed here.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut error = None;
        let mut more = Thread32First(snapshot, &mut entry);
        while more != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    error = Some(io::Error::last_os_error());
                } else {
                    match ResumeThread(thread) {
                        u32::MAX => error = Some(io::Error::last_os_error()),
                        _ => resumed += 1,
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        match (resumed, error) {
            (0, Some(error)) => Err(error),
            (0, None) => Err(io::Error::other("the process has no thread")),
            _ => Ok(()),
        }
    }
}

/// The program `pid` runs (its image path; the command line of another
/// process is not readable without reading its memory), or `?`.
pub(crate) fn image(pid: u32) -> String {
    // SAFETY: plain Win32 calls on a handle closed here.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return "?".into();
        }
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(process);
        match ok {
            0 => "?".into(),
            _ => String::from_utf16_lossy(&buffer[..length as usize]),
        }
    }
}
