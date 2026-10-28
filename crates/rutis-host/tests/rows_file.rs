//! `run`'s `rutis.json` as the layer plugins edit through the service
//! `loader` (#185, design "The host writes its own plugins" §五, §七): rows
//! added and removed are written back into it, its other fields and keys
//! as they were; `--watch-config` takes what was written as no change; a
//! file changed meanwhile is read before the edit is made again; the rows
//! run again after a restart; a file that cannot be written says so.
//!
//! Unix only: `call` is not available on Windows.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;

use common::process::{signal, Host};

/// A Python plugin providing `maker`, which adds and removes rows through
/// `loader` (after its `apply`, in its methods).
const MAKER: &str = r#"
inject = ["loader"]


class Maker:
    def __init__(self, loader):
        self.loader = loader

    async def add(self, id, name):
        return await self.loader.create({"id": id, "name": name})

    async def drop(self, id):
        await self.loader.remove(id)


def apply(ctx, config):
    ctx.provide("maker", Maker(ctx.use("loader")))
"#;

/// A Python plugin that does nothing.
const TINY: &str = "def apply(ctx, config):\n    pass\n";

/// `rutis.json` with `rows` (each `(id, name)`), its keys in an order of
/// their own, written as `rutis-host` writes it.
fn file(rows: &[(&str, &str)]) -> String {
    let rows: Vec<String> = rows
        .iter()
        .map(|(id, name)| {
            format!("    {{\n      \"id\": \"{id}\",\n      \"name\": \"{name}\"\n    }}")
        })
        .collect();
    let python = match std::env::var("RUTIS_PYTHON") {
        Ok(python) => format!(
            ",\n      \"python\": {}",
            serde_json::to_string(&python).unwrap()
        ),
        Err(_) => String::new(),
    };
    format!(
        "{{\n  \"rows\": [\n{}\n  ],\n  \"id\": \"edits\",\n  \"runtimes\": {{\n    \"py\": {{\n      \"project\": \".\"{python}\n    }}\n  }}\n}}\n",
        rows.join(",\n")
    )
}

/// `rutis-host call <args>` in `dir`: exit code, stdout, stderr.
fn call(args: &[&str], dir: &Path, runtime: &Path) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .arg("call")
        .args(args)
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", runtime)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn start(args: &[&str], dir: &Path, runtime: &Path) -> Host {
    Host::start_in_env(args, dir, &[("XDG_RUNTIME_DIR", runtime.to_str().unwrap())])
}

fn read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("rutis.json")).unwrap()
}

/// guarantee: host/rows-written-back
#[test]
fn rows_plugins_add_and_remove_are_written_into_rutis_json() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    std::fs::write(dir.join("maker.py"), MAKER).unwrap();
    std::fs::write(dir.join("tiny.py"), TINY).unwrap();
    let maker = ("maker", "py:maker");
    std::fs::write(dir.join("rutis.json"), file(&[maker])).unwrap();

    // Added and removed, with `--watch-config`: the file as written back.
    let mut host = start(&["run", "--watch-config", "rutis.json"], dir, runtime);
    host.wait_lines(&["maker: running"]);
    assert_eq!(
        call(&["maker.add", "\"weather\"", "\"py:tiny\""], dir, runtime),
        (Some(0), "\"weather\"\n".into(), String::new())
    );
    host.wait_lines(&["weather: running"]);
    assert_eq!(read(dir), file(&[maker, ("weather", "py:tiny")]));
    // What was written is no change: the first reload is the hand edit's.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    std::fs::write(
        dir.join("rutis.json"),
        file(&[maker, ("weather", "py:tiny"), ("hand", "py:tiny")]),
    )
    .unwrap();
    let reloaded = host.wait_line(|line| {
        line.starts_with("rutis-host: reloaded")
            .then(|| line.to_owned())
    });
    assert_eq!(reloaded, "rutis-host: reloaded rutis.json (1 row changed)");
    host.wait_lines(&["hand: running"]);
    assert_eq!(
        call(&["maker.drop", "\"weather\""], dir, runtime),
        (Some(0), "null\n".into(), String::new())
    );
    host.wait_lines(&["weather: removed"]);
    assert_eq!(read(dir), file(&[maker, ("hand", "py:tiny")]));
    signal(&host, libc::SIGINT, true);
    let (status, stderr, _) = host.exit();
    assert_eq!(status.code(), Some(0), "{stderr}");

    // Without `--watch-config`, the file changed by hand meanwhile: read
    // again, its rows run, and the edit is made on it.
    let mut host = start(&["run", "rutis.json"], dir, runtime);
    host.wait_lines(&["maker: running", "hand: running"]);
    std::fs::write(
        dir.join("rutis.json"),
        file(&[maker, ("hand", "py:tiny"), ("later", "py:tiny")]),
    )
    .unwrap();
    assert_eq!(
        call(&["maker.add", "\"made\"", "\"py:tiny\""], dir, runtime),
        (Some(0), "\"made\"\n".into(), String::new())
    );
    host.wait_lines(&["later: running", "made: running"]);
    let written = file(&[
        maker,
        ("hand", "py:tiny"),
        ("later", "py:tiny"),
        ("made", "py:tiny"),
    ]);
    assert_eq!(read(dir), written);

    // A file that cannot be written (a directory where the new content
    // goes before it is renamed into place): the row runs, and the plugin
    // is told.
    let partial = dir.join(format!(".rutis.json.{}.tmp", host.pid()));
    std::fs::create_dir(&partial).unwrap();
    let (code, stdout, stderr) = call(&["maker.add", "\"lost\"", "\"py:tiny\""], dir, runtime);
    std::fs::remove_dir(&partial).unwrap();
    assert_eq!((code, stdout.as_str()), (Some(1), ""), "{stderr}");
    assert!(
        stderr.contains(
            "lost: added, but rutis.json could not be written: Is a directory (os error 21)"
        ),
        "{stderr}"
    );
    host.wait_lines(&["lost: running"]);
    assert_eq!(read(dir), written);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, _) = host.exit();
    assert_eq!(status.code(), Some(0), "{stderr}");

    // Started again: the rows written run, the one not written does not.
    let mut host = start(&["run", "rutis.json"], dir, runtime);
    host.wait_lines(&[
        "maker: running",
        "hand: running",
        "later: running",
        "made: running",
    ]);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, lines) = host.exit();
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert!(
        !lines.iter().any(|line| line.starts_with("lost:")),
        "{lines:#?}"
    );
}
