//! `sessions.open(far)`: a session for a connection plugin of any language
//! (`docs/design-remote-session-2026-10-28.md` §5.2). The caller hands its
//! end as `far = { send(bytes), close({ kind, reason? }) }` and gets
//! `near = { receive(bytes), end({ kind, reason? }) }`; the session is
//! mounted below the calling row, on a [`Channel`] made of the two.
//!
//! - What the session sends goes to `far.send`, one message at a time, in
//!   order, from a queue of [`CAPACITY`] messages: a full queue blocks the
//!   session's sender, as any channel's does.
//! - What `near.receive` takes waits in a queue of [`CAPACITY`] messages:
//!   its promise settles once the message is queued, so a full queue holds
//!   the caller back.
//! - `far.close` is called once, when the session closes the channel;
//!   `near.end` ends what the session receives (`done`: a normal end,
//!   `replaced`, `rejected`).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::{Ctx, Error as RutisError};
use serde_json::{json, Value as Json};

use crate::channel::{Channel, ChannelError, Close, Closer, Receiver, Sender};
use crate::memory::CAPACITY;
use crate::session::rpc::{settle, Reference, Reply, Value};
use crate::session::{caller_row, code, host_key, Error, HostDispatch};

use super::{Session, NAME};

/// The root plugin serving `sessions` ([`Session::service`]).
pub(super) struct Sessions;

impl DynPlugin for Sessions {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        NAME
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            ctx.provide_at(
                host_key("sessions"),
                Arc::new(Service {
                    tokio: tokio::runtime::Handle::current(),
                }),
            )?;
            Ok(Effect::Done)
        })
    }
}

/// The service `sessions`.
pub(super) struct Service {
    tokio: tokio::runtime::Handle,
}

impl HostDispatch for Service {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        match method {
            "open" => self.open(args),
            _ => Err(Error::no_method("sessions", method, vec!["open"])),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(json!({ "open": "sync" }))
    }
}

/// A field of a record argument that must be a function.
fn function(
    fields: &mut BTreeMap<String, Value>,
    name: &str,
    of: &str,
) -> Result<Reference, Error> {
    fields
        .remove(name)
        .ok_or_else(|| bad(format!("{of} needs the function {name}")))?
        .reference()
        .map_err(|_| bad(format!("{of}.{name} must be a function")))
}

fn bad(message: String) -> Error {
    Error::binding(code::BAD_ARGUMENT, message, None)
}

impl Service {
    fn open(&self, args: Value) -> Reply {
        let caller = caller_row().ok_or_else(|| {
            Error::binding(
                code::NOT_DECLARED,
                "declare sessions in inject".into(),
                Some(json!({ "service": "sessions" })),
            )
        })?;
        // A record of functions (Python), or an object whose methods they
        // are (a plain object from Node).
        let far = match args.list()?.into_iter().next() {
            Some(Value::Record(mut fields)) => Far::Functions {
                send: function(&mut fields, "send", "far")?,
                close: function(&mut fields, "close", "far")?,
            },
            Some(Value::Reference(object)) if object.is_object() => Far::Object(object),
            _ => return Err(bad("sessions.open needs far = { send, close }".into())),
        };
        let (channel, near) = far_channel(far, self.tokio.clone());
        caller.ctx().plugin(Session::new(channel));
        Ok(near)
    }
}

/// The caller's end: its `send` and `close`.
pub(super) enum Far {
    Functions { send: Reference, close: Reference },
    Object(Reference),
}

impl Far {
    async fn call(&self, method: &str, argument: Value) -> Reply {
        let args = Value::List(vec![argument]);
        let reply = match (self, method) {
            (Far::Functions { send, .. }, "send") => send.call_async(args).await?,
            (Far::Functions { close, .. }, _) => close.call_async(args).await?,
            (Far::Object(object), method) => object.call_method_async(method, args).await?,
        };
        settle(reply).await
    }
}

/// A channel whose far end is `far`, and the `near` record that feeds what
/// it receives.
pub(super) fn far_channel(far: Far, tokio: tokio::runtime::Handle) -> (Channel, Value) {
    let far = Arc::new(far);
    let queue = Arc::new(Queue::default());
    let (sender, outgoing) = std::sync::mpsc::sync_channel::<Vec<u8>>(CAPACITY);
    // One message at a time, in order.
    let draining = tokio.clone();
    let sending = far.clone();
    let _ = std::thread::Builder::new()
        .name("rutis-sessions-open".into())
        .spawn(move || {
            for message in outgoing {
                let sent = draining.block_on(sending.call("send", Value::Bytes(message)));
                if sent.is_err() {
                    return;
                }
            }
        });
    let channel = Channel {
        sender: Box::new(FarSender(sender)),
        receiver: Box::new(NearReceiver(queue.clone())),
        closer: Arc::new(FarCloser {
            far,
            closed: AtomicBool::new(false),
            queue: queue.clone(),
            tokio,
        }),
    };
    let receiving = queue.clone();
    let receive = Value::callback(move |args| {
        let message = match args.list()?.into_iter().next() {
            Some(Value::Bytes(message)) => message,
            _ => return Err(bad("near.receive takes the message's bytes".into())),
        };
        let queue = receiving.clone();
        Ok(Value::future(async move {
            queue.receive(message).await;
            Ok(Value::Undefined)
        }))
    });
    let end = Value::callback(move |args| {
        let why = match args.list()?.into_iter().next() {
            Some(why) => why.json()?,
            None => Json::Null,
        };
        let why = match why["kind"].as_str() {
            Some("done") => Close::Done,
            Some("replaced") => Close::Replaced,
            Some("rejected") => {
                Close::Rejected(why["reason"].as_str().unwrap_or_default().to_owned())
            }
            _ => {
                return Err(bad(
                    "near.end takes { kind: 'done' | 'replaced' | 'rejected', reason? }".into(),
                ))
            }
        };
        queue.end(why);
        Ok(Value::Undefined)
    });
    let near = Value::Record(BTreeMap::from([
        ("receive".to_owned(), receive),
        ("end".to_owned(), end),
    ]));
    (channel, near)
}

struct FarSender(std::sync::mpsc::SyncSender<Vec<u8>>);

impl Sender for FarSender {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.0
            .send(message.to_vec())
            .map_err(|_| ChannelError::Closed {
                reason: "the far end's send failed".into(),
            })
    }
}

struct FarCloser {
    far: Arc<Far>,
    closed: AtomicBool,
    queue: Arc<Queue>,
    tokio: tokio::runtime::Handle,
}

impl Closer for FarCloser {
    fn close(&self, why: Close) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.queue.close();
        let why = match why {
            Close::Done => json!({ "kind": "done" }),
            Close::Replaced => json!({ "kind": "replaced" }),
            Close::Rejected(reason) => json!({ "kind": "rejected", "reason": reason }),
        };
        let far = self.far.clone();
        self.tokio.spawn(async move {
            let _ = far.call("close", Value::Data(why)).await;
        });
    }
}

struct NearReceiver(Arc<Queue>);

impl Receiver for NearReceiver {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        self.0.recv()
    }
}

/// What `near.receive` takes, until the session reads it.
#[derive(Default)]
pub(super) struct Queue {
    state: Mutex<QueueState>,
    readable: Condvar,
    writable: tokio::sync::Notify,
}

#[derive(Default)]
struct QueueState {
    messages: VecDeque<Vec<u8>>,
    /// How the far end ended (`near.end`).
    end: Option<Close>,
    /// Closed here.
    closed: bool,
}

impl Queue {
    /// Queue `message`, waiting while the queue is full. After the end it
    /// is dropped.
    pub(super) async fn receive(&self, message: Vec<u8>) {
        let mut message = Some(message);
        loop {
            let writable = self.writable.notified();
            let mut writable = std::pin::pin!(writable);
            writable.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if state.closed || state.end.is_some() {
                    return;
                }
                if state.messages.len() < CAPACITY {
                    state.messages.extend(message.take());
                    self.readable.notify_all();
                    return;
                }
            }
            writable.await;
        }
    }

    fn end(&self, why: Close) {
        let mut state = self.state.lock().unwrap();
        state.end.get_or_insert(why);
        self.readable.notify_all();
        self.writable.notify_waiters();
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.readable.notify_all();
        self.writable.notify_waiters();
    }

    fn recv(&self) -> Result<Option<Vec<u8>>, ChannelError> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.closed {
                return Err(ChannelError::Closed {
                    reason: "the channel is closed".into(),
                });
            }
            if let Some(message) = state.messages.pop_front() {
                self.writable.notify_waiters();
                return Ok(Some(message));
            }
            match &state.end {
                Some(Close::Done) => return Ok(None),
                Some(Close::Replaced) => {
                    return Err(ChannelError::Closed {
                        reason: "replaced by a new connection".into(),
                    })
                }
                Some(Close::Rejected(reason)) => {
                    return Err(ChannelError::Rejected {
                        reason: reason.clone(),
                    })
                }
                None => state = self.readable.wait(state).unwrap(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rutis::host::FiberState;
    use rutis::internal::CtxInternal;
    use rutis_test_support::{eventually, within};

    use super::*;
    use crate::session::{call_service, with_caller_row, CallerRow};
    use crate::{LocalId, PeerId};

    /// `near.receive` settles once its message is queued: with 64 waiting,
    /// the 65th waits until the session reads one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_full_queue_holds_receive_back() {
        let queue = Arc::new(Queue::default());
        for n in 0..CAPACITY {
            within("a free place", queue.receive(vec![n as u8])).await;
        }
        let mut next = Box::pin(queue.receive(vec![CAPACITY as u8]));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut next)
                .await
                .is_err(),
            "the 65th waits"
        );
        let reader = queue.clone();
        let first = tokio::task::spawn_blocking(move || reader.recv())
            .await
            .unwrap();
        assert_eq!(first, Ok(Some(vec![0])));
        within("the 65th, queued", next).await;
        // The end comes after what was queued.
        queue.end(Close::Rejected("token".into()));
        let reader = queue.clone();
        let rest = tokio::task::spawn_blocking(move || {
            let mut rest = Vec::new();
            loop {
                match reader.recv() {
                    Ok(Some(message)) => rest.push(message[0]),
                    end => return (rest, end),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(rest.0, (1..=CAPACITY as u8).collect::<Vec<_>>());
        assert_eq!(
            rest.1,
            Err(ChannelError::Rejected {
                reason: "token".into()
            })
        );
    }

    struct Clock;

    impl HostDispatch for Clock {
        fn invoke(&self, _: &str, _: Value) -> Reply {
            Ok(json!("noon").into())
        }

        fn methods(&self) -> Option<Json> {
            Some(json!({ "get": "async" }))
        }
    }

    /// A plugin: provides `clock` when `provides`, and hands its context
    /// out.
    struct Plugin {
        name: &'static str,
        provides: bool,
        ctx: std::sync::mpsc::Sender<Ctx>,
    }

    impl DynPlugin for Plugin {
        fn name(&self) -> &str {
            self.name
        }

        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
            Box::pin(async move {
                if self.provides {
                    ctx.provide_at(host_key("clock"), Arc::new(Clock) as Arc<dyn HostDispatch>)?;
                }
                let _ = self.ctx.send(ctx.clone());
                Ok(Effect::Done)
            })
        }
    }

    fn plugin(ctx: &Ctx, name: &'static str, provides: bool) -> (rutis::host::FiberView, Ctx) {
        let (sender, ctx_of) = std::sync::mpsc::channel();
        let view = ctx.plugin(Plugin {
            name,
            provides,
            ctx: sender,
        });
        (view, rutis_test_support::recv_within("the plugin", &ctx_of))
    }

    fn kernel(id: &str) -> Ctx {
        let root = Ctx::root().unwrap();
        root.provide(LocalId(PeerId::new(id).unwrap())).unwrap();
        root
    }

    /// A connection plugin of another language: `sessions.open` with its
    /// `send` and `close`, its messages fed to `near.receive`. The session
    /// is mounted below it, and its end calls `close` with `done`.
    #[tokio::test(flavor = "multi_thread")]
    async fn sessions_open_runs_a_session_on_the_callers_functions() {
        let (relay, office) = (kernel("relay"), kernel("office"));
        within("sessions", async {
            (&relay.plugin(super::super::Session::service()))
                .await
                .unwrap()
        })
        .await;
        let _clock = plugin(&office, "clock", true);
        let (ours, theirs) = crate::memory::pair();
        let _at_office = office.plugin(super::super::Session::new(theirs));

        let Channel {
            sender,
            mut receiver,
            closer,
        } = ours;
        let sender = Arc::new(Mutex::new(sender));
        let closed: Arc<Mutex<Option<Json>>> = Arc::default();
        let far = {
            let closed = closed.clone();
            Value::Record(BTreeMap::from([
                (
                    "send".to_owned(),
                    Value::callback(move |args| {
                        let Some(Value::Bytes(message)) = args.list()?.into_iter().next() else {
                            return Err(bad("bytes".into()));
                        };
                        sender
                            .lock()
                            .unwrap()
                            .send(&message)
                            .map_err(|error| Error::Transport(error.to_string()))?;
                        Ok(Value::Undefined)
                    }),
                ),
                (
                    "close".to_owned(),
                    Value::callback(move |args| {
                        *closed.lock().unwrap() = Some(args.json()?);
                        closer.close(Close::Done);
                        Ok(Value::Undefined)
                    }),
                ),
            ]))
        };
        let args = Value::List(vec![far]);
        let service = relay
            .get_key::<dyn HostDispatch>(host_key("sessions").erase())
            .unwrap();

        let refused = service.invoke("open", args.clone()).err().unwrap();
        assert_eq!(refused.code(), Some(code::NOT_DECLARED));
        assert_eq!(refused.to_string(), "declare sessions in inject");

        let (row, row_ctx) = plugin(&relay, "connection", false);
        let caller = CallerRow::new("connection", "connection", row_ctx);
        let near = with_caller_row(Some(caller), || service.invoke("open", args)).unwrap();
        let Value::Record(mut near) = near else {
            panic!("near is a record")
        };
        let receive = near.remove("receive").unwrap().reference().unwrap();
        let end = near.remove("end").unwrap().reference().unwrap();
        let tokio = tokio::runtime::Handle::current();
        std::thread::spawn(move || loop {
            let call = |reference: &Reference, value: Value| {
                tokio.block_on(async {
                    settle(reference.call_async(Value::List(vec![value])).await?).await
                })
            };
            match receiver.recv() {
                Ok(Some(message)) => {
                    let _ = call(&receive, Value::Bytes(message));
                }
                _ => {
                    let _ = call(&end, Value::Data(json!({ "kind": "done" })));
                    return;
                }
            }
        });

        let now: String = within("the clock", async {
            eventually(
                || relay.get_key::<dyn HostDispatch>(host_key("clock").erase()),
                "office's clock",
            )
            .await;
            call_service(&relay, "clock", "get", ()).await
        })
        .await
        .unwrap();
        assert_eq!(now, "noon");
        let session = relay
            .diagnostics()
            .plugins
            .into_iter()
            .find(|plugin| plugin.name == "office:clock")
            .unwrap();
        assert_eq!(session.state, FiberState::Active);

        // The row goes, and its session with it: `close` with `done`.
        within("the row unloading", row.dispose()).await.unwrap();
        eventually(|| closed.lock().unwrap().clone(), "far.close").await;
        assert_eq!(
            closed.lock().unwrap().clone().unwrap(),
            json!([{ "kind": "done" }])
        );
    }
}
