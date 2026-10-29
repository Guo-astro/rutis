//! guarantee: sdk/testing-matches-runtime
//! `rutis-host test` (`docs/design-platform-cleanup-2026-10-28.md` §7.5),
//! end to end, as a test tool runs it: `rutis-host test tester -- <runtime
//! command> <project>` starts a real runtime as a local runtime and loads
//! the plugin that drives the test, `tester`; through the service
//! `rutis-host/test` it loads `weather` in the same runtime, hears it on the
//! bus (`on`), asks it (`emit`), updates and unloads it, prints what it saw,
//! and ends its runtime. The host exits 0 once the session ends.
//!
//! Each case runs twice: with the runtime's channel as fd 3, and handed
//! over a loopback address with a one-time token
//! (`RUTIS_LOCAL_HANDOVER=loopback`), as on Windows.
mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::{repo, write};
use rutis_test_support::Guard;
use tokio::io::AsyncBufReadExt;

/// The Python runtime starts cold (#238).
const COLD: Guard = Guard(Duration::from_secs(40));

/// What a runtime prints once the tester is done: `RESULT <json>`. A load
/// the plugin refuses fails with the row's line, as the host prints it.
fn expected(loaded: &str, refused: &str) -> String {
    format!(
        r#"RESULT {{"loaded":"{loaded}","said":["sunny in Oslo","sunny in Bergen"],"after":[],"refused":"{refused}"}}"#
    )
}

/// What the Node tester prints of `load`'s options: a row id given
/// (`oslo`), and given again; `user` sees `forecast`, a copy of it that
/// isolates `forecast` has a scope of its own, without it (it waits).
const OPTIONS: &str = r#"OPTIONS {"named":"oslo","taken":"a row oslo is loaded already","users":["user","user-2"],"saw":["sunny"]}"#;

/// The SDK as a file URL: Node imports a Windows path (`C:\\…`) only as one.
fn sdk() -> String {
    common::process::file_url(
        &repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap(),
    )
}

fn node_plugins(dir: &Path) {
    let weather = format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         export default definePlugin({{\n\
           config: {{ type: 'object', properties: {{ city: {{ type: 'string' }} }}, required: ['city'] }},\n\
           apply(ctx, config) {{\n\
             ctx.on('weather/ask', () => {{ ctx.emit('weather/said', `sunny in ${{config.city}}`) }})\n\
           }},\n\
         }})\n",
        sdk = sdk()
    );
    let forecast = format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         export default definePlugin({{\n\
           provides: {{ forecast: {{ today: 'async' }} }},\n\
           apply(ctx) {{ ctx.provide('forecast', {{ today: () => 'sunny' }}) }},\n\
         }})\n",
        sdk = sdk()
    );
    let user = format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         export default definePlugin({{\n\
           inject: ['forecast'],\n\
           async apply(ctx) {{ ctx.emit('user/saw', await ctx.use('forecast').today()) }},\n\
         }})\n",
        sdk = sdk()
    );
    let tester = format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         const weather = {weather:?}\n\
         const forecast = {forecast:?}\n\
         const user = {user:?}\n\
         const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))\n\
         export default definePlugin({{\n\
           inject: ['rutis-host/test'],\n\
           apply(ctx) {{\n\
             const test = ctx.use('rutis-host/test')\n\
             const said = []\n\
             let heard = () => {{}}\n\
             ctx.on('weather/said', text => {{ said.push(text); heard() }})\n\
             const saw = []\n\
             ctx.on('user/saw', text => {{ saw.push(text) }})\n\
             const ask = () => {{ const answer = new Promise(resolve => {{ heard = resolve }}); ctx.emit('weather/ask'); return answer }}\n\
             setTimeout(async () => {{\n\
               try {{\n\
                 const loaded = await test.load(weather, {{ city: 'Oslo' }})\n\
                 await ask()\n\
                 await test.update(loaded, {{ city: 'Bergen' }})\n\
                 await ask()\n\
                 await test.unload(loaded)\n\
                 const before = said.length\n\
                 ctx.emit('weather/ask')\n\
                 await sleep(300)\n\
                 const after = said.slice(before)\n\
                 const refused = await test.load(weather, {{}}).then(() => 'loaded', error => error.message)\n\
                 console.error('RESULT ' + JSON.stringify({{ loaded, said: said.slice(0, before), after, refused }}))\n\
                 const failed = error => error.message\n\
                 const named = await test.load(weather, {{ city: 'Oslo' }}, {{ id: 'oslo' }})\n\
                 const taken = await test.load(weather, {{ city: 'Oslo' }}, {{ id: 'oslo' }}).catch(failed)\n\
                 await test.load(forecast)\n\
                 const users = [await test.load(user), await test.load(user, null, {{ isolate: {{ forecast: true }} }})]\n\
                 await sleep(300)\n\
                 console.error('OPTIONS ' + JSON.stringify({{ named, taken, users, saw }}))\n\
               }} catch (error) {{ console.error('RESULT failed: ' + error.stack) }}\n\
               process.exit(0)\n\
             }}, 0)\n\
           }},\n\
         }})\n",
        sdk = sdk(),
        weather = dir.join("weather.mjs").to_string_lossy(),
        forecast = dir.join("forecast.mjs").to_string_lossy(),
        user = dir.join("user.mjs").to_string_lossy(),
    );
    write(
        dir,
        &[
            ("weather.mjs", &weather),
            ("forecast.mjs", &forecast),
            ("user.mjs", &user),
            ("tester.mjs", &tester),
            ("package.json", "{}"),
        ],
    );
}

const PY_WEATHER: &str = r#"
from rutis import define_plugin


def apply(ctx, config):
    def ask(*_):
        ctx.emit("weather/said", f"sunny in {config['city']}")

    ctx.on("weather/ask", ask)


plugin = define_plugin(
    apply,
    config={"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]},
)
"#;

const PY_TESTER: &str = r#"
import asyncio
import json
import os
import sys

from rutis import define_plugin


def apply(ctx, config):
    test = ctx.use("rutis-host/test")
    said = []
    heard = []

    def on_said(text):
        said.append(text)
        if heard:
            heard.pop().set()

    ctx.on("weather/said", on_said)

    async def ask():
        event = asyncio.Event()
        heard.append(event)
        ctx.emit("weather/ask")
        await event.wait()

    async def run():
        try:
            loaded = await test.load("weather_plugin", {"city": "Oslo"})
            await ask()
            await test.update(loaded, {"city": "Bergen"})
            await ask()
            await test.unload(loaded)
            before = len(said)
            ctx.emit("weather/ask")
            await asyncio.sleep(0.3)
            after = said[before:]
            try:
                await test.load("weather_plugin", {})
                refused = "loaded"
            except Exception as error:
                refused = str(error)
            result = {"loaded": loaded, "said": said[:before], "after": after, "refused": refused}
            print("RESULT " + json.dumps(result, separators=(",", ":")), file=sys.stderr, flush=True)
        except Exception as error:
            print(f"RESULT failed: {error!r}", file=sys.stderr, flush=True)
        # The test is done: its runtime ends, and the host with it.
        os._exit(0)

    tasks = [asyncio.get_running_loop().create_task(run())]
    ctx.effect(lambda: [task.cancel() for task in tasks])


plugin = define_plugin(apply, inject=["rutis-host/test"])
"#;

fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// How the runtime gets its channel.
#[derive(Clone, Copy, Debug)]
enum Handover {
    Fd,
    Loopback,
}

/// `rutis-host test <tester> -- <runtime…>`: the line its output has that
/// starts with `RESULT`, its exit status, and the rest of its output.
async fn run(tester: &str, runtime: &[&str], handover: Handover) -> (String, i32, Vec<String>) {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command
        .args(["test", tester, "--"])
        .args(runtime)
        .env("PYTHONPATH", repo().join("python/rutis"))
        .env_remove("RUTIS_LOCAL_HANDOVER")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Handover::Loopback = handover {
        command.env("RUTIS_LOCAL_HANDOVER", "loopback");
    }
    let mut host = command.spawn().unwrap();
    let mut lines = tokio::io::BufReader::new(host.stderr.take().unwrap()).lines();
    let said = COLD
        .within("the host's exit", async {
            let mut said = Vec::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                eprintln!("{handover:?}: {line}");
                said.push(line);
            }
            said
        })
        .await;
    let status = COLD.within("the host's exit", host.wait()).await.unwrap();
    let (result, rest): (Vec<String>, Vec<String>) = said
        .into_iter()
        .partition(|line| line.starts_with("RESULT"));
    (
        result.first().cloned().unwrap_or_default(),
        status.code().unwrap_or(-1),
        rest,
    )
}

/// guarantee: host/test-command
#[tokio::test(flavor = "multi_thread")]
async fn node_plugins_load_hear_emit_update_and_unload_through_rutis_host_test() {
    let dir = tempfile::tempdir().unwrap();
    node_plugins(dir.path());
    let runner = repo().join("node/rutis-runtime/src/runner.mjs");
    let anchor = dir.path().join("package.json");
    let tester = dir.path().join("tester.mjs");
    for handover in [Handover::Fd, Handover::Loopback] {
        let (result, status, rest) = run(
            &tester.to_string_lossy(),
            &["node", &runner.to_string_lossy(), &anchor.to_string_lossy()],
            handover,
        )
        .await;
        assert_eq!(
            result,
            expected("weather", "weather: config: city is required"),
            "{handover:?}"
        );
        assert_eq!(status, 0, "{handover:?}: {rest:?}");
        assert_eq!(
            rest,
            [OPTIONS, "runtime test: row tester called process.exit(0)"],
            "{handover:?}"
        );
    }
}

/// guarantee: host/test-command
#[tokio::test(flavor = "multi_thread")]
async fn python_plugins_load_hear_emit_update_and_unload_through_rutis_host_test() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("weather_plugin.py", PY_WEATHER),
            ("tester_plugin.py", PY_TESTER),
        ],
    );
    let python = python();
    for handover in [Handover::Fd, Handover::Loopback] {
        let (result, status, rest) = run(
            "tester_plugin",
            &[&python, "-m", "rutis", &dir.path().to_string_lossy()],
            handover,
        )
        .await;
        assert_eq!(
            result,
            expected(
                "weather_plugin",
                "Error: weather_plugin: config: city is required"
            ),
            "{handover:?}"
        );
        assert_eq!(status, 0, "{handover:?}: {rest:?}");
        assert_eq!(
            rest,
            ["runtime test: row tester_plugin called os._exit(0)"],
            "{handover:?}"
        );
    }
}

/// A command with `fd` as its fd 3.
#[cfg(unix)]
fn with_fd3(command: &mut tokio::process::Command, fd: std::os::fd::RawFd) {
    // SAFETY: between fork and exec only dup2 runs, which is
    // async-signal-safe.
    unsafe {
        command.pre_exec(move || match libc::dup2(fd, 3) {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
}

/// `rutis-host test <tester> --channel <channel>`, the runtime started by
/// the test tool (here: this test) on the other end, as a test tool that
/// runs its language's runtime in its own process does: the host is
/// handed its end as fd 3 (`fd:3`), or dials a loopback address with a
/// one-time token (`tcp:…`). What the runtime prints (`RESULT`, `OPTIONS`),
/// the host's exit status, and what the host printed.
#[cfg(unix)]
async fn given(
    tester: &Path,
    runtime: &[&str],
    handover: Handover,
) -> (Vec<String>, i32, Vec<String>) {
    use std::os::fd::AsRawFd;
    use tokio::io::AsyncReadExt;
    let mut host = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    host.args(["test", &tester.to_string_lossy(), "--channel"])
        .env_remove("RUTIS_LOCAL_HANDOVER")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut runtime_command = tokio::process::Command::new(runtime[0]);
    runtime_command
        .args(&runtime[1..2])
        .arg("fd:3")
        .args(["--id", "test"])
        .args(&runtime[2..])
        .env_remove("RUTIS_LOCAL_HANDOVER")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (mut host, mut runtime) = match handover {
        Handover::Fd => {
            let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
            host.arg("fd:3");
            with_fd3(&mut host, ours.as_raw_fd());
            with_fd3(&mut runtime_command, theirs.as_raw_fd());
            (host.spawn().unwrap(), runtime_command.spawn().unwrap())
        }
        Handover::Loopback => {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            host.arg(format!("tcp:{}", listener.local_addr().unwrap()))
                .env("RUTIS_CHANNEL_TOKEN", "a-one-time-token");
            let host = host.spawn().unwrap();
            let (mut stream, _) = COLD
                .within("the host's connection", listener.accept())
                .await
                .unwrap();
            let mut token = Vec::new();
            loop {
                let byte = stream.read_u8().await.unwrap();
                if byte == b'\n' {
                    break;
                }
                token.push(byte);
            }
            assert_eq!(token, b"a-one-time-token");
            let stream = stream.into_std().unwrap();
            stream.set_nonblocking(false).unwrap();
            with_fd3(&mut runtime_command, stream.as_raw_fd());
            (host, runtime_command.spawn().unwrap())
        }
    };
    let lines = |stderr| async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        let mut said = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            eprintln!("{handover:?}: {line}");
            said.push(line);
        }
        said
    };
    let (printed, said) = COLD
        .within("the runtime's and the host's output", async {
            tokio::join!(
                lines(runtime.stderr.take().unwrap()),
                lines(host.stderr.take().unwrap()),
            )
        })
        .await;
    let status = COLD.within("the host's exit", host.wait()).await.unwrap();
    let _ = runtime.wait().await;
    (printed, status.code().unwrap_or(-1), said)
}

/// guarantee: host/test-command
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_the_test_tool_runs_itself_is_handed_over_with_channel() {
    let dir = tempfile::tempdir().unwrap();
    node_plugins(dir.path());
    let runner = repo().join("node/rutis-runtime/src/runner.mjs");
    let anchor = dir.path().join("package.json");
    let tester = dir.path().join("tester.mjs");
    for handover in [Handover::Fd, Handover::Loopback] {
        let (printed, status, said) = given(
            &tester,
            &["node", &runner.to_string_lossy(), &anchor.to_string_lossy()],
            handover,
        )
        .await;
        assert_eq!(
            printed,
            [
                expected("weather", "weather: config: city is required").as_str(),
                OPTIONS,
                "runtime test: row tester called process.exit(0)"
            ],
            "{handover:?}"
        );
        assert_eq!(status, 0, "{handover:?}: {said:?}");
        assert_eq!(said, Vec::<String>::new(), "{handover:?}");
    }
}

/// A channel that is not there: the host says why and exits 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_channel_that_is_not_there_ends_the_host_with_1() {
    for channel in ["fd:97", "tcp:127.0.0.1:1", "pipe:x"] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
            .args(["test", "tester", "--channel", channel])
            .env("RUTIS_CHANNEL_TOKEN", "t")
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{channel}: {said}");
        assert!(
            said.starts_with(&format!("rutis-host: test: {channel}: ")),
            "{channel}: {said}"
        );
    }
}

/// A runtime that cannot start: the host says why and exits 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_cannot_start_ends_the_host_with_1() {
    let (result, status, rest) =
        run("tester", &["/nonexistent/runtime", "project"], Handover::Fd).await;
    assert_eq!(result, "");
    assert_eq!(status, 1);
    assert_eq!(rest.len(), 1, "{rest:?}");
    assert!(
        rest[0].starts_with("rutis-host: test: ") && rest[0].contains("/nonexistent/runtime"),
        "{rest:?}"
    );
}

/// The command needs the plugin, then `--`, the runtime's command and its
/// project, or `--channel` and a channel.
#[test]
fn the_runtime_command_follows_two_dashes() {
    for args in [
        &["test"][..],
        &["test", "tester"],
        &["test", "tester", "--", "project"],
        &["test", "a", "b", "--", "node", "project"],
        &["test", "tester", "--channel"],
        &["test", "--channel", "fd:3"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "rutis-host: usage: rutis-host test <plugin> -- <runtime command...> <project>\n       rutis-host test <plugin> --channel <channel>\n",
            "{args:?}"
        );
    }
}
