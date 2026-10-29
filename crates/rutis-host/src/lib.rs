//! The library behind the `rutis-host` binary: reading `rutis.json`, making
//! a plugin project into a configuration, and starting what it names.
//!
//! It is public for the binary and its tests, not as an API: it changes with
//! the binary, without a semver promise. The exception is
//! [`connect::SpawnPlugin`], `rutis-host/spawn`: how an application that
//! embeds rutis-loader starts a local runtime (the Rust host and open-host
//! guides).

pub mod call;
pub mod config;
pub mod connect;
pub mod control;
pub mod host;
pub mod install;
mod installer;
pub mod languages;
pub mod new;
pub mod output;
pub mod project;
pub mod restart;
pub mod rows_file;
pub mod status;
pub mod test;

/// The Rust host guide's examples compile against these crates (#667).
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/rust-host.md")]
pub struct RustHostGuide;

/// The English Rust host guide's examples compile against these crates.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/rust-host.en.md")]
pub struct RustHostGuideEn;

/// The open-host guide's examples compile against these crates (#671).
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/open-host.md")]
pub struct OpenHostGuide;

/// The English open-host guide's examples compile against these crates.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/guide/open-host.en.md")]
pub struct OpenHostGuideEn;

/// Print a line on stdout in one write, under the locks the runtimes'
/// output is passed on under: no runtime's line lands inside it (#332).
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::output::write_line(
            $crate::output::OutputStream::Stdout,
            &::std::format!($($arg)*),
        )
    };
}

/// [`outln!`] on stderr.
#[macro_export]
macro_rules! errln {
    ($($arg:tt)*) => {
        $crate::output::write_line(
            $crate::output::OutputStream::Stderr,
            &::std::format!($($arg)*),
        )
    };
}
