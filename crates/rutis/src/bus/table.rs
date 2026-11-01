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

pub(crate) enum Body {
    Async(Arc<dyn ErasedAsync>),
    Sync(Arc<dyn ErasedSync>),
    Remote(Arc<dyn RemoteListener>),
}

/// How many listeners the selection paths have asked whether they are live
/// (feature `testing`): a registration that appends to the list a name's emits
/// share asks nobody (#980).
#[cfg(feature = "testing")]
pub(crate) static SELECTION_CHECKS: AtomicU64 = AtomicU64::new(0);

/// [`Entry::live`] in the selection paths, counted under `testing`.
fn live_for_selection(entry: &Entry) -> bool {
    #[cfg(feature = "testing")]
    SELECTION_CHECKS.fetch_add(1, Ordering::Relaxed);
    entry.live()
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
            owner.live_generation.load(Ordering::SeqCst) == self.generation
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
///
/// The chains are held as they are, not turned into lists: the emits of a name
/// are queued one after another, and a copy made when each of them is queued
/// holds one more entry than the one before, so N queued emits would hold the
/// list N times over (#980).
pub(crate) struct Selected {
    /// The list built when the name was last selected, in call order.
    base: Arc<[Arc<Entry>]>,
    /// Prepends registered after `base` was built, the newest first: their
    /// call order, its position being the smallest.
    before: Option<Arc<Added>>,
    /// Appends registered after `base` was built, the newest last.
    after: Option<Arc<Added>>,
}

impl Selected {
    pub fn from_vec(entries: Vec<Arc<Entry>>) -> Self {
        Self {
            base: entries.into(),
            before: None,
            after: None,
        }
    }

    /// The list the snapshot was built from; what every emit dispatches over
    /// after the chains.
    pub fn base(&self) -> &[Arc<Entry>] {
        &self.base
    }

    /// The chains of what was registered since, in call order: prepends first,
    /// then appends. Built when the dispatch starts, not when the emit is
    /// queued.
    pub fn chains(&self) -> (Vec<Arc<Entry>>, Vec<Arc<Entry>>) {
        (
            Added::entries(self.before.as_ref(), true),
            Added::entries(self.after.as_ref(), false),
        )
    }

    /// Every entry, in call order: registration order by position, prepends
    /// before the list they were prepended to.
    pub fn to_vec(&self) -> Vec<Arc<Entry>> {
        let (before, after) = self.chains();
        before
            .into_iter()
            .chain(self.base.iter().cloned())
            .chain(after)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.before.is_none() && self.after.is_none()
    }

    /// Nothing was registered since the list was built.
    fn only_base(&self) -> bool {
        self.before.is_none() && self.after.is_none()
    }

    /// Whether the list holds a live entry: one check when its first entry is
    /// live. The appends are asked as they are called.
    fn any_live(&self) -> bool {
        self.base.iter().any(|entry| live_for_selection(entry))
    }
}

/// Entries appended to the list a name's emits share, after it was built: the
/// entry, and the one appended before it (#980).
struct Added {
    entry: Arc<Entry>,
    prev: Option<Arc<Added>>,
}

impl Added {
    /// The entries of the chain `head` starts, in call order: newest first (as
    /// prepends are called) or oldest first.
    fn entries(head: Option<&Arc<Added>>, newest_first: bool) -> Vec<Arc<Entry>> {
        let mut entries = Vec::new();
        let mut node = head;
        while let Some(current) = node {
            entries.push(current.entry.clone());
            node = current.prev.as_ref();
        }
        if !newest_first {
            entries.reverse();
        }
        entries
    }
}

/// The entries an `emit` of one name dispatches over, as kept between the
/// emits of that name (#977): the list built when the name was last selected,
/// and the chains of what was appended since (#980).
///
/// Registering a listener appends it here in constant time instead of throwing
/// the list away: rebuilding the list for every registration, and asking every
/// listener whether it is still live for every emit, made creating sessions
/// with a listener each cost time in proportion to the square of their number.
/// The chains die with the last emit that selected them, and the list is
/// rebuilt once half of it was removed, so a withdrawn listener is kept no
/// longer than it was before.
struct Shared {
    list: Weak<[Arc<Entry>]>,
    /// Entries in `list`.
    len: usize,
    /// Entries of `list` removed from the table since it was built.
    removed: usize,
    /// Prepends appended since, the newest first: their call order. Held
    /// here, as `list` is not, so that the appends of a name survive between
    /// its emits; the list is rebuilt once half of it was removed, which drops
    /// the chain with it.
    before: Option<Arc<Added>>,
    /// Appends appended since, the newest last.
    after: Option<Arc<Added>>,
}

impl Shared {
    /// Append `entry` to its chain, in constant time.
    fn add(&mut self, entry: &Arc<Entry>) {
        let node = Arc::new(Added {
            entry: entry.clone(),
            prev: if entry.prepend {
                self.before.clone()
            } else {
                self.after.clone()
            },
        });
        if entry.prepend {
            self.before = Some(node);
        } else {
            self.after = Some(node);
        }
    }

    /// Half of what it holds is not in the table any more, or an entry of the
    /// chains was removed (a chain cannot drop it, and the emits of the name
    /// would keep walking over it): rebuild the list on the next emit.
    fn stale(&self) -> bool {
        self.removed * 2 > self.len || self.before.is_some() || self.after.is_some()
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
        // A prefix matches names this cannot list: their lists are rebuilt. An
        // exact entry is appended to the list the emits of its name hold, in
        // constant time (#980).
        if entry.prefix {
            self.shared
                .retain(|name, _| !name.starts_with(&*entry.name));
        } else if let Some(shared) = self.shared.get_mut(&entry.name) {
            shared.add(&entry);
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
            shared.list.strong_count() == 0 || shared.stale()
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
        let take = |entry: &Arc<Entry>| live_for_selection(entry);
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
        let cached = self.shared.get(name).and_then(|s| {
            let base = s.list.upgrade()?;
            Some((base, s.before.clone(), s.after.clone()))
        });
        let (base, before, after) = match cached {
            Some(parts) => parts,
            None => {
                let mut all: Vec<Arc<Entry>> = Vec::new();
                if let Some(list) = self.exact.get(&**name) {
                    all.extend(list.iter().filter(|e| live_for_selection(e)).cloned());
                }
                let exact = all.len();
                all.extend(
                    self.prefixes
                        .iter()
                        .filter(|e| name.starts_with(&*e.name) && live_for_selection(e))
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
                        before: None,
                        after: None,
                    },
                );
                return Some(Selected {
                    base: list,
                    before: None,
                    after: None,
                });
            }
        };
        let selected = Selected {
            base,
            before,
            after,
        };
        // An entry that is not live never is again, and a dispatch skips it
        // (`Entry::start`): with nothing registered since, the list is asked
        // whether any listener is left, which is one check when the first entry
        // is live; the appends are checked as they are called.
        if selected.is_empty() || (!selected.any_live() && selected.only_base()) {
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
