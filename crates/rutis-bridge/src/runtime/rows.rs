//! Services that rows (plugins loaded one by one into a runtime, see
//! [`Process::load_row_exporting`]) provide to rutis.
//!
//! A row's service is published under [`host_key`]`(name)` as a
//! `dyn HostDispatch`, the same key a Rust-provided host service uses, so a
//! consumer finds it by name whichever side provides it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::runtime::rpc::{Reply, Value as RpcValue};
use crate::runtime::{host_key, HostDispatch, Process, Projection};

/// One object a row's service slot holds, as a host service: calls go to
/// that object in the runtime. Async methods return a future.
pub struct RowService {
    process: Arc<Process>,
    handle: String,
    methods: Value,
    provider: Option<String>,
}

impl RowService {
    pub fn new(process: Arc<Process>, handle: String, methods: Value) -> Self {
        Self {
            process,
            handle,
            methods,
            provider: None,
        }
    }

    /// Who provides it, for messages ("row storage"; see
    /// [`HostDispatch::provider`]).
    pub fn provided_by(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }
}

impl HostDispatch for RowService {
    fn invoke(&self, method: &str, args: RpcValue) -> Reply {
        let target = self.process.connection();
        // A call from another runtime's session carries that session's
        // chain, which must be rebased for this one.
        let source = crate::runtime::rpc::caller().filter(|source| source.tag() != target.tag());
        match self.methods.get(method).and_then(Value::as_str) {
            Some("async") => {
                let connection = target.clone();
                let (handle, method) = (self.handle.clone(), method.to_owned());
                let call = async move {
                    crate::runtime::rpc::settle(
                        connection.invoke_async(&handle, &method, args).await?,
                    )
                    .await
                };
                Ok(match source {
                    Some(source) => RpcValue::future(target.forward_async(&source, call)),
                    None => RpcValue::future(call),
                })
            }
            Some(_) => match source {
                Some(source) => {
                    target.forward(&source, || target.invoke(&self.handle, method, args))
                }
                None => target.invoke(&self.handle, method, args),
            },
            None => Err(crate::runtime::Error::Value(format!(
                "unknown service method {}.{method}",
                self.handle
            ))),
        }
    }

    fn methods(&self) -> Option<Value> {
        Some(self.methods.clone())
    }

    fn origin(&self) -> Option<&str> {
        Some(self.process.connection().tag())
    }

    fn provider(&self) -> Option<String> {
        self.provider.clone()
    }
}

impl Drop for RowService {
    fn drop(&mut self) {
        self.process.release(&self.handle);
    }
}

/// A projection publishing each service in `provides` (`{ name: { method:
/// "sync" | "async" } }`) as a [`RowService`] under [`host_key`]`(name)`.
/// Attach it from the row's own context and pass it to
/// [`Process::load_row_exporting`] as the observer.
pub fn row_projection(provides: &Map<String, Value>) -> Arc<Projection> {
    row_projection_with(provides, host_key)
}

/// [`row_projection`] publishing each service under `key(name)` instead
/// (`host_key_in(name, instance)` for a service inside an instance).
pub fn row_projection_with(
    provides: &Map<String, Value>,
    key: impl Fn(&str) -> rutis::TypeKey,
) -> Arc<Projection> {
    let projection = Projection::new();
    for (name, methods) in provides {
        let methods = methods.clone();
        projection.service_keyed::<dyn HostDispatch>(name, key(name), move |process, handle| {
            Arc::new(RowService::new(process, handle, methods.clone()))
        });
    }
    projection
}

/// [`row_projection_with`] for the row `provider` names ("row storage"):
/// it also publishes, under `key(name)`, each service the row announces
/// without declaring it, with the methods its current object has, except
/// those `private` says the row keeps to a scope of its own (a name its
/// config isolates). A service another provider has already is refused as
/// the later one, naming both ([`Projection::take_refused`]).
pub fn row_projection_of(
    provider: &str,
    provides: &Map<String, Value>,
    key: impl Fn(&str) -> rutis::TypeKey + Send + 'static,
    private: impl Fn(&str) -> bool + Send + 'static,
) -> Arc<Projection> {
    let projection = Projection::new();
    for (name, methods) in provides {
        let methods = Arc::new(Mutex::new(methods.clone()));
        row_slot(&projection, provider, name, key(name), methods);
    }
    // The announced ones: their methods follow what each new object has.
    let mut announced: HashMap<String, Arc<Mutex<Value>>> = HashMap::new();
    let provider = provider.to_owned();
    projection.on_announced(move |projection, name, methods| {
        if let Some(current) = announced.get(name) {
            *current.lock().unwrap() = methods.clone();
            return true;
        }
        // Declared: the declaration says what it offers.
        if private(name) || projection.has_service(name) {
            return false;
        }
        let current = Arc::new(Mutex::new(methods.clone()));
        announced.insert(name.to_owned(), current.clone());
        row_slot(projection, &provider, name, key(name), current);
        true
    });
    projection
}

/// Declare the slot of the row service `name`, with the methods in
/// `methods` when each object is published.
fn row_slot(
    projection: &Projection,
    provider: &str,
    name: &str,
    key: rutis::TypeKey,
    methods: Arc<Mutex<Value>>,
) {
    let named = provider.to_owned();
    let holder = key.clone();
    projection.service_keyed_occupied::<dyn HostDispatch>(
        name,
        key,
        move |process, handle| {
            let methods = methods.lock().unwrap().clone();
            Arc::new(RowService::new(process, handle, methods).provided_by(named.clone()))
        },
        provider.to_owned(),
        move |ctx| {
            let holder = ctx.get_as::<dyn HostDispatch>(holder.clone())?;
            Some(
                holder
                    .provider()
                    .unwrap_or_else(|| "the application".to_owned()),
            )
        },
    );
}
