//! Running the package managers: npm, bun, uv (or pip), go.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

use super::Error;

/// How the package managers are run.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tools {
    /// `PATH` for the commands (tests put recording programs there);
    /// this process's when `None`.
    pub path: Option<OsString>,
}

/// A command to run, and how messages show it.
pub(crate) struct Run {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: Vec<(OsString, OsString)>,
}

impl Run {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Self {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    pub fn cwd(mut self, dir: &Path) -> Self {
        self.cwd = Some(dir.to_owned());
        self
    }

    pub fn env(mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((name.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// As messages show it: `npm install @ana/wordcount`.
    pub fn shown(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|part| quoted(&part.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Tools {
    fn command(&self, run: &Run) -> Command {
        let mut command = Command::new(&run.program);
        command.args(&run.args);
        if let Some(cwd) = &run.cwd {
            command.current_dir(cwd);
        }
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        for (name, value) in &run.env {
            command.env(name, value);
        }
        command
    }

    /// Run it, its output shown as it is.
    pub fn run(&self, run: &Run) -> Result<(), Error> {
        self.run_or(run, |error| no_tool(run, error))
    }

    /// [`Tools::run`]; `missing` says what to do when the program cannot be
    /// started (its error given).
    pub fn run_or(&self, run: &Run, missing: impl FnOnce(&str) -> String) -> Result<(), Error> {
        let status = crate::runtime::retry_busy(|| self.command(run).stdin(Stdio::null()).status())
            .map_err(|error| Error::Other(missing(&io_error(&error))))?;
        match status.success() {
            true => Ok(()),
            false => Err(Error::Failed(format!(
                "{} failed ({})",
                run.shown(),
                exit_status(&status)
            ))),
        }
    }

    /// Run it for what it prints (its errors shown as they are).
    pub fn output(&self, run: &Run) -> Result<String, Error> {
        self.output_or(run, |error| no_tool(run, error))
    }

    /// [`Tools::output`]; `missing` as for [`Tools::run_or`].
    pub fn output_or(
        &self,
        run: &Run,
        missing: impl FnOnce(&str) -> String,
    ) -> Result<String, Error> {
        let output = crate::runtime::retry_busy(|| {
            self.command(run)
                .stdin(Stdio::null())
                .stderr(Stdio::inherit())
                .output()
        })
        .map_err(|error| Error::Other(missing(&io_error(&error))))?;
        match output.status.success() {
            true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            false => Err(Error::Failed(format!(
                "{} failed ({})",
                run.shown(),
                exit_status(&output.status)
            ))),
        }
    }

    /// Whether it runs and succeeds, nothing shown.
    pub fn succeeds(&self, run: &Run) -> bool {
        crate::runtime::retry_busy(|| {
            self.command(run)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        })
        .is_ok_and(|status| status.success())
    }
}

/// A package manager that cannot be run: what to install.
pub(crate) fn no_tool(run: &Run, error: &str) -> String {
    let program = run.program.to_string_lossy();
    let install = match program.as_ref() {
        "npm" => ": install Node.js 22 or later",
        "bun" => ": install Bun (https://bun.com)",
        "uv" => ": install uv",
        _ => "",
    };
    format!("cannot run {program} ({error}){install}")
}

/// `exit status 1`.
fn exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => status.to_string(),
    }
}

/// An I/O error as messages say it: `No such file or directory`, without
/// the `(os error 2)` the standard library adds.
pub(crate) fn io_error(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_owned(),
        None => text,
    }
}

/// `text` as a shell takes it: quoted when it has a space or a character
/// the shell would read.
pub(crate) fn quoted(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || "/\\._-:+=@%,~".contains(c));
    match (plain, cfg!(windows)) {
        (true, _) => text.to_owned(),
        (false, true) => format!("\"{text}\""),
        (false, false) => format!("'{}'", text.replace('\'', "'\\''")),
    }
}

/// `path` as a shell takes it.
#[doc(hidden)]
pub fn quoted_path(path: &Path) -> String {
    quoted(&path.display().to_string())
}
