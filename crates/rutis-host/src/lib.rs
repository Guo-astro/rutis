//! The library behind the `rutis-host` binary: reading `rutis.json`, making
//! a plugin project into a configuration, and starting what it names.
//!
//! It is public for the binary and its tests, not as an API: it changes with
//! the binary, without a semver promise.

pub mod call;
pub mod config;
pub mod connect;
pub mod control;
pub mod host;
pub mod install;
mod installer;
pub mod languages;
pub mod new;
pub mod project;
pub mod restart;
pub mod rows_file;
pub mod status;

/// Print a line on stdout in one write, under the locks the runtimes'
/// output is passed on under: no runtime's line lands inside it (#332).
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        ::rutis_bridge::pipe::write_line(
            ::rutis_bridge::pipe::OutputStream::Stdout,
            &::std::format!($($arg)*),
        )
    };
}

/// [`outln!`] on stderr.
#[macro_export]
macro_rules! errln {
    ($($arg:tt)*) => {
        ::rutis_bridge::pipe::write_line(
            ::rutis_bridge::pipe::OutputStream::Stderr,
            &::std::format!($($arg)*),
        )
    };
}
