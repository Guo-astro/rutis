//! `rutis-host test` (`docs/design-platform-cleanup-2026-10-28.md` §7.5),
//! end to end, as a test tool runs it: the test starts a real runtime and
//! `rutis-host test` on the two ends of one socket pair (the runtime's
//! channel `fd:0`, the host's standard input and output). The plugin that
//! drives the test, `tester`, is loaded first; through the service
//! `rutis-host/test` it loads `weather` at the same runtime, hears it on
//! the bus (`on`), asks it (`emit`), updates and unloads it, and prints
//! what it saw. The host exits 0 once the session ends.
//!
//! Unix only: the runtimes take no channel on a pair of pipes on Windows.
#![cfg(unix)]

mod common;

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
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

fn sdk() -> String {
    repo()
        .join("node/rutis/src/index.mjs")
        .to_string_lossy()
        .into_owned()
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
    let tester = format!(
        "import {{ definePlugin }} from '{sdk}'\n\
         const weather = {weather:?}\n\
         const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))\n\
         export default definePlugin({{\n\
           inject: ['rutis-host/test'],\n\
           apply(ctx) {{\n\
             const test = ctx.use('rutis-host/test')\n\
             const said = []\n\
             let heard = () => {{}}\n\
             ctx.on('weather/said', text => {{ said.push(text); heard() }})\n\
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
               }} catch (error) {{ console.error('RESULT failed: ' + error.stack) }}\n\
             }}, 0)\n\
           }},\n\
         }})\n",
        sdk = sdk(),
        weather = dir.join("weather.mjs").to_string_lossy(),
    );
    write(
        dir,
        &[
            ("weather.mjs", &weather),
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

    tasks = [asyncio.get_running_loop().create_task(run())]
    ctx.effect(lambda: [task.cancel() for task in tasks])


plugin = define_plugin(apply, inject=["rutis-host/test"])
"#;

fn python() -> String {
    std::env::var("RUTIS_PYTHON").unwrap_or_else(|_| "python3".into())
}

/// Run `runtime` (whose channel is `fd:0`) and `rutis-host test <tester>`
/// on one socket pair; the line the runtime printed that starts with
/// `RESULT`, and the host's exit status once the runtime went.
async fn run(mut command: tokio::process::Command, tester: &str) -> (String, i32, String) {
    let (theirs, ours) = UnixStream::pair().unwrap();
    command
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut runtime = command.spawn().unwrap();
    // A command holds its end of the pair until it goes.
    drop(command);
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"));
    command
        .args(["test", tester])
        .stdin(Stdio::from(OwnedFd::from(ours.try_clone().unwrap())))
        .stdout(Stdio::from(OwnedFd::from(ours)))
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut host = command.spawn().unwrap();
    drop(command);
    let host_output = tokio::spawn({
        let mut lines = tokio::io::BufReader::new(host.stderr.take().unwrap()).lines();
        async move {
            let mut said = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("host: {line}");
                said.push_str(&line);
                said.push('\n');
            }
            said
        }
    });
    let mut lines = tokio::io::BufReader::new(runtime.stderr.take().unwrap()).lines();
    let result = COLD
        .within("the tester's result", async {
            loop {
                let line = lines.next_line().await.unwrap().expect("a RESULT line");
                eprintln!("runtime: {line}");
                if line.starts_with("RESULT") {
                    break line;
                }
            }
        })
        .await;
    // The test tool is done: its runtime goes, the session ends.
    runtime.kill().await.unwrap();
    let status = COLD.within("the host's exit", host.wait()).await.unwrap();
    let said = host_output.await.unwrap();
    (result, status.code().unwrap_or(-1), said)
}

/// guarantee: host/test-command
#[tokio::test(flavor = "multi_thread")]
async fn node_plugins_load_hear_emit_update_and_unload_through_rutis_host_test() {
    let dir = tempfile::tempdir().unwrap();
    node_plugins(dir.path());
    let mut runtime = tokio::process::Command::new("node");
    runtime
        .arg(repo().join("node/rutis-runtime/src/runner.mjs"))
        .args(["fd:0", "--id", "node"])
        .arg(dir.path().join("package.json"))
        .current_dir(repo().join("node/rutis-runtime"));
    let tester = dir.path().join("tester.mjs");
    let (result, status, said) = run(runtime, &tester.to_string_lossy()).await;
    assert_eq!(
        result,
        expected("weather", "weather: config: city is required")
    );
    assert_eq!(status, 0, "{said}");
    assert_eq!(said, "", "the host says nothing of a test that went well");
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
    let mut runtime = tokio::process::Command::new(python());
    runtime
        .args(["-m", "rutis", "fd:0", "--id", "py"])
        .arg(dir.path())
        .env("PYTHONPATH", repo().join("python/rutis"))
        .current_dir(dir.path());
    let (result, status, said) = run(runtime, "tester_plugin").await;
    assert_eq!(
        result,
        expected(
            "weather_plugin",
            "Error: weather_plugin: config: city is required"
        )
    );
    assert_eq!(status, 0, "{said}");
    assert_eq!(said, "", "the host says nothing of a test that went well");
}

/// Without a far end that greets, the host says why and exits 1; its
/// standard output carries the session alone (its greeting).
#[tokio::test(flavor = "multi_thread")]
async fn a_far_end_that_goes_before_greeting_ends_the_host_with_1() {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .arg("test")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let output = COLD.within("the host's exit", output).await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    let session = String::from_utf8_lossy(&output.stdout);
    assert!(
        session.starts_with(r#"{"op":"hello","version":4,"endpoint":"host","#)
            && session.lines().count() == 1,
        "{session}"
    );
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.starts_with("rutis-host: test: "), "{said}");
}

/// `--listen`: a listener on the address, for a `rutis-host/listen` row
/// whose token is `RUTIS_TOKEN_TEST`; without it, the host says so and
/// exits 1.
#[tokio::test(flavor = "multi_thread")]
async fn listening_needs_the_token_of_the_row_test() {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .args(["test", "--listen", "127.0.0.1:0"])
        .env_remove("RUTIS_TOKEN_TEST")
        .stdin(Stdio::null())
        .output();
    let output = COLD.within("the host's exit", output).await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    let said = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = said.lines().collect();
    assert!(
        lines[0].starts_with("rutis-host: listening on test 127.0.0.1:"),
        "{said}"
    );
    assert_eq!(
        lines[1..],
        ["rutis-host: test: set RUTIS_TOKEN_TEST: the token the far ends present when they connect"]
    );
}
