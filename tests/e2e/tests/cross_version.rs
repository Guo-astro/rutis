//! Cross-version meetings (#209, quality standard Q9.3.2.4, Q6.2.4): the
//! published releases ([`rutis_e2e::previous`]) meet the current code. Each
//! combination either works, or is refused when it connects or loads with
//! a message that says which side to upgrade; none fails after loading or
//! hangs (every wait has the hang guard). Risks J2, E13, P8 of
//! `docs/quality-status.md`.
//!
//! | Old side | Current side | What is checked |
//! | --- | --- | --- |
//! | `@arcships/rutis-runtime` | rutis-host, Python runtime | its plugins run and are called across the host; a plugin of the current SDK (plugin API 2) is refused at load naming the package to upgrade, or runs on a runtime that has API 2; new values to it fail that call only |
//! | `@arcships/rutis-bun` | rutis-host, Python runtime | the same in Bun |
//! | PyPI `rutis` (local runtime) | rutis-host, Node runtime | its plugins run; new values to it fail that call only |
//! | PyPI `rutis` (`listen:`, remote runtime) | rutis-host dialing it | the same over the link |
//! | Go SDK (a binary built against the tagged module) | rutis-host, Python runtime | its plugins run; new values to it fail that call only |
//! | rutis-host | the current runtimes (Node, Python, Go SDK) | the runtimes' plugins run; a Go binary of plugin API 2 is refused naming the host; new values towards the old host fail that call only |
//! | rutis-host (a node) | rutis-host (a node), both dialing and listening | services both ways over the link; new values to the old node fail that call only; a JS error with `code` and `data` reaches the old node as a failed call, and the session goes on |
//! | `@arcships/rutis-runtime`, or rutis-host | the current host, or the current Node runtime | a service a plugin of the current SDK provides without declaring it stays in its runtime, unknown to the host: the rows injecting it wait, nothing fails |
//! | `@arcships/rutis-runtime`, PyPI `rutis` | a Rust application (`fixture-host`) | each row's calls to a Rust service name that row (`caller_row`, its lease in a scope of its own, U+0001 in the label) |
//!
//! "New values" are the tagged values of 0.11 (#308: a `Date`, an integer
//! beyond ±(2^53−1), …): a side that does not declare `value-<tag>` gets
//! no such value, the call fails with an error naming the value, and the
//! next call works. From 0.11 on they make the round trip.
//!
//! They download the releases, which change as releases are published, so
//! they do not run on pull requests: `cargo test` skips them, the full CI
//! run (nightly, release gate; `ci.yml`'s `e2e` job) runs them:
//!
//!   cargo test -p rutis-e2e --test cross_version -- --ignored
//!
//! Needs, besides the e2e scenarios' own (Node, Python with pip, Bun, Go):
//! the npm registry, PyPI and the Go module proxy. Offline they pass
//! without running, saying so, except in CI.
//!
//! Unix only (Linux, macOS): on Windows the file compiles to nothing.

#![cfg(unix)]

use rutis_e2e::previous::{self, Release};
use rutis_e2e::{build_guard, repo, Host, Lang, Probe, Scenario};
use serde_json::{json, Value};

/// The first release with the tagged values and plugin API 2.
const TAGGED: &str = "0.11.0";

/// A greeter in TypeScript, for the SDK of either version: `hello` names
/// the runtime it runs in, `back` returns what it got.
const GREETER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

const runtime = (globalThis as { Bun?: unknown }).Bun === undefined ? 'node' : 'bun'

export default definePlugin({
  provides: { greeter: { hello: 'sync', back: 'sync' } },
  apply(ctx) {
    ctx.provide('greeter', {
      hello: (name: string) => `Hello, ${name} (${runtime})`,
      back: (value: unknown) => value,
    })
  },
})
"#;

/// A plugin of the current SDK that uses what plugin API 2 added (leaf
/// events).
const NEWER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { newer: { hello: 'sync' } },
  apply(ctx) {
    ctx.on('newer/ping', () => {})
    ctx.provide('newer', { hello: (name: string) => `Hello, ${name} (newer)` })
  },
})
"#;

/// The service `SERVICE` in Python: `hello`, and `back` returning what it
/// got.
const ECHO_PY: &str = r#"from rutis import define_plugin


class Echo:
    def hello(self, name):
        return f"Hello, {name} (python)"

    def back(self, value):
        return value


def apply(ctx, config):
    ctx.provide("SERVICE", Echo())


plugin = define_plugin(apply, provides={"SERVICE": Echo})
"#;

/// `sender` in TypeScript (current SDK): sends values to `TARGET.back`
/// and reports what came back. `plain` a string, `date` a `Date`, `big`
/// 2^60 as a bigint.
const SENDER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

type Target = { back(value: unknown): unknown }

export default definePlugin({
  inject: ['TARGET'],
  provides: { sender: { plain: 'async', date: 'async', big: 'async' } },
  apply(ctx) {
    const target = ctx.use<Target>('TARGET')
    ctx.provide('sender', {
      plain: async (text: string) => await target.back(text),
      async date() {
        const back = await target.back(new Date(Date.UTC(2026, 9, 17)))
        return back instanceof Date ? back.toISOString() : `not a Date: ${typeof back} ${String(back)}`
      },
      async big() {
        const back = await target.back(2n ** 60n)
        return typeof back === 'bigint' ? `${back}n` : `not a bigint: ${typeof back} ${String(back)}`
      },
    })
  },
})
"#;

/// `sender` in Python (current package), as [`SENDER_TS`]: `date` an
/// aware `datetime`, `big` 2^60.
const SENDER_PY: &str = r#"import datetime
import inspect

from rutis import define_plugin


async def back(target, value):
    result = target.back(value)
    if inspect.isawaitable(result):
        result = await result
    return result


class Sender:
    def __init__(self, target):
        self.target = target

    async def plain(self, text):
        return await back(self.target, text)

    async def date(self):
        at = datetime.datetime(2026, 10, 17, tzinfo=datetime.timezone.utc)
        got = await back(self.target, at)
        if isinstance(got, datetime.datetime):
            return got.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z")
        return f"not a datetime: {got!r}"

    async def big(self):
        got = await back(self.target, 2**60)
        return f"{got}n"


def apply(ctx, config):
    ctx.provide("sender", Sender(ctx.use("TARGET")))


plugin = define_plugin(apply, inject=["TARGET"], provides={"sender": Sender})
"#;

/// A Go binary with the plugin `gogreeter`, for the SDK of either version.
const GREETER_GO: &str = r#"package main

import "github.com/arcships/rutis/go/rutis"

type Greeter struct{}

func (*Greeter) Hello(name string) string { return "Hello, " + name + " (go)" }

func (*Greeter) Back(value any) any { return value }

var plugin = rutis.Define(rutis.Plugin[struct{}]{
	Name:     "gogreeter",
	Inject:   []string{},
	Provides: rutis.Provides{"gogreeter": rutis.MethodsOf[*Greeter](rutis.Sync("Hello", "Back"))},
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.Provide("gogreeter", &Greeter{})
		return nil
	},
})

func main() { rutis.Serve(plugin) }
"#;

/// `thrower` in TypeScript (current SDK): `fail` throws an error with a
/// `code` and `data`.
const THROWER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { thrower: { fail: 'async' } },
  apply(ctx) {
    ctx.provide('thrower', {
      async fail() {
        throw Object.assign(new Error('refused by thrower'), {
          code: 'E_REFUSED',
          data: { at: [1, 2], why: 'a test' },
        })
      },
    })
  },
})
"#;

/// What `sender.date` answers when the value made the round trip.
const DATE: &str = "2026-10-17T00:00:00.000Z";
/// What `sender.big` answers when the value made the round trip.
const BIG: &str = "1152921504606846976n";

fn echo_py(service: &str) -> String {
    ECHO_PY.replace("SERVICE", service)
}

/// The scenario for each release; none offline (outside CI).
fn each(name: &str, scenario: impl Fn(&Release, Scenario)) {
    for release in previous::releases() {
        eprintln!("cross-version {name}: release {}", release.version);
        let s = Scenario::new(&format!("{name}-{}", release.version.replace('.', "_")));
        scenario(&release, s);
    }
}

/// Stop a host from a terminal: its cleanups run, exit code 0.
fn stop(mut host: Host) {
    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
}

/// `node_modules/@arcships/rutis` in the project's subdirectory `dir` (or
/// the project): `sdk`, linked, with a package.json beside it.
fn link_sdk(s: &Scenario, dir: &str, sdk: &std::path::Path) {
    let manifest = format!("{dir}/package.json");
    let manifest = manifest.trim_start_matches('/');
    if !s.path(manifest).exists() {
        s.write_json(
            manifest,
            &json!({ "name": "cross-version", "private": true, "type": "module" }),
        );
    }
    let link = format!("{dir}/node_modules/@arcships/rutis");
    s.link(link.trim_start_matches('/'), sdk);
}

/// The calls of `sender` (`probe` calls it), whose target is on a side of
/// `release`: a string makes the round trip; a `Date` and a bigint make it
/// from 0.11 on, and before fail that call only, with an error naming the
/// value; the session goes on.
fn new_values(host: &mut Host, probe: &Probe, release: &Release) {
    let call =
        |host: &mut Host, method: &str, args: Value| probe.call(host, "sender", method, args);
    assert_eq!(call(host, "plain", json!(["before"])), Ok(json!("before")));
    let date = call(host, "date", json!([]));
    let big = call(host, "big", json!([]));
    if release.older_than(TAGGED) {
        let date = date.expect_err("a Date to a side without value-instant");
        eprintln!("cross-version {}: a Date failed: {date}", release.version);
        assert!(
            date.contains("instant") || date.contains("Date"),
            "the error names the value: {date}"
        );
        let big = big.expect_err("2^60 to a side without value-bigint");
        eprintln!("cross-version {}: 2^60 failed: {big}", release.version);
        assert!(
            big.contains("bigint") || big.contains("integer"),
            "the error names the value: {big}"
        );
    } else {
        assert_eq!(date, Ok(json!(DATE)));
        assert_eq!(big, Ok(json!(BIG)));
    }
    // Only those calls failed.
    assert_eq!(call(host, "plain", json!(["after"])), Ok(json!("after")));
}

/// The previous `@arcships/rutis-runtime` under the current host. Its
/// plugins (its SDK's) run, and a current Python probe calls them across
/// the host; a current Python plugin sends it the new values. A plugin of
/// the current SDK is refused at load by a runtime without plugin API 2,
/// naming the package to upgrade, and runs on one that has it.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_runtime_under_the_current_host() {
    each("prev-node", |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        link_sdk(&s, "current", &repo().join("node/rutis"));
        s.write("current/newer.ts", NEWER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        let probe = s.probe("probe", Lang::Python, &["greeter", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": { "runtime": release.node_runtime() }, "py": {} },
                "rows": [
                    { "id": "greeter", "name": "./greeter.ts" },
                    { "id": "newer", "name": s.path("current/newer.ts") },
                    { "id": "sender", "name": "py:sender" },
                    probe.row(),
                ],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        host.expect("greeter: running");
        if release.older_than(TAGGED) {
            let refused = host.expect("newer: unresolved");
            assert!(
                refused.contains("needs plugin API 2")
                    && refused.contains("upgrade @arcships/rutis-runtime"),
                "refused naming what to upgrade: {refused}"
            );
        } else {
            host.expect("newer: running");
        }
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (node)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        s.finish();
    });
}

/// The previous `@arcships/rutis-bun` under the current host: as
/// [`a_previous_node_runtime_under_the_current_host`], in Bun.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_bun_runtime_under_the_current_host() {
    each("prev-bun", |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        link_sdk(&s, "current", &repo().join("node/rutis"));
        s.write("current/newer.ts", NEWER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        let probe = s.probe("probe", Lang::Python, &["greeter", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "bun": { "runtime": release.bun_runtime() }, "py": {} },
                "rows": [
                    { "id": "greeter", "name": "bun:./greeter.ts" },
                    { "id": "newer", "name": "bun:./current/newer.ts" },
                    { "id": "sender", "name": "py:sender" },
                    probe.row(),
                ],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        host.expect("greeter: running");
        if release.older_than(TAGGED) {
            let refused = host.expect("newer: unresolved");
            assert!(
                refused.contains("needs plugin API 2")
                    && refused.contains("upgrade @arcships/rutis-bun"),
                "refused naming what to upgrade: {refused}"
            );
        } else {
            host.expect("newer: running");
        }
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (bun)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        s.finish();
    });
}

/// The previous PyPI `rutis` as the current host's Python runtime: its
/// plugin runs, a current Node probe calls it, a current Node plugin sends
/// it the new values.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_runtime_under_the_current_host() {
    each("prev-python", |release, s| {
        s.link_node_sdk();
        s.write("echo.py", echo_py("echo"));
        s.write("sender.ts", SENDER_TS.replace("TARGET", "echo"));
        let probe = s.probe("probe", Lang::Node, &["echo", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": {}, "py": {} },
                "rows": [
                    { "id": "echo", "name": "py:echo" },
                    { "id": "sender", "name": "./sender.ts" },
                    probe.row(),
                ],
            }),
        );
        let old = release.python_path().display().to_string();
        let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_PYTHON_PATH", &old)]);
        host.expect("echo: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "echo", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (python)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        s.finish();
    });
}

/// The previous PyPI `rutis` as a remote runtime on another machine
/// (`python -m rutis listen:`, here on loopback) that the current host
/// dials: the same calls over the link.
///
/// risk: E13, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_remote_runtime_dialed_by_the_current_host() {
    each("prev-python-listen", |release, s| {
        let token = s.secret();
        let port = s.port();
        s.write("gpu/echo.py", echo_py("echo"));
        let old = release.python_path().display().to_string();
        let mut node = s.program(
            "",
            previous::python(),
            [
                "-m".to_owned(),
                "rutis".into(),
                format!("listen:ws://127.0.0.1:{port}/rutis"),
                "--id".into(),
                "gpu".into(),
                "--peer".into(),
                "main".into(),
                s.path("gpu").display().to_string(),
            ],
            &[("RUTIS_TOKEN", &token), ("PYTHONPATH", &old)],
        );
        node.expect("rutis: listening on");
        s.link_node_sdk();
        s.write("sender.ts", SENDER_TS.replace("TARGET", "echo"));
        let probe = s.probe("probe", Lang::Node, &["echo", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "id": "main",
                "runtimes": {
                    "node": {},
                    "remote": [{ "name": "gpu", "language": "python" }],
                },
                "rows": [
                    {
                        "id": "gpu",
                        "name": "rutis-bridge/peer",
                        "config": {
                            "peer": "gpu",
                            "dial": format!("ws://127.0.0.1:{port}/rutis"),
                            "runtime": "gpu",
                        },
                    },
                    { "id": "echo", "name": "gpu:echo" },
                    { "id": "sender", "name": "./sender.ts" },
                    probe.row(),
                ],
            }),
        );
        let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_TOKEN_GPU", &token)]);
        host.expect("echo: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "echo", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (python)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        node.kill();
        node.wait_exit();
        s.finish();
    });
}

/// Build [`GREETER_GO`] into `<out>/gogreeter` in the project, from the
/// module in the project's subdirectory `src`: against the Go SDK of
/// `release` (from the Go proxy), or this checkout's (`None`).
fn go_build(s: &Scenario, release: Option<&Release>, src: &str, out: &str) {
    let mut module = "module example.com/gogreeter\n\ngo 1.24\n\n".to_owned();
    match release {
        Some(release) => module.push_str(&release.go_require()),
        None => {
            let sdk = repo().join("go/rutis").display().to_string();
            module.push_str(&format!(
                "require github.com/arcships/rutis/go/rutis v{}\n\nreplace github.com/arcships/rutis/go/rutis => {sdk}",
                previous::current()
            ));
        }
    }
    module.push('\n');
    s.write(format!("{src}/go.mod"), module);
    s.write(format!("{src}/main.go"), GREETER_GO);
    let binary = s.path(out).join("gogreeter").display().to_string();
    let mut env = vec![("GOFLAGS", "-mod=mod"), ("GOTOOLCHAIN", "local")];
    if release.is_none() {
        env.push(("GOPROXY", "off"));
    }
    let mut build = s.program(src, "go", ["build", "-o", &binary, "."], &env);
    let status = build.wait_exit_within(build_guard());
    if !status.success() {
        let output: Vec<String> = build.lines().into_iter().map(|line| line.text).collect();
        panic!("go build: {status}\n  {}", output.join("\n  "));
    }
}

/// A Go binary built against the previous Go SDK under the current host:
/// its plugin runs, a current Python probe calls it, a current Python
/// plugin sends it the new values.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_go_sdk_binary_under_the_current_host() {
    each("prev-go", |release, s| {
        go_build(&s, Some(release), "go-src", "plugins/go");
        s.write("sender.py", SENDER_PY.replace("TARGET", "gogreeter"));
        let probe = s.probe("probe", Lang::Python, &["gogreeter", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "py": {}, "go": { "dir": "plugins/go" } },
                "rows": [
                    { "id": "gogreeter", "name": "go:gogreeter" },
                    { "id": "sender", "name": "py:sender" },
                    probe.row(),
                ],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        host.expect("gogreeter: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "gogreeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (go)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        s.finish();
    });
}

/// The previous rutis-host with the current runtimes: the Node runtime
/// with a plugin of the current SDK, the Python runtime, a Go binary of
/// the current SDK. The Node and Python plugins run and call each other; a
/// host without plugin API 2 refuses the Go binary at load, naming the
/// host to upgrade; the new values towards the old host fail that call
/// only.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_runtimes_under_a_previous_host() {
    each("prev-host", |release, s| {
        s.link_node_sdk();
        s.write("greeter.ts", GREETER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        go_build(&s, None, "go-src", "plugins/go");
        let probe = s.probe("probe", Lang::Python, &["greeter", "sender"]);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": {}, "py": {}, "go": { "dir": "plugins/go" } },
                "rows": [
                    { "id": "greeter", "name": "./greeter.ts" },
                    { "id": "gogreeter", "name": "go:gogreeter" },
                    { "id": "sender", "name": "py:sender" },
                    probe.row(),
                ],
            }),
        );
        // The checkout's runtimes, as for any host of a scenario.
        let mut host = s.program("", release.host(), ["run", "rutis.json"], &[]);
        host.expect("greeter: running");
        if release.older_than(TAGGED) {
            let refused = host.expect("gogreeter: unresolved");
            assert!(
                refused.contains("needs plugin API 2") && refused.contains("upgrade the host"),
                "refused naming what to upgrade: {refused}"
            );
        } else {
            host.expect("gogreeter: running");
        }
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (node)"))
        );
        new_values(&mut host, &probe, release);
        stop(host);
        s.finish();
    });
}

/// Two nodes: `old` (the previous rutis-host, its Python runtime the
/// previous PyPI `rutis`) and `current` (this checkout's), in the project's
/// subdirectories of those names. `old` exports `echo` and imports
/// `greeter`; `current` the other way round. `old_listens`: `old` listens
/// and `current` dials, else the other way round.
///
/// Through the link: a current probe calls `echo` on the old node, an old
/// probe calls `greeter` on the current one; a current plugin sends the
/// new values to the old node.
fn nodes(release: &Release, s: Scenario, old_listens: bool) {
    let token = s.secret();
    let port = s.port();
    let url = format!("ws://127.0.0.1:{port}/rutis");
    let link = |peer: &str, listens: bool, export: &[&str], import: &[&str]| {
        let mut config = json!({ "peer": peer, "export": export, "import": import });
        match listens {
            true => config["listen"] = json!("local"),
            false => config["dial"] = json!(url),
        }
        json!({ "id": peer, "name": "rutis-bridge/peer", "config": config })
    };
    let listener = json!([{ "name": "local", "address": format!("127.0.0.1:{port}") }]);

    s.write("old/echo.py", echo_py("echo"));
    let old_probe = s.probe_in("old", "probe-old", Lang::Python, &["greeter", "thrower"]);
    let mut config = json!({
        "id": "old",
        "runtimes": { "py": {} },
            "rows": [
                { "id": "echo", "name": "py:echo" },
                link("current", old_listens, &["echo"], &["greeter", "thrower"]),
            old_probe.row(),
        ],
    });
    if old_listens {
        config["listen"] = listener.clone();
    }
    s.write_json("old/rutis.json", &config);

    s.link_node_sdk();
    s.write_json(
        "current/package.json",
        &json!({ "name": "current", "private": true, "type": "module" }),
    );
    s.write("current/greeter.py", echo_py("greeter"));
    s.write("current/sender.ts", SENDER_TS.replace("TARGET", "echo"));
    s.write("current/thrower.ts", THROWER_TS);
    let probe = s.probe_in("current", "probe", Lang::Node, &["echo", "sender"]);
    let mut config = json!({
        "id": "current",
        "runtimes": { "node": {}, "py": {} },
            "rows": [
                { "id": "greeter", "name": "py:greeter" },
                { "id": "sender", "name": "./sender.ts" },
                { "id": "thrower", "name": "./thrower.ts" },
                link("old", !old_listens, &["greeter", "thrower"], &["echo"]),
            probe.row(),
        ],
    });
    if !old_listens {
        config["listen"] = listener;
    }
    s.write_json("current/rutis.json", &config);

    let python = release.python_path().display().to_string();
    let mut old = s.program(
        "old",
        release.host(),
        ["run", "rutis.json"],
        &[
            ("RUTIS_TOKEN_CURRENT", &token),
            ("RUTIS_PYTHON_PATH", &python),
            (
                "RUTIS_NODE_RUNTIME",
                &release.node_runtime().display().to_string(),
            ),
        ],
    );
    old.expect("echo: running");
    let mut current = s.host_in_with(
        "current",
        ["run", "rutis.json"],
        &[("RUTIS_TOKEN_OLD", &token)],
    );
    current.expect("old: link connected");
    old.expect("current: link connected");
    probe.started(&mut current);
    old_probe.started(&mut old);
    assert_eq!(
        probe.call(&mut current, "echo", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada (python)"))
    );
    assert_eq!(
        old_probe.call(&mut old, "greeter", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada (python)"))
    );
    // A JS error with `code` and `data` (what 0.12 lets an error carry)
    // reaches the old node as that call's failure; the session goes on.
    let mark = old.lines().len();
    for _ in 0..2 {
        let failed = old_probe
            .call(&mut old, "thrower", "fail", json!([]))
            .expect_err("the thrown error fails the call");
        assert!(failed.contains("refused by thrower"), "{failed}");
        assert_eq!(
            old_probe.call(&mut old, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (python)"))
        );
    }
    let down: Vec<String> = old
        .lines()
        .into_iter()
        .skip(mark)
        .map(|line| line.text)
        .filter(|line| line.starts_with("current: link down"))
        .collect();
    assert!(down.is_empty(), "the link went down: {down:?}");
    new_values(&mut current, &probe, release);
    stop(current);
    stop(old);
    s.finish();
}

/// A previous rutis-host node listening, the current one dialing it.
///
/// risk: E13, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_listens_and_the_current_node_dials() {
    each("prev-node-listens", |release, s| nodes(release, s, true));
}

/// The current rutis-host node listening, a previous one dialing it.
///
/// risk: E13, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_node_listens_and_a_previous_node_dials() {
    each("prev-node-dials", |release, s| nodes(release, s, false));
}

/// A plugin of the current SDK that provides `quiet` without declaring
/// it (`provides` left out, as the 0.12 templates do), and a row of the
/// same runtime that uses it and offers `near` (declared).
const QUIET_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    ctx.provide('quiet', { hello: (name: string) => `Hello, ${name} (quiet)` })
  },
})
"#;

const NEAR_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

type Quiet = { hello(name: string): string }

export default definePlugin({
  inject: ['quiet'],
  provides: { near: { hello: 'async' } },
  apply(ctx) {
    const quiet = ctx.use<Quiet>('quiet')
    ctx.provide('near', { hello: async (name: string) => await quiet.hello(name) })
  },
})
"#;

/// Which side of [`undeclared`] is the previous release.
#[derive(Clone, Copy)]
enum Old {
    /// `@arcships/rutis-runtime`, under the current host.
    Runtime,
    /// rutis-host, with the current Node runtime.
    Host,
}

/// A service a row provides without declaring it reaches rutis only when
/// its runtime announces it (`rows.announce`, 0.12) to a host that asks
/// for it. With `old` of `release`, nothing announces it: the service
/// stays in its runtime, unknown to the host, and every row injecting it,
/// in that runtime (`near`) or another (`far`), waits for it; nothing
/// fails, and the row providing it runs on.
fn undeclared(release: &Release, s: Scenario, old: Old) {
    link_sdk(&s, "", &repo().join("node/rutis"));
    s.write("quiet.ts", QUIET_TS);
    s.write("near.ts", NEAR_TS);
    let far = s.probe("far", Lang::Python, &["quiet"]);
    let node = match old {
        Old::Runtime => json!({ "runtime": release.node_runtime() }),
        Old::Host => json!({}),
    };
    s.write_json(
        "rutis.json",
        &json!({
            "runtimes": { "node": node, "py": {} },
            "rows": [
                { "id": "quiet", "name": "./quiet.ts" },
                { "id": "near", "name": "./near.ts" },
                far.row(),
            ],
        }),
    );
    let mut host = match old {
        Old::Runtime => s.host(["run", "rutis.json"]),
        Old::Host => s.program("", release.host(), ["run", "rutis.json"], &[]),
    };
    host.expect("quiet: running");
    host.expect("near: waiting for its services");
    host.expect("far: waiting for its services");
    // Nothing is to come; a while for an announcement to have arrived if
    // there were one (the current runtime sends it as the row loads).
    std::thread::sleep(std::time::Duration::from_secs(2));
    for row in ["near", "far"] {
        assert_eq!(
            host.count(&format!("{row}: running")),
            0,
            "quiet reached {row}"
        );
    }
    let failed: Vec<String> = host
        .lines()
        .into_iter()
        .map(|line| line.text)
        .filter(|line| line.contains(": failed") || line.contains(": unresolved"))
        .collect();
    assert!(failed.is_empty(), "rows failed: {failed:?}");
    stop(host);
    s.finish();
}

/// The previous `@arcships/rutis-runtime` under the current host: a
/// service a current-SDK plugin provides without declaring it stays in the
/// runtime (it announces nothing). Before plugin API 2 the plugin does not
/// load at all ([`a_previous_node_runtime_under_the_current_host`]).
///
/// risk: J2, F8
/// guarantee: bridge/cross-version-meetings, loader/announced-services
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn an_undeclared_service_of_a_previous_runtime_stays_in_it() {
    each("undeclared-runtime", |release, s| {
        if release.older_than(TAGGED) {
            eprintln!(
                "cross-version undeclared-runtime: {} has no plugin API 2, nothing to meet",
                release.version
            );
            s.finish();
            return;
        }
        undeclared(release, s, Old::Runtime);
    });
}

/// The current Node runtime under the previous rutis-host: the host does
/// not ask for announcements, so the same service stays in the runtime.
///
/// risk: J2, F8
/// guarantee: bridge/cross-version-meetings, loader/announced-services
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn an_undeclared_service_under_a_previous_host_stays_in_its_runtime() {
    each("undeclared-host", |release, s| {
        undeclared(release, s, Old::Host)
    });
}

/// Previous runtimes (Node and Python) under a current Rust application
/// (`fixture-host`) whose Rust service `calendar` tells who calls it
/// (`caller()`, `session::caller_row`). Two rows of one plugin in each
/// runtime: each row's calls name that row, through a lease in a scope
/// of its own (a label with U+0001 in it, which the previous runtimes
/// take as any label).
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings, bridge/caller-row
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn previous_runtimes_lease_per_row_and_name_the_calling_row() {
    each("prev-caller-row", |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        let probes = [
            s.probe("node-a", Lang::Node, &["calendar"]),
            s.probe("node-b", Lang::Node, &["calendar"]),
            s.probe("py-a", Lang::Python, &["calendar"]),
            s.probe("py-b", Lang::Python, &["calendar"]),
        ];
        let mut rows = vec![json!({ "id": "calendar", "name": "fixture/calendar" })];
        rows.extend(probes.iter().map(Probe::row));
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": { "runtime": release.node_runtime() }, "py": {} },
                "rows": rows,
            }),
        );
        let old = release.python_path().display().to_string();
        let mut host = s.fixture_host(["rutis.json"], &[("RUTIS_PYTHON_PATH", &old)]);
        for probe in &probes {
            probe.started(&mut host);
        }
        let mut keys = std::collections::BTreeSet::new();
        for (probe, id) in probes.iter().zip(["node-a", "node-b", "py-a", "py-b"]) {
            for _ in 0..2 {
                let caller = probe
                    .call(&mut host, "calendar", "caller", json!([]))
                    .unwrap_or_else(|error| panic!("{id}: {error}"));
                assert_eq!(caller["id"], id, "{id}'s call names it: {caller}");
                // No `isolate` of its own: the label the service is used
                // under is none (the lease's own label stays inside rutis).
                assert_eq!(caller["scope"], Value::Null, "{id}: {caller}");
                keys.insert(caller["key"].as_str().unwrap_or_default().to_owned());
            }
        }
        // One key per row: four rows, four callers.
        assert_eq!(keys.len(), 4, "{keys:?}");
        host.send_line("stop");
        assert!(host.wait_exit().success(), "the application stopped");
        s.finish();
    });
}
