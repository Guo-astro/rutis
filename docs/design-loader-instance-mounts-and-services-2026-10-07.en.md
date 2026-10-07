# Creating Plugin Instances on Demand

[中文](design-loader-instance-mounts-and-services-2026-10-07.md)

Design proposal · Related to [#158](https://github.com/arcships/rutis/issues/158) and [#159](https://github.com/arcships/rutis/issues/159) · Code baseline `330ff51`

## 1. Background

Applications need to manage their own plugins: which plugins are installed and how they are configured come from one configuration, which can change while running.

Some plugins need one copy per business object, created when business logic decides. For example, each conversation gets a set of plugins, and each branch in it another set. This is a general problem for plugin systems (compare IntelliJ's project-level services and the scoped lifetime of dependency injection frameworks). Without framework support, either plugins keep per-object state themselves, or the host writes its own assembly layer; both bypass the framework's dependencies, lifecycle, reload, and diagnostics.

The rutis kernel already provides instance subtrees, instance keys, and permanent subtree shutdown. This design adds the loader side: configuration declares which plugins each instance contains, and configuration changes are synchronized to every instance.

Division of responsibility:

- **Business logic** decides when instances are created and closed;
- **the loader** decides which plugins each instance contains and how they are configured.

This design does not change the rutis kernel and introduces no business categories into the framework.

## 2. Configuration

One attribute is added: **a group can declare `instanced: true`**.

- An ordinary group (from Cordis) is loaded once beneath its parent by reconciliation, and its child rows load in the group's ctx.
- An `instanced` group is not loaded by reconciliation; it is created only through `create_instance` and can have multiple instances. Each instance is an independent group fiber, and child rows load once per instance.

```yaml
- id: session
  group: true
  instanced: true
  config:
    - id: session-scope
      name: dim/session-scope
    - id: tool-registry
      name: dim/tool-registry
    - id: timeline-tree
      name: dim/timeline-tree
    - id: branch
      group: true
      instanced: true
      config:
        - id: branch-scope
          name: dim/branch-scope
        - id: loop
          name: dim/loop
        - id: tool-executor
          name: dim/tool-executor
- id: aimux
  name: dim/aimux
```

Rules:

- The configuration tree decides where an instance goes: it is created beneath its configuration parent's ctx. A top-level `instanced` group is created beneath the loader's ctx; a nested one inside the outer instance it belongs to.
- Ordinary rows and ordinary groups inside an instance load automatically; nested `instanced` groups need another `create_instance` call.
- Plugins in one instance are siblings, and their relationships are expressed by `injects`. For example, `tool-registry` depends on a service provided by `session-scope` and waits for it; `loop` depends on the global `aimux`.
- Rows under no `instanced` group behave as today.
- `instanced` applies only to groups; on a plugin row it makes the row invalid.

## 3. API

```rust
let s1 = loader.create_instance(&ctx, "session").with(assembly).await?;

// From any plugin inside the session instance, for example timeline-tree creating a branch:
let b1 = loader.create_instance(ctx, "branch").await?;

loader.remove_instance(b1.plugin).await?;
```

### 3.1 `create_instance(&ctx, group_id)`

- **The parent instance comes from the ctx**: starting at the managed plugin that `ctx` belongs to, the loader walks up the parent relationships it recorded until it finds the instance of the group's configuration parent, and uses it as the parent instance. Top-level groups take the loader's ctx or an ancestor ctx. If none is found, the call fails.
  - It can therefore be called from any managed plugin inside the parent instance, without holding the parent instance's own ctx.
  - The loader only walks relationships it recorded and does not rely on a kernel ancestry query.
- The target must be an `instanced` group that is enabled and valid; otherwise the call fails.
- `.with(value)` attaches a business value (any `Send + Sync` type) for the factories of plugins in the instance (§4).
- It waits until the instance's child rows settle, then returns `Instance { plugin, view, report }`; `report` lists each child row's result:

| Result | Meaning |
| --- | --- |
| `Active` | Running |
| `Waiting` | Waiting for services it depends on |
| `Failed(error)` | Resolution, validation, expression evaluation, or `apply` failed |
| `Skipped` | The child row is disabled |

- Child failures do not fail creation; the caller decides from `report` whether the instance is usable.
- It can be called and awaited from a plugin's own `apply`.

### 3.2 Closing Instances

- `remove_instance(plugin)` disposes the instance and its subtree.
- Business logic can also close the instance's fiber directly (for example `FiberView::shutdown()`); the loader observes the disposal and only updates bookkeeping, without disposing again.
- When a parent instance closes, every instance beneath it is disposed with the subtree.
- Instances are not written to configuration. After a process restart, business logic creates them again, for example calling `create_instance` for each conversation restored from a database.

## 4. Factories

Plugins inside an instance often need to know which instance they are in, to build instance keys.

```rust
builtins.register_with("dim/loop", |build: &Build| {
    let scope = BranchScope {
        session: SessionScope { instance: build.instance("session")? },
        instance: build.instance("branch")?,
    };
    Ok(LoopFactory::new(scope))
});
```

- `Build` provides:
  - `instance(group_id)`: the `InstanceId` of that group's instance on the instance chain, which is that group fiber's `ctx.instance()`;
  - `value::<T>()`: the nearest value of that type supplied with `with` along the instance chain.
- Plugins in one instance are all inside that group fiber's subtree, so instance keys built from this `InstanceId` are visible to all of them. Plugins that provide instance services (such as `session-scope`) also use this id rather than their own fiber's `ctx.instance()`.
- Plugins registered with plain `register` are unchanged and use the same factory in every instance.
- A `Resolved` can carry a constructor that builds the factory from `Build`; the resolution cache stays keyed by module name and does not grow with instances.
- The loader does not rewrite the `injects()` a factory returns.

## 5. Dynamic Updates

A change to a configuration row acts on all of that row's copies:

| Change | Result |
| --- | --- |
| Add a row to an `instanced` group | Loaded in every instance of that group |
| Delete or disable a row inside instances | Disposed in every instance |
| Change the config of a row inside instances | Updated on the same fiber when `injects()` is unchanged; otherwise a new fiber is created and rutis reloads dependents |
| Disable or delete an `instanced` group | All its instances are closed; later `create_instance` calls are refused |
| Change an `instanced` group's `isolate` or `inject` | Each instance is rebuilt by ordinary group rules, reusing the `with` value from creation |
| `rename_module`, `reload` | All instances dry-run, then applied together; any new failure returns all to the old module |
| Volatile / overlay layers | Likewise applied to all instances |

- An edit is first dry-run on every instance; any failure rolls back the whole edit, writes no editable layer, and leaves the previous generation running.
- Newly created instances load from the current configuration.
- Whether and when to change configuration at runtime is the application's decision. How dependents handle a reload follows each plugin's own contract.

## 6. Management and Diagnostics

| Operation | On a row id | On an instance (`PluginId`) |
| --- | --- | --- |
| `update`, `set_disabled`, `set_inject`, `set_isolate`, `rename_module`, `move_to`, `remove`, `reload` | Act on all instances as in §5 | — |
| `restart` | Restart every copy of the row | `restart_instance`: restart only this one |
| `get` | The row entry | — |
| `locate(plugin)` / `row(instance)` | — | Return the owning row id |

- `entries()` lists rows in tree order; a row inside instances is followed by its entry in each instance. `instanced` groups are visible even with no instances.
- `EntryInfo` gains `instance: Option<InstanceInfo>`, with the containing instance's `PluginId` and instance chain.
- `LoaderChanged` gains `InstanceCreated` and `InstanceRemoved`; events that refer to a row carry instance information.
- A plugin inside an instance calling `dispose_self` stops only that copy, recorded as `Stopped`, without writing configuration; `restart_instance`, a change to that row, or a rebuild of the instance starts it again. `dispose_self` on rows outside instances is unchanged.

## 7. Example: dim-agent

| dim-agent today | With the loader |
| --- | --- |
| `SessionScopePlugin` provides Session services with its own `ctx.instance()` | The `session-scope` row in the `session` group; provides with `build.instance("session")` |
| `session_plugin(\|scope: &SessionScope\| P)`, installed after SessionScope is Active | Rows in the `session` group; factories use `build.instance("session")`; `injects` waits for `session-scope` |
| `timeline_plugin(\|scope: &BranchScope\| P)` | Rows in the `branch` group; factories use `build.instance("session")` and `build.instance("branch")` |
| `ctx.plugin(SessionScopePlugin(assembly))` then `wait_active` | `create_instance(&ctx, "session").with(assembly)`, judge by `report`, then `validate` as before |
| `TimelineFactory::build` creates BranchScope and calls `wait_active` | `create_instance(ctx, "branch").with(branch_assembly)` |
| Closing: `FiberView::shutdown()` | Unchanged |
| Capability set fixed before publication | The application decides whether to change configuration at runtime |

## 8. Out of Scope

- **Cross-language plugins and named instance services inside instances** (#159): second phase, resolving names along the instance chain defined here.
- **Overrides for a single instance** (#158 item 8): overlays remain global.
- **Windows**: see #160.

## 9. Acceptance

- One configuration manages global rows and two levels of `instanced` groups; after creating, closing, and creating again, runtime state matches configuration.
- Plugins with the same name in two instances provide and read services by their own instance keys without seeing each other; plugins in an inner instance can read services of the outer instance; plugins in one instance wait for each other through `injects`.
- Calling `create_instance` with the ctx of any managed plugin inside an instance finds the correct parent instance; a mismatched configuration parent fails.
- `report` reports `Active`, `Waiting`, and `Failed` correctly.
- Adding, deleting, changing, and `reload` of rows inside instances take effect immediately on all instances; dependents reload when `injects()` changes; if any instance's dry run fails, the whole edit rolls back.
- Changing an `instanced` group's `isolate` / `inject` rebuilds its instances and keeps the `with` value.
- `dispose_self` in a plugin inside an instance stops only that copy, which can be recovered; configuration is unchanged.
- After an instance closes, its subtree, bookkeeping, and diagnostic entries are all removed; repeated creation and closing does not grow state.
- Reconciling stored configuration in a new loader and creating instances again yields the same runtime state.

## 10. Relationship to the Current Implementation (`2d577a5`)

The current implementation is built on "hosts register mounts, template rows expand by kind", a different model from this design, and needs rewriting:

| This design | Current implementation |
| --- | --- |
| `instanced` groups | `mount: <kind>` |
| `create_instance(&ctx, group)`, instances placed by the configuration tree | `register_mount(kind, ctx)`, loading into a host-supplied ctx |
| Factories get the instance chain and business value from `Build` | Encoded into runtime names; `Builtins` discards mount information |
| `injects()` not rewritten; no kernel change | `mapped_key` rewrites gating keys; `Ctx::scope_for` made `pub` |
| Cross-language in a second phase | `names` mapping and `ServiceScope` implemented |

Reusable parts: dry run and rollback across all expansions, cleanup that only updates bookkeeping when a host closes, and registration generations that keep stale cleanups from affecting new registrations.

Implementation references: [runtime table](../crates/rutis-loader/src/loader/mod.rs#L228), [loading and cleanup](../crates/rutis-loader/src/loader/reconcile.rs#L64), [update flow](../crates/rutis-loader/src/loader/commit.rs#L85), [groups](../crates/rutis-loader/src/loader/plugins.rs), [Resolver](../crates/rutis-loader/src/resolver.rs#L15), [instance service keys](../crates/rutis/src/key.rs#L166).

Status: design proposal; the current implementation needs rewriting per §10.
