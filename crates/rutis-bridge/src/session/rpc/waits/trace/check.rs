//! What a recorded trace broke, and what the model allows there.

use std::collections::{HashMap, HashSet};

use super::{Decision, Event, Forward};

/// One thing a recorded trace did that the model does not allow: the event,
/// by its index in the trace, and the rule it broke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The event, by its index in the trace.
    pub at: usize,
    /// The event itself.
    pub event: Event,
    /// The model's rule it broke, and why.
    pub why: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "event {}: {}", self.at, self.why)
    }
}

impl std::error::Error for Violation {}

/// Replay `events` as the model does
/// (`specs/cross-runtime-sync/CrossRuntimeSync.tla`): each profile is a
/// runtime's constant (`sync-stack`, `reentrant-sync`), each marked call one
/// a far end waits for, and each forward a call this side sends. Before a
/// hop is recorded the model's graph is rebuilt (`RustSucc`: chain, put
/// aside, stack order) and asked whether the call would wait on itself; a
/// record it would not allow, or a decision it would not make, is a
/// [`Violation`].
///
/// The model allows some events it does not guarantee; those come back as
/// notes, one per event, and the run still passes. A run meant to be fully
/// checked asserts no notes.
pub fn check(events: &[Event]) -> Result<Vec<String>, Box<Violation>> {
    let mut table = Table::default();
    let mut notes = Vec::new();
    for (at, event) in events.iter().enumerate() {
        let broken = |why: String| {
            Box::new(Violation {
                at,
                event: event.clone(),
                why,
            })
        };
        match event {
            Event::Profile {
                session,
                stacked,
                reentrant,
            } => table
                .profile(session, *stacked, *reentrant)
                .map_err(broken)?,
            Event::Marked { session, id } => table.marked(session, id).map_err(broken)?,
            Event::Answered { session, id } => table.answered(session, id).map_err(broken)?,
            Event::Returned { session, id } => table.returned(session, id),
            Event::Ended { session } => table.ended(session).map_err(broken)?,
            Event::Forward(forward) => table.forward(forward, &mut notes).map_err(broken)?,
        }
    }
    Ok(notes)
}

/// What a runtime does while it waits, as its greeting said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Profile {
    /// One thread, one stack (`sync-stack`).
    stacked: bool,
    /// Runs every incoming call while it waits (`reentrant-sync`).
    reentrant: bool,
}

/// A call the replayed trace holds in the wait table (`waits::Hop`).
#[derive(Clone, Debug)]
struct Call {
    dst: String,
    id: String,
    seq: u64,
    /// The session that made it, when one did.
    src: Option<String>,
    /// The id the caller gave it.
    sid: Option<String>,
    /// The chain as this side read it, the call itself last.
    chain: Vec<(String, String)>,
    /// Nobody waits for it.
    entry: bool,
}

/// What the model says a call is (`RustForward`'s `kind`).
enum Expected {
    /// A hop: `under` is the marked call it was made under (`None` for the
    /// host's own call, #597); `entry` when nobody waits for it.
    Hop {
        under: Option<(String, String)>,
        entry: bool,
    },
    /// A call back on its caller's own chain.
    SameChain,
    /// Nobody waits for it: the model records it as an entry while a call it
    /// forwarded is in progress, and the table here need not have it.
    NotWaited,
}

#[derive(Default)]
struct Table {
    profiles: HashMap<String, Profile>,
    /// Calls a far end sent with `sync` and has not answered.
    marked: HashSet<(String, String)>,
    /// Hops in progress, in the order they were recorded.
    hops: Vec<Call>,
    ended: HashSet<String>,
    /// The last call number recorded on each session.
    last: HashMap<String, u64>,
}

impl Table {
    fn profile(&mut self, session: &str, stacked: bool, reentrant: bool) -> Result<(), String> {
        if self.ended.contains(session) {
            return Err(format!("{session} greets after it ended"));
        }
        let profile = Profile { stacked, reentrant };
        if let Some(before) = self.profiles.get(session) {
            if *before != profile {
                return Err(format!(
                    "the profile of {session} changes: {before:?} then {profile:?}: what a \
                     runtime does while it waits is a constant of the model (`sync-stack`, \
                     `reentrant-sync`)"
                ));
            }
        }
        self.profiles.insert(session.to_owned(), profile);
        Ok(())
    }

    fn marked(&mut self, session: &str, id: &str) -> Result<(), String> {
        if self.ended.contains(session) {
            return Err(format!("a call on {session} after it ended"));
        }
        if self
            .hops
            .iter()
            .any(|call| call.dst == session && call.id == id)
        {
            return Err(format!(
                "{session}/{id} is a call this side sent, not one its far end waits for"
            ));
        }
        if !self.marked.insert((session.to_owned(), id.to_owned())) {
            return Err(format!(
                "the call {session}/{id} is marked twice: one call waits for it"
            ));
        }
        Ok(())
    }

    fn answered(&mut self, session: &str, id: &str) -> Result<(), String> {
        if self.ended.contains(session) {
            return Err(format!("{session} answers after it ended"));
        }
        if self
            .hops
            .iter()
            .any(|call| call.dst == session && call.id == id)
        {
            return Err(format!(
                "{session}/{id} is a call this side sent: a reply this side sends belongs to \
                 a call of its far end, never to one of its own"
            ));
        }
        self.marked.remove(&(session.to_owned(), id.to_owned()));
        Ok(())
    }

    fn returned(&mut self, session: &str, id: &str) {
        // The reply to a hop, or a cancel: it waits for nothing any more. A
        // call the table never had (an asynchronous call nobody recorded)
        // returns nothing too.
        self.hops
            .retain(|call| !(call.dst == session && call.id == id));
    }

    fn ended(&mut self, session: &str) -> Result<(), String> {
        if let Some(call) = self
            .hops
            .iter()
            .find(|call| call.src.as_deref() == Some(session))
        {
            return Err(format!(
                "{session} ends while its call {}/{} waits: the model ends a session only \
                 when no call of it is in progress (`End`)",
                call.dst, call.id
            ));
        }
        self.hops
            .retain(|call| call.src.as_deref() != Some(session) && call.dst != session);
        self.marked.retain(|(tag, _)| tag != session);
        self.profiles.remove(session);
        self.ended.insert(session.to_owned());
        Ok(())
    }

    fn forward(&mut self, forward: &Forward, notes: &mut Vec<String>) -> Result<(), String> {
        let dst = forward.dst.as_str();
        if self.ended.contains(dst) {
            return Err(format!("a call into {dst} after it ended"));
        }
        if !self.profiles.contains_key(dst) {
            // Noted nowhere on purpose: another run of this test binary may
            // have greeted the session before this capture started, and the
            // table treats a session it has no profile for as `default()`
            // (`waits.rs`), as the replay does.
        }
        let chain = decode(&forward.path, dst);
        let caller = chain
            .last()
            .cloned()
            .filter(|call| self.marked.contains(call));
        let expected = match &caller {
            Some((src, sid)) if *src == dst => Expected::SameChain,
            Some((src, sid)) => Expected::Hop {
                under: Some((src.clone(), sid.clone())),
                entry: false,
            },
            None if forward.synchronous => Expected::Hop {
                under: None,
                entry: false,
            },
            None => Expected::NotWaited,
        };
        let call = |under: Option<(String, String)>, entry: bool| {
            let mut chain = chain.clone();
            chain.push((dst.to_owned(), forward.id.clone()));
            Call {
                dst: dst.to_owned(),
                id: forward.id.clone(),
                seq: forward.seq,
                src: under.as_ref().map(|(src, _)| src.clone()),
                sid: under.as_ref().map(|(_, sid)| sid.clone()),
                chain,
                entry,
            }
        };
        match (&forward.decision, &expected) {
            (Decision::Hop { src, entry }, Expected::Hop { under, entry: want }) => {
                if src != under {
                    return Err(format!(
                        "the model records {dst}/{} as a hop under {under:?} (`RustForward`: \
                         the chain's last call, when its caller waits), the table as {src:?}",
                        forward.id
                    ));
                }
                if entry != want {
                    return Err(format!(
                        "the model records {dst}/{} with `entry` {want}, the table with {entry}",
                        forward.id
                    ));
                }
                self.hop(call(under.clone(), *entry), forward)?;
            }
            (Decision::SameChain { caller: recorded }, Expected::SameChain) => {
                if Some(recorded) != caller.as_ref() {
                    return Err(format!(
                        "the table has {dst}/{} back on {recorded:?}'s chain, but the marked \
                         call its chain names last is {caller:?}",
                        forward.id
                    ));
                }
            }
            (Decision::Hop { entry, .. }, Expected::NotWaited) => {
                // The model has this call as an entry while a forwarded call
                // is in progress, and `waits.rs` asks a process-wide count of
                // marked calls before it records one: a mark cleared on
                // another thread between that count and this call leaves an
                // entry here with nothing marked, which the model allows (a
                // call nobody waits for).
                if !*entry {
                    return Err(format!(
                        "the table records {dst}/{} as a hop nobody waits for: no marked \
                         call is on its chain and its caller does not wait",
                        forward.id
                    ));
                }
                self.hop(call(None, true), forward)?;
            }
            (Decision::Hop { src, .. }, Expected::SameChain) => {
                return Err(format!(
                    "the table recorded {dst}/{} as a hop under {src:?}, but the call goes \
                     back into the session it came from: on its caller's chain, which runs it, \
                     not a hop of this table",
                    forward.id
                ));
            }
            (Decision::SameChain { caller }, _) => {
                return Err(format!(
                    "the table has {dst}/{} back on {caller:?}'s chain, but the model has no \
                     marked call there: it is a hop",
                    forward.id
                ));
            }
            (Decision::NotWaited { why }, Expected::NotWaited) => {
                let _ = why;
            }
            (Decision::NotWaited { why }, Expected::Hop { under: None, .. }) => {
                // A synchronous call the table leaves out: the model's
                // `hidden` kind, a far end that does not declare `sync-wait`
                // (SYN-9-26). Not guaranteed, and noted.
                notes.push(format!(
                    "the synchronous call {dst}/{} was not recorded ({why}): the model \
                     records a hidden wait as an entry",
                    forward.id
                ));
            }
            (
                Decision::NotWaited { why },
                Expected::Hop {
                    under: Some(under), ..
                },
            ) => {
                return Err(format!(
                    "the table leaves {dst}/{} out ({why}), but it is on the chain of the \
                     marked call {under:?}, which waits for it",
                    forward.id
                ));
            }
            (Decision::NotWaited { why }, Expected::SameChain) => {
                return Err(format!(
                    "the table leaves {dst}/{} out ({why}), but it is a call back on its \
                     caller's own chain",
                    forward.id
                ));
            }
            (Decision::Refused { calls }, Expected::Hop { under, .. }) => {
                let mut all = self.hops.clone();
                all.push(call(under.clone(), false));
                let Some(found) = cycle(&all, &self.profiles) else {
                    return Err(format!(
                        "the table refused {dst}/{} ({}), but the model's graph has no cycle \
                         there (`NoFalseCycle`)",
                        forward.id,
                        calls.join(" → ")
                    ));
                };
                if !is_cycle(&all, &self.profiles, calls) {
                    return Err(format!(
                        "the table refused {dst}/{} naming {}, which is not the cycle it \
                         closes in the model's graph; one is {}",
                        forward.id,
                        calls.join(" → "),
                        found.join(" → ")
                    ));
                }
            }
            (Decision::Refused { calls }, _) => {
                return Err(format!(
                    "the table refused {dst}/{} ({}), but the model does not have it as a \
                     hop: nothing waits for it",
                    forward.id,
                    calls.join(" → ")
                ));
            }
        }
        Ok(())
    }

    /// Record a hop the model allows, asking first whether it would wait on
    /// itself: the model does not forward such a call (`RustForward`).
    fn hop(&mut self, call: Call, forward: &Forward) -> Result<(), String> {
        if let Some(last) = self.last.get(&call.dst) {
            if call.seq <= *last {
                return Err(format!(
                    "the calls of {} do not grow: number {} after {last} (`rustCtr`)",
                    call.dst, call.seq
                ));
            }
        }
        let mut all = self.hops.clone();
        all.push(call.clone());
        if let Some(found) = cycle(&all, &self.profiles) {
            return Err(format!(
                "the call {}/{} waits on itself ({}), which the model does not forward",
                call.dst,
                forward.id,
                found.join(" → ")
            ));
        }
        self.last.insert(call.dst.clone(), call.seq);
        self.hops.push(call);
        Ok(())
    }
}

/// The chain `path` names, as calls of this process: an untagged entry is
/// `dst`'s, a tagged one (`s1/node:3`) the tagged session's (`waits.rs`,
/// `decode`).
fn decode(path: &[String], dst: &str) -> Vec<(String, String)> {
    path.iter()
        .map(|entry| match entry.split_once('/') {
            Some((tag, id)) => (tag.to_owned(), id.to_owned()),
            None => (dst.to_owned(), entry.clone()),
        })
        .collect()
}

/// The edges the model allows (`RustSucc`): chain, put aside and stack
/// order, over `calls` with the last one the call being added.
fn graph(calls: &[Call], profiles: &HashMap<String, Profile>) -> Vec<Vec<usize>> {
    let index: HashMap<(&str, &str), usize> = calls
        .iter()
        .enumerate()
        .map(|(at, call)| ((call.dst.as_str(), call.id.as_str()), at))
        .collect();
    // A call of a runtime that started it: a call out of that runtime names
    // it (`Started`).
    let mut named: HashSet<(&str, &str)> = HashSet::new();
    for call in calls.iter().filter(|call| !call.entry) {
        let Some(src) = call.src.as_deref() else {
            continue;
        };
        for (session, id) in &call.chain {
            if session == src {
                named.insert((session.as_str(), id.as_str()));
            }
        }
    }
    let started: Vec<bool> = calls
        .iter()
        .map(|call| named.contains(&(call.dst.as_str(), call.id.as_str())))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); calls.len()];
    // Chain: a call waits for the calls made under it, through the nearest
    // one; a call nobody waits for bounds them (nothing waits for what is
    // made under it).
    for (d, call) in calls.iter().enumerate() {
        if call.entry {
            continue;
        }
        let own = call.chain.len().saturating_sub(1);
        for (session, id) in call.chain[..own].iter().rev() {
            let gid = (session.as_str(), id.as_str());
            let Some(&c) = index.get(&gid) else {
                continue;
            };
            successors[c].push(d);
            if calls[c].entry {
                break;
            }
        }
    }
    // By runtime: the calls into it in order, and the calls out of it.
    let mut into: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut out: HashMap<&str, Vec<usize>> = HashMap::new();
    for (at, call) in calls.iter().enumerate() {
        into.entry(call.dst.as_str()).or_default().push(at);
        if !call.entry {
            if let Some(src) = call.src.as_deref() {
                out.entry(src).or_default().push(at);
            }
        }
    }
    for (y, mut targets) in into {
        let profile = profiles.get(y).copied().unwrap_or_default();
        // Only a runtime with one stack has either edge.
        if !profile.stacked {
            continue;
        }
        targets.sort_unstable_by_key(|&at| calls[at].seq);
        // Put aside: a call into a non-reentrant runtime with calls out, none
        // of them on its chain and none made under it, waits for all of them.
        if let Some(outs) = out.get(y).filter(|_| !profile.reentrant) {
            let sids: HashSet<&str> = outs
                .iter()
                .filter_map(|&o| calls[o].sid.as_deref())
                .collect();
            for &h in &targets {
                let call = &calls[h];
                let related = call
                    .chain
                    .iter()
                    .any(|(session, id)| session == y && sids.contains(id.as_str()));
                if !call.entry && !related && !started[h] {
                    successors[h].extend(outs.iter().copied().filter(|&o| o != h));
                }
            }
        }
        // Stack order: a call waits for every later call into the same
        // runtime that started there.
        for (at, &h) in targets.iter().enumerate() {
            for &d in &targets[at + 1..] {
                if started[d] {
                    successors[h].push(d);
                }
            }
        }
    }
    successors
}

/// One cycle through the last of `calls`, if the graph has one: its calls in
/// order, the last one first.
fn cycle(calls: &[Call], profiles: &HashMap<String, Profile>) -> Option<Vec<String>> {
    let successors = graph(calls, profiles);
    let new = calls.len() - 1;
    let mut from: Vec<Option<usize>> = vec![None; calls.len()];
    let mut seen = vec![false; calls.len()];
    let mut stack = vec![new];
    while let Some(at) = stack.pop() {
        for &next in &successors[at] {
            if next == new {
                let mut names = vec![name(&calls[new])];
                let mut back = Vec::new();
                let mut here = at;
                while here != new {
                    back.push(name(&calls[here]));
                    here = from[here].expect("reached from the new call");
                }
                names.extend(back.into_iter().rev());
                return Some(names);
            }
            if !seen[next] {
                seen[next] = true;
                from[next] = Some(at);
                stack.push(next);
            }
        }
    }
    None
}

/// Whether `names` (`session/id`) is a cycle of the graph over `calls`.
fn is_cycle(calls: &[Call], profiles: &HashMap<String, Profile>, names: &[String]) -> bool {
    if names.len() < 2 {
        return false;
    }
    let successors = graph(calls, profiles);
    let at = |name: &str| {
        let (session, id) = name.split_once('/')?;
        calls
            .iter()
            .position(|call| call.dst == session && call.id == id)
    };
    let Some(first) = at(&names[0]) else {
        return false;
    };
    // The cycle the new call closes starts at the new call.
    if first != calls.len() - 1 {
        return false;
    }
    let mut previous = first;
    for name in &names[1..] {
        let Some(next) = at(name) else {
            return false;
        };
        if !successors[previous].contains(&next) {
            return false;
        }
        previous = next;
    }
    successors[previous].contains(&first)
}

fn name(call: &Call) -> String {
    format!("{}/{}", call.dst, call.id)
}

#[cfg(test)]
mod tests;
