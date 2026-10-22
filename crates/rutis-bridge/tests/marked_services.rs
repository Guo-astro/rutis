//! `#[rutis::service]` types are served by name once provided: their
//! dispatch decodes arguments, encodes results, throws errors with a code
//! and data, and goes with the native service (#510).

use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, InstanceId, TypeKey};
use rutis_bridge::exports::claimed_in;
use rutis_bridge::session::{host_key, host_key_in, settle, Error, HostDispatch, Value};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Entry {
    title: String,
    day: u32,
}

#[derive(Debug, Serialize, thiserror::Error)]
pub enum CalendarError {
    #[error("no entry on day {day}")]
    Missing { day: u32 },
    #[error("the calendar is full")]
    Full,
}

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tagged {
    #[error("too late by {by}")]
    TooLate { by: u64 },
}

#[derive(Default)]
pub struct Calendar {
    entries: Mutex<Vec<Entry>>,
}

#[rutis::service]
impl Calendar {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn add(&self, entry: Entry) -> Result<usize, CalendarError> {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= 2 {
            return Err(CalendarError::Full);
        }
        entries.push(entry);
        Ok(entries.len())
    }

    pub fn on_day(&self, day: u32) -> Result<Entry, CalendarError> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.day == day)
            .cloned()
            .ok_or(CalendarError::Missing { day })
    }

    pub fn titles(&self, prefix: &str) -> Vec<String> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|entry| format!("{prefix}{}", entry.title))
            .collect()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    pub fn late(&self) -> Result<(), Tagged> {
        Err(Tagged::TooLate { by: 3 })
    }

    pub fn text(&self) -> Result<(), String> {
        Err("plain".into())
    }

    pub fn io(&self) -> std::io::Result<()> {
        Err(std::io::Error::other("disk"))
    }

    pub fn each(
        &self,
        days: Vec<u32>,
        callback: Arc<dyn Fn(u32) -> BoxFuture<'static, Result<String, CordisError>> + Send + Sync>,
    ) -> BoxFuture<'_, Result<Vec<String>, CordisError>> {
        Box::pin(async move {
            let mut out = Vec::new();
            for day in days {
                out.push(callback(day).await?);
            }
            Ok(out)
        })
    }

    pub async fn caller(&self, ctx: &Ctx) -> bool {
        let _ = ctx;
        true
    }

    #[service(skip)]
    pub fn reset(&mut self) {}
}

/// A second marked block of the same type adds its methods.
#[rutis::service]
impl Calendar {
    pub fn count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

pub struct Notes;

#[rutis::service(name = "notebook")]
impl Notes {
    pub fn hello(&self) -> &'static str {
        "hi"
    }
}

/// Not marked: not served.
pub struct Hidden;

async fn call(service: &Arc<dyn HostDispatch>, method: &str, args: Json) -> Result<Json, Error> {
    let args = match args {
        Json::Array(args) => Value::List(args.into_iter().map(Value::Data).collect()),
        _ => panic!("args are a list"),
    };
    let value = settle(service.invoke(method, args)?).await?;
    Ok(match value {
        Value::Undefined => Json::Null,
        Value::Data(json) => json,
        other => panic!("not data: {other:?}"),
    })
}

fn served(ctx: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    ctx.get_as::<dyn HostDispatch>(host_key(name))
}

#[tokio::test]
async fn a_provided_marked_type_is_served_by_name() {
    let root = Ctx::root().unwrap();
    assert!(served(&root, "calendar").is_none());
    let provided = root.provide(Calendar::new()).unwrap();
    let calendar = served(&root, "calendar").expect("served as calendar");

    let methods = calendar.methods().unwrap();
    for method in [
        "add", "onDay", "titles", "clear", "late", "each", "caller", "count",
    ] {
        assert_eq!(methods[method], "async", "{method}");
    }
    assert!(methods.get("reset").is_none());
    assert!(methods.get("new").is_none());

    let entry = json!({ "title": "Standup", "day": 1 });
    assert_eq!(
        call(&calendar, "add", json!([entry])).await.unwrap(),
        json!(1)
    );
    assert_eq!(call(&calendar, "onDay", json!([1])).await.unwrap(), entry);
    assert_eq!(
        call(&calendar, "titles", json!(["- "])).await.unwrap(),
        json!(["- Standup"])
    );
    assert_eq!(call(&calendar, "count", json!([])).await.unwrap(), json!(1));
    assert_eq!(
        call(&calendar, "clear", json!([])).await.unwrap(),
        Json::Null
    );

    // Errors are thrown with their type's name, a code and data.
    let error = call(&calendar, "onDay", json!([9])).await.unwrap_err();
    assert_eq!(error.code(), Some("Missing"));
    assert_eq!(error.data(), Some(json!({ "Missing": { "day": 9 } })));
    assert_eq!(error.to_string(), "CalendarError: no entry on day 9");
    let error = call(&calendar, "late", json!([])).await.unwrap_err();
    assert_eq!(error.code(), Some("too_late"));
    let error = call(&calendar, "text", json!([])).await.unwrap_err();
    assert_eq!(
        (error.code(), error.to_string().as_str()),
        (Some("Error"), "Error: plain")
    );
    let error = call(&calendar, "io", json!([])).await.unwrap_err();
    assert_eq!(error.code(), Some("Error"));
    call(&calendar, "add", json!([entry])).await.unwrap();
    call(&calendar, "add", json!([entry])).await.unwrap();
    let error = call(&calendar, "add", json!([entry])).await.unwrap_err();
    assert_eq!(
        (error.code(), error.data()),
        (Some("Full"), Some(json!("Full")))
    );

    // A wrong call is refused as one.
    assert!(call(&calendar, "add", json!([1])).await.is_err());
    assert!(call(&calendar, "titles", json!(["a", "b"])).await.is_err());
    assert!(call(&calendar, "nothing", json!([])).await.is_err());
    // `&Ctx` needs a calling row.
    let error = call(&calendar, "caller", json!([])).await.unwrap_err();
    assert!(error.to_string().contains("calling row"), "{error}");

    // Named in the marker.
    root.provide(Notes).unwrap();
    assert!(served(&root, "notes").is_none());
    let notes = served(&root, "notebook").unwrap();
    assert_eq!(call(&notes, "hello", json!([])).await.unwrap(), json!("hi"));

    // Unmarked types are not served.
    root.provide(Hidden).unwrap();
    assert!(served(&root, "hidden").is_none());

    // It goes with the native service.
    provided.dispose().await.unwrap();
    assert!(served(&root, "calendar").is_none());
    assert!(root.get::<Calendar>().is_none());
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_second_service_of_the_same_name_is_refused() {
    let root = Ctx::root().unwrap();
    root.provide(Notes).unwrap();
    let other = Ctx::root().unwrap();
    // Another root is another registry: no conflict there.
    other.provide(Notes).unwrap();

    pub struct Clash;
    #[rutis::service(name = "notebook")]
    impl Clash {
        pub fn hello(&self) -> &'static str {
            "clash"
        }
    }
    let error = root.provide(Clash).unwrap_err();
    assert!(
        error.to_string().contains("`notebook`") && error.to_string().contains("Notes"),
        "{error}"
    );
    // Refused before anything is registered: the native service is not
    // provided either.
    assert!(root.get::<Clash>().is_none());
    // A keyed binding of a marked type is a second value of it, not served
    // by name (it would clash with the first).
    root.provide_as(TypeKey::keyed::<Notes>("second"), Arc::new(Notes))
        .unwrap();
    root.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}

#[tokio::test]
async fn provided_in_an_instance_it_is_that_instances() {
    let root = Ctx::root().unwrap();
    let (instance_tx, instance_rx) = std::sync::mpsc::channel::<(InstanceId, Ctx)>();
    struct Instance(std::sync::mpsc::Sender<(InstanceId, Ctx)>);
    impl rutis::Plugin for Instance {
        fn name(&self) -> &str {
            "instance"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<rutis::Effect, CordisError>> {
            Box::pin(async move {
                self.0.send((ctx.instance(), ctx.clone())).unwrap();
                Ok(rutis::Effect::Done)
            })
        }
    }
    root.plugin(Instance(instance_tx)).await.unwrap();
    let (instance, ctx) = instance_rx.recv().unwrap();
    let provided = ctx
        .provide_as(TypeKey::instance::<Notes>(instance), Arc::new(Notes))
        .unwrap();
    assert!(served(&ctx, "notebook").is_none());
    assert!(ctx
        .get_as::<dyn HostDispatch>(host_key_in("notebook", instance))
        .is_some());
    assert!(claimed_in(instance, "notebook"));
    provided.dispose().await.unwrap();
    assert!(!claimed_in(instance, "notebook"));
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unmarked_service_is_named_in_the_diagnostic() {
    let root = Ctx::root().unwrap();
    root.provide(Hidden).unwrap();
    let hint = rutis_bridge::exports::unexported(&root, "hidden").unwrap();
    assert!(
        hint.contains("Hidden") && hint.contains("#[rutis::service]"),
        "{hint}"
    );
    assert!(rutis_bridge::exports::unexported(&root, "other").is_none());
    root.shutdown().await.unwrap();
}

/// A marked service provided in this scope but not served (its check fails)
/// is not described as out of scope.
#[tokio::test]
async fn a_service_provided_here_is_not_described_as_out_of_scope() {
    let root = Ctx::root().unwrap();
    root.provide_as_with_check(
        TypeKey::of::<Calendar>(),
        Arc::new(Calendar::default()),
        || false,
    )
    .unwrap();
    let hint = rutis_bridge::exports::unexported(&root, "calendar").unwrap();
    assert!(hint.contains("provided here but not served"), "{hint}");
    assert!(!hint.contains("not visible here"), "{hint}");
    root.shutdown().await.unwrap();
}

/// A marked service provided in another scope says where it is, and in that
/// scope it is not described as out of scope either.
#[tokio::test]
async fn a_service_in_another_scope_names_the_scope() {
    let root = Ctx::root().unwrap();
    let (scope_tx, scope_rx) = std::sync::mpsc::channel::<Ctx>();
    struct Scoped(std::sync::mpsc::Sender<Ctx>);
    impl rutis::Plugin for Scoped {
        fn name(&self) -> &str {
            "scoped"
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<rutis::Effect, CordisError>> {
            Box::pin(async move {
                ctx.mark_scope("chat")?;
                self.0.send(ctx.clone()).unwrap();
                Ok(rutis::Effect::Done)
            })
        }
    }
    root.plugin(Scoped(scope_tx)).await.unwrap();
    let scope = scope_rx.recv().unwrap();
    let isolated = scope.isolate(TypeKey::of::<Calendar>(), "chat");
    let provided = isolated
        .provide_as_with_check(
            TypeKey::of::<Calendar>(),
            Arc::new(Calendar::default()),
            || false,
        )
        .unwrap();
    let hint = rutis_bridge::exports::unexported(&root, "calendar").unwrap();
    assert!(
        hint.contains("scope `chat`") && hint.contains("not visible here"),
        "{hint}"
    );
    let here = rutis_bridge::exports::unexported(&isolated, "calendar").unwrap();
    assert!(
        here.contains("provided here but not served") || here.contains("not exported"),
        "{here}"
    );
    provided.dispose().await.unwrap();
    root.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_renamed_service_says_the_name_it_is_exported_as() {
    let root = Ctx::root().unwrap();
    root.provide(Notes).unwrap();
    let hint = rutis_bridge::exports::unexported(&root, "notes").unwrap();
    assert!(hint.contains("notebook"), "{hint}");
    assert!(!hint.contains("#[rutis::service]"), "{hint}");
    root.shutdown().await.unwrap();
}

/// A plugin gated on `keys`, as a row of another language is gated on a
/// name.
struct Gated(Vec<TypeKey>);

impl rutis::Plugin for Gated {
    fn name(&self) -> &str {
        "gated"
    }
    fn injects(&self) -> &[TypeKey] {
        &self.0
    }
    fn apply<'a>(&'a self, _: &'a Ctx) -> BoxFuture<'a, Result<rutis::Effect, CordisError>> {
        Box::pin(async { Ok(rutis::Effect::Done) })
    }
}

async fn reach(view: &rutis::FiberView, state: rutis::FiberState) {
    for _ in 0..1000 {
        if view.state().state == state {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{state:?} not reached: {:?}", view.state());
}

/// Provided with a `check()` predicate, a marked service's entry by name is
/// gated by it as the native binding is: its users stop while it fails and
/// start again when it passes.
#[tokio::test]
async fn the_check_gates_the_name_as_it_gates_the_native_service() {
    use rutis::FiberState::{Active, Pending};
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = Ctx::root().unwrap();
    let up = Arc::new(AtomicBool::new(true));
    let flag = up.clone();
    root.provide_as_with_check(TypeKey::of::<Notes>(), Arc::new(Notes), move || {
        flag.load(Ordering::SeqCst)
    })
    .unwrap();
    let by_name = root.plugin(Gated(vec![host_key("notebook")]));
    let native = root.plugin(Gated(vec![TypeKey::of::<Notes>()]));
    reach(&by_name, Active).await;
    reach(&native, Active).await;

    up.store(false, Ordering::SeqCst);
    root.refresh();
    reach(&by_name, Pending).await;
    reach(&native, Pending).await;

    up.store(true, Ordering::SeqCst);
    root.refresh();
    reach(&by_name, Active).await;
    reach(&native, Active).await;
    root.shutdown().await.unwrap();
}

/// A greeting, for the tests below.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Greeter {
    text: String,
}

#[rutis::service]
impl Greeter {
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// Named lifetimes and `Self` in parameter types.
    pub fn joined<'a>(&self, other: std::borrow::Cow<'a, str>, more: Vec<Self>) -> String {
        let mut text = format!("{}{other}", self.text);
        for greeter in more {
            text.push_str(&greeter.text);
        }
        text
    }

    /// Not in this build, so not served (it would not compile).
    #[cfg(any())]
    pub fn never(&self) -> u32 {
        missing_function()
    }

    /// One name, one method per build.
    #[cfg(unix)]
    pub fn platform(&self) -> &'static str {
        "unix"
    }
    #[cfg(not(unix))]
    pub fn platform(&self) -> &'static str {
        "other"
    }

    #[cfg_attr(any(), cfg(any()))]
    pub fn kept(&self) -> bool {
        true
    }

    /// The explicit form of `&self`.
    #[allow(clippy::needless_arbitrary_self_type)]
    pub fn explicit(self: &Self) -> String {
        self.text.clone()
    }
}

/// A marked block not in this build: nothing of it is served, and its
/// generated entry goes with it (it names methods that would not
/// compile; #541).
#[rutis::service]
#[cfg(any())]
impl Greeter {
    pub fn gone(&self) -> u32 {
        missing_function()
    }
}

/// The same, removed by a `cfg_attr`.
#[rutis::service]
#[cfg_attr(not(any()), cfg(any()))]
impl Greeter {
    pub fn also_gone(&self) -> u32 {
        missing_function()
    }
}

fn greeter(text: &str) -> Greeter {
    Greeter { text: text.into() }
}

/// Parameter types with named lifetimes or `Self`, methods and blocks
/// under `#[cfg]`, and a `self: &Self` receiver: served as the build has
/// them.
#[tokio::test]
async fn cfg_lifetimes_and_self_are_as_the_build_has_them() {
    let root = Ctx::root().unwrap();
    root.provide(greeter("a")).unwrap();
    let service = served(&root, "greeter").unwrap();
    let methods = service.methods().unwrap();
    assert!(methods.get("never").is_none(), "{methods}");
    assert!(methods.get("gone").is_none(), "{methods}");
    assert!(methods.get("alsoGone").is_none(), "{methods}");
    assert_eq!(methods["explicit"], "async");
    assert_eq!(
        call(&service, "explicit", json!([])).await.unwrap(),
        json!("a")
    );
    assert_eq!(methods["platform"], "async");
    assert_eq!(methods["kept"], "async");
    assert_eq!(
        call(&service, "joined", json!(["b", [{ "text": "c" }]]))
            .await
            .unwrap(),
        json!("abc")
    );
    let platform = if cfg!(unix) { "unix" } else { "other" };
    assert_eq!(
        call(&service, "platform", json!([])).await.unwrap(),
        json!(platform)
    );
    assert!(call(&service, "never", json!([])).await.is_err());
    root.shutdown().await.unwrap();
}

/// Provided in two isolated scopes, a marked type is served in each, by
/// name in the same scope.
#[tokio::test]
async fn each_isolated_scope_serves_its_own() {
    let root = Ctx::root().unwrap();
    let native = TypeKey::of::<Greeter>();
    let name = host_key("greeter");
    let a = root.isolate(native.clone(), "a");
    let b = root.isolate(native.clone(), "b");
    a.provide(greeter("in a")).unwrap();
    b.provide(greeter("in b")).unwrap();
    for (ctx, text) in [(&a, "in a"), (&b, "in b")] {
        let readers = ctx.isolate(name.clone(), if text == "in a" { "a" } else { "b" });
        let service = readers.get_as::<dyn HostDispatch>(name.clone()).unwrap();
        assert_eq!(
            call(&service, "text", json!([])).await.unwrap(),
            json!(text)
        );
    }
    assert!(served(&root, "greeter").is_none());
    root.shutdown().await.unwrap();
}

/// A provide refused for its native slot serves nothing by name.
#[tokio::test]
async fn a_provide_refused_for_its_native_slot_serves_nothing() {
    let root = Ctx::root().unwrap();
    // A mutable binding is not served by name, but holds the native slot.
    let (_disposer, _writer) = root
        .provide_mut_as(TypeKey::of::<Greeter>(), Arc::new(greeter("first")))
        .unwrap();
    let error = root.provide(greeter("refused")).unwrap_err();
    assert!(matches!(error, CordisError::ServiceExists(_)), "{error}");
    assert!(served(&root, "greeter").is_none());
    root.shutdown().await.unwrap();
}

/// Calls by name read the service through the strict read path: read
/// interceptors deny or replace it, as for Rust readers.
#[tokio::test]
async fn calls_by_name_go_through_read_interceptors() {
    use rutis::ServiceIntercept;
    use std::sync::atomic::{AtomicU8, Ordering};
    let root = Ctx::root().unwrap();
    root.provide(greeter("native")).unwrap();
    let service = served(&root, "greeter").unwrap();
    let policy = Arc::new(AtomicU8::new(0));
    let read = policy.clone();
    root.intercept_require_as::<Greeter>(TypeKey::of::<Greeter>(), move |_| {
        match read.load(Ordering::SeqCst) {
            0 => ServiceIntercept::Continue,
            1 => ServiceIntercept::Replace(Arc::new(greeter("replaced"))),
            _ => ServiceIntercept::Deny,
        }
    })
    .unwrap();
    assert_eq!(
        call(&service, "text", json!([])).await.unwrap(),
        json!("native")
    );
    policy.store(1, Ordering::SeqCst);
    assert_eq!(
        call(&service, "text", json!([])).await.unwrap(),
        json!("replaced")
    );
    policy.store(2, Ordering::SeqCst);
    let error = call(&service, "text", json!([])).await.unwrap_err();
    assert!(error.to_string().contains("not available"), "{error}");
    root.shutdown().await.unwrap();
}
