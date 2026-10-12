#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use rutis_test_support::HANG_GUARD;
use serde_json::Value;

/// The npm project with dsh, when installed.
pub fn project() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("dsh");
    dir.join("node_modules/@deepseek-ai/dsh-app-boot")
        .exists()
        .then_some(dir)
}

/// Every `cordis.patch.yml` of the installed dsh bundles, plus aimux's.
pub fn bundle_patch_files() -> Option<Vec<PathBuf>> {
    let project = project()?;
    let scope = project.join("node_modules/@deepseek-ai");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&scope)
        .ok()?
        .filter_map(|entry| {
            let file = entry.ok()?.path().join("cordis.patch.yml");
            file.exists().then_some(file)
        })
        .collect();
    files.push(project.join("aimux/aimux.patch.yml"));
    files.sort();
    Some(files)
}

/// Run `tests/support/dsh.mjs` and parse its JSON output.
pub fn node<'a>(args: &[&str], rest: impl IntoIterator<Item = &'a str>) -> Value {
    node_with(args, rest, &[])
}

/// `node`, with an exact environment: only `vars` (and `PATH`).
pub fn node_with<'a>(
    args: &[&str],
    rest: impl IntoIterator<Item = &'a str>,
    vars: &[(&str, &str)],
) -> Value {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/dsh.mjs");
    let mut command = Command::new("node");
    if !vars.is_empty() {
        command.env_clear();
        command.env("PATH", std::env::var("PATH").unwrap_or_default());
        for (key, value) in vars {
            command.env(key, value);
        }
    }
    let child = command
        .arg(script)
        .args(args)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node");
    let pid = child.id();
    let (send, done) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    // Node loading dsh's packages cold: three times HANG_GUARD; only
    // against a hang.
    let output = match done.recv_timeout(HANG_GUARD * 3) {
        Ok(output) => output.expect("node"),
        Err(error) => {
            #[cfg(unix)]
            // SAFETY: plain kill(2) on the child this call started.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = pid;
            panic!("dsh.mjs {args:?}: {error} ({:?})", HANG_GUARD * 3);
        }
    };
    assert!(
        output.status.success(),
        "dsh.mjs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("dsh.mjs output")
}
