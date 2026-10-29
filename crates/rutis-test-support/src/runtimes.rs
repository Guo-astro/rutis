//! Which language runtimes a test run uses, and how the tests start them
//! (`docs/design-platform-cleanup-2026-10-28.md` §4.4 item 5).
//!
//! `RUTIS_TEST_RUNTIMES` lists the languages, comma separated
//! (`node,python`): a test of a listed language whose toolchain is missing
//! fails saying what is missing; a test of a language not listed prints
//! `skipped: <language> not in RUTIS_TEST_RUNTIMES` and returns. Each CI job
//! sets it. It is a test convention, not a product setting.
//!
//! [`Command`] is how a runtime process starts, as plain data: this crate
//! depends on nothing of rutis, and a test turns it into its own launcher.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The variable listing the languages a test run uses.
pub const VARIABLE: &str = "RUTIS_TEST_RUNTIMES";

/// The languages a test can need.
pub const LANGUAGES: &[&str] = &["node", "python", "bun", "go", "swift", "kotlin"];

/// Whether this run uses `language` (one of [`LANGUAGES`]): listed in
/// `RUTIS_TEST_RUNTIMES`, its toolchain there. Not listed: prints
/// `skipped: <language> not in RUTIS_TEST_RUNTIMES` and gives `false`, the
/// test returning. Listed without its toolchain: panics, saying what is
/// missing.
pub fn enabled(language: &str) -> bool {
    assert!(
        LANGUAGES.contains(&language),
        "{language} is not a test language: {LANGUAGES:?}"
    );
    if !listed().iter().any(|listed| listed == language) {
        println!("skipped: {language} not in {VARIABLE}");
        return false;
    }
    if let Err(missing) = toolchain(language) {
        panic!("{VARIABLE} lists {language}, but {missing}");
    }
    true
}

/// Whether this run uses every one of `languages`; prints the skip line
/// for the first it does not.
pub fn all_enabled(languages: &[&str]) -> bool {
    languages.iter().all(|language| enabled(language))
}

/// The languages `RUTIS_TEST_RUNTIMES` lists; an unknown one fails the run.
fn listed() -> &'static [String] {
    static LISTED: OnceLock<Vec<String>> = OnceLock::new();
    LISTED.get_or_init(|| {
        let value = std::env::var(VARIABLE).unwrap_or_default();
        let listed: Vec<String> = value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        for name in &listed {
            assert!(
                LANGUAGES.contains(&name.as_str()),
                "{VARIABLE} lists {name:?}, not one of {LANGUAGES:?}"
            );
        }
        listed
    })
}

/// Whether `language`'s toolchain runs here: why not, once per language.
fn toolchain(language: &str) -> Result<(), String> {
    static CHECKED: OnceLock<std::sync::Mutex<Vec<(String, Result<(), String>)>>> =
        OnceLock::new();
    let checked = CHECKED.get_or_init(Default::default);
    let mut checked = checked.lock().unwrap();
    if let Some((_, result)) = checked.iter().find(|(name, _)| name == language) {
        return result.clone();
    }
    let python = python();
    let (program, args): (OsString, &[&str]) = match language {
        "node" => ("node".into(), &["--version"]),
        "python" => (
            python.into_os_string(),
            &["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 12) else 1)"],
        ),
        "bun" => ("bun".into(), &["--version"]),
        "go" => ("go".into(), &["version"]),
        "swift" => ("swift".into(), &["--version"]),
        _ => ("java".into(), &["-version"]),
    };
    let ran = std::process::Command::new(&program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let result = match ran {
        Ok(status) if status.success() => Ok(()),
        Ok(_) if language == "python" => Err(format!(
            "{} is not Python 3.12 or later",
            program.to_string_lossy()
        )),
        Ok(status) => Err(format!("{} failed ({status})", program.to_string_lossy())),
        Err(error) => Err(format!("{} does not run ({error})", program.to_string_lossy())),
    };
    checked.push((language.to_owned(), result.clone()));
    result
}

/// This checkout.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Python interpreter: `RUTIS_PYTHON`, else `python3` (`python` on
/// Windows).
pub fn python() -> PathBuf {
    std::env::var_os("RUTIS_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "python" } else { "python3" }))
}

/// How a runtime process starts: its program, arguments, environment and
/// directory, and whether it takes its channel as an inherited socket
/// (`fd:3`) rather than dialing back.
#[derive(Debug, Clone)]
pub struct Command {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
    pub inherit_fd: bool,
}

impl Command {
    fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            inherit_fd: true,
        }
    }

    fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
}

/// The Node runtime of the npm package `package` (`node/rutis-runtime`):
/// `node --import tsx src/runner.mjs` in the package.
pub fn node(package: &Path) -> Command {
    Command::new("node")
        .arg("--import")
        .arg("tsx")
        .arg(package.join("src/runner.mjs"))
        .cwd(package)
}

/// The repository's Node runtime.
pub fn node_runtime() -> Command {
    node(&repo().join("node/rutis-runtime"))
}

/// The Python runtime: [`python`]` -m rutis` in `project`, with `project`
/// and then the repository's `python/rutis` ahead of the inherited
/// `PYTHONPATH`.
pub fn python_runtime(project: &Path) -> Command {
    let mut entries = vec![project.to_owned(), repo().join("python/rutis")];
    if let Some(inherited) = std::env::var_os("PYTHONPATH").filter(|path| !path.is_empty()) {
        entries.extend(std::env::split_paths(&inherited));
    }
    let path = std::env::join_paths(&entries).expect("paths without separators");
    Command::new(python())
        .arg("-m")
        .arg("rutis")
        .env("PYTHONPATH", path)
        .env("PYTHONUNBUFFERED", "1")
        // A plugin imported again after an edit must not come from a
        // bytecode file written in the same second as the old source.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .cwd(project)
}

/// The Bun runtime of the npm package `package` (`bun/rutis-bun`), run by
/// `bun`, importing plugin modules from `project`.
pub fn bun(package: &Path, project: &Path) -> Command {
    Command::new("bun")
        .arg("--no-install")
        .arg("--no-env-file")
        .arg(package.join("src/main.ts"))
        .cwd(project)
}

/// The repository's Bun runtime, for plugins in `project`.
pub fn bun_runtime(project: &Path) -> Command {
    bun(&repo().join("bun/rutis-bun"), project)
}

/// A compiled language's runtime (Go, Swift): `binary`, in `project`.
pub fn binary(binary: &Path, project: &Path) -> Command {
    Command::new(binary).cwd(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_start_as_the_runtimes_expect() {
        let node = node_runtime();
        assert_eq!(node.program, "node");
        assert!(node.args.last().unwrap().to_string_lossy().ends_with("src/runner.mjs"));
        assert!(node.inherit_fd);
        let dir = Path::new("/tmp/project");
        let py = python_runtime(dir);
        assert_eq!(py.args, ["-m", "rutis"]);
        assert_eq!(py.cwd.as_deref(), Some(dir));
        assert!(py.env.iter().any(|(name, _)| name == "PYTHONPATH"));
    }
}
