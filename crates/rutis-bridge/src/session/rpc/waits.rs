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
//! waits for it (an `emit`'s `events.call`, an asynchronous call whose frame
//! says `awaited: false`). An asynchronous call that says nothing (or
//! `awaited: true`) on a marked chain is a hop: its caller awaits the result
//! before the marked call finishes (SESS-83). An entry is a boundary of
//! chains: it waits for the synchronous calls made under it, but what it was
//! made under does not wait for them (an `events.call` carries its emitter's
//! chain). In a runtime with one stack a reentrant runtime runs it on top of
//! the call that waits there, so stack order holds for it like for a hop. A
//! session whose far end does not mark its synchronous waits makes no hops:
//! it is outside the check.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
#[cfg(not(rutis_shuttle))]
use std::sync::LazyLock;

use crate::sync::{Count, Mutex as SeamMutex};

/// The record points below, for a test to check against the model (feature
/// `testing`).
#[cfg(feature = "testing")]
pub mod trace;

/// The record points, compiled out: what this file does is unchanged.
#[cfg(not(feature = "testing"))]
mod trace {
    pub(super) fn profile(_: &str, _: bool, _: bool) {}
    pub(super) fn marked(_: &str, _: &str) {}
    pub(super) fn answered(_: &str, _: &str) {}
    pub(super) fn returned(_: &str, _: &str) {}
    pub(super) fn ended(_: &str) {}
    pub(super) fn forward_hop(
        _: &str,
        _: &str,
        _: u64,
        _: &[String],
        _: bool,
        _: bool,
        _: &super::Hop,
    ) {
    }
    pub(super) fn forward_same_chain(
        _: &str,
        _: &str,
        _: u64,
        _: &[String],
        _: bool,
        _: bool,
        _: (&str, &str),
    ) {
    }
    pub(super) fn forward_not_waited(
        _: &str,
        _: &str,
        _: u64,
        _: &[String],
        _: bool,
        _: bool,
        _: &'static str,
    ) {
    }
    pub(super) fn forward_refused(
        _: &str,
        _: &str,
        _: u64,
        _: &[String],
        _: bool,
        _: bool,
        _: &[String],
    ) {
    }
}

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
    /// The row that made the call and the row it went to, when a row's
    /// service forwarded it ([`with_rows`]): for the cycle's text.
    rows: Option<(String, String)>,
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

/// The table of this process. Under `--cfg rutis_shuttle` it is the
/// execution's: a shuttle model runs every task of one execution on one OS
/// thread, and shuttle's primitives belong to one execution — a table shared
/// by two models running at once would have both interleaving inside one
/// mutex (#628, SYN-9-29).
#[cfg(not(rutis_shuttle))]
static WAITS: LazyLock<SeamMutex<Waits>> = LazyLock::new(SeamMutex::default);

#[cfg(rutis_shuttle)]
thread_local! {
    static WAITS: &'static SeamMutex<Waits> = Box::leak(Box::new(SeamMutex::default()));
}

/// The table, as [`WAITS`] holds it.
#[cfg(not(rutis_shuttle))]
fn table() -> &'static SeamMutex<Waits> {
    LazyLock::force(&WAITS)
}

#[cfg(rutis_shuttle)]
fn table() -> &'static SeamMutex<Waits> {
    WAITS.with(|waits| *waits)
}

/// How many hops and marked calls the table holds, readable without its
/// lock: a host where no call is marked never takes it on a reply.
#[cfg(not(rutis_shuttle))]
static HOPS: Count = Count::new(0);
#[cfg(not(rutis_shuttle))]
static MARKS: Count = Count::new(0);

#[cfg(rutis_shuttle)]
thread_local! {
    static HOPS: &'static Count = Box::leak(Box::new(Count::new(0)));
    static MARKS: &'static Count = Box::leak(Box::new(Count::new(0)));
}

/// The hops' count ([`HOPS`]).
#[cfg(not(rutis_shuttle))]
fn hops() -> &'static Count {
    &HOPS
}

#[cfg(rutis_shuttle)]
fn hops() -> &'static Count {
    HOPS.with(|count| *count)
}

/// The marked calls' count ([`MARKS`]).
#[cfg(not(rutis_shuttle))]
fn marks() -> &'static Count {
    &MARKS
}

#[cfg(rutis_shuttle)]
fn marks() -> &'static Count {
    MARKS.with(|count| *count)
}

impl Waits {
    /// After a change, under the lock.
    fn counted(&self) {
        hops().store(self.hops.len(), Ordering::SeqCst);
        marks().store(self.marked.len(), Ordering::SeqCst);
    }
}

/// `session`'s far end, once it greeted.
pub(super) fn profile(session: &str, profile: Profile) {
    let mut waits = table().lock().unwrap();
    waits.profiles.insert(session.to_owned(), profile);
    // Recorded under the lock, like every operation here: a forward on
    // another thread must not be filed before what it read.
    trace::profile(session, profile.stacked, profile.reentrant);
}

/// A call `id` received on `session` whose caller waits synchronously.
pub(super) fn marked(session: &str, id: &str) {
    let mut waits = table().lock().unwrap();
    waits.marked.insert((session.to_owned(), id.to_owned()));
    waits.counted();
    trace::marked(session, id);
}

/// How many calls received on `session` are marked: a test's own count,
/// which the process-wide one mixes with other tests' sessions.
#[cfg(test)]
pub(super) fn marks_of(session: &str) -> usize {
    let waits = table().lock().unwrap();
    waits
        .marked
        .iter()
        .filter(|(tag, _)| tag == session)
        .count()
}

/// How many hops sent on `session` are recorded: a test's own count, for the
/// same reason as [`marks_of`].
#[cfg(all(test, rutis_shuttle))]
pub(super) fn hops_of(session: &str) -> usize {
    let waits = table().lock().unwrap();
    waits.hops.keys().filter(|(tag, _)| tag == session).count()
}

/// Call `id` received on `session` was answered.
pub(super) fn answered(session: &str, id: &str) {
    if marks().load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut waits = table().lock().unwrap();
    waits.marked.remove(&(session.to_owned(), id.to_owned()));
    waits.counted();
    trace::answered(session, id);
}

/// The reply to call `id` that this side sent on `session` was read (or the
/// call was cancelled): it waits for nothing any more.
pub(super) fn returned(session: &str, id: &str) {
    if hops().load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut waits = table().lock().unwrap();
    waits.hops.remove(&(session.to_owned(), id.to_owned()));
    waits.counted();
    trace::returned(session, id);
}

/// `session` ended: its calls wait for nothing, and nothing waits for them.
pub(super) fn ended(session: &str) {
    let mut waits = table().lock().unwrap();
    waits
        .hops
        .retain(|_, hop| hop.src != session && hop.dst != session);
    waits.marked.retain(|(tag, _)| tag != session);
    waits.profiles.remove(session);
    waits.counted();
    trace::ended(session);
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
/// with chain `path`. When the caller waits for it (a `sync` call; or the
/// chain's last call is marked and the call says its result is awaited), the
/// call is a hop: if it would close a wait cycle, nothing is recorded and the
/// cycle is returned, each call as `session/id`. When nobody waits for it (the
/// chain's last call is not marked; or the call says `awaited: false`), it is
/// an entry: the caller does not wait for it, though what is made under it is
/// on its chain. Returns whether either was recorded.
pub(super) fn forward(
    dst: &str,
    did: &str,
    seq: u64,
    path: &[String],
    synchronous: bool,
    awaited: bool,
) -> Result<bool, Cycle> {
    if crate::session::rpc::unchained_now() {
        trace::forward_not_waited(
            dst,
            did,
            seq,
            path,
            synchronous,
            awaited,
            "the thread sends unchained",
        );
        return Ok(false);
    }
    if marks().load(Ordering::SeqCst) == 0 && !synchronous {
        trace::forward_not_waited(
            dst,
            did,
            seq,
            path,
            synchronous,
            awaited,
            "nothing is marked",
        );
        return Ok(false);
    }
    let rows = ROWS.with(|rows| rows.borrow().clone());
    let mut waits = table().lock().unwrap();
    let mut chain = decode(path, dst);
    let caller = chain
        .last()
        .cloned()
        .filter(|caller| waits.marked.contains(caller));
    chain.push((dst.to_owned(), did.to_owned()));
    let hop = match caller {
        // A synchronous call back into the session it came from: on its
        // caller's chain, which runs it; not recorded.
        Some((src, sid)) if src == dst => {
            trace::forward_same_chain(dst, did, seq, path, synchronous, awaited, (&src, &sid));
            return Ok(false);
        }
        // The chain's last call waits for this one: a synchronous call, or
        // an asynchronous one whose caller says it awaits the result. A
        // call whose caller says it will not wait is an entry: nobody waits
        // for it, so it is no edge, but what is made under it is on its
        // chain (SESS-83).
        Some((src, sid)) => Hop {
            src,
            sid,
            dst: dst.to_owned(),
            did: did.to_owned(),
            seq,
            chain,
            entry: !(synchronous || awaited),
            rows: rows.clone(),
        },
        // The caller waits synchronously but no marked call is on its chain:
        // the host's own `invoke`/`request_sync`, say. It is a hop all the
        // same: a runtime may call back into this process, which forwards
        // again, and the chain has to lead back here (#537).
        None if synchronous => Hop {
            src: String::new(),
            sid: String::new(),
            dst: dst.to_owned(),
            did: did.to_owned(),
            seq,
            chain,
            entry: false,
            rows: rows.clone(),
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
            rows: rows.clone(),
        },
    };
    if !hop.entry {
        if let Some(calls) = cycle_among(waits.hops.values(), &hop, &waits.profiles) {
            trace::forward_refused(dst, did, seq, path, synchronous, awaited, &calls);
            let rows = rows_of(&calls, &waits.hops, &hop);
            return Err(Cycle { calls, rows });
        }
    }
    trace::forward_hop(dst, did, seq, path, synchronous, awaited, &hop);
    waits.hops.insert(hop.gid(), hop);
    waits.counted();
    Ok(true)
}

/// A wait cycle a call would close: its calls (`session/id`), and, when
/// rows made every call of it, the rows in order (`chat`, `weather`,
/// `chat`).
#[derive(Debug)]
pub(super) struct Cycle {
    #[cfg_attr(not(test), allow(dead_code))] // read by the tests
    pub calls: Vec<String>,
    pub rows: Option<Vec<String>>,
}

thread_local! {
    /// The calling row and the row called, while a row's service forwards
    /// a call ([`with_rows`]).
    static ROWS: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

/// Run `f`, a call from row `from` into row `to`'s service: the hops it
/// records carry both, so a cycle can be written with row names.
pub(crate) fn with_rows<R>(rows: Option<(String, String)>, f: impl FnOnce() -> R) -> R {
    /// Puts the previous rows back, also when `f` panics.
    struct Restore(Option<(String, String)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            ROWS.with(|current| current.replace(previous));
        }
    }
    let _restore = Restore(ROWS.with(|current| current.replace(rows)));
    f()
}

/// The rows of the cycle `calls` (`new` first): for each call, the row
/// that made it and the row it went to. `None` when a call of it was not
/// a row's.
fn rows_of(calls: &[String], hops: &HashMap<Global, Hop>, new: &Hop) -> Option<Vec<String>> {
    let mut rows = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        let hop = match index {
            0 => new,
            _ => {
                let (session, id) = call.split_once('/')?;
                hops.get(&(session.to_owned(), id.to_owned()))?
            }
        };
        let (from, to) = hop.rows.as_ref()?;
        // Each call's rows, a row met twice in a row once.
        for row in [from, to] {
            if rows.last() != Some(row) {
                rows.push(row.clone());
            }
        }
    }
    // The cycle ends where it began.
    if rows.last() != rows.first() {
        rows.push(rows.first()?.clone());
    }
    Some(rows)
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

#[cfg(test)]
thread_local! {
    /// The steps [`cycle_among`] took on this thread: what a test counts
    /// to see how the check grows, whatever the machine's load.
    static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `n` steps of [`cycle_among`]: one for each call, chain entry and edge
/// it looks at. Only tests count them.
#[inline(always)]
fn steps(n: usize) {
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + n));
    #[cfg(not(test))]
    let _ = n;
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
    steps(all.len());
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
            steps(1);
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
            steps(1);
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
        steps(1);
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
                let related = hop.chain.iter().any(|(session, id)| {
                    steps(1);
                    session == y && sids.contains(id.as_str())
                });
                if !hop.entry && !related && !started[h] {
                    steps(outs.len());
                    successors[h].extend(outs.iter().copied().filter(|&o| o != h));
                }
            }
        }
        // Stack order: c waits for every later call into y that started
        // there; through the next one that did, which waits for the rest.
        let mut next_started = None;
        for &c in calls.iter().rev() {
            steps(1);
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
            steps(1);
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
            rows: None,
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
            rows: None,
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

    /// A notification sent with `unchained` is not part of the wait graph,
    /// even while a call is marked.
    #[test]
    fn a_notification_sent_unchained_is_not_in_the_graph() {
        marked("notice_a", "node:1");
        let recorded = crate::session::rpc::unchained(|| {
            forward(
                "notice_b",
                "rust:1",
                1,
                &["notice_a/node:1".to_owned()],
                false,
                true,
            )
        })
        .unwrap();
        assert!(!recorded);
        assert!(!table()
            .lock()
            .unwrap()
            .hops
            .contains_key(&("notice_b".to_owned(), "rust:1".to_owned())));
        answered("notice_a", "node:1");
        ended("notice_a");
        ended("notice_b");
    }

    /// A forward whose chain ends at a marked call is a hop even when the
    /// forwarded call is asynchronous (a relayed function reference): the
    /// caller still waits for the relayed call to settle.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn an_asynchronous_forward_on_a_marked_chain_is_a_hop() {
        marked("hop_a", "node:1");
        let recorded = forward(
            "hop_b",
            "rust:1",
            1,
            &["hop_a/node:1".to_owned()],
            false,
            true,
        )
        .unwrap();
        assert!(recorded);
        let entry = table()
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

    /// An asynchronous forward whose frame says its result is not awaited is
    /// an entry, even on a marked chain: nobody waits for it, so it is no
    /// edge (SESS-83). What is made under it is still on its chain.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn an_asynchronous_forward_nobody_awaits_is_an_entry() {
        marked("unawaited_a", "node:1");
        let recorded = forward(
            "unawaited_b",
            "rust:1",
            1,
            &["unawaited_a/node:1".to_owned()],
            false,
            false,
        )
        .unwrap();
        assert!(
            recorded,
            "the entry is recorded: chains made under it stop here"
        );
        let hop = table()
            .lock()
            .unwrap()
            .hops
            .get(&("unawaited_b".to_owned(), "rust:1".to_owned()))
            .cloned();
        assert_eq!(hop.as_ref().map(|hop| hop.entry), Some(true));
        assert_eq!(
            hop.as_ref().map(|hop| hop.src.as_str()),
            Some("unawaited_a"),
            "the call is on the marked call's chain, though nobody waits for it"
        );
        returned("unawaited_b", "rust:1");
        answered("unawaited_a", "node:1");
        ended("unawaited_a");
        ended("unawaited_b");
    }

    /// The same cycle as [`a_host_root_sync_call_closes_a_cycle_through_two_runtimes`],
    /// but the host's call into B is one its caller will not await: it is an
    /// entry, so the call back into A closes no cycle.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_fire_and_forget_forward_closes_no_cycle() {
        let one_stack = Profile {
            stacked: true,
            reentrant: false,
        };
        profile("forgot_a", one_stack);
        profile("forgot_b", one_stack);
        // The host calls A synchronously; A calls the host, which calls B
        // without waiting for the result.
        forward("forgot_a", "rust:1", 1, &[], true, true).unwrap();
        marked("forgot_a", "node:1");
        let recorded = forward(
            "forgot_b",
            "rust:2",
            2,
            &["forgot_a/node:1".to_owned()],
            false,
            false,
        )
        .unwrap();
        assert!(recorded, "an entry is recorded");
        // B calls the host; the host calls A again. Nobody waits for the
        // call into B, so the graph has no cycle.
        marked("forgot_b", "node:2");
        forward(
            "forgot_a",
            "rust:3",
            3,
            &["forgot_b/node:2".to_owned()],
            true,
            true,
        )
        .expect("no cycle through a call nobody waits for");
        returned("forgot_a", "rust:3");
        returned("forgot_a", "rust:1");
        returned("forgot_b", "rust:2");
        answered("forgot_a", "node:1");
        answered("forgot_b", "node:2");
        ended("forgot_a");
        ended("forgot_b");
    }

    /// The same shape, with the host's call into B awaited: it is a hop and
    /// the call back into A closes the cycle, refused.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn an_awaited_forward_closes_the_cycle() {
        let one_stack = Profile {
            stacked: true,
            reentrant: false,
        };
        profile("awaits_a", one_stack);
        profile("awaits_b", one_stack);
        forward("awaits_a", "rust:1", 1, &[], true, true).unwrap();
        marked("awaits_a", "node:1");
        forward(
            "awaits_b",
            "rust:2",
            2,
            &["awaits_a/node:1".to_owned()],
            false,
            true,
        )
        .unwrap();
        marked("awaits_b", "node:2");
        let cycle = forward(
            "awaits_a",
            "rust:3",
            3,
            &["awaits_b/node:2".to_owned()],
            true,
            true,
        )
        .expect_err("the awaited call into B is an edge, and the cycle closes");
        assert!(cycle.calls.iter().any(|call| call == "awaits_a/rust:3"));
        returned("awaits_a", "rust:1");
        returned("awaits_b", "rust:2");
        answered("awaits_a", "node:1");
        answered("awaits_b", "node:2");
        ended("awaits_a");
        ended("awaits_b");
    }

    /// The host's own synchronous call begins a chain: recorded as a hop
    /// even with no chain and nothing marked yet, so a runtime that calls
    /// back and forwards again is checked against it (#537).
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_synchronous_call_without_a_chain_is_a_hop() {
        let recorded = forward("root_b", "rust:1", 1, &[], true, true).unwrap();
        assert!(recorded);
        let entry = table()
            .lock()
            .unwrap()
            .hops
            .get(&("root_b".to_owned(), "rust:1".to_owned()))
            .map(|hop| hop.entry);
        assert_eq!(entry, Some(false), "the host waits for it");
        returned("root_b", "rust:1");
        ended("root_b");
    }

    /// A host's root synchronous call joins the graph: A calls back into the
    /// host, the host calls B, B calls back, and the host's next call into A
    /// closes a cycle. Without the root hop the two runtimes' waits would
    /// not be connected (#537).
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_host_root_sync_call_closes_a_cycle_through_two_runtimes() {
        let one_stack = Profile {
            stacked: true,
            reentrant: false,
        };
        profile("cycle_a", one_stack);
        profile("cycle_b", one_stack);
        // The host calls A synchronously, with no chain of its own.
        forward("cycle_a", "rust:1", 1, &[], true, true).unwrap();
        // A calls the host (marked); the host calls B.
        marked("cycle_a", "node:1");
        forward(
            "cycle_b",
            "rust:2",
            2,
            &["cycle_a/node:1".to_owned()],
            true,
            true,
        )
        .unwrap();
        // B calls the host (marked); the host calls A again: A is non-
        // reentrant and waits for B, B waits for A. The call is refused.
        marked("cycle_b", "node:2");
        let cycle = forward(
            "cycle_a",
            "rust:3",
            3,
            &["cycle_b/node:2".to_owned()],
            true,
            true,
        )
        .expect_err("the host's call into A closes the cycle");
        assert!(
            cycle.calls.iter().any(|call| call == "cycle_a/rust:3"),
            "{cycle:?}"
        );
        // Made by no row's service: no rows to name.
        assert_eq!(cycle.rows, None);
        returned("cycle_a", "rust:1");
        returned("cycle_b", "rust:2");
        answered("cycle_a", "node:1");
        answered("cycle_b", "node:2");
        ended("cycle_a");
        ended("cycle_b");
    }

    /// An asynchronous call with no chain is still an entry: nobody waits
    /// for it, so it bounds the chain made under it.
    #[test]
    fn an_asynchronous_call_without_a_chain_is_an_entry() {
        marked("call_a", "node:1");
        let recorded = forward("call_b", "rust:1", 2, &[], false, true).unwrap();
        assert!(recorded);
        let entry = table()
            .lock()
            .unwrap()
            .hops
            .get(&("call_b".to_owned(), "rust:1".to_owned()))
            .map(|hop| hop.entry);
        assert_eq!(entry, Some(true));
        returned("call_b", "rust:1");
        answered("call_a", "node:1");
        ended("call_a");
        ended("call_b");
    }

    /// A call removed after cancellation or a reply leaves no mark behind.
    #[test]
    fn an_answered_call_leaves_no_mark() {
        marked("gone_a", "node:1");
        assert_eq!(marks_of("gone_a"), 1);
        answered("gone_a", "node:1");
        assert_eq!(marks_of("gone_a"), 0);
        ended("gone_a");
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

    /// The steps one check of `new` among `hops` takes.
    fn steps_of(hops: &[Hop], new: &Hop, profiles: &HashMap<String, Profile>) -> usize {
        STEPS.with(|steps| steps.set(0));
        std::hint::black_box(cycle(hops, new, profiles));
        STEPS.with(|steps| steps.get())
    }

    /// The check stays linear in the calls in progress: many entries
    /// (asynchronous calls and emits while a synchronous call waits) cost
    /// in proportion. It counts the check's steps, not its time, so the
    /// machine's load does not change the result (#922). Quadratic, four
    /// times the entries took sixteen times the steps.
    #[test]
    fn the_check_is_near_linear_in_entries() {
        let p = profiles(&[("n", true, false), ("py", true, true), ("g", false, true)]);
        let count = |count: usize| {
            let mut hops = vec![hop("n", "node:1", "py", 1, &["n/node:1"])];
            for i in 0..count {
                hops.push(entry("py", 2 + i as u64, &["g/go:1", "g/go:2", "g/go:3"]));
            }
            let new = hop("py", "py:1", "g", 1, &["n/node:1", "py/rust:1", "py/py:1"]);
            steps_of(&hops, &new, &p)
        };
        let (small, large) = (count(1_000), count(4_000));
        assert!(
            large <= small * 5,
            "4,000 entries took {large} steps, 1,000 took {small}: not linear"
        );
    }

    /// The counted steps see a quadratic graph: the put-aside edges, each
    /// hop not started in a non-reentrant runtime to each call out of it,
    /// are the one part of the check that is not linear. Four times the
    /// calls take more than ten times the steps, which the check above
    /// would reject.
    #[test]
    fn the_steps_see_a_quadratic_graph() {
        let p = profiles(&[("n", true, false), ("g", false, true), ("h", false, true)]);
        let count = |count: usize| {
            let mut hops = Vec::new();
            for i in 1..=count {
                // g calls n (not started there), and n calls h.
                hops.push(hop(
                    "g",
                    &format!("go:{i}"),
                    "n",
                    i as u64,
                    &[&format!("g/go:{i}")],
                ));
                hops.push(hop(
                    "n",
                    &format!("node:{i}"),
                    "h",
                    i as u64,
                    &[&format!("n/node:{i}")],
                ));
            }
            let new = hop("g", "go:0", "h", 0, &["g/go:0"]);
            steps_of(&hops, &new, &p)
        };
        let (small, large) = (count(250), count(1_000));
        assert!(
            large > small * 10,
            "1,000 calls took {large} steps, 250 took {small}: the steps miss the quadratic edges"
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

    // ── The check against the model's `RustSucc` (#613, item 12) ──────

    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};

    /// A wait graph to check: the calls in progress, the call about to be
    /// forwarded, and what each session's far end does.
    #[derive(Debug)]
    struct WaitGraph {
        hops: Vec<Hop>,
        new: Hop,
        profiles: HashMap<String, Profile>,
    }

    /// The sessions a generated graph runs across.
    const SESSIONS: [&str; 5] = ["ra", "rb", "rc", "rd", "re"];

    /// A session no call reaches: a chain entry naming it names a call that
    /// is no longer in the graph (a missing edge).
    const GONE: &str = "gone";

    /// One entry of a generated chain, before ids are assigned.
    #[derive(Clone, Debug)]
    enum Step {
        /// Call `i` by the id it went to (`calls.len()` is the new call): the
        /// model's `Gid`.
        Call(usize),
        /// Call `i` by the id its caller gave it, the form a path carries for
        /// the call that made it; `RustSucc`'s `Related` reads these.
        Named(usize),
        /// A call of [`GONE`]: a chain naming a call already returned.
        Absent(u64),
        /// The hop's own call: a chain that names the call itself.
        Own,
        /// The hop's caller, as the hop's own path ends with.
        Caller,
    }

    #[derive(Clone, Debug)]
    struct RawCall {
        entry: bool,
        src: usize,
        dst: usize,
        path: Vec<Step>,
    }

    #[derive(Clone, Debug)]
    struct RawGraph {
        calls: Vec<RawCall>,
        new: RawCall,
        /// `(stacked, reentrant)` for each of [`SESSIONS`].
        profiles: Vec<(bool, bool)>,
    }

    fn step(calls: usize) -> impl Strategy<Value = Step> {
        prop_oneof![
            4 => (0..=calls).prop_map(Step::Call),
            2 => (0..=calls).prop_map(Step::Named),
            2 => (0..4u64).prop_map(Step::Absent),
            1 => Just(Step::Own),
            3 => Just(Step::Caller),
        ]
    }

    fn raw_call(calls: usize) -> impl Strategy<Value = RawCall> {
        (
            prop::bool::weighted(0.25),
            0..SESSIONS.len(),
            0..SESSIONS.len(),
            prop::collection::vec(step(calls), 0..=5),
        )
            .prop_map(|(entry, src, dst, path)| RawCall {
                entry,
                src,
                dst,
                path,
            })
    }

    /// A graph of up to 20 hops: entries among them, sessions with one stack
    /// and reentrant, chains naming calls present, returned and themselves.
    fn raw_graph() -> impl Strategy<Value = RawGraph> {
        (0..=20usize).prop_flat_map(|calls| {
            (
                prop::collection::vec(raw_call(calls), calls),
                raw_call(calls),
                prop::collection::vec((any::<bool>(), any::<bool>()), SESSIONS.len()),
            )
                .prop_map(|(calls, mut new, profiles)| {
                    new.entry = false;
                    RawGraph {
                        calls,
                        new,
                        profiles,
                    }
                })
        })
    }

    /// The session a call went to.
    fn destination(call: &RawCall) -> &'static str {
        SESSIONS[call.dst % SESSIONS.len()]
    }

    /// The session a call came from: never the one it went to, as `forward`
    /// records no hop for a call back into its own session.
    fn source(call: &RawCall) -> &'static str {
        let sessions = SESSIONS.len();
        SESSIONS[(call.dst + 1 + call.src % (sessions - 1)) % sessions]
    }

    /// The id call `i` went to: numbered in `seq` order, as the model's ids
    /// are. A runtime's own ids (the ones its calls carry back) are `node:M`,
    /// so the two id spaces never meet.
    fn went_to(call: &RawCall, i: usize) -> Global {
        (destination(call).to_owned(), format!("rust:{}", i + 1))
    }

    /// The session of call `i`'s caller and the id that caller gave it.
    fn caller_of(call: &RawCall, i: usize) -> Global {
        if call.entry {
            // Nobody made it.
            (String::new(), String::new())
        } else {
            (source(call).to_owned(), format!("node:{}", i + 1))
        }
    }

    fn chain(steps: &[Step], own: usize, calls: &[RawCall], new: &RawCall) -> Vec<Global> {
        let at = |i: usize| if i == calls.len() { new } else { &calls[i] };
        let mut chain: Vec<Global> = Vec::new();
        for step in steps {
            let gid = match *step {
                Step::Call(i) => went_to(at(i), i),
                Step::Named(i) => caller_of(at(i), i),
                Step::Absent(n) => (GONE.to_owned(), format!("rust:{n}")),
                Step::Own => went_to(at(own), own),
                Step::Caller => caller_of(at(own), own),
            };
            // The model's `Pos` takes the first occurrence; keep chains
            // single-valued rather than guessing at a repeat.
            if !chain.contains(&gid) {
                chain.push(gid);
            }
        }
        chain
    }

    fn built(i: usize, call: &RawCall, calls: &[RawCall], new: &RawCall, entry: bool) -> Hop {
        let (dst, did) = went_to(call, i);
        let mut chain = chain(&call.path, i, calls, new);
        chain.push((dst.clone(), did.clone()));
        Hop {
            src: if entry {
                String::new()
            } else {
                source(call).to_owned()
            },
            sid: if entry {
                String::new()
            } else {
                format!("node:{}", i + 1)
            },
            dst,
            did,
            seq: i as u64 + 1,
            chain,
            entry,
            rows: None,
        }
    }

    /// The graph the raw one names: ids in `seq` order, entries without a
    /// caller, and no hop from the session it goes to, the shape `forward`
    /// records. The ids a call went to are `rust:N`, the ids a runtime gives
    /// its own calls are `node:M`; the call `Hop::chain` keeps last can then
    /// neither start a call nor be the id `Related` reads, as on a session
    /// whose ids are distinct.
    fn build(raw: RawGraph) -> WaitGraph {
        let len = raw.calls.len();
        let hops: Vec<Hop> = raw
            .calls
            .iter()
            .enumerate()
            .map(|(i, call)| built(i, call, &raw.calls, &raw.new, call.entry))
            .collect();
        let new = built(len, &raw.new, &raw.calls, &raw.new, false);
        let profiles = SESSIONS
            .iter()
            .zip(&raw.profiles)
            .map(|(session, &(stacked, reentrant))| {
                ((*session).to_owned(), Profile { stacked, reentrant })
            })
            .collect();
        WaitGraph {
            hops,
            new,
            profiles,
        }
    }

    // ── the model's graph ──

    /// The model's successors (`RustSucc`, `specs/cross-runtime-sync/
    /// CrossRuntimeSync.tla`) of every call in `nodes`, built the naive way:
    /// one clause per kind of edge, every edge listed.
    fn model_successors(nodes: &[&Hop], profiles: &HashMap<String, Profile>) -> Vec<Vec<usize>> {
        let gid = |hop: &Hop| (hop.dst.clone(), hop.did.clone());
        let gids: Vec<Global> = nodes.iter().map(|hop| gid(hop)).collect();
        let index: HashMap<Global, usize> = gids
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, gid)| (gid, i))
            .collect();
        assert_eq!(index.len(), nodes.len(), "generated ids are distinct");
        // `Hop::chain` keeps the call itself last; the model's `chain` is the
        // path the call carried, without it.
        fn path(hop: &Hop) -> &[Global] {
            &hop.chain[..hop.chain.len() - 1]
        }
        let stops = |gid: &Global| index.get(gid).is_some_and(|&at| nodes[at].entry);
        // Started: some call out of its session names it (`Started`).
        let started: Vec<bool> = (0..nodes.len())
            .map(|d| {
                nodes.iter().any(|out| {
                    !out.entry && out.src == nodes[d].dst && path(out).contains(&gids[d])
                })
            })
            .collect();
        let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for (a, hop) in nodes.iter().enumerate() {
            let y = &hop.dst;
            // Chain: a call waits for the calls whose path names it, with no
            // entry between (`ChainFree`).
            for (c, called) in nodes.iter().enumerate() {
                if called.entry {
                    continue;
                }
                let chain = path(called);
                if let Some(at) = chain.iter().position(|gid| gid == &gids[a]) {
                    if !chain[at + 1..].iter().any(stops) {
                        successors[a].push(c);
                    }
                }
            }
            let profile = profiles.get(y).copied().unwrap_or_default();
            if !profile.stacked {
                continue;
            }
            let outs: Vec<usize> = (0..nodes.len())
                .filter(|&d| !nodes[d].entry && nodes[d].src == *y)
                .collect();
            // Put aside: not started there, not on its chain, and the runtime
            // is not reentrant, it waits for every call out of it (`defer`).
            if !profile.reentrant && !outs.is_empty() {
                let related = outs.iter().any(|&d| {
                    path(hop)
                        .iter()
                        .any(|(session, id)| session == y && id == &nodes[d].sid)
                });
                if !related && !started[a] {
                    successors[a].extend(outs.iter().copied());
                }
            }
            // Stack order: every later call into the same runtime that has
            // started. The generated ids number in `seq` order, as the
            // model's `h.did < d.did` does.
            successors[a].extend(
                (0..nodes.len())
                    .filter(|&d| nodes[d].dst == *y && nodes[d].seq > hop.seq && started[d]),
            );
        }
        successors
    }

    /// A cycle through `new` (the last node) in the model's graph.
    fn model_has_a_cycle(successors: &[Vec<usize>], nodes: usize) -> bool {
        let new = nodes - 1;
        let mut seen = vec![false; nodes];
        seen[new] = true;
        let mut stack = vec![new];
        while let Some(at) = stack.pop() {
            for &next in &successors[at] {
                if next == new {
                    return true;
                }
                if !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        false
    }

    /// `cycle_among` and the model agree on `graph`; returns the cycle found,
    /// checked to be one cycle of the model's graph.
    fn agrees(graph: &WaitGraph) -> Option<Vec<String>> {
        let mut nodes: Vec<&Hop> = graph.hops.iter().collect();
        nodes.push(&graph.new);
        let successors = model_successors(&nodes, &graph.profiles);
        let found = cycle_among(graph.hops.iter(), &graph.new, &graph.profiles);
        assert_eq!(
            found.is_some(),
            model_has_a_cycle(&successors, nodes.len()),
            "the check and the model disagree on {graph:?}"
        );
        let calls = found?;
        // The names are one cycle: the new call first, each call leading to
        // the next, the last back to the first.
        let index: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, hop)| (name(hop), i))
            .collect();
        let ats: Vec<usize> = calls
            .iter()
            .map(|call| {
                *index
                    .get(call)
                    .unwrap_or_else(|| panic!("{call} names no call in {graph:?}"))
            })
            .collect();
        assert!(!ats.is_empty(), "a cycle names no call: {calls:?}");
        assert_eq!(
            ats[0],
            nodes.len() - 1,
            "the cycle does not start at the new call: {calls:?}"
        );
        assert_eq!(
            ats.iter().collect::<std::collections::HashSet<_>>().len(),
            ats.len(),
            "a call is named twice: {calls:?}"
        );
        for (i, &at) in ats.iter().enumerate() {
            let next = ats[(i + 1) % ats.len()];
            assert!(
                successors[at].contains(&next),
                "the model has no edge {} -> {}: {graph:?}",
                name(nodes[at]),
                name(nodes[next])
            );
        }
        Some(calls)
    }

    // ── the shapes and the property ──

    /// The shapes a random graph meets, one by one: an empty graph, a chain
    /// closing a cycle, a call whose chain names itself, a chain cut by an
    /// entry (two entries in the graph), and a chain naming a call that has
    /// returned. No session has a stack, so only the chain edges are here;
    /// the property test adds runtimes with stacks.
    #[test]
    fn the_model_check_agrees_on_the_small_graphs() {
        let profiles = HashMap::new();
        // Nothing in progress: nothing to close.
        assert_eq!(
            agrees(&WaitGraph {
                hops: vec![],
                new: hop("a", "node:1", "b", 1, &[]),
                profiles: profiles.clone(),
            }),
            None
        );
        // a's call to b names the host's call into a, b's call to c names
        // a's call to b, and the host's next call into a names b's call to c:
        // a cycle.
        let a = hop("a", "node:1", "b", 1, &["a/rust:2"]);
        let b = hop("b", "node:1", "c", 1, &["b/rust:1"]);
        let new = hop("c", "node:1", "a", 2, &["c/rust:1"]);
        assert_eq!(
            agrees(&WaitGraph {
                hops: vec![a.clone(), b.clone()],
                new: new.clone(),
                profiles: profiles.clone(),
            }),
            Some(vec![
                "a/rust:2".to_owned(),
                "b/rust:1".to_owned(),
                "c/rust:1".to_owned()
            ])
        );
        // A chain naming the call itself: it waits on itself.
        assert_eq!(
            agrees(&WaitGraph {
                hops: vec![],
                new: hop("a", "node:1", "b", 1, &["b/rust:1"]),
                profiles: profiles.clone(),
            }),
            Some(vec!["b/rust:1".to_owned()])
        );
        // An entry between a's call to b and the host's call into a: nobody
        // waits across it, so the same chain closes no cycle. The second
        // entry is in the graph and waits for nothing.
        let notice = entry("e", 3, &[]);
        let inert = entry("d", 4, &[]);
        let cut = hop("a", "node:1", "b", 1, &["a/rust:2", "e/rust:3"]);
        assert_eq!(
            agrees(&WaitGraph {
                hops: vec![cut, b.clone(), notice, inert],
                new: new.clone(),
                profiles: profiles.clone(),
            }),
            None
        );
        // A chain naming a call that is not in the graph: no edge either.
        assert_eq!(
            agrees(&WaitGraph {
                hops: vec![hop("a", "node:1", "b", 1, &["x/rust:9"]), b],
                new,
                profiles,
            }),
            None
        );
    }

    /// A capture begins with the table as it stands: a hop and a profile
    /// another run of the test binary left there are in the trace's initial
    /// state, so a refusal that closes a cycle through them replays as the
    /// model allows. Replayed from the model's empty state, the same trace
    /// names a cycle the graph does not have (#1175).
    #[cfg(feature = "testing")]
    #[test]
    fn a_capture_starts_from_the_table() {
        profile(
            "cap_n1",
            Profile {
                stacked: true,
                reentrant: false,
            },
        );
        profile(
            "cap_n2",
            Profile {
                stacked: true,
                reentrant: false,
            },
        );
        // n1's call into n2, in the table before the capture begins.
        marked("cap_n1", "node:1");
        assert!(forward(
            "cap_n2",
            "rust:1",
            1,
            &["cap_n1/node:1".to_owned()],
            true,
            true
        )
        .unwrap());
        let capture = trace::capture();
        // n2's far end calls back; the call into n1 closes the cycle through
        // the hop and the mark the table held when recording began.
        marked("cap_n2", "node:1");
        assert!(
            forward(
                "cap_n1",
                "rust:2",
                2,
                &["cap_n2/node:1".to_owned()],
                true,
                true,
            )
            .is_err(),
            "the call would wait on itself"
        );
        let trace = capture.trace();
        let notes = trace::check(&trace).expect("the refusal the model makes");
        assert_eq!(notes, Vec::<String>::new());
        // From the model's empty state the graph has neither the hop nor the
        // profiles, so the refusal names a call that is not there.
        let empty = trace::Trace::new(trace.events().to_vec());
        assert!(
            trace::check(&empty).is_err(),
            "the cycle needs the table's state"
        );
        returned("cap_n2", "rust:1");
        answered("cap_n2", "node:1");
        answered("cap_n1", "node:1");
        ended("cap_n1");
        ended("cap_n2");
    }

    /// Item 12 of #613: for a wait graph of up to 20 hops, `cycle_among` finds
    /// a cycle exactly when a depth-first search of the model's `RustSucc`
    /// graph (`specs/cross-runtime-sync/CrossRuntimeSync.tla`) does, and the
    /// calls it names form one such cycle. `PROPTEST_CASES` cases (256 by
    /// default); a failure prints the seed, rerun with
    /// `RUTIS_PROPTEST_SEED=<seed>` (Q7.2).
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn cycle_among_agrees_with_the_models_graph() {
        check("cycle_among", raw_graph(), |raw| {
            agrees(&build(raw));
            Ok(())
        });
    }

    /// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED`, or
    /// a fresh one; on failure, panic with the seed and the shrunk input.
    fn check<S: Strategy>(
        name: &str,
        strategy: S,
        test: impl Fn(S::Value) -> Result<(), TestCaseError>,
    ) where
        S::Value: std::fmt::Debug,
    {
        let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
            Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
            Err(_) => {
                use std::hash::{BuildHasher, Hasher};
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u128(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                );
                hasher.finish()
            }
        };
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&seed.to_le_bytes());
        }
        // `Config::default()` reads PROPTEST_CASES (256 when unset).
        let config = Config {
            failure_persistence: None,
            ..Config::default()
        };
        let cases = config.cases;
        let mut runner =
            TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
        let started = std::time::Instant::now();
        if let Err(error) = runner.run(&strategy, test) {
            panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
        }
        eprintln!(
            "{name}: {cases} cases in {:?} (seed {seed})",
            started.elapsed()
        );
    }
}

/// The shuttle model of the fast path: an answer or a return that reads no
/// table (#628, SYN-9-29).
#[cfg(all(test, rutis_shuttle))]
mod shuttle_tests;
