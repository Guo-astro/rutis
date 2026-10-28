//! `rutis-bridge/session` (`docs/design-remote-session-2026-10-28.md`):
//! two rutis kernels, each with its `LocalId`, over a memory channel, or
//! one kernel and a far end scripted frame by frame.

use std::time::Duration;

use rutis::host::{CtxHost, FiberState, FiberView};
use rutis::Ctx;
use rutis_bridge::channel::{Channel, ChannelError};
use rutis_bridge::session::ENDPOINT_PROTOCOL;
use rutis_bridge::{LocalId, PeerId, Session};
use rutis_test_support::within;
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
}
