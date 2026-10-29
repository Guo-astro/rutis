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
use crate::session::CallerRow;
use rutis::host::CtxHost;
use rutis::host::Key;

/// One object a row's service slot holds, as a host service: calls go to
/// that object in the runtime. Async methods return a future.
pub struct RowService {
    process: Arc<Process>,
    handle: String,
    methods: Value,
    provider: Option<String>,
    owner: Option<CallerRow>,
    /// The service's name, for the site of the errors its methods throw.
    service: Option<String>,
}

impl RowService {
    pub fn new(process: Arc<Process>, handle: String, methods: Value) -> Self {
        Self {
            process,
            handle,
            methods,
            provider: None,
            owner: None,
            service: None,
        }
    }

    /// The name it is provided under: an error a method throws records
    /// the row and `service.method` it came from (`Site`, #677).
    pub fn named(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// Where an error of `method` comes from, when the row and the service
    /// are known.
    fn site(&self, method: &str) -> Option<crate::session::Site> {
        let row = self.owner.as_ref()?.id();
        let service = self.service.as_deref()?;
        Some(crate::session::Site::call(row, service, method))
    }

    /// Who provides it, for messages ("row storage"; see
    /// [`HostDispatch::provider`]).
    pub fn provided_by(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// The row providing it: a plugin definition it returns belongs to that
    /// row ([`crate::runtime::returned`]).
    pub fn owned_by(mut self, row: Option<CallerRow>) -> Self {
        self.owner = row;
        self
    }

    /// The runtime serving it.
    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    /// The row providing it, when it is known.
    pub fn owner(&self) -> Option<&CallerRow> {
        self.owner.as_ref()
    }
}

impl RowService {
    /// Record a call of `method` on the process until the guard is dropped
    /// ([`Process::calls`]): from the calling row (none outside rows), to
    /// the row providing it.
    fn begin(&self, method: &str) -> crate::runtime::calls::Ongoing {
        let from = crate::session::caller_row().map(|row| row.id().to_owned());
        let to = self.owner.as_ref().map(|row| row.id().to_owned());
        let service = self.service.as_deref().unwrap_or(&self.handle);
        self.process.begin_call(from, to, service, method)
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
                let ongoing = self.begin(method);
                let connection = target.clone();
                let site = self.site(method);
                let process = self.process.clone();
                let (handle, method) = (self.handle.clone(), method.to_owned());
                let call = async move {
                    let _ongoing = ongoing;
                    let settled = async {
                        crate::runtime::rpc::settle(
                            connection.invoke_async(&handle, &method, args).await?,
                        )
                        .await
                    };
                    settled
                        .await
                        .map_err(|error| gone(&process, error))
                        .map_err(|error| match &site {
                            Some(site) => error.with_site(site),
                            None => error,
                        })
                };
                Ok(match source {
                    Some(source) => RpcValue::future(target.forward_async(&source, call)),
                    None => RpcValue::future(call),
                })
            }
            Some(_) => {
                let _ongoing = self.begin(method);
                // From which row to which: a wait cycle it closes is
                // written with their names.
                let rows = crate::session::caller_row()
                    .zip(self.owner.as_ref())
                    .map(|(from, to)| (from.id().to_owned(), to.id().to_owned()));
                let reply = crate::session::rpc::waits::with_rows(rows, || match source {
                    Some(source) => {
                        target.forward(&source, || target.invoke(&self.handle, method, args))
                    }
                    None => target.invoke(&self.handle, method, args),
                });
                reply
                    .map_err(|error| gone(&self.process, error))
                    .map_err(|error| match self.site(method) {
                        Some(site) => error.with_site(&site),
                        None => error,
                    })
            }
            None => {
                let methods: Vec<&str> = self
                    .methods
                    .as_object()
                    .map(|methods| methods.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                let service = self.service.as_deref().unwrap_or(&self.handle);
                Err(crate::runtime::Error::no_method(service, method, methods))
            }
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

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// A call in progress when the runtime went away: `runtime py exited
/// during the call` (code `peer-gone`, data `{runtime, reason}`); another
/// error as it is.
fn gone(process: &Process, error: crate::runtime::Error) -> crate::runtime::Error {
    let crate::runtime::Error::Transport(reason) = &error else {
        return error;
    };
    let Some(runtime) = process.runtime_name() else {
        return error;
    };
    let how = if process.exit_status().is_some() || reason.starts_with("the process ") {
        "exited"
    } else {
        "disconnected"
    };
    // With a graph, so the call's site is recorded on it: printed, it
    // reads `call to weather.tomorrow failed: runtime py exited …`.
    crate::runtime::Error::binding(
        crate::session::code::PEER_GONE,
        format!("runtime {runtime} {how} during the call"),
        Some(serde_json::json!({ "runtime": runtime, "reason": reason })),
    )
    .graphed()
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
    key: impl Fn(&str) -> Key<dyn HostDispatch>,
) -> Arc<Projection> {
    let projection = Projection::new();
    for (name, methods) in provides {
        let methods = methods.clone();
        projection.service_keyed(name, key(name), move |process, handle| {
            Arc::new(RowService::new(process, handle, methods.clone()))
        });
    }
    projection
}

/// [`row_projection_with`] for the row `provider` names ("row storage"):
/// it also publishes, under `key(name)`, each service the row announces
/// without declaring it, with the methods its current object has. Neither
/// a declared nor an announced service is published when `private` says
/// the row keeps it to a scope of its own (a name its config isolates). A service another provider has already is refused as
/// the later one, naming both ([`Projection::take_refused`]).
pub fn row_projection_of(
    provider: &str,
    provides: &Map<String, Value>,
    key: impl Fn(&str) -> Key<dyn HostDispatch> + Send + 'static,
    private: impl Fn(&str) -> bool + Send + 'static,
) -> Arc<Projection> {
    let projection = Projection::new();
    // A name the row isolates stays in its scope, declared or not.
    for (name, methods) in provides.iter().filter(|(name, _)| !private(name)) {
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
    key: Key<dyn HostDispatch>,
    methods: Arc<Mutex<Value>>,
) {
    let named = provider.to_owned();
    let service = name.to_owned();
    let holder = key.clone();
    let owner = projection.weak();
    projection.service_keyed_occupied(
        name,
        key,
        move |process, handle| {
            let methods = methods.lock().unwrap().clone();
            let row = owner.upgrade().and_then(|projection| projection.owner());
            Arc::new(
                RowService::new(process, handle, methods)
                    .provided_by(named.clone())
                    .owned_by(row)
                    .named(service.clone()),
            )
        },
        provider.to_owned(),
        move |ctx| {
            let holder = ctx.get_at(&holder)?;
            // A row or a peer by its name; anything else (a Rust service,
            // whose type path a plugin author never wrote) is the host's.
            Some(
                holder
                    .provider()
                    .filter(|by| by.starts_with("row ") || by.starts_with("peer "))
                    .unwrap_or_else(|| "the host".to_owned()),
            )
        },
    );
}
