//! `rutis-host add`, `remove` and `upgrade` (plugin install design §十三
//! P2): a row named after a record runs where the record says, old row
//! names beside it; the file written back in its own key order; and each
//! error of §十一 word for word, the file unchanged. The package managers
//! are recording programs on `PATH` (Unix); Node and Python run as the
//! host's other tests have them.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{normalize, repo, write, VARIABLES};
use rutis_test_support::{Guard, HANG_GUARD};

struct Ran {
    status: i32,
    stdout: String,
    stderr: String,
}

/// `rutis-host <args>` in `dir`, with `PATH` set to `path` when given and
/// only the given variables of the host's own set.
async fn host(dir: &Path, args: &[&str], path: Option<&Path>, env: &[(&str, &Path)]) -> Ran {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command.args(args).current_dir(dir).kill_on_drop(true);
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if VARIABLES.contains(&name.as_str()) || name.starts_with("RUTIS_TOKEN_") {
            command.env_remove(name);
        }
    }
    if let Some(path) = path {
        command.env("PATH", path);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = Guard(HANG_GUARD * 6)
        .within("rutis-host ends", command.output())
        .await
        .expect("rutis-host runs");
    Ran {
        status: output.status.code().expect("an exit status"),
        stdout: normalize(&String::from_utf8_lossy(&output.stdout), dir),
        stderr: normalize(&String::from_utf8_lossy(&output.stderr), dir),
    }
}

/// A recording program `name` in `bin`, running `body`.
fn program(bin: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `bin`, then where `node` is (the Node runtime's), then the system's
/// programs (`mkdir`, `cp`, `sh`).
fn path_with(bin: &Path) -> PathBuf {
    let node = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .find(|dir| dir.join("node").is_file())
        .map(|dir| format!(":{}", dir.display()))
        .unwrap_or_default();
    PathBuf::from(format!("{}{node}:/usr/bin:/bin", bin.display()))
}

fn python_settings() -> serde_json::Value {
    match std::env::var_os("RUTIS_PYTHON") {
        Some(python) => serde_json::json!({ "python": python.to_string_lossy() }),
        None => serde_json::json!({}),
    }
}

/// A record is kept under its row's name and changes nothing about how
/// rows resolve: the recorded row and one beside it run as rows do.
/// guarantee: host/plugin-install
#[tokio::test(flavor = "multi_thread")]
async fn rows_of_recorded_plugins_resolve_as_rows_do() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = "provides = {'geo': {'find': 'sync'}}\ndef apply(ctx, config):\n    pass\n";
    write(
        dir.path(),
        &[
            ("geo.py", plugin),
            ("old.py", &plugin.replace("geo", "old")),
            (
                "rutis.json",
                &serde_json::json!({
                    "runtimes": { "py": python_settings() },
                    "installed": { "py:geo": { "from": "pypi:geo-plugin" } },
                    "rows": [
                        { "id": "geo", "name": "py:geo" },
                        { "id": "old", "name": "py:old" }
                    ]
                })
                .to_string(),
            ),
        ],
    );
    let python = repo().join("python/rutis");
    let ran = host(
        dir.path(),
        &["check"],
        None,
        &[("RUTIS_PYTHON_PATH", &python)],
    )
    .await;
    assert_eq!(ran.stderr, "");
    assert_eq!(
        ran.stdout,
        "geo (py:geo): ok\n  inject: []\n  provides: {\"geo\":{\"find\":\"sync\"}}\n\
         old (py:old): ok\n  inject: []\n  provides: {\"old\":{\"find\":\"sync\"}}\n"
    );
    assert_eq!(ran.status, 0);
}

/// A record under a name that is not a row name `add` writes is refused;
/// a record keeps `from` only.
#[test]
fn a_record_not_under_a_row_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rutis.json");
    let error = rutis_host::config::HostConfig::parse(
        r#"{ "installed": { "ruby:geo": { "from": "./geo" } } }"#,
        &path,
    )
    .unwrap_err();
    assert_eq!(
        normalize(&error, dir.path()),
        "<dir>/rutis.json: installed.ruby:geo: name it by its row: a Node package, or bun:, py:, go: or swift: and the plugin"
    );
    let error = rutis_host::config::HostConfig::parse(
        r#"{ "installed": { "py:geo": { "from": "pypi:geo", "language": "py" } } }"#,
        &path,
    )
    .unwrap_err();
    assert!(error.contains("unknown field `language`"), "{error}");
}

/// The Node plugin the recording npm installs.
const WORDCOUNT: &str = "export const name = 'wordcount'\nexport const inject = ['notes']\nexport function apply(ctx) {}\n";

/// `add` of an npm package: what it says, and the file written back in
/// its own key order, with the record, the runtime, and a row whose id is
/// free; `remove` takes them out again.
/// guarantee: host/plugin-install
#[tokio::test(flavor = "multi_thread")]
async fn add_writes_the_file_back_in_its_own_order() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let source = dir.path().join("source");
    write(
        &source,
        &[
            (
                "package.json",
                r#"{ "name": "@ana/wordcount", "version": "1.2.0", "type": "module", "main": "index.mjs" }"#,
            ),
            ("index.mjs", WORDCOUNT),
        ],
    );
    write(
        &app,
        &[
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "rutis.json",
                "{\n  \"rows\": [\n    { \"id\": \"wordcount\", \"name\": \"./other.mjs\", \"disabled\": true }\n  ],\n  \"id\": \"desk\"\n}\n",
            ),
        ],
    );
    let bin = dir.path().join("bin");
    program(
        &bin,
        "npm",
        &format!(
            "echo \"npm $*\" >> '{log}'\ncase \"$1\" in\nview) echo '{{}}';;\ninstall) mkdir -p node_modules/@ana/wordcount && cp '{source}'/* node_modules/@ana/wordcount/;;\nuninstall) rm -rf node_modules/@ana/wordcount;;\nesac",
            log = dir.path().join("log").display(),
            source = source.display()
        ),
    );
    let node = repo().join("node/rutis-runtime");
    let env = [("RUTIS_NODE_RUNTIME", node.as_path())];
    let ran = host(
        &app,
        &["add", "npm:@ana/wordcount"],
        Some(&path_with(&bin)),
        &env,
    )
    .await;
    assert_eq!(ran.stderr, "");
    assert_eq!(
        ran.stdout,
        "installed @ana/wordcount 1.2.0 (node) into <dir>\n\
         wordcount-2 (@ana/wordcount): cannot run: nothing provides notes\n  \
         version: \"1.2.0\"\n  \
         inject: [\"notes\"]\n  \
         provides: none declared (it may provide more once it runs)\n\
         rutis.json: added row wordcount-2, runtimes.node\n"
    );
    assert_eq!(ran.status, 0);
    assert_eq!(
        std::fs::read_to_string(app.join("rutis.json")).unwrap(),
        r#"{
  "runtimes": {
    "node": {}
  },
  "installed": {
    "@ana/wordcount": {
      "from": "npm:@ana/wordcount"
    }
  },
  "rows": [
    {
      "id": "wordcount",
      "name": "./other.mjs",
      "disabled": true
    },
    {
      "id": "wordcount-2",
      "name": "@ana/wordcount"
    }
  ],
  "id": "desk"
}
"#
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("log")).unwrap(),
        "npm view @ana/wordcount --json\nnpm install @ana/wordcount\n"
    );

    // Added again: no second row.
    let ran = host(
        &app,
        &["add", "npm:@ana/wordcount"],
        Some(&path_with(&bin)),
        &env,
    )
    .await;
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert!(
        ran.stdout
            .ends_with("rutis.json: rows naming @ana/wordcount were there already\n"),
        "{}",
        ran.stdout
    );

    let ran = host(
        &app,
        &["remove", "@ana/wordcount"],
        Some(&path_with(&bin)),
        &env,
    )
    .await;
    assert_eq!(ran.stderr, "");
    assert_eq!(
        ran.stdout,
        "uninstalled @ana/wordcount from <dir>\nrutis.json: removed row wordcount-2\n"
    );
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(app.join("rutis.json")).unwrap()).unwrap();
    assert_eq!(written["installed"], serde_json::json!({}));
    assert_eq!(written["rows"].as_array().unwrap().len(), 1);
}

/// The environment `uv venv` makes: an interpreter that has the rutis
/// package and answers the distribution query with `$ANSWER`.
fn uv(answer: &str) -> String {
    [
        "case \"$1\" in",
        "venv) mkdir -p \"$2/bin\"; cat > \"$2/bin/python\" <<'PY'",
        "#!/bin/sh",
        "case \"$2\" in",
        "\"import rutis\") exit 0;;",
        &format!("*) echo '{answer}';;"),
        "esac",
        "PY",
        "chmod +x \"$2/bin/python\";;",
        "esac",
    ]
    .join("\n")
}

/// Each error of §十一, word for word, with status 1 and the file as it
/// was.
/// guarantee: host/plugin-install
#[tokio::test(flavor = "multi_thread")]
async fn the_errors_say_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let file = "{ \"runtimes\": { \"go\": {} }, \"installed\": { \"wordcount\": { \"from\": \"./plugins/wordcount\" } }, \"rows\": [] }\n";
    write(
        &app,
        &[
            ("rutis.json", file),
            ("package.json", "{}"),
            ("plugins/notes/README.md", ""),
        ],
    );
    let node = repo().join("node/rutis-runtime");
    let errors = [
        (
            vec!["add", "wordcount"],
            None,
            "rutis-host: wordcount: say where it comes from: npm:wordcount, pypi:wordcount, a Go module path or a local path",
        ),
        (
            vec!["add", "npm:@ana/wordcount"],
            None,
            "rutis-host: cannot run npm (No such file or directory): install Node.js 22 or later",
        ),
        (
            vec!["add", "pypi:bo-llm"],
            None,
            "rutis-host: cannot create the Python environment <dir>/.venv: install uv, or Python 3.10 or later",
        ),
        (
            vec!["add", "npm:@ana/wordcount"],
            Some(("npm", "case \"$1\" in view) echo '{}';; install) exit 1;; esac".to_owned())),
            "rutis-host: npm install @ana/wordcount failed (exit status 1); rutis.json is unchanged",
        ),
        (
            vec!["add", "pypi:bo-utils"],
            Some((
                "uv",
                uv(r#"{"name": "bo-utils", "version": "1.0", "plugins": []}"#),
            )),
            "rutis-host: bo-utils has no rutis.plugins entry point: it is not a plugin; it stays installed: \
             uv pip uninstall --python <dir>/.venv/bin/python bo-utils",
        ),
        (
            vec!["add", "./plugins/notes"],
            None,
            "rutis-host: ./plugins/notes has no package.json, pyproject.toml or go.mod",
        ),
        (
            vec!["add", "npm:wordcount"],
            None,
            "rutis-host: wordcount is already installed from ./plugins/wordcount: remove it first",
        ),
        (
            vec!["add", "github.com/bo/netkit/cmd/netkit"],
            None,
            "rutis-host: runtimes.go has no dir to install into: add one, or remove runtimes.go to use plugins/go",
        ),
        (
            vec!["remove", "llm"],
            None,
            "rutis-host: llm was not installed by add: uninstall it with the package manager that installed it, and remove its row",
        ),
    ];
    for (args, recording, said) in errors {
        // Only the recording program of this case, and nothing else on PATH.
        let _ = std::fs::remove_dir_all(&bin);
        std::fs::create_dir_all(&bin).unwrap();
        if let Some((name, body)) = &recording {
            program(&bin, name, body);
        }
        let path = match recording {
            Some(_) => path_with(&bin),
            None => bin.clone(),
        };
        let ran = host(&app, &args, Some(&path), &[("RUTIS_NODE_RUNTIME", &node)]).await;
        assert_eq!(ran.status, 1, "{args:?}: {}", ran.stderr);
        assert_eq!(ran.stderr.lines().last(), Some(said), "{args:?}");
        assert_eq!(
            std::fs::read_to_string(app.join("rutis.json")).unwrap(),
            file,
            "{args:?}"
        );
        let _ = std::fs::remove_dir_all(app.join(".venv"));
    }
}
