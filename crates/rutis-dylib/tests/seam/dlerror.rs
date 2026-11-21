//! A failed `dlopen` whose `dlerror` message is absent (#631).
//!
//! POSIX does not promise that `dlopen` leaves a message behind; reading
//! the null `dlerror` returns as a `CStr` is undefined behaviour.
//! `open_library` checks it, and this reaches the same check.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

/// A real failed `dlopen` leaves a message, which the first read takes; the
/// read after it finds none, as `open_library` can, and says so instead of
/// reading a null pointer.
#[test]
fn a_failed_dlopen_without_a_message_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-a-library");
    std::fs::write(&path, b"not an ELF file").unwrap();
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
    assert!(handle.is_null(), "a text file is not a library");

    let message = rutis_dylib::take_dl_error();
    assert!(!message.is_empty(), "{message}");
    assert_ne!(message, "no error message from dlerror");
    // `dlerror` also clears the message: there is none now.
    assert_eq!(
        rutis_dylib::take_dl_error(),
        "no error message from dlerror"
    );
}
