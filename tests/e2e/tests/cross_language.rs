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

    // Killed: `run` has no signal handling yet (#174); its runtimes must
    // end with their channels.
    host.kill();
    host.wait_exit();
    s.finish();
}

/// risk: C6, C7 (nested cross-runtime calls). Python → Node → Python: the
/// reply carried a future whose origin held the Python session's ids,
/// tagged with it; the Node runtime refused them, ended its session and
/// exited normally, and the call failed with `BindingError: the process
/// exited normally`. Found by this harness: #225.
#[test]
fn a_python_call_through_ts_back_to_python() {
    let s = Scenario::new("py-ts-py");
    let (mut host, _ts, py) = start(&s);
    assert_eq!(
        py.call(&mut host, "report", "summary", json!(["greeting"])),
        Ok(json!("report: hello from python"))
    );
    host.kill();
    host.wait_exit();
    s.finish();
}
