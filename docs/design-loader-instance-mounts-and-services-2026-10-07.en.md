# Managing Multiple Instances with a Single Loader

[中文](./design-loader-instance-mounts-and-services-2026-10-07.md) | English

Design proposal · [#158](https://github.com/arcships/rutis/issues/158), [#159](https://github.com/arcships/rutis/issues/159) · Baseline `330ff51`

## 1. Configuration and Loading

```yaml
- id: assistant
  mount: document
  name: assistant-plugin
  inject: [document]
```

The host registers two document instances, and the loader creates `assistant@d1` and `assistant@d2`, respectively. Editing assistant updates both plugins; closing d1 unloads `assistant@d1`.

| Interface | Result |
| --- | --- |
| `register_mount("document", ctx, wait)` | Loads matching templates under ctx; optionally waits and returns per-row results |
| `handle.unregister()` | Unloads the managed plugins for that mount |

`mount` is specified on a top-level row or group and inherited by descendants. The ID of the host instance root is referred to as the owner; runtime records are keyed by `(row ID, owner)`. The configuration file stores templates, while runtime state is queried per instance.

## 2. Naming and Construction

The configuration stores the module name. The loader generates a runtime name for each mount:

```text
assistant-plugin + d1 → resolve(instance runtime name 1) → factory 1 → child plugin 1
assistant-plugin + d2 → resolve(instance runtime name 2) → factory 2 → child plugin 2
```

The Resolver extracts the original module name and instance information from the runtime name, looks up the module, and constructs a factory that holds the owner. Each factory returns its own dependency list. The runtime name also serves as the resolution cache key.

## 3. Services

The existing service registration mechanism is retained, with an additional choice of global or instance scope; global scope is the default. For instance-scoped services, the loader generates service keys based on the owner. Rust type information and cross-language method descriptors continue to be registered as before. Scope and cross-language callability are independent.

```text
document@d1 → host_key("document").with_instance(d1)
document@d2 → host_key("document").with_instance(d2)
```

Plugins continue to access the service as `document`. Before creating a plugin, the loader determines its instance and translates the name into the corresponding service key.

- Instance rows prefer instance-scoped declarations; other names use global declarations. If the selected service is not yet available, required dependencies enter a waiting state.
- inject, isolate, expressions, and cross-language proxies share the same resolution results.
- Isolation labels are distinguished by instance; identical shared labels within the same instance refer to the same service location.
- Node uses Cordis's `isolate(name, symbol)` to assign name locations; Python queries the name mapping through the current row.

## 4. Updates and Shutdown

| Operation or Result | Behavior |
| --- | --- |
| Edit a configuration row | Updates all instances of that row |
| Disable, delete, or unload a configuration row | Cleans up all instances of that row; for groups, this includes descendants in each instance |
| Change the module | Cleans up all old instances of that row, then loads them using the new module |
| reload | Checks all rows and instances that use the module |
| Preflight failure | Rejects the edit; the current version continues running |
| apply failure | Reloads the previous configuration and reports both execution and recovery results |
| Failure to load a new instance | Reports that instance's per-row errors to the host |
| Plugin stops itself | Temporarily disables that row in the current instance; for groups, descendants are also disabled |
| Recover from temporary disablement | Restarts that instance row or re-registers the mount |
| Host closes an instance or revokes a mount | Cleans up the managed plugins under that instance |
| Unload the loader | Cleans up all instances of all configuration rows |

Editing workflow: **Identify affected instances → Run all preflight checks → Update plugins → Persist.**

Registration, revocation, and editing execute serially. A host shutdown signal invalidates the mount immediately. After restarting, the host re-registers, invalidating the previous registration identifier.

## 5. Scope of Changes

| Change | Implementation Area |
| --- | --- |
| Template expansion | The loader registers mounts and generates runtime names and runtime records for each instance |
| Name mapping | The Resolver constructs plugins from runtime names; the catalog generates instance keys; the bridge's string indexes use internal service names |
| Batch management | Iterates over all instances of each configuration row for preflight checks, updates, and unloading; cleans up the corresponding subtree by host instance; aggregates instance results for queries |

Cross-language control only adds a `names` mapping to `rows.load`:

```text
names = { service name within the plugin: internal service name }
internal service name = encode(service name, owner, effective isolation location)
```

Host registration, reference counting, duplicate-export checks, service notifications, and revocation all use internal service names. Existing `name` fields and string tables carry these values directly. Object calls continue to use the handle of the specific object.

## Appendix A: Detailed Rules

### Mounts and Diagnostics

- `mount` is a nonempty string. The registered ctx must share the loader's root and belong to a valid runtime generation. Each owner corresponds to one category; duplicate registration returns an error.
- The registration wait option returns per-row results. Waiting for startup uses the existing FiberView state observation mechanism. A timeout lists the rows that are still waiting, and results are tied to the current registration and configuration. The existing loader settle operation only waits for state transitions to finish; it cannot be treated as proof of successful startup.
- The host provides services and completes apply before waiting for consumers to start. Explicit unregister performs asynchronous unloading; dropping the handle only releases the handle.
- entries/get aggregate instance states by template; locate/row resolve the row and owner from a fiber. Templates without instances are displayed as “not expanded.”
- disabled is evaluated at the mount root. config is evaluated in the context after isolation for the parent groups and the current row has been applied. Instance-specific preflight checks run at registration time.
- Group edits check descendants; volatile is sent per instance after all preflight checks pass. Restoring the previous configuration constitutes reloading; plugins handle external operations.

### Services and Factories

- The actual service identity is `(TypeKey, effective ScopeId)`; the effective ScopeId includes ancestor isolation. Private labels include the row and owner; shared labels include the owner.
- Global rows use global declarations. If only an instance-scoped declaration exists and no owner is available, a context error is reported. Unregistered native Cordis names are delegated to Cordis; share_by_name follows the existing rules.
- The set of instance service names is determined before assembly. Each Node row receives the complete mapping of instance names, covering dynamic provide/get/inject and access by internal child plugins. Unknown dynamic names follow native Cordis rules.
- In-process native consumers obtain objects directly after verifying the connection and service location. Cordis manages independent isolation for internal child plugins; exports read from the scope of the row where the declaration is defined.
- Child plugins of the same type are constructed separately, each with its own dependency list. Dependencies may be generated dynamically for each mount instance and are fixed when the corresponding fiber is created. Changing the dependency list of an existing fiber requires rebuilding the fiber.
- Runtime names use a dedicated, versioned encoding with fields for the module name, category, owner, and mount registration identifier. The example suffix `@d1` is for display only. Shared encoding and decoding logic handles special characters in module names; invalid encodings return parsing errors.
- Builtins decodes the runtime name and looks up the original module name. The instance registration entry point accepts a `MountInfo → PluginFactory` constructor; ordinary registrations reuse the original factory. RuntimeResolver decodes the runtime name, locates the package using the original module name, and generates a JsFactory that holds the owner. Third-party Resolvers use the same decoding function when supporting instance construction.
- The factory captures instance information; injects returns the dependencies for that instance, and build(config) passes the instance information to the plugin constructor. Chain continues to select a Resolver based on NotFound.
- Resolved and offline records in the loader and RuntimeResolver are indexed by the full runtime name; module lookup and version checks use the original module name. Existing runtime caching policies are retained. reconcile reuses the corresponding Arc, and mount revocation removes the runtime names generated by that registration. The public schema_of uses the configured module name.
- spawn, update, volatile, dry-run, and state/schema/meta queries use each target's cached Resolved. New preflight results are reused at commit time. Dependency list changes follow the existing rebuild decision logic; configuration changes use update. Direct mounting checks the instance ancestry of ctx.
- reload first invalidates the caches for all runtime names of the module, obtains new results, and runs preflight checks before replacing the entire batch. On failure, it restores the entire batch of old caches. Disabled targets are invalidated as well. RuntimeRowsPlugin handles offline refreshes by actual runtime name; rename cleans up the corresponding runtime names. Remote module declarations follow the existing re-query rules.

### Communication and Cleanup

- host, export slots, observers, and Projection initialization all use internal service names. Duplicate exports under the same name follow the existing duplicate-checking rules. Multiple instances using the same global service share a lease.
- Node slots store plugin-local names. The exporter reads the local name from the corresponding scope, while notifications carry the internal service name. Host registration also provides the local name in that scope. Python queries services through the row mapping, and update carries the original mapping.
- Handles use monotonically increasing identifiers within a session to distinguish specific objects. Service notifications retain the existing version-based stale-notification filtering rules. Capability checks add a mapping-support flag to the existing features.
- When unloading a configuration row, first lock all of its runtime records, then revoke projections, unload plugins, and release leases for each instance; finally, aggregate the cleanup results. Shared services are released according to reference counts.
- Explicit unloading order: revoke Rust projections and wait for consumers to stop → unload language rows → release the host lease.
- Cordis self-disposal and Rust consumer shutdown occur concurrently. Rust revokes projections after receiving the termination notification.
- Local disablement in #158 first modifies the existing Rust FiberView observation path. Synchronizing native Cordis self-termination with the loader is a separate lifecycle issue and will be handled separately.

### Scope

In this phase, configuration overrides apply per template. Remote Runtime uses the extended control protocol; cross-node Export/Import and broadcast events retain global semantics. Older endpoints and peer rows return a capability error when they receive an instance-binding request.

## Appendix B: Ablation Results

| Proposed Change Removed | Direct Consequence | Conclusion |
| --- | --- | --- |
| Add owner to mount registration and runtime records | Two instances of the same row occupy the same runtime record | Required |
| Pass owner and generate dependencies per instance | Instance plugins still use global service keys | Construction data must change; plugins of the same type may have different dependencies |
| New resolution entry point | Instance information can be passed to resolve(name) through the runtime name | Remove resolve_in |
| Separate bind phase and generic binder | The Resolver constructs instance factories from names | Remove |
| Composite resolution cache key | The runtime name already includes mount registration identity | Continue using string keys |
| Preflight checks for all instances | Configuration errors in other instances are discovered only after the first instance passes its checks | Required |
| Asynchronous cleanup of externally managed rows | Plugins in host subtrees continue running after the loader is unloaded | Required |
| Add instance identity to host, export slots, and language-side lookups | Same-name services overwrite one another, exports conflict, or reads return the wrong instance | Indexes must change; reuse existing algorithms |
| Separate MountHandle.settled | A registration wait option is sufficient for assembly needs | Remove the separate interface |
| A set of public kernel inspection interfaces | Root identity can be compared via root_view().instance(); cancellation is available through cancellation_token() | Remove the entire proposed set; add only what is actually missing |
| New object management and allowlist lifecycle changes | These are existing object lifecycle issues | Move out of scope; only extend handles to distinguish instances and loads |
| Native Cordis termination notification protocol | Host shutdown still uses existing per-row cleanup | Move out of scope; track native lifecycle synchronization separately |
| New negotiation, disconnection, and notification management mechanisms | Existing features, observers, and cleanup can be reused | Add only a feature flag and new indexes |

An effective isolation location is a functional requirement; exposing ScopeId publicly is an interface choice. `host_key_in` is a convenience constructor. Neither is treated as a separate new system.

## Appendix C: Source References and Acceptance Coverage

| Basis | Source |
| --- | --- |
| Factory dependencies and construction | [plugin.rs:30](../crates/rutis/src/plugin.rs#L30), [plugins.rs:12](../crates/rutis-loader/src/loader/plugins.rs#L12), [resolver.rs:15](../crates/rutis-loader/src/resolver.rs#L15) |
| Row management, preflight checks, and rollback | [reconcile.rs:64](../crates/rutis-loader/src/loader/reconcile.rs#L64), [commit.rs:24](../crates/rutis-loader/src/loader/commit.rs#L24) |
| Instance ownership, isolation, and cancellation | [ctx.rs:163](../crates/rutis/src/ctx.rs#L163), [491](../crates/rutis/src/ctx.rs#L491), [1252](../crates/rutis/src/ctx.rs#L1252) |
| Service binding and runtime | [catalog.rs:52](../crates/rutis-loader/src/catalog.rs#L52), [runtime.rs:274](../crates/rutis-loader/src/runtime.rs#L274), [process.rs:570](../crates/rutis-bridge/src/runtime/process.rs#L570) |
| Language rows and native isolation | [runner.mjs:205](../node/rutis-runtime/src/runner.mjs#L205), [runner.py:90](../python/rutis/rutis/runner.py#L90), `@deepseek-ai/cordis@4.0.4/src/context.ts:121` (local dependency source) |
| Native termination | `@deepseek-ai/cordis@4.0.4/src/events.ts:329`, `src/fiber.ts:265` (local dependency source) |

Acceptance coverage: multiple mount categories and recovery; same-name instance services in Rust/TS/Python; global sharing and ancestor isolation; native Cordis plugin behavior; preflight checks for all instances; recovery failures; self-termination; cascading shutdown; disconnections and late notifications.

Status: source research and independent review are complete; the functionality remains to be implemented. The Cordis references are based on the locally installed version 4.0.4; deployment compatibility must be checked against the plugins' actual dependencies.
