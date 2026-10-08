# Service Names Inside Instances

[中文](design-instance-services-2026-10-08.md)

Design proposal · Related to [#159](https://github.com/arcships/rutis/issues/159) · Depends on [#161](https://github.com/arcships/rutis/pull/161) (`instanced` groups)

## 1. Background

#161 lets the loader create instances on demand: each instance of an `instanced` group is a group fiber, and the group's rows load once per instance. A Rust plugin can get the instance's `InstanceId` with `build.instance("session")` and build instance keys itself (`TypeKey::instance::<T>(id)`).

Configuration and TS / Python plugins know services only by **name**, and the mapping from names to keys is process-wide today:

- a `ServiceCatalog` name maps to one fixed key;
- cross-language services all live at `host_key(name)`, with no instance;
- inside a language runtime process, services, host proxies and export slots are registered by name, so two instances' services of the same name collide (the second is refused, or the later one silently replaces the earlier);
- export, import and hosting between peers go by name too.

So a cross-language service inside an instance can only encode the instance id in its name, which loses instance visibility and the "goes with the instance" lifetime.

This design lets a name be declared an **instance service**: the same name resolves to each instance's own key. Configuration and plugin code write only the name and never need an instance id.

## 2. Declaration

The service catalog declares, per name, which kind of instance it belongs to:

```rust
let mut catalog = ServiceCatalog::new();
catalog.register::<Aimux>("aimux");                       // global, as today
catalog.register_instance::<Tools>("tools", "session");  // a Rust service inside session instances
catalog.register_shared_instance("timeline", "session"); // a cross-language service inside session instances
```

| Method | Key |
| --- | --- |
| `register_instance::<T>(name, group)` | `TypeKey::of::<T>()` with the `InstanceId` of the enclosing `group` instance |
| `register_instance_keyed::<T>(name, group, key)` | `key` with that instance's `InstanceId` (named or dynamic keys) |
| `register_shared_instance(name, group)` | `host_key_in(name, instance)` = `host_key(name)` with that instance's `InstanceId` |
| `readable_instance::<T>(name, group)`, `readable_instance_keyed` | As `register_instance{,_keyed}`, and expressions can `read` the value of the instance's service |

- `group` is the id of an `instanced` group. A row using the name gets the **nearest `group` instance** on its instance chain, so rows in a nested instance can use the outer instance's services.
- Each name has one scope: global, or one kind of instance. **There is no fallback**: an instance name never resolves to a global service outside the instance, nor the other way round. For "a default implementation that some instances replace", put a row providing the name inside the instance, whose default implementation forwards to the global service.
- `rutis-bridge` adds `host_key_in(name, instance)`.

## 3. Resolution

Names become keys in three places, and all three now resolve **per copy**: from the copy's instance chain (#161's `Build`), following the name's scope.

| Where | Today | After |
| --- | --- | --- |
| `isolate`, `inject` | Resolved once per row when the tree is composed, shared by all copies | Scopes checked when composing; keys resolved for each copy from its instance chain when it loads |
| Expressions `has` / `read` | A fixed key | Resolved from the copy's instance chain when evaluated |
| Gating and hosts of language rows (`RuntimeResolver`) | Fixed to `host_key(name)` when the module resolves | Each copy's factory is built by a `scoped` factory from its instance chain |

- **Checked when composing**: a row whose `isolate` / `inject` uses an instance name but which is not inside that `group` is invalid (`Unresolved`), and the error says which group the name needs. A misplaced row is reported as soon as the configuration loads. This applies to rows whose resolver handles scope itself (language rows, peer rows) too.
- An expression that refers to an instance name outside the instance fails rather than returning `false`.
- `ExprScope::new(ctx, catalog)` is unchanged (global); `ExprScope::in_instances(ctx, catalog, build)` is added.

## 4. Language Runtimes

### 4.1 The Rust side

A `JsRow` (a TS / Python row) is built per copy by a `scoped` factory, with the copy's instance chain:

- gating: the cross-language services it depends on resolve along the chain, and an instance name gates on `host_key_in(name, instance)`;
- leased host services: the same name in different instances is a different host entry;
- projection: an instance service the row provides is published to rutis at `host_key_in(name, instance)`, seen only by plugins of the same instance.

### 4.2 Scopes inside the process

A runtime process is shared (one process per language serves the rows of every instance), so its registrations must tell instances apart too. This reuses `isolate`:

- each copy automatically isolates the instance names it may use, under the label `rutis-loader/instance/<InstanceId>` of that instance; when its configuration already isolates the name, the configured label is used (since #161, those labels are already per copy / per instance).
- A service in the process is identified by **(name, label)**, written on the wire as an id: `name` without a label, `name` + NUL + `label` with one. Names and labels may not contain NUL, so no two pairs share an id (a global name like `x@L` cannot pass for `x` in scope `L`). Export slots, handles, host proxies and the `host:<id>` call targets are registered by id.
- Plugins still see the name `x`: Node uses Cordis isolation, so rows with the same label share one scope; the Python runtime looks names up through the row's isolate table.

Protocol changes (new feature `scopes`):

| Message | Change |
| --- | --- |
| `hosts.provide [name, methods, label?]` | With a label, the proxy is visible only in that label's scope; its call target is `host:<id>` |
| `hosts.withdraw [id]` | By id |
| `service [id, handle, version]` | Export slots report by id |
| `rows.load` | Unchanged; an exported name's label comes from the row's isolates |

- Rust requires `scopes` from the runtime only when a label is actually involved; rows without labels are byte-for-byte as today. A runtime without the feature fails instance services with "needs a runtime that supports `scopes`".
- Both the Node and the Python runtime implement `scopes`; the Python runtime also gains `isolate` support (it ignores isolates today).
- This also fixes three existing problems: a language row that isolates a name could not see the host proxy leased for it (proxies were registered in the root scope); two rows that each isolate and export the same name had the second one refused; in Node, two names isolated with one label landed in the same Cordis store slot (Cordis stores isolated services by symbol alone), and the symbol now carries the name as well as the label.

## 5. Peers

When a peer link (a `rutis-bridge/peer` row) sits in an `instanced` group, each instance has its own link, and its export, import and host map names by **the instance that row is in**:

- `ExportPlugin` / `ImportPlugin` accept a name-to-key mapping (`ServiceKeys`), still `host_key(name)` by default; `Features` gains `services`, which the loader provides from the row's instance chain;
- `HostPlugin` uses the same mapping, and the plugins it loads for the peer get factories built from the row's instance chain (a `register_with` plugin no longer gets the instance-less factory);
- the protocol is unchanged: the link itself is per instance, so names do not collide on a link;
- each instance's link isolates the `Peer#<id>` it provides under the instance's label, so one configuration gives each instance its own link without collisions. Instances' links usually go to different peers (one sandbox machine per conversation, say, its address read from an instance service by an expression): a peer tells links apart by identity, and cannot accept two links from the same node at once.

Limits and errors:

- a link outside instances that exports or imports an instance name fails, with "`tools` is a service inside `session` instances: put the row in the `session` group";
- a link inside instances cannot turn on `rows` or `runtime`: the `PeerRows#<id>` and `RuntimeSession#<name>` they publish are global keys used by rows outside the link, and every instance would publish them. `peer:` rows and remote-runtime rows inside instances keep using links outside instances; a remote runtime goes through the same `scopes` as a local one, so instance services work there too.

## 6. Lifetime

- Instance service keys carry the instance id, so the kernel withdraws them when the instance subtree closes;
- host leases, projections and export slots go when the row's copy unloads; the process removes the entries registered under that id, and other instances' services of the same name are untouched;
- a peer link inside an instance closes with it, and the services it imported are withdrawn.

## 7. Compatibility

- Applications that register only global names see no change in behavior or protocol;
- New public API: `ServiceCatalog::register_instance{,_keyed}`, `readable_instance{,_keyed}`, `register_shared_instance`, `key_in`, `ExprScope::in_instances`, `host_key_in`, `Process::lease_host_in`, `row_projection_with`, `ExportPlugin::with_keys` / `ImportPlugin::with_keys`, `Features::services`;
- `ServiceCatalog::key(name)` returns keys of global names only; use `key_in(name, build)` for instance names.

## 8. Acceptance

- Two instances each provide a Rust service of the same name (an instance cross-language name), and each instance's TS row and Python row see only their own;
- a service provided by a TS row and by a Python row in an instance is read by name by a Rust plugin of the same instance, and not by other instances;
- a row outside instances that uses an instance name fails when the configuration is composed, naming the group it needs;
- a peer link inside an instance exports and imports that instance's services; a link outside instances that exports an instance name fails;
- after an instance closes, its registrations, proxies and export slots are all withdrawn, with nothing left in diagnostics or in the runtime processes; the other instance keeps running.

## 9. Implementation

| Where | What |
| --- | --- |
| `crates/rutis-loader/src/catalog.rs` | Name scopes, `key_in`, `ExprScope::in_instances` |
| `crates/rutis-loader/src/loader/desired.rs`, `reconcile.rs`, `commit.rs` | Checks when composing; `isolate` / `inject` / expressions resolved per copy |
| `crates/rutis-loader/src/runtime.rs` | `RuntimeResolver`'s `scoped` factory; `JsRow` gates, leases and projects per instance and adds instance labels |
| `crates/rutis-bridge/src/session/services.rs` | `host_key_in` |
| `crates/rutis-bridge/src/runtime/process.rs`, `rows.rs` | Export slots and hosts by id; `lease_host_in`, `row_projection_with`; feature `scopes` |
| `node/rutis-runtime/src/runner.mjs`, `python/rutis/rutis/runner.py` | `scopes`: registrations by (name, label); `isolate` in Python |
| `crates/rutis-bridge/src/services.rs`, `compose.rs`, `crates/rutis-loader/src/peer.rs` | Export, import and host use the name mapping; the peer row provides the mapping and factories from its instance chain |
| `crates/rutis-loader/tests/instance_services.rs`, `instance_runtimes.rs`, `instance_peers.rs` | §8's acceptance: Rust-side resolution, the Node and Python runtimes, peer links |
