# Rutis Development Handbook

This handbook covers plugin implementation, service calls, resource management, and runtime control. For application structure, see the [Application Development Guide](development-guide.md).

## Quick Start

Add dependencies to a Rust project:

```toml
[dependencies]
rutis = "0.17"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
serde = { version = "1", features = ["derive"] }
```

The following example demonstrates service registration and dependency declaration with one provider and one consumer. Backend provides a service; Indexer declares and uses it.

```rust
use std::sync::Arc;
use rutis::host::*;
use rutis::{Ctx, Plugin};

struct Backend(String);
struct BackendPlugin(String);

impl Plugin for BackendPlugin {
    type Deps = ();

    fn name(&self) -> &str { "backend" }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide(Backend(self.0.clone()))?;
        Ok(())
    }
}

struct IndexerPlugin;

impl Plugin for IndexerPlugin {
    type Deps = Arc<Backend>;

    fn name(&self) -> &str { "indexer" }

    async fn apply(&self, _: &Ctx, backend: Arc<Backend>) -> rutis::Result {
        println!("Indexer connected to {}", backend.0);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let started = async {
        let backend = root.plugin(BackendPlugin("local".into()));
        (&backend).await?;

        let indexer = root.plugin(IndexerPlugin);
        (&indexer).await
    }.await;

    let closed = root.shutdown().await;
    started?;
    closed?;
    Ok(())
}
```

Running it prints `Indexer connected to local`. Indexer's `type Deps = Arc<Backend>` is both its dependency declaration and the argument its `apply` receives. The example initializes the backend before loading Indexer. A real application may register the consumer first; it remains `Pending` until its dependency is ready. Host methods such as `Ctx::root()` and `shutdown()` come from `use rutis::host::*;`.

Here, `(&view).await` waits for the plugin to process operations already queued for it. To determine whether a running service is usable, observe whether its provider plugin has entered `Active`; see [Startup and Shutdown](#startup-and-shutdown).

The repository's [complete Indexer example](../crates/rutis/examples/development_workflow.rs) adds a bounded queue, background task, and config update:

```bash
cargo run -p rutis --example development_workflow
```

## Implementing Plugins

### Initialization

Organize `apply` in this order:

1. Obtain services needed for this load: required ones are written in `type Deps` and passed in as an argument.
2. Create resources and register their cleanup with `ctx.effect`.
3. Provide services, register listeners, or create child plugins.
4. Return `Ok(())` to complete initialization.

Once a plugin is `Active`, its services are available to external consumers. Run continuing work in background tasks and use cleanups to stop them.

`type Deps` declares service dependencies (`()` when there are none); `name()` is used for logs and diagnostics and defaults to the last segment of the type name; `validate()` validates config owned by the plugin, before `apply`. Plugin implementations must satisfy `Send + Sync + 'static`.

### Choosing a Creation Method

| Creation method | Use when | Reload behavior |
|---|---|---|
| `ctx.plugin(plugin)` | Config is owned by the plugin object | Run `apply` again on the same object |
| `ctx.plugin_from(build, config)` | Config needs to change at runtime | Call `build` to create a new plugin object from current config |
| `ctx.plugin_with(factory, config)` | A factory lists the dependencies at run time (`host::PluginFactory`) | Create a new plugin object from current config |

A plugin declares its dependencies in `type Deps`; `plugin_from` takes them from the `Deps` of the plugin type `build` returns; a factory declares them in `PluginFactory::injects()`; a plugin that depends on services by name implements `host::DynPlugin` and declares them in `DynPlugin::injects()`. Declarations are fixed at registration and remain fixed for the lifetime of that fiber.

Put resource creation in `apply`. Each reload then uses current dependencies, creates needed resources, and registers fresh cleanup functions.

## Service Keys and Scopes

### Defining Service Keys

Use the same key for registration, dependency declaration, and lookup:

| Service form | Registration | Dependency declaration | Lookup |
|---|---|---|---|
| Concrete type | `provide(value)` | `type Deps = Arc<T>` | `apply`'s argument; optional read `get::<T>()` |
| Trait interface | `provide_as::<dyn T>(value)` | `type Deps = Arc<dyn T>` | `apply`'s argument; optional read `get::<dyn T>()` |
| Named service | `provide_at(key, value)` | `key.erase()` in `DynPlugin::injects()` | `require_at(&key)`; optional read `get_at(&key)` |

`provide_as` and `provide_at` take `Arc<T>`. A trait service is registered under the trait object type; for example, what `provide_as::<dyn Store>(Arc::new(store))` provides is the service an `Arc<dyn Store>` dependency gets. The keyed methods `provide_at`, `get_at` and `require_at` belong to the host layer and are available after `use rutis::host::*;`.

Distinguish multiple services of the same type by name; the key, `host::Key<T>`, carries the service's type:

```rust
use rutis::host::Key;

struct Backend;

const PRIMARY_BACKEND: Key<Backend> = Key::named("primary");
```

Put such keys in a public interface module so providers and consumers share them. For names generated at runtime, use `Key::dynamic`.

### Declaring Dependencies

Write the services a plugin needs to run in `type Deps`, as a tuple when there are several. The framework waits until each service exists, its provider enters `Active`, and any optional health check passes before calling `apply`.

`get` / `get_at` finds currently visible services and returns `Option<Arc<T>>`; it does not wait for the service and creates no reload relationship, and diagnostics record it as an undeclared access. Automatic reload relationships are established by the dependency declaration (`Deps`, or `DynPlugin::injects()`), so declare every required service there. An `Option<Arc<T>>` in `Deps` is a snapshot taken at load: it does not wait, and its arrival or departure does not reload the plugin.

Services with additional health conditions (a synchronous predicate; when the condition changes, dependencies are rechecked and consumer states adjusted) are provided through `provide_checked` and `refresh` in `rutis::internal`, which only rutis's own crates use, with no stability promise.

### Choosing a Scope

A service provided by its type is shared within root. One active service binding may exist for a given key and scope; duplicate registration returns `ServiceExists`.

`isolate` selects a scope for a particular service key. This snippet creates two independent lookup scopes for the same key:

```rust,ignore
let scope_a = root.isolate(PRIMARY_BACKEND, "a");
let scope_b = root.isolate(PRIMARY_BACKEND, "b");
```

Register providers and consumers in the corresponding contexts to use each binding. Within one root, the same key and label share a scope. Isolate applies to the specified service key; resource ownership follows the original context. For instance-level resource management, see [Instance Subtrees](#instance-subtrees).

## Managing Resources

Plugin authors own the full lifecycle of business resources: initialize through `apply`, and register cleanup through `ctx.effect`. Rutis invokes these implementations according to lifecycle rules. See [Responsibility Boundaries](development-guide.md#responsibility-boundaries).

### Registering Cleanup

The framework registers cleanup for services added with `ctx.provide`, listeners registered through the current `ctx`, and child plugins created with `ctx.plugin`. Register external subscriptions, connections, and tasks through `ctx.effect`.

The following snippet belongs in a plugin's `apply` and demonstrates asynchronous cleanup:

```rust,ignore
ctx.effect(move || async move {
    connection.close().await?;
    Ok(())
})?;
```

The `effect` closure runs immediately; the future it returns is the cleanup and runs at unload. When the plugin is already stopping, `effect` returns `InactiveEffect` and the closure does not run. Register cleanup promptly after acquiring a resource, and release the resource if registration fails. If `apply` fails partway through, the framework runs cleanup already registered.

A `Disposer` handle can release a resource early: call `disposer.dispose().await` and handle its result. Whether the handle is retained or dropped, the owning plugin remains responsible for final cleanup at unload.

### Managing Background Tasks

Background tasks need both a cancellation plan and a way to wait for exit. This shows the basic structure; replace the wait inside the task with the business loop:

```rust,ignore
let stopping = ctx.cancelled();

ctx.effect(move || {
    let task = tokio::spawn(async move {
        stopping.await;
    });
    async move {
        task.await?;
        Ok(())
    }
})?;
```

Each `apply` takes the current generation's `ctx.cancelled()` once and passes it to that generation's tasks; it completes when the plugin starts stopping, before the cleanups. After this signal, a task completes or cancels its current operation as specified, then exits. The cleanup waits for actual exit through the `JoinHandle`. When this cleanup is released early through its `Disposer`, the plugin has not stopped, so the task needs another way to stop.

Business loops typically use `tokio::select!` to wait for both input and cancellation. The service interface defines whether queued requests complete, cancel, or are saved. See the complete [IndexerPlugin example](../crates/rutis/examples/development_workflow.rs).

Synchronous computation, blocking operations, and subprocesses also need their own stop mechanism, such as checking cancellation periodically, closing I/O, or terminating and waiting for a process.

### Ordering Cleanup

Cleanup within one plugin runs in reverse registration order; later-registered resources are released first. (The `Effect` returned by a `host::DynPlugin`'s `apply` is registered last.)

When a consumer must exit before its underlying connection is closed, use this order:

```text
Initialize: create connection and register close → provide service → create dependent child plugin
Cleanup:    close child plugin → remove service and wait for consumers → close connection
```

Acquire objects needed during cleanup in `apply` and capture them. If one cleanup function fails, the framework records the error and continues the remaining cleanup.

After a service is removed from the registry, callers holding an `Arc` can still use it. The implementation should handle calls according to its own running state. For example, the old Indexer in the sample returns an error after its queue closes.

## Using Events

### Event Bus

The event bus manages listener registration, event dispatch, and handler invocation. The sender supplies event data, listeners implement handling logic, and the dispatch API determines execution order, completion conditions, and how results are returned.

Use this mechanism for notifications, request dispatch, or processing shared among multiple plugins. Event types define data and results; the application defines their precise meaning.

### Defining Events and Listeners

Implement `Event` for an event type, specifying the event name and result type; the event and result types implement serde's `Serialize` and `Deserialize` (add `serde = { version = "1", features = ["derive"] }` to the dependencies). A listener is a closure taking the event (`Arc<E>`) and the call (`Call<E>`). The following `DocumentIndexed` event carries a document name:

```rust
use rutis::{Ctx, Event};

#[derive(serde::Serialize, serde::Deserialize)]
struct DocumentIndexed(String);

impl Event for DocumentIndexed {
    const NAME: &'static str = "app/document-indexed";
    type Value = ();
}

fn index(ctx: &Ctx, path: String) -> rutis::Result {
    ctx.on::<DocumentIndexed>(|event, _call| async move {
        println!("indexed: {}", event.0);
        Ok(None)
    })?;
    ctx.emit(DocumentIndexed(path))?;
    Ok(())
}
```

Here, `on::<DocumentIndexed>` registers a listener for the name `DocumentIndexed::NAME`, and `emit` sends an event under it. Both return `Result`; if an operation fails (for example, the context is already closed), registration or delivery does not occur. The compiler infers the closure's parameter types.

Events match by name. `ctx.listen::<E>().name("…")` listens to the same event type under another name, and `ctx.listen::<E>().prefix("…")` to every name under a prefix (`call.name()` is the actual name); an event type without a fixed name overrides `Event::name` so that each value carries its own. Different Rust types under the same name hear the same events, converted through serde, so unrelated event types should use different names. Names starting with `rutis/` are reserved for framework events; plugins can only listen to them.

A listener belongs to the `Ctx` that registered it, and `call.ctx()` in the callback is that same `Ctx`, not the sender's. Resources the callback registers through it therefore belong to the listening plugin, and `ctx.cancelled()` is that plugin's unload signal. Once a plugin starts unloading, its listeners are no longer called, and the unload waits for their running calls to finish. Runnable code is in the [context ownership example](../crates/rutis/examples/listener_ctx_ownership.rs).

### Choosing a Dispatch Mode

A listener is registered once and every dispatch mode calls it; the sender chooses the mode, and the callback can check it with `call.mode()`.

| API | Execution | Return and error handling |
|---|---|---|
| `emit` | Start each listener's call in registration order in background; same-name events start in send order; calls run concurrently | Returns immediately; each failure goes to ErrorSink as `ListenerFailed` and does not affect other listeners |
| `parallel` | Call listeners concurrently; wait for all | Returns `()` on success; failures aggregate into `ListenersFailed` (also for a single failure), in registration order |
| `serial` | Call one at a time in registration order; stop on a result or error | Return first result; `None` and `Some(false)` count as no result |
| `waterfall` | Listeners call later handlers through `call.next()` and can process their result | Return the outermost listener's result |
| `bail` | As `serial`, but synchronously on the caller's thread | Listeners must be registered with `on_sync`; an async listener is an `AsyncListener` error |

Listeners return `rutis::Result<Option<E::Value>>`. For example, if the event declares `type Value = String`, a serial listener can return a title with `Ok(Some(title))` or pass to the next handler with `Ok(None)`.

In a waterfall, a listener invokes the rest of the chain with `call.next().await` and can process the result; returning directly ends the current flow. In other modes, `next` returns a `WaterfallNext` error. The caller provides the chain's final handler with `ctx.waterfall(event).terminal(…)`, which may be left out. The event argument stays the same throughout the chain; results flow through return values.

Emit and parallel ignore `Some(value)` returned by listeners; serial uses it to decide when to stop. Choose a dispatch mode based on whether you need to wait for completion, receive a return value, or enforce handler order.

A slow listener does not hold up other calls of the same name, but each emit stays in the backlog until every call it started has finished. `ctx.diagnostics().event_backlogs` lists how many emits per event name have been accepted but not completed and the oldest one's wait duration; use it to identify slow listeners. High-rate data streams can use bounded queues with batching or coalescing to limit backlog; persist data separately if it must be replayed. Processing errors are expressed with `Result`; the caller handles panic in waterfall.

## Instance Subtrees

An instance subtree groups a plugin and its descendants for shared management. Instance service keys limit service scope; events are delivered by name to every listener, and instance information is only a filter descriptor; `FiberView::shutdown()` closes the subtree.

Use this structure to manage multiple runtime instances with independent lifetimes. The following workspace example shows how to associate services and events.

### Example: Shared Instance State

The workspace parent plugin creates a service key with its instance ID and passes the key to child plugins. The following snippet belongs in `WorkspacePlugin::apply`; the business types are application-defined:

```rust,ignore
let workspace = ctx.instance();
let state_key = Key::<WorkspaceState>::of().instance(workspace);

ctx.provide_at(state_key.clone(), Arc::new(state))?;
ctx.plugin(IndexerPlugin::new(state_key));
Ok(())
```

Indexer implements `host::DynPlugin`: the key it is given goes into `injects()` as `state_key.erase()`, and `apply` reads it with `require_at(&state_key)`. This service is visible within the workspace and its child plugins; external management features call through an explicit business interface exposed by the workspace.

Once the parent provides state and creates the child plugin, initialization is complete. The host then waits for Indexer to become ready so the child can obtain the already Active parent service.

### Example: Dispatch Within an Instance

Progress and Indexer are child plugins of the workspace plugin and use their own contexts. With rutis-loader's instanced groups, the loader marks each group instance as a scope:

```rust,ignore
// Register from the Progress child plugin.
progress_ctx.on::<DocumentIndexed>(|event, _call| async move {
    println!("indexed: {}", event.0);
    Ok(None)
})?;

// Emit from the Indexer child plugin.
indexer_ctx.emit(DocumentIndexed(path))?;
```

Events are one flat bus: within a root they are delivered by name to every listener, whatever the emitter's or listener's scope. A scope is only a `ListenerInfo` descriptor for filters; to narrow by instance, use `filter` or distinct event names. When the subtree closes, the framework waits for the running calls of its listeners to finish before completing cleanup. Any additional background task created by a callback should be registered using the task-management pattern above.

Isolate service scope is separate from event scopes.

The host coordinates shutdown. If an instance callback needs to close its workspace, it should send a request to the host and return; the host can then wait for shutdown to finish normally, including the current callback.

## Updating Configuration

A plugin that supports hot config updates is built from its config by a constructor. This constructor reuses `BackendPlugin` from Quick Start and validates config during construction:

```rust,ignore
use rutis::host::ErrorKind;

fn backend(label: &String) -> rutis::Result<BackendPlugin> {
    if label.trim().is_empty() {
        return Err(ErrorKind::Validation {
            issues: vec!["backend label is required".into()],
        }
        .into());
    }
    Ok(BackendPlugin(label.clone()))
}
```

The host registers the plugin with the constructor and submits new config later:

```rust,ignore
let backend = root.plugin_from(backend, String::from("local"));
backend.update(String::from("remote")).await?;
```

An update has two stages:

1. **Preflight.** Run the constructor (for a factory, `validate_config` and `build`) and the plugin's `validate`. On failure, return an error and leave current config and instance unchanged.
2. **Reload.** Save new config, clean up the old instance, then construct and load again once dependencies are ready. Consumers reload when services change.

The constructor (and a factory's `build`) should perform pure object construction; put network connections, task startup, and similar side effects in `apply`. Validation also needed on initial load belongs in the constructor or plugin `validate`; a factory's `validate_config` is for update preflight.

The real load can still encounter network or resource errors. The host should retain a recovery policy, such as retrying or submitting the last known-good config again. After update, observe the state of the actual business entry point before resuming request handling. To switch to a different dependency set, have the host choose and register the corresponding plugin.

`current_config::<C>()` returns a snapshot of stored config for display and diagnostics. Config updates affect plugin instances in the current process; code upgrades belong to the application release process.

## Startup and Shutdown

### Observing Runtime State

| State | Meaning |
|---|---|
| `Pending` | Waiting for dependencies |
| `Loading` | Initialization in progress |
| `Active` | Initialization succeeded; services are available |
| `Failed` | Initialization failed; error is in the state snapshot |
| `Unloading` | Resource cleanup in progress |
| `Disposed` | Plugin is unloaded |

Use `view.state()` to read a snapshot and `view.watch()` to wait for changes. Startup waits should handle Active, Failed, Disposed, and timeout; see `wait_active` in the [example](../crates/rutis/examples/development_workflow.rs).

`(&view).await` means that previously queued operations for this plugin have been processed. At that point the plugin may still be `Pending` due to missing dependencies; open business entry points only after required plugins are Active. Background-task health is reflected by service state and error reports.

### Choosing Shutdown Scope

To close one plugin and its subtree, call that plugin's `FiberView::shutdown()`. To close the entire application, call `Ctx::shutdown()`. Calling `Ctx::shutdown()` on any child context also shuts down the whole root.

`view.restart()` cleans up and reloads the current plugin. `view.dispose()` unloads it; a regular plugin is done at that point, and future use requires registering a new instance. Root supports restart after ordinary dispose; use shutdown for final application exit.

### Setting a Wait Limit

`dispose_with_timeout` and root `shutdown_with_timeout` provide bounded waits. After timeout, shutdown continues; the caller can record state and wait for the same result again.

To limit waiting for a subtree shutdown, wrap `view.shutdown()` with Tokio timeout:

```rust
use std::sync::Arc;
use std::time::Duration;
use rutis::host::{ErrorKind, FiberView};

async fn try_shutdown(view: &FiberView, limit: Duration)
    -> Result<bool, Arc<ErrorKind>>
{
    match tokio::time::timeout(limit, view.shutdown()).await {
        Ok(result) => {
            result?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}
```

`true` means shutdown completed; `false` means this wait timed out. Keep the view and call `view.shutdown().await` again later to obtain the result. Once started, shutdown proceeds independently of the waiter.

When a plugin unloads, the framework waits for its listeners' running calls to finish (they see `ctx.cancelled()`). Manage other work that must be awaited explicitly through task handles and cleanup functions.

## Diagnostics and Verification

### Investigating Problems

`root.diagnostics()` provides snapshots of plugin states, dependencies, and service bindings. Start with these checks:

| Symptom | Check |
|---|---|
| Plugin remains Pending | Missing keys, service scope, provider state, health-check result |
| Service lookup returns None | Registration/lookup type, qualifier, instance ID, and whether provider is Active |
| Old service still used after update | Dependency declarations complete? Did consumer obtain new-generation instance? |
| Plugin enters Failed | Specific reason in `view.state().error` |
| Shutdown waits for a long time | Did initialization finish? Does task respond to cancellation? Are cleanup and listener callbacks complete? |

`diagnostics()` reads current state. For services using health predicates (see [Declaring Dependencies](#declaring-dependencies)), call `refresh()` after conditions change and inspect the new check result.

### Handling Errors

Lifecycle operations and waiting event APIs return errors directly. The error type is `rutis::Error`, and `?` converts any standard error into it; to tell rutis's own failures apart, use `err.kind()`, which returns `Option<&ErrorKind>` and is `None` for an error from plugin code. Emit listener errors (wrapped as `ListenerFailed`) and cleanup errors during reload go to ErrorSink; default output is stderr, and hosts can use `Ctx::root_with_sink` to connect their own logger. The ErrorSink, `take_cleanup_errors()`, and waiting on a `FiberView`, `dispose()` or `shutdown()` give shared `Arc<ErrorKind>` errors.

When releasing an effect early, handle the result of `Disposer::dispose()`. Long-lived plugins may periodically call `take_cleanup_errors()` to retrieve completed historical cleanup failures. The owner of each background task observes and reports its errors.

### Verifying Critical Flows

Verify these scenarios for each plugin:

- When a dependency appears later, the plugin moves from Pending to Active and provides its service correctly.
- After a dependency update, old tasks exit, the new instance uses the new service, and long-lived state is retained as designed.
- If initialization fails midway, already-created resources are cleaned up.
- If update preflight fails, the old instance continues working; if the real load fails, recovery policy can run.
- After one instance subtree shuts down, other independent subtrees keep running.
- If shutdown occurs while a task is running, the task returns its specified request result and eventually completes cleanup.

Coordinate test timing with state, oneshot, Notify, or Barrier, and set timeouts on waits. In the repository, run:

```bash
cargo test -p rutis
cargo run -p rutis --example development_workflow
```

For more details, see [lifecycle contract tests](../crates/rutis/tests/contract.rs), [config update tests](../crates/rutis/tests/config_update.rs), [instance subtree tests](../crates/rutis/tests/instance_subtrees.rs), and [shutdown and deadline notes](core-shutdown-and-disposal-deadline.md).

---

Applies to Rutis 0.17. Code marked as a snippet belongs in its stated context; complete programs are in Quick Start and the companion examples.
