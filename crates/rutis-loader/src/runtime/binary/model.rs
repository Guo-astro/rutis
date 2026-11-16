//! The models' stand-ins (#622, #627): the file a runtime starts from, as the
//! model replaces it, and what a start and a stop did, in order.
//!
//! Built only with `--cfg rutis_shuttle` (`cargo test`), like the models
//! themselves. [`super::stamp`] asks this module for a file's stamp first, so
//! a model replaces a binary without the file system (a real one would make
//! the stamp a machine's clock, and its resolution a flake); and the
//! production paths that build a launcher, dispose an old runtime's view and
//! install the new one record what they did here ([`note`]), so a model
//! asserts the order rather than polling the slots for it.
//!
//! What is recorded is per thread, which is one model execution: shuttle runs
//! every task of one execution on the thread that started it, the reason
//! `crate::shuttle` keys its clock by thread too.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::Stamp;

thread_local! {
    /// The files the model put there, and the stamp each one has: a path the
    /// model did not put there is no file, the models' paths being no file
    /// system's.
    static FILES: RefCell<Option<HashMap<PathBuf, Stamp>>> = const { RefCell::new(None) };
    /// What the runtimes did, in order, since [`begin`].
    static NOTES: RefCell<Vec<Note>> = const { RefCell::new(Vec::new()) };
}

/// Forget the last execution's files and notes: the start of every one.
pub(super) fn begin() {
    FILES.with(|files| *files.borrow_mut() = Some(HashMap::new()));
    NOTES.with(|notes| notes.borrow_mut().clear());
}

/// The file at `path` now, `len` bytes: a replacement, with a stamp of its
/// own. What [`super::stamp`] answers for it from here on.
pub(super) fn file(path: &Path, len: u64) -> Stamp {
    let stamp = Stamp {
        len,
        modified: None,
        changed: None,
    };
    FILES.with(|files| {
        files
            .borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(path.to_owned(), stamp.clone());
    });
    stamp
}

/// What the model says the file at `path` is, or `None` when no model put it
/// there and the real file system answers.
pub(super) fn stamp_of(path: &Path) -> Option<Stamp> {
    FILES.with(|files| {
        files
            .borrow()
            .as_ref()
            .and_then(|files| files.get(path).cloned())
    })
}

/// Record one thing a runtime did.
pub(super) fn note(note: Note) {
    NOTES.with(|notes| notes.borrow_mut().push(note));
}

/// Everything recorded since [`begin`], in order.
pub(super) fn notes() -> Vec<Note> {
    NOTES.with(|notes| notes.borrow().clone())
}

/// One thing a runtime did, as a model checks the order of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Note {
    /// The binary was read and its launcher built, to start this file's
    /// runtime: the new configuration, before the old one is touched.
    Launched {
        runtime: String,
        program: OsString,
        stamp: Option<Stamp>,
    },
    /// The old runtime's view was disposed: its rows are gone, and what it
    /// provided with them.
    Disposed { runtime: String },
    /// The stop waits for the process of the runtime taken out of the slots.
    Waited { runtime: String },
    /// That process ended: the name is free.
    Ended { runtime: String },
    /// A runtime was installed to start from this file: the slots hold it.
    Installed {
        runtime: String,
        stamp: Option<Stamp>,
    },
}
