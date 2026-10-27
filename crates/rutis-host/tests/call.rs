//! `rutis-host call <service>.<method> [arguments…]` (#670): a running
//! host's service called from the command line, through the socket
//! `rutis-host status` asks. `dev` accepts calls, `run` only with
//! `--allow-call`. What it prints is locked word for word (design
//! try-and-test §八); a service of each language is called once.
//!
//! Unix only: `call` is not available on Windows, as `status`.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::process::{file_url, signal, Host, GUARD};
use common::repo;
use serde_json::json;

/// `rutis-host <args>` in `dir`, with the runtime directory `runtime`.
fn host_command(args: &[&str], dir: &Path, runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command
        .args(args)
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", runtime);
    command
}

/// `rutis-host call <args>` in `dir`: its exit code, stdout and stderr.
fn call(args: &[&str], dir: &Path, runtime: &Path) -> (Option<i32>, String, String) {
    let mut all = vec!["call"];
    all.extend_from_slice(args);
    let Output {
        status,
        stdout,
        stderr,
    } = host_command(&all, dir, runtime).output().unwrap();
    (
        status.code(),
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// `rutis-host status` in `dir`.
fn status(dir: &Path, runtime: &Path) -> String {
    let output = host_command(&["status"], dir, runtime).output().unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Ask `status` until `done` says what it printed will do.
fn status_until(dir: &Path, runtime: &Path, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + rutis_test_support::scaled(GUARD);
    loop {
        let said = status(dir, runtime);
        if done(&said) {
            return said;
        }
        assert!(Instant::now() < deadline, "status: {said}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The SDK of this checkout, as a module a plugin imports.
fn sdk() -> String {
    file_url(
        &repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap(),
    )
}

/// The Node plugin `weather`, providing `forecast`: `today(city)` answers,
/// or throws for `Nowhere`; `week({days})` lists the days; `wait()` does
/// not return.
fn weather() -> String {
    format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         export default definePlugin({{\n\
           inject: ['llm'],\n\
           provides: {{ forecast: {{ today: 'async', week: 'async', wait: 'async' }} }},\n\
           apply(ctx) {{\n\
             ctx.provide('forecast', {{\n\
               async today(city) {{\n\
                 if (city === 'Nowhere') throw new RangeError('no region for Nowhere')\n\
                 return `sunny in ${{city}}`\n\
               }},\n\
               async week(options) {{\n\
                 return {{ days: Array.from({{ length: options.days }}, (_, day) => ({{ day, sky: 'sunny' }})) }}\n\
               }},\n\
               wait() {{ return new Promise(() => {{}}) }},\n\
             }})\n\
           }},\n\
         }})\n",
        sdk = sdk(),
    )
}

/// A fake `llm` in Node.
fn llm() -> String {
    format!(
        "import {{ definePlugin }} from '{}'\n\
         export default definePlugin({{\n\
           provides: {{ llm: {{ ask: 'async' }} }},\n\
           apply(ctx) {{ ctx.provide('llm', {{ async ask() {{ return 'sunny' }} }}) }},\n\
         }})\n",
        sdk()
    )
}

/// A Node plugin project `weather` with a fake `llm`, for `dev`.
fn node_project(dir: &Path) {
    let modules = dir.join("node_modules/@arcships");
    std::fs::create_dir_all(&modules).unwrap();
    std::os::unix::fs::symlink(
        repo().join("node/rutis-runtime").canonicalize().unwrap(),
        modules.join("rutis-runtime"),
    )
    .unwrap();
    common::write(
        dir,
        &[
            ("package.json", r#"{ "name": "weather", "type": "module" }"#),
            ("src/index.mjs", &weather()),
            ("dev/fake-llm.mjs", &llm()),
            (
                "rutis.dev.json",
                &json!({ "rows": [
                    { "id": "weather" },
                    { "id": "llm", "name": "./dev/fake-llm.mjs" },
                ] })
                .to_string(),
            ),
        ],
    );
}

/// In a `dev` project: each line of §八 for `call`, and the call in the
/// snapshot as the host's; Ctrl-C cancels a call; a provider waiting for
/// its runtime is not available.
/// guarantee: host/call
#[test]
fn call_in_a_dev_project_says_what_it_did() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    node_project(dir);
    let mut host = Host::start_in_env(
        &["dev", "."],
        dir,
        &[("XDG_RUNTIME_DIR", runtime.to_str().unwrap())],
    );
    host.wait_line(|line| {
        line.starts_with("rutis-host dev: running weather")
            .then_some(())
    });

    let ok = |args: &[&str], want: &str| {
        let (code, stdout, stderr) = call(args, dir, runtime);
        assert_eq!(
            (code, stdout.as_str(), stderr.as_str()),
            (Some(0), want, ""),
            "{args:?}"
        );
    };
    let fails = |args: &[&str], want: &str| {
        let (code, stdout, stderr) = call(args, dir, runtime);
        assert_eq!(
            (code, stdout.as_str(), stderr.as_str()),
            (Some(1), "", want),
            "{args:?}"
        );
    };
    ok(&["forecast.today", "\"Oslo\""], "\"sunny in Oslo\"\n");
    ok(
        &["forecast.week", "{\"days\": 2}"],
        "{\n  \"days\": [\n    {\n      \"day\": 0,\n      \"sky\": \"sunny\"\n    },\n    {\n      \"day\": 1,\n      \"sky\": \"sunny\"\n    }\n  ]\n}\n",
    );
    fails(
        &["forecast.today", "Oslo"],
        "rutis-host: argument 1 is not JSON: write a string in quotes, as '\"Oslo\"'\n",
    );
    fails(
        &["forecast"],
        "rutis-host: name a method, as forecast.<method>\n",
    );
    fails(
        &["forcast.today", "\"Oslo\""],
        "rutis-host: no row provides forcast; did you mean forecast? (services: forecast, llm)\n",
    );
    fails(
        &["forecast.tomorow", "\"Oslo\""],
        "rutis-host: forecast has no method tomorow (methods: today, wait, week)\n",
    );
    // The method throws: the call, then the row and its frames.
    let (code, stdout, stderr) = call(&["forecast.today", "\"Nowhere\""], dir, runtime);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!((code, stdout.as_str()), (Some(1), ""), "{stderr}");
    assert_eq!(
        lines[..2],
        [
            "rutis-host: call to forecast.today failed: RangeError: no region for Nowhere",
            "    in weather:",
        ],
        "{stderr}"
    );
    assert!(
        lines[2].starts_with("    at ") && lines[2].contains("src/index.mjs:"),
        "{stderr}"
    );

    // A call not returned: the host's in the snapshot, until Ctrl-C ends
    // `call` (130, nothing printed), which cancels it.
    let waiting = host_command(&["call", "forecast.wait"], dir, runtime)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let said = status_until(dir, runtime, |said| said.contains("  host → forecast.wait"));
    let line = said
        .lines()
        .find(|line| line.starts_with("  host → "))
        .unwrap();
    assert!(
        line.starts_with("  host → forecast.wait (row weather)  "),
        "{said}"
    );
    // SAFETY: plain kill(2).
    assert_eq!(unsafe { libc::kill(waiting.id() as i32, libc::SIGINT) }, 0);
    let ended = waiting.wait_with_output().unwrap();
    assert_eq!(ended.status.code(), Some(130));
    assert!(
        ended.stdout.is_empty() && ended.stderr.is_empty(),
        "{ended:?}"
    );
    // The host dropped it (a TypeScript method itself runs on, as any
    // cancelled call's).
    status_until(dir, runtime, |said| said.ends_with("calls:\n  none\n"));

    // The runtime ends: weather waits for it, and its service is not
    // available until the next change starts it again.
    let pid = status(dir, runtime)
        .lines()
        .find_map(|line| line.strip_prefix("  node: running (pid "))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    // SAFETY: plain kill(2).
    unsafe { libc::kill(pid, libc::SIGKILL) };
    status_until(dir, runtime, |said| said.contains("  node: down"));
    fails(
        &["forecast.today", "\"Oslo\""],
        "rutis-host: forecast is not available (row weather waits for its runtime node)\n",
    );

    signal(&host, libc::SIGINT, true);
    let (exit, stderr, _) = host.exit();
    assert_eq!(exit.code(), Some(0), "{stderr}");
    // No host now.
    fails(
        &["forecast.today", "\"Oslo\""],
        &format!(
            "rutis-host: no rutis-host run or dev is running for {} (start one, or name its \
             configuration with --config)\n",
            dir.canonicalize().unwrap().display()
        ),
    );
}

/// `rutis.json` running one row `row` of `name`, with `runtimes`.
fn run_config(dir: &Path, runtimes: serde_json::Value, rows: serde_json::Value) -> PathBuf {
    let file = dir.join("rutis.json");
    std::fs::write(
        &file,
        json!({ "runtimes": runtimes, "rows": rows }).to_string(),
    )
    .unwrap();
    file
}

/// `run` answers `status` but refuses `call`, saying how to accept it;
/// `run --allow-call` accepts it; `--config` names the configuration.
/// guarantee: host/call
#[test]
fn run_accepts_calls_only_when_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    node_project(dir);
    let file = run_config(
        dir,
        json!({ "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") } }),
        json!([
            { "id": "weather", "name": "./src/index.mjs" },
            { "id": "llm", "name": "./dev/fake-llm.mjs" },
        ]),
    );
    let elsewhere = tempfile::tempdir().unwrap();
    let config = file.to_str().unwrap();
    for allow in [false, true] {
        let mut args = vec!["run", "rutis.json"];
        if allow {
            args.push("--allow-call");
        }
        let mut host = Host::start_in_env(
            &args,
            dir,
            &[("XDG_RUNTIME_DIR", runtime.to_str().unwrap())],
        );
        host.wait_line(|line| (line == "weather: running").then_some(()));
        let called = call(
            &["--config", config, "forecast.today", "\"Oslo\""],
            elsewhere.path(),
            runtime,
        );
        match allow {
            false => assert_eq!(
                called,
                (
                    Some(1),
                    String::new(),
                    "rutis-host: this rutis-host run does not accept calls; start it with \
                     rutis-host run --allow-call\n"
                        .into()
                )
            ),
            true => assert_eq!(
                called,
                (Some(0), "\"sunny in Oslo\"\n".into(), String::new())
            ),
        }
        signal(&host, libc::SIGTERM, false);
        let (exit, stderr, _) = host.exit();
        assert_eq!(exit.code(), Some(0), "{stderr}");
    }
}

/// A Python service, called from the command line.
/// guarantee: host/call
#[test]
fn call_reaches_a_python_service() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    common::write(
        dir,
        &[(
            "forecast.py",
            "class Forecast:\n\
             \x20   async def today(self, city: str) -> str:\n\
             \x20       return f\"sunny in {city}\"\n\
             \n\
             provides = {\"forecast\": Forecast}\n\
             \n\
             def apply(ctx, config):\n\
             \x20   ctx.provide(\"forecast\", Forecast())\n",
        )],
    );
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    run_config(
        dir,
        json!({ "py": py }),
        json!([{ "id": "forecast", "name": "py:forecast" }]),
    );
    expect_calls(
        dir,
        runtime,
        &[(&["forecast.today", "\"Oslo\""], "\"sunny in Oslo\"\n", "")],
    );
}

/// A Bun service, called from the command line.
/// guarantee: host/call
#[test]
fn call_reaches_a_bun_service() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    common::write(
        dir,
        &[
            ("package.json", r#"{ "name": "weather", "type": "module" }"#),
            ("bun.lock", ""),
            (
                "forecast.ts",
                &format!(
                    "import {{ definePlugin }} from '{}'\n\
                     export default definePlugin({{\n\
                       provides: {{ forecast: {{ today: 'async' }} }},\n\
                       apply(ctx) {{ ctx.provide('forecast', {{ async today(city: string) {{ return `sunny in ${{city}}` }} }}) }},\n\
                     }})\n",
                    sdk()
                ),
            ),
        ],
    );
    run_config(
        dir,
        json!({ "bun": { "project": ".", "runtime": repo().join("bun/rutis-bun") } }),
        json!([{ "id": "forecast", "name": "bun:./forecast.ts" }]),
    );
    expect_calls(
        dir,
        runtime,
        &[(&["forecast.today", "\"Oslo\""], "\"sunny in Oslo\"\n", "")],
    );
}

/// A Go service (the loader's `ping` fixture), called from the command
/// line; a typed parameter refuses another type.
/// guarantee: host/call
#[test]
fn call_reaches_a_go_service() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(dir.join("plugins/netkit"))
        .arg("./cmd/netkit")
        .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success(), "go build");
    run_config(
        dir,
        json!({ "go": { "dir": "plugins" } }),
        json!([{ "id": "ping", "name": "go:ping" }]),
    );
    expect_calls(
        dir,
        runtime,
        &[
            (&["ping.echo", "\"Oslo\""], "\"netkit pong Oslo\"\n", ""),
            (
                &["ping.echo", "3"],
                "",
                "rutis-host: ping.echo: argument 1 should be a string, got 3\n",
            ),
        ],
    );
}

/// A Swift service (the package's `RutisPing`), called from the command
/// line.
/// guarantee: host/call
#[cfg(target_os = "macos")]
#[test]
fn call_reaches_a_swift_service() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let (dir, runtime) = (dir.path(), runtime.path());
    let status = Command::new("swift")
        .args(["build", "--product", "RutisPing"])
        .current_dir(repo())
        .status()
        .expect("swift is on PATH");
    assert!(status.success(), "swift build");
    let bin = Command::new("swift")
        .args(["build", "--show-bin-path"])
        .current_dir(repo())
        .output()
        .unwrap();
    let bin = PathBuf::from(String::from_utf8(bin.stdout).unwrap().trim());
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::copy(bin.join("RutisPing"), dir.join("plugins/netkit")).unwrap();
    run_config(
        dir,
        json!({ "swift": { "dir": "plugins" } }),
        json!([{ "id": "ping", "name": "swift:ping" }]),
    );
    expect_calls(
        dir,
        runtime,
        &[(&["ping.echo", "\"Oslo\""], "\"netkit pong Oslo\"\n", "")],
    );
}

/// `run --allow-call` the `rutis.json` in `dir`; once its rows run, each
/// call of `calls` prints its stdout and stderr (exit 0 when stderr is
/// empty, else 1).
fn expect_calls(dir: &Path, runtime: &Path, calls: &[(&[&str], &str, &str)]) {
    let mut host = Host::start_in_env(
        &["run", "rutis.json", "--allow-call"],
        dir,
        &[("XDG_RUNTIME_DIR", runtime.to_str().unwrap())],
    );
    let row = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(dir.join("rutis.json")).unwrap(),
    )
    .unwrap()["rows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.wait_line(|line| (line == format!("{row}: running")).then_some(()));
    for (args, stdout, stderr) in calls {
        let code = match stderr.is_empty() {
            true => 0,
            false => 1,
        };
        assert_eq!(
            call(args, dir, runtime),
            (Some(code), stdout.to_string(), stderr.to_string()),
            "{args:?}"
        );
    }
    signal(&host, libc::SIGTERM, false);
    let (exit, stderr, _) = host.exit();
    assert_eq!(exit.code(), Some(0), "{stderr}");
}
