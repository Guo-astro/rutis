//! Cross-version meetings (#209, quality standard Q9.3.2.4, Q6.2.4): the
//! published releases ([`rutis_e2e::previous`]) meet the current code.
//! From 0.13 every published release is older than the event protocol of
//! 0.13 (#464, `docs/design-events-protocol-2026-10-18.md` §2): each
//! meeting is refused right after the handshake, before anything loads,
//! with a message that says which side to upgrade, and is not retried.
//! None hangs (every wait has the hang guard). Risks J2, E13, P8 of
//! `docs/quality-status.md`.
//!
//! | Old side | Current side | What is checked |
//! | --- | --- | --- |
//! | `@arcships/rutis-runtime` | rutis-host | refused naming the runtime; nothing loads |
//! | PyPI `rutis` (local runtime) | rutis-host | the same |
//! | PyPI `rutis` (`listen:`, remote runtime) | rutis-host dialing it | refused naming it and its version; the link stops and is not dialed again |
//! | rutis-host | the current runtimes (Node, Python) | each runtime refuses the host on stderr and stops |
//! | rutis-host (a node) | rutis-host (a node), both dialing and listening | the link is refused naming the old node and its version, not retried |
//!
//! The meetings that worked before 0.13 (calls across the host, new values,
//! undeclared services, leases per row) are gone with it: an old side never
//! gets that far.
//!
//! They download the releases, which change as releases are published, so
//! they do not run on pull requests: `cargo test` skips them, the full CI
//! run (nightly, release gate; `ci.yml`'s `e2e` job) runs them:
//!
//!   cargo test -p rutis-e2e --test cross_version -- --ignored
//!
//! Needs, besides the e2e scenarios' own (Node, Python with pip): the npm
//! registry and PyPI. Offline they pass without running, saying so, except
//! in CI.
//!
//! Unix only (Linux, macOS): on Windows the file compiles to nothing.
#![cfg(unix)]

use rutis_e2e::previous::{self, Release};
use rutis_e2e::{Host, Scenario};
use serde_json::json;

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

// ── The event protocol of 0.13 (#464, docs/design-events-protocol-2026-10-18.md §2) ──
//
// Every published release before 0.13 lacks the capability `events-v2`, so
// each meeting with one is refused right after the handshake, before
// anything loads, saying which side to upgrade; nothing is retried.

/// The first release with the unified events (`events-v2`).
const EVENTS_V2: &str = "0.13.0";

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

/// A previous `@arcships/rutis-runtime` under the current host: refused at
/// the handshake (compat format: the far end does not name itself), the
/// row never runs, the runtime is not restarted for it.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-node", |release, s| {
        assert!(
            release.older_than(EVENTS_V2),
            "{} declares events-v2",
            release.version
        );
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
            line.contains("runtime node"),
            "the refusal names the runtime: {line}"
        );
        assert_eq!(
            host.count("greeter: running"),
            0,
            "nothing loads in a refused runtime"
        );
        stop(host);
        s.finish();
    });
}

/// A previous PyPI `rutis` as a local runtime: refused the same way.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_python_runtime_is_refused_by_the_event_protocol() {
    each("events-prev-python", |release, s| {
        s.write("echo.py", echo_py("echo"));
        s.write_json(
            "rutis.json",
            &json!({ "runtimes": { "py": {} }, "rows": [{ "id": "echo", "name": "py:echo" }] }),
        );
        let old = release.python_path().display().to_string();
        let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_PYTHON_PATH", &old)]);
        host.expect(&refused_far_end("a runtime older than rutis 0.13"));
        assert_eq!(host.count("echo: running"), 0);
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
    each("events-prev-remote", |release, s| {
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

/// The current runtimes under a previous host: each refuses the host on
/// stderr with the §2.2 line and exits with status 1; a dialing one does
/// not dial again.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn the_current_runtimes_refuse_a_previous_host() {
    each("events-prev-host", |release, s| {
        s.link_node_sdk();
        s.write("greeter.ts", GREETER_TS);
        s.write("echo.py", echo_py("echo"));
        s.write_json(
            "rutis.json",
            &json!({
                "runtimes": { "node": {}, "py": {} },
                "rows": [
                    { "id": "greeter", "name": "./greeter.ts" },
                    { "id": "echo", "name": "py:echo" },
                ],
            }),
        );
        let mut host = s.program("", release.host(), ["run", "rutis.json"], &[]);
        let refusal = "rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping";
        // Both runtimes, Node and Python, say it.
        host.expect(refusal);
        host.expect(refusal);
        assert_eq!(
            host.count("greeter: running") + host.count("echo: running"),
            0
        );
        stop(host);
        s.finish();
    });
}

/// A previous rutis-host node and the current one, either dialing: the
/// current node refuses the link naming the old node and its version; the
/// link stops and is not retried.
///
/// guarantee: events/old-peer-refused
#[test]
#[ignore = "downloads the published releases: the full CI run runs it (ci.yml e2e)"]
fn a_previous_node_is_refused_by_the_event_protocol() {
    for old_listens in [true, false] {
        each(
            &format!("events-prev-node-link-{old_listens}"),
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
                let stopped = current.wait_for("the link stopped as incompatible", |line| {
                    line.starts_with("old: link stopped: incompatible (")
                });
                assert!(refused_named(&stopped, release), "{stopped}");
                std::thread::sleep(std::time::Duration::from_secs(3));
                assert_eq!(
                    current.count("old: link stopped: incompatible ("),
                    1,
                    "not retried"
                );
                assert_eq!(current.count("old: link connected"), 0);
                stop(current);
                stop(old);
                s.finish();
            },
        );
    }
}
