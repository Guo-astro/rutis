//! S8 (#192): one `rutis-host run` for a long time, with random operations,
//! then every way a host ends. Risks L1 (the host's descriptors, threads,
//! memory grow with time), L3 (runtimes killed over and over leave the
//! host's state or counts wrong), L4 (a failing random sequence that cannot
//! be replayed) of `docs/quality-status.md`; quality standard Q5.4.3,
//! Q5.4.4, Q5.7.2.
//!
//! The project is S3's (`runtime_crash.rs`): a Python row `store`, a TS row
//! `report` using it, a Bun row `audit` using `report` (not on Windows,
//! where Bun is not declared), and two probes: `probe` uses the chain,
//! `watch` uses nothing and looks services up at each call. Added to it: a
//! remote Python runtime (node B, `python -m rutis listen`, as in S5) with
//! the row `infer`, linked through the harness's TCP proxy, and a third
//! probe `far` that uses it; and up to
//! [`SLOTS`] rows `x1`… that the random edits add, change (and move between
//! runtimes) and remove.
//!
//! Events (#464): each slot row also listens to `t/slot/<its id>` and
//! answers what its `ping` does; `report` (Node) and B's `infer` (the
//! remote Python runtime) `serial` it. Every [`Op::Call`] asks each slot
//! from both, so whatever the random edits, kills and cuts did, an event
//! reaches exactly the listener the configuration has now, in whichever
//! runtime, and nothing once the slot is removed.
//!
//! Phases:
//!
//! 1. The soak host: the Node and Python runtimes have `"restart":
//!    "on-failure"`. For `RUTIS_S8_SECS` seconds (150 by default; the
//!    nightly run, an hour), random operations from a seeded generator
//!    ([`Op`]): calls through every row; configuration edits that add,
//!    remove and change rows, reloaded by SIGUSR1 or `--watch-config` (one
//!    per host, at random; Windows has only the watch); edits the host must
//!    reject (not JSON, a plugin that does not exist, a changed runtime
//!    setting), then the file put back; `kill -9` of the Node or Python
//!    runtime, which the host starts again; the link to B cut and restored.
//!    Every operation waits for what the host must print (the reload's
//!    result, `runtime <name>: down (…)` / `restarting` / `running`, row
//!    states, the link's lines) and checks it with calls. Meanwhile the
//!    host, its runtimes and B are sampled ([`rutis_e2e::usage`]): process
//!    count, descriptors (handles), threads, resident memory; their growth
//!    is bounded ([`Bounds`]); a timed run too short to compare the
//!    samples fails. It starts with one fault of each kind ([`Planner`]),
//!    whatever the seed. The host ends in
//!    one of the ways below, chosen by the seed.
//! 2. One shorter host per way of ending, every run: Ctrl-C (Ctrl-Break on
//!    Windows), SIGTERM (Unix), `kill -9` (TerminateProcess). Its runtimes
//!    have no restart policy: after a few random operations its Python
//!    runtime is killed, and stays down, its rows waiting and calls into it
//!    failing explicitly. Before a `kill -9`, rows whose cleanup never
//!    returns are added in every runtime (#216, #353): on Unix each runtime
//!    must unload its rows by itself when the host is gone, give up the
//!    stuck cleanup at the shutdown deadline (`--shutdown-timeout`, passed
//!    to it as `RUTIS_SHUTDOWN_TIMEOUT`) and exit; on Windows the host's Job
//!    Object ends them. After every end, no process of the host is left,
//!    within a bound, and the host's output agrees with what was done
//!    (reloads accepted and rejected, runtimes down and restarted, links).
//!
//! The residue checks end the scenario (Q5.4.2).
//!
//! Replay: the operations are a function of the seed alone ([`Planner`]),
//! never of timing. A failure writes the seed, the operations done and the
//! samples into the scenario's `logs/` and prints the command that runs
//! the same operations again (`RUTIS_S8_SEED`, `RUTIS_S8_OPS`).
//!
//! Ignored by default (it runs for minutes):
//! `cargo test -p rutis-e2e --test soak -- --ignored --nocapture`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rutis_e2e::usage::{sample, Usage};
use rutis_e2e::{repo, Host, Lang, Probe, Proxy, Scenario};
use serde_json::json;

const STORE: &str = r#"
from rutis import define_plugin


class Store:
    def __init__(self):
        self.data = {"greeting": "hello from python"}

    async def get(self, key):
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
  provides: { report: { summary: 'async', ask: 'async' } },
  apply(ctx) {
    const store = ctx.use<Store>('store')
    ctx.provide('report', {
      summary: async (key: string) => `report: ${await store.get(key)}`,
      // Events (#464): the slot row's listener, wherever it runs.
      ask: async (slot: string) => (await ctx.serial(`t/slot/${slot}`)) ?? null,
    })
  },
})
"#;

const AUDIT: &str = r#"
export const inject = ['report']
export const provides = { audit: { check: 'async' } }
export function apply(ctx) {
  const report = ctx.use('report')
  const engine = process.versions.bun ? 'bun' : 'not bun'
  ctx.provide('audit', { check: async key => `audit (${engine}): ${await report.summary(key)}` })
}
"#;

/// B's plugin, as in S5, and `ask`: the slot row's listener through the
/// link (#464).
const INFER: &str = r#"
provides = {"infer": {"run": "async", "ask": "async"}}


class Infer:
    def __init__(self, ctx):
        self.ctx = ctx

    async def run(self, job):
        return f"inferred {job}"

    async def ask(self, slot):
        return await self.ctx.serial(f"t/slot/{slot}")


def apply(ctx, config):
    ctx.provide("infer", Infer(ctx))
"#;

/// A slot row in each runtime: `NAME.ping()` says which row, runtime and
/// revision of its configuration answer.
const SLOT_NODE: &str = r#"
import { definePlugin } from '@arcships/rutis'

export default definePlugin<{ rev: number }>({
  provides: { NAME: { ping: 'sync' } },
  apply(ctx, config) {
    ctx.provide('NAME', { ping: () => `NAME node rev ${config.rev}` })
    ctx.on('t/slot/NAME', () => `NAME node rev ${config.rev}`)
  },
})
"#;

const SLOT_BUN: &str = r#"
export const provides = { NAME: { ping: 'sync' } }
export function apply(ctx, config) {
  ctx.provide('NAME', { ping: () => `NAME bun rev ${config.rev}` })
  ctx.on('t/slot/NAME', () => `NAME bun rev ${config.rev}`)
}
"#;

const SLOT_PY: &str = r#"
provides = {"NAME": {"ping": "sync"}}


class Slot:
    def __init__(self, rev):
        self.rev = rev

    def ping(self):
        return f"NAME py rev {self.rev}"


def apply(ctx, config):
    ctx.provide("NAME", Slot(config["rev"]))
    ctx.on("t/slot/NAME", lambda: f"NAME py rev {config['rev']}")
"#;

/// Rows whose cleanup leaves a mark, then never returns; the Node and Bun
/// ones also keep a timer that nothing clears (as `stray_runtime.rs`).
const STUCK_NODE: &str = r#"
import { writeFileSync } from 'node:fs'
import { definePlugin } from '@arcships/rutis'

export default definePlugin<{ marker: string }>({
  provides: { stucknode: { ping: 'sync' } },
  apply(ctx, config) {
    setInterval(() => {}, 1 << 30)
    ctx.provide('stucknode', { ping: () => 'pong' })
    return () => {
      writeFileSync(config.marker, 'cleaned up')
      return new Promise(() => {})
    }
  },
})
"#;

const STUCK_BUN: &str = r#"
import { writeFileSync } from 'node:fs'
export const provides = { stuckbun: { ping: 'sync' } }
export function apply(ctx, config) {
  setInterval(() => {}, 1 << 30)
  ctx.provide('stuckbun', { ping: () => 'pong' })
  return () => {
    writeFileSync(config.marker, 'cleaned up')
    return new Promise(() => {})
  }
}
"#;

const STUCK_PY: &str = r#"
import asyncio

provides = {"stuckpy": {"ping": "sync"}}


class Stuck:
    def ping(self):
        return "pong"


def apply(ctx, config):
    ctx.provide("stuckpy", Stuck())

    async def cleanup():
        with open(config["marker"], "w") as marker:
            marker.write("cleaned up")
        await asyncio.Event().wait()

    return cleanup
"#;

/// Whether the project has Bun rows (see the module's comment).
const BUN: bool = cfg!(not(windows));

/// The rows the random edits add and remove: `x1`…
const SLOTS: usize = 3;

/// Every host's `--shutdown-timeout`, in seconds: the deadline of the
/// cleanups, which the runtimes also keep once their host is gone.
const SHUTDOWN: u64 = 3;

/// How long a killed host's runtimes may take to exit: the deadline, and
/// as long again as the hang guard for a slow machine to unload and exit.
fn exit_bound() -> Duration {
    Duration::from_secs(SHUTDOWN) + rutis_e2e::hang_guard()
}

/// A runtime with `"restart": "on-failure"` is killed only once it has run
/// this long: past 60 s its next restart waits 1 s again, and at most four
/// restarts fall within five minutes, so the host never gives it up
/// (`docs/guide/rutis-host.md`, "When a runtime ends"). It waits; the
/// operations stay the same.
const KILL_SPACING: Duration = Duration::from_secs(75);

/// The link to B is cut only once its session has lasted this long: the
/// link's redial wait (0.5 s, doubling up to 30 s) starts over only after a
/// session of 60 s (`docs/guide/nodes.md`), so cuts in quick succession
/// would have it wait up to 36 s to come back, past the hang guard. As
/// with kills, it waits; the operations stay the same.
const CUT_SPACING: Duration = Duration::from_secs(65);

// ---------------------------------------------------------------------
// The operations, from the seed alone.

/// splitmix64: small, and the same sequence on every platform.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Runtime {
    Node,
    Py,
    Bun,
}

impl Runtime {
    fn name(self) -> &'static str {
        match self {
            Runtime::Node => "node",
            Runtime::Py => "py",
            Runtime::Bun => "bun",
        }
    }

    /// The runtimes of the project on this platform.
    fn all() -> &'static [Runtime] {
        if BUN {
            &[Runtime::Node, Runtime::Py, Runtime::Bun]
        } else {
            &[Runtime::Node, Runtime::Py]
        }
    }

    /// Whether the process with this command line (Unix) or program
    /// (Windows) is this runtime. Not the esbuild service the Node
    /// runtime's TypeScript loader may run (its program is under
    /// `node/rutis-runtime/node_modules`): it ends with the runtime, so
    /// killing it too would race with that.
    fn runs(self, command: &str) -> bool {
        let command = command.to_lowercase();
        match self {
            Runtime::Py => command.contains("python"),
            Runtime::Bun => command.contains("bun"),
            Runtime::Node if cfg!(windows) => command.ends_with("node.exe"),
            Runtime::Node => {
                command.contains("rutis-runtime")
                    && !command.contains("bun")
                    && !command.contains("esbuild")
            }
        }
    }
}

/// An edit the host must reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bad {
    /// Not JSON.
    Syntax,
    /// A row whose plugin does not exist.
    MissingPlugin,
    /// A local runtime's setting changed (its `restart`): a restart of
    /// the host is needed.
    RuntimeSetting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// Calls through every row.
    Call,
    /// Add the slot row `x<slot+1>` in `runtime`, at revision `rev`.
    Add {
        slot: usize,
        runtime: Runtime,
        rev: u64,
    },
    /// Remove a slot row.
    Remove { slot: usize },
    /// Give a slot row a new revision, in `runtime` (the same or another).
    ChangeSlot {
        slot: usize,
        runtime: Runtime,
        rev: u64,
    },
    /// Give one of `store`, `report`, `audit` a new revision: it restarts,
    /// and what uses it waits for it meanwhile.
    ChangeMain { row: &'static str, rev: u64 },
    /// An edit to reject, then the file put back.
    Reject(Bad),
    /// `kill -9` a runtime that restarts on failure.
    Kill(Runtime),
    /// Cut the link to B, then restore it.
    Cut,
    /// Do nothing for this many milliseconds.
    Idle(u64),
}

/// What the configuration holds besides its fixed rows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Model {
    slots: [Option<(Runtime, u64)>; SLOTS],
    /// The revision of `store`, `report`, `audit`.
    revs: BTreeMap<&'static str, u64>,
    /// The rows whose cleanup never returns ([`STUCK_NODE`]…).
    stuck: bool,
}

impl Model {
    fn new() -> Self {
        let mut revs = BTreeMap::from([("store", 0), ("report", 0)]);
        if BUN {
            revs.insert("audit", 0);
        }
        Self {
            slots: [None; SLOTS],
            revs,
            stuck: false,
        }
    }
}

/// The operations of one host: a function of its seed alone. `kills`: it
/// may kill runtimes (those of a host whose runtimes restart); such a host
/// (the soak host) starts with one fault of each kind, so that every kind
/// is done whatever the seed and however short the run: a rejected edit of
/// each kind, a cut of the link to B, a kill of the Node runtime and one
/// of the Python runtime (the cut and the kills wait for their spacing,
/// some 75 s in all, sampled as idle time).
struct Planner {
    rng: Rng,
    model: Model,
    kills: bool,
    rev: u64,
    opening: Vec<Op>,
}

impl Planner {
    fn new(seed: u64, kills: bool) -> Self {
        // Taken from the end: the first operation is last.
        let opening = match kills {
            true => vec![
                Op::Kill(Runtime::Py),
                Op::Kill(Runtime::Node),
                Op::Cut,
                Op::Reject(Bad::RuntimeSetting),
                Op::Reject(Bad::MissingPlugin),
                Op::Reject(Bad::Syntax),
            ],
            false => Vec::new(),
        };
        Self {
            rng: Rng(seed),
            model: Model::new(),
            kills,
            rev: 0,
            opening,
        }
    }

    fn runtime(&mut self) -> Runtime {
        let all = Runtime::all();
        all[self.rng.below(all.len() as u64) as usize]
    }

    /// The next operation, applied to the model.
    fn next(&mut self) -> Op {
        if let Some(op) = self.opening.pop() {
            return op;
        }
        let free: Vec<usize> = (0..SLOTS)
            .filter(|&slot| self.model.slots[slot].is_none())
            .collect();
        let used: Vec<usize> = (0..SLOTS)
            .filter(|&slot| self.model.slots[slot].is_some())
            .collect();
        #[derive(Clone, Copy)]
        enum Kind {
            Call,
            Add,
            Remove,
            ChangeSlot,
            ChangeMain,
            Reject,
            Cut,
            Idle,
            Kill,
        }
        // Weights: calls most, then edits, then faults.
        let mut choices = vec![
            (5, Kind::Call),
            (2, Kind::ChangeMain),
            (2, Kind::Reject),
            (1, Kind::Cut),
            (1, Kind::Idle),
        ];
        if !free.is_empty() {
            choices.push((3, Kind::Add));
        }
        if !used.is_empty() {
            choices.extend([(2, Kind::Remove), (2, Kind::ChangeSlot)]);
        }
        if self.kills {
            choices.push((1, Kind::Kill));
        }
        let total: u64 = choices.iter().map(|(weight, _)| weight).sum();
        let mut pick = self.rng.below(total);
        let mut kind = Kind::Call;
        for (weight, choice) in choices {
            if pick < weight {
                kind = choice;
                break;
            }
            pick -= weight;
        }
        match kind {
            Kind::Call => Op::Call,
            Kind::Add => {
                let slot = free[self.rng.below(free.len() as u64) as usize];
                let runtime = self.runtime();
                self.rev += 1;
                self.model.slots[slot] = Some((runtime, self.rev));
                Op::Add {
                    slot,
                    runtime,
                    rev: self.rev,
                }
            }
            Kind::Remove => {
                let slot = used[self.rng.below(used.len() as u64) as usize];
                self.model.slots[slot] = None;
                Op::Remove { slot }
            }
            Kind::ChangeSlot => {
                let slot = used[self.rng.below(used.len() as u64) as usize];
                let runtime = self.runtime();
                self.rev += 1;
                self.model.slots[slot] = Some((runtime, self.rev));
                Op::ChangeSlot {
                    slot,
                    runtime,
                    rev: self.rev,
                }
            }
            Kind::ChangeMain => {
                let rows: Vec<&'static str> = self.model.revs.keys().copied().collect();
                let row = rows[self.rng.below(rows.len() as u64) as usize];
                self.rev += 1;
                self.model.revs.insert(row, self.rev);
                Op::ChangeMain { row, rev: self.rev }
            }
            Kind::Reject => Op::Reject(
                [Bad::Syntax, Bad::MissingPlugin, Bad::RuntimeSetting][self.rng.below(3) as usize],
            ),
            Kind::Cut => Op::Cut,
            Kind::Idle => Op::Idle(self.rng.below(2000)),
            Kind::Kill => Op::Kill([Runtime::Node, Runtime::Py][self.rng.below(2) as usize]),
        }
    }
}

/// How a host ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// Ctrl-C as a terminal sends it (Ctrl-Break on Windows).
    CtrlC,
    /// SIGTERM (Unix).
    Term,
    /// `kill -9` (TerminateProcess on Windows), after rows whose cleanup
    /// never returns were added.
    Kill,
}

impl End {
    fn all() -> &'static [End] {
        if cfg!(unix) {
            &[End::CtrlC, End::Term, End::Kill]
        } else {
            &[End::CtrlC, End::Kill]
        }
    }
}

/// How a host is told to read its configuration again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// SIGUSR1 (Unix).
    Signal,
    /// `--watch-config`.
    Watch,
}

/// Everything a run does, from the seed: the soak host's operations, how
/// it ends and is told to reload, and the same for each host after it.
struct Plan {
    seed: u64,
}

impl Plan {
    fn soak(&self) -> Planner {
        Planner::new(self.seed, true)
    }

    fn trigger(&self, host: u64) -> Trigger {
        let mut rng = Rng(self.seed ^ 0x5452_4947 ^ host);
        match cfg!(unix) && rng.below(2) == 0 {
            true => Trigger::Signal,
            false => Trigger::Watch,
        }
    }

    /// How the soak host ends; then one host for every way, in turn from
    /// the next.
    fn ends(&self) -> (End, Vec<End>) {
        let all = End::all();
        let first = (self.seed % all.len() as u64) as usize;
        let rest = (1..=all.len()).map(|n| all[(first + n) % all.len()]);
        (all[first], rest.collect())
    }

    /// The operations of the `n`th host after the soak host (from 1):
    /// four to seven, from a seed of their own.
    fn short(&self, n: u64) -> (Planner, usize) {
        let mut planner = Planner::new(self.seed.wrapping_mul(31).wrapping_add(n), false);
        let count = 4 + planner.rng.below(4) as usize;
        (planner, count)
    }
}

/// The same seed, the same operations; another seed, others.
#[test]
fn the_operations_are_a_function_of_the_seed() {
    let ops = |seed| {
        let mut planner = Plan { seed }.soak();
        (0..500).map(|_| planner.next()).collect::<Vec<_>>()
    };
    assert_eq!(ops(42), ops(42));
    assert_ne!(ops(42), ops(43));
    let all = ops(7);
    assert_eq!(
        all[..6],
        [
            Op::Reject(Bad::Syntax),
            Op::Reject(Bad::MissingPlugin),
            Op::Reject(Bad::RuntimeSetting),
            Op::Cut,
            Op::Kill(Runtime::Node),
            Op::Kill(Runtime::Py),
        ],
        "the soak host starts with one fault of each kind"
    );
    for kind in [
        "Call",
        "Add",
        "Remove",
        "ChangeSlot",
        "ChangeMain",
        "Reject",
        "Kill",
        "Cut",
        "Idle",
    ] {
        assert!(
            all.iter().any(|op| format!("{op:?}").starts_with(kind)),
            "no {kind} in 500 operations"
        );
    }
    let plan = Plan { seed: 5 };
    assert_eq!(plan.ends(), plan.ends());
    let (_, rest) = plan.ends();
    assert_eq!(rest.len(), End::all().len(), "every way of ending, once");
}

// ---------------------------------------------------------------------
// Samples and their bounds.

#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Seconds since the soak host started.
    at: f64,
    /// The host's processes, itself included.
    processes: u64,
    host: Usage,
    /// The host and everything it started.
    tree: Usage,
    /// Node B.
    peer: Usage,
}

impl Sample {
    const HEADER: &'static str = "seconds,processes,host_rss_kib,host_threads,host_fds,\
                                  tree_rss_kib,tree_threads,tree_fds,peer_rss_kib,peer_threads,peer_fds";

    fn csv(&self) -> String {
        let usage = |u: &Usage| format!("{},{},{}", u.rss_kib, u.threads, u.fds);
        format!(
            "{:.1},{},{},{},{}",
            self.at,
            self.processes,
            usage(&self.host),
            usage(&self.tree),
            usage(&self.peer)
        )
    }
}

/// How much a measure may grow from the early window of the soak to the
/// late one: the late median may be at most `early × ratio + slack`.
///
/// The windows: the samples of the first 10 % of the soak are its warm-up
/// (caches, JIT, the allocator's pools) and do not count; the early window
/// is the next 25 %, the late window the last 25 %. Medians, so that a
/// sample taken during one operation's transient (a runtime restarting, a
/// reload's new rows) does not decide. The process count may grow by one
/// at most: the Node runtime's TypeScript loader (tsx) runs an esbuild
/// service process while it compiles a file it has not cached, so after a
/// `kill -9` of the runtime it may come back or not. The others have room for what varies without leaking: tokio's
/// blocking threads come and go (they end after 10 s idle), V8 and the
/// allocators keep memory they freed; runtimes restart, so the tree's
/// memory is that of young processes in one window and older ones in the
/// other. What they still catch over the nightly hour (some 400
/// operations, the spacing of kills and cuts holding them back: about 200
/// reloads, 40 runtime kills, 45 cuts; the medians are about 0.6 of the run
/// apart): a descriptor per reload, per kill or per cut; a thread per
/// reload; about 100 KiB of the host's memory per operation.
#[derive(Debug, Clone, Copy)]
struct Bound {
    ratio: f64,
    slack: f64,
}

/// The bounds, per measure.
struct Bounds;

impl Bounds {
    /// Descriptors (handles on Windows): of the host, of B.
    const FDS: Bound = Bound {
        ratio: 1.2,
        slack: 10.0,
    };
    /// The tree's descriptors: runtimes restart, and the slot rows move
    /// between them.
    const TREE_FDS: Bound = Bound {
        ratio: 1.3,
        slack: 30.0,
    };
    const THREADS: Bound = Bound {
        ratio: 1.5,
        slack: 8.0,
    };
    const TREE_THREADS: Bound = Bound {
        ratio: 1.5,
        slack: 24.0,
    };
    /// KiB.
    const RSS: Bound = Bound {
        ratio: 1.3,
        slack: 16.0 * 1024.0,
    };
    /// KiB.
    const TREE_RSS: Bound = Bound {
        ratio: 1.5,
        slack: 64.0 * 1024.0,
    };
    const PROCESSES: Bound = Bound {
        ratio: 1.0,
        slack: 1.0,
    };
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = values.len();
    match n % 2 {
        1 => values[n / 2],
        _ => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

/// Check the samples of a soak of `seconds` against the bounds: what
/// grew too much, and one summary line per measure.
fn check_growth(samples: &[Sample], seconds: f64) -> (Vec<String>, Vec<String>) {
    let early: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.at >= 0.10 * seconds && s.at < 0.35 * seconds)
        .collect();
    let late: Vec<&Sample> = samples.iter().filter(|s| s.at >= 0.75 * seconds).collect();
    let mut grew = Vec::new();
    let mut summary = Vec::new();
    if early.len() < 3 || late.len() < 3 {
        summary.push(format!(
            "growth not checked: {} early and {} late samples (3 each needed)",
            early.len(),
            late.len()
        ));
        return (grew, summary);
    }
    type Measure = (&'static str, fn(&Sample) -> u64, Bound);
    let measures: [Measure; 10] = [
        ("processes", |s| s.processes, Bounds::PROCESSES),
        ("host fds", |s| s.host.fds, Bounds::FDS),
        ("host threads", |s| s.host.threads, Bounds::THREADS),
        ("host rss KiB", |s| s.host.rss_kib, Bounds::RSS),
        ("tree fds", |s| s.tree.fds, Bounds::TREE_FDS),
        ("tree threads", |s| s.tree.threads, Bounds::TREE_THREADS),
        ("tree rss KiB", |s| s.tree.rss_kib, Bounds::TREE_RSS),
        ("peer fds", |s| s.peer.fds, Bounds::FDS),
        ("peer threads", |s| s.peer.threads, Bounds::THREADS),
        ("peer rss KiB", |s| s.peer.rss_kib, Bounds::RSS),
    ];
    for (name, measure, bound) in measures {
        let before = median(early.iter().map(|s| measure(s) as f64).collect());
        let after = median(late.iter().map(|s| measure(s) as f64).collect());
        let limit = before * bound.ratio + bound.slack;
        let line = format!("{name}: {before} -> {after} (limit {limit:.0})");
        if after > limit {
            grew.push(line.clone());
        }
        summary.push(line);
    }
    (grew, summary)
}

/// Growth within the bounds passes; growth past one fails.
#[test]
fn growth_is_checked_against_the_bounds() {
    let usage = |n: u64| Usage {
        rss_kib: 100_000,
        threads: 20,
        fds: 50 + n,
    };
    let samples = |grow: u64| -> Vec<Sample> {
        (0..100)
            .map(|i| Sample {
                at: i as f64,
                processes: 4,
                host: usage(i * grow / 100),
                tree: usage(0),
                peer: usage(0),
            })
            .collect()
    };
    let (grew, _) = check_growth(&samples(10), 100.0);
    assert!(grew.is_empty(), "{grew:?}");
    let (grew, _) = check_growth(&samples(100), 100.0);
    assert_eq!(grew.len(), 1, "{grew:?}");
    assert!(grew[0].starts_with("host fds"), "{grew:?}");
    let (grew, summary) = check_growth(&samples(100)[..5], 100.0);
    assert!(grew.is_empty() && summary[0].starts_with("growth not checked"));
}

// ---------------------------------------------------------------------
// The run.

/// What a failure keeps: the seed, the operations done, the samples. On a
/// panic, written to the scenario's `logs/` with the command that replays
/// the run.
struct Record {
    seed: u64,
    dir: PathBuf,
    /// How many operations the soak host was given.
    soak_ops: RefCell<usize>,
    ops: RefCell<Vec<String>>,
    samples: RefCell<Vec<Sample>>,
}

impl Record {
    fn op(&self, line: String) {
        println!("soak-s8 op {line}");
        self.ops.borrow_mut().push(line);
    }

    fn replay(&self) -> String {
        format!(
            "RUTIS_S8_SEED={} RUTIS_S8_OPS={} cargo test -p rutis-e2e --test soak -- --ignored --nocapture",
            self.seed,
            self.soak_ops.borrow()
        )
    }

    fn samples_csv(&self) -> String {
        let mut csv = format!("{}\n", Sample::HEADER);
        for sample in self.samples.borrow().iter() {
            let _ = writeln!(csv, "{}", sample.csv());
        }
        csv
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let logs = self.dir.join("logs");
        let _ = std::fs::create_dir_all(&logs);
        let mut ops = format!("seed {}\n", self.seed);
        for op in self.ops.borrow().iter() {
            let _ = writeln!(ops, "{op}");
        }
        let _ = std::fs::write(logs.join("soak-ops.txt"), ops);
        let _ = std::fs::write(logs.join("soak-samples.csv"), self.samples_csv());
        eprintln!(
            "soak-s8: failed with seed {}; the operations and samples are in {}\nsoak-s8: replay: {}",
            self.seed,
            logs.display(),
            self.replay()
        );
    }
}

/// The project's fixed parts: probes, B, the proxy to it.
struct Project {
    probe: Probe,
    watch: Probe,
    /// Uses `infer`, on B.
    far: Probe,
    /// One per slot row: `p1` uses `x1`…; it runs while its slot does.
    slots: Vec<Probe>,
    proxy: Proxy,
    token: String,
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn setup(s: &Scenario) -> (Project, Host) {
    s.link_node_sdk();
    s.write("store.py", STORE);
    s.write("report.ts", REPORT);
    if BUN {
        s.link(
            "node_modules/@arcships/rutis-bun",
            &repo().join("bun/rutis-bun"),
        );
        s.write("audit.ts", AUDIT);
        s.write("stuck_bun.ts", STUCK_BUN);
    }
    s.write("stuck.ts", STUCK_NODE);
    s.write("stuck_py.py", STUCK_PY);
    for n in 1..=SLOTS {
        let name = format!("x{n}");
        s.write(format!("{name}.ts"), SLOT_NODE.replace("NAME", &name));
        s.write(format!("{name}_py.py"), SLOT_PY.replace("NAME", &name));
        if BUN {
            s.write(format!("{name}_bun.ts"), SLOT_BUN.replace("NAME", &name));
        }
    }
    let uses: &[&str] = if BUN {
        &["report", "audit"]
    } else {
        &["report"]
    };
    let probe = s.probe("probe", Lang::Node, uses);
    let watch = s.probe("watch", Lang::Node, &[]);
    let far = s.probe("far", Lang::Node, &["infer"]);
    let slots = (1..=SLOTS)
        .map(|n| s.probe(&format!("p{n}"), Lang::Node, &[&format!("x{n}")]))
        .collect();

    // Node B, listening for this project's host, through the proxy.
    s.write("gpu/infer.py", INFER);
    let token = s.secret();
    let port = s.port();
    let pythonpath = repo().join("python/rutis").display().to_string();
    let mut b = s.program(
        "",
        python(),
        [
            "-m".into(),
            "rutis".into(),
            format!("listen:ws://127.0.0.1:{port}/rutis"),
            "--id".into(),
            "gpu".into(),
            "--peer".into(),
            "main".into(),
            s.path("gpu").display().to_string(),
        ],
        &[
            ("RUTIS_TOKEN", token.as_str()),
            ("PYTHONPATH", pythonpath.as_str()),
        ],
    );
    b.expect("rutis: listening on");
    let proxy = s.proxy(SocketAddr::from(([127, 0, 0, 1], port)));
    (
        Project {
            probe,
            watch,
            far,
            slots,
            proxy,
            token,
        },
        b,
    )
}

impl Project {
    /// The configuration for `model`; `restart`: the Node and Python
    /// runtimes restart on failure; `bad`: an edit to reject.
    fn config(&self, s: &Scenario, model: &Model, restart: bool, bad: Option<Bad>) -> String {
        let rev = |row: &str| json!({ "rev": model.revs.get(row).copied().unwrap_or(0) });
        let policy = |restart: bool| match restart {
            true => json!({ "restart": "on-failure" }),
            false => json!({}),
        };
        // The changed setting: the other policy.
        let py_restart = restart != (bad == Some(Bad::RuntimeSetting));
        let mut runtimes = json!({
            "node": policy(restart),
            "py": policy(py_restart),
            "remote": [{ "name": "gpu", "language": "python" }],
        });
        let mut rows = vec![
            json!({ "id": "store", "name": "py:store", "config": rev("store") }),
            json!({ "id": "report", "name": "./report.ts", "config": rev("report") }),
        ];
        if BUN {
            runtimes["bun"] = json!({});
            rows.push(json!({ "id": "audit", "name": "bun:./audit.ts", "config": rev("audit") }));
        }
        rows.extend([self.probe.row(), self.watch.row(), self.far.row()]);
        rows.extend(self.slots.iter().map(Probe::row));
        rows.extend([
            json!({
                "id": "gpu",
                "name": "rutis-bridge/peer",
                "config": {
                    "peer": "gpu",
                    "dial": format!("ws://{}/rutis", self.proxy.addr()),
                    "runtime": "gpu",
                },
            }),
            json!({ "id": "infer", "name": "gpu:infer" }),
        ]);
        for (slot, row) in model.slots.iter().enumerate() {
            if let Some((runtime, rev)) = row {
                let id = format!("x{}", slot + 1);
                let name = match runtime {
                    Runtime::Node => format!("./{id}.ts"),
                    Runtime::Py => format!("py:{id}_py"),
                    Runtime::Bun => format!("bun:./{id}_bun.ts"),
                };
                rows.push(json!({ "id": id, "name": name, "config": { "rev": rev } }));
            }
        }
        if model.stuck {
            let marker = |name: &str| s.dir().join(format!("stuck-{name}")).display().to_string();
            rows.push(json!({ "id": "stucknode", "name": "./stuck.ts", "config": { "marker": marker("node") } }));
            rows.push(json!({ "id": "stuckpy", "name": "py:stuck_py", "config": { "marker": marker("py") } }));
            if BUN {
                rows.push(json!({ "id": "stuckbun", "name": "bun:./stuck_bun.ts", "config": { "marker": marker("bun") } }));
            }
        }
        if bad == Some(Bad::MissingPlugin) {
            rows.push(json!({ "id": "missing", "name": "./missing.ts" }));
        }
        let text = serde_json::to_string_pretty(&json!({
            "id": "main",
            "runtimes": runtimes,
            "rows": rows,
        }))
        .unwrap();
        match bad {
            // Cut at a character boundary: the paths in it may not be
            // ASCII.
            Some(Bad::Syntax) => {
                let mut half = text.len() / 2;
                while !text.is_char_boundary(half) {
                    half -= 1;
                }
                text[..half].to_owned()
            }
            _ => text,
        }
    }
}

/// One host of the run, and what it was told.
struct Run<'a> {
    s: &'a Scenario,
    project: &'a Project,
    record: &'a Record,
    host: Host,
    #[cfg_attr(windows, allow(dead_code))]
    trigger: Trigger,
    restart: bool,
    model: Model,
    /// When each runtime last started (the host's start, a restart).
    started: BTreeMap<Runtime, Instant>,
    /// What the host's output must agree with at its end.
    reloads: usize,
    rejects: usize,
    downs: BTreeMap<Runtime, usize>,
    restarts: BTreeMap<Runtime, usize>,
    cuts: usize,
    calls: u64,
    /// Events dispatched across runtimes and checked (#464).
    emits: u64,
    /// When the link to B last connected.
    connected: Instant,
}

impl<'a> Run<'a> {
    /// Start a host on the base configuration and wait until every row
    /// runs, the probes that use them have started and the link is up.
    fn start(
        s: &'a Scenario,
        project: &'a Project,
        record: &'a Record,
        trigger: Trigger,
        restart: bool,
    ) -> Self {
        let model = Model::new();
        s.replace("rutis.json", project.config(s, &model, restart, None));
        let timeout = SHUTDOWN.to_string();
        let mut args = vec!["run", "--shutdown-timeout", timeout.as_str()];
        if trigger == Trigger::Watch {
            args.push("--watch-config");
        }
        args.push("rutis.json");
        let mut host = s.host_with(args, &[("RUTIS_TOKEN_GPU", project.token.as_str())]);
        for row in model.revs.keys().chain(&["infer"]) {
            host.expect(&format!("{row}: running"));
        }
        host.expect("gpu: link connected");
        project.probe.started(&mut host);
        project.watch.started(&mut host);
        project.far.started(&mut host);
        let now = Instant::now();
        record.op(format!(
            "host {} started: {trigger:?}, restart on failure: {restart}",
            host.pid()
        ));
        Self {
            s,
            project,
            record,
            host,
            trigger,
            restart,
            model,
            started: Runtime::all().iter().map(|&rt| (rt, now)).collect(),
            reloads: 0,
            rejects: 0,
            downs: BTreeMap::new(),
            restarts: BTreeMap::new(),
            cuts: 0,
            calls: 0,
            emits: 0,
            connected: now,
        }
    }

    /// Write `text` as the configuration and have it read.
    fn reload(&mut self, text: String) {
        self.s.replace("rutis.json", text);
        #[cfg(unix)]
        if self.trigger == Trigger::Signal {
            self.host.signal(libc::SIGUSR1);
        }
    }

    /// The reload's result line.
    fn result(&mut self) -> String {
        self.host.wait_for("the reload's result", |line| {
            line.starts_with("rutis-host: reloaded ")
                || line.starts_with("rutis-host: reload rejected: ")
        })
    }

    /// Reload `model`: accepted, with `changed` rows changed.
    fn apply(&mut self, model: Model, changed: usize) {
        let text = self.project.config(self.s, &model, self.restart, None);
        self.model = model;
        self.reload(text);
        let result = self.result();
        let rows = match changed {
            1 => "1 row changed".to_owned(),
            n => format!("{n} rows changed"),
        };
        assert_eq!(
            result,
            format!("rutis-host: reloaded rutis.json ({rows})"),
            "the reload"
        );
        self.reloads += 1;
    }

    /// The slot row's probe answers for it, at its revision.
    fn slot(&mut self, slot: usize) {
        let Some((runtime, rev)) = self.model.slots[slot] else {
            return;
        };
        let id = format!("x{}", slot + 1);
        assert_eq!(
            self.project.slots[slot].call(&mut self.host, &id, "ping", json!([])),
            Ok(json!(format!("{id} {} rev {rev}", runtime.name())))
        );
    }

    /// Events (#464): `t/slot/x<n>` from Node (`report`) and from the
    /// remote runtime (`infer`) reaches the slot's listener as configured
    /// now, or nothing when the slot is empty.
    fn slot_events(&mut self, slot: usize) {
        let id = format!("x{}", slot + 1);
        let expected = match self.model.slots[slot] {
            Some((runtime, rev)) => json!(format!("{id} {} rev {rev}", runtime.name())),
            None => json!(null),
        };
        let host = &mut self.host;
        assert_eq!(
            self.project.probe.call(host, "report", "ask", json!([id])),
            Ok(expected.clone()),
            "serial of t/slot/{id} from the Node runtime"
        );
        assert_eq!(
            self.project.far.call(host, "infer", "ask", json!([id])),
            Ok(expected),
            "serial of t/slot/{id} from the remote runtime"
        );
        self.emits += 2;
    }

    /// Calls through every row.
    fn calls(&mut self) {
        let host = &mut self.host;
        let probe = &self.project.probe;
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
        self.calls += 1;
        let job = format!("job-{}", self.calls);
        assert_eq!(
            self.project.far.call(host, "infer", "run", json!([job])),
            Ok(json!(format!("inferred {job}")))
        );
        for slot in 0..SLOTS {
            self.slot(slot);
            self.slot_events(slot);
        }
    }

    /// `kill -9` the processes of `runtime`.
    fn kill(&mut self, runtime: Runtime) {
        let processes = self.s.descendants(&self.host);
        let pids: Vec<u32> = processes
            .iter()
            .filter(|(_, command)| runtime.runs(command))
            .map(|(pid, _)| *pid)
            .collect();
        assert!(
            !pids.is_empty(),
            "no {} runtime among the host's processes: {processes:?}",
            runtime.name()
        );
        self.host.skip_written();
        for pid in pids {
            self.s.kill(&self.host, pid);
        }
        let down = self
            .host
            .expect(&format!("runtime {}: down (", runtime.name()));
        assert!(down.contains("exited with"), "{down}");
        *self.downs.entry(runtime).or_default() += 1;
    }

    /// The probes that start again once `runtime`'s rows run again.
    fn probes_back(&mut self, runtime: Runtime) {
        let host = &mut self.host;
        let project = self.project;
        if runtime == Runtime::Node {
            // They run in it: killed with it, they print no `stopped`.
            project.probe.started(host);
            project.watch.started(host);
            project.far.started(host);
            for (slot, row) in self.model.slots.iter().enumerate() {
                if row.is_some() {
                    project.slots[slot].started(host);
                }
            }
        } else {
            project.probe.event(host, "stopped");
            project.probe.started(host);
            for (slot, row) in self.model.slots.iter().enumerate() {
                if row.is_some_and(|(on, _)| on == runtime) {
                    project.slots[slot].event(host, "stopped");
                    project.slots[slot].started(host);
                }
            }
        }
    }

    fn execute(&mut self, op: &Op, sampler: &mut Sampler) {
        match *op {
            Op::Call => self.calls(),
            Op::Add { slot, runtime, rev } | Op::ChangeSlot { slot, runtime, rev } => {
                let mut model = self.model.clone();
                model.slots[slot] = Some((runtime, rev));
                let added = self.model.slots[slot].is_none();
                self.host.skip_written();
                self.apply(model, 1);
                let probe = &self.project.slots[slot];
                if added {
                    self.host.expect(&format!("x{}: running", slot + 1));
                } else {
                    // It restarts: its probe stops, and starts again.
                    probe.event(&mut self.host, "stopped");
                }
                probe.started(&mut self.host);
                self.slot(slot);
            }
            Op::Remove { slot } => {
                let mut model = self.model.clone();
                model.slots[slot] = None;
                self.host.skip_written();
                self.apply(model, 1);
                self.host.expect(&format!("x{}: removed", slot + 1));
                self.project.slots[slot].event(&mut self.host, "stopped");
            }
            Op::ChangeMain { row, rev } => {
                let mut model = self.model.clone();
                model.revs.insert(row, rev);
                self.host.skip_written();
                self.apply(model, 1);
                // What uses it, the probe last, waits and comes back.
                self.project.probe.event(&mut self.host, "stopped");
                self.project.probe.started(&mut self.host);
            }
            Op::Reject(bad) => {
                let text = self
                    .project
                    .config(self.s, &self.model, self.restart, Some(bad));
                self.reload(text);
                let result = self.result();
                assert!(
                    result.starts_with("rutis-host: reload rejected: ")
                        && result.ends_with("; nothing changed"),
                    "{bad:?}: {result}"
                );
                self.rejects += 1;
                // Put back: read again, nothing to change.
                let model = self.model.clone();
                self.apply(model, 0);
            }
            Op::Kill(runtime) => {
                if let Some(wait) = KILL_SPACING.checked_sub(self.started[&runtime].elapsed()) {
                    sampler.idle(self, wait);
                }
                self.kill(runtime);
                self.host
                    .expect(&format!("runtime {}: restarting", runtime.name()));
                self.host
                    .expect(&format!("runtime {}: running", runtime.name()));
                *self.restarts.entry(runtime).or_default() += 1;
                self.started.insert(runtime, Instant::now());
                self.probes_back(runtime);
            }
            Op::Cut => {
                if let Some(wait) = CUT_SPACING.checked_sub(self.connected.elapsed()) {
                    sampler.idle(self, wait);
                }
                self.host.skip_written();
                self.project.proxy.cut();
                self.host.expect("gpu: link down (");
                self.host.expect("infer: waiting for its services");
                self.project.far.event(&mut self.host, "stopped");
                self.project.proxy.restore();
                self.host.expect("gpu: link connected");
                self.host.expect("infer: running");
                self.project.far.started(&mut self.host);
                self.connected = Instant::now();
                self.cuts += 1;
            }
            Op::Idle(ms) => sampler.idle(self, Duration::from_millis(ms)),
        }
    }

    /// The Python runtime killed for good (no restart policy): it stays
    /// down, its rows and what uses them wait, calls into it fail
    /// explicitly.
    fn kill_for_good(&mut self) {
        assert!(!self.restart);
        self.kill(Runtime::Py);
        self.host.expect("store: waiting for its services");
        for (slot, row) in self.model.slots.iter().enumerate() {
            if let Some((Runtime::Py, _)) = row {
                self.host
                    .expect(&format!("x{}: waiting for its services", slot + 1));
                self.project.slots[slot].event(&mut self.host, "stopped");
            }
        }
        self.project.probe.event(&mut self.host, "stopped");
        for (service, method) in [("store", "get"), ("report", "summary")] {
            let error = self
                .project
                .watch
                .call(&mut self.host, service, method, json!(["greeting"]))
                .unwrap_err();
            assert_eq!(error, format!("service {service} is not available"));
        }
        self.record.op("python runtime killed for good".into());
    }

    /// End the host as `end` says; then no process of it is left within
    /// the bound, and its output agrees with what was done.
    fn end(mut self, end: End) {
        if end == End::Kill {
            let mut model = self.model.clone();
            model.stuck = true;
            let stuck = if BUN { 3 } else { 2 };
            self.apply(model, stuck);
            for row in ["stucknode", "stuckpy", "stuckbun"].iter().take(stuck) {
                self.host.expect(&format!("{row}: running"));
            }
        }
        self.record
            .op(format!("host {} ends: {end:?}", self.host.pid()));
        let ended = Instant::now();
        match end {
            End::CtrlC | End::Term => {
                if end == End::CtrlC {
                    self.host.ctrl_c();
                } else {
                    #[cfg(unix)]
                    self.host.signal(libc::SIGTERM);
                }
                let status = self.host.wait_exit();
                assert_eq!(status.code(), Some(0), "after {end:?}: {status}");
            }
            End::Kill => {
                self.host.kill();
                self.host.wait_exit();
            }
        }
        // No process of the host is left, within the bound.
        let bound = exit_bound();
        loop {
            let left = self.s.descendants(&self.host);
            if left.is_empty() {
                break;
            }
            assert!(
                ended.elapsed() < bound,
                "after {end:?}, the host's processes still run after {bound:?}: {left:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let took = ended.elapsed();
        println!("soak-s8: host ended by {end:?}; its processes gone after {took:.1?}");
        // Unix: each runtime unloaded its rows by itself (the stuck
        // cleanups started) and kept the deadline before giving them up.
        // Windows: the host's job ended them, no cleanup ran.
        #[cfg(unix)]
        if end == End::Kill {
            for runtime in Runtime::all() {
                let marker = self.s.dir().join(format!("stuck-{}", runtime.name()));
                assert_eq!(
                    std::fs::read_to_string(&marker).ok().as_deref(),
                    Some("cleaned up"),
                    "the {} runtime did not unload its rows once its host was killed",
                    runtime.name()
                );
                let _ = std::fs::remove_file(marker);
            }
            assert!(
                took >= Duration::from_secs(SHUTDOWN) - Duration::from_millis(500),
                "the runtimes gave up their stuck cleanups after {took:?}, before the deadline ({SHUTDOWN} s)"
            );
        }
        self.host.wait_output_end();
        self.agrees();
    }

    /// The host's output agrees with what was done.
    fn agrees(&self) {
        let host = &self.host;
        let count = |text: &str| host.count(text);
        let mut wrong = Vec::new();
        let mut expect = |what: String, found: usize, expected: usize| {
            if found != expected {
                wrong.push(format!("{what}: {found}, expected {expected}"));
            }
        };
        expect(
            "reloads accepted".into(),
            count("rutis-host: reloaded "),
            self.reloads,
        );
        expect(
            "reloads rejected".into(),
            count("rutis-host: reload rejected: "),
            self.rejects,
        );
        for runtime in Runtime::all() {
            let name = runtime.name();
            let downs = self.downs.get(runtime).copied().unwrap_or(0);
            let restarts = self.restarts.get(runtime).copied().unwrap_or(0);
            expect(
                format!("runtime {name}: down"),
                count(&format!("runtime {name}: down (")),
                downs,
            );
            expect(
                format!("runtime {name}: restarting"),
                count(&format!("runtime {name}: restarting")),
                restarts,
            );
            expect(
                format!("runtime {name}: running"),
                count(&format!("runtime {name}: running")),
                restarts,
            );
        }
        expect(
            "links connected".into(),
            count("gpu: link connected"),
            self.cuts + 1,
        );
        expect("runtimes given up".into(), count(": gave up after"), 0);
        let failed: Vec<String> = host
            .lines()
            .into_iter()
            .map(|line| line.text)
            .filter(|line| line.contains(": failed"))
            .collect();
        expect(format!("rows failed {failed:?}"), failed.len(), 0);
        assert!(
            wrong.is_empty(),
            "host {}: its output does not agree with what was done:\n  {}",
            host.pid(),
            wrong.join("\n  ")
        );
    }
}

/// Samples every `interval`, at the ends of operations (when what they
/// started has settled) and while idle.
struct Sampler {
    started: Instant,
    interval: Duration,
    last: Option<Instant>,
    peer: u32,
}

impl Sampler {
    fn maybe(&mut self, run: &Run) {
        if self.last.is_some_and(|last| last.elapsed() < self.interval) {
            return;
        }
        self.last = Some(Instant::now());
        let tree: Vec<u32> = std::iter::once(run.host.pid())
            .chain(run.s.descendants(&run.host).into_iter().map(|(pid, _)| pid))
            .collect();
        let read = sample(&tree);
        let host = read
            .iter()
            .find(|(pid, _)| *pid == run.host.pid())
            .map(|(_, usage)| *usage)
            .expect("the host can be read");
        let total = read
            .iter()
            .fold(Usage::default(), |total, (_, usage)| total + *usage);
        let peer = sample(&[self.peer])
            .first()
            .map(|(_, usage)| *usage)
            .expect("node B can be read");
        run.record.samples.borrow_mut().push(Sample {
            at: self.started.elapsed().as_secs_f64(),
            processes: tree.len() as u64,
            host,
            tree: total,
            peer,
        });
    }

    /// Wait `wait`, sampling meanwhile.
    fn idle(&mut self, run: &Run, wait: Duration) {
        let until = Instant::now() + wait;
        loop {
            self.maybe(run);
            let now = Instant::now();
            if now >= until {
                return;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(500)));
        }
    }
}

/// A knob: unset is `None`; set, it must be a number.
fn env(name: &str) -> Option<u64> {
    let value = std::env::var(name).ok()?;
    Some(
        value
            .parse()
            .unwrap_or_else(|e| panic!("{name}={value:?} is not a number: {e}")),
    )
}

/// risk: L1, L3, L4, B3 (runtimes outlive a killed host), C3, B4
/// guarantee: host/killed-host-leaves-no-runtimes, host/stop-on-signal,
/// host/reload-config, host/runtime-restart, host/runtime-state-lines,
/// host/row-state-lines
#[test]
#[ignore = "soak (S8): run with --ignored; RUTIS_S8_SECS sets the duration"]
fn a_long_running_host_holds_no_more_and_every_end_leaves_nothing() {
    let seed = env("RUTIS_S8_SEED").unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    });
    let ops_limit = env("RUTIS_S8_OPS").map(|ops| ops as usize);
    let seconds = env("RUTIS_S8_SECS").unwrap_or(150);
    let plan = Plan { seed };
    let (soak_end, ends) = plan.ends();
    match ops_limit {
        Some(ops) => println!("soak-s8: seed {seed}, {ops} operations (replay)"),
        None => println!("soak-s8: seed {seed}, {seconds} s"),
    }

    let s = Scenario::new("soak");
    let record = Record {
        seed,
        dir: s.dir().to_owned(),
        soak_ops: RefCell::new(0),
        ops: RefCell::new(Vec::new()),
        samples: RefCell::new(Vec::new()),
    };
    let (project, mut b) = setup(&s);

    // 1. The soak host.
    let mut run = Run::start(&s, &project, &record, plan.trigger(0), true);
    let mut sampler = Sampler {
        started: Instant::now(),
        // About a hundred samples, every 2 to 30 s.
        interval: Duration::from_secs(seconds / 100)
            .clamp(Duration::from_secs(2), Duration::from_secs(30)),
        last: None,
        peer: b.pid(),
    };
    let mut planner = plan.soak();
    let soak = Duration::from_secs(seconds);
    let mut done = 0;
    loop {
        match ops_limit {
            Some(limit) if done >= limit => break,
            None if sampler.started.elapsed() >= soak => break,
            _ => {}
        }
        let op = planner.next();
        done += 1;
        *record.soak_ops.borrow_mut() = done;
        record.op(format!("{done}\t{op:?}"));
        run.execute(&op, &mut sampler);
        sampler.maybe(&run);
    }
    let elapsed = sampler.started.elapsed().as_secs_f64();
    let (grew, summary) = check_growth(&record.samples.borrow(), elapsed);
    println!(
        "soak-s8: seed {seed}: {done} operations in {elapsed:.0} s, {} samples; {} reloads, {} rejected, {} kills, {} cuts, {} events checked",
        record.samples.borrow().len(),
        run.reloads,
        run.rejects,
        run.restarts.values().sum::<usize>(),
        run.cuts,
        run.emits
    );
    for line in &summary {
        println!("soak-s8: {line}");
    }
    // A replay (`RUTIS_S8_OPS`) repeats operations, however long they
    // take; a timed run must have checked growth and done every fault.
    if ops_limit.is_none() {
        assert!(
            !summary[0].starts_with("growth not checked"),
            "{}: set RUTIS_S8_SECS higher (150 by default)",
            summary[0]
        );
        // The opening operations did each fault (Planner).
        let killed = |runtime| run.restarts.get(&runtime).copied().unwrap_or(0);
        assert!(
            killed(Runtime::Node) > 0
                && killed(Runtime::Py) > 0
                && run.cuts > 0
                && run.rejects >= 3,
            "not every fault was done: restarts {:?}, {} cuts, {} rejected edits",
            run.restarts,
            run.cuts,
            run.rejects
        );
    }
    assert!(
        grew.is_empty(),
        "resources grew over the soak:\n  {}",
        grew.join("\n  ")
    );
    run.end(soak_end);

    // 2. A host for every way of ending.
    for (n, end) in ends.into_iter().enumerate() {
        let n = n as u64 + 1;
        let mut run = Run::start(&s, &project, &record, plan.trigger(n), false);
        let (mut planner, count) = plan.short(n);
        for i in 1..=count {
            let op = planner.next();
            record.op(format!("host {n}.{i}\t{op:?}"));
            run.execute(&op, &mut sampler);
        }
        if end != End::Kill {
            run.kill_for_good();
        }
        run.end(end);
    }

    b.kill();
    b.wait_exit();
    s.finish();
}
