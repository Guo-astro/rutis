//! S3, cross-language composition and a runtime crash (#187; plan
//! `docs/design-quality-step2-2026-10-11.md` §3.2). One rutis.json: a
//! Python row provides `store`, a TS row `report` uses it, a Bun row
//! `audit` uses `report`, a TS probe uses both. The Python runtime is
//! killed (`kill -9`) while a call into it is in progress; then the host is
//! stopped and started again with the same configuration, which is how a
//! killed runtime comes back by default (maintainer decision Q1 of the
//! plan). With `"restart": "on-failure"` (#274) the host starts it again
//! itself, and its rows come back while the host runs. The residue checks
//! end each (Q5.4.2).
//!
//! Windows: no Bun row. Bun is not declared on Windows (Bun design §10,
//! phase B3) and no Windows CI job installs it.

use std::net::TcpStream;

use rutis_e2e::{repo, Host, Lang, Probe, Scenario};
use serde_json::json;

/// `hold(gate)` prints `store holding <gate>` and answers once the file
/// `gate` exists: a call the scenario keeps in progress for as long as it
/// needs.
const STORE: &str = r#"
import asyncio
import os

from rutis import define_plugin


class Store:
    def __init__(self):
        self.data = {"greeting": "hello from python"}

    async def get(self, key):
        return self.data[key]

    async def hold(self, gate):
        name = os.path.basename(gate)
        print(f"store holding {name}", flush=True)
        while not os.path.exists(gate):
            await asyncio.sleep(0.02)
        return f"released {name}"


def apply(ctx, config):
    ctx.provide("store", Store())


plugin = define_plugin(apply, provides={"store": Store})
"#;

const REPORT: &str = r#"
import { definePlugin } from '@arcships/rutis'

interface Store {
  get(key: string): Promise<string>
  hold(gate: string): Promise<string>
}

export default definePlugin({
  inject: ['store'],
  provides: { report: { summary: 'async', hold: 'async' } },
  apply(ctx) {
    const store = ctx.use<Store>('store')
    ctx.provide('report', {
      summary: async (key: string) => `report: ${await store.get(key)}`,
      hold: async (gate: string) => `report: ${await store.hold(gate)}`,
    })
  },
})
"#;

/// Says which engine runs it: the Bun row is not quietly run by Node.
const AUDIT: &str = r#"
export const inject = ['report']
export const provides = { audit: { check: 'async' } }
export function apply(ctx) {
  const report = ctx.use('report')
  const engine = process.versions.bun ? 'bun' : 'not bun'
  ctx.provide('audit', { check: async key => `audit (${engine}): ${await report.summary(key)}` })
}
"#;

/// Whether the scenario has the Bun row `audit` (see the module's comment).
const BUN: bool = cfg!(not(windows));

/// The project, written once and run by both hosts.
struct Project {
    /// Uses `report` (and `audit`): stops while they wait.
    probe: Probe,
    /// Declares no service, so it keeps running while the others wait; it
    /// looks the service up at each call.
    watch: Probe,
}

fn project(s: &Scenario) -> Project {
    project_with(s, json!({}))
}

/// [`project`], with `py` as the Python runtime's settings.
fn project_with(s: &Scenario, py: serde_json::Value) -> Project {
    s.link_node_sdk();
    s.write("store.py", STORE);
    s.write("report.ts", REPORT);
    let mut rows = vec![
        json!({ "id": "store", "name": "py:store" }),
        json!({ "id": "report", "name": "./report.ts" }),
    ];
    let mut runtimes = json!({ "node": {}, "py": py });
    let mut uses = vec!["report"];
    if BUN {
        s.link(
            "node_modules/@arcships/rutis-bun",
            &repo().join("bun/rutis-bun"),
        );
        s.write("audit.ts", AUDIT);
        rows.push(json!({ "id": "audit", "name": "bun:./audit.ts" }));
        runtimes["bun"] = json!({});
        uses.push("audit");
    }
    let probe = s.probe("probe", Lang::Node, &uses);
    let watch = s.probe("watch", Lang::Node, &[]);
    rows.extend([probe.row(), watch.row()]);
    // Port 0: the first host picks it and reports it (#455).
    s.write_json(
        "rutis.json",
        &json!({
            "id": "e2e",
            "runtimes": runtimes,
            "listen": [{ "name": "local", "address": "127.0.0.1:0" }],
            "rows": rows,
        }),
    );
    Project { probe, watch }
}

/// The rows other than the probes.
fn rows() -> Vec<&'static str> {
    let mut rows = vec!["store", "report"];
    if BUN {
        rows.push("audit");
    }
    rows
}

/// Start `rutis-host run rutis.json` and wait until every row runs and both
/// probes have started.
fn start(s: &Scenario, project: &Project) -> Host {
    let mut host = s.host(["run", "rutis.json"]);
    for row in rows().into_iter().chain(["probe", "watch"]) {
        host.expect(&format!("{row}: running"));
    }
    project.probe.started(&mut host);
    project.watch.started(&mut host);
    host
}

/// The calls that go through every row: `report` through `store`, and
/// `audit` (in Bun) through both.
fn calls_work(host: &mut Host, project: &Project) {
    for probe in [&project.probe, &project.watch] {
        assert_eq!(
            probe.call(host, "report", "summary", json!(["greeting"])),
            Ok(json!("report: hello from python"))
        );
        if BUN {
            assert_eq!(
                probe.call(host, "audit", "check", json!(["greeting"])),
                Ok(json!("audit (bun): report: hello from python"))
            );
        }
    }
}

/// The Python runtime's processes among the host's.
fn python(processes: &[(u32, String)]) -> Vec<u32> {
    processes
        .iter()
        .filter(|(_, command)| command.to_lowercase().contains("python"))
        .map(|(pid, _)| *pid)
        .collect()
}

/// The host's other processes: the Node and Bun runtimes (and what they
/// started), by pid.
fn others(processes: &[(u32, String)]) -> Vec<(u32, String)> {
    let python = python(processes);
    let mut others: Vec<_> = processes
        .iter()
        .filter(|(pid, _)| !python.contains(pid))
        .cloned()
        .collect();
    others.sort();
    others
}

/// Ctrl-C (Ctrl-Break on Windows): every row's cleanup runs, exit code 0.
fn stop(host: &mut Host) {
    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
}

/// risk: C3 (a runtime's crash stops other runtimes or unrelated rows), C4
/// (a call in progress when its runtime crashes hangs instead of failing),
/// B10 (a restarted host cannot start: the last one's sockets or ports are
/// not released).
///
/// guarantee: bridge/in-flight-call-fails-once, core/D30, boundary/5.8 (a
/// call in progress when its runtime's session ends fails explicitly, not
/// a hang; here the session ends with the killed Python runtime; a runtime
/// process that ends takes its services with it).
/// guarantee: host/row-state-lines, host/runtime-state-lines
#[test]
fn a_killed_runtime_fails_its_calls_and_a_restart_recovers() {
    let s = Scenario::new("runtime-crash");
    let project = project(&s);
    let Project { probe, watch } = &project;

    // 1. Every row runs; calls cross Node → Python, and Bun → Node →
    //    Python. A held call answers once its gate opens.
    let mut host = start(&s, &project);
    let port = s.listening(&mut host, "local").port();
    calls_work(&mut host, &project);
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the host does not listen on {port}"
    );
    let gate = s.dir().join("gate-1");
    let held = probe.send("report", "hold", json!([gate]));
    host.expect("store holding gate-1");
    std::fs::write(&gate, "").expect("the gate");
    assert_eq!(
        probe.answer(&mut host, held),
        Ok(json!("report: released gate-1"))
    );

    // 2. `kill -9` the Python runtime while a call is in it.
    let before = s.descendants(&host);
    let killed = python(&before);
    assert!(
        !killed.is_empty(),
        "no Python runtime among the host's processes: {before:?}"
    );
    let gate = s.dir().join("gate-2");
    let held = probe.send("report", "hold", json!([gate]));
    host.expect("store holding gate-2");
    // Every row's state so far is settled: the waits below see only what
    // the crash changes.
    host.skip_written();
    for pid in &killed {
        s.kill(&host, *pid);
    }
    // The call in progress fails, naming why (C4).
    let error = probe.answer(&mut host, held).unwrap_err();
    assert!(error.contains("exited"), "the call in progress: {error}");
    // The host says which runtime went down, and how its process ended.
    let down = host.expect("runtime py: down (");
    assert!(down.contains("exited with"), "{down}");
    // `store` is withdrawn: the rows that use it, and what uses those,
    // wait for it; nothing failed.
    let waiting: Vec<&str> = rows().into_iter().chain(["probe"]).collect();
    for row in &waiting {
        host.expect(&format!("{row}: waiting for its services"));
    }
    probe.event(&mut host, "stopped");
    // Later calls fail, naming the service, instead of hanging.
    for (service, method) in [("store", "get"), ("report", "summary")] {
        let error = watch
            .call(&mut host, service, method, json!(["greeting"]))
            .unwrap_err();
        assert_eq!(error, format!("service {service} is not available"));
    }
    // The other runtimes are the same processes (C3), and the Python
    // runtime is gone.
    let after = s.descendants(&host);
    assert_eq!(others(&after), others(&before), "the other runtimes");
    assert!(
        python(&after).is_empty(),
        "a Python runtime still runs: {after:?}"
    );
    let failed: Vec<String> = host
        .lines()
        .into_iter()
        .map(|line| line.text)
        .filter(|line| line.contains(": failed"))
        .collect();
    assert!(failed.is_empty(), "rows failed: {failed:?}");

    // 3. Stopped, and started again at once with the same configuration,
    //    its listener on the port the first host got (B10: the listener's
    //    port and the runtimes' channels are free).
    stop(&mut host);
    s.edit("rutis.json", |text| {
        text.replace("127.0.0.1:0", &format!("127.0.0.1:{port}"))
    });
    let mut host = start(&s, &project);
    assert_eq!(s.listening(&mut host, "local").port(), port);
    calls_work(&mut host, &project);
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the restarted host does not listen on {port}"
    );

    // 4. Stopped; nothing is left (the residue checks).
    stop(&mut host);
    s.finish();
}

/// `"restart": "on-failure"`: the killed Python runtime is started again
/// by the host, which keeps running; the rows that waited for it run
/// again, calls through every row work again, and the other runtimes are
/// the same processes.
///
/// risk: C3 (a runtime's crash stops other runtimes or unrelated rows)
///
/// guarantee: host/runtime-restart, host/runtime-state-lines
#[test]
fn a_killed_runtime_restarted_on_failure_brings_its_rows_back() {
    let s = Scenario::new("runtime-restart");
    let project = project_with(&s, json!({ "restart": "on-failure" }));
    let mut host = start(&s, &project);
    calls_work(&mut host, &project);
    let pid = host.pid();

    let before = s.descendants(&host);
    let killed = python(&before);
    assert!(
        !killed.is_empty(),
        "no Python runtime among the host's processes: {before:?}"
    );
    host.skip_written();
    for pid in &killed {
        s.kill(&host, *pid);
    }
    let down = host.expect("runtime py: down (");
    assert!(down.contains("exited with"), "how it ended: {down}");
    host.expect("runtime py: restarting");
    host.expect("runtime py: running");
    for row in rows().into_iter().chain(["probe"]) {
        host.expect(&format!("{row}: running"));
    }
    project.probe.started(&mut host);
    calls_work(&mut host, &project);
    assert_eq!(
        host.count("runtime py: restarting"),
        1,
        "one kill, one restart"
    );
    let after = s.descendants(&host);
    assert_eq!(others(&after), others(&before), "the other runtimes");
    let restarted = python(&after);
    assert!(
        !restarted.is_empty() && restarted.iter().all(|pid| !killed.contains(pid)),
        "the Python runtime after the restart: {after:?}"
    );
    assert_eq!(host.pid(), pid, "the host is the same process");

    stop(&mut host);
    s.finish();
}
