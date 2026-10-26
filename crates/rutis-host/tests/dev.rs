//! `rutis-host dev` following a project as it changes (#665): a changed file
//! reloads only the rows whose file it is, the rows that use them restart
//! by themselves and say why; `rutis.dev.json` is read again; a broken edit
//! leaves the previous version running, also when a row it uses reloads; a
//! runtime that ended starts again with the next change instead of rows
//! said to reload while nothing runs. A Go project rebuilds only for its
//! own sources.
//!
//! The project: its own plugin `weather` (injects `llm`, provides
//! `weather`), a fake `llm` and a `caller` that injects `weather`, in Node
//! and in Python. Each plugin prints `plugin <row> <version> applied ... in
//! <pid>` when it starts.
//!
//! Unix only, as the signal tests: the host is stopped with kill(2).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::process::{file_url, signal, Host};
use common::repo;
use serde_json::json;

/// A language's project: where its files are, and their sources.
struct Lang {
    /// The runtime's name in the lines (`node`, `py`).
    runtime: &'static str,
    /// The project's own plugin, relative to the project.
    own: &'static str,
    /// The fake `llm`'s file, relative to the project.
    llm: &'static str,
    /// The source of `row` at `version`.
    source: fn(row: &str, version: &str) -> String,
    /// A source of the own plugin that does not parse.
    broken: &'static str,
}

const NODE: Lang = Lang {
    runtime: "node",
    own: "src/index.mjs",
    llm: "dev/fake-llm.mjs",
    source: node_source,
    broken: "export default definePlugin({ const const = 1\n",
};

const PYTHON: Lang = Lang {
    runtime: "py",
    own: "src/weather/__init__.py",
    llm: "src/fake_llm.py",
    source: python_source,
    broken: "def broken(:\n",
};

fn node_source(row: &str, version: &str) -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let (inject, provides) = match row {
        "weather" => ("['llm']", "{ weather: { now: 'async' } }"),
        "llm" => ("[]", "{ llm: { ask: 'async' } }"),
        _ => ("['weather']", "{}"),
    };
    let service = match row {
        "caller" => String::new(),
        name => format!(
            "ctx.provide('{name}', {{ {}: async () => 'ok' }})",
            match name {
                "weather" => "now",
                _ => "ask",
            }
        ),
    };
    // Only weather is configured.
    let city = match row {
        "weather" => " city=${config.city}",
        _ => "",
    };
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{\n\
           inject: {inject},\n\
           provides: {provides},\n\
           apply(ctx, config) {{\n\
             console.log(`plugin {row} {version} applied{city} in ${{process.pid}}`)\n\
             {service}\n\
           }},\n\
         }})\n",
        file_url(&sdk)
    )
}

fn python_source(row: &str, version: &str) -> String {
    let (inject, provides, service) = match row {
        "weather" => (
            "[\"llm\"]",
            "{\"weather\": Service}",
            "ctx.provide(\"weather\", Service())",
        ),
        "llm" => (
            "[]",
            "{\"llm\": Service}",
            "ctx.provide(\"llm\", Service())",
        ),
        _ => ("[\"weather\"]", "{}", "pass"),
    };
    // Only weather is configured.
    let city = match row {
        "weather" => " city={config['city']}",
        _ => "",
    };
    format!(
        "import os, sys\n\
         from rutis import define_plugin\n\
         \n\
         class Service:\n\
         \x20   async def now(self):\n\
         \x20       return \"ok\"\n\
         \n\
         def apply(ctx, config):\n\
         \x20   sys.stdout.write(f\"plugin {row} {version} applied{city} in {{os.getpid()}}\\n\")\n\
         \x20   sys.stdout.flush()\n\
         \x20   {service}\n\
         \n\
         plugin = define_plugin(apply, inject={inject}, provides={provides})\n"
    )
}

/// Write `text` to `path`, its modification time later than any before:
/// two edits within one tick of the clock are two changes.
fn edit(path: &Path, text: &str) {
    static LATER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    let ahead = LATER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(ahead))
        .unwrap();
}

/// The project of `lang` in `dir`, with its dev file: `weather` (the
/// project's own row, configured with a city), `llm` and `caller`.
fn project(dir: &Path, lang: &Lang) -> PathBuf {
    let caller = match lang.runtime {
        "node" => "dev/caller.mjs",
        _ => "src/caller.py",
    };
    match lang.runtime {
        "node" => {
            std::fs::write(
                dir.join("package.json"),
                r#"{ "name": "weather", "type": "module" }"#,
            )
            .unwrap();
            // The runtime of this checkout, where `dev` finds it.
            let runtimes = dir.join("node_modules/@arcships");
            std::fs::create_dir_all(&runtimes).unwrap();
            std::os::unix::fs::symlink(
                repo().join("node/rutis-runtime").canonicalize().unwrap(),
                runtimes.join("rutis-runtime"),
            )
            .unwrap();
        }
        _ => std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"weather\"\nversion = \"0.1.0\"\n",
        )
        .unwrap(),
    }
    edit(&dir.join(lang.own), &(lang.source)("weather", "v1"));
    edit(&dir.join(lang.llm), &(lang.source)("llm", "v1"));
    edit(&dir.join(caller), &(lang.source)("caller", "v1"));
    let (llm, caller) = match lang.runtime {
        "node" => (format!("./{}", lang.llm), format!("./{caller}")),
        _ => ("py:fake_llm".to_owned(), "py:caller".to_owned()),
    };
    let dev = json!({
        "rows": [
            { "id": "weather", "config": { "city": "Oslo" } },
            { "id": "llm", "name": llm },
            { "id": "caller", "name": caller },
        ]
    });
    let file = dir.join("rutis.dev.json");
    std::fs::write(&file, serde_json::to_string_pretty(&dev).unwrap()).unwrap();
    file
}

/// Read lines until each of `wants` was read, in any order (the state
/// lines are printed every 200 ms, apart from the rest): a line equal to
/// it, or starting with it when it ends in `*`. The lines read for them,
/// in the order of `wants`.
fn expect(host: &mut Host, wants: &[&str]) -> Vec<String> {
    let matches = |want: &str, line: &str| match want.strip_suffix('*') {
        Some(prefix) => line.starts_with(prefix),
        None => line == want,
    };
    let mut found: Vec<Option<String>> = vec![None; wants.len()];
    while found.contains(&None) {
        let (index, line) = host.wait_line(|line| {
            let index = (0..wants.len())
                .find(|index| found[*index].is_none() && matches(wants[*index], line))?;
            Some((index, line.to_owned()))
        });
        found[index] = Some(line);
    }
    found.into_iter().flatten().collect()
}

/// The process in a plugin's line, `plugin <row> <version> applied ... in
/// <pid>`.
fn pid(line: &str) -> i32 {
    line.rsplit_once(" in ").unwrap().1.trim().parse().unwrap()
}

fn the_dev_loop(lang: &Lang) {
    let dir = tempfile::tempdir().unwrap();
    let dev_file = project(dir.path(), lang);
    let own = dir.path().join(lang.own);
    let runtime = lang.runtime;
    let mut host = Host::start(&["dev", "."], dir.path());
    expect(
        &mut host,
        &["weather: running", "llm: running", "caller: running"],
    );

    // 1. The own plugin's file: it reloads, its user restarts, llm (which
    // it uses) does not.
    edit(&own, &(lang.source)("weather", "v2"));
    let reloaded = format!("weather: reloaded ({} changed)", lang.own);
    expect(
        &mut host,
        &[
            "plugin weather v2 applied city=Oslo in *",
            &reloaded,
            "caller: restarted (weather reloaded)",
        ],
    );

    // 2. The dev file: weather's config changes, and weather restarts with
    // it.
    let text = std::fs::read_to_string(&dev_file).unwrap();
    std::fs::write(&dev_file, text.replace("Oslo", "Tromso")).unwrap();
    expect(
        &mut host,
        &[
            "plugin weather v2 applied city=Tromso in *",
            "rutis-host: reloaded rutis.dev.json (1 row changed)",
            "weather: updated (its row in rutis.dev.json changed)",
            "caller: restarted (weather updated)",
        ],
    );

    // 3. A broken edit: the row that broke says so, and keeps running; a
    // reload of the row it uses restarts it on the version it ran.
    edit(&own, lang.broken);
    let failed = expect(&mut host, &["weather: cannot reload: *"]);
    assert!(
        failed[0].ends_with("; the previous version keeps running"),
        "{failed:?}"
    );
    edit(&dir.path().join(lang.llm), &(lang.source)("llm", "v2"));
    let llm = format!("llm: reloaded ({} changed)", lang.llm);
    let lines = expect(
        &mut host,
        &[
            "plugin llm v2 applied in *",
            "plugin weather v2 applied city=Tromso in *",
            &llm,
            "weather: restarted (llm reloaded)",
        ],
    );

    // 4. The runtime ends (`restart` is `no`): the next change starts it
    // again, with the code as it is now.
    // SAFETY: plain kill(2) of the runtime process this host started.
    unsafe { libc::kill(pid(&lines[1]), libc::SIGKILL) };
    let down = format!("runtime {runtime}: down (exited with signal: 9 (SIGKILL))");
    expect(&mut host, &[&down]);
    edit(&own, &(lang.source)("weather", "v3"));
    let again = format!("runtime {runtime}: starting again ({} changed)", lang.own);
    let running = format!("runtime {runtime}: running");
    expect(
        &mut host,
        &[
            &again,
            "plugin weather v3 applied city=Tromso in *",
            &running,
            "weather: running",
        ],
    );

    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    let count = |text: &str| output.iter().filter(|line| line.starts_with(text)).count();
    // llm reloaded once, for its own file; weather's reloads did not.
    assert_eq!(count("plugin llm v1 applied"), 1, "{context}");
    assert_eq!(count("llm: reloaded"), 1, "{context}");
    // Nothing said to reload while the runtime was down.
    assert_eq!(count("weather: reloaded"), 1, "{context}");
    assert!(!stderr.contains("cannot resolve row"), "{context}");
}

/// A Go project with a Python row its dev file adds (`dev/probe.py`):
/// a change to the probe reloads the probe and builds nothing; a change to
/// the Go sources rebuilds and restarts the Go runtime. A plugin whose
/// apply panicked, fixed and rebuilt, runs again without a word about
/// resolving its row again (the rows the new runtime resolves again start
/// on it).
/// guarantee: host/dev-reload
#[test]
fn dev_rebuilds_a_go_project_only_for_its_sources() {
    let dir = tempfile::tempdir().unwrap();
    rutis_host::new::create(dir.path(), "net-probe", "go").unwrap();
    let project = dir.path().join("net-probe");
    // The SDK of this checkout, not a published one.
    let sdk = repo().join("go/rutis").canonicalize().unwrap();
    let module = std::fs::read_to_string(project.join("go.mod")).unwrap();
    std::fs::write(
        project.join("go.mod"),
        format!(
            "{module}\nreplace github.com/arcships/rutis/go/rutis => {}\n",
            sdk.display()
        ),
    )
    .unwrap();
    let probe = |version: &str| {
        format!(
            "import os, sys\n\
             from rutis import define_plugin\n\
             \n\
             def apply(ctx, config):\n\
             \x20   sys.stdout.write(f\"plugin probe {version} applied in {{os.getpid()}}\\n\")\n\
             \x20   sys.stdout.flush()\n\
             \n\
             plugin = define_plugin(apply, inject=[\"greeter\"])\n"
        )
    };
    let probe_file = project.join("dev/probe.py");
    edit(&probe_file, &probe("v1"));
    let mut py = json!({ "project": "dev" });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let dev = json!({
        "runtimes": { "py": py },
        "rows": [
            { "id": "net-probe", "config": { "greeting": "Hello" } },
            { "id": "probe", "name": "py:probe" },
        ]
    });
    std::fs::write(project.join("rutis.dev.json"), dev.to_string()).unwrap();
    // Builds use the module cache, never the network.
    let env = [
        ("GOPROXY", "off"),
        ("GOTOOLCHAIN", "local"),
        ("GOFLAGS", "-mod=mod"),
    ];
    let mut host = Host::start_in_env(&["dev", "."], &project, &env);
    expect(
        &mut host,
        &[
            "net-probe: built in *",
            "net-probe: running",
            "probe: running",
        ],
    );

    // The probe's file: the probe reloads, nothing is built.
    edit(&probe_file, &probe("v2"));
    expect(
        &mut host,
        &[
            "plugin probe v2 applied in *",
            "probe: reloaded (dev/probe.py changed)",
        ],
    );

    // A panic in apply, rebuilt: the row fails. Fixed and rebuilt: it runs.
    let plugin = project.join("plugin.go");
    let source = std::fs::read_to_string(&plugin).unwrap();
    let panics = source.replace(
        "greeting := config.Greeting",
        "panic(\"boom\"); greeting := config.Greeting",
    );
    assert_ne!(panics, source, "the template's apply");
    edit(&plugin, &panics);
    expect(
        &mut host,
        &[
            "go-net-probe: rebuilt and restarted",
            "net-probe: failed: *",
        ],
    );
    edit(&plugin, &source);
    expect(
        &mut host,
        &["go-net-probe: rebuilt and restarted", "net-probe: running"],
    );

    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    let count = |text: &str| output.iter().filter(|line| line.starts_with(text)).count();
    // The first build, the panic's and the fix's: not one for the probe.
    assert_eq!(count("net-probe: building"), 3, "{context}");
    assert!(!stderr.contains("cannot resolve row"), "{context}");
}

/// guarantee: host/dev-reload, host/dev-failed-reload-keeps-old
#[test]
fn dev_follows_a_node_project() {
    the_dev_loop(&NODE);
}

/// guarantee: host/dev-reload, host/dev-failed-reload-keeps-old
#[test]
fn dev_follows_a_python_project() {
    the_dev_loop(&PYTHON);
}
