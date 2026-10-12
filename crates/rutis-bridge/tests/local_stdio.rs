//! Where a runtime process's standard streams go: its input is none unless
//! its launcher gives it this process's (for a plugin that reads the
//! terminal), and its output and error are this process's unless discarded.
//! Both ways a runtime is started: `LocalRuntime` and the `Process` facade
//! (`Mount::launcher`).
//!
//! Its own test binary: while a probe runs, the test points this process's
//! standard input at a pipe holding a line, and its output and error at a
//! file. The probe copies what it reads, writes a line to each stream, and
//! exits (so the start fails: it never greets).
// Unix only (Q7.8): the test points this process's standard streams at a
// pipe and a file with dup2 on raw descriptors.
#![cfg(unix)]
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::Path;
use std::time::Duration;

use rutis::Ctx;
use rutis_bridge::runtime::{Launcher, LocalRuntime, Mount, Process, Stdio};

/// This process's `fd` replaced by `with` until dropped.
struct Swapped {
    fd: i32,
    saved: i32,
}

impl Swapped {
    fn new(fd: i32, with: i32) -> Self {
        let saved = unsafe { libc::dup(fd) };
        assert!(saved >= 0);
        assert!(unsafe { libc::dup2(with, fd) } >= 0);
        Swapped { fd, saved }
    }
}

impl Drop for Swapped {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, self.fd);
            libc::close(self.saved);
        }
    }
}

/// A pipe holding `line`, its write end closed: as standard input.
fn input(line: &[u8]) -> Swapped {
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let mut writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    writer.write_all(line).unwrap();
    drop(writer);
    let swapped = Swapped::new(std::io::stdin().as_raw_fd(), fds[0]);
    unsafe { libc::close(fds[0]) };
    swapped
}

/// What a probe did: the input it read, and the lines it wrote that reached
/// this process's output.
struct Seen {
    read: String,
    out: bool,
    err: bool,
}

/// Run `start` with this process's streams redirected, the probe writing to
/// `dir/<name>.in`.
async fn probe<F: std::future::Future<Output = ()>>(dir: &Path, name: &str, start: F) -> Seen {
    let captured = dir.join(format!("{name}.captured"));
    let file = std::fs::File::create(&captured).unwrap();
    {
        let _stdin = input(b"hello\n");
        let _stdout = Swapped::new(libc::STDOUT_FILENO, file.as_raw_fd());
        let _stderr = Swapped::new(libc::STDERR_FILENO, file.as_raw_fd());
        tokio::time::timeout(Duration::from_secs(30), start)
            .await
            .expect("the probe ends");
    }
    let captured = std::fs::read_to_string(&captured).unwrap();
    Seen {
        read: std::fs::read_to_string(dir.join(format!("{name}.in"))).unwrap_or_default(),
        out: captured.contains("to-stdout"),
        err: captured.contains("to-stderr"),
    }
}

/// A process that copies its input to `<name>.in`, writes a line to its
/// output and its error, and exits.
fn launcher(dir: &Path, name: &str) -> Launcher {
    Launcher::new("sh")
        .arg("-c")
        .arg(format!(
            "cat > {name}.in; echo to-stdout; echo to-stderr >&2; exit 3"
        ))
        .cwd(dir)
        .inherit_fd()
}

async fn local(runtime: LocalRuntime) {
    let root = Ctx::root().unwrap();
    let view = root.plugin(runtime);
    let _ = (&view).await;
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_process_gets_the_standard_streams_its_launcher_names() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    // By default: no input (the line stays unread), output and error shown.
    let seen = probe(dir, "default", async {
        local(LocalRuntime::launcher(
            "probe",
            launcher(dir, "default"),
            dir,
        ))
        .await
    })
    .await;
    assert_eq!(seen.read, "");
    assert!(seen.out && seen.err, "output and error are this process's");

    // LocalRuntime's builders: this process's input, output and error discarded.
    let seen = probe(dir, "local", async {
        local(
            LocalRuntime::launcher("probe", launcher(dir, "local"), dir)
                .stdin(Stdio::Inherit)
                .stdout(Stdio::Null)
                .stderr(Stdio::Null),
        )
        .await
    })
    .await;
    assert_eq!(seen.read, "hello\n");
    assert!(!seen.out && !seen.err, "output and error discarded");

    // The Process facade (`Mount::launcher`), each stream on its own.
    let facade = launcher(dir, "facade")
        .stdin(Stdio::Inherit)
        .stdout(Stdio::Null);
    let seen = probe(dir, "facade", async {
        let mount = Mount {
            anchor: Some(dir),
            launcher: Some(&facade),
            ..Mount::default()
        };
        assert!(Process::mount(dir, mount).await.is_err(), "it never greets");
    })
    .await;
    assert_eq!(seen.read, "hello\n");
    assert!(!seen.out, "output discarded");
    assert!(seen.err, "error still this process's");
}
