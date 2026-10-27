# Guides by layer: a short path for plugin authors, "Build a host open to plugins" for host owners

[中文](design-guides-by-layer-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: main `103c0d22`. Scheduled: 0.19.
Issue: [#671](https://github.com/arcships/rutis/issues/671) (with [#685](https://github.com/arcships/rutis/issues/685) merged into it). Part of [#663](https://github.com/arcships/rutis/issues/663) (experience requirements 2 and 3; A4, H2, O1, O5).
Relation to 0.18: the guides describe the behaviour after 0.18 — config gets its Schema defaults ([config Schema](design-config-schema-2026-10-27.en.md)), error text and `Loader::status()` / `rutis-host status` ([diagnostics](design-diagnostics-2026-10-26.en.md) §4.5, §5), and the "Debugging" section each guide already has (#679). This design changes none of that; it decides how the guides are written and where things go.

This design is **a plan for rewriting the guides**: the new structure of each guide, what each step covers and with which example, what moves where, the outline and examples of the host owner's guide, the few code changes needed, and how the result is accepted. The guides themselves are rewritten in the implementation steps.

Out of scope:
- The APIs and output text of other 0.19 issues: `rutis-host call` and the service list (#670), strict fakes and Python types (#684), the "keep plugins in check" options (#683), `rutis-host add` and the install API (#691), the names at each SDK's root (#692). This design refers to them and does not design them.
- A Kotlin plugin guide: Kotlin plugins are hosted only by Android apps; there is no `rutis-host new` or `dev`, so steps 1 and 7 of the short path do not exist; its readers are app authors. `kotlin-app.md` and `swift-app.md` are unchanged.
- Products that use `rutis-host` as their runtime, and Swift / Kotlin apps as hosts: #663 has not evaluated them yet; this version of the host owner's guide covers Rust embedding.
- `rutis-host new --lang rust` or `dev` for Rust plugins: a Rust plugin is compiled into a Rust host and has no runtime of its own there; the Rust short path uses a minimal Rust host in step 7 (§3.3).
- The content of the debugging guide (#679; 0.18 adds a "Debugging" section to each guide; this design only fixes where it sits).
- Doc comments in `rutis-agent`, `rutis-cli` and `aimux-llm`: example projects, not on the release train ([release](release.en.md)).

## 1. Where things stand

**Plugin authors**. In the 0.17.0 evaluation, plugin authors needed 7 concepts for the common tasks: plugin, service, `inject`, config, events (`on` / `emit`), the testing tool, `dev` and rows. The guides instead:

- The TypeScript guide follows `weather` in §2 with "a plugin is itself a value"; §3 "Testing" covers the five ways to emit, `filter`, `prepend`, scopes and cancellation, with two plugins rewriting a greeting through `serial` and `waterfall` on `host()`. The Python, Go and Swift guides do the same.
- The generated project is `greeter.hello`, the guide's example is `weather`: pasting the guide's code into the generated project breaks the template's test (#663 A1).
- "Row" and "the row with the plugin's id is the plugin's own config" get one sentence at the end of §4 in the TypeScript and Python guides; the Go guide does not explain them.
- Every guide ends with "Used by a host", the install commands per language; that is the host author's business, and rutis-host.md already has the full table (0.17.2, #666).
- Only the TypeScript and Python guides' examples are run by the doc-example check (`tools/doc-examples.mjs`, followed on Linux after a release); the Rust guides' examples are crate doctests; nothing checks the Bun, Go and Swift examples.

**Host owners**. In the 0.17.0 host-owner end-to-end evaluation (deskmate), these were found only by reading crate sources: row state (`Loader::entries()`, `EntryStatus`, `waiting_for`, `settled`), that `reconcile` waits for every plugin's `apply`, `LocalRuntime::python_path`, `Launcher::env`, `processes().kill()`, `RuntimeState`, `caller_row()` in a marked service, a custom `Resolver`, and why the loader's parts fit together the way they do ("copied it, didn't understand it"). rust-host.md is a reference organized by API; no guide is written around the task "build a host that is open to plugins". Also:

- `use rutis_loader::EntryStatus` fails with `E0603 enum import EntryStatus is private`: the crate root has `pub(crate) use crate::report::*` (and the same for `edit`, `resolve`, `internal`, `runtime`, `peer`), so the root path is taken by an internal import and the public path `rutis_loader::report::EntryStatus` has to be guessed.
- The published crates carry about 280 lines of Chinese doc comments (`FiberState`, `Snapshot`, `shutdown_with_timeout` and others), hard to read on docs.rs.

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | The six plugin guides (TypeScript, Bun, Python, Go, Rust, Swift) follow one 8-step short path (§3), each step with an example that runs as written; Kotlin is not among them (see out of scope) |
| D2 | One example throughout, the template's `greeter`: the project `rutis-host new greeter` generates is step 2; steps 3–7 add using a `translate` service, a `greeting` config, emitting `greeter/greeted`, tests, and `dev` with a fake `translate`. The templates therefore become step 2 (the config moves out of the template into step 4) |
| D3 | Before step 7, a guide shows only the 7 concepts; everything else moves to plugin-api.md, cordis.md, nodes.md or rutis-host.md (the table in §4), leaving one line with a link |
| D4 | Each guide explains "row" itself: in step 7, with the two rows of `rutis.dev.json` |
| D5 | rutis-host.md opens for people assembling plugins: the quick start uses `rutis-host add` (#691), sees the service list after start and calls once with `rutis-host call` (#670), and `rutis-host status` (0.18); installing each language's plugins by hand stays as a reference table |
| D6 | The host owner's guide is a **new file** `docs/guide/open-host.md` (both languages), written around the 6 things a host does (§5); rust-host.md stays the API reference and gets one line at the top pointing to it. What #683 calls "a section 'Keep plugins in check' in rust-host.md" is section 3 of the new guide; #683 adds the options' reference rows to rust-host.md's tables |
| D7 | Every example in the guides is run by a check: commands and files in the plugin guides and the host owner's guide carry `tools/doc-examples.mjs` markers; Rust code is also compiled as doctests (as now). doc-examples gains `matches=<path>` (the block must equal that file in the project, which keeps guide and template in step) and code fences with attributes (` ```rust,no_run `); Swift's examples run on macOS |
| D8 | Public paths: remove the crate-internal glob imports at rutis-loader's root so they no longer take the root paths; the other names #685 lists already have public paths, and the guide uses those, locked by doctests |
| D9 | Doc comments in the release-train crates become English (files listed in §6), with a CI check |
| D10 | Implementation comes after the features it depends on: each section is written once its issue has merged (§9); acceptance is the #663 evaluation after 0.19 is released: plugin authors meet no more than 7 concepts before "it runs" |

## 3. The plugin guides' short path

### 3.1 Eight steps

TypeScript as the example; each step's code block is **the whole file** (the reader replaces it), marked `file=` for doc-examples; commands are marked `run`.

| Step | What it covers | Example | Concept | Waits for |
| --- | --- | --- | --- | --- |
| 1 Start a project | `npx @arcships/rutis-host new greeter --lang node`, `npm install`, `npm test` passes; only 4 generated files listed (plugin, test, `rutis.dev.json`, `package.json`) | commands | plugin | — |
| 2 Provide a service | The generated `src/index.ts`: `definePlugin`, `apply`, `ctx.provide('greeter', …)`, one line on `provides` (`check` knows who provides what without running). The block carries `matches=greeter/src/index.ts` | `greeter.hello(name)` | plugin, service | #692 |
| 3 Use a service | `inject: ['translate']`, `ctx.use('translate')`; one sentence: it starts when all are ready | `hello(name, lang)` calls `translate.text(greeting, lang)` | `inject` | #692 |
| 4 Config | `default` in the Schema; `apply` receives the config with defaults filled and checked; where config comes from: `config` in tests, the host's row at run time (step 7) | `greeting`, default `"Hello"` | config | 0.18 #669 |
| 5 Emit and listen to events | `ctx.emit('greeter/greeted', name)`; one `ctx.on(name, listener)` example; one line: "the other four ways, filters and scopes: Plugin API" | emitted on every greeting | events | — |
| 6 Test | `load(plugin, { config, services })`, `t.service`, `t.on('greeter/greeted')`, `t.unload`; one sentence: when you write for a host that ships strict fakes, pass those as `services` | a fake `translate` | testing tool | #670 (`t.on`), #684 (that sentence) |
| 7 Run locally and try it | `rutis-host dev`; the two rows of `rutis.dev.json`: the row with the plugin's id is the plugin itself (its `config` is what the plugin receives), the other is a fake `translate` running alongside (written in the same language); the service list `dev` prints; `rutis-host call greeter.hello '"Ada"' '"sv"'`; change the code once and see `reloaded` | `dev/fake-translate.ts` | `dev` and rows | #670 |
| 8 Publish | A package name of your own, the tag workflow, one line on the dependency range (link to Plugin API "Versions"), one line "how a host installs your plugin" (link to rutis-host.md) | commands | — | #691 (the link says `add`) |

After the path each guide has only: "Debugging" (added by #679), what is specific to the language (Swift's "System permissions", Bun's "Runtime conventions"), and one line "Advanced: Plugin API".

### 3.2 Names met before step 7 (TypeScript)

| Name | Concept |
| --- | --- |
| `definePlugin`, `apply` | plugin |
| `ctx.provide`, `provides` | service |
| `inject`, `ctx.use` | `inject` |
| `config` (JSON Schema), `default` | config |
| `ctx.emit`, `ctx.on` | events |
| `load`, `t.service`, `t.on`, `t.unload` | testing tool |
| `rutis-host dev`, rows of `rutis.dev.json`, `rutis-host call` | `dev` and rows |

Not shown: cleanup functions and `ctx.effect` (the template does not need them; Plugin API "Lifecycle"), `'sync'`, `ctx.signal`, plugin definitions, `host()`, `serial` / `waterfall` / `parallel` / `bail` / `filter`, scopes, strict-mode details, the plugin API version, Cordis. Each language's implementation PR carries the same table, checked in review.

### 3.3 Differences between languages

Every language takes the same 8 steps with the same example, differing only here:

| Language | Differences |
| --- | --- |
| Bun | Commands become `bunx --bun`, `bun install`, `bun test`; otherwise as TypeScript (the guide still has all 8 steps, so readers do not jump between two guides) |
| Python | Step 2, one sentence: `async def` is an async method; step 8: the entry point name is the host's row name |
| Go | Step 1, one sentence: one binary is one runtime process in the host; step 2: methods are lower-cased for other languages and must allow concurrent calls; step 3: the struct of function fields, **pass ctx down**; step 4: fields not written are zero values, defaults are given in `Apply` (0.18's Go rule); step 7: the fake `translate` is Go code, in the form #670 item 3 decides |
| Rust | Step 1: `cargo new --lib greeter` plus dependencies (no template); step 2: `#[rutis::service]`; step 3: `type Deps = Arc<…>`; step 4: `fn new(config: &Config)`, defaults with serde's `default`; step 5: the event type and emitting from a service method (keeping the plugin's `Ctx`); step 6: a root in `#[tokio::test]`; step 7: `examples/try.rs` is a minimal Rust host that builds a root, adds the plugin and a fake `translate`, and calls once with `call_service`: `cargo run --example try`; step 8: `cargo publish`, the host adds it with `Builtins::register` (dylib: link to the external plugin guide). Rust's seventh concept is therefore "the host adds the plugin", not "rows" |
| Swift | Step 1: `rutis-host new greeter --lang swift`, `swift test`, trusting the SDK's macros once; step 7: `dev` builds first; "System permissions" stays after the path |

### 3.4 Templates

The node, bun, python, go and swift templates of `rutis-host new` become step 2: they only provide `greeter.hello(name)`, with its test, and `rutis.dev.json` has the single row `{ "id": "<name>" }`; comments in the code point to steps 3, 4 and 5 of the guide. The service is still `greeter` whatever the project's name (as now). The templates and the guides change in the same PR; `matches=` keeps them equal.

## 4. What moves where

| Now in | Content | Moves to |
| --- | --- | --- |
| TS §2, Python §2, Go §2, Swift §2 | Details of `provides` (`'sync'`, inference when absent, `--strict`) | One line in step 2; the rest to Plugin API "Declarations" |
| TS §2, Python §2, Go §2, Bun §2, Swift "Handing a plugin to a service", Rust "Plugin definitions" | A plugin is itself a value | Plugin API "Plugin definitions" (the general rules are there; each language's form joins that section) |
| TS §3, Python §3, Go §2, Bun §2, Swift §2, Rust §2 | The five ways to emit, `filter`, `prepend` / `global`, `once`, prefix listeners, scopes, unloading and cancellation, `on_sync`; each language's greeter / shout example; Python's cross-language example (`sessions` and `audit`) | Plugin API "Events", one subsection per language; step 5 keeps only `emit` / `on` |
| TS §3, Python §3, Go §3, Swift §3 | Strict mode, `t.methods`, Go's `InScope` / `Update` / loading a definition, Swift's `load(config:row:scope:)` | Plugin API "Testing tools", plus a table of each language's testing-tool interface (including the Python testing tool's, the documentation #670 item 4 asks for) |
| Go §2 "Key points", Swift §2 "Key points" | Method signatures, sync or async, values, errors, concurrency, cancellation, the call chain | One necessary line each in steps 2 and 3 (see §3.3); the rest to Plugin API "Services", "How values are passed", "Errors" (a Go and a Swift column) |
| Swift §2 value table | Swift's values | Removed: Plugin API "Ten kinds of values" already has a Swift column |
| Rust's opening name table and `Ctx` method table, §2's "three things" and "points" table, `provide_as` / `get` / `Option` deps / the `Deps` table, "Cleanup and background tasks", "Errors" | Rust's advanced forms | A new "Rust" section of the Plugin API; plugin-api.md is therefore compiled as a doctest of the `rutis` crate too (as rust-plugin.md is) |
| TS, Python, Go, Swift §5, §6 | Why the dependency is `^` or a lower bound; how a host installs (npm, uv, `go add`, the directory, on-demand start) | One line each in step 8, linking Plugin API "Versions" and rutis-host.md "How a host installs plugins" |
| Go §7, Bun §6, Swift §8 | Running on another machine | nodes.md "Language runtimes on other machines" (moved after #698 rewrites the remote config) |
| Bun §3 | The host's `rutis.json` and the `runtimes.bun` fields | rutis-host.md `runtimes` (already there); removed from the guide |
| TS "Existing Cordis plugins" | Cordis packages as rows | cordis.md |
| rutis-host.md opening | "Plugin authors use it to develop locally" | Rewritten for people assembling plugins; the `dev` section stays as reference, linked from the plugin guides |

Not moved: all of rust-host.md stays (it is the embedding API reference); Swift's "System permissions" and Bun's "Runtime conventions" stay after the path in their guides.

The guide indexes in docs/guide/README and the root README are grouped by layer: writing plugins for a host (six languages), assembling plugins with rutis-host, building a host open to plugins (new), reference (Plugin API, linking nodes, Cordis, embedding in Rust, app hosting).

## 5. The host owner's guide "Build a host open to plugins"

`docs/guide/open-host.md` / `open-host.en.md`. Readers: building rutis into their own Rust product and opening its plugin capability to other developers. One example throughout: `jot`, a command-line note tool; the host provides a `notes` service and opens one extension point, `commands.register(name, handler)`; third parties write commands in TypeScript or Python.

| Section | What it covers | Example | Names used | Waits for |
| --- | --- | --- | --- | --- |
| 1 Integrate | Dependencies; a root; providing the host's own services; adding plugins with the loader (Node and Python runtimes); waiting until ready (`reconcile` returns once every row's `apply` has run once; wrap a stuck plugin with `tokio::time::timeout`, then look at the status); shutting down (`shutdown_with_timeout`, then `kill_processes()` for what is left). One table on what each loader part does: `Chain` asks in order who knows a row name, `with_shared` lets `RuntimeRowsPlugin` use a runtime's resolver too, `RuntimeResolver` is the row names one runtime knows, `RuntimeRowsPlugin` resolves a runtime's rows again after it restarts, `ServiceCatalog` is the table of service names, `Layer` / `Patch` are where rows come from. What paths are relative to, `python_path` for Python plugins that are not installed, environment variables for one runtime | `jot`'s `main.rs` and a TS command `wordcount` | `Ctx::root`, `rutis::host::*`, `LoaderPlugin`, `Chain`, `Builtins`, `ServiceCatalog`, `Layer`, `Patch`, `rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin}`, `rutis_bridge::runtime::{LocalRuntime, Launcher}`, `rutis_bridge::transport::local::kill_processes` | 0.18 (default output, `status()`) |
| 2 Define the plugin surface | `Notes` with `#[rutis::service]`; callback registration `Commands::register`: the handler comes by reference, the `&Ctx` parameter is the calling row, registrations hang on it and go away when the plugin unloads; `caller_row()` says which plugin it is; `rutis-host types` generates TypeScript and Python types for plugin authors, with the host's names in the header and error types | `Commands`, `Notes`, the generated `jot.d.ts` / `jot.py` | `rutis::service`, `rutis::host::BoxFuture`, `rutis_bridge::session::caller_row` | #684 |
| 3 Keep plugins in check | A runtime process per plugin; call time limits and cancellation; the hello time limit (off while debugging); limiting which services a plugin may use; what host and plugin author see on a timeout or a refusal | `jot` gives each third-party command its own process, a 5-second limit, and only `notes` and `commands` | the options #683 defines | #683 |
| 4 Manage plugins | Installing, upgrading, removing plugins (without knowing their language); adding and removing rows, changing config (the loader's imperative edits and persistence); how a row is doing now | `jot install <plugin ref>`, `jot remove` | #691's embedding API, `rutis_loader::edit`, `Loader::status()` | #691 |
| 5 Serve your plugin authors | What to hand plugin authors: a template (the host's own, with the generated types and strict fakes), a development loop (`--plugin <dir>` using `RuntimeResolver::invalidate_all` and `Loader::reload`), hints by error code on failure (`rutis_bridge::thrown()` gives name, code, site) | `jot`'s TS template and its test | #684's strict fakes, `thrown()` | #684, #670, 0.18 #677 |
| 6 See the state | One place showing which rows run, what they wait for, runtime processes, calls in flight and their callers; listen to `rutis/row`, `rutis/runtime` to act on changes | `jot status` prints `Loader::status()` | `Loader::status()`, `RuntimeHandle::state()`, `RowEvent` | 0.18 #678 |

- Each section's Rust code is both a doc-examples `file=` block (section 1 writes `Cargo.toml` and `src/main.rs`, then `cargo run` once and sees `wordcount` run) and a doctest: both open-host.md files are included by `rutis-loader`'s `lib.rs`, as rust-host.md is.
- Section 5's TS template and test are run by doc-examples too: a third party writes `wordcount` from the template and runs `npm test` against the strict fakes generated in section 2.
- Only what the task needs; each name links, where it first appears, to its reference entry in rust-host.md, with parameter details in one place only.

## 6. Small code changes

**Public paths** (D8). Where the names #685 lists are now:

| Name | Now | The guide writes |
| --- | --- | --- |
| `EntryStatus`, `EntryInfo`, `RowStatus` and the other names under `report` | Public under `rutis_loader::report::`; importing from the root fails with E0603 (the root's `pub(crate) use crate::report::*` takes the name) | **Change**: remove the root's `pub(crate) use …::*` for `report`, `edit`, `resolve`, `internal`, `runtime`, `peer` and the like; the crate's own code uses the module paths. The public API is unchanged (`public-api.txt` unchanged) |
| `Loader::entries`, `waiting_for`, `settled`, `status` (0.18) | Methods of `rutis_loader::Loader` | as is |
| `Snapshot`, `FiberState`, `FiberView` | `rutis::host` | as is |
| `LocalRuntime::python_path`, `Launcher::env`, `RuntimeState`, `RuntimeHandle` | `rutis_bridge::runtime` | as is |
| `caller_row`, `CallerRow`, `call_service` | `rutis_bridge::session` | as is |
| `Resolver` and the types for writing a resolver | `rutis_loader::Resolver`, `rutis_loader::resolve` | as is |
| `kill_processes` | `rutis_bridge::transport::local` | as is |
| `RuntimeResolver`, `RuntimeRowsPlugin` | `rutis_loader::runtime` | as is |

Only the first row needs code. From then on doctests guarantee it: every `use` line in the guide must compile.

**English doc comments** (D9). In the release-train crates (`rutis`, `rutis-macros`, `rutis-bridge`, `rutis-loader`, `rutis-host`, `rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta`, `rutis-dylib-launcher`, `rutis-migrate`), files whose `///` or `//!` comments contain Chinese (test files excluded):

- `rutis`: `src/fiber.rs` (112 lines), `registry.rs` (32), `host.rs` (21), `ctx.rs` (20), `effect.rs` (18), `key.rs` (17), `plugin.rs` (16), `error.rs` (12), `lib.rs` (7), `internal.rs` (7), `events.rs` (1), `future.rs` (1)
- `rutis-bridge`: `src/identity.rs` (2), `access_point.rs` (1), `runtime/testing/events.rs` (1)
- `rutis-loader`: `src/runtime/connections.rs` (1)
- `rutis-host`: `src/languages/mod.rs` (1)
- `rutis-dylib`: `src/windows.rs` (1)
- `rutis-dylib-launcher`: `src/main.rs` (1)

They become English, describing what the code does now (old design references such as "§四" or "D24" go). The CI lint job gains a check: no Chinese in `///` / `//!` under these crates' `src/` (plain `//` comments are not checked). The implementation counts again against main at that time.

**The doc-example check** (D7). `tools/doc-examples.mjs`: a `matches=<path>` marker (relative to the example's root like `file=`; compared, not written; a mismatch prints the difference and fails); code fences may carry attributes (` ```rust,no_run ` counts as `rust`). `tests/e2e/tests/doc_examples.rs`: carries out `matches=`; examples whose name starts with `swift` run only on macOS. install.yml's "the guides' examples" step also runs on `darwin-arm64` besides Linux (Swift's examples only). Both plugin-api.md files are included as doctests of `rutis`.

## 7. What developers see

**A TypeScript plugin author following the guide** (the formats of `dev`'s service list and of `call` are #670's; written here as it means them):

```text
$ npx @arcships/rutis-host new greeter --lang node && cd greeter && npm install && npm test
# tests 1  pass 1
(in step 3, with ctx.use written but inject forgotten, the test fails with today's text:)
service translate is not available to this plugin, which does not declare it in inject: add 'translate' to its inject
(passes once it is in inject; step 6 asserts with t.on that greeter/greeted arrived)
$ npx rutis-host dev
greeter: running
translate: running
(service list: greeter provided by greeter, translate provided by translate)
$ npx rutis-host call greeter.hello '"Ada"' '"sv"'
"Hej, Ada!"
(save a change to src/index.ts)
greeter: reloaded (src/index.ts changed)
```

**When guide and template drift apart** (what maintainers see; doc-examples check after a release):

```text
docs/guide/typescript-plugin.md:41: the block does not match greeter/src/index.ts
-  provides: { greeter: { hello: 'async' } },
+  provides: { greeter: { hello: 'sync' } },
```

**A Go plugin author, step 7**: the fake `translate` in `rutis.dev.json` is Go code in the project; `rutis-host dev` builds and runs it, with no Node or Python (in #670's form).

**A host owner**:

```text
$ cargo build      # 0.17: use rutis_loader::EntryStatus;
error[E0603]: enum import `EntryStatus` is private
$ cargo build      # 0.19: the same line
error[E0432]: unresolved import `rutis_loader::EntryStatus`
help: consider importing this enum instead: rutis_loader::report::EntryStatus
```

(The second `help` is rustc's own; implementation step 1 checks it once.)

After section 1 of the guide, `cargo run -- wordcount one two three` prints `3 words`; `jot status` prints 0.18's `Status` `Display` (rows, services, runtimes, calls in flight, [diagnostics](design-diagnostics-2026-10-26.en.md) §5.1). The text when a third-party command calls a service it is not allowed, or a call times out, is #683's; the guide quotes it as is, and it follows diagnostics §4.5 (no rutis, binding, internal ids).

## 8. Compatibility and migration

- The plugin API, the config format and behaviour do not change; nothing goes into "Migrating to 0.19".
- Templates becoming step 2 affects only new projects.
- Removing the `pub(crate)` imports at rutis-loader's root does not change the public API; importing those names from the root never compiled.
- The guides' names follow #692 to the names at each SDK's root; hints for old imports belong to #692 and "Migrating to 0.19".

## 9. Implementation

| Step | Content | Needs | Done when |
| --- | --- | --- | --- |
| 1 | §6's public-path change and English doc comments, with the CI check | nothing; can go first | `use rutis_loader::EntryStatus` no longer reports E0603; the listed files have no Chinese doc comments; the CI check runs in the lint job |
| 2 | doc-examples' `matches=` and fences with attributes; `doc_examples.rs` supports them and runs Swift on macOS; plugin-api.md included as a doctest | nothing | `node tools/doc-examples.mjs --check` passes; a deliberately different example is caught by `matches=` (a test of the tool) |
| 3 | plugin-api.md takes the advanced content of §4 (per-language subsections, a Rust section, the testing-tool interface table) | #692 (the new home of advanced names), 0.18 merged | everything §4 marks "Plugin API" is in plugin-api.md, both languages agree, the Rust examples compile |
| 4 | The six plugin guides rewritten per §3, templates changed with them, what follows the path trimmed per §4; README and guide/README indexes grouped by layer | steps 2 and 3; #670 (`call`, the service list, `t.on`, fakes in the plugin's language); #698 (before moving "another machine") | each guide has 8 steps, in both languages; every code block has a doc-examples marker (doctests for Rust), step 2 has `matches=`; each language carries a name table like §3.2 |
| 5 | rutis-host.md's opening rewritten per D5 | #691, #670 | the quick start uses `add`, sees the service list, calls once and runs `status` once; examples marked |
| 6 | open-host.md in both languages per §5: sections 1 and 6 after 0.18 merges; sections 2 and 5 after #684; section 3 after #683; section 4 after #691 | steps 1 and 2 and each section's issue | all 6 sections; the Rust code is doctests and one doc-examples run passes `cargo run`; every name used can be imported from the path the guide writes; #691's "host owner's guide in both languages, in step" item is met by section 4 |
| 7 | Acceptance | 0.19 released | in #663's 0.19 evaluation record: plugin authors in each language meet no more than 7 concepts (plugin, service, `inject`, config, events, testing tool, `dev` and rows) before "it runs" (step 7); host owners integrate and open their host without reading crate sources. If exceeded, the guides are fixed in 0.19.x |

#671's "Done" items: 1 and 2 → steps 4 and 3; 3 "examples run by the doc-example check" → steps 2, 4 and 6; 4 "no more than 7 concepts" → step 7. #685 merged in: "the guide in both languages, examples compile and run" → step 6; "names usable from public paths" → steps 1 and 6; "doc comments in English" → step 1.

## 10. Self-review: is it too heavy?

- A new file for the host owner's guide: #685 asks for a guide written around tasks; putting it into the 658-line rust-host.md would mix reference and tutorial. Kept.
- `matches=`: used by step 2 and the templates (the 0.17 evaluation's A1; #668 fixed it once and it would drift again). Kept; no general "snippet comparison".
- Swift on macOS: otherwise #671's "every example is run by the check" does not hold for Swift. Only Swift's examples run there; the other languages do not run again on macOS.
- plugin-api.md as a doctest: Rust's advanced examples must keep compiling after the move. Kept.
- The CI check for Chinese comments: a grep-sized check so new comments do not come back in Chinese. Kept.
- Dropped: re-exporting `Processes` from `rutis_bridge::runtime` (the guide uses `kill_processes()` and does not need that type's name); a template or `dev` for Rust; covering Swift / Kotlin apps and products running on rutis-host in the host owner's guide; a separate example crate per section (doc-examples writing the files and running `cargo run` is enough).
