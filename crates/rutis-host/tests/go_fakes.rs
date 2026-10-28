//! Fakes written in Go (#670): in a Go project, a row of `rutis.dev.json`
//! whose name is a directory (`./dev/fakellm`, a main package of the same
//! module) is built into a binary of its own and runs as its own runtime
//! (`go-fakellm`). A change to the sources builds every binary again and
//! restarts only the runtimes whose build changed; `check` builds the fakes
//! too. What it prints is locked word for word (design try-and-test §八).
//!
//! Unix only, as the other dev tests: the host is stopped with kill(2).
#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::{Duration, SystemTime};

use common::process::{signal, unprefixed, Host};
use common::repo;
use serde_json::json;

/// Builds use the module cache, never the network.
const ENV: [(&str, &str); 3] = [
    ("GOPROXY", "off"),
    ("GOTOOLCHAIN", "local"),
    ("GOFLAGS", "-mod=mod"),
];

/// Write `text` to `path` (under `dir`), its modification time later than
/// any before: two edits within one tick of the clock are two changes.
fn edit(dir: &Path, path: &str, text: &str) {
    static LATER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    let ahead = LATER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(ahead))
        .unwrap();
}

const PLUGIN: &str = r#"package weather

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

type Forecast struct{ llm LLM }

func (f *Forecast) Today(ctx context.Context, city string) (string, error) {
	answer, err := f.llm.Ask(ctx, "weather in "+city)
	return answer + " in " + city, err
}

var Plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "weather",
	Inject:   []string{"llm"},
	Provides: rutis.Provides{"forecast": rutis.MethodsOf[*Forecast]()},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("forecast", &Forecast{llm: llm})
		return nil
	},
})
"#;

const MAIN: &str = r#"package main

import (
	"github.com/arcships/rutis/go/rutis"

	weather "example.com/weather"
)

func main() { rutis.Serve(weather.Plugin) }
"#;

/// The fake `llm`, answering `answer`, in the package `package`; `plugins`
/// copies of it in one binary.
fn fake(package: &str, answer: &str, plugins: &[&str]) -> String {
    let defs: Vec<String> = plugins
        .iter()
        .map(|name| {
            format!(
                "rutis.Define(rutis.Plugin[rutis.NoConfig]{{Name: \"{name}\", Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {{ ctx.Provide(\"llm\", llm{{}}); return nil }}}})"
            )
        })
        .collect();
    format!(
        "package {package}\n\n\
         import (\n\t\"context\"\n\n\t\"github.com/arcships/rutis/go/rutis\"\n)\n\n\
         type llm struct{{}}\n\n\
         func (llm) Ask(ctx context.Context, question string) (string, error) {{ return \"{answer}\", nil }}\n\n\
         func main() {{ rutis.Serve({}) }}\n",
        defs.join(", ")
    )
}

/// A Go project `weather` (its plugin uses `llm`), its fake `llm` in Go in
/// `dev/fakellm`, and its dev file naming the fake's directory.
fn project(dir: &Path) {
    let sdk = repo().join("go/rutis").canonicalize().unwrap();
    // The SDK's own requirements, for the module cache.
    let sum = std::fs::read_to_string(sdk.join("go.sum")).unwrap_or_default();
    edit(
        dir,
        "go.mod",
        &format!(
            "module example.com/weather\n\ngo 1.24\n\n\
             require github.com/arcships/rutis/go/rutis v0.0.0\n\n\
             replace github.com/arcships/rutis/go/rutis => {}\n",
            sdk.display()
        ),
    );
    edit(dir, "go.sum", &sum);
    edit(dir, "plugin.go", PLUGIN);
    edit(dir, "cmd/weather/main.go", MAIN);
    edit(
        dir,
        "dev/fakellm/main.go",
        &fake("main", "sunny", &["fakellm"]),
    );
    edit(
        dir,
        "rutis.dev.json",
        &json!({
            "format": 1,
            "rows": [
                { "id": "weather" },
                { "id": "llm", "name": "./dev/fakellm" },
            ]
        })
        .to_string(),
    );
}

/// Read lines until each of `wants` was read in that order, another line
/// in between or not: a line equal to it, or starting with it when it ends
/// in `*`. The lines read on the way.
fn in_order(host: &mut Host, wants: &[&str]) -> Vec<String> {
    let matches = |want: &str, line: &str| match want.strip_suffix('*') {
        Some(prefix) => unprefixed(line).starts_with(prefix),
        None => unprefixed(line) == want,
    };
    let mut read = Vec::new();
    for want in wants {
        loop {
            let line = host.wait_line(|line| Some(line.to_owned()));
            read.push(line.clone());
            if matches(want, &line) {
                break;
            }
        }
    }
    read
}

/// `dev` builds the project and its fake, each run as its own runtime; a
/// change to the fake builds both again, and only the fake's runtime
/// restarts (the plugin using it restarts with it, as any user does); a
/// fake that does not compile keeps its last build running.
/// guarantee: host/dev-reload
#[test]
fn a_go_fake_runs_as_its_own_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    project(dir);
    let mut host = Host::start_in_env(&["dev", "."], dir, &ENV);
    in_order(
        &mut host,
        &[
            "weather: building",
            "weather: built in *",
            "fakellm: building",
            "fakellm: built in *",
        ],
    );
    let mut running = vec!["weather: running", "llm: running"];
    while !running.is_empty() {
        let line = host.wait_line(|line| Some(line.to_owned()));
        running.retain(|want| *want != line);
    }

    // The fake changes: both are built, only the fake's build differs.
    edit(
        dir,
        "dev/fakellm/main.go",
        &fake("main", "rainy", &["fakellm"]),
    );
    let read = in_order(
        &mut host,
        &[
            "weather: building",
            "weather: built in *",
            "fakellm: building",
            "fakellm: built in *",
            "go-fakellm: rebuilt and restarted",
            "weather: restarted (go-fakellm restarted)",
        ],
    );
    assert!(
        !read.iter().any(|line| line.starts_with("go-weather:")),
        "{read:#?}"
    );
    assert!(dir.join(".rutis/go/fakellm-2").exists());
    assert!(!dir.join(".rutis/go/weather-2").exists(), "{read:#?}");

    // A fake that does not compile: the last build runs on.
    edit(
        dir,
        "dev/fakellm/main.go",
        "package main\n\nfunc main() { nope }\n",
    );
    in_order(
        &mut host,
        &["fakellm: the build failed; the last build keeps running"],
    );

    signal(&host, libc::SIGINT, true);
    let (status, stderr, _) = host.exit();
    assert_eq!(status.code(), Some(0), "{stderr}");
}

/// `check` in the project builds the fake too; a fake that is not a main
/// package, serves two plugins, or would run as the project's runtime is
/// refused with what to do.
#[test]
fn a_go_fake_that_cannot_run_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    project(dir);
    let check = || {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
            .arg("check")
            .current_dir(dir)
            .envs(ENV)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let (code, stdout, stderr) = check();
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(stderr.contains("fakellm: built in "), "{stderr}");
    assert!(stdout.contains("llm (go-fakellm:fakellm): ok"), "{stdout}");

    let refused = |files: &[(&str, String)], row: &str| {
        for (path, text) in files {
            edit(dir, path, text);
        }
        edit(
            dir,
            "rutis.dev.json",
            &json!({ "rows": [{ "id": "weather" }, { "id": "llm", "name": row }] }).to_string(),
        );
        let (code, _, stderr) = check();
        assert_eq!(code, Some(1), "{stderr}");
        stderr.lines().last().unwrap_or_default().to_owned()
    };
    assert_eq!(
        refused(
            &[("dev/notmain/llm.go", fake("fakellm", "sunny", &["fakellm"]))],
            "./dev/notmain"
        ),
        "rutis-host: rutis.dev.json: row llm: ./dev/notmain is not a Go main package (its package \
         is fakellm); a fake needs package main and a main that calls rutis.Serve"
    );
    assert_eq!(
        refused(
            &[("dev/fakes/main.go", fake("main", "sunny", &["fakellm", "fakedb"]))],
            "./dev/fakes"
        ),
        "rutis-host: rutis.dev.json: row llm: ./dev/fakes serves 2 plugins (fakedb, fakellm); give \
         each its own directory"
    );
    assert_eq!(
        refused(
            &[("dev/weather/main.go", fake("main", "sunny", &["fakellm"]))],
            "./dev/weather"
        ),
        "rutis-host: rutis.dev.json: row llm: ./dev/weather would run as go-weather, as the \
         project does; rename the directory"
    );
}

/// A fake that does not compile when `dev` starts: there is no last build,
/// and `dev` ends with 1.
#[test]
fn a_go_fake_that_does_not_compile_at_the_start_ends_dev() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    project(dir);
    edit(
        dir,
        "dev/fakellm/main.go",
        "package main\n\nfunc main() { nope }\n",
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .arg("dev")
        .current_dir(dir)
        .envs(ENV)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("fakellm: building\n"), "{stderr}");
    assert!(stderr.contains("undefined: nope"), "{stderr}");
}
