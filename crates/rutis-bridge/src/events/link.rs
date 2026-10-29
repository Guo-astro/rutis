//! Events between two rutis kernels on a session (events protocol §9):
//! the listeners of one become entries of the other's logical bus, every
//! name, as the listeners of a runtime are (one flat bus).
//!
//! Each kernel announces its own entries (`events.on`) and accepts those
//! the far end announces. An accepted entry carries the session's scope as
//! a descriptor; a call of it is one `events.call` back to the far end,
//! which runs that one entry. Entries that came over a session are never
//! announced again: events go one hop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use rutis::events::{ListenerInfo, Mode, Scope};
use rutis::host::{BoxFuture, DynPlugin, Effect, Key, TypeKey};
use rutis::host::{CtxHost, ErrorKind, ListenerKind};
use rutis::internal::events::{EntryCall, RawTerminal, RemoteEntry, TableChange, TableEntry};
use rutis::internal::{CtxInternal, EventOptions};
use rutis::{Ctx, Disposer, Error as RutisError};
use serde_json::{json, Value as Json};

use super::args::{from_wire, result_from_wire, result_to_wire, SessionTranscoder};
use super::errors::{binding, to_cordis, to_session};
use super::hub::{dispose_now, next_origin, WireEntry};
use crate::channel::PeerId;
use crate::peer::{peer_key, Offered, Peer};
use crate::session::rpc::with_caller;
use crate::session::{settle, Connection, Error, Reference, Reply, Value, WeakConnection};

/// The origins of entries imported over links: never announced again.
static LINK_ORIGINS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn imported(origin: Option<u64>) -> bool {
    origin.is_some_and(|origin| LINK_ORIGINS.lock().unwrap().contains(&origin))
}

/// The events of a session with a far rutis kernel, on its `Peer#<peer>`.
pub(crate) struct NodeEvents {
    label: String,
    key: Key<Peer>,
    peer: PeerId,
}

impl NodeEvents {
    pub(crate) fn new(peer: PeerId) -> Self {
        Self {
            label: format!("rutis-bridge/events#{peer}"),
            key: peer_key(&peer),
            peer,
        }
    }
}

/// One session of the link's events.
struct Link {
    ctx: Ctx,
    peer: PeerId,
    connection: WeakConnection,
    /// Where the link is, and so where what it imports sits.
    scope: Vec<Scope>,
    origin: u64,
    state: Mutex<LinkState>,
}

#[derive(Default)]
struct LinkState {
    /// The peer took `events.on`: announcements go out as they happen.
    announcing: bool,
    /// Announcing as entries register (not the whole table at once): a
    /// `prepend` entry is announced as one.
    live: bool,
    /// The table's snapshot is in `table`.
    ready: bool,
    /// Changes seen before the snapshot was in.
    queued: Vec<TableChange>,
    /// This bus's entries, by kernel id.
    table: HashMap<u64, TableEntry>,
    /// Own entries announced: kernel entry id -> (announced id, name).
    announced: HashMap<u64, Vec<(String, String)>>,
    /// Announced id -> the kernel entry it calls.
    calls: HashMap<String, u64>,
    /// The peer's entries accepted here, by its id.
    accepted: HashMap<String, Vec<Disposer>>,
    next: u64,
    /// The epoch of this node's offer of `events` on the session: the
    /// start of each id it announces, so the peer tells the announcements
    /// of one offer from those of another.
    epoch: u64,
    /// The latest epoch of the peer's offer of `events` seen here: what the
    /// peer announced under an older one is gone.
    seen: u64,
    /// The latest epoch of the peer's offer of `events` its offers showed
    /// (`link.offers`). `seen` can be ahead of it: an announcement can come
    /// before its offer, which the peer announces on a task.
    shown: Option<u64>,
}

/// The offer (epoch) an announced id was made under, `<epoch>.<n>`, if it
/// says (a node of another implementation may not: its entries are
/// replaced by id only, as before).
fn epoch_of(id: &str) -> Option<u64> {
    id.split_once('.').and_then(|(epoch, _)| epoch.parse().ok())
}

impl Link {
    /// Whether this node's entry goes to the peer at all: its own (not
    /// imported). Delivery is by name.
    fn own(&self, entry: &TableEntry) -> bool {
        !imported(entry.origin) && !super::hub::process_local(&entry.name)
    }

    fn send(&self, method: &str, argument: Json) {
        let Some(connection) = self.connection.upgrade() else {
            return;
        };
        // A table change of this node, whichever call made it: it carries no
        // chain of that call.
        if let (Ok(answer), Ok(runtime)) = (
            crate::session::rpc::unchained(|| {
                connection.invoke_started("", method, Value::List(vec![Value::Data(argument)]))
            }),
            tokio::runtime::Handle::try_current(),
        ) {
            runtime.spawn(async move {
                let _ = answer.await;
            });
        }
    }

    /// Announce `entry` to the peer.
    fn announce(&self, state: &mut LinkState, entry: &TableEntry) {
        if !self.own(entry) || state.announced.contains_key(&entry.id) {
            return;
        }
        let mut sent = Vec::new();
        {
            let (name, prefix) = (entry.name.clone(), entry.prefix);
            state.next += 1;
            let id = format!("{}.{}", state.epoch, state.next);
            state.calls.insert(id.clone(), entry.id);
            self.send(
                "events.on",
                json!({
                    "id": id,
                    "name": name,
                    "prefix": prefix,
                    "plugin": entry.info.plugin,
                    "row": entry.info.row,
                    "runtime": entry.info.runtime,
                    "kind": entry.kind,
                    // Registered in front: it goes in front on the peer
                    // too (when announced as it registers; the first
                    // announcement goes in table order).
                    "prepend": entry.prepend && state.live,
                    "global": entry.info.global,
                }),
            );
            sent.push((id, name));
        }
        if !sent.is_empty() {
            state.announced.insert(entry.id, sent);
        }
    }

    fn withdraw(&self, state: &mut LinkState, entry: &TableEntry) {
        let Some(sent) = state.announced.remove(&entry.id) else {
            return;
        };
        let ids: Vec<String> = sent.into_iter().map(|(id, _)| id).collect();
        for id in &ids {
            state.calls.remove(id);
        }
        self.send("events.off", json!(ids));
    }

    fn changed(&self, change: &TableChange) {
        let mut state = self.state.lock().unwrap();
        if !state.ready {
            state.queued.push(change.clone());
            return;
        }
        self.apply(&mut state, change);
    }

    fn apply(&self, state: &mut LinkState, change: &TableChange) {
        match change {
            TableChange::Added(entry) => {
                state.table.insert(entry.id, entry.clone());
                if state.announcing {
                    self.announce(state, entry);
                }
            }
            TableChange::Removed(entry) => {
                state.table.remove(&entry.id);
                self.withdraw(state, entry);
            }
        }
    }

    /// The peer offers `events` anew: it knows nothing of before, and hears
    /// every own entry now.
    fn offered(&self) {
        let mut state = self.state.lock().unwrap();
        state.announced.clear();
        state.calls.clear();
        state.announcing = true;
        // In table order, each appended there: the peer's order is this.
        let mut entries: Vec<TableEntry> = state.table.values().cloned().collect();
        entries.sort_by_key(|entry| entry.position);
        state.live = false;
        for entry in &entries {
            self.announce(&mut state, entry);
        }
        state.live = true;
    }

    /// The peer's offer of `events` is now `now`: what it announced under
    /// an older offer is gone with that offer (its events feature
    /// restarted, or stopped), and is withdrawn here. An announcement of a
    /// newer offer, come before the offer itself, stays.
    fn peer_offer(&self, now: Option<u64>) {
        let gone = Self::offer_now(&mut self.state.lock().unwrap(), now);
        for disposer in gone {
            dispose_now(disposer);
        }
    }

    /// [`Link::peer_offer`] on `state`: the accepted entries now gone.
    fn offer_now(state: &mut LinkState, now: Option<u64>) -> Vec<Disposer> {
        match now {
            Some(epoch) => {
                state.shown = state.shown.max(Some(epoch));
                Self::newer(state, epoch)
            }
            // The offer shown last is withdrawn: what was announced under
            // it or before goes. What a later offer announced, come before
            // that offer, stays (#530).
            None => match state.shown {
                Some(shown) => Self::older_than(state, shown.saturating_add(1)),
                None => Vec::new(),
            },
        }
    }

    /// An announcement made under the peer's offer `epoch` arrived: `None`
    /// if that offer is already gone (the entry is not taken), else the
    /// entries of older offers, gone now.
    fn announced_under(state: &mut LinkState, epoch: u64) -> Option<Vec<Disposer>> {
        if epoch < state.seen {
            return None;
        }
        Some(Self::newer(state, epoch))
    }

    /// The peer's offer `epoch` is seen: the entries of older offers.
    fn newer(state: &mut LinkState, epoch: u64) -> Vec<Disposer> {
        if epoch <= state.seen {
            return Vec::new();
        }
        state.seen = epoch;
        Self::older_than(state, epoch)
    }

    /// Take out the accepted entries announced under an offer before
    /// `epoch`.
    fn older_than(state: &mut LinkState, epoch: u64) -> Vec<Disposer> {
        let ids: Vec<String> = state
            .accepted
            .keys()
            .filter(|id| epoch_of(id).is_some_and(|of| of < epoch))
            .cloned()
            .collect();
        ids.iter()
            .filter_map(|id| state.accepted.remove(id))
            .flatten()
            .collect()
    }

    /// The peer announced one of its entries.
    fn accept(&self, announced: Value) -> Reply {
        #[derive(serde::Deserialize)]
        struct Announced {
            id: String,
            name: String,
            #[serde(default)]
            prefix: bool,
            #[serde(default)]
            plugin: String,
            #[serde(default)]
            row: Option<String>,
            #[serde(default)]
            runtime: Option<String>,
            kind: ListenerKind,
            #[serde(default)]
            prepend: bool,
            #[serde(default)]
            global: bool,
        }
        let announced: Announced = crate::session::decode(announced.json()?)?;
        if announced.name.starts_with("internal/") {
            return Err(binding(format!(
                "event {} is internal to its runtime",
                announced.name
            )));
        }
        // Of an offer already gone: it is not taken. Of a newer one than
        // seen so far (its announcement may come before the offer): the
        // older offers' entries are gone.
        if let Some(epoch) = epoch_of(&announced.id) {
            let Some(gone) = Self::announced_under(&mut self.state.lock().unwrap(), epoch) else {
                return Ok(Value::Undefined);
            };
            for disposer in gone {
                dispose_now(disposer);
            }
        }
        let mut disposers = Vec::new();
        {
            let (name, prefix) = (announced.name.clone(), announced.prefix);
            let info = ListenerInfo {
                name: name.clone(),
                prefix,
                plugin: announced.plugin.clone(),
                row: announced.row.clone(),
                runtime: announced.runtime.clone(),
                node: Some(self.peer.to_string()),
                scope: self.scope.clone(),
                global: announced.global,
            };
            let entry = RemoteEntry {
                name,
                prefix,
                info,
                options: EventOptions::default()
                    .prepend(announced.prepend)
                    .global(announced.global),
                origin: self.origin,
            };
            let listener = Arc::new(WireEntry {
                connection: self.connection.clone(),
                id: announced.id.clone(),
                kind: announced.kind,
                origin: self.origin,
                owner: self.ctx.clone(),
            });
            disposers.push(
                self.ctx
                    .events()
                    .on_remote(&self.ctx, entry, listener)
                    .map_err(|error| to_session(&error))?,
            );
        }
        if !disposers.is_empty() {
            let previous = self
                .state
                .lock()
                .unwrap()
                .accepted
                .insert(announced.id, disposers);
            for disposer in previous.into_iter().flatten() {
                dispose_now(disposer);
            }
        }
        Ok(Value::Undefined)
    }

    fn forget(&self, ids: Value) -> Reply {
        let ids: Vec<String> = crate::session::decode(ids.json()?)?;
        let gone: Vec<Disposer> = {
            let mut state = self.state.lock().unwrap();
            ids.iter()
                .filter_map(|id| state.accepted.remove(id))
                .flatten()
                .collect()
        };
        for disposer in gone {
            dispose_now(disposer);
        }
        Ok(Value::Undefined)
    }

    /// The peer calls one of this node's entries it accepted.
    /// The peer's call of one of this bus's entries, on `source` (the
    /// link's session): what it calls in other sessions is forwarded from
    /// it, its chain rebased; it stops when the peer gives it up (§5.7).
    fn call(&self, source: &Connection, request: Value) -> Reply {
        let mut fields = match request {
            Value::Record(fields) => fields,
            Value::Data(Json::Object(fields)) => fields
                .into_iter()
                .map(|(key, value)| (key, Value::Data(value)))
                .collect(),
            _ => return Err(binding("events.call needs its call")),
        };
        let text =
            |fields: &mut std::collections::BTreeMap<String, Value>, field: &str| match fields
                .remove(field)
            {
                Some(Value::Data(Json::String(text))) => Ok(text),
                _ => Err(binding(format!("events.call needs a {field}"))),
            };
        let listener = text(&mut fields, "listener")?;
        let name = text(&mut fields, "name")?;
        let dispatch = text(&mut fields, "dispatch")?;
        let mode = match text(&mut fields, "mode")?.as_str() {
            "emit" => Mode::Emit,
            "parallel" => Mode::Parallel,
            "serial" => Mode::Serial,
            "waterfall" => Mode::Waterfall,
            "bail" => Mode::Bail,
            other => return Err(binding(format!("events.call: unknown mode {other}"))),
        };
        let args = match fields.remove("args") {
            None | Some(Value::Undefined) => Vec::new(),
            Some(args) => args.list()?,
        };
        let next = match fields.remove("next") {
            Some(Value::Reference(next)) if next.is_function() => Some(next),
            _ => None,
        };
        let withdrawn_message = format!("the listener of {name} is withdrawn");
        let withdrawn = move || binding(withdrawn_message.clone());
        let id = *self
            .state
            .lock()
            .unwrap()
            .calls
            .get(&listener)
            .ok_or_else(withdrawn.clone())?;
        let call = EntryCall {
            name: name.clone(),
            mode,
            payload: from_wire(args),
            next: next.map(|next| next_terminal(next, mode)),
            dispatch: dispatch_id(self.origin, &dispatch),
            origin: Some(self.origin),
        };
        let ctx = self.ctx.clone();
        let mapped = move |error: RutisError| match &*error {
            ErrorKind::ListenerWithdrawn { .. } => withdrawn(),
            error => to_session(error),
        };
        if mode.is_sync() {
            let result = ctx
                .events()
                .invoke_entry_sync(&ctx, id, call)
                .map_err(mapped)?;
            return result_to_wire(result);
        }
        let run = with_caller(source.clone(), async move {
            let result = ctx
                .events()
                .invoke_entry(&ctx, id, call)
                .await
                .map_err(mapped)?;
            result_to_wire(result)
        });
        if mode != Mode::Emit {
            return Ok(Value::cancellable_future(run));
        }
        // `emit` invocations start in arrival order (§5.5): this one starts
        // here, in its turn on the session's ordered task (never on its
        // reader), before the next event operation; the rest runs on a task.
        let mut run: rutis::host::BoxFuture<'static, Reply> = Box::pin(run);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        if let std::task::Poll::Ready(reply) = run.as_mut().poll(&mut context) {
            return reply;
        }
        let (answer, answered) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = answer.send(run.await);
        });
        Ok(Value::future(async move {
            answered
                .await
                .unwrap_or_else(|_| Err(binding("the emit was dropped")))
        }))
    }
}

/// The far end's dispatch id as one of this bus's: the same for every call
/// of that dispatch over this link.
fn dispatch_id(origin: u64, dispatch: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (origin, dispatch).hash(&mut hasher);
    hasher.finish() | (1 << 63)
}

/// The far end's `next` as the rest of the chain here.
fn next_terminal(next: Reference, mode: Mode) -> RawTerminal {
    RawTerminal::Async(Box::new(move |_| {
        Box::pin(async move {
            let reply = next
                .call_async(Value::List(Vec::new()))
                .await
                .map_err(to_cordis)?;
            let value = settle(reply).await.map_err(to_cordis)?;
            Ok(result_from_wire(value, mode))
        })
    }))
}

/// Serves `events.*` from the peer.
struct Handler(Weak<Link>);

impl crate::peer::Handler for Handler {
    fn invoke(&self, connection: &Connection, _: &str, method: &str, args: Value) -> Reply {
        let link = self
            .0
            .upgrade()
            .ok_or_else(|| Error::Transport("the link's events are gone".into()))?;
        let first = args.list()?.into_iter().next().unwrap_or(Value::Undefined);
        match method {
            "events.on" => link.accept(first),
            "events.off" => link.forget(first),
            "events.call" => link.call(connection, first),
            _ => Err(binding(format!("{method} is not offered here"))),
        }
    }
}

impl DynPlugin for NodeEvents {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![self.key.erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let peer = ctx.get_at(&self.key).ok_or_else(|| {
                rutis::Error::from(ErrorKind::PluginFailed(
                    format!("{}: the peer is gone", self.label).into(),
                ))
            })?;
            ctx.events().set_transcoder(Arc::new(SessionTranscoder));
            let origin = next_origin();
            LINK_ORIGINS.lock().unwrap().push(origin);
            let link = Arc::new(Link {
                ctx: ctx.clone(),
                peer: self.peer.clone(),
                connection: peer.connection().downgrade(),
                scope: ctx.scope(),
                origin,
                state: Mutex::default(),
            });
            let offered: Offered =
                peer.register("events", Arc::new(Handler(Arc::downgrade(&link))))?;
            link.state.lock().unwrap().epoch = offered.epoch();
            // Own entries change from now on; they are announced once the
            // peer takes them.
            let watched = Arc::downgrade(&link);
            let (snapshot, watching) = ctx.events().watch_table(ctx, move |change| {
                if let Some(link) = watched.upgrade() {
                    link.changed(change);
                }
            })?;
            {
                let mut state = link.state.lock().unwrap();
                for entry in snapshot {
                    state.table.insert(entry.id, entry);
                }
                for change in std::mem::take(&mut state.queued) {
                    link.apply(&mut state, &change);
                }
                state.ready = true;
            }
            let announcer = {
                let link = link.clone();
                let mut offers = peer.offers();
                tokio::spawn(async move {
                    let mut epoch = None;
                    loop {
                        let now = offers.borrow_and_update().epoch("events");
                        link.peer_offer(now);
                        if now.is_some() && now != epoch {
                            epoch = now;
                            link.offered();
                        }
                        if offers.changed().await.is_err() {
                            return;
                        }
                    }
                })
            };
            let kept = Arc::new(Mutex::new(Some((offered, link, watching))));
            ctx.effect(move || async move {
                announcer.abort();
                if let Some((offered, link, watching)) = kept.lock().unwrap().take() {
                    drop(offered);
                    dispose_now(watching);
                    let accepted: Vec<Disposer> =
                        std::mem::take(&mut link.state.lock().unwrap().accepted)
                            .into_values()
                            .flatten()
                            .collect();
                    for disposer in accepted {
                        dispose_now(disposer);
                    }
                    LINK_ORIGINS.lock().unwrap().retain(|o| *o != link.origin);
                }
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peer's `events.on` can come before its `link.offers` (the offer
    /// is announced on a task, the entry as the peer sees this node's
    /// offer). Its entry stays while the offers this node sees do not show
    /// that offer yet, and goes once the offer shown is withdrawn (#530).
    #[tokio::test]
    async fn an_entry_announced_before_its_offer_stays() {
        let root = Ctx::root().unwrap();
        let mut state = LinkState::default();
        let accept = |state: &mut LinkState, id: &str| {
            let gone = Link::announced_under(state, epoch_of(id).unwrap()).expect("taken");
            assert!(gone.is_empty());
            let disposer = root.effect_raw(|| Effect::Done).unwrap();
            state.accepted.insert(id.to_owned(), vec![disposer]);
        };
        // Offers seen without `events` (other families), then its entry.
        assert!(Link::offer_now(&mut state, None).is_empty());
        accept(&mut state, "3.1");
        // The offers change again before they show `events`.
        assert!(
            Link::offer_now(&mut state, None).is_empty(),
            "an entry of an offer not shown yet is not withdrawn"
        );
        assert!(state.accepted.contains_key("3.1"));
        assert!(Link::offer_now(&mut state, Some(3)).is_empty());
        assert!(state.accepted.contains_key("3.1"));
        // The offer shown is withdrawn: its entry goes.
        assert_eq!(Link::offer_now(&mut state, None).len(), 1);
        assert!(state.accepted.is_empty());
        // Offered anew; an entry of the newer offer, come first, stays
        // when the older one is seen withdrawn.
        accept(&mut state, "5.1");
        assert!(Link::offer_now(&mut state, None).is_empty());
        assert!(state.accepted.contains_key("5.1"));
    }

    #[test]
    fn announced_ids_carry_their_offer() {
        assert_eq!(epoch_of("7.12"), Some(7));
        assert_eq!(epoch_of("12"), None);
        assert_eq!(epoch_of("x.1"), None);
    }
}
