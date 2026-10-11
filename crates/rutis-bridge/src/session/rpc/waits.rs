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
//! runtime that does not declare it is taken as non-reentrant. A session whose far end does not mark its
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
}

impl Hop {
    fn gid(&self) -> Global {
        (self.dst.clone(), self.did.clone())
    }
}

#[derive(Default)]
struct Waits {
    hops: Vec<Hop>,
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
    waits.hops.retain(|hop| hop.dst != session || hop.did != id);
    waits.counted();
}

/// `session` ended: its calls wait for nothing, and nothing waits for them.
pub(super) fn ended(session: &str) {
    let mut waits = WAITS.lock().unwrap();
    waits
        .hops
        .retain(|hop| hop.src != session && hop.dst != session);
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
/// `session/id`. Returns whether a hop was recorded.
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
    let chain = decode(path, dst);
    let Some((src, sid)) = chain.last().cloned() else {
        return Ok(false);
    };
    if src == dst || !waits.marked.contains(&(src.clone(), sid.clone())) {
        return Ok(false);
    }
    let mut chain = chain;
    chain.push((dst.to_owned(), did.to_owned()));
    let hop = Hop {
        src,
        sid,
        dst: dst.to_owned(),
        did: did.to_owned(),
        seq,
        chain,
    };
    if let Some(cycle) = cycle(&waits.hops, &hop, &waits.profiles) {
        return Err(cycle);
    }
    waits.hops.push(hop);
    waits.counted();
    Ok(true)
}

/// The cycle that `new` would close among `hops`, if any.
fn cycle(hops: &[Hop], new: &Hop, profiles: &HashMap<String, Profile>) -> Option<Vec<String>> {
    let mut all: Vec<&Hop> = hops.iter().collect();
    all.push(new);
    let last = all.len() - 1;
    // Whether each hop has started where it went: a call out of that
    // session names it.
    let started: Vec<bool> = all
        .iter()
        .map(|d| {
            let gid = d.gid();
            all.iter().any(|e| e.src == d.dst && e.chain.contains(&gid))
        })
        .collect();
    let successors: Vec<Vec<usize>> = (0..all.len())
        .map(|h| successors(&all, &started, h, profiles))
        .collect();
    // Depth-first from `new`, remembering how each hop was reached.
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

/// What hop `h` waits for (`RustSucc` in the model).
fn successors(
    all: &[&Hop],
    started: &[bool],
    h: usize,
    profiles: &HashMap<String, Profile>,
) -> Vec<usize> {
    let hop = all[h];
    let y = &hop.dst;
    let gid = hop.gid();
    let profile = profiles.get(y).copied().unwrap_or_default();
    let mut next = Vec::new();
    for (d, other) in all.iter().enumerate() {
        if d == h {
            continue;
        }
        // Chain: made under this hop.
        if other.chain.contains(&gid) {
            next.push(d);
        }
    }
    if !profile.stacked {
        return next;
    }
    let out: Vec<usize> = (0..all.len()).filter(|&d| all[d].src == *y).collect();
    let related = out
        .iter()
        .any(|&d| hop.chain.contains(&(y.clone(), all[d].sid.clone())));
    if !profile.reentrant && !out.is_empty() && !related && !started[h] {
        next.extend(out.iter().copied().filter(|&d| d != h));
    }
    for (d, other) in all.iter().enumerate() {
        if d != h && other.dst == *y && hop.seq < other.seq && started[d] {
            next.push(d);
        }
    }
    next.sort_unstable();
    next.dedup();
    next
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
        }
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
