//! The framework events rutis-bridge emits (events protocol §8): plugins
//! of every language listen to them; only rutis emits them, in `emit`
//! mode. `rutis/row` is rutis-loader's.

use std::sync::Arc;

use rutis::events::Scope;
use rutis::{Ctx, Event, EventKey};
use serde::{Deserialize, Serialize};

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

fn report(ctx: &Ctx, error: rutis::CordisError) {
    (ctx.error_sink())(Arc::new(error));
}

/// Emit `rutis/service` from `ctx` (the providing row's context).
pub(crate) fn service(ctx: &Ctx, name: &str, available: bool) {
    let event = ServiceEvent {
        name: name.to_owned(),
        scope: ctx.scope(),
        available,
        row: ctx.row(),
    };
    if let Err(error) =
        ctx.events()
            .emit_framework(ctx, &EventKey::<ServiceEvent>::of(), Arc::new(event))
    {
        report(ctx, error);
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
