//! Synchronous waits across sessions, and the cycles they can close
//! (multi-language design §9, policy B; the model is
//! `specs/cross-runtime-sync/CrossRuntimeSync.tla`, `RustSucc`).
//!
//! A *hop* is a call this process forwarded from one session to another on
//! behalf of a caller that waits for it synchronously (its call frame says
//! `sync`). Before each such forward, the wait graph is built from the hops
//! in progress; a forward that would close a cycle is not sent, and its
//! caller gets `SyncWaitCycle` instead.
//!
//! An edge c → d means c cannot finish before d does:
//! - chain: d was made under c (d's chain names c);
//! - put aside: c entered a non-reentrant runtime y that has calls out, none
//!   of them on c's chain, and c has not started there: y runs c only once
//!   its stack is empty, so c waits for every call y has out;
//! - stack order: c and then d entered the same runtime y and d has started
//!   there (a call out of y names d): y's stack is LIFO, so c waits for d.
//!   Had c returned first, its reply would have been read before any frame
//!   made under d.
//!
//! The last two hold only for a runtime with one thread and one stack (it
//! declared `sync-stack`); its reentrancy is `reentrant-sync`, and a
//! runtime that does not declare it is taken as non-reentrant.
//!
//! An *entry* is any other call sent while some call is marked: nobody
//! waits for it synchronously (an `emit`'s `events.call`, an asynchronous
//! call). It is a boundary of chains: it waits for the synchronous calls
//! made under it, but what it was made under does not wait for them (an
//! `events.call` carries its emitter's chain). In a runtime with one stack
//! a reentrant runtime runs it on top of the call that waits there, so
//! stack order holds for it like for a hop. A session whose far end does not mark its
//! synchronous waits makes no hops: it is outside the check.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

/// A call in this process: a session's tag and the call's id on it.
type Global = (String, String);

/// What a session's far end does while it waits synchronously.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Profile {
    /// One thread, one stack: nested calls run on top of the waiting one.
    pub stacked: bool,
    /// While waiting, it runs every incoming call, not only its chain's.
    pub reentrant: bool,
}

#[derive(Clone, Debug)]
struct Hop {
    /// The session the forwarded call came from, and its id there.
    src: String,
    sid: String,
    /// The session it was forwarded to, its id there and that id's number.
    dst: String,
    did: String,
    seq: u64,
    /// The forwarded call's chain, the call itself last.
    chain: Vec<Global>,
    /// An entry: no caller waits for it (`src` and `sid` are empty).
    entry: bool,
}

impl Hop {
    fn gid(&self) -> Global {
        (self.dst.clone(), self.did.clone())
    }
}

#[derive(Default)]
struct Waits {
    /// Hops and entries in progress, by the call's id where it went.
    hops: HashMap<Global, Hop>,
    /// Calls received with `sync`, not answered yet.
    marked: std::collections::HashSet<Global>,
    profiles: HashMap<String, Profile>,
}

static WAITS: LazyLock<Mutex<Waits>> = LazyLock::new(Mutex::default);

/// How many hops and marked calls the table holds, readable without its
/// lock: a host where no call is marked never takes it on a reply.
static HOPS: AtomicUsize = AtomicUsize::new(0);
static MARKS: AtomicUsize = AtomicUsize::new(0);

impl Waits {
    /// After a change, under the lock.
    fn counted(&self) {
        HOPS.store(self.hops.len(), Ordering::SeqCst);
        MARKS.store(self.marked.len(), Ordering::SeqCst);
    }
}

/// `session`'s far end, once it greeted.
pub(super) fn profile(session: &str, profile: Profile) {
    WAITS
        .lock()
        .unwrap()
        .profiles
        .insert(session.to_owned(), profile);
}

/// A call `id` received on `session` whose caller waits synchronously.
pub(super) fn marked(session: &str, id: &str) {
    let mut waits = WAITS.lock().unwrap();
    waits.marked.insert((session.to_owned(), id.to_owned()));
    waits.counted();
}

/// Call `id` received on `session` was answered.
pub(super) fn answered(session: &str, id: &str) {
    if MARKS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut waits = WAITS.lock().unwrap();
    waits.marked.remove(&(session.to_owned(), id.to_owned()));
    waits.counted();
}

/// The reply to call `id` that this side sent on `session` was read (or the
/// call was cancelled): it waits for nothing any more.
pub(super) fn returned(session: &str, id: &str) {
    if HOPS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut waits = WAITS.lock().unwrap();
    waits.hops.remove(&(session.to_owned(), id.to_owned()));
    waits.counted();
}

/// `session` ended: its calls wait for nothing, and nothing waits for them.
pub(super) fn ended(session: &str) {
    let mut waits = WAITS.lock().unwrap();
    waits
        .hops
        .retain(|_, hop| hop.src != session && hop.dst != session);
    waits.marked.retain(|(tag, _)| tag != session);
    waits.profiles.remove(session);
    waits.counted();
}

/// The chain `path` (as sent on session `dst`, see `rebase`) names, as
/// calls of this process: untagged entries are `dst`'s, tagged ones
/// (`s1/node:3`) the tagged session's.
fn decode(path: &[String], dst: &str) -> Vec<Global> {
    path.iter()
        .map(|entry| match entry.split_once('/') {
            Some((tag, id)) => (tag.to_owned(), id.to_owned()),
            None => (dst.to_owned(), entry.clone()),
        })
        .collect()
}

/// Record that call `did` (number `seq`) on session `dst` is being sent
/// with chain `path`. When the chain's last call came from another session
/// and its caller waits synchronously, the call is a hop: if it would close
/// a wait cycle, nothing is recorded and the cycle is returned, each call as
/// `session/id`. When nobody waits for it synchronously (the chain's last
/// call is not marked), it is an entry. Returns whether either was recorded.
pub(super) fn forward(
    dst: &str,
    did: &str,
    seq: u64,
    path: &[String],
) -> Result<bool, Vec<String>> {
    if MARKS.load(Ordering::SeqCst) == 0 {
        return Ok(false);
    }
    let mut waits = WAITS.lock().unwrap();
    let mut chain = decode(path, dst);
    let caller = chain
        .last()
        .cloned()
        .filter(|caller| waits.marked.contains(caller));
    chain.push((dst.to_owned(), did.to_owned()));
    let hop = match caller {
        // A synchronous call back into the session it came from: on its
        // caller's chain, which runs it; not recorded.
        Some((src, _)) if src == dst => return Ok(false),
        Some((src, sid)) => Hop {
            src,
            sid,
            dst: dst.to_owned(),
            did: did.to_owned(),
            seq,
            chain,
            entry: false,
        },
        // Nobody waits for it: a boundary of the chains made under it, and,
        // in a runtime with one stack, a call stack order holds for.
        None => Hop {
            src: String::new(),
            sid: String::new(),
            dst: dst.to_owned(),
            did: did.to_owned(),
            seq,
            chain,
            entry: true,
        },
    };
    if !hop.entry {
        if let Some(cycle) = cycle_among(waits.hops.values(), &hop, &waits.profiles) {
            return Err(cycle);
        }
    }
    waits.hops.insert(hop.gid(), hop);
    waits.counted();
    Ok(true)
}

/// The cycle that `new` would close among `hops`, if any.
#[cfg(test)]
fn cycle(hops: &[Hop], new: &Hop, profiles: &HashMap<String, Profile>) -> Option<Vec<String>> {
    cycle_among(hops.iter(), new, profiles)
}

/// A call of this process, borrowed.
type Gid<'a> = (&'a str, &'a str);

fn gid_of(hop: &Hop) -> Gid<'_> {
    (&hop.dst, &hop.did)
}

/// The cycle that `new` would close among `hops`, if any. Linear in the
/// calls and the lengths of their chains, but for the put-aside edges (each
/// hop into a non-reentrant runtime, for each call out of it).
fn cycle_among<'a>(
    hops: impl Iterator<Item = &'a Hop>,
    new: &'a Hop,
    profiles: &HashMap<String, Profile>,
) -> Option<Vec<String>> {
    // In a stable order (session, then sequence), `new` last.
    let mut all: Vec<&Hop> = hops.collect();
    all.sort_unstable_by(|a, b| (&a.dst, a.seq).cmp(&(&b.dst, b.seq)));
    all.push(new);
    let last = all.len() - 1;
    let index: HashMap<Gid, usize> = all
        .iter()
        .enumerate()
        .map(|(i, h)| (gid_of(h), i))
        .collect();
    let is_entry = |gid: &Gid| index.get(gid).is_some_and(|&i| all[i].entry);
    // Started where it went: a call out of that session names it.
    let mut named: std::collections::HashSet<Gid> = std::collections::HashSet::new();
    for hop in all.iter().filter(|hop| !hop.entry) {
        for (session, id) in &hop.chain {
            if *session == hop.src {
                named.insert((session, id));
            }
        }
    }
    let started: Vec<bool> = all.iter().map(|hop| named.contains(&gid_of(hop))).collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); all.len()];
    // Chain: c waits for a hop d made under it, unless an entry is between
    // them (nobody waits for an entry; it waits for what is made under it).
    for (d, hop) in all.iter().enumerate() {
        if hop.entry {
            continue;
        }
        for (session, id) in hop.chain[..hop.chain.len() - 1].iter().rev() {
            let gid = (session.as_str(), id.as_str());
            if let Some(&c) = index.get(&gid) {
                successors[c].push(d);
            }
            if is_entry(&gid) {
                break;
            }
        }
    }
    // By runtime: the calls into it in order, and the hops out of it.
    let mut into: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut out: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, hop) in all.iter().enumerate() {
        into.entry(&hop.dst).or_default().push(i);
        if !hop.entry {
            out.entry(&hop.src).or_default().push(i);
        }
    }
    for (y, calls) in &mut into {
        let profile = profiles.get(*y).copied().unwrap_or_default();
        if !profile.stacked {
            continue;
        }
        calls.sort_unstable_by_key(|&i| all[i].seq);
        // Put aside: a hop not started in a non-reentrant runtime with calls
        // out, none of them on its chain, waits for all of them. (An entry
        // not started has nothing waiting for it.)
        if let Some(outs) = out.get(y).filter(|_| !profile.reentrant) {
            let sids: std::collections::HashSet<&str> =
                outs.iter().map(|&o| all[o].sid.as_str()).collect();
            for &h in calls.iter() {
                let hop = all[h];
                let related = hop
                    .chain
                    .iter()
                    .any(|(session, id)| session == y && sids.contains(id.as_str()));
                if !hop.entry && !related && !started[h] {
                    successors[h].extend(outs.iter().copied().filter(|&o| o != h));
                }
            }
        }
        // Stack order: c waits for every later call into y that started
        // there; through the next one that did, which waits for the rest.
        let mut next_started = None;
        for &c in calls.iter().rev() {
            if let Some(d) = next_started {
                successors[c].push(d);
            }
            if started[c] {
                next_started = Some(c);
            }
        }
    }
    // Depth-first from `new`, remembering how each call was reached.
    let mut from: Vec<Option<usize>> = vec![None; all.len()];
    let mut stack = vec![last];
    let mut seen = vec![false; all.len()];
    while let Some(h) = stack.pop() {
        for &next in &successors[h] {
            if next == last {
                let mut names = vec![name(all[last])];
                let mut at = h;
                let mut back = Vec::new();
                while at != last {
                    back.push(name(all[at]));
                    at = from[at].expect("reached from new");
                }
                names.extend(back.into_iter().rev());
                return Some(names);
            }
            if !seen[next] {
                seen[next] = true;
                from[next] = Some(h);
                stack.push(next);
            }
        }
    }
    None
}

fn name(hop: &Hop) -> String {
    format!("{}/{}", hop.dst, hop.did)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(src: &str, sid: &str, dst: &str, seq: u64, path: &[&str]) -> Hop {
        let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let did = format!("rust:{seq}");
        let mut chain = decode(&path, dst);
        chain.push((dst.into(), did.clone()));
        Hop {
            src: src.into(),
            sid: sid.into(),
            dst: dst.into(),
            did,
            seq,
            chain,
            entry: false,
        }
    }

    fn entry(dst: &str, seq: u64, path: &[&str]) -> Hop {
        let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let did = format!("rust:{seq}");
        let mut chain = decode(&path, dst);
        chain.push((dst.into(), did.clone()));
        Hop {
            src: String::new(),
            sid: String::new(),
            dst: dst.into(),
            did,
            seq,
            chain,
            entry: true,
        }
    }

    /// A reentrant runtime runs an emit's listener (an entry: nobody waits
    /// for it) on top of a call that waits there; the listener's call into
    /// a non-reentrant runtime waiting on that call closes a cycle.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_listener_run_on_top_of_a_wait() {
        let p = profiles(&[("c", true, false), ("a", true, true), ("b", false, false)]);
        // c's node:1 → a (x, rust:1); x calls b (slow).
        let x = hop("c", "node:1", "a", 1, &["c/node:1"]);
        let slow = hop("a", "py:1", "b", 1, &["c/node:1", "a/rust:1", "a/py:1"]);
        // The host's events.call to a (rust:2), run on top of x; its
        // listener calls c (y).
        let listener = entry("a", 2, &[]);
        let y = hop("a", "py:2", "c", 1, &["a/rust:2", "a/py:2"]);
        let found = cycle(&[x.clone(), slow.clone(), listener.clone()], &y, &p).expect("a cycle");
        assert_eq!(found, ["c/rust:1", "a/rust:1", "a/rust:2"]);
        // In a non-reentrant runtime the listener has not started (it waits
        // for x): no call names it, and the call it makes later is no cycle.
        let p = profiles(&[("c", true, false), ("a", true, false), ("b", false, false)]);
        assert_eq!(
            cycle(
                &[x.clone(), slow.clone(), listener.clone()],
                &hop("c", "node:2", "b", 2, &["c/node:2"]),
                &p
            ),
            None
        );
        // Nothing waits for an entry by its chain: an emit made under x
        // whose listener runs elsewhere and waits for x does not wait for x
        // to return.
        let p = profiles(&[("c", true, false), ("a", true, true), ("d", true, true)]);
        let emitted = entry("d", 1, &["c/node:1", "a/rust:1", "a/py:1"]);
        let back = hop(
            "d",
            "js:1",
            "a",
            3,
            &["c/node:1", "a/rust:1", "a/py:1", "d/rust:1", "d/js:1"],
        );
        assert_eq!(cycle(&[x, emitted], &back, &p), None);
    }

    /// A forward whose chain ends at a marked call is a hop even when the
    /// forwarded call is asynchronous (a relayed function reference): the
    /// caller still waits for the relayed call to settle.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn an_asynchronous_forward_on_a_marked_chain_is_a_hop() {
        marked("hop_a", "node:1");
        let recorded = forward("hop_b", "rust:1", 1, &["hop_a/node:1".to_owned()]).unwrap();
        assert!(recorded);
        let entry = WAITS
            .lock()
            .unwrap()
            .hops
            .get(&("hop_b".to_owned(), "rust:1".to_owned()))
            .map(|hop| hop.entry);
        assert_eq!(entry, Some(false));
        returned("hop_b", "rust:1");
        answered("hop_a", "node:1");
        ended("hop_a");
        ended("hop_b");
    }

    fn profiles(list: &[(&str, bool, bool)]) -> HashMap<String, Profile> {
        list.iter()
            .map(|(tag, stacked, reentrant)| {
                (
                    tag.to_string(),
                    Profile {
                        stacked: *stacked,
                        reentrant: *reentrant,
                    },
                )
            })
            .collect()
    }

    /// Counterexample 1: two non-reentrant runtimes call each other.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn two_node_runtimes_calling_each_other() {
        let p = profiles(&[("n1", true, false), ("n2", true, false)]);
        // n1's node:1 is forwarded to n2 as rust:1; n2's node:1 to n1.
        let first = hop("n1", "node:1", "n2", 1, &["n1/node:1"]);
        let second = hop("n2", "node:1", "n1", 1, &["n2/node:1"]);
        assert_eq!(cycle(&[], &first, &p), None);
        let found = cycle(&[first], &second, &p).expect("a cycle");
        assert_eq!(found, ["n1/rust:1", "n2/rust:1"]);
    }

    /// Counterexample 2: stack order on a reentrant runtime.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn stack_order_on_a_reentrant_runtime() {
        let p = profiles(&[("n", true, false), ("p1", true, true), ("p2", true, true)]);
        // 1. n's node:1 → p2 (s1, rust:1).
        let s1 = hop("n", "node:1", "p2", 1, &["n/node:1"]);
        // 2. s1 (on p2, as rust:1) calls p1: p2's node:1 → p1 (rust:1).
        let under = hop(
            "p2",
            "node:1",
            "p1",
            1,
            &["n/node:1", "p2/rust:1", "p2/node:1"],
        );
        // 3. p1 on its own: p1's node:1 → p2 (s2, rust:2), nested above s1.
        let s2 = hop("p1", "node:1", "p2", 2, &["p1/node:1"]);
        // 4. s2 calls n: p2's node:2 → n (rust:1). n waits for s1 and puts
        //    it aside; s1 is buried under s2, which waits for n.
        let back = hop(
            "p2",
            "node:2",
            "n",
            1,
            &["p1/node:1", "p2/rust:2", "p2/node:2"],
        );
        let hops = vec![s1, under, s2];
        let found = cycle(&hops, &back, &p).expect("a cycle");
        assert_eq!(found, ["n/rust:1", "p2/rust:1", "p2/rust:2"]);
        // Without stack order (all runtimes treated as having no stack) the
        // same graph closes no cycle.
        let flat = profiles(&[("n", true, false)]);
        assert_eq!(cycle(&hops, &back, &flat), None);
    }

    /// Counterexample 4: a call into an idle non-reentrant runtime is fine.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn no_cycle_into_an_idle_runtime() {
        let p = profiles(&[("n1", true, false), ("n2", true, false)]);
        let call = hop("n1", "node:1", "n2", 1, &["n1/node:1"]);
        assert_eq!(cycle(&[], &call, &p), None);
        // Nor a callback on the caller's own chain.
        let back = hop(
            "n2",
            "node:1",
            "n1",
            1,
            &["n1/node:1", "n2/rust:1", "n2/node:1"],
        );
        assert_eq!(cycle(&[call], &back, &p), None);
    }

    /// What the check costs with `n` synchronous calls in progress, each a
    /// chain of three hops across six runtimes (#228): run with
    /// `cargo test -p rutis-bridge --release --lib measure_the_check -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn measure_the_check() {
        let runtimes = ["r0", "r1", "r2", "r3", "r4", "r5"];
        let p = profiles(&runtimes.map(|r| (r, true, r.ends_with('1'))));
        for n in [1, 4, 16, 64] {
            let mut hops = Vec::new();
            for root in 0..n {
                let mut path = vec![format!("{}/node:{}", runtimes[root % 6], root + 1)];
                for depth in 0..3 {
                    let (src, dst) = (
                        runtimes[(root + depth) % 6],
                        runtimes[(root + depth + 1) % 6],
                    );
                    let seq = (root * 3 + depth + 1) as u64;
                    let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                    let sid = path.last().unwrap().split_once('/').unwrap().1.to_owned();
                    hops.push(hop(src, &sid, dst, seq, &refs));
                    path.push(format!("{dst}/rust:{seq}"));
                    path.push(format!("{dst}/node:{}", 1000 + seq));
                }
            }
            let new = hop("r0", "node:9999", "r1", 99_999, &["r0/node:9999"]);
            let rounds = 1000;
            let started = std::time::Instant::now();
            for _ in 0..rounds {
                std::hint::black_box(cycle(&hops, &new, &p));
            }
            println!(
                "{} hops in progress: {:?} per check",
                hops.len(),
                started.elapsed() / rounds
            );
        }
    }

    /// An `events.call` carries its emitter's chain, but what the emitter
    /// ran under does not wait for the listener: p (Node) waits on h into
    /// a (Python); x, from c, runs on top of h, emits n and waits on a slow
    /// call into b; n's listener calls p (y). b returns, then x, h, and p
    /// runs y: no cycle, whether the listener's runtime has one stack or not.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn an_emitters_chain_is_not_waited_for() {
        for stacked in [false, true] {
            let p = profiles(&[
                ("p", true, false),
                ("a", true, true),
                ("c", false, true),
                ("d", stacked, true),
                ("b", false, true),
            ]);
            let h = hop("p", "node:1", "a", 1, &["p/node:1"]);
            let x = hop("c", "go:1", "a", 2, &["c/go:1"]);
            let slow = hop("a", "py:1", "b", 1, &["c/go:1", "a/rust:2", "a/py:1"]);
            // The listener's call, with the emit's chain (a/py:2 is x's
            // events.dispatch).
            let listener = entry("d", 1, &["c/go:1", "a/rust:2", "a/py:2"]);
            let y = hop(
                "d",
                "py:5",
                "p",
                2,
                &["c/go:1", "a/rust:2", "a/py:2", "d/rust:1", "d/py:5"],
            );
            assert_eq!(
                cycle(&[h, x, slow, listener], &y, &p),
                None,
                "stacked: {stacked}"
            );
        }
    }

    /// The listener run on top of a wait still closes its cycle with the
    /// emitter's chain in its path.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_listener_with_its_emitters_chain_on_top_of_a_wait() {
        let p = profiles(&[("c", true, false), ("a", true, true), ("e", true, true)]);
        let x = hop("c", "node:1", "a", 1, &["c/node:1"]);
        // e's own activity emits; its listener in a runs on top of x and
        // calls c.
        let listener = entry("a", 2, &["e/js:1", "e/js:2"]);
        let y = hop(
            "a",
            "py:2",
            "c",
            1,
            &["e/js:1", "e/js:2", "a/rust:2", "a/py:2"],
        );
        let found = cycle(&[x, listener], &y, &p).expect("a cycle");
        assert_eq!(found, ["c/rust:1", "a/rust:1", "a/rust:2"]);
    }

    /// The check stays near linear in the calls in progress: many entries
    /// (asynchronous calls and emits while a synchronous call waits) cost
    /// in proportion. Quadratic, four times the entries took about sixteen
    /// times as long (1,000: 5.9 ms, 3,000: 57 ms).
    #[test]
    fn the_check_is_near_linear_in_entries() {
        let p = profiles(&[("n", true, false), ("py", true, true), ("g", false, true)]);
        let time = |count: usize| {
            let mut hops = vec![hop("n", "node:1", "py", 1, &["n/node:1"])];
            for i in 0..count {
                hops.push(entry("py", 2 + i as u64, &["g/go:1", "g/go:2", "g/go:3"]));
            }
            let new = hop("py", "py:1", "g", 1, &["n/node:1", "py/rust:1", "py/py:1"]);
            (0..5)
                .map(|_| {
                    let start = std::time::Instant::now();
                    std::hint::black_box(cycle(&hops, &new, &p));
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(1_000), time(4_000));
        assert!(
            large < small * 8,
            "4,000 entries took {large:?}, 1,000 took {small:?}: not near linear"
        );
    }

    #[test]
    fn tagged_entries_decode_to_their_session() {
        assert_eq!(
            decode(&["s1/node:3".into(), "rust:2".into()], "s2"),
            [
                ("s1".into(), "node:3".into()),
                ("s2".into(), "rust:2".into())
            ]
        );
    }
}
