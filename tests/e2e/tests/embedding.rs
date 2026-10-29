//! S7, a Rust application embedding rutis as a multi-tenant service (#191;
//! quality-status scenario D): one instance subtree per tenant, through a
//! real lifecycle, restart included.
//!
//! The application is the fixture `tests/e2e/fixture-tenants` (on the side
//! under test; its module comment says what it does and which commands it
//! reads on stdin). It is assembled from the crates as
//! `docs/guide/rust-host.md` says. Its rows (`app/rows.json`):
//!
//! | row | where | what |
//! | --- | --- | --- |
//! | `words` | global, Python | `words.shout(text)` |
//! | `cfg-<tenant>` | global, Rust, in the admin layer | the tenant's `{ tenant, greeting, quota }` |
//! | `tenant` | the instanced group, one instance per open tenant | |
//! | `settings` | in `tenant`, Rust | the instance service `settings`, from `cfg-<tenant>` |
//! | `greeter` | in `tenant`, TS | the instance service `greeter`: `hello`, `token` |
//! | `py-greeter` | in `tenant`, Python | the instance service `py_greeter` (uses `words`) |
//! | `caller` | in `tenant`, Rust | uses `greeter`, `py_greeter`: calls "from the tenant" go through it |
//!
//! The admin layer (`state/edits.json`) is the application's only storage:
//! a tenant is a row `cfg-<tenant>` there, disabled while it is closed.
//!
//! What is observed how:
//!
//! - which implementation answers: `call <tenant> …` through the tenant's
//!   `caller` (each greeter says its tenant and language); `peek` looks up
//!   another tenant's instance service from inside a tenant, and from the
//!   root;
//! - whether a copy was reloaded: `state`'s `identity`, the loader's plugin
//!   ids of the tenant's rows and a token each greeter draws as it starts;
//! - what is left after closing: `inspect`, which reads the kernel's
//!   diagnostics (plugins under the instance, bindings with its instance
//!   key), the loader's entries, and what each runtime process holds (rows,
//!   host proxies, export slots, object handles, services; Node's Cordis
//!   plugins and fibers, Python's listeners), through the runtimes' test
//!   control method `inspect` (`RUTIS_TEST_INSPECT`);
//! - what is stored: `state/edits.json`, read here;
//! - resources: the application's processes (the harness's `descendants`),
//!   each one's descriptors (handles on Windows) and resident memory, read
//!   from the system.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use rutis_e2e::{hang_guard, Host, Scenario};
use serde_json::{json, Value};

/// The global Python row: `words`.
const WORDS: &str = r#"
class Words:
    async def shout(self, text):
        return text.upper()


provides = {"words": Words}


def apply(ctx, config):
    ctx.provide("words", Words())
"#;

/// The tenant's Python row: `py_greeter`, from the tenant's `settings` and
/// the global `words`.
const PY_GREETER: &str = r#"
import secrets

inject = ["settings", "words"]


class Greeter:
    def __init__(self, tenant, greeting, words):
        self._tenant = tenant
        self._greeting = greeting
        self._words = words
        self._token = secrets.token_hex(8)

    async def hello(self, name):
        return f"{await self._words.shout(self._greeting)}, {name} (py {self._tenant})"

    async def token(self):
        return self._token


provides = {"py_greeter": Greeter}


def apply(ctx, config):
    settings = ctx.use("settings")
    ctx.provide("py_greeter", Greeter(settings.tenant(), settings.greeting(), ctx.use("words")))
"#;

/// The tenant's TS row: `greeter`, from the tenant's `settings`.
const GREETER: &str = r#"
import { definePlugin } from '@arcships/rutis'
import { randomUUID } from 'node:crypto'

interface Settings {
  tenant(): string
  greeting(): string
}

export default definePlugin({
  inject: ['settings'],
  provides: { greeter: { hello: 'async', token: 'async' } },
  apply(ctx) {
    const settings = ctx.use<Settings>('settings')
    const tenant = settings.tenant()
    const greeting = settings.greeting()
    const token = randomUUID()
    ctx.provide('greeter', {
      hello: async (name: string) => `${greeting}, ${name} (ts ${tenant})`,
      token: async () => token,
    })
  },
})
"#;

/// The rows inside a tenant.
const TENANT_ROWS: [&str; 4] = ["settings", "greeter", "py-greeter", "caller"];

fn project(s: &Scenario) {
    s.link_node_sdk();
    s.write("plugins/words.py", WORDS);
    s.write("plugins/py_greeter.py", PY_GREETER);
    s.write("plugins/greeter.ts", GREETER);
    let greeter = s.path("plugins/greeter.ts").display().to_string();
    s.write_json(
        "app/rows.json",
        &json!([{ "insert": [
            { "id": "words", "name": "py:words" },
            { "id": "tenant", "group": true, "instanced": true, "config": [
                { "id": "settings", "name": "app/settings" },
                { "id": "greeter", "name": greeter },
                { "id": "py-greeter", "name": "py:py_greeter" },
                { "id": "caller", "name": "app/caller" },
            ] },
        ] }]),
    );
}

/// The application, started on the project; its `ready` answer.
fn start(s: &Scenario) -> (Host, Value) {
    let mut app = s.fixture_tenants(Vec::<&str>::new(), &[("RUTIS_TEST_INSPECT", "1")]);
    let ready = answer(&mut app, "ready");
    (app, ready)
}

/// Send `command` and return its answer.
fn ask(app: &mut Host, command: &str) -> Value {
    app.send_line(command);
    answer(app, command)
}

/// [`ask`], which must not fail.
fn ok(app: &mut Host, command: &str) -> Value {
    let answer = ask(app, command);
    assert!(answer.get("error").is_none(), "{command}: {answer}");
    answer
}

/// The next answer line.
fn answer(app: &mut Host, what: &str) -> Value {
    let line = app.wait_for(&format!("the answer to {what:?}"), |line| {
        line.starts_with("{\"answer\":")
    });
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line}: {e}"))
}

/// Every row of a new tenant started, or waits for a service a row next
/// to it is about to provide (`create_instance` returns once the rows
/// settled; a consumer may not have seen its provider start yet). The
/// application answers `create` once the tenant's door, `caller`, the last
/// to start, has opened.
fn started(created: &Value) {
    for row in TENANT_ROWS {
        let result = created["report"][row].as_str().unwrap_or_default();
        assert!(["Active", "Waiting"].contains(&result), "{row}: {created}");
    }
}

/// Stop the application: every cleanup runs, it exits 0.
fn stop(app: &mut Host) {
    app.send_line("stop");
    assert!(app.wait_exit().success(), "the application stopped cleanly");
}

/// What `call` gives.
fn call(app: &mut Host, tenant: &str, service: &str) -> Value {
    ok(app, &format!("call {tenant} {service} hello [\"Ada\"]"))["ok"].take()
}

fn visible(app: &mut Host, from: &str, of: &str, service: &str) -> bool {
    ok(app, &format!("peek {from} {of} {service}"))["visible"] == true
}

/// A tenant's state without what identifies the running copies.
fn without_identity(mut state: Value) -> Value {
    let fields = state.as_object_mut().unwrap();
    fields.remove("identity");
    fields.remove("answer");
    state
}

fn edits(s: &Scenario) -> String {
    std::fs::read_to_string(s.path("state/edits.json")).expect("the admin layer")
}

/// The numbers `inspect` reports: what rutis, the loader and each runtime
/// hold (a list counts its items).
fn counts(inspect: &Value) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for part in ["rutis", "loader"] {
        for (name, value) in inspect[part].as_object().unwrap() {
            counts.insert(format!("{part}.{name}"), value.as_u64().unwrap());
        }
    }
    for (runtime, held) in inspect["runtimes"].as_object().unwrap() {
        assert!(held.get("error").is_none(), "{runtime}: {held}");
        for (name, value) in held.as_object().unwrap() {
            let count = match value {
                Value::Array(items) => items.len() as u64,
                other => other.as_u64().unwrap(),
            };
            counts.insert(format!("{runtime}.{name}"), count);
        }
    }
    counts
}

/// Every id a runtime holds (rows, services, proxies, slots, handles).
fn ids(inspect: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (runtime, held) in inspect["runtimes"].as_object().unwrap() {
        for (name, value) in held.as_object().unwrap() {
            for id in value.as_array().into_iter().flatten() {
                ids.insert(format!("{runtime}.{name}: {}", id.as_str().unwrap()));
            }
        }
    }
    ids
}

/// What of a tenant `inspect` finds in the runtimes: rows keyed by its
/// fibers, and ids in its instance's scope.
fn tenant_ids(inspect: &Value, tenant: &Value) -> Vec<String> {
    let label = tenant["label"].as_str().unwrap();
    let rows: Vec<&str> = tenant["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row.as_str().unwrap())
        .collect();
    ids(inspect)
        .into_iter()
        .filter(|id| {
            let (kind, id) = id.split_once(": ").unwrap();
            id.contains(label)
                || (kind.ends_with(".rows") && rows.iter().any(|r| id.starts_with(r)))
        })
        .collect()
}

/// Inspect until the numbers are `expected` (what a closed instance leaves
/// goes as its last releases arrive), within the hang guard; the last
/// inspection.
fn inspect_until(app: &mut Host, what: &str, expected: &BTreeMap<String, u64>) -> Value {
    let deadline = Instant::now() + hang_guard();
    loop {
        let inspect = ok(app, "inspect");
        let found = counts(&inspect);
        if found == *expected {
            return inspect;
        }
        if Instant::now() >= deadline {
            let differ: Vec<String> = expected
                .keys()
                .chain(found.keys())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|name| expected.get(*name) != found.get(*name))
                .map(|name| {
                    format!(
                        "{name}: {:?}, expected {:?}",
                        found.get(name),
                        expected.get(name)
                    )
                })
                .collect();
            panic!(
                "{what}: not within {:?}: {differ:?}; last inspection: {inspect:#}",
                hang_guard()
            );
        }
        // Polling: the application has no event for "the runtimes let go".
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Inspect until what a runtime holds no longer moves: nothing left of
/// `gone` (a tenant's entry from an earlier inspection, closed since), and
/// two inspections in a row with the same numbers, within the hang guard;
/// the last inspection. A baseline taken while a close still settles would
/// count what is about to go.
fn inspect_settled(app: &mut Host, what: &str, gone: Option<&Value>) -> Value {
    let deadline = Instant::now() + hang_guard();
    let mut last: Option<BTreeMap<String, u64>> = None;
    loop {
        let inspect = ok(app, "inspect");
        let found = counts(&inspect);
        let left = gone
            .map(|tenant| tenant_ids(&inspect, tenant))
            .unwrap_or_default();
        if left.is_empty() && last.as_ref() == Some(&found) {
            return inspect;
        }
        if Instant::now() >= deadline {
            panic!(
                "{what}: not settled within {:?}: left {left:?}, numbers {last:?} then {found:?}",
                hang_guard()
            );
        }
        last = left.is_empty().then_some(found);
        // Polling: the application has no event for "the runtimes let go".
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Steps 1 to 5 of #191: start with global and template rows; open two
/// tenants that see only their own instance services; edit one tenant's
/// configuration, all or nothing; close a tenant and find nothing of it
/// left, in rutis or in the runtime processes; restart and find the open
/// tenant as it was.
///
/// risk: D1 (a tenant sees another's services), D2 (a closed instance
/// leaves registrations, in rutis or in the runtimes), D4 (an edit half
/// applied), D5 (the state differs after a restart)
/// guarantee: loader/instance-services, loader/instance-close-leaves-nothing,, loader/saved-layers-restore
/// loader/instance-edits-all-or-none, loader/saved-layers-restore
#[test]
fn tenants_are_created_edited_closed_and_restored_after_a_restart() {
    let s = Scenario::new("embedding");
    project(&s);
    let (mut app, ready) = start(&s);

    // 1. The global rows run; the instanced group waits for instances.
    assert_eq!(ready["restored"], json!([]), "{ready}");
    assert_eq!(ready["rows"]["words"], "Active", "{ready}");
    let empty = ok(&mut app, "inspect");
    assert_eq!(
        empty["runtimes"]["py"]["rows"].as_array().unwrap().len(),
        1,
        "{empty}"
    );
    assert_eq!(empty["runtimes"]["node"]["rows"], json!([]), "{empty}");

    // 2. Two tenants: each one's rows run, and the same names answer from
    // the tenant they are called in.
    let t1 = ok(&mut app, "create t1 Hello");
    let only_t1 = inspect_settled(&mut app, "t1 alone", None);
    let t2 = ok(&mut app, "create t2 Hej");
    for created in [&t1, &t2] {
        started(created);
    }
    assert_eq!(call(&mut app, "t1", "greeter"), "Hello, Ada (ts t1)");
    assert_eq!(call(&mut app, "t2", "greeter"), "Hej, Ada (ts t2)");
    assert_eq!(call(&mut app, "t1", "py_greeter"), "HELLO, Ada (py t1)");
    assert_eq!(call(&mut app, "t2", "py_greeter"), "HEJ, Ada (py t2)");
    for service in ["greeter", "py_greeter", "settings"] {
        assert!(visible(&mut app, "t1", "t1", service), "{service}");
        assert!(!visible(&mut app, "t1", "t2", service), "{service}");
        assert!(!visible(&mut app, "t2", "t1", service), "{service}");
        assert!(!visible(&mut app, "-", "t1", service), "{service}");
    }

    // 3. An admin edit of t1 reaches t1's rows (TS and Python included)
    // and nothing of t2's; it is stored.
    let t2_before = ok(&mut app, "state t2");
    let t1_before = ok(&mut app, "state t1");
    ok(
        &mut app,
        r#"edit t1 {"tenant": "t1", "greeting": "Bonjour", "quota": 2}"#,
    );
    let t1_edited = ok(&mut app, "state t1");
    assert_eq!(t1_edited["answers"]["greeter"], "Bonjour, Ada (ts t1)");
    assert_eq!(t1_edited["answers"]["py_greeter"], "BONJOUR, Ada (py t1)");
    assert_eq!(t1_edited["settings"]["quota"], 2);
    assert_ne!(
        t1_edited["identity"]["tokens"], t1_before["identity"]["tokens"],
        "t1's rows restarted with the new configuration"
    );
    assert_eq!(ok(&mut app, "state t2"), t2_before, "t2 is untouched");
    let stored = edits(&s);
    assert!(stored.contains("Bonjour"), "{stored}");

    // Edits that cannot apply change nothing: one the configuration's
    // validation refuses (a negative quota, with a new greeting), and one
    // that passes it but fails as the row starts (a quota of 0), which is
    // rolled back. Nothing is stored; t2 is untouched.
    let refused = ask(
        &mut app,
        r#"edit t1 {"tenant": "t1", "greeting": "Salut", "quota": -1}"#,
    );
    assert_eq!(refused["kind"], "rejected", "{refused}");
    assert_eq!(
        ok(&mut app, "state t1"),
        t1_edited,
        "a refused edit touches nothing"
    );
    let rolled_back = ask(
        &mut app,
        r#"edit t1 {"tenant": "t1", "greeting": "Salut", "quota": 0}"#,
    );
    assert_eq!(rolled_back["kind"], "apply-failed", "{rolled_back}");
    assert_eq!(
        without_identity(ok(&mut app, "state t1")),
        without_identity(t1_edited.clone()),
        "an edit rolled back leaves t1 as it was"
    );
    assert_eq!(edits(&s), stored, "nothing more is stored");
    assert_eq!(ok(&mut app, "state t2"), t2_before, "t2 is untouched");

    // 4. Closing t2 leaves nothing of it: no plugin under its instance, no
    // binding with its key, no row, proxy, slot, handle or service of it
    // in either runtime process; t1 runs on.
    let before = ok(&mut app, "inspect");
    let t2_info = before["tenants"]["t2"].clone();
    let t2_ids = tenant_ids(&before, &t2_info);
    for part in [
        "node.rows",
        "node.hosts",
        "node.slots",
        "py.rows",
        "py.hosts",
        "py.slots",
    ] {
        assert!(
            t2_ids.iter().any(|id| id.starts_with(part)),
            "t2 is seen in {part} while open: {t2_ids:?}"
        );
    }
    assert!(
        t2_info["plugins"].as_u64().unwrap() > TENANT_ROWS.len() as u64,
        "{t2_info}"
    );
    assert!(t2_info["bindings"].as_u64().unwrap() >= 3, "{t2_info}");
    let t1_running = ok(&mut app, "state t1");
    ok(&mut app, "close t2");
    // The same numbers as with t1 alone (t1 restarted since: the ids
    // differ, the numbers do not), but for t2's row `cfg-t2`, which stays,
    // disabled.
    let mut expected = counts(&only_t1);
    *expected.get_mut("loader.entries").unwrap() += 1;
    let after = inspect_until(&mut app, "t2's registrations go", &expected);
    assert!(after["tenants"].get("t2").is_none(), "{after}");
    assert_eq!(tenant_ids(&after, &t2_info), Vec::<String>::new());
    let new: Vec<_> = ids(&after).difference(&ids(&before)).cloned().collect();
    assert!(new.is_empty(), "closing t2 added {new:?}");
    assert!(!visible(&mut app, "t1", "t2", "greeter"));
    assert!(ask(&mut app, "call t2 greeter hello [\"Ada\"]")
        .get("error")
        .is_some());
    assert_eq!(ok(&mut app, "state t1"), t1_running, "t1 is untouched");

    // 5. Restarted, the application opens t1 again from the admin layer
    // alone, as it was; t2 stays closed, its configuration kept.
    let stored = edits(&s);
    stop(&mut app);
    let (mut app, ready) = start(&s);
    assert_eq!(ready["restored"], json!(["t1"]), "{ready}");
    assert_eq!(
        without_identity(ok(&mut app, "state t1")),
        without_identity(t1_running),
        "t1 after the restart"
    );
    assert_eq!(edits(&s), stored, "starting stores nothing");
    started(&ok(&mut app, "create t2"));
    assert_eq!(call(&mut app, "t2", "greeter"), "Hej, Ada (ts t2)");
    stop(&mut app);
    s.finish();
}

/// `ready` comes once the global rows started (#568), however late a
/// runtime's rows are released: here 3 s after the loader mounted, as on a
/// loaded machine, so `reconcile` returns with `words` still `Pending`.
#[test]
fn ready_waits_for_the_global_rows_to_start() {
    let s = Scenario::new("embedding-late-rows");
    project(&s);
    let mut app = s.fixture_tenants(
        Vec::<&str>::new(),
        &[
            ("RUTIS_TEST_INSPECT", "1"),
            ("RUTIS_TEST_ROWS_LATE_MS", "3000"),
        ],
    );
    let ready = answer(&mut app, "ready");
    assert_eq!(ready["rows"]["words"], "Active", "{ready}");
    stop(&mut app);
    s.finish();
}

// ── Step 6: churn ───────────────────────────────────────────────

/// How many tenants step 6 creates and closes after its warm-up:
/// `RUTIS_E2E_TENANT_CYCLES`, else 20 (pull requests, about a second);
/// the full runs of the `e2e` job (nightly) set 3000 (D3, about a minute
/// on macOS).
fn cycles() -> usize {
    match std::env::var("RUTIS_E2E_TENANT_CYCLES") {
        Ok(n) => n
            .parse()
            .unwrap_or_else(|e| panic!("RUTIS_E2E_TENANT_CYCLES={n:?}: {e}")),
        Err(_) => 20,
    }
}

/// A process's open descriptors (handles on Windows) and resident memory
/// (the working set on Windows) in KiB, as far as this platform tells.
#[derive(Debug, Clone, Copy)]
struct Held {
    fds: Option<u64>,
    rss: Option<u64>,
}

#[cfg(target_os = "linux")]
fn held(pid: u32) -> Held {
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .map(|dir| dir.count() as u64);
    let rss = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|kib| kib.trim().trim_end_matches("kB").trim().parse().ok())
        });
    Held { fds, rss }
}

#[cfg(target_os = "macos")]
fn held(pid: u32) -> Held {
    let size = std::mem::size_of::<libc::proc_fdinfo>() as i32;
    // SAFETY: proc_pidinfo(2) with a null buffer returns the size it needs.
    let fds = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDLISTFDS,
            0,
            std::ptr::null_mut(),
            0,
        )
    };
    // With a buffer, what it filled: the size above is an estimate.
    let fds = (fds > 0).then(|| {
        let mut buffer = vec![0u8; fds as usize + 64 * size as usize];
        // SAFETY: the buffer is as long as the size passed.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDLISTFDS,
                0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as i32,
            )
        };
        (filled.max(0) / size) as u64
    });
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let info_size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    // SAFETY: the buffer is one proc_taskinfo.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size,
        )
    };
    // SAFETY: filled whole when it returned its size.
    let rss = (read == info_size).then(|| unsafe { info.assume_init() }.pti_resident_size / 1024);
    Held { fds, rss }
}

#[cfg(windows)]
fn held(pid: u32) -> Held {
    // The handle count and the working set, as the soak reads them.
    let usage = rutis_e2e::usage::sample(&[pid])
        .first()
        .map(|(_, usage)| *usage);
    Held {
        fds: usage.map(|u| u.fds),
        rss: usage.map(|u| u.rss_kib),
    }
}

/// The application's processes and what each holds, by pid.
fn sample(s: &Scenario, app: &Host) -> BTreeMap<u32, (String, Held)> {
    let mut processes: BTreeMap<u32, (String, Held)> = s
        .descendants(app)
        .into_iter()
        .map(|(pid, command)| (pid, (command, held(pid))))
        .collect();
    processes.insert(app.pid(), ("the application".into(), held(app.pid())));
    processes
}

/// Descriptors a process may hold more after the churn than before it:
/// what a runtime opens lazily (a module, a cache), never one per cycle.
const MORE_FDS: u64 = 8;

/// Resident memory a process may grow by during `cycles` cycles after the
/// warm-up, in KiB. The application (Rust, where rutis runs): 1 MiB and
/// 256 B per cycle. The runtimes and what they start: 32 MiB and 1 KiB per
/// cycle, for a heap that a garbage collector sizes as it likes.
///
/// Measured on macOS (debug build, 3000 cycles): the application stayed
/// at 10.3 MiB; Node went 143 → 147 MiB (151 halfway); Python
/// 31.0 → 31.5 MiB. Before the projection fix of #191 the application
/// grew by 1.4 KiB per cycle (a task per language row loaded, waiting for
/// its runtime's session to end): 4.1 MiB over 3000 cycles, above the
/// 1.7 MiB allowed. So the nightly run (3000 cycles) catches a leak of a
/// task per cycle in the application, and gross leaks in the runtimes;
/// on pull requests (20 cycles) what catches a leak is the exact checks:
/// the numbers `inspect` reports back to the baseline, the same processes,
/// the descriptors.
fn more_rss(application: bool, cycles: usize) -> u64 {
    let cycles = cycles as u64;
    match application {
        true => 1024 + cycles / 4,
        false => 32 * 1024 + cycles,
    }
}

/// Step 6 of #191: tenants created and closed over and over, next to a
/// tenant that stays open, hold nothing more: the same processes, the
/// numbers of what rutis, the loader and the runtimes hold back to the
/// baseline exactly, descriptors and memory flat; the resident tenant is
/// never restarted.
///
/// risk: D3 (instances created and closed many times pile up tasks,
/// memory, table entries)
/// guarantee: loader/instance-close-leaves-nothing
#[test]
fn creating_and_closing_tenants_over_and_over_holds_nothing_more() {
    let s = Scenario::new("embedding-churn");
    project(&s);
    let (mut app, _) = start(&s);
    ok(&mut app, "create resident Hi");
    let resident = ok(&mut app, "state resident");

    let cycles = cycles();
    let warm_up = (cycles / 10).max(5);
    // One cycle; with `last`, what `inspect` sees of the tenant while it
    // is open, to wait for it to go after.
    let churn = |app: &mut Host, cycle: usize, last: bool| {
        // Four names: the admin layer keeps four more rows, enabled and
        // disabled in turn.
        let tenant = format!("churn{}", cycle % 4);
        started(&ok(app, &format!("create {tenant}")));
        assert_eq!(
            ok(app, &format!("call {tenant} greeter hello [\"Ada\"]"))["ok"],
            format!("Hello, Ada (ts {tenant})")
        );
        assert_eq!(
            ok(app, &format!("call {tenant} py_greeter hello [\"Ada\"]"))["ok"],
            format!("HELLO, Ada (py {tenant})")
        );
        let open = last.then(|| ok(app, "inspect")["tenants"][&tenant].clone());
        ok(app, &format!("close {tenant}"));
        open
    };
    for cycle in 0..warm_up - 1 {
        churn(&mut app, cycle, false);
    }
    let last = churn(&mut app, warm_up - 1, true).unwrap();
    assert!(last.get("label").is_some(), "{last}");
    let base_inspect = inspect_settled(&mut app, "the baseline", Some(&last));
    let base = sample(&s, &app);
    let started = Instant::now();
    let mut middle = None;
    for cycle in warm_up..warm_up + cycles {
        churn(&mut app, cycle, false);
        if cycle == warm_up + cycles / 2 {
            middle = Some(sample(&s, &app));
        }
    }
    let elapsed = started.elapsed();
    let end_inspect = inspect_until(
        &mut app,
        "the numbers back to the baseline",
        &counts(&base_inspect),
    );
    let end = sample(&s, &app);

    let report: Vec<String> = base
        .iter()
        .map(|(pid, (command, before))| {
            let after = end.get(pid).map(|(_, held)| *held);
            let middle = middle
                .as_ref()
                .and_then(|m| m.get(pid))
                .map(|(_, held)| *held);
            format!(
                "{command} ({pid}): fds {:?} → {:?} → {:?}, rss KiB {:?} → {:?} → {:?}",
                before.fds,
                middle.and_then(|h| h.fds),
                after.and_then(|h| h.fds),
                before.rss,
                middle.and_then(|h| h.rss),
                after.and_then(|h| h.rss),
            )
        })
        .collect();
    eprintln!(
        "S7 churn: {cycles} tenants created and closed in {elapsed:?} (after {warm_up} to warm up)\n  {}",
        report.join("\n  ")
    );

    let pids = |sample: &BTreeMap<u32, (String, Held)>| sample.keys().copied().collect::<Vec<_>>();
    assert_eq!(pids(&end), pids(&base), "the same processes: {report:#?}");
    assert_eq!(counts(&end_inspect), counts(&base_inspect));
    for (pid, (command, before)) in &base {
        let after = end[pid].1;
        if let (Some(before), Some(after)) = (before.fds, after.fds) {
            assert!(
                after <= before + MORE_FDS,
                "{command}: descriptors {before} → {after}\n{report:#?}"
            );
        }
        if let (Some(before), Some(after)) = (before.rss, after.rss) {
            assert!(
                after <= before + more_rss(*pid == app.pid(), cycles),
                "{command}: resident KiB {before} → {after}\n{report:#?}"
            );
        }
    }
    assert_eq!(
        ok(&mut app, "state resident"),
        resident,
        "the resident tenant was never restarted"
    );
    stop(&mut app);
    s.finish();
}
