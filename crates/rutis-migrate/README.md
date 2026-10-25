# rutis-migrate

Migrates Rust code that uses [rutis](https://github.com/arcships/rutis) from 0.16 to 0.17: one plugin trait, `rutis::host` for what hosts use, events on `Ctx`, and `rutis::Error`. 0.17 changes only names and spellings, not behaviour, so code that compiles after the migration runs as it did. The full guide, with the manual steps: [migrating to 0.17](https://github.com/arcships/rutis/blob/main/docs/migration-0.17.en.md).

## Usage

```bash
cargo install rutis-migrate
rutis-migrate 0.17 path/to/crate             # prints the diff, changes nothing
rutis-migrate 0.17 path/to/crate --write     # writes the files, then runs cargo check
```

```text
rutis-migrate 0.17 [PATH] [--write] [--no-check]
```

- `PATH` is a file or a directory (default `.`); every `.rs` file under it that uses `rutis`, `rutis_sdk` or `rutis_loader` is migrated. `target`, hidden directories and `node_modules` are skipped.
- `--write` writes the files, then, when `PATH` has a `Cargo.toml`, runs `cargo check --all-targets` there and lists the errors left. `--no-check` skips that check.
- Every `// TODO(rutis 0.17): …` comment it adds is printed as `file:line`; the compile errors left are next to them.

In the rutis repository: `cargo run -p rutis-migrate -- 0.17 PATH --write`.

## What it does

The syntax tree (syn, with line and column positions) only locates the edits; they are made on the original text, so comments and formatting stay. It rewrites:

- `CordisError` to `rutis::Error`, and its variant paths and `Arc<CordisError>` to `rutis::host::ErrorKind`; `.map_err(|e| …PluginFailed(Box::new(e)))?` to `?`; `match` on an error to `match &*err`.
- The 0.16 object-safe `impl Plugin` to `impl rutis::host::DynPlugin` (the same trait renamed), `injects` returning `Vec<TypeKey>`, and `Plugin` as a type or bound (`Box<dyn Plugin>`, `P: Plugin`) to `DynPlugin`.
- `impl TypedPlugin` to the new `impl Plugin` with `async fn apply`; `Typed::new(p)` to `p`.
- `ctx.effect(|| Effect::AsyncDisposer(…))` and `Effect::Disposer(…)` to `ctx.effect(|| async move { … })`.
- Listeners on `ctx.events()` (`on`, `on_sync`, `on_opt`, `on_pattern`, `once` and the rest) to `ctx.on::<E>`, `ctx.on_sync::<E>` and `ctx.listen::<E>()…` with closures; a hand-written `impl Listener<E>` struct to an `async fn on_event` method called from a closure.
- Dispatches on `ctx.events()` to `ctx.emit`, `parallel`, `serial`, `waterfall(e).terminal(t)` and `bail`.
- `provide_as` / `get_as` / `require_as` with a `TypeKey` to `provide_as::<T>` / `get::<T>`, or, with a named or instance key, to `provide_at` / `get_at` / `require_at` with a typed `rutis::host::Key<T>`.
- Imports of names moved off the `rutis` root to `rutis::host`, `rutis::host::diagnostics` and `rutis::events`, plus `use rutis::host::CtxHost;` where host methods are called; names moved off the `rutis_loader` root to their modules (`report`, `edit`, `resolve`, `runtime`, `peer`).
- `Builtins::register_fn(name, |c| Ok(Box::new(P::new(c))))` to `register(name, |c| Ok(P::new(c)))`.

## What it does not do

It is not fully automatic. Where a rewrite is not mechanical it leaves a `TODO(rutis 0.17)` comment saying what to write instead, among them: `Effect::Many` and cleanups returned from `apply`, event keys whose type it cannot find, `Gate` / `Keyed` / `KeyedGate` / `Typed::with_keys`, per-copy factories given to `register_with`, and events sent under a name chosen at run time (written as `rutis::internal::CtxInternal::named(..)` for now; prefer an event type that overrides `Event::name`). It does not turn the plugins it renamed to `DynPlugin` into the new `Plugin`, nor factories into constructors: those are the manual steps of the migration guide. Read the diff before `--write`.

## Running it again

A file already in the 0.17 style (one that mentions `rutis::host`, `DynPlugin`, `async fn apply(`, `rutis::Result` or a `TODO(rutis 0.17)` comment) is left alone, except for rutis-loader paths, which are safe to rewrite again. So it can run more than once on the same tree. The check is per file: a file is migrated in one go, not partly.
