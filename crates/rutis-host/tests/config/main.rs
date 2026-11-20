//! The `config` integration tests, one module per former test binary.
#[path = "../common/mod.rs"]
mod common;

mod android;
mod call;
mod check;
mod clash;
mod config;
mod config_props;
mod debug;
mod dev;
mod errors;
mod help;
mod implicit_runtimes;
mod install;
mod output;
mod persisted;
mod project;
mod remote_runtimes;
mod row_ready;
mod rows_file;
mod status;
mod templates;
mod test_command;
mod undeclared;
mod unhandled;
mod waiting;
