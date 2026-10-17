//! What a process holds now, for the soak (S8, #192): its resident memory,
//! its threads, its open file descriptors (handles on Windows). Read from
//! outside the process, as a user's tools would:
//!
//! - Linux: `/proc/<pid>/status` (`VmRSS`, `Threads`) and the entries of
//!   `/proc/<pid>/fd`;
//! - macOS: libproc, `proc_pidinfo` with `PROC_PIDTASKINFO` (resident size,
//!   thread count) and `PROC_PIDLISTFDS` (the descriptor table);
//! - Windows: `GetProcessMemoryInfo` (working set), `GetProcessHandleCount`,
//!   and the thread count of a Toolhelp process snapshot.
//!
//! A process that ended meanwhile, or that cannot be read, is left out.

/// One process's holdings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Resident memory (the working set on Windows), in KiB.
    pub rss_kib: u64,
    pub threads: u64,
    /// Open file descriptors; on Windows, open handles of every kind.
    pub fds: u64,
}

impl std::ops::Add for Usage {
    type Output = Usage;

    fn add(self, other: Usage) -> Usage {
        Usage {
            rss_kib: self.rss_kib + other.rss_kib,
            threads: self.threads + other.threads,
            fds: self.fds + other.fds,
        }
    }
}

/// The holdings of each of `pids` that could be read.
pub fn sample(pids: &[u32]) -> Vec<(u32, Usage)> {
    #[cfg(windows)]
    let threads = windows::threads();
    pids.iter()
        .filter_map(|&pid| {
            #[cfg(windows)]
            let usage = windows::usage(pid, &threads);
            #[cfg(not(windows))]
            let usage = usage(pid);
            usage.map(|usage| (pid, usage))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn usage(pid: u32) -> Option<Usage> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
    };
    // A zombie has no VmRSS: it holds nothing any more.
    let rss_kib = field("VmRSS:")?;
    let threads = field("Threads:")?;
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count() as u64;
    Some(Usage {
        rss_kib,
        threads,
        fds,
    })
}

#[cfg(target_os = "macos")]
fn usage(pid: u32) -> Option<Usage> {
    use std::mem::{size_of, zeroed};
    // SAFETY: proc_pidinfo(3) writes at most the size given into buffers
    // of that size.
    unsafe {
        let mut task: libc::proc_taskinfo = zeroed();
        let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
        let read = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut task as *mut libc::proc_taskinfo).cast(),
            size,
        );
        if read != size {
            return None;
        }
        // With no buffer: the size the table may need now.
        let needed = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDLISTFDS,
            0,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return None;
        }
        let entry = size_of::<libc::proc_fdinfo>();
        // Room for some opened between the two calls.
        let mut table: Vec<libc::proc_fdinfo> = vec![zeroed(); needed as usize / entry + 32];
        let read = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDLISTFDS,
            0,
            table.as_mut_ptr().cast(),
            (table.len() * entry) as libc::c_int,
        );
        if read < 0 {
            return None;
        }
        Some(Usage {
            rss_kib: task.pti_resident_size / 1024,
            threads: task.pti_threadnum as u64,
            fds: read as u64 / entry as u64,
        })
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn usage(_pid: u32) -> Option<Usage> {
    None
}

#[cfg(windows)]
mod windows {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    use super::Usage;

    /// The thread count of every process, from one snapshot.
    pub(super) fn threads() -> HashMap<u32, u64> {
        let mut threads = HashMap::new();
        // SAFETY: plain Win32 calls on a snapshot handle closed here.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return threads;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry);
            while more != 0 {
                threads.insert(entry.th32ProcessID, entry.cntThreads as u64);
                more = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
        }
        threads
    }

    pub(super) fn usage(pid: u32, threads: &HashMap<u32, u64>) -> Option<Usage> {
        let threads = *threads.get(&pid)?;
        // SAFETY: plain Win32 calls on a handle closed here; the counters
        // are as large as the size given.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut memory: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            memory.cb = size;
            let read = GetProcessMemoryInfo(process, &mut memory, size);
            let mut handles = 0u32;
            let counted = GetProcessHandleCount(process, &mut handles);
            CloseHandle(process);
            if read == 0 || counted == 0 {
                return None;
            }
            Some(Usage {
                rss_kib: memory.WorkingSetSize as u64 / 1024,
                threads,
                fds: handles as u64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    /// This test process can be read, and holds what any process holds.
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn this_process_is_read() {
        let file = std::fs::File::open(env!("CARGO_MANIFEST_DIR")).ok();
        let read = super::sample(&[std::process::id()]);
        let [(pid, usage)] = read.as_slice() else {
            panic!("not read: {read:?}");
        };
        assert_eq!(*pid, std::process::id());
        assert!(
            usage.rss_kib > 0 && usage.threads >= 1 && usage.fds >= 1,
            "{usage:?}"
        );
        drop(file);
    }

    /// A process that does not exist is left out.
    #[test]
    fn a_process_that_does_not_exist_is_left_out() {
        assert!(super::sample(&[u32::MAX - 1]).is_empty());
    }
}
