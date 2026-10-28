//! `rutis-bridge/session` (`docs/design-remote-session-2026-10-28.md`):
//! two rutis kernels, each with its `LocalId`, over a memory channel, or
//! one kernel and a far end scripted frame by frame.

use std::sync::Arc;
use std::time::Duration;

use rutis::events::Call;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView};
use rutis::internal::CtxInternal;
use rutis::Ctx;
use rutis_bridge::channel::{Channel, ChannelError};
use rutis_bridge::events::{ArgValue, Args};
use rutis_bridge::session::{
    call_service, host_key, HostDispatch, Reply, Value, ENDPOINT_PROTOCOL,
};
use rutis_bridge::{LocalId, PeerId, Session};
use rutis_test_support::{eventually, within};
use serde_json::{json, Value as Json};

fn id(id: &str) -> PeerId {
    PeerId::new(id).unwrap()
}

/// A kernel greeting as `name`.
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(id(name))).unwrap();
    root
}

fn read(channel: &mut Channel) -> Result<Option<Json>, ChannelError> {
    Ok(channel
        .receiver
        .recv()?
        .map(|message| serde_json::from_slice(&message).unwrap()))
}

fn send(channel: &mut Channel, frame: Json) {
    channel
        .sender
        .send(&serde_json::to_vec(&frame).unwrap())
        .unwrap();
}

/// The far end's greeting, as `endpoint` speaking `version`.
fn hello(version: u32, endpoint: &str) -> Json {
    json!({ "op": "hello", "version": version, "endpoint": endpoint,
            "capabilities": ["events-v2", "objects"] })
}

/// Until `view` is disposed.
async fn disposed(view: &FiberView) {
    let mut states = view.watch();
    within("the session unloading", async {
        while states.borrow_and_update().state != FiberState::Disposed {
            states.changed().await.unwrap();
        }
    })
    .await;
}

/// The error `view`'s load ended with.
async fn failure(view: &FiberView) -> String {
    within("the session's load", async { view.await })
        .await
        .unwrap_err()
        .to_string()
}

/// A scripted far end on its own thread: its frames as it reads them,
/// until its channel ends, and how it ended.
fn far_end(
    mut channel: Channel,
    script: impl FnOnce(&mut Channel) + Send + 'static,
) -> std::thread::JoinHandle<(Vec<Json>, Result<(), ChannelError>)> {
    std::thread::spawn(move || {
        script(&mut channel);
        let mut frames = Vec::new();
        loop {
            match read(&mut channel) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => return (frames, Ok(())),
                Err(error) => return (frames, Err(error)),
            }
        }
    })
}

async fn joined<T: Send + 'static>(thread: std::thread::JoinHandle<T>) -> T {
    within(
        "the far end",
        tokio::task::spawn_blocking(move || thread.join().unwrap()),
    )
    .await
    .unwrap()
}

/// A far end speaking another protocol version: the session fails saying
/// which end to upgrade; the far end reads this end's greeting, which says
/// its version, and then the channel's normal end.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_handshake_fails_the_session() {
    let main = kernel("main");
    let (ours, theirs) = rutis_bridge::memory::pair();
    let far = far_end(theirs, |channel| send(channel, hello(3, "old")));
    let session = main.plugin(Session::new(ours));
    let error = failure(&session).await;
    assert!(
        error.contains("the far end speaks protocol 3, this side 4: upgrade the far end"),
        "{error}"
    );
    let (frames, ended) = joined(far).await;
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(
        (
            &frames[0]["op"],
            &frames[0]["version"],
            &frames[0]["endpoint"]
        ),
        (&json!("hello"), &json!(ENDPOINT_PROTOCOL), &json!("main"))
    );
    assert_eq!(ended, Ok(()), "closed with Done");
}

/// A far end that never greets: the handshake gives up after 10 s.
#[tokio::test(start_paused = true)]
async fn a_far_end_that_does_not_greet_times_out() {
    let main = kernel("main");
    let (ours, mut theirs) = rutis_bridge::memory::pair();
    let started = tokio::time::Instant::now();
    let error = match Session::handshake(&main, ours).await {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a far end that never greets is not a session"),
    };
    assert!(
        error.contains("the far end did not greet in time"),
        "{error}"
    );
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    // Its greeting, then the channel's end.
    let channel = tokio::task::spawn_blocking(move || {
        let first = read(&mut theirs);
        (first, read(&mut theirs))
    })
    .await
    .unwrap();
    assert_eq!(channel.0.unwrap().unwrap()["op"], "hello");
    assert_eq!(channel.1, Ok(None));
}

/// Without a `LocalId` there is no session, and its channel is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_local_id_fails() {
    let main = Ctx::root().unwrap();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let far = far_end(theirs, |_| {});
    let session = main.plugin(Session::new(ours));
    assert!(failure(&session)
        .await
        .ends_with("the host provides no id for its sessions"),);
    let (frames, ended) = joined(far).await;
    assert!(frames.is_empty(), "{frames:?}");
    assert_eq!(ended, Ok(()));
}

/// Two kernels with a session each on the two ends of a channel.
async fn pair(a: &Ctx, b: &Ctx) -> (FiberView, FiberView) {
    let (ours, theirs) = rutis_bridge::memory::pair();
    let at_a = a.plugin(Session::new(ours));
    let at_b = b.plugin(Session::new(theirs));
    within("both sessions", async {
        (&at_a).await.unwrap();
        (&at_b).await.unwrap();
    })
    .await;
    (at_a, at_b)
}

/// The far end's session unloading ends the channel, and this one unloads.
#[tokio::test(flavor = "multi_thread")]
async fn the_far_end_leaving_unloads_the_session() {
    let (a, b) = (kernel("a"), kernel("b"));
    let (at_a, at_b) = pair(&a, &b).await;
    within("b's session unloading", at_b.dispose())
        .await
        .unwrap();
    disposed(&at_a).await;
}

/// This end unloading its session closes the channel with `Done`: the far
/// end reads the channel's normal end.
#[tokio::test(flavor = "multi_thread")]
async fn the_local_end_leaving_closes_with_done() {
    let main = kernel("main");
    let (ours, theirs) = rutis_bridge::memory::pair();
    let far = far_end(theirs, |channel| {
        send(channel, hello(ENDPOINT_PROTOCOL, "far"))
    });
    let session = main.plugin(Session::new(ours));
    within("the session", async { (&session).await.unwrap() }).await;
    within("the session unloading", session.dispose())
        .await
        .unwrap();
    let (frames, ended) = joined(far).await;
    assert_eq!(frames[0]["op"], "hello", "{frames:?}");
    assert_eq!(ended, Ok(()), "closed with Done");
}

/// A new channel is a new session: the old one is gone, and the new one
/// starts as the first did.
#[tokio::test(flavor = "multi_thread")]
async fn a_reconnect_is_a_new_session() {
    let (a, b) = (kernel("a"), kernel("b"));
    let (at_a, at_b) = pair(&a, &b).await;
    within("b leaving", at_b.dispose()).await.unwrap();
    disposed(&at_a).await;
    let (again_a, _again_b) = pair(&a, &b).await;
    assert_eq!(again_a.state().state, FiberState::Active);
}

/// A handshake alone mounts nothing; dropping the greeted session closes
/// its channel. A greeted session mounts once the old one is gone, as a
/// listener replacing a connection does.
#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_alone_registers_nothing() {
    let (a, b) = (kernel("a"), kernel("b"));
    (&a.plugin(provides("clock", "clock", "noon")))
        .await
        .unwrap();
    let (ours, theirs) = rutis_bridge::memory::pair();
    let at_b = b.plugin(Session::new(theirs));
    let greeted = within("the handshake", Session::handshake(&a, ours))
        .await
        .unwrap();
    assert_eq!(greeted.id(), "b");
    let handle = greeted.handle();
    assert_eq!(handle.id(), "b");
    within("b's session", async { (&at_b).await.unwrap() }).await;
    assert!(
        !a.diagnostics()
            .plugins
            .iter()
            .any(|plugin| plugin.name == "rutis-bridge/session"),
        "nothing is mounted at a"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        b.get_key::<dyn HostDispatch>(host_key("clock")).is_none(),
        "a greeted session shares nothing"
    );
    drop(greeted);
    disposed(&at_b).await;

    // A replacement: the old session unloads, then the new one mounts.
    let (old_a, _old_b) = pair(&a, &b).await;
    let (ours, theirs) = rutis_bridge::memory::pair();
    let _new_b = b.plugin(Session::new(theirs));
    let greeted = within("the handshake", Session::handshake(&a, ours))
        .await
        .unwrap();
    within("the old session unloading", old_a.dispose())
        .await
        .unwrap();
    let new_a = a.plugin(greeted);
    within("the new session", async { (&new_a).await.unwrap() }).await;
    assert_eq!(get(&b, "clock").await, "noon");
}

// ── services and events ─────────────────────────────────────────

/// Answers `get` with its text; `slow` never answers.
struct Answer(&'static str);

impl HostDispatch for Answer {
    fn invoke(&self, method: &str, _: Value) -> Reply {
        match method {
            "get" => Ok(json!(self.0).into()),
            _ => Ok(Value::future(std::future::pending())),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "get": "async", "slow": "async" }))
    }
}

/// The plugin `name` (row `row`, if any) providing `service`, whose `get`
/// answers `answer`.
struct Provides {
    name: &'static str,
    row: Option<&'static str>,
    service: &'static str,
    answer: &'static str,
}

impl DynPlugin for Provides {
    fn name(&self) -> &str {
        self.name
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            if let Some(row) = self.row {
                ctx.mark_row(row)?;
            }
            ctx.provide_key::<dyn HostDispatch>(
                host_key(self.service),
                Arc::new(Answer(self.answer)),
            )?;
            Ok(Effect::Done)
        })
    }
}

fn provides(name: &'static str, service: &'static str, answer: &'static str) -> Provides {
    Provides {
        name,
        row: None,
        service,
        answer,
    }
}

/// What `service`'s `get` answers at `ctx`, once it is there.
async fn get(ctx: &Ctx, service: &str) -> String {
    eventually(
        || {
            ctx.get_key::<dyn HostDispatch>(host_key(service))
                .map(|_| ())
        },
        &format!("{service} here"),
    )
    .await;
    within("the call", call_service(ctx, service, "get", ()))
        .await
        .unwrap()
}

/// The state of the plugin `name` at `ctx`, and its error.
fn plugin(ctx: &Ctx, name: &str) -> Option<(FiberState, Option<String>)> {
    ctx.diagnostics()
        .plugins
        .into_iter()
        .find(|plugin| plugin.name == name)
        .map(|plugin| (plugin.state, plugin.error.map(|error| error.to_string())))
}

/// Each kernel uses the other's services, provided here by the remote
/// plugin `<id>:<plugin>`.
#[tokio::test(flavor = "multi_thread")]
async fn two_kernels_use_each_others_services() {
    let (a, b) = (kernel("a"), kernel("b"));
    (&a.plugin(provides("clock", "clock", "noon")))
        .await
        .unwrap();
    let _pair = pair(&a, &b).await;
    // Provided after the session started.
    (&b.plugin(provides("weather", "weather", "rain")))
        .await
        .unwrap();
    assert_eq!(get(&b, "clock").await, "noon");
    assert_eq!(get(&a, "weather").await, "rain");
    assert_eq!(plugin(&b, "a:clock").map(|p| p.0), Some(FiberState::Active));
    assert_eq!(
        plugin(&a, "b:weather").map(|p| p.0),
        Some(FiberState::Active)
    );
}

/// A name provided on both sides: the far end's plugin, the later
/// provider, fails, naming who has it here.
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_name_is_refused_by_kernel_rules() {
    let (a, b) = (kernel("office"), kernel("main"));
    (&a.plugin(provides("weather", "weather", "office")))
        .await
        .unwrap();
    (&b.plugin(Provides {
        row: Some("weather"),
        ..provides("local-weather", "weather", "main")
    }))
        .await
        .unwrap();
    let _pair = pair(&a, &b).await;
    let refused = eventually(
        || {
            plugin(&b, "office:weather")
                .and_then(|(state, error)| (state == FiberState::Failed).then_some(error))
        },
        "office:weather failing",
    )
    .await
    .unwrap();
    assert!(
        refused.contains(
            "office:weather: weather is provided here by row weather: not registered from office"
        ),
        "{refused}"
    );
    assert_eq!(get(&b, "weather").await, "main");
}

/// A service never goes back over the session it came by; through a
/// kernel between two others it reaches the third.
#[tokio::test(flavor = "multi_thread")]
async fn a_service_never_returns_through_its_session() {
    let (a, b, c) = (kernel("a"), kernel("b"), kernel("c"));
    (&a.plugin(provides("clock", "clock", "noon")))
        .await
        .unwrap();
    let _ab = pair(&a, &b).await;
    let _bc = pair(&b, &c).await;
    assert_eq!(get(&b, "clock").await, "noon");
    assert_eq!(get(&c, "clock").await, "noon");
    assert_eq!(
        plugin(&c, "b:a:clock").map(|p| p.0),
        Some(FiberState::Active)
    );
    // Nothing of a's own came back to a.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !a.diagnostics()
            .plugins
            .iter()
            .any(|p| p.name.starts_with("b:")),
        "{:?}",
        a.diagnostics()
            .plugins
            .iter()
            .map(|p| &p.name)
            .collect::<Vec<_>>()
    );
}

/// Listeners of each kernel hear what the other emits.
#[tokio::test(flavor = "multi_thread")]
async fn events_reach_both_ways() {
    let (a, b) = (kernel("a"), kernel("b"));
    a.listen::<Args>()
        .name("t/ping")
        .on(|_: Arc<Args>, _: Call<Args>| async { Ok(Some(ArgValue(json!("from a").into()))) })
        .unwrap();
    b.listen::<Args>()
        .name("t/pong")
        .on(|_: Arc<Args>, _: Call<Args>| async { Ok(Some(ArgValue(json!("from b").into()))) })
        .unwrap();
    let _pair = pair(&a, &b).await;
    assert_eq!(heard(&b, "t/ping").await, json!("from a"));
    assert_eq!(heard(&a, "t/pong").await, json!("from b"));
}

/// The first answer of `serial` of `name` at `ctx`, once someone answers.
async fn heard(ctx: &Ctx, name: &str) -> Json {
    eventually_async(|| async {
        ctx.named(name)
            .serial(Args(vec![]))
            .await
            .unwrap()
            .map(|ArgValue(value)| value.json().unwrap())
    })
    .await
}

async fn eventually_async<T, F: std::future::Future<Output = Option<T>>>(
    mut check: impl FnMut() -> F,
) -> T {
    within("an answer", async {
        loop {
            if let Some(found) = check().await {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

/// The far end leaving: its plugins here unload with the session, and a
/// call into one of them under way fails.
#[tokio::test(flavor = "multi_thread")]
async fn the_far_end_leaving_unloads_its_plugins() {
    let (a, b) = (kernel("a"), kernel("b"));
    (&b.plugin(provides("clock", "clock", "noon")))
        .await
        .unwrap();
    let (at_a, at_b) = pair(&a, &b).await;
    assert_eq!(get(&a, "clock").await, "noon");
    let slow = {
        let a = a.clone();
        tokio::spawn(async move { call_service::<Json>(&a, "clock", "slow", ()).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    within("b's session unloading", at_b.dispose())
        .await
        .unwrap();
    disposed(&at_a).await;
    assert!(within("the call", slow).await.unwrap().is_err());
    assert!(plugin(&a, "b:clock").is_none_or(|p| p.0 == FiberState::Disposed));
    assert!(a.get_key::<dyn HostDispatch>(host_key("clock")).is_none());
}
