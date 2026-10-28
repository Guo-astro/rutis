mod common;

use std::future::{ready, IntoFuture, Ready};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::events::{Call, Listener};
use rutis::host::diagnostics::DependencyStatus;
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, Key, PluginFactory, TypeKey,
};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error, Event, Plugin};
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Semaphore};

use common::still_pending;

struct Capture(Arc<Mutex<Option<Ctx>>>);

struct PanicMetadata;
impl DynPlugin for PanicMetadata {
    fn name(&self) -> &str {
        panic!("closed fiber must not inspect plugin metadata")
    }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        unreachable!()
    }
}

impl Plugin for Capture {
    type Deps = ();

    fn name(&self) -> &str {
        "capture"
    }
    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        *self.0.lock().unwrap() = Some(ctx.clone());
        Ok(())
    }
}

async fn child(parent: &Ctx) -> (rutis::host::FiberView, Ctx) {
    let slot = Arc::new(Mutex::new(None));
    let view = parent.plugin(Capture(slot.clone()));
    (&view).await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    (view, ctx)
}

struct Read {
    key: Key<u64>,
    seen: Arc<Mutex<Vec<u64>>>,
}

struct ProbeRead(Key<u64>);
impl DynPlugin for ProbeRead {
    fn name(&self) -> &str {
        "probe-read"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            assert!(ctx.get_at(&self.0).is_none());
            Ok(Effect::Done)
        })
    }
}

impl DynPlugin for Read {
    fn name(&self) -> &str {
        "read"
    }
    fn injects(&self) -> Vec<TypeKey> {
        vec![self.key.erase()]
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let value = ctx.get_at(&self.key).unwrap();
            self.seen.lock().unwrap().push(*value);
            Ok(Effect::Done)
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Ping(u32);
impl Event for Ping {
    const NAME: &'static str = "instance-subtree-ping";
    type Value = ();
}

struct Record {
    log: Arc<Mutex<Vec<u32>>>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Option<Arc<Semaphore>>,
}

impl Record {
    /// The listener: records every `Ping`; for `1` it first tells
    /// `entered` and waits for `release`.
    fn listener(self) -> impl Listener<Ping> {
        let record = Arc::new(self);
        move |event: Arc<Ping>, _: Call<Ping>| {
            let record = record.clone();
            async move { record.call(event.0).await }
        }
    }

    async fn call(&self, value: u32) -> rutis::Result<Option<()>> {
        if value == 1 {
            if let Some(tx) = self.entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            if let Some(gate) = &self.release {
                let permit = gate.acquire().await.unwrap();
                permit.forget();
            }
        }
        self.log.lock().unwrap().push(value);
        Ok(None)
    }
}

fn bail(_: Arc<Ping>, _: Call<Ping>) -> Ready<rutis::Result<Option<()>>> {
    ready(Ok(Some(())))
}

#[derive(Serialize, Deserialize)]
struct Fault;
impl Event for Fault {
    const NAME: &'static str = "instance-subtree-fault";
    type Value = ();
}

fn fail_listener(_: Arc<Fault>, _: Call<Fault>) -> Ready<rutis::Result<Option<()>>> {
    ready(Err(ErrorKind::PluginFailed("listener".into()).into()))
}

/// Panics when called, before returning a future.
fn panic_call(_: Arc<Ping>, _: Call<Ping>) -> Ready<rutis::Result<Option<()>>> {
    panic!("listener call failed before returning a future")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emit_reports_synchronous_listener_panic() {
    let (tx, rx) = oneshot::channel();
    let sink = Arc::new(Mutex::new(Some(tx)));
    let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), {
        let sink = sink.clone();
        Arc::new(move |error| {
            if let Some(tx) = sink.lock().unwrap().take() {
                let _ = tx.send(error);
            }
        })
    });
    let (view, ctx) = child(&root).await;
    ctx.on::<Ping>(panic_call).unwrap();
    ctx.emit(Ping(1)).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap();
    assert!(format!("{error:?}").contains("listener call failed"));
    view.shutdown().await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serial_short_circuits_and_parallel_aggregates() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    let log = Arc::new(Mutex::new(Vec::new()));
    ctx.on::<Ping>(bail).unwrap();
    ctx.on::<Ping>(
        Record {
            log: log.clone(),
            entered: Mutex::new(None),
            release: None,
        }
        .listener(),
    )
    .unwrap();
    let value = ctx.serial(Ping(2)).await.unwrap();
    assert_eq!(value, Some(()));
    assert!(log.lock().unwrap().is_empty());

    ctx.on::<Fault>(fail_listener).unwrap();
    ctx.on::<Fault>(fail_listener).unwrap();
    let error = ctx.parallel(Fault).await.unwrap_err();
    assert!(matches!(&*error, ErrorKind::ListenersFailed { errors, .. } if errors.len() == 2));
    view.shutdown().await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instance_keys_enforce_subtree_and_explain_pending_dependency() {
    let root = Ctx::root().unwrap();
    let (_a, a) = child(&root).await;
    let (_b, b) = child(&root).await;
    let key_a = Key::<u64>::of().instance(a.instance());
    let key_b = Key::<u64>::of().instance(b.instance());
    a.provide_at(key_a.clone(), Arc::new(10u64)).unwrap();
    b.provide_at(key_b.clone(), Arc::new(20u64)).unwrap();

    assert_eq!(*a.get_at(&key_a).unwrap(), 10);
    assert_eq!(*b.get_at(&key_b).unwrap(), 20);
    assert!(b.get_at(&key_a).is_none());
    assert!(root.get_at(&key_a).is_none());
    assert!(matches!(
        root.provide_at(key_a.clone(), Arc::new(99u64))
            .map_err(Error::into_kind),
        Err(ErrorKind::InstanceOutOfScope { .. })
    ));
    let (_nested, nested) = child(&a).await;
    assert_eq!(*nested.get_at(&key_a).unwrap(), 10);

    let pending = b.plugin(Read {
        key: key_a.clone(),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    (&pending).await.unwrap();
    assert_eq!(pending.state().state, FiberState::Pending);
    let diag = root.diagnostics();
    let node = diag.plugins.iter().find(|p| p.id == pending.id).unwrap();
    assert_eq!(node.injects[0].status, DependencyStatus::OutOfScope);
    assert_ne!(node.instance, b.instance());
    assert_eq!(
        node.parent,
        root.diagnostics()
            .plugins
            .iter()
            .find(|p| p.instance == b.instance())
            .map(|p| p.id)
    );
    assert!(node.resolved_dependencies.is_empty());
    let probe = b.plugin(ProbeRead(key_a.clone()));
    (&probe).await.unwrap();
    let diag = root.diagnostics();
    let access = &diag
        .plugins
        .iter()
        .find(|p| p.id == probe.id)
        .unwrap()
        .accesses[0];
    assert!(access.out_of_scope);
    assert!(access.provider.is_none());
    assert!(access.generation.is_none());
    root.shutdown().await.unwrap();
    assert_eq!(a.instance(), key_a.erase().instance_id().unwrap());
    assert!(a.get_at(&key_a).is_none());
    let new_root = Ctx::root().unwrap();
    assert_ne!(new_root.instance(), root.instance());
    new_root.shutdown().await.unwrap();
}

/// Instance event channels are gone (#464): only the service registry and
/// listener registration are checked here. Dispatch no longer validates the
/// emitter's state, so the old emit / serial / parallel assertions are gone.
#[tokio::test]
async fn registration_errors_prioritize_closed_then_inactive_then_instance_scope() {
    let root = Ctx::root().unwrap();
    let (disposed_view, disposed) = child(&root).await;
    let (closed_view, closed) = child(&root).await;
    let foreign = Key::<u64>::of().instance(closed.instance());

    assert!(matches!(
        root.provide_at(foreign.clone(), Arc::new(1u64))
            .map_err(Error::into_kind),
        Err(ErrorKind::InstanceOutOfScope { .. })
    ));

    disposed_view.dispose().await.unwrap();
    assert!(matches!(
        disposed
            .effect(|| async { Ok(()) })
            .map_err(Error::into_kind),
        Err(ErrorKind::InactiveEffect)
    ));
    assert!(matches!(
        disposed
            .provide_at(foreign.clone(), Arc::new(1u64))
            .map_err(Error::into_kind),
        Err(ErrorKind::InactiveEffect)
    ));
    assert!(matches!(
        disposed.on::<Ping>(bail).map_err(Error::into_kind),
        Err(ErrorKind::InactiveEffect)
    ));

    closed_view.shutdown().await.unwrap();
    assert!(matches!(
        closed.effect(|| async { Ok(()) }).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    assert!(matches!(
        closed
            .provide_at(foreign.clone(), Arc::new(1u64))
            .map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    assert!(matches!(
        closed.on::<Ping>(bail).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));

    drop(closed_view);
    assert!(matches!(
        closed.effect(|| async { Ok(()) }).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    root.shutdown().await.unwrap();
    assert!(matches!(
        root.effect(|| async { Ok(()) }).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    assert!(matches!(
        root.provide_at(foreign.clone(), Arc::new(1u64))
            .map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    assert!(matches!(
        root.on::<Ping>(bail).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_service_reload_reaches_both_instances() {
    let root = Ctx::root().unwrap();
    let old = root.provide(7u64).unwrap();
    let (_a, a) = child(&root).await;
    let (_b, b) = child(&root).await;
    let seen_a = Arc::new(Mutex::new(Vec::new()));
    let seen_b = Arc::new(Mutex::new(Vec::new()));
    let read_a = a.plugin(Read {
        key: Key::of(),
        seen: seen_a.clone(),
    });
    let read_b = b.plugin(Read {
        key: Key::of(),
        seen: seen_b.clone(),
    });
    (&read_a).await.unwrap();
    (&read_b).await.unwrap();
    old.dispose().await.unwrap();
    root.provide(9u64).unwrap();
    (&read_a).await.unwrap();
    (&read_b).await.unwrap();
    assert_eq!(*seen_a.lock().unwrap(), vec![7, 9]);
    assert_eq!(*seen_b.lock().unwrap(), vec![7, 9]);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn instance_service_reload_does_not_touch_sibling_consumer() {
    let root = Ctx::root().unwrap();
    let (_a_view, a) = child(&root).await;
    let (_b_view, b) = child(&root).await;
    let key_a = Key::<u64>::of().instance(a.instance());
    let key_b = Key::<u64>::of().instance(b.instance());
    let old_a = a.provide_at(key_a.clone(), Arc::new(1u64)).unwrap();
    b.provide_at(key_b.clone(), Arc::new(2u64)).unwrap();
    let seen_a = Arc::new(Mutex::new(Vec::new()));
    let seen_b = Arc::new(Mutex::new(Vec::new()));
    let read_a = a.plugin(Read {
        key: key_a.clone(),
        seen: seen_a.clone(),
    });
    let read_b = b.plugin(Read {
        key: key_b,
        seen: seen_b.clone(),
    });
    (&read_a).await.unwrap();
    (&read_b).await.unwrap();
    let b_generation = read_b.state().generation;

    old_a.dispose().await.unwrap();
    assert_eq!(read_a.state().state, FiberState::Pending);
    a.provide_at(key_a, Arc::new(3u64)).unwrap();
    (&read_a).await.unwrap();
    assert_eq!(*seen_a.lock().unwrap(), vec![1, 3]);
    assert_eq!(*seen_b.lock().unwrap(), vec![2]);
    assert_eq!(read_b.state().generation, b_generation);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn fiber_instance_stays_stable_across_restart_update_and_dependency_reload() {
    let root = Ctx::root().unwrap();
    let (static_view, static_ctx) = child(&root).await;
    let static_id = static_ctx.instance();
    static_view.restart().await.unwrap();
    assert_eq!(
        root.diagnostics()
            .plugins
            .iter()
            .find(|plugin| plugin.id == static_view.id)
            .unwrap()
            .instance,
        static_id
    );

    let slot = Arc::new(Mutex::new(None));
    struct CaptureFactory(Arc<Mutex<Option<Ctx>>>);
    impl PluginFactory<u32> for CaptureFactory {
        fn build(&self, _config: &u32) -> Result<Box<dyn DynPlugin>, Error> {
            Ok(Box::new(Capture(self.0.clone())))
        }
    }
    let factory = root.plugin_with(CaptureFactory(slot.clone()), 1u32);
    (&factory).await.unwrap();
    let factory_id = slot.lock().unwrap().as_ref().unwrap().instance();
    factory.update(2u32).await.unwrap();
    assert_eq!(
        root.diagnostics()
            .plugins
            .iter()
            .find(|plugin| plugin.id == factory.id)
            .unwrap()
            .instance,
        factory_id
    );

    let old = root.provide(4u64).unwrap();
    let reader = root.plugin(Read {
        key: Key::of(),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    (&reader).await.unwrap();
    let reader_id = root
        .diagnostics()
        .plugins
        .iter()
        .find(|plugin| plugin.id == reader.id)
        .unwrap()
        .instance;
    old.dispose().await.unwrap();
    root.provide(5u64).unwrap();
    (&reader).await.unwrap();
    assert_eq!(
        root.diagnostics()
            .plugins
            .iter()
            .find(|plugin| plugin.id == reader.id)
            .unwrap()
            .instance,
        reader_id
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_borrowed_serial_releases_shutdown_flight() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let gate = Arc::new(Semaphore::new(0));
    ctx.on::<Ping>(
        Record {
            log: Arc::new(Mutex::new(Vec::new())),
            entered: Mutex::new(Some(entered_tx)),
            release: Some(gate),
        }
        .listener(),
    )
    .unwrap();
    let mut dispatch = ctx.serial(Ping(1));
    tokio::select! {
        result = &mut dispatch => panic!("serial completed before its gate: {result:?}"),
        result = entered_rx => result.unwrap(),
    }
    drop(dispatch);
    tokio::time::timeout(Duration::from_secs(10), view.shutdown())
        .await
        .unwrap()
        .unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn callback_can_start_own_shutdown_and_return() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    let calls = Arc::new(AtomicUsize::new(0));
    // Starts its own fiber's shutdown and returns.
    let (own, count) = (view.clone(), calls.clone());
    ctx.on::<Ping>(move |_, _| {
        let (own, count) = (own.clone(), count.clone());
        async move {
            drop(own.shutdown());
            count.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    })
    .unwrap();
    ctx.serial(Ping(1)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), view.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    root.shutdown().await.unwrap();
}

struct CountInjects {
    key: TypeKey,
    calls: Arc<AtomicUsize>,
}

impl DynPlugin for CountInjects {
    fn name(&self) -> &str {
        "count-injects"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        vec![self.key.clone()]
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dependency_gate_reuses_registration_snapshot() {
    let root = Ctx::root().unwrap();
    root.provide(1u64).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(CountInjects {
        key: TypeKey::of::<u64>(),
        calls: calls.clone(),
    });
    (&view).await.unwrap();
    // A notification for the key that changes nothing here: the same key
    // provided in another isolation scope.
    root.isolate(TypeKey::of::<u64>(), "elsewhere")
        .provide(2u64)
        .unwrap();
    (&view).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    view.shutdown().await.unwrap();
    root.shutdown().await.unwrap();
}

/// Events are one flat bus, so both sibling scopes' listeners hear every
/// emit; shutting one scope down waits for its own listener's running call
/// and leaves the other's alone.
///
/// Changed with #464: a slow call no longer holds up the next emit of the
/// name, so `3` and `2` are recorded while `1` still waits for its gate; and
/// an emit from a closing context is no longer rejected (not asserted).
#[tokio::test(start_paused = true)]
async fn sibling_scopes_share_the_bus_and_drain_by_shutdown() {
    let root = Ctx::root().unwrap();
    let (view_a, a) = child(&root).await;
    let (_view_b, b) = child(&root).await;
    a.mark_scope("tenant").unwrap();
    b.mark_scope("tenant").unwrap();
    let log_a = Arc::new(Mutex::new(Vec::new()));
    let log_b = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Semaphore::new(0));
    let (entered_tx, entered_rx) = oneshot::channel();
    a.on::<Ping>(
        Record {
            log: log_a.clone(),
            entered: Mutex::new(Some(entered_tx)),
            release: Some(release.clone()),
        }
        .listener(),
    )
    .unwrap();
    b.on::<Ping>(
        Record {
            log: log_b.clone(),
            entered: Mutex::new(None),
            release: None,
        }
        .listener(),
    )
    .unwrap();
    a.emit(Ping(1)).unwrap();
    entered_rx.await.unwrap();
    a.emit(Ping(3)).unwrap();
    b.emit(Ping(2)).unwrap();
    // Polls on the paused clock, so the hang guard still fires.
    tokio::time::timeout(Duration::from_secs(10), async {
        while log_b.lock().unwrap().is_empty() || log_a.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*log_a.lock().unwrap(), vec![3, 2]);

    let pending = view_a.shutdown();
    assert!(matches!(
        a.provide_at(Key::<u32>::of().instance(a.instance()), Arc::new(1u32))
            .map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    assert!(matches!(
        a.on::<Ping>(bail).map_err(Error::into_kind),
        Err(ErrorKind::Closed)
    ));
    let closed_child = a.plugin(PanicMetadata);
    assert_eq!(closed_child.state().state, FiberState::Disposed);
    let mut waiter = tokio::spawn(pending);
    assert!(
        still_pending(&mut waiter).await,
        "shutdown waits for the accepted callback"
    );
    release.add_permits(1);
    waiter.await.unwrap().unwrap();
    assert_eq!(*log_a.lock().unwrap(), vec![3, 2, 1]);
    assert_eq!(*log_b.lock().unwrap(), vec![1, 3, 2]);
    assert!(view_a.restart().await.is_err());
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_listener_is_excluded_from_new_ancestor_dispatches() {
    let root = Ctx::root().unwrap();
    let (_parent, parent) = child(&root).await;
    let (child_view, listener_ctx) = child(&parent).await;
    let log = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Semaphore::new(0));
    let (entered_tx, entered_rx) = oneshot::channel();
    listener_ctx
        .on::<Ping>(
            Record {
                log: log.clone(),
                entered: Mutex::new(Some(entered_tx)),
                release: Some(release.clone()),
            }
            .listener(),
        )
        .unwrap();
    parent.emit(Ping(1)).unwrap();
    entered_rx.await.unwrap();
    let closing = child_view.shutdown();
    parent.emit(Ping(2)).unwrap();
    release.add_permits(1);
    closing.await.unwrap();
    assert_eq!(*log.lock().unwrap(), vec![1]);
    root.shutdown().await.unwrap();
}

/// Changed with #464: dropping a `parallel` future cancels the calls it
/// started (was: the calls ran on and shutdown waited for them), so the
/// shutdown no longer waits for the gate and the call never records.
#[tokio::test(start_paused = true)]
async fn dropped_parallel_waiter_cancels_its_calls() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    let release = Arc::new(Semaphore::new(0));
    let (entered_tx, entered_rx) = oneshot::channel();
    let log = Arc::new(Mutex::new(Vec::new()));
    ctx.on::<Ping>(
        Record {
            log: log.clone(),
            entered: Mutex::new(Some(entered_tx)),
            release: Some(release.clone()),
        }
        .listener(),
    )
    .unwrap();
    let dispatch_ctx = ctx.clone();
    let waiting = tokio::spawn(async move { dispatch_ctx.parallel(Ping(1)).await });
    entered_rx.await.unwrap();
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), view.shutdown())
        .await
        .expect("shutdown does not wait for a cancelled call")
        .unwrap();
    release.add_permits(1);
    assert!(log.lock().unwrap().is_empty());
    root.shutdown().await.unwrap();
}

/// Changed with #464: disposing one listener withdraws it at once (no new
/// call starts) and no longer waits for its running call; the fiber's
/// unload is what waits for running calls.
#[tokio::test(start_paused = true)]
async fn manual_listener_disposal_stops_new_calls() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    let release = Arc::new(Semaphore::new(0));
    let (entered_tx, entered_rx) = oneshot::channel();
    let log = Arc::new(Mutex::new(Vec::new()));
    let listener = ctx
        .on::<Ping>(
            Record {
                log: log.clone(),
                entered: Mutex::new(Some(entered_tx)),
                release: Some(release.clone()),
            }
            .listener(),
        )
        .unwrap();
    ctx.emit(Ping(1)).unwrap();
    entered_rx.await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), listener.dispose())
        .await
        .expect("disposal does not wait for the running call")
        .unwrap();
    ctx.emit(Ping(2)).unwrap();
    assert!(log.lock().unwrap().is_empty());
    let mut closing = tokio::spawn(view.shutdown());
    assert!(
        still_pending(&mut closing).await,
        "the fiber's unload waits for the running call"
    );
    release.add_permits(1);
    closing.await.unwrap().unwrap();
    assert_eq!(*log.lock().unwrap(), vec![1]);
    root.shutdown().await.unwrap();
}

struct FailCleanup;
impl DynPlugin for FailCleanup {
    fn name(&self) -> &str {
        "fail-cleanup"
    }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async {
            Ok(Effect::Disposer(Box::new(|| {
                Err(ErrorKind::PluginFailed("cleanup".into()).into())
            })))
        })
    }
}

struct FailApply;
impl DynPlugin for FailApply {
    fn name(&self) -> &str {
        "fail-apply"
    }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Err(ErrorKind::PluginFailed("apply".into()).into()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutting_down_failed_fiber_keeps_its_original_error() {
    let root = Ctx::root().unwrap();
    let view = root.plugin(FailApply);
    let original = (&view).await.unwrap_err();
    let closed = view.shutdown().await.unwrap_err();
    assert!(Arc::ptr_eq(&original, &closed));
    assert_eq!(view.state().state, FiberState::Disposed);
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtree_shutdown_aggregates_child_error_and_shares_result() {
    let root = Ctx::root().unwrap();
    let (parent, parent_ctx) = child(&root).await;
    let bad = parent_ctx.plugin(FailCleanup);
    (&bad).await.unwrap();
    let (one, two) = tokio::join!(parent.shutdown(), parent.shutdown());
    let one = one.unwrap_err();
    let two = two.unwrap_err();
    assert!(Arc::ptr_eq(&one, &two));
    assert_eq!(bad.state().state, FiberState::Disposed);
    assert!(parent.restart().await.is_err());
    root.shutdown().await.unwrap();
}

struct BlockCleanup {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Arc<Semaphore>,
}

impl DynPlugin for BlockCleanup {
    fn name(&self) -> &str {
        "block-cleanup"
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        let entered = self.entered.lock().unwrap().take().unwrap();
        let release = self.release.clone();
        Box::pin(async move {
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    let _ = entered.send(());
                    let permit = release.acquire().await.unwrap();
                    permit.forget();
                    Ok(())
                })
            })))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_queued_after_clean_shutdown_stays_successful() {
    let root = Ctx::root().unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let release = Arc::new(Semaphore::new(0));
    let view = root.plugin(BlockCleanup {
        entered: Mutex::new(Some(entered_tx)),
        release: release.clone(),
    });
    (&view).await.unwrap();
    let shutdown = view.shutdown();
    entered_rx.await.unwrap();
    let settle = (&view).into_future();
    release.add_permits(1);
    assert!(settle.await.is_ok());
    shutdown.await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_parent_and_child_shutdown_join_without_cycle() {
    let root = Ctx::root().unwrap();
    let (parent, parent_ctx) = child(&root).await;
    let (nested, _nested_ctx) = child(&parent_ctx).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let p = parent.clone();
    let pbarrier = barrier.clone();
    let parent_waiter = tokio::spawn(async move {
        pbarrier.wait().await;
        p.shutdown().await
    });
    let c = nested.clone();
    let cbarrier = barrier.clone();
    let child_waiter = tokio::spawn(async move {
        cbarrier.wait().await;
        c.shutdown().await
    });
    barrier.wait().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        parent_waiter.await.unwrap().unwrap();
        child_waiter.await.unwrap().unwrap();
    })
    .await
    .unwrap();
    assert_eq!(root.diagnostics().plugins.len(), 1);
    root.shutdown().await.unwrap();
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl DynPlugin for DropProbe {
    fn name(&self) -> &str {
        "drop-probe"
    }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousand_subtrees_leave_no_live_fibers() {
    let root = Ctx::root().unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    for n in 1..=1000 {
        let child = root.plugin(DropProbe(dropped.clone()));
        (&child).await.unwrap();
        if n == 1 {
            drop(child.shutdown());
        }
        child.shutdown().await.unwrap();
        drop(child);
        assert_eq!(dropped.load(Ordering::SeqCst), n);
    }
    assert_eq!(root.diagnostics().plugins.len(), 1);
    root.shutdown().await.unwrap();
}

struct SlowApply {
    started: Mutex<Option<oneshot::Sender<()>>>,
    cleaned: Arc<AtomicUsize>,
}

impl DynPlugin for SlowApply {
    fn name(&self) -> &str {
        "slow-apply"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            if let Some(tx) = self.started.lock().unwrap().take() {
                let _ = tx.send(());
            }
            ctx.cancelled().await;
            let cleaned = self.cleaned.clone();
            ctx.effect(move || async move {
                cleaned.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_keeps_cleanup_from_loading_apply() {
    let root = Ctx::root().unwrap();
    let (tx, rx) = oneshot::channel();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let view = root.plugin(SlowApply {
        started: Mutex::new(Some(tx)),
        cleaned: cleaned.clone(),
    });
    rx.await.unwrap();
    view.shutdown().await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(view.state().state, FiberState::Disposed);
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispose_child_shutdown_and_parent_shutdown_converge() {
    for _ in 0..32 {
        let root = Ctx::root().unwrap();
        let (parent, ctx) = child(&root).await;
        let leaf = ctx.plugin(Capture(Arc::new(Mutex::new(None))));
        (&leaf).await.unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(4));
        let dispose = tokio::spawn({
            let leaf = leaf.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                leaf.dispose().await
            }
        });
        let close_leaf = tokio::spawn({
            let leaf = leaf.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                leaf.shutdown().await
            }
        });
        let close_parent = tokio::spawn({
            let parent = parent.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                parent.shutdown().await
            }
        });
        barrier.wait().await;
        let (disposed, leaf_closed, parent_closed) =
            tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(dispose, close_leaf, close_parent)
            })
            .await
            .expect("concurrent shutdown deadlocked");
        disposed.unwrap().unwrap();
        leaf_closed.unwrap().unwrap();
        parent_closed.unwrap().unwrap();
        assert_eq!(leaf.state().state, FiberState::Disposed);
        assert_eq!(root.diagnostics().plugins.len(), 1);
        root.shutdown().await.unwrap();
    }
}

struct Supply(Arc<Mutex<Vec<&'static str>>>);
impl DynPlugin for Supply {
    fn name(&self) -> &str {
        "supply"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let log = self.0.clone();
            ctx.effect(move || async move {
                log.lock().unwrap().push("provider");
                Ok(())
            })?;
            ctx.provide(1u32)?;
            Ok(Effect::Done)
        })
    }
}

struct Use(Arc<Mutex<Vec<&'static str>>>);
impl DynPlugin for Use {
    fn name(&self) -> &str {
        "use"
    }
    fn injects(&self) -> Vec<TypeKey> {
        vec![TypeKey::of::<u32>()]
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            assert_eq!(*ctx.get::<u32>().unwrap(), 1);
            let log = self.0.clone();
            Ok(Effect::Disposer(Box::new(move || {
                log.lock().unwrap().push("consumer");
                Ok(())
            })))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_subtree_disposes_consumer_before_provider() {
    let root = Ctx::root().unwrap();
    let (parent, ctx) = child(&root).await;
    let log = Arc::new(Mutex::new(Vec::new()));
    let consumer = ctx.plugin(Use(log.clone()));
    (&consumer).await.unwrap();
    assert_eq!(consumer.state().state, FiberState::Pending);
    let provider = ctx.plugin(Supply(log.clone()));
    (&provider).await.unwrap();
    (&consumer).await.unwrap();
    parent.shutdown().await.unwrap();
    assert_eq!(consumer.state().state, FiberState::Disposed);
    assert_eq!(provider.state().state, FiberState::Disposed);
    assert_eq!(*log.lock().unwrap(), vec!["consumer", "provider"]);
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consumer_outside_closed_subtree_can_reload_from_new_provider() {
    let root = Ctx::root().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let external = root.plugin(Use(log.clone()));
    (&external).await.unwrap();
    assert_eq!(external.state().state, FiberState::Pending);
    let (parent, ctx) = child(&root).await;
    let provider = ctx.plugin(Supply(log));
    (&provider).await.unwrap();
    (&external).await.unwrap();
    assert_eq!(external.state().state, FiberState::Active);
    parent.shutdown().await.unwrap();
    (&external).await.unwrap();
    assert_eq!(external.state().state, FiberState::Pending);
    root.provide(1u32).unwrap();
    (&external).await.unwrap();
    assert_eq!(external.state().state, FiberState::Active);
    root.shutdown().await.unwrap();
}

/// A descendant's listener is running for an event emitted by `ancestor` and
/// only returns once its owner is cancelled. Returns how often it returned.
async fn start_owner_waiting_callback(ancestor: &Ctx, owner: &Ctx) -> Arc<AtomicUsize> {
    let entered = Arc::new(Semaphore::new(0));
    let returned = Arc::new(AtomicUsize::new(0));
    // Waits for its owner's cancellation, then records that it returned.
    let listener_owner = owner.clone();
    let (listener_entered, listener_returned) = (entered.clone(), returned.clone());
    owner
        .on::<Ping>(move |_, _| {
            let owner = listener_owner.clone();
            let (entered, returned) = (listener_entered.clone(), listener_returned.clone());
            async move {
                entered.add_permits(1);
                owner.cancelled().await;
                returned.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        })
        .unwrap();
    ancestor.emit(Ping(1)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), entered.acquire())
        .await
        .expect("callback entered")
        .unwrap()
        .forget();
    returned
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_shutdown_cancels_descendants_before_draining_listener_calls() {
    let root = Ctx::root().unwrap();
    let (_owner_view, owner) = child(&root).await;
    let returned = start_owner_waiting_callback(&root, &owner).await;

    // A waiter dropped by its timeout does not stop the shutdown.
    let _ = tokio::time::timeout(Duration::from_millis(1), root.shutdown()).await;
    tokio::time::timeout(Duration::from_secs(10), root.shutdown())
        .await
        .expect("root shutdown finishes without a separate child shutdown")
        .unwrap();
    assert!(owner.cancellation_token().is_cancelled());
    // Completion waited for the accepted callback.
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    // A repeated shutdown joins the same result.
    root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subtree_shutdown_cancels_descendants_before_draining_listener_calls() {
    let root = Ctx::root().unwrap();
    let (ancestor_view, ancestor) = child(&root).await;
    let (_owner_view, owner) = child(&ancestor).await;
    let returned = start_owner_waiting_callback(&ancestor, &owner).await;

    tokio::time::timeout(Duration::from_secs(10), ancestor_view.shutdown())
        .await
        .expect("subtree shutdown finishes")
        .unwrap();
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    root.shutdown().await.unwrap();
}

/// A child mounted again is a new fiber with a new ID; the old ID stays
/// retired and does not reach the new child's instance services.
///
/// guarantee: instance/subtrees.2.2
#[tokio::test]
async fn a_recreated_child_gets_a_new_instance_id() {
    let root = Ctx::root().unwrap();
    let (old_view, old) = child(&root).await;
    let old_id = old.instance();
    assert_eq!(old_view.instance(), old_id);
    old_view.shutdown().await.unwrap();
    drop(old_view);

    let mut seen = vec![old_id];
    for _ in 0..3 {
        let (view, ctx) = child(&root).await;
        let id = ctx.instance();
        assert_eq!(view.instance(), id);
        assert!(!seen.contains(&id), "{id} handed out again ({seen:?})");
        ctx.provide_at(Key::<u64>::of().instance(id), Arc::new(7u64))
            .unwrap();
        assert!(ctx.get_at(&Key::<u64>::of().instance(old_id)).is_none());
        seen.push(id);
        view.dispose().await.unwrap();
    }
    // The retired ID is still reported by contexts kept from its fiber.
    assert_eq!(old.instance(), old_id);
    root.shutdown().await.unwrap();
}

/// Lives through the churn next to it: provides `ResidentService`, listens
/// to `Ping`, counts its loads.
struct Resident {
    loads: Arc<AtomicUsize>,
    pings: Arc<AtomicUsize>,
}

struct ResidentService;

/// Counts the `Ping`s it hears in `pings`.
fn count_pings(pings: Arc<AtomicUsize>) -> impl Listener<Ping> {
    move |_: Arc<Ping>, _: Call<Ping>| {
        pings.fetch_add(1, Ordering::SeqCst);
        ready(Ok(None))
    }
}

impl DynPlugin for Resident {
    fn name(&self) -> &str {
        "resident"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            self.loads.fetch_add(1, Ordering::SeqCst);
            ctx.provide(ResidentService)?;
            ctx.on::<Ping>(count_pings(self.pings.clone()))?;
            Ok(Effect::Done)
        })
    }
}

/// A short-lived subtree: needs the resident's service, provides an
/// instance service, is a scope, listens to and emits an event under a name
/// of its own (so the resident does not hear it), and mounts a child of its
/// own.
struct Transient {
    dropped: Arc<AtomicUsize>,
    pings: Arc<AtomicUsize>,
    injects: Vec<TypeKey>,
}

impl Drop for Transient {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl DynPlugin for Transient {
    fn name(&self) -> &str {
        "transient"
    }
    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.require::<ResidentService>()?;
            let id = ctx.instance();
            ctx.provide_at(Key::<u64>::of().instance(id), Arc::new(1u64))?;
            ctx.mark_scope("transient")?;
            let name = format!("instance-subtree-ping/{id}");
            ctx.listen::<Ping>()
                .name(name.clone())
                .on(count_pings(self.pings.clone()))?;
            ctx.named(name).emit(Ping(0))?;
            ctx.plugin(DropProbe(self.dropped.clone()));
            Ok(Effect::Done)
        })
    }
}

/// 1000 subtrees created and shut down through the public API on a
/// long-lived root, next to a resident sibling: afterwards every subtree
/// plugin has been dropped, the runtime has as many live tasks as before
/// the churn, and the resident has neither reloaded nor lost its listener.
///
/// guarantee: instance/subtrees.5
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousand_subtrees_next_to_a_resident_leave_no_tasks_behind() {
    const ROUNDS: usize = 1000;
    let root = Ctx::root().unwrap();
    let metrics = tokio::runtime::Handle::current().metrics();
    let loads = Arc::new(AtomicUsize::new(0));
    let resident_pings = Arc::new(AtomicUsize::new(0));
    let resident = root.plugin(Resident {
        loads: loads.clone(),
        pings: resident_pings.clone(),
    });
    (&resident).await.unwrap();
    let generation = resident.state().generation;
    let baseline = metrics.num_alive_tasks();

    let dropped = Arc::new(AtomicUsize::new(0));
    let instance_pings = Arc::new(AtomicUsize::new(0));
    for _ in 0..ROUNDS {
        let view = root.plugin(Transient {
            dropped: dropped.clone(),
            pings: instance_pings.clone(),
            injects: vec![TypeKey::of::<ResidentService>()],
        });
        (&view).await.unwrap();
        assert_eq!(view.state().state, FiberState::Active);
        view.shutdown().await.unwrap();
    }

    // The transient plugin and its child, per round.
    rutis_test_support::until(
        || dropped.load(Ordering::SeqCst) == 2 * ROUNDS,
        "every subtree plugin dropped",
    )
    .await;
    rutis_test_support::eventually_with(
        || (metrics.num_alive_tasks() == baseline).then_some(()),
        "live tasks back to the baseline",
        || format!("{} live, baseline {baseline}", metrics.num_alive_tasks()),
    )
    .await;
    assert_eq!(root.diagnostics().plugins.len(), 2, "root and resident");

    assert_eq!(loads.load(Ordering::SeqCst), 1, "the resident reloaded");
    let state = resident.state();
    assert_eq!(
        (state.state, state.generation),
        (FiberState::Active, generation)
    );
    root.parallel(Ping(1)).await.unwrap();
    assert_eq!(resident_pings.load(Ordering::SeqCst), 1);
    root.shutdown().await.unwrap();
}
