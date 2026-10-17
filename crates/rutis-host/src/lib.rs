//! The library behind the `rutis-host` binary: reading `rutis.json`, making
//! a plugin project into a configuration, and starting what it names.
//!
//! It is public for the binary and its tests, not as an API: it changes with
//! the binary, without a semver promise.

pub mod config;
pub mod host;
pub mod languages;
pub mod new;
pub mod project;
pub mod restart;
pub mod status;

/// Print a line on stdout in one write, under the locks the runtimes'
/// output is passed on under: no runtime's line lands inside it (#332).
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        ::rutis_bridge::transport::local::write_line(
            ::rutis_bridge::transport::local::OutputStream::Stdout,
            &::std::format!($($arg)*),
        )
    };
}

/// [`outln!`] on stderr.
#[macro_export]
macro_rules! errln {
    ($($arg:tt)*) => {
        ::rutis_bridge::transport::local::write_line(
            ::rutis_bridge::transport::local::OutputStream::Stderr,
            &::std::format!($($arg)*),
        )
    };
}
