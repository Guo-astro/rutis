# rutis-loader

Data-driven plugin management for [rutis](https://github.com/arcships/rutis): which plugins run, with which config, decided by ordered layers of rows rather than code. The counterpart of Cordis' `cordis-plugin-loader` and `cordis-plugin-include`.

- `reconcile` converges the running plugins to the layers; imperative edits (`create`, `update`, `set_disabled`, `move_to`, `remove`, …) change the editable layer, wait for the tree to settle and roll back on failure; `edit::Persist` saves edits.
- `inject` / `isolate` name services through a `ServiceCatalog`; `{ "__jsExpr": … }` expressions are evaluated by `LoaderOptions::expressions`.
- Volatile config fields change without a restart; a plugin can unload itself.
- An `instanced` group runs once per instance the application creates (`create_instance(&ctx, "session")`, `remove_instance`); edits reach every instance, and plugins registered with `Builtins::register_with` are built for each copy from where it runs (`Build::instance`).

The root holds what assembling a loader takes (`LoaderPlugin`, `LoaderOptions`, `Loader`, `Layer`, `Patch`, `Builtins`, `Chain`, `Resolver`, `ServiceCatalog`, `LoaderError`, `RowEvent`); the rest is grouped in modules: `report` (what the loader's methods return), `edit` (imperative edits and persistence), `resolve` (writing a resolver) and `runtime` (plugins of other languages, and rows on a connection's far end).

Plugin sources (`Resolver`):

| source | row name | feature |
| --- | --- | --- |
| `Builtins` | any registered name | |
| `runtime::SessionResolver::new` | `<runtime>:<plugin>`, the plugin run on the far end of that row's session (a local runtime started by rutis-host's `connect::SpawnPlugin`, or a remote one): `py:<module>` for Python; for a Node runtime of its own (one process per plugin), `<runtime>:<npm name or file>` | `node`, `python` or `bun` |
| `runtime::SessionResolver::npm` | also npm package, subpath or file, on the Node runtime's row | `node` |
| `runtime::binary::BinaryResolver` | `go:<plugin>`, `swift:<plugin>`: one runtime per compiled binary | `go`, `swift` |
| `rutis_dylib::DylibResolver` | `dylib:<dir>` | rutis-dylib's `loader` |

```rust
let mut builtins = Builtins::new();
builtins.register("weather", Weather::new);   // fn new(config: &WeatherConfig) -> rutis::Result<Weather>
let plugin = LoaderPlugin::new(Chain::new().with(builtins), LoaderOptions::default());
let loader = plugin.handle();
root.plugin(plugin).await?;
loader.reconcile(vec![Layer::new("defaults", defaults)], None).await?;
```

Guides (Chinese): [embedding in Rust](https://github.com/arcships/rutis/blob/main/docs/guide/rust-host.md); design: [docs/design-rutis-loader-2026-10-02.md](https://github.com/arcships/rutis/blob/main/docs/design-rutis-loader-2026-10-02.md).
