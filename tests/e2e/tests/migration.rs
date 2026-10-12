//! S4, an implementation moving (#188; plan
//! `docs/design-quality-step2-2026-10-11.md` §3.5, design philosophy §4):
//! the TS row `planner` uses the service `calendar`, whose implementation
//! moves from a Python row, to a plugin on another node, to Rust, while
//! `planner`'s row stays the same and the host keeps running.
//!
//! Host A is the fixture Rust host (`tests/e2e/fixture-host`): `rutis-host
//! run` does not watch its configuration, and only a Rust program has
//! Rust rows (maintainer decision Q1 of the plan). It reads `rutis.json`
//! with rutis-host's library, has the row `fixture/calendar` compiled in,
//! and on the stdin command `reload` applies the file again as one loader
//! edit. Node B is `rutis-host run`, listening on loopback (WebSocket, a
//! token), with its own `calendar` row that it exports; A dials it through
//! the harness's TCP proxy, whose connections show when A's link closes.
//!
//! | stage | A's `calendar` | the result |
//! | --- | --- | --- |
//! | 1 | the Python row `py:py_calendar` | `python` |
//! | 2 | imported from B over a link (the Python row and runtime removed) | `node B` |
//! | 3 | the Rust row `fixture/calendar` (the link removed) | `rust` |
//!
//! A probe that declares no service (it keeps running while `planner`
//! restarts) calls `planner.plan` without pause across each switch.

use std::time::Instant;

use rutis_e2e::{hang_guard, Host, Lang, Probe, Scenario};
use serde_json::{json, Value};

/// The service that does not move: its row is the same at every stage.
const PLANNER: &str = r#"
import { definePlugin } from '@arcships/rutis'

interface Calendar {
  today(): Promise<string>
}

export default definePlugin({
  inject: ['calendar'],
  provides: { planner: { plan: 'async' } },
  apply(ctx) {
    const calendar = ctx.use<Calendar>('calendar')
    ctx.provide('planner', { plan: async () => `plan on ${await calendar.today()}` })
  },
})
"#;

/// Stage 1's `calendar`, a module named so that it does not hide Python's
/// own `calendar`.
const PY_CALENDAR: &str = r#"
provides = {"calendar": {"today": "async"}}


class Calendar:
    async def today(self):
        return "python"


def apply(ctx, config):
    ctx.provide("calendar", Calendar())
"#;

/// Stage 2's `calendar`, on B: each call prints a line B's output counts.
const B_CALENDAR: &str = r#"
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { calendar: { today: 'async' } },
  apply(ctx) {
    ctx.provide('calendar', {
      today: async () => {
        console.log('calendar on B: today')
        return 'node B'
      },
    })
  },
})
"#;

/// Where A's `calendar` comes from.
enum Calendar<'a> {
    Python,
    /// Imported over a link dialing this address.
    Node(&'a str),
    Rust,
}

/// Write A's `rutis.json` with `calendar` from `calendar`; the `planner`
/// row (and the probe's) is the same at every stage.
fn a_config(s: &Scenario, calendar: Calendar, watch: &Probe) {
    let planner = json!({ "id": "planner", "name": "./planner.ts" });
    let (runtimes, row) = match calendar {
        Calendar::Python => (
            json!({ "node": {}, "py": {} }),
            json!({ "id": "calendar", "name": "py:py_calendar" }),
        ),
        Calendar::Node(dial) => (
            json!({ "node": {} }),
            json!({
                "id": "b",
                "name": "rutis-bridge/peer",
                "config": { "peer": "b", "dial": dial, "import": ["calendar"] },
            }),
        ),
        Calendar::Rust => (
            json!({ "node": {} }),
            json!({ "id": "calendar", "name": "fixture/calendar" }),
        ),
    };
    let config = json!({
        "id": "a",
        "runtimes": runtimes,
        "rows": [planner, row, watch.row()],
    });
    s.replace("rutis.json", serde_json::to_string_pretty(&config).unwrap());
}

/// Node B: `rutis-host run b.json`, listening on `port` for A with
/// `token`, exporting its `calendar`. Returns once its rows run (its
/// listener is bound before they load).
fn node_b(s: &Scenario, port: u16, token: &str) -> Host {
    s.write("calendar-b.ts", B_CALENDAR);
    s.write_json(
        "b.json",
        &json!({
            "id": "b",
            "runtimes": { "node": {} },
            "listen": [{ "name": "local", "address": format!("127.0.0.1:{port}") }],
            "rows": [
                { "id": "calendar", "name": "./calendar-b.ts" },
                {
                    "id": "a",
                    "name": "rutis-bridge/peer",
                    "config": { "peer": "a", "listen": "local", "export": ["calendar"] },
                },
            ],
        }),
    );
    let mut b = s.host_with(["run", "b.json"], &[("RUTIS_TOKEN_A", token)]);
    b.expect("calendar: running");
    b.expect("a: running");
    b
}

/// The Python runtime's processes among A's.
fn python(processes: &[(u32, String)]) -> Vec<(u32, String)> {
    processes
        .iter()
        .filter(|(_, command)| command.to_lowercase().contains("python"))
        .cloned()
        .collect()
}

/// A's other processes (the Node runtime, which runs `planner` and the
/// probe), by pid.
fn others(processes: &[(u32, String)]) -> Vec<(u32, String)> {
    let python = python(processes);
    let mut others: Vec<_> = processes
        .iter()
        .filter(|process| !python.contains(process))
        .cloned()
        .collect();
    others.sort();
    others
}

/// What one call of `planner.plan` gave.
fn plan(on: &str) -> Result<Value, String> {
    Ok(json!(format!("plan on {on}")))
}

/// Send `reload` to A and call `planner.plan` from `watch` one call after
/// another until it answers from `to`; then wait for A's answer to the
/// reload, which must be `applied`. Every call must end, either from
/// `from`, from `to`, or failing with a message, and none from `from`
/// once one came from `to`. Returns the outcomes.
///
/// risk: G2 (a call in the switch's window hangs), G4 (a user keeps the
/// old implementation)
fn switch(a: &mut Host, watch: &Probe, from: &str, to: &str) -> Vec<Result<Value, String>> {
    a.skip_written();
    a.send_line("reload");
    let deadline = Instant::now() + hang_guard();
    let mut outcomes = Vec::new();
    loop {
        // Each call ends within the hang guard, or the scenario fails (G2).
        let outcome = watch.call(a, "planner", "plan", json!([]));
        let done = outcome == plan(to);
        outcomes.push(outcome);
        if done {
            break;
        }
        let rejected = a
            .lines()
            .into_iter()
            .find(|line| line.text.starts_with("rejected: "));
        assert!(rejected.is_none(), "A refused the reload: {rejected:?}");
        assert!(
            Instant::now() < deadline,
            "no call answered from {to} within {:?} (hang guard): {outcomes:?}",
            hang_guard()
        );
    }
    let answer = a.wait_for("A's answer to reload", |line| {
        line == "applied" || line.starts_with("rejected: ")
    });
    assert_eq!(answer, "applied");
    let first_new = outcomes.iter().position(|outcome| *outcome == plan(to));
    for (index, outcome) in outcomes.iter().enumerate() {
        match outcome {
            Ok(_) if *outcome == plan(from) => assert!(
                first_new.is_none_or(|new| index < new),
                "a call answered from {from} after one from {to}: {outcomes:?}"
            ),
            Ok(_) if *outcome == plan(to) => {}
            Ok(other) => panic!("a call answered neither from {from} nor {to}: {other}"),
            Err(error) => assert!(!error.is_empty(), "a failure without a message"),
        }
    }
    outcomes
}

/// Stage 1: A with `planner` and the Python `calendar`, every row running;
/// a call answers from Python. Returns A, with its processes (a Python
/// runtime among them).
fn stage_1(s: &Scenario, watch: &Probe, token: &str) -> (Host, Vec<(u32, String)>) {
    s.link_node_sdk();
    s.write("planner.ts", PLANNER);
    s.write("py_calendar.py", PY_CALENDAR);
    a_config(s, Calendar::Python, watch);
    let mut a = s.fixture_host(["rutis.json"], &[("RUTIS_TOKEN_B", token)]);
    for row in ["calendar", "planner", "watch"] {
        a.expect(&format!("{row}: running"));
    }
    watch.started(&mut a);
    assert_eq!(
        watch.call(&mut a, "planner", "plan", json!([])),
        plan("python")
    );
    let before = s.descendants(&a);
    assert!(
        !python(&before).is_empty(),
        "no Python runtime among A's processes: {before:?}"
    );
    (a, before)
}

/// After a switch away from Python: no Python runtime among A's
/// processes (G3), and the Node runtime is the same process as `before`.
fn python_gone(s: &Scenario, a: &Host, before: &[(u32, String)]) {
    let after = s.descendants(a);
    assert!(
        python(&after).is_empty(),
        "A's Python runtime still runs after its row and runtime were removed: {after:?}"
    );
    assert_eq!(others(&after), others(before), "A's Node runtime");
}

/// The end: a few more calls answer from `on`; A is the process `pid`
/// throughout (G5) and no row of it failed; A stops (every cleanup runs,
/// exit 0).
fn end_a(a: &mut Host, watch: &Probe, on: &str, pid: u32) {
    for _ in 0..3 {
        assert_eq!(watch.call(a, "planner", "plan", json!([])), plan(on));
    }
    assert_eq!(a.pid(), pid, "A is the same process");
    let failed: Vec<String> = a
        .lines()
        .into_iter()
        .map(|line| line.text)
        .filter(|line| line.contains(": failed"))
        .collect();
    assert!(failed.is_empty(), "rows failed: {failed:?}");
    a.send_line("stop");
    let status = a.wait_exit();
    assert_eq!(status.code(), Some(0), "A after stop: {status}");
}

/// The three stages, one switch at a time: Python, B's `calendar` over a
/// link, Rust.
///
/// risk: G2 (a call in the switch's window hangs), G3 (the old
/// implementation's process or link is left after the switch), G4 (a user
/// keeps the old implementation), G5 (the host has to restart for the
/// switch)
///
/// guarantee: core/D14 (a provider withdrawn stops its consumers, which
/// start again once it is back: `planner` follows each switch, its row
/// untouched), core/D22 (`planner` waits, without failing, while no
/// `calendar` runs)
#[test]
#[ignore = "bug #325: a call path crossing the link reuses process-local session tags, and B's runtime ends its session (invalid invocation)"]
fn an_implementation_moves_from_python_to_a_node_to_rust_while_the_host_runs() {
    let s = Scenario::new("migration");
    let watch = s.probe("watch", Lang::Node, &[]);
    let token = s.secret();
    let port = s.port();

    // 1. A: `planner` with the Python `calendar`.
    let (mut a, before) = stage_1(&s, &watch, &token);
    let pid = a.pid();

    // 2. One edit: the Python row and runtime out, a link to B importing
    //    its `calendar` in. Results come from B; A's Python runtime has
    //    exited (G3); the Node runtime is the same process.
    let mut b = node_b(&s, port, &token);
    let proxy = s.proxy(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    let dial = format!("ws://{}/rutis", proxy.addr());
    a_config(&s, Calendar::Node(&dial), &watch);
    switch(&mut a, &watch, "python", "node B");
    b.expect("calendar on B: today");
    python_gone(&s, &a, &before);

    // 3. One edit: the link out, the Rust `calendar` in. Results come from
    //    Rust; A's connection to B is closed (G3), and B runs no more
    //    calls.
    a_config(&s, Calendar::Rust, &watch);
    switch(&mut a, &watch, "node B", "rust");
    proxy.wait_closed(1);
    let ran_on_b = b.count("calendar on B: today");
    python_gone(&s, &a, &before);

    // Throughout: the same A process (G5), no row failed. Then B stops
    // (Ctrl-C); it ran nothing after the switch away from it. The residue
    // checks end it.
    end_a(&mut a, &watch, "rust", pid);
    b.ctrl_c();
    let status = b.wait_exit();
    assert_eq!(status.code(), Some(0), "B after Ctrl-C: {status}");
    b.wait_output_end();
    assert_eq!(
        b.count("calendar on B: today"),
        ran_on_b,
        "B ran calls after A's link to it was removed"
    );
    s.finish();
}

/// Stages 1 and 3 without the node in between (it is blocked by #325):
/// one edit moves `calendar` from the Python row to the Rust row.
///
/// risk: G2 (a call in the switch's window hangs), G3 (the Python runtime
/// is left after the switch), G4 (a user keeps the old implementation), G5
/// (the host has to restart for the switch)
///
/// guarantee: core/D14 (`planner` follows the switch, its row untouched),
/// core/D22 (`planner` waits, without failing, while no `calendar` runs)
#[test]
fn an_implementation_moves_from_python_to_rust_while_the_host_runs() {
    let s = Scenario::new("migration-rust");
    let watch = s.probe("watch", Lang::Node, &[]);
    let token = s.secret();
    let (mut a, before) = stage_1(&s, &watch, &token);
    let pid = a.pid();

    a_config(&s, Calendar::Rust, &watch);
    switch(&mut a, &watch, "python", "rust");
    python_gone(&s, &a, &before);

    end_a(&mut a, &watch, "rust", pid);
    s.finish();
}
