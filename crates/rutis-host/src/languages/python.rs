//! Python: `runtimes.py`, an interpreter with the `rutis` package, rows
//! `py:<module>`.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::LocalRuntime;
use serde_json::{json, Value};

use super::{id_of, rebase_program, Language};
use crate::config::{venv_python, PythonRuntime, Runtimes, DEFAULT_PYTHON};
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
    fn dev(&self, dir: &Path) -> Result<(Vec<Value>, Runtimes), String> {
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
    let interpreter = py
        .python
        .clone()
        .or_else(|| std::env::var_os("VIRTUAL_ENV").map(|venv| venv_python(Path::new(&venv))))
        .or_else(|| Some(venv_python(&py.project.join(".venv"))).filter(|venv| venv.exists()))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PYTHON));
    // A source checkout of the rutis package (tests, development of rutis).
    let source = std::env::var_os("RUTIS_PYTHON_PATH");
    let mut check = std::process::Command::new(&interpreter);
    check.args(["-c", "import rutis"]);
    if let Some(source) = &source {
        check.env("PYTHONPATH", source);
    }
    let found = check.output().map_err(|error| {
        format!(
            "the Python runtime cannot run its interpreter {} ({error}): install Python, \
             or set runtimes.py.python to an interpreter that has the rutis package",
            interpreter.display()
        )
    })?;
    if !found.status.success() {
        return Err(format!(
            "the Python runtime needs the rutis package in {}: run `{} -m pip install rutis` (or `uv add rutis`)",
            interpreter.display(),
            interpreter.display()
        ));
    }
    let mut runtime = LocalRuntime::python(&py.project).interpreter(interpreter);
    if let Some(source) = source {
        runtime = runtime.python_path(source);
    }
    Ok(runtime)
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
