//! The one listener table (protocol spec §4): every entry, Rust or remote,
//! with its position and descriptor.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crate::ctx::Ctx;
use crate::events::{
    ErasedAsync, ErasedSync, ListenerInfo, ListenerKind, RemoteListener, TableEntry,
};
use crate::fiber::FiberInner;
use crate::FiberState;

pub(crate) enum Body {
    Async(Arc<dyn ErasedAsync>),
    Sync(Arc<dyn ErasedSync>),
    Remote(Arc<dyn RemoteListener>),
}

pub(crate) struct Entry {
    pub id: u64,
    /// Registration order: smaller first (protocol spec §4.3).
    pub position: i64,
    /// The name, or the prefix of a prefix entry.
    pub name: Arc<str>,
    pub prefix: bool,
    pub once: bool,
    pub prepend: bool,
    pub info: Arc<ListenerInfo>,
    pub origin: Option<u64>,
    pub body: Body,
    /// The registering context: what a Rust listener is called with.
    pub ctx: Ctx,
    pub owner: Weak<FiberInner>,
    pub generation: u64,
    pub withdrawn: AtomicBool,
    /// A `once` entry's first call started ([`super::EventBus::start`]).
    pub claimed: AtomicBool,
    pub calls: AtomicU64,
}

impl Entry {
    pub fn kind(&self) -> ListenerKind {
        match &self.body {
            Body::Async(_) => ListenerKind::Async,
            Body::Sync(_) => ListenerKind::Sync,
            Body::Remote(remote) => remote.kind(),
        }
    }

    pub fn table_entry(&self) -> TableEntry {
        TableEntry {
            id: self.id,
            name: self.name.to_string(),
            prefix: self.prefix,
            origin: self.origin,
            kind: self.kind(),
            info: self.info.clone(),
            prepend: self.prepend,
            position: self.position,
        }
    }

    /// Not withdrawn, and its owner's generation is loading or active.
    pub fn live(&self) -> bool {
        if self.withdrawn.load(Ordering::SeqCst) {
            return false;
        }
        self.owner.upgrade().is_some_and(|owner| {
            if !owner.alive.load(Ordering::SeqCst)
                || owner.closing.load(Ordering::SeqCst)
                || owner.ctx.registration_open().is_err()
            {
                return false;
            }
            let snapshot = owner.snapshot_rx.borrow();
            snapshot.generation == self.generation
                && matches!(snapshot.state, FiberState::Loading | FiberState::Active)
        })
    }

    /// Begin one call: count it on the owner (its unload waits for it, §7.1),
    /// then check it is still live. `None`: skip it, as if it had never been
    /// selected (§5.1 step 3). Through [`super::EventBus::start`], which
    /// claims a `once` entry.
    pub fn start(&self) -> Option<CallFlight> {
        let flight = CallFlight::begin(self.owner.upgrade()?);
        if !self.live() {
            return None;
        }
        Some(flight)
    }
}

/// One running call, counted on its owner fiber.
pub(crate) struct CallFlight(Arc<FiberInner>);

impl CallFlight {
    /// Count a call on `owner`; check its liveness after this.
    pub fn begin(owner: Arc<FiberInner>) -> Self {
        owner.begin_event();
        Self(owner)
    }
}

impl Drop for CallFlight {
    fn drop(&mut self) {
        self.0.finish_event();
    }
}

/// The entries an `emit` of one name dispatches over, shared by the emits
/// queued on that name (#977), and the entries registered since the list was
/// built (#980).
pub(crate) struct Selected {
    /// The list built when the name was last selected, in call order.
    base: Arc<[Arc<Entry>]>,
    /// Prepends registered after `base` was built, in call order: the last
    /// one first, its position being the smallest.
    before: Vec<Arc<Entry>>,
    /// Appends registered after `base` was built, in call order.
    after: Vec<Arc<Entry>>,
}

impl Selected {
    pub fn from_vec(entries: Vec<Arc<Entry>>) -> Self {
        Self {
            base: entries.into(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    /// Every entry, in call order: registration order by position, prepends
    /// before the list they were prepended to.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Entry>> {
        self.before
            .iter()
            .chain(self.base.iter())
            .chain(self.after.iter())
    }

    pub fn to_vec(&self) -> Vec<Arc<Entry>> {
        self.iter().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.before.is_empty() && self.after.is_empty()
    }
}

/// The entries an `emit` of one name dispatches over, as kept between the
/// emits of that name (#977).
///
/// Registering a listener adds it here instead of throwing the list away
/// (#980): rebuilding the list for every registration, and asking every
/// listener whether it is still live, made creating sessions with a listener
/// each cost time in proportion to the square of their number. The added
/// entries are weak, so a withdrawn listener is not kept here, and the list
/// is rebuilt once half of what it holds was removed or [`ADDED`] listeners
/// were added, which keeps the cost of one registration amortized.
struct Shared {
    list: Weak<[Arc<Entry>]>,
    len: usize,
    /// Entries of `list` removed from the table since it was built.
    removed: usize,
    /// Prepends added since `list` was built, in call order.
    before: Vec<Weak<Entry>>,
    /// Appends added since `list` was built, in call order.
    after: Vec<Weak<Entry>>,
}

/// How many entries are added to a list before it is rebuilt. Bounds what one
/// emit walks beside the list (an `emit` dispatches to every listener anyway)
/// and keeps the rebuild, which asks every listener whether it is live, to
/// one per that many registrations.
const ADDED: usize = 32;

impl Shared {
    fn add(&mut self, entry: &Arc<Entry>) {
        if entry.prepend {
            self.before.insert(0, Arc::downgrade(entry));
        } else {
            self.after.push(Arc::downgrade(entry));
        }
    }

    /// Half of what it holds is not in the table any more, or enough was
    /// added: rebuild the list on the next emit.
    fn stale(&self) -> bool {
        self.removed * 2 > self.len || self.before.len() + self.after.len() >= ADDED
    }
}

#[derive(Default)]
pub(crate) struct Table {
    exact: HashMap<Arc<str>, Vec<Arc<Entry>>>,
    prefixes: Vec<Arc<Entry>>,
    shared: HashMap<Arc<str>, Shared>,
    /// `shared`'s size at which its unused lists are dropped.
    shared_prune: usize,
    by_id: HashMap<u64, Arc<Entry>>,
    back: i64,
    front: i64,
    next_id: u64,
}

fn shrink_if_sparse<K: Eq + std::hash::Hash, V>(map: &mut HashMap<K, V>) {
    if map.capacity() > 64 && map.len() * 4 < map.capacity() {
        map.shrink_to_fit();
    }
}

fn shrink_vec_if_sparse<T>(list: &mut Vec<T>) {
    if list.capacity() > 64 && list.len() * 4 < list.capacity() {
        list.shrink_to_fit();
    }
}

impl Table {
    pub fn next_id(&mut self) -> u64 {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("event entry ids exhausted");
        self.next_id
    }

    pub fn position(&mut self, prepend: bool) -> i64 {
        if prepend {
            self.front -= 1;
            self.front
        } else {
            self.back += 1;
            self.back
        }
    }

    pub fn insert(&mut self, entry: Arc<Entry>) {
        // A prefix matches names this cannot list: their lists are rebuilt.
        // An exact entry is added to the list the emits of its name hold
        // (#980), unless that list is full of additions already.
        if entry.prefix {
            self.shared
                .retain(|name, _| !name.starts_with(&*entry.name));
        } else if self.shared.get_mut(&entry.name).is_some_and(|shared| {
            shared.add(&entry);
            shared.stale()
        }) {
            self.shared.remove(&entry.name);
        }
        let list = if entry.prefix {
            &mut self.prefixes
        } else {
            self.exact.entry(entry.name.clone()).or_default()
        };
        // Appends have the largest position, prepends the smallest.
        if entry.prepend {
            list.insert(0, entry.clone());
        } else {
            list.push(entry.clone());
        }
        self.by_id.insert(entry.id, entry);
    }

    /// Remove it; `false` when it was not in the table any more.
    pub fn remove(&mut self, entry: &Arc<Entry>) -> bool {
        if self.by_id.remove(&entry.id).is_none() {
            return false;
        }
        // A shared list keeps it until the emits holding the list are done
        // (it is withdrawn, so they skip it); rebuilt once half of it is
        // gone, so the emits of a name hold a few lists, not one per change.
        // An entry added to a shared list since it was built is not in it.
        let stale = |shared: &mut Shared| {
            shared.removed += 1;
            shared.list.strong_count() == 0 || shared.removed * 2 > shared.len
        };
        if entry.prefix {
            self.shared
                .retain(|name, shared| !(name.starts_with(&*entry.name) && stale(shared)));
        } else if self.shared.get_mut(&entry.name).is_some_and(stale) {
            self.shared.remove(&entry.name);
        }
        shrink_if_sparse(&mut self.shared);
        if entry.prefix {
            self.prefixes.retain(|e| !Arc::ptr_eq(e, entry));
            shrink_vec_if_sparse(&mut self.prefixes);
        } else if let Some(list) = self.exact.get_mut(&entry.name) {
            list.retain(|e| !Arc::ptr_eq(e, entry));
            shrink_vec_if_sparse(list);
            if list.is_empty() {
                self.exact.remove(&entry.name);
                shrink_if_sparse(&mut self.exact);
            }
        }
        shrink_if_sparse(&mut self.by_id);
        true
    }

    pub fn get(&self, id: u64) -> Option<Arc<Entry>> {
        self.by_id.get(&id).cloned()
    }

    /// §5.1 step 1: the live entries matching `name`, in registration
    /// order. Delivery is by name alone (Cordis's flat bus); a listener's
    /// scope is a descriptor for filters, not a routing rule. A `once`
    /// entry stays until a call of it starts.
    pub fn select(&self, name: &str) -> Vec<Arc<Entry>> {
        let mut out: Vec<Arc<Entry>> = Vec::new();
        let take = |entry: &Arc<Entry>| entry.live();
        if let Some(list) = self.exact.get(name) {
            out.extend(list.iter().filter(|e| take(e)).cloned());
        }
        if !self.prefixes.is_empty() {
            let before = out.len();
            out.extend(
                self.prefixes
                    .iter()
                    .filter(|e| name.starts_with(&*e.name) && take(e))
                    .cloned(),
            );
            if before > 0 && out.len() > before {
                out.sort_by_key(|e| e.position);
            }
        }
        out
    }

    /// For an `emit` without a filter: the entries of `name`, as one list
    /// shared by the emits of `name` (#977) plus the entries registered
    /// since it was built (#980); `None` when no entry is live.
    ///
    /// The list asks each listener whether it is live when it is built, and
    /// not again until it is rebuilt: a registration adds the listener to the
    /// list the emits hold instead of throwing it away (#980). Rebuilding per
    /// registration made every queued emit check every listener, so creating
    /// and closing instances cost time in proportion to the square of their
    /// number. A call skips a dead entry itself (`Entry::start`), as it skips
    /// any entry that stopped being live after its dispatch selected it: an
    /// entry that is not live never is again (it is withdrawn, its owner is
    /// closing or gone, or the generation it was registered in has ended).
    pub fn shared(&mut self, name: &Arc<str>) -> Option<Selected> {
        let base = match self.shared.get(name).and_then(|s| {
            let base = s.list.upgrade()?;
            Some((
                base,
                s.before
                    .iter()
                    .filter_map(Weak::upgrade)
                    .collect::<Vec<_>>(),
                s.after.iter().filter_map(Weak::upgrade).collect::<Vec<_>>(),
            ))
        }) {
            Some(parts) => parts,
            None => {
                let mut all: Vec<Arc<Entry>> = Vec::new();
                if let Some(list) = self.exact.get(&**name) {
                    all.extend(list.iter().filter(|e| e.live()).cloned());
                }
                let exact = all.len();
                all.extend(
                    self.prefixes
                        .iter()
                        .filter(|e| name.starts_with(&*e.name) && e.live())
                        .cloned(),
                );
                if all.is_empty() {
                    self.shared.remove(name);
                    return None;
                }
                if exact > 0 && all.len() > exact {
                    all.sort_by_key(|e| e.position);
                }
                let list: Arc<[Arc<Entry>]> = all.into();
                if self.shared.len() >= self.shared_prune {
                    self.shared
                        .retain(|_, shared| shared.list.strong_count() > 0);
                    self.shared_prune = (self.shared.len() * 2).max(64);
                }
                self.shared.insert(
                    name.clone(),
                    Shared {
                        list: Arc::downgrade(&list),
                        len: list.len(),
                        removed: 0,
                        before: Vec::new(),
                        after: Vec::new(),
                    },
                );
                return Some(Selected {
                    base: list,
                    before: Vec::new(),
                    after: Vec::new(),
                });
            }
        };
        let (base, before, after) = base;
        let selected = Selected {
            base,
            before,
            after,
        };
        if selected.is_empty() || !selected.iter().any(|e| e.live()) {
            return None;
        }
        Some(selected)
    }

    pub fn entries(&self) -> Vec<Arc<Entry>> {
        let mut all: Vec<_> = self.by_id.values().cloned().collect();
        all.sort_by_key(|e| e.position);
        all
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[cfg(test)]
    pub fn names(&self) -> usize {
        self.exact.len()
    }

    #[cfg(test)]
    pub fn capacities(&self) -> (usize, usize) {
        (self.exact.capacity(), self.by_id.capacity())
    }

    #[cfg(test)]
    pub fn max_list_capacity(&self) -> usize {
        self.exact
            .values()
            .map(Vec::capacity)
            .chain([self.prefixes.capacity()])
            .max()
            .unwrap_or(0)
    }
}
