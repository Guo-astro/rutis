//! The framework events rutis-bridge emits (events protocol §8): plugins
//! of every language listen to them; only rutis emits them, in `emit`
//! mode. `rutis/row` is rutis-loader's.

use std::future::Future;
use std::sync::Arc;

use rutis::events::Call;
use rutis::events::{Mode, Scope};
use rutis::host::diagnostics::{ServiceChange, ServiceChanged};
use rutis::host::CtxHost;
use rutis::internal::events::{Dispatch, Payload};
use rutis::internal::CtxInternal;
use rutis::internal::EventKey;
use rutis::{Ctx, Event};
use serde::{Deserialize, Serialize};

use crate::session::host_key;

/// `rutis/service`: a service name got a provider in a scope
/// (`available`), its provider was replaced (`available` again, the new
/// `row`), or it was withdrawn with none left (not `available`, the last
/// provider's `row`). Emitted at the service's scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceEvent {
    pub name: String,
    pub scope: Vec<Scope>,
    pub available: bool,
    pub row: Option<String>,
}

impl Event for ServiceEvent {
    const NAME: &'static str = "rutis/service";
    type Value = ();
}

/// What a runtime is doing, in [`RuntimeEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEventState {
    Running,
    Down,
}

/// `rutis/runtime`: a runtime's mount succeeded (`running`, also after each
/// reconnect), or its session ended or its mount failed (`down`, `reason`
/// saying why). Emitted at the root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub runtime: String,
    pub state: RuntimeEventState,
    pub reason: Option<String>,
}

impl Event for RuntimeEvent {
    const NAME: &'static str = "rutis/runtime";
    type Value = ();
}

fn report(ctx: &Ctx, error: rutis::Error) {
    (ctx.error_sink())(Arc::new(error.into_kind()));
}

/// The name of the host service `key` is ([`host_key`], in an instance or
/// not; an isolated one without its label), if it is one.
fn service_name(key: &rutis::host::TypeKey) -> Option<&str> {
    let name = key.name()?;
    let plain = match key.instance_id() {
        Some(instance) => host_key(name).instance(instance),
        None => host_key(name),
    };
    (&plain.erase() == key).then(|| name.split('\0').next().unwrap_or(name))
}

/// Turns the kernel's [`ServiceChanged`] of every host service, whoever
/// provides it (a Rust plugin, a runtime's row, a node), into
/// `rutis/service` at the provider's scope (events protocol §8). Listens
/// at the root, which hears every scope.
pub(crate) fn service_events(
    changed: Arc<ServiceChanged>,
    call: Call<ServiceChanged>,
) -> impl Future<Output = rutis::Result<Option<()>>> + Send + 'static {
    let dispatch = service_name(&changed.key).map(|name| {
        let event = ServiceEvent {
            name: name.to_owned(),
            scope: changed.chain.clone(),
            available: changed.change == ServiceChange::Provided,
            row: changed.row.clone(),
        };
        let mut dispatch = Dispatch::new(
            ServiceEvent::NAME,
            Mode::Emit,
            Payload::new(Arc::new(event)),
        );
        dispatch.framework = true;
        dispatch
    });
    let ctx = call.ctx().clone();
    async move {
        if let Some(dispatch) = dispatch {
            if let Err(error) = ctx.events().dispatch(&ctx, dispatch).await {
                report(&ctx, error);
            }
        }
        Ok(None)
    }
}

/// Emit `rutis/runtime` at the root of `ctx`'s tree.
pub(crate) fn runtime(ctx: &Ctx, runtime: &str, state: RuntimeEventState, reason: Option<String>) {
    let root = ctx.root_ctx();
    let event = RuntimeEvent {
        runtime: runtime.to_owned(),
        state,
        reason,
    };
    if let Err(error) =
        root.events()
            .emit_framework(&root, &EventKey::<RuntimeEvent>::of(), Arc::new(event))
    {
        report(&root, error);
    }
}
