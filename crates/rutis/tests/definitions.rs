//! Plugin definitions made in Rust (design-plugin-values R3–R7): copies
//! mount with their own config, end before their origin's cleanups, and
//! mount only where the origin's instance allows.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, Key};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Definition, Error, Plugin};

type Log = Arc<Mutex<Vec<String>>>;

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

async fn soon<F: std::future::IntoFuture>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("timed out")
}

async fn reach(view: &FiberView, state: FiberState) {
    let mut rx = view.watch();
    soon(rx.wait_for(|snapshot| snapshot.state == state))
        .await
        .expect("fiber dropped");
}

/// A plugin that hands its context out, to mount things on.
struct Capture(Arc<Mutex<Option<Ctx>>>);

impl DynPlugin for Capture {
    fn name(&self) -> &str {
        "capture"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ctx.clone());
            Ok(Effect::Done)
        })
    }
}

async fn child(parent: &Ctx) -> (FiberView, Ctx) {
    let slot = Arc::new(Mutex::new(None));
    let view = parent.plugin(Capture(slot.clone()));
    soon(&view).await.unwrap();
    let ctx = slot.lock().unwrap().take().unwrap();
    (view, ctx)
}

/// One copy: logs its config when it applies and when it is cleaned up.
struct Part {
    depth: u32,
    log: Log,
}

impl Plugin for Part {
    type Deps = ();

    fn name(&self) -> &str {
        "part"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        self.log
            .lock()
            .unwrap()
            .push(format!("part {} applied", self.depth));
        let (log, depth) = (self.log.clone(), self.depth);
        ctx.effect(move || async move {
            log.lock().unwrap().push(format!("part {depth} cleaned up"));
            Ok(())
        })?;
        Ok(())
    }
}

/// The origin: makes a definition of `Part` in apply and hands it out,
/// after registering a cleanup of its own.
struct Origin {
    log: Log,
    out: Arc<Mutex<Option<Definition>>>,
}

impl DynPlugin for Origin {
    fn name(&self) -> &str {
        "origin"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let log = self.log.clone();
            ctx.effect(move || async move {
                log.lock().unwrap().push("origin cleaned up".into());
                Ok(())
            })?;
            let log = self.log.clone();
            let definition = Definition::new(ctx, move |_at: &Ctx, depth: u32| {
                Ok(Part {
                    depth,
                    log: log.clone(),
                })
            });
            *self.out.lock().unwrap() = Some(definition);
            Ok(Effect::Done)
        })
    }
}

async fn origin(parent: &Ctx, log: &Log) -> (FiberView, Definition) {
    let out = Arc::new(Mutex::new(None));
    let view = parent.plugin(Origin {
        log: log.clone(),
        out: out.clone(),
    });
    soon(&view).await.unwrap();
    let definition = out.lock().unwrap().take().unwrap();
    (view, definition)
}

#[tokio::test]
async fn each_copy_mounts_with_its_own_config() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (_origin, definition) = origin(&root, &log).await;
    let (_host, host) = child(&root).await;
    let one = host.plugin(definition.at(&host, 1u32).unwrap());
    let two = host.plugin(definition.at(&host, 2u32).unwrap());
    soon(&one).await.unwrap();
    soon(&two).await.unwrap();
    assert_eq!(entries(&log), ["part 1 applied", "part 2 applied"]);
    assert_eq!(definition.name(), "Part");
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_origin_s_copies_end_before_its_own_cleanups() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (origin, definition) = origin(&root, &log).await;
    let (_host, host) = child(&root).await;
    let one = host.plugin(definition.at(&host, 1u32).unwrap());
    let two = host.plugin(definition.at(&host, 2u32).unwrap());
    soon(&one).await.unwrap();
    soon(&two).await.unwrap();
    assert!(!definition.is_ended());

    soon(origin.dispose()).await.unwrap();
    let after: Vec<String> = entries(&log).into_iter().skip(2).collect();
    assert_eq!(after.len(), 3, "{after:?}");
    assert!(
        after[..2].contains(&"part 1 cleaned up".to_owned()),
        "{after:?}"
    );
    assert!(
        after[..2].contains(&"part 2 cleaned up".to_owned()),
        "{after:?}"
    );
    assert_eq!(after[2], "origin cleaned up");
    assert_eq!(one.state().state, FiberState::Disposed);
    assert_eq!(two.state().state, FiberState::Disposed);
    soon(definition.ended()).await;
    assert!(definition.is_ended());
    assert!(definition.at(&host, 3u32).is_err());
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_restart_ends_the_copies_and_a_new_generation_makes_a_new_definition() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let out = Arc::new(Mutex::new(None));
    let origin = root.plugin(Origin {
        log: log.clone(),
        out: out.clone(),
    });
    soon(&origin).await.unwrap();
    let first = out.lock().unwrap().take().unwrap();
    let (_host, host) = child(&root).await;
    let copy = host.plugin(first.at(&host, 1u32).unwrap());
    soon(&copy).await.unwrap();

    soon(origin.restart()).await.unwrap();
    assert_eq!(copy.state().state, FiberState::Disposed);
    assert!(first.is_ended());
    let second = out.lock().unwrap().take().unwrap();
    assert_ne!(first, second);
    let again = host.plugin(second.at(&host, 2u32).unwrap());
    soon(&again).await.unwrap();
    assert!(entries(&log).contains(&"part 2 applied".to_owned()));
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_copy_waiting_for_its_dependencies_ends_with_the_origin() {
    struct Waits;
    impl Plugin for Waits {
        type Deps = Arc<u64>;
        async fn apply(&self, _: &Ctx, _: Arc<u64>) -> rutis::Result {
            Ok(())
        }
    }
    struct WaitingOrigin(Arc<Mutex<Option<Definition>>>);
    impl DynPlugin for WaitingOrigin {
        fn name(&self) -> &str {
            "waiting-origin"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                let definition = Definition::new(ctx, |_: &Ctx, (): ()| Ok(Waits));
                *self.0.lock().unwrap() = Some(definition);
                Ok(Effect::Done)
            })
        }
    }
    let root = Ctx::root().unwrap();
    let out = Arc::new(Mutex::new(None));
    let origin = root.plugin(WaitingOrigin(out.clone()));
    soon(&origin).await.unwrap();
    let definition = out.lock().unwrap().take().unwrap();
    let (_host, host) = child(&root).await;
    let copy = host.plugin(definition.at(&host, ()).unwrap());
    reach(&copy, FiberState::Pending).await;

    soon(origin.dispose()).await.unwrap();
    assert_eq!(copy.state().state, FiberState::Disposed);
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_boxed_copy_is_recorded_when_it_applies() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (origin, definition) = origin(&root, &log).await;
    let (_host, host) = child(&root).await;
    let boxed: Box<dyn DynPlugin> = Box::new(definition.at(&host, 7u32).unwrap());
    let copy = host.plugin(boxed);
    soon(&copy).await.unwrap();
    soon(origin.dispose()).await.unwrap();
    assert_eq!(copy.state().state, FiberState::Disposed);
    let order = entries(&log);
    let part = order.iter().position(|e| e == "part 7 cleaned up").unwrap();
    let own = order.iter().position(|e| e == "origin cleaned up").unwrap();
    assert!(part < own, "{order:?}");
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn copies_mount_only_inside_the_origin_s_instance() {
    let root = Ctx::root().unwrap();
    let (_a_view, a) = child(&root).await;
    let (_b_view, b) = child(&root).await;
    a.mark_scope("session").unwrap();
    b.mark_scope("session").unwrap();
    let log: Log = Arc::default();
    let (_origin, definition) = origin(&a, &log).await;

    // Inside the origin's instance, and deeper.
    let (_inner_view, inner) = child(&a).await;
    let here = a.plugin(definition.at(&a, 1u32).unwrap());
    let deeper = inner.plugin(definition.at(&inner, 2u32).unwrap());
    soon(&here).await.unwrap();
    soon(&deeper).await.unwrap();

    // Not in another instance, nor at the root.
    let refused = definition.at(&b, 3u32).err().expect("another instance");
    assert!(
        refused.to_string().contains("can only be mounted inside"),
        "{refused}"
    );
    assert!(definition.at(&root, 4u32).is_err());

    // One made at the root mounts anywhere.
    let (_root_origin, anywhere) = origin(&root, &log).await;
    let copy = b.plugin(anywhere.at(&b, 5u32).unwrap());
    soon(&copy).await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_copy_mounted_on_another_ctx_fails_to_start() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (_origin, definition) = origin(&root, &log).await;
    let (_a_view, a) = child(&root).await;
    let (_b_view, b) = child(&root).await;
    let copy = b.plugin(definition.at(&a, 1u32).unwrap());
    reach(&copy, FiberState::Failed).await;
    assert!(entries(&log).is_empty());
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_config_that_does_not_fit_fails_at() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (_origin, definition) = origin(&root, &log).await;
    let error = definition.at(&root, "deep").err().expect("not a number");
    assert!(
        error
            .to_string()
            .contains("the config of plugin definition Part"),
        "{error}"
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn clones_are_one_definition_and_cannot_be_sent() {
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let (_origin, definition) = origin(&root, &log).await;
    let (_other_origin, other) = origin(&root, &log).await;
    assert_eq!(definition, definition.clone());
    assert_ne!(definition, other);
    assert!([other.clone(), definition.clone()].contains(&definition));
    let refused = serde_json::to_value(&definition).unwrap_err().to_string();
    assert!(
        refused.contains("cannot be handed to another language"),
        "{refused}"
    );
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_definition_made_by_a_stopped_generation_has_ended() {
    let root = Ctx::root().unwrap();
    let (view, ctx) = child(&root).await;
    soon(view.dispose()).await.unwrap();
    let definition = Definition::new(&ctx, |_: &Ctx, (): ()| {
        Ok(Part {
            depth: 0,
            log: Log::default(),
        })
    });
    assert!(definition.is_ended());
    assert!(definition.at(&root, ()).is_err());
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn build_sees_where_the_copy_mounts() {
    struct Reads(Option<u64>, Log);
    impl Plugin for Reads {
        type Deps = ();
        async fn apply(&self, _: &Ctx, (): ()) -> rutis::Result {
            self.1.lock().unwrap().push(format!("{:?}", self.0));
            Ok(())
        }
    }
    struct ReadingOrigin(Arc<Mutex<Option<Definition>>>, Log);
    impl DynPlugin for ReadingOrigin {
        fn name(&self) -> &str {
            "reading-origin"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                let log = self.1.clone();
                let definition = Definition::new(ctx, move |at: &Ctx, (): ()| {
                    let value = at.get_at(&Key::<u64>::of()).map(|value| *value);
                    Ok(Reads(value, log.clone()))
                });
                *self.0.lock().unwrap() = Some(definition);
                Ok(Effect::Done)
            })
        }
    }
    let root = Ctx::root().unwrap();
    let log: Log = Arc::default();
    let out = Arc::new(Mutex::new(None));
    let origin = root.plugin(ReadingOrigin(out.clone(), log.clone()));
    soon(&origin).await.unwrap();
    let definition = out.lock().unwrap().take().unwrap();
    let (_host, host) = child(&root).await;
    host.provide_at(Key::<u64>::of(), Arc::new(42u64)).unwrap();
    let copy = host.plugin(definition.at(&host, ()).unwrap());
    soon(&copy).await.unwrap();
    assert_eq!(entries(&log), ["Some(42)"]);
    root.shutdown().await.unwrap();
}
