//! Cross-version meetings (#209, quality standard Q9.3.2.4, Q6.2.4): the
//! published releases ([`rutis_e2e::previous`]) meet the current code.
//! Which scenarios run depends on the release met: one older than the
//! event protocol of 0.13 (#464, `docs/design-events-protocol-2026-10-18.md`
//! §2; [`previous::EVENTS_V2`]) is refused right after the handshake,
//! before anything loads, with a message that says which side to upgrade,
//! and is not retried; one from 0.13 on meets the current code: services
//! and events work across it. Each scenario is one or the other and says
//! so for a release it does not run for. None hangs (every wait has the
//! hang guard). Risks J2, E13, P8 of `docs/quality-status.md`.
//!
//! Refused, a release older than 0.13:
//!
//! | Old side | Current side | What is checked |
//! | --- | --- | --- |
//! | `@arcships/rutis-runtime` | rutis-host | refused naming the runtime; nothing loads |
//! | `@arcships/rutis-bun` | rutis-host | the same |
//! | PyPI `rutis` (local runtime) | rutis-host | the same |
//! | Go SDK (a binary built against the tagged module) | rutis-host | refused naming the binary's runtime, not started again; its row fails with the refusal |
//! | PyPI `rutis` (`listen:`, remote runtime) | rutis-host dialing it | refused naming it and its version; the link stops and is not dialed again |
//! | rutis-host | the current runtimes (Node, Python) | each runtime refuses the host on stderr and stops |
//! | rutis-host (a node) | rutis-host (a node), both dialing and listening | the link is refused naming the old node and its version, not retried |
//!
//! Met, a release from 0.13 on:
//!
//! | Old side | Current side | What is checked |
//! | --- | --- | --- |
//! | `@arcships/rutis-runtime` | rutis-host, Python runtime | its plugins run and are called across the host, a plugin of the current SDK runs on it; events both ways; tagged values make the round trip |
//! | `@arcships/rutis-bun` | rutis-host, Python runtime | the same in Bun |
//! | PyPI `rutis` (local runtime) | rutis-host, Node runtime | its plugins run and are called; events both ways; tagged values |
//! | PyPI `rutis` (`listen:`, remote runtime) | rutis-host dialing it | the same over the link |
//! | Go SDK (a binary built against the tagged module) | rutis-host, Python runtime | its plugin runs and is called; events both ways; tagged values |
//! | rutis-host | the current runtimes (Node, Python, Go SDK) | their plugins run and call each other through the old host; events both ways through it; tagged values |
//! | rutis-host (a node) | rutis-host (a node), both dialing and listening | services both ways over the link; events both ways (`export` / `import`); tagged values; a JS error with `code` and `data` reaches the old node as a failed call, and the session goes on |
//! | `@arcships/rutis-runtime`, or rutis-host | the current host, or the current Node runtime | a service a plugin of the current SDK provides without declaring it is announced and reaches rows of either runtime; the current `check` warns |
//! | `@arcships/rutis-runtime`, PyPI `rutis` | a Rust application (`fixture-host`) | each row's calls to a Rust service name that row (`caller_row`, its lease in a scope of its own, U+0001 in the label) |
//!
//! "Events both ways": a listener on the old side answers a `serial` the
//! current side emits, and a listener on the current side answers one the
//! old side emits. "Tagged values" are those of 0.11 (#308: a `Date`, an
//! integer beyond ±(2^53−1)).
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

/// A greeter in TypeScript, for the SDK of either version: `hello` names
/// the runtime it runs in, `back` returns what it got. Events: it answers
/// `t/ping/greeter`, and `ring` emits `t/ring/greeter` (`serial`).
const GREETER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

const runtime = (globalThis as { Bun?: unknown }).Bun === undefined ? 'node' : 'bun'

export default definePlugin({
  provides: { greeter: { hello: 'sync', back: 'sync', ring: 'async' } },
  apply(ctx) {
    ctx.on('t/ping/greeter', (n: string) => `pong ${n} (${runtime})`)
    ctx.provide('greeter', {
      hello: (name: string) => `Hello, ${name} (${runtime})`,
      back: (value: unknown) => value,
      ring: async (n: string) => (await ctx.serial('t/ring/greeter', n)) ?? null,
    })
  },
})
"#;

/// A plugin of the current SDK, for a previous runtime to load.
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
/// got. Events: it answers `t/ping/SERVICE`, and `ring` emits
/// `t/ring/SERVICE` (`serial`).
const ECHO_PY: &str = r#"from rutis import define_plugin


class Echo:
    def __init__(self, ctx):
        self.ctx = ctx

    def hello(self, name):
        return f"Hello, {name} (python)"

    def back(self, value):
        return value

    async def ring(self, n):
        return await self.ctx.serial("t/ring/SERVICE", n)


def apply(ctx, config):
    ctx.on("t/ping/SERVICE", lambda n: f"pong {n} (python)")
    ctx.provide("SERVICE", Echo(ctx))


plugin = define_plugin(apply, provides={"SERVICE": Echo})
"#;

/// `sender` in TypeScript (current SDK): sends values to `TARGET.back`
/// and reports what came back. `plain` a string, `date` a `Date`, `big`
/// 2^60 as a bigint. Events: `ping` emits `t/ping/TARGET` (`serial`), and
/// it answers `t/ring/TARGET`.
const SENDER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

type Target = { back(value: unknown): unknown }

export default definePlugin({
  inject: ['TARGET'],
  provides: { sender: { plain: 'async', date: 'async', big: 'async', ping: 'async' } },
  apply(ctx) {
    const target = ctx.use<Target>('TARGET')
    ctx.on('t/ring/TARGET', (n: string) => `ring ${n} (sender)`)
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
      ping: async (n: string) => (await ctx.serial('t/ping/TARGET', n)) ?? null,
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
    def __init__(self, ctx, target):
        self.ctx = ctx
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

    async def ping(self, n):
        return await self.ctx.serial("t/ping/TARGET", n)


def apply(ctx, config):
    ctx.on("t/ring/TARGET", lambda n: f"ring {n} (sender)")
    ctx.provide("sender", Sender(ctx, ctx.use("TARGET")))


plugin = define_plugin(apply, inject=["TARGET"], provides={"sender": Sender})
"#;

/// A Go binary with the plugin `gogreeter`, for any version of the Go SDK
/// (no events: the refused releases have none).
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

/// [`GREETER_GO`] with events, for a Go SDK from 0.13 on: it answers
/// `t/ping/gogreeter`, and `Ring` emits `t/ring/gogreeter` (`Serial`).
const GREETER_GO_EVENTS: &str = r#"package main

import (
	"context"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
)

type Greeter struct{ ctx *rutis.Ctx }

func (*Greeter) Hello(name string) string { return "Hello, " + name + " (go)" }

func (*Greeter) Back(value any) any { return value }

func (g *Greeter) Ring(ctx context.Context, n string) (any, error) {
	return g.ctx.Serial(ctx, "t/ring/gogreeter", n)
}

var plugin = rutis.Define(rutis.Plugin[struct{}]{
	Name:     "gogreeter",
	Inject:   []string{},
	Provides: rutis.Provides{"gogreeter": rutis.MethodsOf[*Greeter](rutis.Sync("Hello", "Back"))},
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.On("t/ping/gogreeter", func(_ context.Context, args ...any) (any, error) {
			return fmt.Sprintf("pong %v (go)", args[0]), nil
		})
		ctx.Provide("gogreeter", &Greeter{ctx: ctx})
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

/// Which releases a scenario is for.
#[derive(Clone, Copy, PartialEq)]
enum Meeting {
    /// Those older than 0.13: refused at the handshake.
    Refused,
    /// Those from 0.13 on: they work with the current code.
    Works,
}

/// The scenario for each release it is for ([`Meeting`]); none offline
/// (outside CI).
fn each(name: &str, meeting: Meeting, scenario: impl Fn(&Release, Scenario)) {
    for release in previous::releases() {
        let works = release.declares_events_v2();
        if works != (meeting == Meeting::Works) {
            eprintln!(
                "cross-version {name}: release {} {}, not this scenario's",
                release.version,
                match works {
                    true => "declares events-v2 and meets the current code",
                    false => "is older than the event protocol and is refused",
                }
            );
            continue;
        }
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

/// Build a Go binary `gogreeter` from `source` into `<out>/gogreeter` in
/// the project, from the module in the project's subdirectory `src`:
/// against the Go SDK of `release` (from the Go proxy), or this checkout's
/// (`None`).
fn go_build(s: &Scenario, release: Option<&Release>, source: &str, src: &str, out: &str) {
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
    s.write(format!("{src}/main.go"), source);
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

// ── Releases older than 0.13: refused (#464, docs/design-events-protocol-2026-10-18.md §2) ──
//
// They lack the capability `events-v2`, so each meeting with one is refused
// right after the handshake, before anything loads, saying which side to
// upgrade; nothing is retried.

/// §2.2: what a 0.13 Rust side says of a far end that does not declare
/// `events-v2`; `who` is how the far end is named.
fn refused_far_end(who: &str) -> String {
    format!(
        "incompatible session: the far end ({who}) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later"
    )
}

/// The same, where the far end named itself (endpoint format): the name
/// and version it reported, which must be `release`'s.
fn refused_named(line: &str, release: &Release) -> bool {
    let (prefix, rest) = ("incompatible session: the far end (", ") does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later");
    line.split_once(prefix)
        .and_then(|(_, after)| after.split_once(rest))
        .is_some_and(|(who, _)| who.ends_with(&release.version))
}

/// A runtime the host starts with did not start: the host says why and
/// ends with a failure status; nothing ran.
fn ended_failing(mut host: Host) {
    let status = host.wait_exit();
    assert!(
        !status.success(),
        "a host whose runtime did not start fails: {status}"
    );
}

/// A previous `@arcships/rutis-runtime` under the current host: refused at
/// the handshake (compat format: the far end does not name itself), the
/// row never runs, the host says which runtime did not start and fails.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-node", Meeting::Refused, |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": { "runtime": release.node_runtime() } },
                "rows": [{ "id": "greeter", "name": "./greeter.ts" }],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        let line = host.expect(&refused_far_end("a runtime older than rutis 0.13"));
        assert!(
            line.contains("the node runtime did not start"),
            "the refusal names the runtime: {line}"
        );
        ended_failing(host);
        // Read after the exit: all the host wrote.
        s.finish();
    });
}

/// A previous `@arcships/rutis-bun` under the current host: refused the
/// same way.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_bun_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-bun", Meeting::Refused, |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "bun": { "runtime": release.bun_runtime() } },
                "rows": [{ "id": "greeter", "name": "bun:./greeter.ts" }],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        let line = host.expect(&refused_far_end("a runtime older than rutis 0.13"));
        assert!(
            line.contains("the bun runtime did not start"),
            "the refusal names the runtime: {line}"
        );
        assert_eq!(host.count("greeter: running"), 0);
        ended_failing(host);
        s.finish();
    });
}

/// A previous PyPI `rutis` as a local runtime: refused the same way.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-python", Meeting::Refused, |release, s| {
        s.write("echo.py", echo_py("echo"));
        s.write_json(
            "rutis.json",
            &json!({ "runtimes": { "py": {} }, "rows": [{ "id": "echo", "name": "py:echo" }] }),
        );
        let old = release.python_path().display().to_string();
        let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_PYTHON_PATH", &old)]);
        let line = host.expect(&refused_far_end("a runtime older than rutis 0.13"));
        assert!(
            line.contains("the py runtime did not start"),
            "the refusal names the runtime: {line}"
        );
        assert_eq!(host.count("echo: running"), 0);
        ended_failing(host);
        s.finish();
    });
}

/// A Go binary built against a previous Go SDK under the current host:
/// refused at the handshake the same way. The binary's runtime starts on
/// demand (one per binary): it is down, naming why, and is not started
/// again; its row fails with the same message (it does not wait for the
/// runtime), and the host runs on.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_go_sdk_binary_is_refused_by_the_event_protocol() {
    each("events-prev-go", Meeting::Refused, |release, s| {
        go_build(&s, Some(release), GREETER_GO, "go-src", "plugins/go");
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "go": { "dir": "plugins/go" } },
                "rows": [{ "id": "gogreeter", "name": "go:gogreeter" }],
            }),
        );
        let mut host = s.host(["run", "rutis.json"]);
        let refusal = refused_far_end("a runtime older than rutis 0.13");
        let line = host.expect(&refusal);
        assert!(
            line.starts_with("runtime go-gogreeter: down ("),
            "the refusal names the binary's runtime: {line}"
        );
        let row = host.expect("gogreeter: unresolved: ");
        assert!(
            row.contains(&refusal),
            "the row fails with the refusal: {row}"
        );
        // Not started again within a restart's first steps.
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert_eq!(host.count(&format!("down ({refusal}")), 1, "started again");
        assert_eq!(host.count("gogreeter: running"), 0);
        stop(host);
        s.finish();
    });
}

/// A previous Python remote runtime the current host dials (endpoint
/// format: it names itself): refused naming it and its version; the link
/// stops, `link stopped: incompatible (…)`, and is not dialed again.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_remote_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-remote", Meeting::Refused, |release, s| {
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
        s.write_json(
            "rutis.json",
            &json!({
                "id": "main",
                "runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
                "rows": [
                    { "id": "gpu", "name": "rutis-bridge/peer", "config": {
                        "peer": "gpu", "dial": format!("ws://127.0.0.1:{port}/rutis"), "runtime": "gpu",
                    } },
                    { "id": "echo", "name": "gpu:echo" },
                ],
            }),
        );
        let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_TOKEN_GPU", &token)]);
        let stopped = host.wait_for("the link stopped as incompatible", |line| {
            line.starts_with("gpu: link stopped: incompatible (")
        });
        assert!(refused_named(&stopped, release), "{stopped}");
        // Not dialed again: no second refusal within a backoff's first steps.
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert_eq!(host.count("gpu: link stopped: incompatible ("), 1);
        assert_eq!(host.count("echo: running"), 0);
        stop(host);
        node.kill();
        node.wait_exit();
        s.finish();
    });
}

/// The current runtimes under a previous host: each (Node, Bun, Python)
/// refuses the host on stderr with the §2.2 line and exits with status 1
/// (the old host then stops: a runtime of it did not start). One runtime
/// per host, so that each one's refusal is seen.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_runtimes_refuse_a_previous_host() {
    let bun = json!({ "runtime": repo().join("bun/rutis-bun") });
    let runtimes = [
        ("node", json!({}), "greeter", "./greeter.ts"),
        ("bun", bun, "greeter", "bun:./greeter.ts"),
        ("py", json!({}), "echo", "py:echo"),
    ];
    for (runtime, config, row, name) in runtimes {
        each(
            &format!("events-prev-host-{runtime}"),
            Meeting::Refused,
            |release, s| {
                s.link_node_sdk();
                s.write("greeter.ts", GREETER_TS);
                s.write("echo.py", echo_py("echo"));
                s.write_json(
                    "rutis.json",
                    &json!({
                        "runtimes": { runtime: config },
                        "rows": [{ "id": row, "name": name }],
                    }),
                );
                let mut host = s.program("", release.host(), ["run", "rutis.json"], &[]);
                host.expect("rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping");
                host.wait_exit();
                assert_eq!(host.count(&format!("{row}: running")), 0);
                s.finish();
            },
        );
    }
}

/// A previous rutis-host node and the current one, either dialing: the
/// current node refuses the link naming the old node and its version. When
/// the current node dials, its link stops and is not dialed again; when it
/// listens, it refuses the old node (`link refused`) however often that
/// one dials. Never connected.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_is_refused_by_the_event_protocol() {
    for old_listens in [true, false] {
        each(
            &format!("events-prev-node-link-{old_listens}"),
            Meeting::Refused,
            |release, s| {
                let token = s.secret();
                let port = s.port();
                let url = format!("ws://127.0.0.1:{port}/rutis");
                let listener = json!([{ "name": "local", "address": format!("127.0.0.1:{port}") }]);
                let link = |peer: &str, listens: bool| {
                    let mut config = json!({ "peer": peer, "export": [], "import": [] });
                    match listens {
                        true => config["listen"] = json!("local"),
                        false => config["dial"] = json!(url),
                    }
                    json!({ "id": peer, "name": "rutis-bridge/peer", "config": config })
                };
                let mut old = json!({ "id": "old", "rows": [link("current", old_listens)] });
                let mut current = json!({ "id": "current", "rows": [link("old", !old_listens)] });
                if old_listens {
                    old["listen"] = listener;
                } else {
                    current["listen"] = listener;
                }
                s.write_json("old/rutis.json", &old);
                s.write_json("current/rutis.json", &current);
                let old = s.program(
                    "old",
                    release.host(),
                    ["run", "rutis.json"],
                    &[("RUTIS_TOKEN_CURRENT", &token)],
                );
                let mut current = s.host_in_with(
                    "current",
                    ["run", "rutis.json"],
                    &[("RUTIS_TOKEN_OLD", &token)],
                );
                // Listening, the link waits for the next dial; dialing, it stops.
                let said = match old_listens {
                    true => "old: link stopped: incompatible (",
                    false => "old: link refused: incompatible (",
                };
                let refused = current.wait_for("the link refused as incompatible", |line| {
                    line.starts_with(said)
                });
                assert!(refused_named(&refused, release), "{refused}");
                std::thread::sleep(std::time::Duration::from_secs(3));
                if old_listens {
                    assert_eq!(current.count(said), 1, "not dialed again");
                }
                assert_eq!(current.count("old: link connected"), 0);
                stop(current);
                stop(old);
                s.finish();
            },
        );
    }
}

// ── Releases from 0.13 on: they meet the current code ──

/// The calls of `sender` (`probe` calls it), whose target is on a side of
/// `release`: a string, a `Date` and a bigint make the round trip (the
/// tagged values of 0.11, which every release met here has).
fn values(host: &mut Host, probe: &Probe) {
    let call =
        |host: &mut Host, method: &str, args: Value| probe.call(host, "sender", method, args);
    assert_eq!(call(host, "plain", json!(["before"])), Ok(json!("before")));
    assert_eq!(call(host, "date", json!([])), Ok(json!(DATE)));
    assert_eq!(call(host, "big", json!([])), Ok(json!(BIG)));
    assert_eq!(call(host, "plain", json!(["after"])), Ok(json!("after")));
}

/// Events both ways between `sender` and its target `target` (whose
/// listener says it runs in `runtime`), `probe` calling them: `sender.ping`
/// emits `t/ping/<target>`, which the target's listener answers;
/// `<target>.ring` emits `t/ring/<target>`, which `sender`'s answers.
fn events(host: &mut Host, probe: &Probe, target: &str, runtime: &str) {
    assert_eq!(
        probe.call(host, "sender", "ping", json!(["a"])),
        Ok(json!(format!("pong a ({runtime})"))),
        "the current side's emission reaches {target}'s listener"
    );
    assert_eq!(
        probe.call(host, target, "ring", json!(["b"])),
        Ok(json!("ring b (sender)")),
        "{target}'s emission reaches the current side's listener"
    );
}

/// The previous `@arcships/rutis-runtime` under the current host. Its
/// plugins (its SDK's) run, and a current Python probe calls them across
/// the host; a current Python plugin sends it the tagged values and emits
/// to its listener, and hears its emission. A plugin of the current SDK
/// runs on it.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_runtime_under_the_current_host() {
    each("prev-node", Meeting::Works, |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        link_sdk(&s, "current", &repo().join("node/rutis"));
        s.write("current/newer.ts", NEWER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        let probe = s.probe("probe", Lang::Python, &["greeter", "sender", "newer"]);
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
        host.expect("newer: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (node)"))
        );
        assert_eq!(
            probe.call(&mut host, "newer", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (newer)"))
        );
        values(&mut host, &probe);
        events(&mut host, &probe, "greeter", "node");
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
    each("prev-bun", Meeting::Works, |release, s| {
        link_sdk(&s, "", &release.node_sdk());
        s.write("greeter.ts", GREETER_TS);
        link_sdk(&s, "current", &repo().join("node/rutis"));
        s.write("current/newer.ts", NEWER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        let probe = s.probe("probe", Lang::Python, &["greeter", "sender", "newer"]);
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
        host.expect("newer: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (bun)"))
        );
        assert_eq!(
            probe.call(&mut host, "newer", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (newer)"))
        );
        values(&mut host, &probe);
        events(&mut host, &probe, "greeter", "bun");
        stop(host);
        s.finish();
    });
}

/// The previous PyPI `rutis` as the current host's Python runtime: its
/// plugin runs, a current Node probe calls it, a current Node plugin sends
/// it the tagged values and emits to its listener, and hears its emission.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_runtime_under_the_current_host() {
    each("prev-python", Meeting::Works, |release, s| {
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
        values(&mut host, &probe);
        events(&mut host, &probe, "echo", "python");
        stop(host);
        s.finish();
    });
}

/// The previous PyPI `rutis` as a remote runtime on another machine
/// (`python -m rutis listen:`, here on loopback) that the current host
/// dials: the same calls and events over the link.
///
/// risk: E13, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_remote_runtime_dialed_by_the_current_host() {
    each("prev-python-listen", Meeting::Works, |release, s| {
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
        values(&mut host, &probe);
        events(&mut host, &probe, "echo", "python");
        stop(host);
        node.kill();
        node.wait_exit();
        s.finish();
    });
}

/// A Go binary built against the previous Go SDK under the current host:
/// its plugin runs, a current Python probe calls it, a current Python
/// plugin sends it the tagged values and emits to its listener, and hears
/// its emission.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_go_sdk_binary_under_the_current_host() {
    each("prev-go", Meeting::Works, |release, s| {
        go_build(&s, Some(release), GREETER_GO_EVENTS, "go-src", "plugins/go");
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
        values(&mut host, &probe);
        events(&mut host, &probe, "gogreeter", "go");
        stop(host);
        s.finish();
    });
}

/// The previous rutis-host with the current runtimes: the Node runtime
/// with a plugin of the current SDK, the Python runtime, a Go binary of
/// the current SDK. Their plugins run and call each other through the old
/// host, the tagged values make the round trip through it, and events
/// cross it both ways.
///
/// risk: J2, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_runtimes_under_a_previous_host() {
    each("prev-host", Meeting::Works, |release, s| {
        s.link_node_sdk();
        s.write("greeter.ts", GREETER_TS);
        s.write("sender.py", SENDER_PY.replace("TARGET", "greeter"));
        go_build(&s, None, GREETER_GO_EVENTS, "go-src", "plugins/go");
        let probe = s.probe("probe", Lang::Python, &["greeter", "gogreeter", "sender"]);
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
        host.expect("gogreeter: running");
        probe.started(&mut host);
        assert_eq!(
            probe.call(&mut host, "greeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (node)"))
        );
        assert_eq!(
            probe.call(&mut host, "gogreeter", "hello", json!(["Ada"])),
            Ok(json!("Hello, Ada (go)"))
        );
        values(&mut host, &probe);
        events(&mut host, &probe, "greeter", "node");
        stop(host);
        s.finish();
    });
}

/// Two nodes: `old` (the previous rutis-host, its Python runtime the
/// previous PyPI `rutis`) and `current` (this checkout's), in the project's
/// subdirectories of those names. `old` exports `echo` and imports
/// `greeter`; `current` the other way round; the events `t/ping/echo`
/// (current to old) and `t/ring/echo` (old to current) are shared the same
/// way. `old_listens`: `old` listens and `current` dials, else the other
/// way round.
///
/// Through the link: a current probe calls `echo` on the old node, an old
/// probe calls `greeter` on the current one; a current plugin sends the
/// tagged values to the old node; events cross both ways.
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
            link(
                "current",
                old_listens,
                &["echo", "t/ring/echo"],
                &["greeter", "thrower", "t/ping/echo"],
            ),
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
            link(
                "old",
                !old_listens,
                &["greeter", "thrower", "t/ping/echo"],
                &["echo", "t/ring/echo"],
            ),
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
    // A JS error with `code` and `data` reaches the old node as that
    // call's failure; the session goes on.
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
    values(&mut current, &probe);
    events(&mut current, &probe, "echo", "python");
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
    each("prev-node-listens", Meeting::Works, |release, s| {
        nodes(release, s, true)
    });
}

/// The current rutis-host node listening, a previous one dialing it.
///
/// risk: E13, P8
/// guarantee: bridge/cross-version-meetings
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_node_listens_and_a_previous_node_dials() {
    each("prev-node-dials", Meeting::Works, |release, s| {
        nodes(release, s, false)
    });
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

/// A service a row provides without declaring it reaches rutis when its
/// runtime announces it (`rows.announce`, 0.12) to a host that asks for
/// it, as every release met here does: `near` and `far` run and `far`
/// calls `quiet` across the host; the current runtime says nothing of an
/// older host. What the current host's `check` says of it (#446): a
/// warning for each row that uses `quiet`, which some row may provide at
/// runtime.
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
    if let Old::Runtime = old {
        let mut check = s.host(["check", "rutis.json"]);
        let status = check.wait_exit();
        let printed: Vec<String> = check.lines().into_iter().map(|line| line.text).collect();
        let has = |text: &str| printed.iter().any(|line| line.contains(text));
        assert!(status.success(), "check: {status}\n{printed:#?}");
        for row in ["near", "far"] {
            assert!(
                has(&format!("rutis-host: warning: {row}: nothing declares quiet; it may be provided at runtime by")),
                "check warns for {row}: {printed:#?}"
            );
        }
    }
    let mut host = match old {
        Old::Runtime => s.host(["run", "rutis.json"]),
        Old::Host => s.program("", release.host(), ["run", "rutis.json"], &[]),
    };
    host.expect("quiet: running");
    far.started(&mut host);
    assert_eq!(
        far.call(&mut host, "quiet", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada (quiet)"))
    );
    host.expect("near: running");
    let told = "does not take undeclared services";
    assert_eq!(host.count(told), 0, "an older host was named");
    stop(host);
    s.finish();
}

/// The previous `@arcships/rutis-runtime` under the current host: a
/// service a current-SDK plugin provides without declaring it is announced
/// and reaches the other rows.
///
/// risk: J2, F8
/// guarantee: bridge/cross-version-meetings, loader/announced-services
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn an_undeclared_service_of_a_previous_runtime_reaches_the_other_rows() {
    each("undeclared-runtime", Meeting::Works, |release, s| {
        undeclared(release, s, Old::Runtime)
    });
}

/// The current Node runtime under the previous rutis-host: the host asks
/// for announcements, and the same service reaches the other rows.
///
/// risk: J2, F8
/// guarantee: bridge/cross-version-meetings, loader/announced-services
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn an_undeclared_service_under_a_previous_host_reaches_the_other_rows() {
    each("undeclared-host", Meeting::Works, |release, s| {
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
    each("prev-caller-row", Meeting::Works, |release, s| {
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
