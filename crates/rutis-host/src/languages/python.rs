//! Python: `runtimes.py`, an interpreter with the `rutis` package, rows
//! `py:<module>`.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::Launcher;
use serde_json::{json, Value};

use super::{id_of, rebase_program, Language, Mount};
use crate::config::{venv_python, HostConfig, PythonRuntime, Restart, Runtimes, DEFAULT_PYTHON};
use crate::connect::SpawnPlugin;
use crate::host::Error;
use crate::installer::quoted_path as quoted;

pub struct Python;

impl Language for Python {
    fn key(&self) -> &'static str {
        "py"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.py.is_some()
    }

    fn settings(&self, runtimes: &Runtimes) -> Option<String> {
        runtimes.py.as_ref().map(|py| format!("{py:?}"))
    }

    fn restart(&self, runtimes: &Runtimes) -> Restart {
        runtimes.py.as_ref().map_or(Restart::No, |py| py.restart)
    }

    /// The dev file's `restart`; the project's other settings stay.
    fn dev_file(&self, own: &mut Runtimes, dev: &Runtimes) {
        if let (Some(own), Some(dev)) = (&mut own.py, &dev.py) {
            own.restart = dev.restart;
        }
    }

    fn runs_by_default(&self, name: &str) -> bool {
        name.starts_with("py:")
    }

    fn enable_default(&self, runtimes: &mut Runtimes) {
        runtimes.py = Some(PythonRuntime {
            project: PathBuf::from("."),
            python: None,
            restart: Restart::No,
        });
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(py) = &mut runtimes.py {
            at(&mut py.project);
            rebase_program(&mut py.python, at);
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.py.is_none() {
            runtimes.py = other.py.take();
        }
    }

    fn runtime(&self, runtimes: &Runtimes) -> Option<Result<SpawnPlugin, Error>> {
        runtimes.py.as_ref().map(runtime)
    }

    /// Under `dev --debug` the runtime reads this terminal: `breakpoint()`
    /// stops in pdb there.
    fn start(&self, config: &HostConfig, mount: &mut Mount<'_>) -> Result<(), Error> {
        let Some(runtime) = self.runtime(&config.runtimes) else {
            return Ok(());
        };
        let runtime = runtime?;
        match config.runtimes.debug {
            true => mount.local_terminal(self.key(), runtime),
            false => mount.local(self.key(), runtime),
        }
        Ok(())
    }

    fn debugging(&self) -> Option<Result<&'static str, String>> {
        Some(Ok(
            "the Python runtime reads this terminal: breakpoint() stops there in pdb",
        ))
    }

    fn new_names(&self) -> &'static [&'static str] {
        &["python", "py"]
    }

    fn template(&self) -> &'static [(&'static str, &'static str)] {
        TEMPLATE
    }

    fn next_steps(&self) -> &'static str {
        "uv sync && uv run python -m unittest discover -s tests && uv run rutis-host dev"
    }

    fn project_file(&self) -> &'static str {
        "pyproject.toml"
    }

    fn is_project(&self, dir: &Path) -> bool {
        dir.join("pyproject.toml").exists()
    }

    /// The module of the first entry point of the group rutis.plugins (so
    /// it runs from the source, installed or not), else the project name as
    /// a module.
    fn dev(&self, dir: &Path, _built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String> {
        let text = std::fs::read_to_string(dir.join("pyproject.toml"))
            .map_err(|error| error.to_string())?;
        let (id, module) = entry_point(&text)
            .or_else(|| project_name(&text).map(|name| (name.clone(), name.replace('-', "_"))))
            .ok_or("pyproject.toml has no [project] name")?;
        let row = json!({ "id": id_of(&id), "name": format!("py:{module}"), "config": {} });
        // Plugin modules are found in the project, or in src/ for that layout.
        let project = match dir.join("src").is_dir() {
            true => dir.join("src"),
            false => dir.to_owned(),
        };
        let runtimes = Runtimes {
            py: Some(PythonRuntime {
                restart: Restart::No,
                project,
                python: Some(venv_python(&dir.join(".venv"))).filter(|venv| venv.exists()),
            }),
            ..Runtimes::default()
        };
        Ok((vec![row], runtimes))
    }
}

/// The Python runtime of `py.project`, with an interpreter that has the
/// `rutis` package.
fn runtime(py: &PythonRuntime) -> Result<SpawnPlugin, Error> {
    let named = py
        .python
        .clone()
        .or_else(|| std::env::var_os("VIRTUAL_ENV").map(|venv| venv_python(Path::new(&venv))))
        .or_else(|| Some(venv_python(&py.project.join(".venv"))).filter(|venv| venv.exists()));
    let interpreter = named
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PYTHON));
    // A source checkout of the rutis package (tests, development of rutis).
    let source = std::env::var_os("RUTIS_PYTHON_PATH");
    let mut check = std::process::Command::new(&interpreter);
    check.args(["-c", "import rutis"]);
    if let Some(source) = &source {
        check.env("PYTHONPATH", source);
    }
    let found = check.output().map_err(|error| {
        not_there(&interpreter).unwrap_or_else(|| {
            format!(
                "the Python runtime cannot run its interpreter {} ({error}): install Python, \
                 or set runtimes.py.python to an interpreter that has the rutis package",
                interpreter.display()
            )
        })
    })?;
    if !found.status.success() {
        return Err(match named {
            Some(interpreter) => missing_rutis(&interpreter),
            None => no_environment(&py.project),
        });
    }
    let mut runtime =
        SpawnPlugin::new("py", Launcher::python(&py.project), &py.project).interpreter(interpreter);
    if let Some(source) = source {
        runtime = runtime.python_path(source);
    }
    Ok(runtime)
}

/// The `rutis` package of this host's version, as pip names it.
fn package() -> String {
    format!("rutis=={}", env!("CARGO_PKG_VERSION"))
}

/// The interpreter `interpreter` without the rutis package: how to add it.
fn missing_rutis(interpreter: &Path) -> String {
    let python = quoted(interpreter);
    format!(
        "the Python runtime needs the rutis package in {}: run `uv pip install --python {python} {package}` \
         (or `{python} -m pip install {package}`)",
        interpreter.display(),
        package = package()
    )
}

/// No environment named or found, and `python3` without rutis: the
/// project's own, which the host then uses by itself.
fn no_environment(project: &Path) -> String {
    let venv = project.join(".venv");
    format!(
        "the Python runtime found no environment (runtimes.py.python, $VIRTUAL_ENV or {}), \
         and {DEFAULT_PYTHON} has no rutis package: create the project's environment with {}",
        venv.display(),
        create_venv(&venv)
    )
}

/// An interpreter named that is not there, in a virtual environment's
/// place: the environment is to be created (it is not there), or created
/// again (it lacks its interpreter). `None` for any other interpreter.
fn not_there(interpreter: &Path) -> Option<String> {
    if interpreter.exists() {
        return None;
    }
    let venv = venv_of(interpreter)?;
    if !venv.exists() {
        return Some(format!(
            "the Python runtime's environment {} does not exist: create it with {}",
            venv.display(),
            create_venv(&venv)
        ));
    }
    venv.join("pyvenv.cfg").exists().then(|| {
        format!(
            "the Python runtime's environment {} has no interpreter {}: create it again with {}",
            venv.display(),
            interpreter.display(),
            create_venv(&venv)
        )
    })
}

/// The commands that create the virtual environment `venv` with the
/// rutis package in it.
fn create_venv(venv: &Path) -> String {
    let python = quoted(&venv_python(venv));
    format!(
        "`uv venv {venv}` and `uv pip install --python {python} {package}` \
         (or `{DEFAULT_PYTHON} -m venv {venv}` and `{python} -m pip install {package}`)",
        venv = quoted(venv),
        package = package()
    )
}

/// The virtual environment whose interpreter `interpreter` is, by its
/// place (`<venv>/bin/python`, on Windows `<venv>\Scripts\python.exe`).
pub(crate) fn venv_of(interpreter: &Path) -> Option<PathBuf> {
    let dir = interpreter.parent()?;
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    let named = interpreter.file_name()?.to_str()?.starts_with("python");
    (named && dir.file_name()? == bin).then(|| dir.parent().map(Path::to_path_buf))?
}

/// The first entry under `[project.entry-points."rutis.plugins"]`: its name
/// and its module.
fn entry_point(pyproject: &str) -> Option<(String, String)> {
    let mut inside = false;
    for line in pyproject.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == r#"[project.entry-points."rutis.plugins"]"#;
            continue;
        }
        if inside {
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim().trim_matches('"');
                let module = value
                    .trim()
                    .trim_matches('"')
                    .split(':')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() && !name.starts_with('#') && !module.is_empty() {
                    return Some((name.to_owned(), module.to_owned()));
                }
            }
        }
    }
    None
}

fn project_name(pyproject: &str) -> Option<String> {
    let mut inside = false;
    for line in pyproject.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[project]";
            continue;
        }
        if let (true, Some(("name", value))) =
            (inside, line.split_once('=').map(|(k, v)| (k.trim(), v)))
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

const TEMPLATE: &[(&str, &str)] = &[
    (
        "pyproject.toml",
        include_str!("../../templates/python/pyproject.toml"),
    ),
    (
        "src/__module__/__init__.py",
        include_str!("../../templates/python/src/__module__/__init__.py"),
    ),
    (
        "tests/test_plugin.py",
        include_str!("../../templates/python/tests/test_plugin.py"),
    ),
    (
        "rutis.dev.json",
        include_str!("../../templates/python/rutis.dev.json"),
    ),
    (
        "README.md",
        include_str!("../../templates/python/README.md"),
    ),
    (
        ".gitignore",
        include_str!("../../templates/python/gitignore"),
    ),
    (
        ".github/workflows/publish.yml",
        include_str!("../../templates/python/.github/workflows/publish.yml"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// An interpreter's environment is known by its place.
    #[cfg(unix)]
    #[test]
    fn an_interpreter_in_an_environment_names_it() {
        assert_eq!(
            venv_of(Path::new("/app/.venv/bin/python")),
            Some(PathBuf::from("/app/.venv"))
        );
        assert_eq!(
            venv_of(Path::new("/app/env/bin/python3.12")),
            Some(PathBuf::from("/app/env"))
        );
        assert_eq!(venv_of(Path::new("/usr/local/python")), None);
        assert_eq!(venv_of(Path::new("python3")), None);
        // A system interpreter that is not there is not an environment to
        // create: its directory is there, and is no environment.
        assert_eq!(not_there(Path::new("/usr/bin/python-not-here")), None);
        let dir = std::env::temp_dir().join("rutis-not-there-test");
        assert!(not_there(&dir.join("absent/.venv/bin/python"))
            .unwrap()
            .contains("does not exist: create it with `uv venv"));
    }

    /// Paths with spaces are quoted in the commands given.
    #[cfg(unix)]
    #[test]
    fn paths_with_spaces_are_quoted() {
        assert_eq!(quoted(Path::new("/app/.venv")), "/app/.venv");
        assert_eq!(quoted(Path::new("/my app/.venv")), "'/my app/.venv'");
        assert!(create_venv(Path::new("/my app/.venv"))
            .starts_with("`uv venv '/my app/.venv'` and `uv pip install --python '/my app/.venv/bin/python' rutis=="));
    }

    /// What creates an environment, and what adds the package to one: the
    /// package of this host's version (#666).
    /// guarantee: host/error-messages
    #[cfg(unix)]
    #[test]
    fn the_python_messages_give_the_commands() {
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            create_venv(Path::new("/app/.venv")),
            format!(
                "`uv venv /app/.venv` and `uv pip install --python /app/.venv/bin/python rutis=={version}` \
                 (or `python3 -m venv /app/.venv` and `/app/.venv/bin/python -m pip install rutis=={version}`)"
            )
        );
        assert_eq!(
            missing_rutis(Path::new("/usr/bin/python3")),
            format!(
                "the Python runtime needs the rutis package in /usr/bin/python3: run \
                 `uv pip install --python /usr/bin/python3 rutis=={version}` \
                 (or `/usr/bin/python3 -m pip install rutis=={version}`)"
            )
        );
    }
}
