# Cordis

rutis and Cordis (the Node plugin framework) interoperate in two ways:

- **Use a Cordis application as a node:** connect an existing Cordis application to a rutis node so they can use each other's services, forward events, and let rutis run plugins inside it. Use `@arcships/rutis-runtime/bridge`.
- **Mount Cordis plugins in a Rust application:** run published Cordis plugins in a real Cordis context without changing their source, and use Rust types generated at build time from their type declarations. Enable the `cordis` feature of `rutis-bridge`.

A Cordis plugin can also run as a row in rutis-loader: name its package in `rutis.json`, with the package unchanged; see [Write a TypeScript plugin](typescript-plugin.en.md#existing-cordis-plugins):

- Cordis rows in the same Node runtime use each other's services by Cordis's native rules, getting the objects themselves.
- The services a row provides reach rutis by themselves: rows of other runtimes, other nodes and Rust plugins use them by name. The methods are the functions of the service object (its own and its prototype chain's, not `constructor`), all called as `async`. When the package's `package.json` declares `rutis.provides`, that declaration applies (to mark some methods `sync`, say).
- When a service of the same name already has another provider (a row of another runtime, a Rust plugin, an import from a node), the row providing it later fails, also when it provides it after loading, and the error names both, for example `service storage is already provided by row prompts: row storage cannot provide it too`.
- `rutis-host check` runs no plugin, so it does not see undeclared services: a service a row depends on that only such rows may provide is reported as `may be provided at runtime by …`, not as a failure.

## Use a Cordis application as a node

```bash
npm install @arcships/rutis-runtime
```

```js
import { Link, Export, Import, Host, Events } from '@arcships/rutis-runtime/bridge'

ctx.plugin(Link, { peer: 'main', id: 'mac', dial: 'wss://main.example.com/rutis', token: process.env.RUTIS_TOKEN })  // or listen: 'wss://…'
ctx.plugin(Export, { peer: 'main', services: { calendar: { today: 'sync', later: 'async' } } })
ctx.plugin(Import, { peer: 'main', services: ['clock'] })
ctx.plugin(Host, { peer: 'main' })                     // let main run npm plugins installed here
ctx.plugin(Events, { peer: 'main', out: ['tock'], in: ['tick'] })
```

- `Link` maintains a connection to one peer and exposes it as `rutisPeer.<id>`. Other features depend on it and are recreated after every reconnect.
- `Export` exposes Cordis services to the peer with the declared method shapes. `Import` registers peer services as Cordis services and rejects a name already used locally.
- `Host` lets the peer load npm plugins installed here, running them with the row's `isolate` and `inject` settings. Enable it only for trusted peers.
- Credentials can also come from `RUTIS_TOKEN`, `RUTIS_CA`, `RUTIS_CERT`, and `RUTIS_KEY`.

For the rutis node configuration, see [Connect nodes](nodes.en.md).

## Mount Cordis plugins in a Rust application

Currently supported on Unix (Linux and macOS), with Node 22 or later. See the [compatibility layer design](../design-protocol-plugin-mount.en.md) for design boundaries and [`examples/dsh-baseline`](../../examples/dsh-baseline) for a complete example.

### 1. Prepare an npm project

Create an npm project beside the application and install the plugins to mount and the [`@arcships/rutis-runtime`](https://www.npmjs.com/package/@arcships/rutis-runtime) runtime (source is in `node/rutis-runtime` in this repository). Use the versions in this project's lockfile. The runtime protocol version must match the crate; this is checked during the build.

```json
{
  "private": true,
  "type": "module",
  "dependencies": {
    "@deepseek-ai/cordis": "4.0.4",
    "@deepseek-ai/dsh-credentials-local": "0.2.0-rc.1",
    "@arcships/rutis-runtime": "0.11.0"
  }
}
```

```sh
npm --prefix cordis ci
```

The build does not install npm dependencies automatically. It fails with the required command if a package is missing. During development in this repository, you can use a source reference such as `"file:../path/to/rutis/node/rutis-runtime"`.

### 2. Declare mounts in `Cargo.toml`

```toml
[dependencies]
rutis = "…"
rutis-bridge = { version = "0.11", features = ["cordis"] }
tokio = { version = "1", features = ["full"] }

[build-dependencies]
rutis-bridge = { version = "0.11", features = ["cordis"] }

[package.metadata.rutis-cordis]
npm = "cordis"                   # npm project from step 1, relative to Cargo.toml
# runtime = "…"                  # defaults to <npm>/node_modules/@arcships/rutis-runtime

# Each mount generates one module
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"   # npm package, or path = "src/plugin.ts"
version = "0.2.0-rc.1"           # optional: build fails if the installed version differs
events = ["credentials/record-updated"]         # Cordis events forwarded to rutis listeners
# emits = ["…"]                  # events sent by rutis to Cordis listeners
# provide = ["…"]                # services provided to the plugin by the rutis application

# Combined mount: dependent plugins share a single Cordis Context
[package.metadata.rutis-cordis.mounts.workspace]
group = [
    { name = "storage", plugin = "@deepseek-ai/dsh-storage" },
    { name = "storage_json", plugin = "@deepseek-ai/dsh-storage-json" },
    { name = "storage_domain", plugin = "@deepseek-ai/dsh-storage-domain" },
    { name = "sessions", plugin = "@deepseek-ai/dsh-session-persistence-jsonl" },
    { name = "workspace", plugin = "@deepseek-ai/dsh-workspace" },
]
```

In `build.rs`:

```rust
fn main() {
    rutis_bridge::cordis::build::from_manifest().expect("generate Cordis bindings");
}
```

Include all generated mount modules in the application:

```rust
rutis_bridge::include_mounts!();   // generates modules such as credentials and workspace
```

Generated files live in Cargo's build directory and should not be committed or maintained by hand. Cargo regenerates them when a plugin or its types change. Members that cannot be bound are listed as build warnings with their location and reason; the remaining members are still generated.

### 3. Use a mount

```rust
let ctx = rutis::Ctx::root()?;
let view = ctx.plugin(credentials::Plugin::new(credentials::Config {
    dsh_home: Some("/tmp/home".into()),
    ..Default::default()
}));
(&view).await?;

// This is a regular rutis service: dependency gating, replacement, and revocation use native rutis rules.
let store = ctx.require::<credentials::CredentialProvider>()?;
store.set(&credentials::CredentialRef::from("app/api-key"), "s3cret").await?;
```

| Capability | Usage |
| --- | --- |
| Methods | Synchronous methods remain synchronous; Promise-returning methods become `async fn`. They return `Result<T, rutis_bridge::cordis::Error>`; Cordis business errors are `Error::Remote`. |
| Data types | Branded types become newtypes (`CredentialRef::from("…")`); interfaces become structs; literal unions become enums. Optional (`x?: T`) becomes `Option` where `None` sends `undefined`. Required but nullable (`T \| null`) becomes `Option` where `None` sends `null`. Optional and nullable becomes `Option<Option<T>>`. |
| Live objects | Objects with methods (such as `Workspace`) are proxies: property getters read live values, methods call the original object, and passing them back restores the original object. Unions of live objects become `ObjectRef`; unions mixing data and live objects become enums. |
| Callbacks | Function arguments take Rust closures. Returned functions (such as unsubscribe functions) become `RemoteFunction`. |
| Cancellation / timeout | Dropping the returned future cancels the call; Cordis methods receive an `AbortSignal` and abort. For example: `tokio::time::timeout(d, store.read_record(&key)).await`. |
| Events | Events in `events` generate rutis event types and can be subscribed to with `ctx.events().on(&ctx, &EventKey::<CredentialsRecordUpdated>::of(), listener)`. Events in `emits` are sent from rutis to Cordis through `emit` / `parallel`. |
| Host services | Services in `provide` generate traits (such as `SystemPromptHost`). Implement the trait and register it with the generated `provide_system_prompt(&ctx, host)` function. The mount waits until the service is ready. |

### Deployment

Manifest-driven mounts locate the runtime and plugins relative to the npm project. When moving the binary to another machine or directory, deploy the npm project with its resolved `node_modules` and point `RUTIS_CORDIS_ROOT` to it:

```sh
cp -RL cordis /opt/app/cordis        # -L expands symlinks such as file: dependencies
RUTIS_CORDIS_ROOT=/opt/app/cordis /opt/app/my-app
```

If unset, the build-time location is used, so development needs no extra configuration. TypeScript source files mounted with `path` should be inside the npm project and copied with it. Use the default runtime at `node_modules/@arcships/rutis-runtime` (leave `runtime` unset) so the project can move as a unit.

### 4. Cordis plugin boundaries

Some behavior provided by the JavaScript call stack cannot be preserved across processes. The rules are in [Requirements §5](../requirements-protocol-plugins.en.md). The main points are:

- Cross-boundary `emit` is a notification; rutis is not guaranteed to have processed it when `emit` returns. Use `parallel` when you need to wait.
- Event order is guaranteed only within one side. Waterfall events and events with return values are not forwarded.
- After direct `ctx.set` replacement, rutis switches to the new object on the next call to that service.
- A synchronous method cannot wait during execution for a result that requires the Node event loop; this returns `SyncWaitCycle`.
- A host-provided service is a proxy on the Cordis side, so `instanceof` checks fail.
- A plugin must not block the event loop for long periods. The compatibility layer does not impose call timeouts; use an async method with `tokio::time::timeout` when needed. A timeout cancels the call.
- An uncaught exception or unhandled Promise rejection in a plugin ends the whole Node process under Node's default behavior, stopping every plugin in the same mount. The mount's services are then revoked and rutis plugins that depend on them stop waiting. Calls return `Error::Transport`, which reports how the process ended. To recover, the application unloads and mounts it again.

### 5. Common build errors

| Error | Resolution |
| --- | --- |
| `the Cordis plugins are not installed: run npm --prefix … ci` | Install the npm project's dependencies. |
| `… is not installed: add it to …/package.json` | Add the plugin to the npm project and install it. |
| `… 1.0.0 is installed, 2.0.0 is required` | Make the npm project match the configured `version`. |
| `… speaks protocol N, this rutis-bridge speaks M` | Install an `@arcships/rutis-runtime` version that matches the crate. |
| `native plugin dependencies are unresolved: … (name)` (at runtime) | Put the missing dependency in the same `group`, or provide it from rutis with `provide`. |

## Export Rust services to rows

The reverse of a mount: existing Rust services, used by name from the rows of every language (TypeScript, JavaScript, Python, Go), with the dispatcher and TypeScript declarations generated at build time from the Rust definitions. `build.rs` lists the types with `rutis_bridge::cordis::build::Exports` (feature `cordis` of the build dependency); the application declares the generated module with `rutis_bridge::include_exports!()` and registers `exports::all()` with `ServiceCatalog::export_all`. Usage: [Embed rutis in a Rust application](rust-host.en.md#export-an-existing-rust-service); example: [`examples/rust-services`](../../examples/rust-services).

| `Exports` | Effect |
| --- | --- |
| `.service(name, path)` | Export the struct or enum at `path` as the service `name`, outside instances |
| `.service_in(name, path, group)` | Inside each instance of the instanced group `group` (as `register_shared_instance`) |
| `.declared(path, typescript)` | The TypeScript type to use for the Rust type at `path`, where the generator cannot derive it |
| `.typescript(file)` | Also write the declarations to `file` (relative to `Cargo.toml`), only when they change |
| `.generate()` | Write `OUT_DIR/rutis_exports.rs` and `rutis_exports.d.ts`; `exports::TYPESCRIPT` holds the declarations |

### Where the source comes from

`path` is written as the application writes it: `crate::…` for its own types, a dependency's crate name (as `Cargo.toml` names it, renames included) for a dependency's. The build script runs `cargo metadata` (for the target being built, offline first) to find each crate's entry file and the features Cargo resolved, and reads the module tree with syn: `mod` files (`#[path]` included), `use` items, globs and `pub use` re-exports (a type re-exported from a private module is found), type aliases (generic ones too). The methods are the public `&self` methods of the type's inherent `impl` blocks anywhere in its crate. `#[cfg]` is evaluated: `feature = "…"` from the features Cargo resolved, target `cfg`s from `CARGO_CFG_*`, `test` and `doc` as off. Stable Rust only: nothing is compiled or expanded for this.

### What crosses

| Rust | TypeScript | |
| --- | --- | --- |
| `bool`, `String`, `&str`, `char`, `PathBuf`, `&Path` | `boolean`, `string` | |
| `f32`, `f64`, `u8`–`u32`, `i8`–`i32` | `number` | |
| `u64`, `i64`, `usize`, `isize`, `u128`, `i128` | `number \| bigint` | beyond ±(2^53−1) it crosses as a bigint (0.11 value rules); both are accepted |
| `Option<T>` | `T \| null` | a trailing `Option` parameter may be left out; an `Option` field is optional |
| `Vec<T>`, `&[T]`, sets, `VecDeque<T>`, `[T; N]` / tuples | `T[]` / `[A, B]` | |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `Record<string, V>` | |
| `Box<T>`, `Arc<T>`, `Rc<T>`, `Cow<T>` | `T` | |
| `serde_json::Value` | `unknown` | |
| `rutis_bridge::session::{Instant, Bytes, BigInt, ValueMap, ValueSet}` | `Date`, `Uint8Array`, `bigint`, `Map`, `Set` | the tagged values |
| A struct or enum deriving `Serialize` / `Deserialize` | an `interface` or a union, as serde writes it | `rename`, `rename_all`, `rename_all_fields`, `tag`, `content`, `untagged`, `transparent`, `flatten`, `skip`, `default`, `skip_serializing_if`; generic types are generic in TypeScript; docs become JSDoc |
| `impl Fn(A, ..) -> BoxFuture<'static, Result<X, E>>`, a generic `F: Fn(..)`, `Arc<dyn Fn ..+ Send + Sync>`, `Box<dyn Fn .. + Send>` (aliases too) | `(arg0: A, ..) => X \| Promise<X>` | a function passed by reference; up to four arguments, taken by value |
| `async fn`, or a return type `BoxFuture<'_, T>`, `Pin<Box<dyn Future<Output = T> ..>>`, `impl Future<Output = T>` | `Promise<T>` | every method is `Promise`-returning for the caller |

Arguments are decoded with serde (any `Deserialize`), results encoded with serde (any `Serialize`); Cargo checks every generated call against the real signatures.

**Functions passed by reference:** the Rust side gets a function that calls the caller's: its future resolves once the function returned and the Promise it returned settled, so a subscriber has handled an event before the native side goes on; a throw or rejection is the future's `Err` (`E: From<rutis_bridge::session::Error>`, which `CordisError`, `anyhow::Error` and boxed errors are), so it takes the native failure path. What the function returns where Rust expects `()` is ignored.

**Cancellation:** a call the caller stops waiting for (an `AbortSignal`, a dropped future) drops the method's future on the Rust side, as for every call.

**The calling row's context:** a parameter `&rutis::Ctx` (or `Ctx`) is not a parameter in TypeScript: it is the context of the row making the call (`rutis_bridge::session::caller_row()`), so what the method registers on it (`ctx.effect`, a listener) is that row's and goes when the row unloads or restarts, for example a subscription. Called from outside any row (from Rust, from a peer node), such a method fails, saying it needs the calling row's context.

### Errors

A method returning `Result<T, E>` (`std::result::Result`, an alias of it, `anyhow::Result`, `io::Result`) throws `Err(e)` in the caller's language: a JavaScript `Error`, a Python `rutis.RemoteError`, a Go `*rutis.RemoteError`. It is not an unclassified call error:

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| An enum with serde | its type name | its variant as serde writes it: the string it serializes to, the one key of an externally tagged variant, the `tag` field of an internally or adjacently tagged one | its serde value |
| An enum without serde (or `untagged`) | its type name | the variant's Rust name (from its derived `Debug`) | its serde value if it has one |
| A struct with serde | its type name | the string it serializes to, else its type name | its serde value |
| `String` | `Error` | the string | the string |
| `rutis::CordisError` | `CordisError` | its variant | — |
| `rutis_bridge::session::Error` | thrown as it is (a call it forwarded keeps the far end's error) | | |
| Anything else (`anyhow::Error`, `io::Error`) | its type name | its type name | — |

`message` is always `e`'s `Display`. JavaScript reads `error.code` and `error.data`; Python `error.code`, `error.data`; Go `remote.Code`, `remote.Data` (big integers as `*big.Int`); Rust `Error::code()`, `Error::data()`. They cross in the error graph, so a runtime that does not know them still gets the name and message. In the declarations each error type has a `<Name>Thrown` type, `RustError<'Name', 'Code1' | 'Code2', Data>`, named in the method's JSDoc (`@throws {CalendarErrorThrown}`).

### Declarations

`rutis_exports.d.ts` has an interface per service type, the data types it uses, `RustError`, the `<Name>Thrown` types, and `Services`, the services by name: `ctx.use<Services['calendar']>('calendar')`. It passes `tsc --strict` (`examples/rust-services` checks it). Commit the `.typescript(…)` copy: the build rewrites it when the Rust side changes.

### Limits

- What a macro writes is not seen: types, `impl` blocks or methods written by a macro (other than derives) are not found. A service type must be a concrete struct or enum (not generic, not a trait object).
- Methods taking `&mut self`, borrowing a reference out, taking a reference to a function's argument (`Fn(&A)`), a generic parameter other than a function, `impl Trait` other than `Fn`, or a function that does not return a future of a `Result` are left out with a warning.
- A data type needs a serde derive the generator can read: a hand-written `Serialize`, `#[serde(with / serialize_with / into / remote)]`, or a type of a crate not read (other than `std`, `serde_json`, `chrono`, `uuid`, `url` and the tagged values) needs `.declared(path, typescript)`: until then the methods using it are left out, the warning naming the type.
- `#[cfg]` is evaluated as above; a `cfg` the build script cannot see is taken as off.
- A method's future must be `Send` (the compile fails otherwise).
