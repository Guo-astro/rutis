//! Linux and macOS: `dlopen`/`dlsym`, and `dladdr` to find the loaded SDK.

use std::ffi::{c_void, CStr, CString, OsStr};
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

/// A `dlopen` handle. Never closed.
pub(crate) type Handle = NonNull<c_void>;

/// The `LoadError::step` for a failed library open.
pub(crate) const OPEN_STEP: &str = "dlopen";

pub(crate) fn loaded_sdk_path() -> Result<PathBuf, String> {
    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
    if unsafe {
        libc::dladdr(
            rutis_sdk::rutis_sdk_boot_id as *const c_void,
            info.as_mut_ptr(),
        )
    } == 0
    {
        return Err("dladdr failed for SDK boot function".into());
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return Err("dladdr returned no SDK path".into());
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) };
    Ok(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
}

/// Opens a cached library for reading. Unix has no share modes; the cache
/// directory must be writable only by trusted users.
pub(crate) fn open_pinned(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

pub(crate) unsafe fn open_library(path: &Path) -> Result<Handle, String> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    NonNull::new(libc::dlopen(
        path.as_ptr(),
        libc::RTLD_NOW | libc::RTLD_LOCAL,
    ))
    .ok_or_else(take_dl_error)
}

/// The message of the last failed `dlopen`, or that there is none.
///
/// POSIX does not promise that a failed `dlopen` leaves a message, and
/// `CStr::from_ptr` on the null `dlerror` then returns is undefined
/// behaviour. `find_symbol` checks the same thing.
pub(crate) fn take_dl_error() -> String {
    let message = unsafe { libc::dlerror() };
    if message.is_null() {
        "no error message from dlerror".into()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    }
}

/// `name` must end with a NUL byte.
pub(crate) unsafe fn find_symbol(handle: Handle, name: &[u8]) -> Result<*mut c_void, String> {
    debug_assert_eq!(name.last(), Some(&0));
    let ptr = libc::dlsym(handle.as_ptr(), name.as_ptr().cast());
    if ptr.is_null() {
        let message = libc::dlerror();
        return Err(if message.is_null() {
            "symbol not found".into()
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        });
    }
    Ok(ptr)
}
