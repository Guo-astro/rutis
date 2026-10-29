//! A service two sessions away: b provides `clock`; b and c each have a
//! session with a, so the clock is seen at a and, through a's session with
//! c, at c (`docs/design-remote-session-2026-10-28.md` §2.6: no export or
//! import, services seen on both sides). Calls from c reach b; a callback c
//! passes is called back by b during c's synchronous call, and that
//! callback calls the clock again: a re-entrant chain c → a → b → a → c →
//! a → b. A withdrawal at b reaches c.

use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect};
use rutis::internal::CtxInternal;
use rutis::Ctx;
use rutis_bridge::channel::PeerId;
use rutis_bridge::session::{host_key, Error, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value};
use rutis_bridge::{LocalId, Session};
use rutis_test_support::eventually;
use serde_json::{json, Value as Json};

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// b's clock: `now` counts, `each` calls back for every day, `later` is async.
struct Clock(std::sync::atomic::AtomicU64);
impl HostDispatch for Clock {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        match method {
            "now" => Ok(json!(self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)).into()),
            "each" => {
                let callback = args.list()?.remove(0).reference()?;
                let days = ["mon", "tue"]
                    .into_iter()
                    .map(|day| {
                        callback
                            .call(Value::List(vec![Value::Data(json!(day))]))?
                            .json()
                    })
                    .collect::<Result<Vec<Json>, Error>>()?;
                Ok(Value::Data(Json::Array(days)))
            }
            "later" => Ok(Value::future(async { Ok(json!("later").into()) })),
            _ => Err(Error::Value(method.into())),
        }
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "now": "sync", "each": "sync", "later": "async" }))
    }
}

/// A kernel greeting as `name`.
fn kernel(name: &str) -> Ctx {
    let root = Ctx::root().unwrap();
    root.provide(LocalId(id(name))).unwrap();
    root
}

/// Provides the clock (a plugin's service is shared with far ends).
struct ProvidesClock(Mutex<Option<Clock>>);

impl DynPlugin for ProvidesClock {
    fn name(&self) -> &str {
        "clock"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            let clock = self.0.lock().unwrap().take().expect("applied once");
            ctx.provide_key::<dyn HostDispatch>(host_key("clock"), Arc::new(clock))?;
            Ok(Effect::Done)
        })
    }
}

/// Mount a session between `a` and `b` on a memory pair.
fn connect(a: &Ctx, b: &Ctx) {
    let (near, far) = rutis_bridge::memory::pair();
    a.plugin(Session::new(near));
    b.plugin(Session::new(far));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_two_sessions_away_keeps_calls_callbacks_and_withdrawals() {
    let (a, b, c) = (kernel("a"), kernel("b"), kernel("c"));
    // b owns the clock; c uses it, two sessions away.
    let provided = b.plugin(ProvidesClock(Mutex::new(Some(Clock(Default::default())))));
    connect(&a, &b);
    connect(&a, &c);

    let clock = eventually(
        || c.get_at::<dyn HostDispatch>(&host_key("clock")),
        "the clock at c",
    )
    .await;
    assert_eq!(
        clock.methods(),
        Some(json!({ "now": "sync", "each": "sync", "later": "async" }))
    );

    let calls = tokio::task::spawn_blocking({
        let clock = clock.clone();
        move || -> Result<(Json, Json), Error> {
            let first = clock.invoke("now", Value::List(vec![]))?.json()?;
            // b calls this back while c waits; it calls the clock again.
            let again = clock.clone();
            let callback = Value::callback(move |args| {
                let [day]: [String; 1] = rutis_bridge::session::decode_value(args)?;
                let now = again.invoke("now", Value::List(vec![]))?.json()?;
                Ok(json!(format!("{}@{now}", day.to_uppercase())).into())
            });
            let days = clock.invoke("each", Value::List(vec![callback]))?.json()?;
            Ok((first, days))
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(calls.0, json!(0));
    assert_eq!(calls.1, json!(["MON@1", "TUE@2"]));
    let later = settle(clock.invoke("later", Value::List(vec![])).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("later"));
    drop(clock);

    // Withdrawn at b: gone at a, then at c.
    provided.dispose().await.unwrap();
    eventually(
        || {
            a.get_at::<dyn HostDispatch>(&host_key("clock"))
                .is_none()
                .then_some(())
        },
        "the withdrawal at a",
    )
    .await;
    eventually(
        || {
            c.get_at::<dyn HostDispatch>(&host_key("clock"))
                .is_none()
                .then_some(())
        },
        "the withdrawal at c",
    )
    .await;
}
