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
