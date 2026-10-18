//! Publishes imported Cordis service objects as ordinary rutis services.
//!
//! Only public rutis API is used: the first object is registered with
//! `provide_mut_as`; a replacement goes through the returned `ServiceWriter`,
//! so earlier `Arc` snapshots keep their original remote object. An
//! unavailable slot withdraws the binding, which lets native dependency
//! gating stop and restart consumers. When the Node process goes away every
//! slot becomes unavailable.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use rutis::{BoxFuture, CordisError, Ctx, Disposer, ServiceWriter, TypeKey};

use crate::runtime::process::ServiceEvents;
use crate::runtime::{Error, Process};

/// Moves the native binding to `handle`. A withdrawal returns the future that
/// completes it: no new registration may start before it finishes.
type Apply = Box<
    dyn FnMut(&Ctx, &Arc<Process>, Option<String>) -> Result<Option<BoxFuture<'static, ()>>, Error>
        + Send,
>;

/// Declares the slot of a service a row announces (see
/// [`Projection::on_announced`]).
type Announce = Box<dyn FnMut(&Projection, &str, &serde_json::Value) -> bool + Send>;

/// Says who already provides a service a slot could not be published as,
/// for the error naming both sides.
type Occupied = Arc<dyn Fn(&Ctx) -> Option<String> + Send + Sync>;

struct Slot {
    /// Newest handle reported by the Cordis side.
    handle: Option<String>,
    /// Handle the native binding currently holds.
    applied: Option<String>,
    /// Handle whose proxy is being published outside the lock.
    publishing: Option<String>,
    /// A withdrawal is still completing.
    withdrawing: bool,
    apply: Option<Apply>,
}

#[derive(Default)]
struct State {
    target: Option<(Ctx, Arc<Process>)>,
    runtime: Option<tokio::runtime::Handle>,
    applying: bool,
    /// Closed while a publication was under way: the publisher applies the
    /// withdrawals reported until then, and closes when it is done.
    closing: bool,
    slots: HashMap<String, Slot>,
    /// Ends the task that watches the process's connection: dropped when
    /// the projection closes (or goes), so the task does not outlive it.
    /// Without it every row ever loaded left a task waiting for its
    /// runtime's session to end (#191).
    watching: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Declared before launching, attached once the mounting plugin has a
/// process, closed when that plugin is disposed.
pub struct Projection {
    state: Mutex<State>,
    /// Woken when a publisher finishes.
    idle: tokio::sync::Notify,
    me: Weak<Projection>,
    announce: Mutex<Option<Announce>>,
    /// The first slot refused because another provider has its key, and
    /// why (see [`Projection::take_refused`]).
    refused: Mutex<Option<String>>,
}

impl Projection {
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            state: Mutex::default(),
            idle: tokio::sync::Notify::new(),
            me: me.clone(),
            announce: Mutex::new(None),
            refused: Mutex::new(None),
        })
    }

    /// Follow the services a row announces without declaring them
    /// ([`ServiceEvents::announced`]): `declare` is given each one's name
    /// and methods, before each new object of it, and declares its slot
    /// the first time (with [`Projection::service_keyed`]). It returns
    /// whether the projection follows the service.
    pub fn on_announced(
        &self,
        declare: impl FnMut(&Projection, &str, &serde_json::Value) -> bool + Send + 'static,
    ) {
        *self.announce.lock().unwrap() = Some(Box::new(declare));
    }

    /// Whether a slot named `name` is declared.
    pub fn has_service(&self, name: &str) -> bool {
        self.state.lock().unwrap().slots.contains_key(name)
    }

    /// Why a slot could not be published since the last call, when another
    /// provider has its key: the error names both sides
    /// ([`Projection::service_keyed_occupied`]). A row loading checks it to
    /// fail as the later provider.
    pub fn take_refused(&self) -> Option<String> {
        self.refused.lock().unwrap().take()
    }

    /// Declare an exported slot. `make` builds the native proxy for one handle.
    pub fn service<T: Send + Sync + 'static>(
        &self,
        name: &str,
        make: fn(Arc<Process>, String) -> T,
    ) {
        self.service_keyed(name, TypeKey::of::<T>(), move |process, handle| {
            Arc::new(make(process, handle))
        });
    }

    /// Declare an exported slot published under `key`, for example a
    /// `dyn HostDispatch` under [`crate::runtime::host_key`].
    pub fn service_keyed<T: ?Sized + Send + Sync + 'static>(
        &self,
        name: &str,
        key: TypeKey,
        make: impl FnMut(Arc<Process>, String) -> Arc<T> + Send + 'static,
    ) {
        self.declare(name, key, make, None);
    }

    /// [`Projection::service_keyed`], saying who has the key when the
    /// slot cannot be published because another provider does:
    /// `occupied` describes it ("row prompts", say). The error then names
    /// both sides, `this` being the slot's own provider ("row storage"),
    /// and [`Projection::take_refused`] returns it.
    pub fn service_keyed_occupied<T: ?Sized + Send + Sync + 'static>(
        &self,
        name: &str,
        key: TypeKey,
        make: impl FnMut(Arc<Process>, String) -> Arc<T> + Send + 'static,
        this: String,
        occupied: impl Fn(&Ctx) -> Option<String> + Send + Sync + 'static,
    ) {
        self.declare(name, key, make, Some((this, Arc::new(occupied))));
    }

    fn declare<T: ?Sized + Send + Sync + 'static>(
        &self,
        name: &str,
        key: TypeKey,
        mut make: impl FnMut(Arc<Process>, String) -> Arc<T> + Send + 'static,
        occupied: Option<(String, Occupied)>,
    ) {
        let service = name.to_owned();
        let me = self.me.clone();
        let mut binding: Option<(Disposer, ServiceWriter<T>)> = None;
        // A proxy whose publication failed is kept for the retry: it holds its
        // handle, and dropping it would release that handle on the Cordis side.
        let mut candidate: Option<(String, Arc<T>)> = None;
        let apply: Apply = Box::new(move |ctx, process, handle| {
            let Some(handle) = handle else {
                candidate = None;
                if let Some((disposer, _)) = binding.take() {
                    return Ok(Some(Box::pin(async move {
                        let _ = disposer.dispose().await;
                    })));
                }
                return Ok(None);
            };
            let proxy = match candidate.take() {
                Some((kept, proxy)) if kept == handle => proxy,
                _ => make(process.clone(), handle.clone()),
            };
            let published = match binding.as_ref() {
                Some((_, writer)) => writer
                    .set(ctx, proxy.clone())
                    .map_err(|error| Error::Value(error.to_string())),
                None => match ctx.provide_mut_as(key.clone(), proxy.clone()) {
                    Ok(registered) => {
                        binding = Some(registered);
                        Ok(())
                    }
                    // Another provider has the key: the later one fails,
                    // and says who has it.
                    Err(CordisError::ServiceExists(_)) if occupied.is_some() => {
                        let (this, occupied) = occupied.as_ref().unwrap();
                        let holder = occupied(ctx).unwrap_or_else(|| "another plugin".to_owned());
                        let why = format!(
                            "service {service} is already provided by {holder}: {this} cannot provide it too"
                        );
                        if let Some(me) = me.upgrade() {
                            me.refused
                                .lock()
                                .unwrap()
                                .get_or_insert_with(|| why.clone());
                        }
                        Err(Error::Value(why))
                    }
                    Err(error) => Err(Error::Value(error.to_string())),
                },
            };
            if published.is_err() {
                candidate = Some((handle, proxy));
            }
            published.map(|()| None)
        });
        self.state.lock().unwrap().slots.insert(
            name.to_owned(),
            Slot {
                handle: None,
                applied: None,
                publishing: None,
                withdrawing: false,
                apply: Some(apply),
            },
        );
    }

    /// Publish the current handles from the mounting plugin's own context.
    pub fn attach(&self, ctx: &Ctx, process: Arc<Process>) -> Result<(), CordisError> {
        let (watching, closed) = tokio::sync::oneshot::channel::<()>();
        {
            let mut state = self.state.lock().unwrap();
            for (name, slot) in state.slots.iter_mut() {
                slot.handle = process.service(name);
            }
            state.target = Some((ctx.clone(), process.clone()));
            state.runtime = Some(tokio::runtime::Handle::current());
            state.watching = Some(watching);
        }
        // A Node process that goes away leaves every slot unavailable: the
        // services are withdrawn and native gating stops their consumers.
        // The watch ends with the projection.
        let connection = process.connection().clone();
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = connection.closed() => {}
                _ = closed => return,
            }
            if let Some(me) = me.upgrade() {
                me.disconnected();
            }
        });
        self.publish().map_err(Into::into)
    }

    fn disconnected(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if state.target.is_none() {
                return; // closed by disposal
            }
            for slot in state.slots.values_mut() {
                slot.handle = None;
            }
        }
        if let Err(error) = self.publish() {
            eprintln!("rutis: cannot withdraw Cordis services: {error}");
        }
    }

    /// Withdraw every published service now and wait until that is done:
    /// the kernel stops their consumers first. Then stop following changes,
    /// as [`Projection::close`]. A plugin that unloads its provider calls
    /// this first, so the provider outlives everything that uses it.
    ///
    /// A publication under way finishes first: until then its slot is not
    /// here to withdraw, and its service would stay.
    pub async fn withdraw(&self) {
        let (target, mut slots) = loop {
            let idle = self.idle.notified();
            let mut idle = std::pin::pin!(idle);
            idle.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if !state.applying {
                    state.watching = None;
                    break (state.target.take(), std::mem::take(&mut state.slots));
                }
            }
            idle.await;
        };
        let Some((ctx, process)) = target else {
            return;
        };
        let mut withdrawals = Vec::new();
        for slot in slots.values_mut() {
            if let Some(apply) = slot.apply.as_mut() {
                if let Ok(Some(withdrawal)) = apply(&ctx, &process, None) {
                    withdrawals.push(withdrawal);
                }
            }
        }
        for withdrawal in withdrawals {
            withdrawal.await;
        }
        drop(slots);
    }

    /// Stop following changes and drop the bindings' writers, which hold the
    /// process; the plugin's own effects withdraw the registrations.
    ///
    /// During a publication the publisher closes the projection once it is
    /// done, after applying the withdrawals reported before this call (an
    /// unloaded row reports its services' withdrawal and leaves them to the
    /// publication under way). It publishes nothing new meanwhile.
    pub fn close(&self) {
        let slots = {
            let mut state = self.state.lock().unwrap();
            if state.applying {
                state.closing = true;
                return;
            }
            state.target = None;
            state.watching = None;
            std::mem::take(&mut state.slots)
        };
        drop(slots);
    }

    /// Apply pending handles outside the lock: registering or replacing a
    /// service may run native code that calls back into this projection.
    /// A reentrant or concurrent change is left to the active publisher.
    fn publish(&self) -> Result<(), Error> {
        let mut failure = None;
        // Each (slot, handle) is attempted at most once per pass: a failure
        // keeps the slot pending for the next change, while a newer handle
        // that arrived meanwhile is still published in this pass.
        let mut attempted = HashSet::new();
        {
            let mut state = self.state.lock().unwrap();
            if state.applying {
                return Ok(());
            }
            state.applying = true;
        }
        loop {
            let mut closed = HashMap::new();
            let work = {
                let mut state = self.state.lock().unwrap();
                let closing = state.closing;
                let next = state.target.clone().and_then(|target| {
                    state
                        .slots
                        .iter_mut()
                        .find(|(name, slot)| {
                            slot.handle != slot.applied
                                && (!closing || slot.handle.is_none())
                                && !slot.withdrawing
                                && slot.apply.is_some()
                                && !attempted.contains(&(name.to_string(), slot.handle.clone()))
                        })
                        .map(|(name, slot)| {
                            slot.publishing = slot.handle.clone();
                            let work = (
                                name.clone(),
                                slot.handle.clone(),
                                slot.apply.take().unwrap(),
                            );
                            (target, work)
                        })
                });
                if next.is_none() {
                    state.applying = false;
                    if std::mem::take(&mut state.closing) {
                        state.target = None;
                        state.watching = None;
                        closed = std::mem::take(&mut state.slots);
                    }
                }
                next
            };
            let Some(((ctx, process), (name, handle, mut apply))) = work else {
                drop(closed);
                self.idle.notify_waiters();
                return failure.map_or(Ok(()), Err);
            };
            attempted.insert((name.clone(), handle.clone()));
            let result = apply(&ctx, &process, handle.clone());
            let mut state = self.state.lock().unwrap();
            let runtime = state.runtime.clone();
            let Some(slot) = state.slots.get_mut(&name) else {
                continue; // closed meanwhile
            };
            slot.apply = Some(apply);
            slot.publishing = None;
            match result {
                Ok(None) => slot.applied = handle,
                Ok(Some(withdrawal)) => {
                    slot.applied = None;
                    slot.withdrawing = true;
                    let me = self.me.clone();
                    let name = name.clone();
                    let finish = async move {
                        withdrawal.await;
                        if let Some(me) = me.upgrade() {
                            if let Some(slot) = me.state.lock().unwrap().slots.get_mut(&name) {
                                slot.withdrawing = false;
                            }
                            if let Err(error) = me.publish() {
                                eprintln!("rutis: cannot publish Cordis service {name}: {error}");
                            }
                        }
                    };
                    match runtime {
                        Some(runtime) => {
                            runtime.spawn(finish);
                        }
                        None => {
                            tokio::spawn(finish);
                        }
                    }
                }
                Err(error) => failure = failure.or(Some(error)),
            }
        }
    }
}

impl ServiceEvents for Projection {
    fn announced(&self, name: &str, methods: &serde_json::Value) -> bool {
        // Not under the state's lock: declaring a slot takes it.
        let mut announce = self.announce.lock().unwrap();
        match announce.as_mut() {
            Some(declare) => declare(self, name, methods),
            None => false,
        }
    }

    fn changed(&self, name: &str, handle: Option<String>, _version: u64) {
        let skipped = {
            let mut state = self.state.lock().unwrap();
            let process = state.target.as_ref().map(|(_, process)| process.clone());
            let Some(slot) = state.slots.get_mut(name) else {
                return;
            };
            let previous = std::mem::replace(&mut slot.handle, handle);
            // A handle replaced before it got a native proxy is released here;
            // applied or in-flight proxies release their handle when dropped.
            previous
                .filter(|previous| {
                    slot.applied.as_ref() != Some(previous)
                        && slot.publishing.as_ref() != Some(previous)
                })
                .zip(process)
        };
        if let Some((handle, process)) = skipped {
            process.release(&handle);
        }
        if let Err(error) = self.publish() {
            eprintln!("rutis: cannot publish Cordis service {name}: {error}");
        }
    }
}
