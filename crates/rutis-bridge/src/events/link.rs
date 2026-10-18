//! Events between nodes (events protocol §9): the listeners of one node
//! become entries of the other's logical bus, as the link's `export` and
//! `import` select them.
//!
//! `export: [x]` here means this node's emissions of `x` reach the peer's
//! listeners; `import: [x]` means this node's listeners hear the peer's
//! `x`. Entries therefore travel from importer to exporter: this node
//! announces its own entries matching its `import` (`events.on`), and
//! accepts those the peer announces that match its `export`. An accepted
//! entry sits at the link's scope; a call of it is one `events.call` back
//! to the peer, which runs that one entry. Entries imported over a link are
//! never announced again: events go one hop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use rutis::events::{
    scope_contains, EntryCall, EventOptions, ListenerInfo, ListenerKind, Mode, RawTerminal,
    RemoteEntry, Scope, TableChange, TableEntry,
};
use rutis::{BoxFuture, CordisError, Ctx, Disposer, Effect, Plugin, TypeKey};
use serde_json::{json, Value as Json};

use super::args::{from_wire, result_from_wire, result_to_wire, SessionTranscoder};
use super::errors::{binding, to_cordis, to_session};
use super::hub::{dispose_now, next_origin, WireEntry};
use crate::channel::PeerId;
use crate::session::rpc::with_caller;
use crate::session::{settle, Connection, Error, Reference, Reply, Value, WeakConnection};
use crate::{peer_key, Offered, Peer};

/// The origins of entries imported over links: never announced again.
static LINK_ORIGINS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn imported(origin: Option<u64>) -> bool {
    origin.is_some_and(|origin| LINK_ORIGINS.lock().unwrap().contains(&origin))
}

/// An item of `export` / `import`: a name, or a prefix (`session/*`, `*`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Name(String),
    Prefix(String),
}

impl Item {
    fn parse(item: &str) -> Self {
        match item.strip_suffix('*') {
            Some(prefix) => Item::Prefix(prefix.to_owned()),
            None => Item::Name(item.to_owned()),
        }
    }
}

/// What both an entry (`name`, a prefix when `prefix`) and `item` match
/// (events protocol §9): a name, a prefix, or nothing.
fn narrowed(name: &str, prefix: bool, item: &Item) -> Option<(String, bool)> {
    match (prefix, item) {
        (false, Item::Name(n)) => (n == name).then(|| (name.to_owned(), false)),
        (false, Item::Prefix(p)) => name.starts_with(p).then(|| (name.to_owned(), false)),
        (true, Item::Name(n)) => n.starts_with(name).then(|| (n.clone(), false)),
        (true, Item::Prefix(p)) => {
            if p.starts_with(name) {
                Some((p.clone(), true))
            } else if name.starts_with(p.as_str()) {
                Some((name.to_owned(), true))
            } else {
                None
            }
        }
    }
}

/// Everything `items` select of an entry, each once.
fn selected(name: &str, prefix: bool, items: &[Item]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for item in items {
        if let Some(found) = narrowed(name, prefix, item) {
            if !out.contains(&found) {
                out.push(found);
            }
        }
    }
    // A name a prefix also selects is that prefix's already.
    let prefixes: Vec<String> = out
        .iter()
        .filter(|(_, prefix)| *prefix)
        .map(|(p, _)| p.clone())
        .collect();
    out.retain(|(n, prefix)| *prefix || !prefixes.iter().any(|p| n.starts_with(p.as_str())));
    out
}

/// The events of one link, on its `Peer#<peer>`.
pub struct LinkEventsPlugin {
    label: String,
    injects: [TypeKey; 1],
    peer: PeerId,
    export: Vec<Item>,
    import: Vec<Item>,
}

impl LinkEventsPlugin {
    /// Share events with `peer`: `export` and `import` are its link's lists
    /// (names, `*` prefixes); service names in them select those events
    /// too.
    pub fn new(
        peer: PeerId,
        export: impl IntoIterator<Item = impl Into<String>>,
        import: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let items = |items: Vec<String>| items.iter().map(|item| Item::parse(item)).collect();
        Self {
            label: format!("rutis-bridge/events#{peer}"),
            injects: [peer_key(&peer)],
            peer,
            export: items(export.into_iter().map(Into::into).collect()),
            import: items(import.into_iter().map(Into::into).collect()),
        }
    }
}

/// One session of the link's events.
struct Link {
    ctx: Ctx,
    peer: PeerId,
    connection: WeakConnection,
    export: Vec<Item>,
    import: Vec<Item>,
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
}

impl Link {
    /// Whether this node's entry goes to the peer at all: its own (not
    /// imported), at or above the link's scope.
    fn own(&self, entry: &TableEntry) -> bool {
        !imported(entry.origin)
            && !super::hub::process_local(&entry.name)
            && scope_contains(&entry.info.scope, &self.scope)
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

    /// Announce `entry` to the peer, as `import` narrows it.
    fn announce(&self, state: &mut LinkState, entry: &TableEntry) {
        if !self.own(entry) || state.announced.contains_key(&entry.id) {
            return;
        }
        let mut sent = Vec::new();
        for (name, prefix) in selected(&entry.name, entry.prefix, &self.import) {
            state.next += 1;
            let id = format!("{}", state.next);
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
        let mut disposers = Vec::new();
        for (name, prefix) in selected(&announced.name, announced.prefix, &self.export) {
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
            "waterfall-sync" => Mode::WaterfallSync,
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
        let withdrawn_message = format!("listener {listener} of {name} is withdrawn");
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
        let mapped = move |error: CordisError| match error {
            CordisError::ListenerWithdrawn { .. } => withdrawn(),
            error => to_session(&error),
        };
        if mode.is_sync() {
            let result = ctx
                .events()
                .invoke_entry_sync(&ctx, id, call)
                .map_err(mapped)?;
            return result_to_wire(result);
        }
        Ok(Value::cancellable_future(with_caller(
            source.clone(),
            async move {
                let result = ctx
                    .events()
                    .invoke_entry(&ctx, id, call)
                    .await
                    .map_err(mapped)?;
                result_to_wire(result)
            },
        )))
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
    match mode {
        Mode::WaterfallSync => RawTerminal::Sync(Box::new(move |_| {
            let value = next.call(Value::List(Vec::new())).map_err(to_cordis)?;
            Ok(result_from_wire(value, mode))
        })),
        _ => RawTerminal::Async(Box::new(move |_| {
            Box::pin(async move {
                let reply = next
                    .call_async(Value::List(Vec::new()))
                    .await
                    .map_err(to_cordis)?;
                let value = settle(reply).await.map_err(to_cordis)?;
                Ok(result_from_wire(value, mode))
            })
        })),
    }
}

/// Serves `events.*` from the peer.
struct Handler(Weak<Link>);

impl crate::Handler for Handler {
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

impl Plugin for LinkEventsPlugin {
    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> &[TypeKey] {
        &self.injects
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let peer = ctx.get_as::<Peer>(self.injects[0].clone()).ok_or_else(|| {
                CordisError::PluginFailed(format!("{}: the peer is gone", self.label).into())
            })?;
            ctx.events().set_transcoder(Arc::new(SessionTranscoder));
            let origin = next_origin();
            LINK_ORIGINS.lock().unwrap().push(origin);
            let link = Arc::new(Link {
                ctx: ctx.clone(),
                peer: self.peer.clone(),
                connection: peer.connection().downgrade(),
                export: self.export.clone(),
                import: self.import.clone(),
                scope: ctx.scope(),
                origin,
                state: Mutex::default(),
            });
            let offered: Offered = peer
                .register("events", Arc::new(Handler(Arc::downgrade(&link))))
                .map_err(|error| CordisError::PluginFailed(error.to_string().into()))?;
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
            ctx.effect(move || {
                Effect::Disposer(Box::new(move || {
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
                }))
            })?;
            Ok(Effect::Done)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_narrow_entries() {
        let items = |list: &[&str]| list.iter().map(|i| Item::parse(i)).collect::<Vec<_>>();
        assert_eq!(
            selected("session/created", false, &items(&["session/*"])),
            vec![("session/created".to_owned(), false)]
        );
        assert!(selected("t/x", false, &items(&["n/*", "t/y"])).is_empty());
        // A prefix entry narrowed by a longer prefix, and by a name.
        assert_eq!(
            selected("t/", true, &items(&["t/a/*", "t/b"])),
            vec![("t/a/".to_owned(), true), ("t/b".to_owned(), false)]
        );
        // A shorter item keeps the entry's prefix.
        assert_eq!(
            selected("t/a/", true, &items(&["*"])),
            vec![("t/a/".to_owned(), true)]
        );
        // A name a selected prefix covers is not selected twice.
        assert_eq!(
            selected("t/", true, &items(&["t/*", "t/b"])),
            vec![("t/".to_owned(), true)]
        );
    }
}
