//! The first scenario: one Python row provides a service, one TS row uses
//! it, both started by `rutis-host run` from a rutis.json; probes in both
//! languages call them. Scenario C (quality-status §4.2), its shortest
//! path; the residue checks end it (Q5.4.2).

use rutis_e2e::{Host, Lang, Probe, Scenario};
use serde_json::json;

const STORE: &str = r#"
from rutis import define_plugin


class Store:
    def __init__(self):
        self.data = {"greeting": "hello from python"}

    async def get(self, key):
        if key not in self.data:
            raise KeyError(key)
        return self.data[key]


def apply(ctx, config):
    ctx.provide("store", Store())


plugin = define_plugin(apply, provides={"store": Store})
"#;

const REPORT: &str = r#"
import { definePlugin } from '@arcships/rutis'

interface Store {
  get(key: string): Promise<string>
}

export default definePlugin({
  inject: ['store'],
  provides: { report: { summary: 'async', made: 'sync' } },
  apply(ctx) {
    const store = ctx.use<Store>('store')
    let made = 0
    ctx.provide('report', {
      async summary(key: string) {
        const summary = `report: ${await store.get(key)}`
        made += 1
        return summary
      },
      made: () => made,
    })
  },
})
"#;

/// The project: `store` (Python), `report` (TS, uses `store`), a TS and a
/// Python probe; the host running it, with both probes started.
fn start(s: &Scenario) -> (Host, Probe, Probe) {
    s.link_node_sdk();
    s.write("store.py", STORE);
    s.write("report.ts", REPORT);
    let ts = s.probe("probe-ts", Lang::Node, &["report"]);
    let py = s.probe("probe-py", Lang::Python, &["report"]);
    s.write_json(
        "rutis.json",
        &json!({
            "id": "e2e",
            "runtimes": { "node": {}, "py": {} },
            "rows": [
                { "id": "store", "name": "py:store" },
                { "id": "report", "name": "./report.ts" },
                ts.row(),
                py.row(),
            ],
        }),
    );
    // As the documentation runs it: the configuration named relative to
    // the project (#226).
    let mut host = s.host(["run", "rutis.json"]);
    host.expect("report: running");
    ts.started(&mut host);
    py.started(&mut host);
    (host, ts, py)
}

/// risk: C (cross-language composition), B3 (runtimes outlive a killed host)
/// guarantee: host/killed-host-leaves-no-runtimes
#[test]
fn a_ts_row_uses_a_python_service() {
    let s = Scenario::new("ts-uses-py");
    let (mut host, ts, py) = start(&s);

    // Node → Node (same process) → Python.
    assert_eq!(
        ts.call(&mut host, "report", "summary", json!(["greeting"])),
        Ok(json!("report: hello from python"))
    );
    // A Python exception crosses back as an error, not a hang.
    let error = ts
        .call(&mut host, "report", "summary", json!(["missing"]))
        .unwrap_err();
    assert!(error.contains("missing"), "{error}");
    // Python → Node: the same `report`, which made one summary.
    assert_eq!(
        py.call(&mut host, "report", "made", json!([])),
        Ok(json!(1))
    );

    // Killed, the host cleans nothing up: its runtimes must end by
    // themselves when their channels close (on Windows, rutis-host's job
    // ends them).
    host.kill();
    host.wait_exit();
    s.finish();
}

/// risk: C6, C7 (nested cross-runtime calls). Python → Node → Python: the
/// reply carried a future whose origin held the Python session's ids,
/// tagged with it; the Node runtime refused them, ended its session and
/// exited normally, and the call failed with `BindingError: the process
/// exited normally`. Found by this harness: #225.
/// guarantee: host/stop-on-signal
#[test]
fn a_python_call_through_ts_back_to_python() {
    let s = Scenario::new("py-ts-py");
    let (mut host, _ts, py) = start(&s);
    assert_eq!(
        py.call(&mut host, "report", "summary", json!(["greeting"])),
        Ok(json!("report: hello from python"))
    );
    // Stopped as from a terminal (Ctrl-Break on Windows): every row's
    // cleanup runs, exit code 0.
    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
    s.finish();
}

// ── Services no declaration describes (#442) ────────────────────

/// Node: a service whose methods are a class's, one inherited; no
/// `provides`.
const CLOCK_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

class Zoned {
  zone() { return 'UTC (node)' }
}
class Clock extends Zoned {
  now() { return 'noon (node)' }
}

export default definePlugin({
  apply(ctx) {
    ctx.provide('clock', new Clock())
  },
})
"#;

/// Node: `provides` declares `next` sync, as before.
const COUNTER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { counter: { next: 'sync' } },
  apply(ctx) {
    let n = 0
    ctx.provide('counter', { next: () => ++n })
  },
})
"#;

/// Bun: a module of its own exports, no `provides`.
const BUNNY_TS: &str = r#"export function apply(ctx: { provide(name: string, value: unknown): void }) {
  ctx.provide('bunny', { hop(n: number) { return `hopped ${n} (bun)` } })
}
"#;

/// Python: no `provides`.
const SNAKE_PY: &str = r#"
class Snake:
    def hiss(self, times):
        return "s" * times + " (python)"


def apply(ctx, config):
    ctx.provide("snake", Snake())
"#;

/// Go: no `Provides`.
const GOPHER_GO: &str = r#"package main

import (
	"context"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
)

type Gopher struct{}

func (Gopher) Dig(ctx context.Context, depth int) string { return fmt.Sprintf("dug %d (go)", depth) }

var Plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name: "gopher",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		ctx.Provide("gopher", Gopher{})
		return nil
	},
})

func main() { rutis.Serve(Plugin) }
"#;

/// The Go plugin, built against this checkout's SDK into `plugins/go`.
fn build_gopher(s: &Scenario) {
    let sdk = rutis_e2e::repo().join("go/rutis").display().to_string();
    let sdk = sdk.strip_prefix(r"\\?\").unwrap_or(&sdk).replace('\\', "/");
    s.write(
        "gopher/go.mod",
        format!(
            "module example.com/gopher\n\ngo 1.24\n\nrequire github.com/arcships/rutis/go/rutis v{}\n\nreplace github.com/arcships/rutis/go/rutis => {sdk}\n",
            rutis_e2e::previous::current()
        ),
    );
    s.write("gopher/main.go", GOPHER_GO);
    let binary = s
        .path("plugins/go")
        .join(format!("gopher{}", std::env::consts::EXE_SUFFIX));
    let binary = binary.display().to_string();
    let env = [
        ("GOFLAGS", "-mod=mod"),
        ("GOTOOLCHAIN", "local"),
        ("GOPROXY", "off"),
    ];
    let mut build = s.program("gopher", "go", ["build", "-o", &binary, "."], &env);
    let status = build.wait_exit_within(rutis_e2e::build_guard());
    if !status.success() {
        let output: Vec<String> = build.lines().into_iter().map(|line| line.text).collect();
        panic!("go build: {status}\n  {}", output.join("\n  "));
    }
}

/// Plugins of the four languages that declare no `provides` are called
/// from another language by name, their methods those of the objects they
/// provide (an inherited one too); a method `provides` marks `sync` still
/// is. `check` says what the probes use may be provided at runtime, and
/// passes. (Bun is not declared for Windows, Bun design §10: there the
/// Bun row is left out.)
///
/// risk: C (cross-language composition), A6
/// guarantee: loader/announced-services, host/check
#[test]
fn services_no_declaration_describes_cross_languages() {
    let s = Scenario::new("undeclared");
    s.link_node_sdk();
    s.write("clock.ts", CLOCK_TS);
    s.write("counter.ts", COUNTER_TS);
    s.write("bunny.ts", BUNNY_TS);
    s.write("snake.py", SNAKE_PY);
    build_gopher(&s);
    let bun = !cfg!(windows);
    let mut used = vec!["clock", "counter", "gopher"];
    if bun {
        used.push("bunny");
    }
    let py = s.probe("probe-py", Lang::Python, &used);
    let ts = s.probe("probe-ts", Lang::Node, &["snake"]);
    let mut runtimes = json!({ "node": {}, "py": {}, "go": { "dir": "plugins/go" } });
    let mut rows = vec![
        json!({ "id": "clock", "name": "./clock.ts" }),
        json!({ "id": "counter", "name": "./counter.ts" }),
        json!({ "id": "snake", "name": "py:snake" }),
        json!({ "id": "gopher", "name": "go:gopher" }),
        py.row(),
        ts.row(),
    ];
    if bun {
        runtimes["bun"] = json!({ "runtime": rutis_e2e::repo().join("bun/rutis-bun") });
        rows.push(json!({ "id": "bunny", "name": "bun:./bunny.ts" }));
    }
    s.write_json("rutis.json", &json!({ "runtimes": runtimes, "rows": rows }));

    let mut check = s.host(["check", "rutis.json"]);
    let status = check.wait_exit();
    let printed: Vec<String> = check.lines().into_iter().map(|line| line.text).collect();
    let printed = printed.join("\n");
    assert!(status.success(), "check: {status}\n{printed}");
    assert!(!printed.contains("nothing provides"), "{printed}");
    for service in used.iter().filter(|service| **service != "counter") {
        let line = format!(
            "  {service}: may be provided at runtime by rows that do not declare all they provide ("
        );
        assert!(printed.contains(&line), "check, {service}:\n{printed}");
    }
    // Leaf plugins, no Cordis rows: nothing about resolving in Cordis (#446).
    assert!(!printed.contains("resolves in Cordis"), "{printed}");
    assert!(
        printed.contains("  provides: {\"counter\":{\"next\":\"sync\"}}"),
        "check, what counter declares:\n{printed}"
    );

    let mut host = s.host(["run", "rutis.json"]);
    py.started(&mut host);
    ts.started(&mut host);
    let mut calls = vec![
        (&py, "clock", "now", json!([]), json!("noon (node)")),
        (&py, "clock", "zone", json!([]), json!("UTC (node)")),
        (&py, "counter", "next", json!([]), json!(1)),
        (&py, "gopher", "dig", json!([3]), json!("dug 3 (go)")),
        (&ts, "snake", "hiss", json!([3]), json!("sss (python)")),
    ];
    if bun {
        calls.push((&py, "bunny", "hop", json!([2]), json!("hopped 2 (bun)")));
    }
    for (probe, service, method, args, expected) in calls {
        assert_eq!(
            probe.call(&mut host, service, method, args),
            Ok(expected),
            "{service}.{method}"
        );
    }
    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
    s.finish();
}
