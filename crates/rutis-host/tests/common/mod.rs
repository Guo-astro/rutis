//! What the tests of rutis-host share: a lock for the environment, and
//! output made the same on every machine.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

#[cfg(unix)]
pub mod process;

/// The environment variables the host reads. A test that sets one, or
/// starts a host, holds [`environment`] and starts from none of them set.
pub const VARIABLES: &[&str] = &[
    "RUTIS_TOKEN",
    "RUTIS_CA",
    "RUTIS_CERT",
    "RUTIS_KEY",
    "RUTIS_NODE_RUNTIME",
    "RUTIS_PYTHON_PATH",
    "VIRTUAL_ENV",
];

static ENVIRONMENT: Mutex<()> = Mutex::new(());

/// The environment, to this test alone, with none of [`VARIABLES`] (nor
/// any `RUTIS_TOKEN_<PEER>`) set.
pub fn environment() -> MutexGuard<'static, ()> {
    let guard = ENVIRONMENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if VARIABLES.contains(&name.as_ref()) || name.starts_with("RUTIS_TOKEN_") {
            std::env::remove_var(name.as_ref());
        }
    }
    guard
}

/// This checkout.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `text` as on any machine: `dir` (as given, or canonical) is `<dir>`,
/// separators are `/`, and the system's words for a missing file are
/// `<not found>`: ENOENT; on Windows ERROR_FILE_NOT_FOUND (2, as ENOENT),
/// ERROR_PATH_NOT_FOUND (3: its directory is missing too) and, for a
/// program, what std says when it finds none on PATH.
pub fn normalize(text: &str, dir: &Path) -> String {
    let mut text = text.to_owned();
    let mut forms = vec![dir.to_path_buf()];
    if let Ok(canonical) = dir.canonicalize() {
        forms.push(canonical);
    }
    // Longest first: one form may contain the other.
    forms.sort_by_key(|form| std::cmp::Reverse(form.as_os_str().len()));
    for form in forms {
        text = text.replace(&config_url(&form), "file://<dir>");
        text = text.replace(&form.display().to_string(), "<dir>");
    }
    text = text.replace(
        &std::io::Error::from_raw_os_error(2).to_string(),
        "<not found>",
    );
    if cfg!(windows) {
        text = text.replace(
            &std::io::Error::from_raw_os_error(3).to_string(),
            "<not found>",
        );
        text = text.replace("program not found", "<not found>");
    }
    text.replace('\\', "/")
}

fn config_url(path: &Path) -> String {
    rutis_host::config::file_url(path)
}

/// Write `files` (path, contents) under `dir`.
pub fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}
