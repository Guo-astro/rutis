//! The synchronous waterfall, `ctx.waterfall(event).sync(terminal)` (#946):
//! the chain runs inline on the emitting thread, each `on_sync` listener's
//! `next` synchronous, under the rules of `bail`.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rutis::events::{ListenerInfo, Mode, SyncCall};
use rutis::host::{BoxFuture, CtxHost, ErrorKind, ListenerKind};
use rutis::internal::events::{RemoteCall, RemoteEntry, RemoteListener, RemoteSyncCall, Value};
use rutis::internal::{CtxInternal, EventOptions};
use rutis::{Ctx, Error, Event};
use serde::{Deserialize, Serialize};

// ── fixtures ────────────────────────────────────────────────────

/// An event that keeps a record no other process could read: the steps
/// stay with the emitter, which reads them after the dispatch through its
/// own `Arc`.
#[derive(Serialize, Deserialize)]
struct Draft {
    text: String,
    #[serde(skip)]
    steps: Mutex<Vec<String>>,
}
impl Event for Draft {
    const NAME: &'static str = "t/draft";
    type Value = String;
}

fn draft(text: &str) -> Arc<Draft> {
    Arc::new(Draft {
        text: text.into(),
        steps: Mutex::default(),
    })
}

fn steps(draft: &Draft) -> Vec<String> {
    draft.steps.lock().unwrap().clone()
}

/// A middleware: logs `<label>:in`, runs the rest, logs `<label>:out`
/// and appends its label to what the rest returned.
fn middleware(
    label: &'static str,
) -> impl Fn(Arc<Draft>, SyncCall<'_, Draft>) -> Result<Option<String>, Error> + Send + Sync + 'static
{
    move |draft: Arc<Draft>, call: SyncCall<'_, Draft>| {
        assert_eq!(call.mode(), Mode::Waterfall);
        draft.steps.lock().unwrap().push(format!("{label}:in"));
        let rest = call.next()?.unwrap_or_default();
        draft.steps.lock().unwrap().push(format!("{label}:out"));
        Ok(Some(format!("{rest}{label}")))
    }
}

// ── chain ───────────────────────────────────────────────────────

/// Listeners registered earlier wrap later ones, as in the asynchronous
/// waterfall; the last `next` calls the terminal; every listener borrows
/// the emitter's event, so what they record is read through its `Arc`.
///
/// guarantee: events/waterfall
#[tokio::test]
async fn sync_waterfall_chains_listeners_into_the_terminal() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    root.on_sync::<Draft>(middleware("b")).unwrap();

    let offered = draft("t");
    // The terminal borrows a local value that is not `Send`.
    let local = Rc::new(String::from("!"));
    let out = root
        .waterfall(offered.clone())
        .sync(|draft| {
            draft.steps.lock().unwrap().push("terminal".into());
            Ok(Some(format!("{}{local}", draft.text)))
        })
        .unwrap();
    assert_eq!(out.as_deref(), Some("t!ba"));
    assert_eq!(
        steps(&offered),
        ["a:in", "b:in", "terminal", "b:out", "a:out"]
    );
    root.shutdown().await.unwrap();
}

/// With no listener, the terminal alone; without a value from it, `None`.
/// A listener that does not call `next` vetoes the rest.
///
/// guarantee: events/waterfall
#[tokio::test]
async fn sync_waterfall_without_listeners_and_with_a_veto() {
    let root = Ctx::root().unwrap();
    let offered = draft("t");
    let out = root
        .waterfall(offered.clone())
        .sync(|draft| Ok(Some(draft.text.clone())))
        .unwrap();
    assert_eq!(out.as_deref(), Some("t"));
    assert_eq!(root.waterfall(draft("t")).sync(|_| Ok(None)).unwrap(), None);

    // Under a name chosen at run time.
    let out = root
        .named("t/other")
        .waterfall(draft("o"))
        .sync(|draft| Ok(Some(draft.text.clone())))
        .unwrap();
    assert_eq!(out.as_deref(), Some("o"));

    root.on_sync::<Draft>(|draft, _| {
        draft.steps.lock().unwrap().push("veto".into());
        Ok(Some("vetoed".into()))
    })
    .unwrap();
    root.on_sync::<Draft>(middleware("b")).unwrap();
    let offered = draft("t");
    let out = root
        .waterfall(offered.clone())
        .sync(|_| panic!("vetoed: the terminal is not called"))
        .unwrap();
    assert_eq!(out.as_deref(), Some("vetoed"));
    assert_eq!(steps(&offered), ["veto"]);
    root.shutdown().await.unwrap();
}

/// A failure in the chain or the terminal is the dispatch's.
#[tokio::test]
async fn sync_waterfall_errors_reach_the_emitter() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    let offered = draft("t");
    let error = root
        .waterfall(offered.clone())
        .sync(|_| Err(ErrorKind::Closed.into()))
        .unwrap_err();
    assert!(matches!(&*error, ErrorKind::Closed), "{error:?}");
    assert_eq!(steps(&offered), ["a:in"]);
    root.shutdown().await.unwrap();
}

// ── the rules of bail ───────────────────────────────────────────

/// An asynchronous listener (`on`) fails the dispatch with
/// `AsyncListener`; listeners before it ran, the terminal did not.
///
/// guarantee: events/serial-bail
#[tokio::test]
async fn sync_waterfall_meets_an_async_listener() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    root.on::<Draft>(|_, call| async move { call.next().await })
        .unwrap();
    let offered = draft("t");
    let error = root
        .waterfall(offered.clone())
        .sync(|_| panic!("the terminal is not reached"))
        .unwrap_err();
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    assert_eq!(
        error.to_string(),
        "listener of t/draft in plugin root is asynchronous and cannot run in a synchronous waterfall"
    );
    assert_eq!(steps(&offered), ["a:in"]);
    root.shutdown().await.unwrap();
}

/// A terminal given with `.terminal(…)` is asynchronous: `.sync` after it
/// is a `Validation` error, and nothing runs.
#[tokio::test]
async fn sync_after_an_async_terminal_is_refused() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    let offered = draft("t");
    let error = root
        .waterfall(offered.clone())
        .terminal(|_| async { Ok(Some("async".to_owned())) })
        .sync(|_| Ok(Some("sync".to_owned())))
        .unwrap_err();
    assert!(matches!(&*error, ErrorKind::Validation { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        "config validation failed: [\"a waterfall with an asynchronous terminal cannot run \
         synchronously: pass the terminal to .sync(…) instead of .terminal(…)\"]"
    );
    assert!(steps(&offered).is_empty());
    root.shutdown().await.unwrap();
}

/// A synchronous listener still cannot call `next` in an asynchronous
/// waterfall: only `.sync(…)` hands it one.
///
/// guarantee: core/D17
#[tokio::test]
async fn a_sync_listener_calls_next_only_in_a_sync_waterfall() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    let error = root
        .waterfall(draft("t"))
        .terminal(|_| async { Ok(Some("t".to_owned())) })
        .await
        .unwrap_err();
    assert_eq!(error.wire_name(), Some("WaterfallNext"));
    let out = root
        .waterfall(draft("t"))
        .sync(|_| Ok(Some("t".to_owned())))
        .unwrap();
    assert_eq!(out.as_deref(), Some("ta"));
    root.shutdown().await.unwrap();
}

// ── listeners in another process ────────────────────────────────

struct Remote {
    kind: ListenerKind,
    called: Arc<Mutex<Vec<String>>>,
}

impl RemoteListener for Remote {
    fn kind(&self) -> ListenerKind {
        self.kind
    }
    fn call<'a>(&'a self, call: RemoteCall<'a>) -> BoxFuture<'a, Result<Option<Value>, Error>> {
        self.called.lock().unwrap().push(call.mode().to_string());
        Box::pin(async { Ok(None) })
    }
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, Error> {
        self.called.lock().unwrap().push(call.mode().to_string());
        Ok(None)
    }
}

fn remote(root: &Ctx, name: &str, kind: ListenerKind) -> Arc<Mutex<Vec<String>>> {
    let called = Arc::<Mutex<Vec<String>>>::default();
    root.events()
        .on_remote(
            root,
            RemoteEntry {
                name: name.into(),
                prefix: false,
                info: ListenerInfo {
                    name: String::new(),
                    prefix: false,
                    plugin: "row".into(),
                    row: Some("row".into()),
                    runtime: Some("node".into()),
                    node: None,
                    scope: Vec::new(),
                    global: false,
                },
                options: EventOptions::default(),
                origin: 7,
            },
            Arc::new(Remote {
                kind,
                called: called.clone(),
            }),
        )
        .unwrap();
    called
}

/// A listener in another process cannot take part: the protocol has no
/// synchronous `next` to hand it. A synchronous one fails the dispatch with
/// `Validation`, an asynchronous one with `AsyncListener`, before it is
/// called; listeners before it ran.
#[tokio::test]
async fn listeners_elsewhere_fail_a_sync_waterfall() {
    let root = Ctx::root().unwrap();
    root.on_sync::<Draft>(middleware("a")).unwrap();
    let called = remote(&root, Draft::NAME, ListenerKind::Sync);
    let offered = draft("t");
    let error = root
        .waterfall(offered.clone())
        .sync(|_| panic!("the terminal is not reached"))
        .unwrap_err();
    assert!(matches!(&*error, ErrorKind::Validation { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        "config validation failed: [\"listener of t/draft in row row is in another process \
         and cannot run in a synchronous waterfall; register it in this process with on_sync\"]"
    );
    assert_eq!(steps(&offered), ["a:in"]);
    assert!(called.lock().unwrap().is_empty());

    let called = remote(&root, "t/async", ListenerKind::Async);
    let error = root
        .named("t/async")
        .waterfall(draft("t"))
        .sync(|_| Ok(None))
        .unwrap_err();
    assert_eq!(error.wire_name(), Some("AsyncListener"));
    assert!(called.lock().unwrap().is_empty());
    root.shutdown().await.unwrap();
}

// ── Dim's timeline ──────────────────────────────────────────────

/// Dim's `Timeline` (arcships/dim, `dim-agent-runtime/src/core/timeline.rs`):
/// a synchronous `append` offers each draft to the branch's `before`
/// middleware under a private name; each wrapper calls downstream first,
/// then transforms and records its step in the event, which never leaves
/// the process. Middleware registered earlier wraps later ones, so later
/// registrations transform first.
mod dim {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct EventDraft {
        pub kind: String,
        pub body: String,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct TraceStep {
        pub handler: String,
        pub result: Option<EventDraft>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct BeforeCommit {
        #[serde(skip)]
        pub name: String,
        pub draft: EventDraft,
        #[serde(skip)]
        pub steps: Mutex<Vec<TraceStep>>,
    }
    impl Event for BeforeCommit {
        const NAME: &'static str = "";
        type Value = EventDraft;
        fn name(&self) -> &str {
            &self.name
        }
    }

    pub const BEFORE: &str = "dim/timeline/7/before";

    /// `Timeline::before`.
    pub fn before(
        ctx: &Ctx,
        name: &str,
        handler: impl Fn(EventDraft) -> Result<EventDraft, Error> + Send + Sync + 'static,
    ) -> Result<rutis::Disposer, Error> {
        let name = name.to_owned();
        let middleware = move |offered: Arc<BeforeCommit>, next: SyncCall<'_, BeforeCommit>| {
            let draft = next.next()?.ok_or_else(|| {
                Error::from(ErrorKind::PluginFailed(
                    "the before chain returned no draft".into(),
                ))
            })?;
            let result = handler(draft.clone())?;
            offered.steps.lock().unwrap().push(TraceStep {
                handler: name.clone(),
                result: (result != draft).then(|| result.clone()),
            });
            Ok(Some(result))
        };
        ctx.listen::<BeforeCommit>()
            .name(BEFORE)
            .on_sync(middleware)
    }

    /// The part of `Timeline::append` that runs the chain.
    pub fn append(ctx: &Ctx, original: EventDraft) -> (EventDraft, Vec<TraceStep>) {
        let offered = Arc::new(BeforeCommit {
            name: BEFORE.into(),
            draft: original.clone(),
            steps: Mutex::default(),
        });
        let result = ctx
            .waterfall(offered.clone())
            .sync(|offered| Ok(Some(offered.draft.clone())));
        match result {
            Ok(Some(draft)) => {
                let steps = std::mem::take(&mut *offered.steps.lock().unwrap());
                (draft, steps)
            }
            Ok(None) | Err(_) => (original, Vec::new()),
        }
    }
}

#[tokio::test]
async fn dims_timeline_before_chain() {
    use dim::*;
    let root = Ctx::root().unwrap();
    let note = |body: &str| EventDraft {
        kind: "note".into(),
        body: body.into(),
    };

    // No middleware: the draft as offered, no steps.
    let (draft, steps) = append(&root, note("hi"));
    assert_eq!(draft, note("hi"));
    assert!(steps.is_empty());

    before(&root, "upper", |mut draft| {
        draft.body = draft.body.to_uppercase();
        Ok(draft)
    })
    .unwrap();
    let keep = before(&root, "keep", Ok).unwrap();
    before(&root, "bang", |mut draft| {
        draft.body.push('!');
        Ok(draft)
    })
    .unwrap();

    // Registered last, transforms first: "hi" → "hi!" → same → "HI!".
    let (draft, steps) = append(&root, note("hi"));
    assert_eq!(draft, note("HI!"));
    assert_eq!(
        steps,
        [
            TraceStep {
                handler: "bang".into(),
                result: Some(note("hi!")),
            },
            TraceStep {
                handler: "keep".into(),
                result: None,
            },
            TraceStep {
                handler: "upper".into(),
                result: Some(note("HI!")),
            },
        ]
    );

    // A middleware disposed leaves the chain.
    keep.dispose().await.unwrap();
    let (_, steps) = append(&root, note("x"));
    assert_eq!(
        steps.iter().map(|s| s.handler.as_str()).collect::<Vec<_>>(),
        ["bang", "upper"]
    );

    // A failing middleware: the original, no steps (as Dim falls back).
    before(&root, "fails", |_| Err(ErrorKind::Closed.into())).unwrap();
    let (draft, steps) = append(&root, note("x"));
    assert_eq!(draft, note("x"));
    assert!(steps.is_empty());
    root.shutdown().await.unwrap();
}
