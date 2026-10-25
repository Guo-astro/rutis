# Plugin values across languages: a plugin defined in another language can be mounted by the Rust host

[中文](design-plugin-values-2026-10-25.md)

Status: design (2026-10-25), #642.

## 1. Goal

In Cordis a plugin is a value: a function, or an object with `apply`. It can carry what the enclosing `apply` prepared and be handed to another plugin. Whoever calls `ctx.plugin(plugin, config)` owns the copy it mounts. Every copy of one plugin is recorded together, so when the plugin reloads, `ctx.registry.delete(plugin)` ends all of them at once (`registry.ts` in `@deepseek-ai/cordis` 4).

This design makes that rule hold across languages:

> A plugin defined in one language is handed to Rust as an ordinary argument or return value of a service call. Rust receives a plugin it can mount, and mounts it with `ctx.plugin`. Each copy runs in the process that defined the plugin and ends with the current generation of the row that handed it over. Plugin authors write nothing specific to rutis.

The need only arises across languages. Within one process this already works today: between Rust plugins, and between Cordis plugins in one Node runtime. Cordis itself has no matching problem. So this design adds only the cross-language step and no new way of composing plugins.

Not in this design:

- Generators, registries and registration events. How to register, when to mount and into which instance are decided by the user. Dim, for example, keeps its own part registry and writes it the Cordis way.
- New SDK methods.
- Services in other languages receiving and mounting plugins. The leaf ctx has no `ctx.plugin()`, and today only the Rust side needs to mount.
- A Rust caller handing a Rust plugin to a Rust service. That still uses Rust's own types; a service that accepts both handles them itself.
- Loader instance groups and configured rows. They are unchanged.

## 2. Today

| Case | Works? |
| --- | --- |
| A Rust plugin hands a Rust plugin to a Rust service, which mounts it with `ctx.plugin` | Yes |
| In one Node runtime, a Cordis plugin hands a plugin object to another Cordis plugin, which mounts it with `ctx.plugin` | Yes, natively in Cordis |
| A plugin in Node, Python or another language hands a plugin to a Rust service | No, see below |

This is what happens today when each SDK's plugin definition is handed to another language:

| SDK | Plugin definition | Handed to another language today |
| --- | --- | --- |
| Node, Bun | the result of `definePlugin(...)` | Crosses as an object reference and loses its plugin mark |
| Cordis plugins in Node | a function, a class, or an object with `apply` | Functions and classes cross as function references, objects with `apply` as object references. A class called through a reference is called without `new` and fails |
| Python | the result of `define_plugin(...)` (a dataclass) | Raises `TypeError` |
| Go | the result of `rutis.Define(...)` (`*rutis.Definition`) | Crosses as data and becomes an empty object `{}` |
| Swift | the result of `Rutis.define(...)` (`Rutis.Definition`) | Raises an error |
| Kotlin | the result of `Rutis.define(...)` (`Plugin`) | Raises an error |

The Rust side and the runtimes have three more gaps:

- Generated services (`#[rutis::service]`, `Exports` in `build.rs`) accept only data and functions as parameters, and fail on an object reference.
- A reference's lifetime is counted per session only; nothing records which row handed it over. When a row unloads, the runtime neither releases nor invalidates the references it handed out.
- `rows.load` and `rows.schema` accept only a module or binary name.

The first user of this is Dim (arcships/dim ADR 006 D3). In Dim every plugin has one process-level row. In `apply` it hands its session-level and branch-level parts to Dim's part registry, and when a session opens, it mounts the registered parts under itself. The session is in Rust; the parts may be written in other languages. Today a non-Rust part can only be handed over as "module name + config" and is loaded as an ordinary row, so:

- the part cannot use what the plugin's `apply` prepared, except through a shared module or a service;
- the part does not end with the plugin's generation. When the plugin unloads or reloads, Dim has to find and unload those parts itself, and make sure the parts stop before the plugin does;
- one feature is split into two modules.

Until this design lands, Dim keeps using "module name + config".

## 3. Usage

Plugin authors write plugins and call services as usual. In the examples, `parts` is Dim's part registry, an ordinary Rust service.

```js
// Node / Bun leaf plugin
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  inject: ['parts'],
  async apply(ctx) {
    const pool = await openPool()
    await ctx.use('parts').register('session', definePlugin({
      inject: ['history'],
      config: { type: 'object', properties: { depth: { type: 'number' } } },
      apply(ctx, config) {
        ctx.provide('gitStatus', new GitStatus(pool, config.depth))
      },
    }))
  },
})
```

```js
// Cordis plugin: Cordis's own style, unchanged
export const inject = ['parts']
export async function apply(ctx) {
  const pool = await openPool()
  await ctx.parts.register('session', {
    inject: ['history'],
    apply(ctx, config) { /* use pool directly */ },
  })
}
```

```python
# Python
from rutis import define_plugin

async def apply(ctx, config):
    pool = await open_pool()

    def session(ctx, config):
        ctx.provide("gitStatus", GitStatus(pool, config["depth"]))

    await ctx.use("parts").register("session", define_plugin(session, inject=["history"]))
```

```go
// Go: function-field arguments are converted by value as usual; no new declaration
var parts struct {
	Register func(ctx context.Context, scope string, part *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* use pool directly */ return nil },
}))
```

The receiver is Rust. Its parameter type is `ForeignPlugin`. To mount, first call `at(&ctx, config)` to build the plugin for that position with that config, then pass it to `ctx.plugin`:

```rust
use rutis_bridge::ForeignPlugin;

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, part: ForeignPlugin) {
        self.add(scope, part); // keep it; drop it when part.ended() completes
    }
}

// When a session opens, mount on the session's own ctx
for part in parts.of("session") {
    ctx.plugin(part.at(&ctx, config.clone())?);
}
```

## 4. Rules

| # | Decision |
| --- | --- |
| R1 | **What can be handed over as a plugin:** each SDK's plugin definition: the result of `definePlugin` in Node and Bun, `define_plugin` in Python, `rutis.Define` in Go, and `Rutis.define` in Swift and Kotlin. They are sent as a new reference kind, `plugin` (§5). Most of them cannot cross today (§2), so no working code changes. Cordis plugins in the Node runtime (functions, classes, objects with `apply`) are still sent as function or object references, and the receiver recognises them (R2). |
| R2 | **The receiver decides:** when a Rust parameter or return type is `ForeignPlugin`, a received `plugin`, function or object reference is taken as a plugin, and the host reads its declarations (name, `inject`, `provides`, config schema) once from the runtime that defined it. When they are read depends on the direction: <br>· a parameter: the reference arrives → the declarations are read → the Rust method is called; <br>· Rust calls a service in another language that returns a plugin: the far method completes → the declarations are read → the Rust caller gets the `ForeignPlugin`; <br>· a Rust method returns a `ForeignPlugin`: it can only go back to its defining runtime (R3), and nothing is read. <br>If the declarations cannot be read (the value is not a plugin), the call fails, and the error names the parameter or the return value. Other types are unchanged: a data parameter refuses a `plugin` reference, and a function parameter is still called as a function. A Cordis object handed to an object parameter is still an object reference. |
| R3 | **The Rust type:** `rutis_bridge::ForeignPlugin`, which is `Clone`. It has four methods: `name()`; `at(&ctx, config)`, which builds the plugin for `ctx`'s position with that config, to pass to `ctx.plugin`; `ended()`, which waits until the plugin becomes invalid; and `is_ended()`. One value handed over several times is one plugin: the two `ForeignPlugin`s are equal, and their copies are recorded together. It takes no part in ordinary reference relaying (§5): sent to a third runtime as an argument or a return value, it fails to encode; passed back to its defining runtime, the original reference is used and it arrives there as the original value. |
| R4 | **Where to mount:** `part.at(&ctx, config)` computes the copy's keys from `ctx`'s position. Inside an instance, shared names are isolated per instance, and gating and provided services use that instance's keys, exactly as for a row's copy in an instance. The plugin it builds may only be mounted on that `ctx`; mounting it elsewhere fails to start. A plugin can only be mounted inside the instance of the row it came from, or deeper; a plugin from a row at the root can be mounted anywhere. A value that belongs to a process (R6) can be mounted anywhere if the runtime is local, and only inside its connection's instance, or deeper, if the runtime connected in. In a wrong position `at` fails and says why. Mounting needs rutis-loader; without a loader in the tree, `at` fails. |
| R5 | **Each mounted copy:** <br>· runs in the process that defined it; <br>· is gated in rutis on its declared `inject`; <br>· publishes the services it provides through the catalog, like a row's; <br>· is owned by the fiber that mounted it, and ends when that fiber closes; <br>· has its config fixed when it is mounted and validated against its schema, and each copy may have a different config; <br>· to change the config, the mounting side unmounts the copy and mounts it again with the new config, like any other Rust child plugin in 0.17; <br>· ends alone when the plugin ends itself. <br>In listener descriptions, `CallerRow` and errors, its `row` is the id of the row it came from (empty for a value that belongs to a process) and its `scope` is the instance chain where it is mounted. This is how a row's copies in instances already look: they share the row's id. |
| R6 | **Ending with the origin:** a plugin value belongs to the current generation of the row that handed it over: as a call argument, to the calling row; as a return value, to the row that provides the service; handed over outside any row's call, to the runtime process. One value belongs to the first row that handed it over. When that row unloads, reloads or ends itself: <br>1. every copy mounted from the value ends first, and the host waits until they are cleaned up; <br>2. then the row unloads; <br>3. then `ended()` completes, and `at` fails from then on. <br>When the process exits, the copies end at once (the connection is gone) and `ended()` completes. This matches Cordis, where `registry.delete` ends every copy when a plugin reloads. |
| R7 | **After a reload:** the new generation's `apply` hands over new values, and the user registers and mounts them again. By then the copies of the old values have ended (R6). |
| R8 | **Checking:** `rutis-host check` does not know about plugins handed over at run time. If a mounted copy's gating is not met, only that copy waits, as a row would. |
| R9 | **Clients in each language:** <br>· Go: pass `*rutis.Definition` to a function field; arguments are converted by value as usual. <br>· Swift: give the `@RutisClient` parameter the type `Rutis.Definition`. <br>· Kotlin: give the KSP client interface parameter the type `Plugin`. <br>· TS: `rutis-host types` declares `ForeignPlugin` as the TS plugin type (a `definePlugin` result or a Cordis plugin). <br>· Python, Node, Bun: nothing to declare. |
| R10 | **Testing tools:** when a plugin value is handed to a test double, the double receives a value it can pass straight to the testing tool's `load`, as with the real runtime. In Node strict mode, a frozen `definePlugin` result can be handed over too (today the Proxy throws on it). The Python testing tool accepts a `Plugin` today while the real runtime raises; now both agree. |

## 5. Wire format and runtime contract

1. **A new reference kind, `plugin`:** the shape is `{"type":"reference","value":{"id","home","kind":"plugin","origin"}}`. The other fields and the lifetime are the same as for function and object references: counted per session, freed with `Release`, invalid once the session ends. Each SDK sends its plugin definitions as this kind (R1).
2. **No ordinary relaying:** today a reference the host received can be relayed to another session as a relay object. `plugin` references and `ForeignPlugin`s (whatever the underlying `plugin`, function or object reference) do not take that path. When the host would send one to a third runtime, the call fails, and the error says a plugin value can only go back to its defining runtime. When it goes back to its defining runtime, the host sends the original reference (`home: true`) and creates no relay.
3. **The `plugins` capability:** the host declares `plugins` in both the endpoint and the compat handshakes. If the far end has not declared it, the call in which a runtime sends a `plugin` reference fails with "the far end does not accept plugin values", and the session continues. This is the same rule as for value types ([Value types](design-value-types-2026-10-15.en.md) §4). Runtimes add `plugin-values` to the `features` of their `mount` reply. If a runtime without it hands over a reference that Rust takes as a plugin, Rust reports an error saying the runtime's SDK is too old.
4. **The control call `plugins.read [value]`:** the argument is the value itself. It goes back to its defining runtime as a reference and is decoded there into the original value. The runtime answers `{ name, config, inject, provides, announces }`, using the same code that describes rows (`describe` behind `rows.schema`), and fails if the value is not a plugin. In Node, Cordis plugins are recognised by Cordis's rule: a function, a class, or an object with `apply`.
5. **The `entry` of `rows.load` may be a plugin value:** like `plugins.read`, the value goes back to its defining runtime as a reference. The other arguments (`key`, config, `isolate`, `inject`, `exports`, scope) are the same as for a row. In the runtime the copy is a row: `rows.unload` and `rows.ended` are unchanged; its config does not change once it is mounted, so it never receives `rows.update`. The Node runtime wraps a leaf plugin as a Cordis plugin and calls `ctx.plugin` on a Cordis plugin value directly, as for rows.
6. **The host decides the unload order:** it always unloads the copies before the row they came from, so runtimes need not record which row a copy belongs to.

## 6. Host-side implementation

1. **Type and decoding (rutis-bridge):** a `ForeignPlugin` holds the reference, the declarations read for it, and its origin: the key of the origin row's generation, or the process.
   - In generated services, the parameter kinds gain "plugin" next to "function" and "data" (the macro's `Param` and `Wire` in `rutis::service`), and `Exports` in `build.rs` recognises the type too.
   - Decoding a `ForeignPlugin` sends `plugins.read`. For a parameter, the generated dispatch code waits for it before calling the method. For a return value, when Rust calls a service in another language, the declarations are read after the far reply arrives and before the result is handed to the Rust caller.
   - The origin is fixed at decoding time: `caller_row()` for an argument, the row providing the service for a return value.
   - Encoding a `ForeignPlugin` checks the session it is going to: the defining runtime gets the original reference; any other session makes the call fail (§5, item 2).
2. **Mounting (rutis-loader):** `at(&ctx, config)` finds the loader in the tree through an interface defined in rutis-bridge and provided by rutis-loader.
   - The loader walks up the parent fibers to find the row and the instance `ctx` belongs to (the same walk as `Loader::locate`), computes the instance chain and the keys, and builds the copy's plugin (a `DynPlugin`, whose dependencies are fixed at this point).
   - The copy starts through the same code as a row's copy in an instance (`JsRow::start`): leasing host services, publishing services, `rows.load`, handling `rows.ended`. There are only two differences: the runtime is the origin's process (`RowRuntime::Connection`), and the `entry` is the plugin value.
3. **Ending (rutis-loader):** the loader records, for each origin, the copies mounted from its plugin values.
   - When the origin row unloads (the cleanup of its `JsRow`), the loader first has those copies end themselves (`dispose_self`), waits until they are cleaned up, and only then withdraws the row's published services and sends `rows.unload`.
   - When the process exits, the published services are withdrawn as the connection closes, and the copies end along the same path as `rows.ended`.
   - Then `ended()` completes; the host drops the reference, and the runtime frees the value.
4. **Each runtime:**
   - Node and Bun: implement `plugins.read` and `rows.load` with a plugin value as `entry`; send `definePlugin` results as the `plugin` kind.
   - Python: send `Plugin` as the `plugin` kind.
   - Go: send `*rutis.Definition` as the `plugin` kind.
   - Swift and Kotlin: send plugin definitions as the `plugin` kind; the client macros and KSP recognise plugin definition types.
   - All of them reuse their own `describe` and row-loading code.

## 7. Performance

| Aspect | Impact |
| --- | --- |
| Handing over a plugin | One `plugins.read` round trip per received value. Registration is rare and not on a hot path |
| Mounting a copy | The same as loading a row's copy in an instance |
| Existing calls | Encoding checks once more whether a value is a plugin definition; nothing else changes |
| Memory | A table of copies per origin; one reference per `ForeignPlugin` |

## 8. Alternatives considered

| Alternative | Why not |
| --- | --- |
| rutis adds `ctx.generator(name, plugin)` and keeps its own generator registration, list and `rutis/generator` event | Cordis has no such method, so Cordis plugins could not use it. A registry is the user's concept (Dim's ADR 006 D2 assigns it to Dim); moving it into rutis adds a way of composing plugins that Cordis does not have |
| The author hands a plugin over with `ctx.provide('sessionPart:git', plugin)`, and the session looks up providers by name prefix | It uses only existing entry points, but "provide a plugin as a service and look it up by prefix" is a convention Cordis does not have, and Cordis cannot look up providers by prefix |
| Keep "module name + config" | Parts cannot use what `apply` prepared, and do not end with the plugin's generation (§2) |
| Decide by the sender's kind only: only `plugin` references are plugins | Cordis plugins would need wrapping before they could be handed over, and ordinary objects with an `apply` method could not be told apart. Letting the receiver's type decide leaves Cordis plugins unchanged |
| Decide by the receiver only, without a `plugin` kind | Python, Go, Swift and Kotlin plugin definitions cannot cross at all today (§2), so they need some way to be sent; a new kind also makes "cannot be called, only mounted" visible on the wire |
| Send the declarations with the reference and skip `plugins.read` | That works only for SDK definitions; Cordis plugins would still need a read. One path is simpler than two, and one round trip per registration is negligible |
| Drop `at` and write `ctx.plugin(part, config)` as in Cordis | Rust has no overloading, and `ctx.plugin` takes one argument. Also, dependencies are fixed at spawn time (`DynPlugin::injects` takes no ctx, [core/D32](design-config-hot-update-and-dynamic-events-2026-09-21.en.md)), and the plugin value does not know which instance it will be mounted in; names inside instances have per-instance keys, so the position has to be known first |
| Use the host-layer `plugin_with(factory, config)` so the config can change in place | That is a host-layer method (`host::CtxHost`); plugin code mounts children with `ctx.plugin`. In 0.17 the config is fixed when a plugin is mounted, and changing it means the mounting side rebuilds the plugin ([Rust API convergence](design-rust-api-convergence-2026-10-24.en.md)); Cordis also takes the config at `ctx.plugin(plugin, config)` |
| Have runtimes record which row each reference belongs to | All six runtimes would need that bookkeeping. The host knows the row at the moment it receives the value (the calling row, or the row providing the service), so recording it on the host side is enough |
| Let services in other languages receive and mount plugins | The leaf ctx has no `ctx.plugin()`, and today only the Rust side needs to mount. If that is needed later, it can be added with the same wire format |

## 9. Acceptance

- A Node row hands a plugin to a Rust service in `apply`; the plugin's closure uses a value prepared in `apply`. Rust host parts in two instances each mount a copy with `ctx.plugin(part.at(&ctx, config)?)`. Check that:
  - both copies use that value;
  - each copy sees only its own instance's services;
  - the declared `inject` gates them;
  - the two copies can have different configs;
  - after one copy is unmounted and mounted again with a new config, the new copy uses the new config and the old one has been cleaned up.
- Cordis plugin objects and functions can be handed over and mounted the same way; class plugins too.
- When that Node row unloads, both copies end before the row finishes unloading; `ended()` completes, and `at` fails afterwards. After the row reloads, the new value it hands over can be mounted again. When the runtime process is killed, both copies end and `ended()` completes.
- Mounting outside the instance of the row the plugin came from makes `at` fail and say why.
- Ordinary parameters are unchanged: a data parameter refuses a plugin definition, and a Cordis object handed to an object parameter is still an object reference.
- When Rust calls a Node service that returns a plugin, the `ForeignPlugin` Rust gets already has its declarations read; if the returned value is not a plugin, the call fails.
- Handing a `ForeignPlugin` to a third runtime as an argument or a return value makes that call fail; handing it back to its defining runtime gives it the original value.
- If the host has not declared the `plugins` capability, the call in which a runtime hands over a plugin fails, and the session continues.
- Testing tools: in Node strict mode, a plugin value handed to a test double can be loaded with `load`; the Python testing tool and the runtime give the same result.
- These cases are written as a conformance test shared by all runtimes (like the events conformance test), so the runtimes that follow reuse it.
- [Guarantees](guarantees.en.md) gain two rows: `bridge/plugin-values` (R1–R3, §5) and `loader/plugin-value-copies` (R4–R7).

## 10. Implementation

| Phase | Content |
| --- | --- |
| P1 | Wire format and capability; `ForeignPlugin` and the plugin parameter kind in the macro and `Exports`; `plugins.read`; mounting and ending in the loader; Node and Bun (including testing tools); TS declarations; guarantee rows; docs (a "plugins" row in "Passing values" of the [plugin API](guide/plugin-api.en.md), the [Rust host](guide/rust-host.en.md) guide, and the TypeScript and Bun plugin guides) |
| P2 | Python and Go: SDK sending, runtime, testing tools, guides |
| P3 | Swift and Kotlin: SDK sending, client macros and KSP, testing tools, guides. A connected app runtime can only mount inside its connection's instance (R4) |

One PR per phase, merged in order; P2 and P3 reuse P1's conformance test.
