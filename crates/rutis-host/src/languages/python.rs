//! Python: `runtimes.py`, an interpreter with the `rutis` package, rows
//! `py:<module>`.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::LocalRuntime;
use serde_json::{json, Value};

use super::{id_of, rebase_program, Language};
use crate::config::{venv_python, PythonRuntime, Restart, Runtimes, DEFAULT_PYTHON};
use crate::host::Error;

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

    fn remote_names(&self) -> &'static [&'static str] {
        &["python", "py"]
    }

    fn runtime(&self, runtimes: &Runtimes) -> Option<Result<LocalRuntime, Error>> {
        runtimes.py.as_ref().map(runtime)
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
fn runtime(py: &PythonRuntime) -> Result<LocalRuntime, Error> {
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
    let found = check
        .output()
        .map_err(|error| match venv_of(&interpreter) {
            // An environment named that is not there yet (#666).
            Some(venv) if !interpreter.exists() => format!(
                "the Python runtime's environment {} does not exist: create it with {}",
                venv.display(),
                create_venv(&venv)
            ),
            _ => format!(
                "the Python runtime cannot run its interpreter {} ({error}): install Python, \
             or set runtimes.py.python to an interpreter that has the rutis package",
                interpreter.display()
            ),
        })?;
    if !found.status.success() {
        return Err(match named {
            Some(interpreter) => missing_rutis(&interpreter),
            // No environment: the project's own, which the host then uses.
            None => format!(
                "the Python runtime found no environment (runtimes.py.python, $VIRTUAL_ENV or {}), \
                 and {DEFAULT_PYTHON} has no rutis package: create the project's environment with {}",
                py.project.join(".venv").display(),
                create_venv(&py.project.join(".venv"))
            ),
        });
    }
    let mut runtime = LocalRuntime::python(&py.project).interpreter(interpreter);
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
    let python = interpreter.display();
    format!(
        "the Python runtime needs the rutis package in {python}: run `uv pip install --python {python} {package}` \
         (or `{python} -m pip install {package}`)",
        package = package()
    )
}

/// The commands that create the virtual environment `venv` with the
/// rutis package in it.
fn create_venv(venv: &Path) -> String {
    let python = venv_python(venv);
    format!(
        "`uv venv {venv}` and `uv pip install --python {python} {package}` \
         (or `{DEFAULT_PYTHON} -m venv {venv}` and `{python} -m pip install {package}`)",
        venv = venv.display(),
        python = python.display(),
        package = package()
    )
}

/// The virtual environment whose interpreter `interpreter` is, by its
/// place (`<venv>/bin/python`, on Windows `<venv>\Scripts\python.exe`).
fn venv_of(interpreter: &Path) -> Option<PathBuf> {
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
