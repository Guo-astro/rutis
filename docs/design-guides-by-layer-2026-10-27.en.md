# Guides by layer: a short path for plugin authors, "Build a host open to plugins" for host owners

[中文](design-guides-by-layer-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: main `103c0d22`. Scheduled: 0.19.
Issue: [#671](https://github.com/arcships/rutis/issues/671) (with [#685](https://github.com/arcships/rutis/issues/685) merged into it). Part of [#663](https://github.com/arcships/rutis/issues/663) (experience requirements 2 and 3; A4, H2, O1, O5).
Relation to 0.18: the guides describe the behaviour after 0.18 — config gets its Schema defaults ([config Schema](design-config-schema-2026-10-27.en.md)), error text and `Loader::status()` / `rutis-host status` ([diagnostics](design-diagnostics-2026-10-26.en.md) §4.5, §5), and the "Debugging" section each guide already has (#679). This design changes none of that.

This design fixes only **the structure of the guides and a few necessary code changes**. Each section's concrete examples, name lists and the files to change are settled in the PR that writes that guide, and checked in review against the rules here.

Out of scope:
- The APIs and output text of other 0.19 issues: `rutis-host call`, the service list, the test kits' `t.on` and fakes in the plugin's own language (#670), the contract description and Python types (#684), the "keep plugins in check" options (#683), the install command and API (#691), the names at each SDK's root (#692), remote configuration (#698). This design refers to them and does not restate them.
- A Kotlin plugin guide: Kotlin plugins are hosted only by Android apps; there is no `rutis-host new` or `dev`. `kotlin-app.md` and `swift-app.md` are unchanged.
- This version of the host owner's guide covers Rust embedding only; #663 has not evaluated products that use `rutis-host` as their runtime, or Swift / Kotlin apps as hosts.
- A template or `dev` for Rust plugins; separate example crates for the guides; a CI check on doc comments; a `matches=` comparison marker (see D7).
- Doc comments in `rutis-agent`, `rutis-cli` and `aimux-llm`: example projects, not on the release train.

## 1. Where things stand

**Plugin authors.** In the 0.17.0 evaluation a plugin author needed only 7 concepts for the common tasks: plugin, service, `inject`, config, events (`on` / `emit`), test kit, `dev` and rows. The current guides cover plugins as values, the five ways to emit, filters, scopes and cancellation before "it runs"; the example is `weather` while the generated project is `greeter`, so pasting guide code into a generated project breaks the template's test (#663 A1); "rows" get one sentence in the TypeScript and Python guides and none in the Go guide; only the TypeScript and Python guides' examples are run by the doc example check.

**Host owners.** In the 0.17.0 host owner evaluation (deskmate), the names #685 lists could only be found by reading crate source, and the loader's parts were "copied without understanding". rust-host.md is a reference organised by API; there is no guide written around the task "build a host open to plugins". `use rutis_loader::EntryStatus` fails with `E0603`: crate-internal imports such as `pub(crate) use crate::report::*` at the crate root occupy the root path. The published crates have about 280 lines of Chinese doc comments, mostly in `rutis/src/fiber.rs`.

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | The six plugin guides — TypeScript, Bun, Python, Go, Rust, Swift — follow one 8-step short path (§3), with one example per step that runs as written |
| D2 | One example throughout, the template's `greeter`: `rutis-host new greeter` generates step 2; later steps add using a `translate` service, events, tests, `dev` and a fake `translate` |
| D3 | Before step 7 only the 7 concepts appear, using only the root names #692 fixes; everything else moves to plugin-api.md and the other reference docs (§4), with a one-line link left behind |
| D4 | Each guide explains "rows" in step 7 with the two rows of `rutis.dev.json` |
| D5 | rutis-host.md opens for people assembling plugins: a TypeScript + Python quick start (with #691's install command, #670's service list and `call`), how each language's plugins are installed (moved from the plugin guides), a minimal `rutis.json` |
| D6 | The host owner's guide is a new file, `docs/guide/open-host.md` (both languages), written around #685's six tasks (§5), with the command-line note tool `jot` as its example; rust-host.md stays the API reference and gets a pointer to it at the top |
| D7 | The existing checks run the examples: TypeScript, Bun, Python, Go and Swift commands and files carry `tools/doc-examples.mjs`'s `run` / `file=` markers; Rust code compiles as doctests, as it does today. Two small changes only: doc-examples accepts code blocks with attributes such as ` ```rust,no_run `; Swift examples run on macOS |
| D8 | Public paths: remove the crate-internal glob imports at rutis-loader's root; the other names #685 lists already have public paths, and the guides use those |
| D9 | Chinese doc comments in the release-train crates become English |
| D10 | Each section is written after the issues it depends on merge; acceptance is the #663 evaluation after 0.19 ships |

## 3. The plugin guides' short path

### 3.1 Eight steps

| Step | What it covers | Depends on |
| --- | --- | --- |
| 1 Start a project | `rutis-host new greeter`, install dependencies, run the test | — |
| 2 Provide a service | The generated plugin file: defining the plugin, `provide`, one sentence on `provides` | #692 |
| 3 Use a service | `inject: ['translate']`, `use`; one sentence: "it starts once all are ready" | #692 |
| 4 Config | The template's existing `greeting`: the Schema's `default`; `apply` gets config with defaults filled in; where config comes from (passed in tests, the host's row at run time) | 0.18 #669 |
| 5 Emit and listen to events | One example each of `emit('greeter/greeted', …)` and `on`; one line linking the plugin API for the other ways | — |
| 6 Test | `load`, getting the service, listening to the event, unloading; a fake `translate`; one sentence on using a host's contract description (as #684 defines) | #670, #684 |
| 7 Run it locally and try it | `rutis-host dev`; the two rows of `rutis.dev.json`: the row with the plugin's id is the plugin itself, the other is a fake `translate` in the same language; try it once with #670's service list and `rutis-host call`; change the code and see it reload | #670 |
| 8 Publish | Package name, the tag workflow, one line on dependency ranges; one line "how a host installs your plugin" linking rutis-host.md | #691 |

After the path, each guide keeps only "Debugging" (#679), what is specific to the language (Swift's "System permissions", Bun's "Runtime conventions"), and one line "Going further: the plugin API".

Each step's code block is the whole file and carries a doc-examples marker, so #663 A1 — guide code plus the template's test failing — fails the check.

### 3.2 Differences between languages

- **Bun**: the commands become `bun`'s; it still has all 8 steps, so readers do not jump between two guides.
- **Go, Swift**: they compile to a binary, and one binary is one runtime process in the host (one sentence in step 1); step 7's fake `translate` is written as #670 defines.
- **Rust**: no template and no `dev` (a Rust plugin is compiled into a Rust host). Step 1 is `cargo new --lib`; step 7 is a minimal Rust host under `examples/` that builds a root, adds the plugin and a fake `translate`, and calls it once; step 8 is `cargo publish`. Rust's seventh concept is therefore "the host adds the plugin", not "rows".

### 3.3 Templates

The templates keep the existing `greeter.hello` and the `greeting` config; only their comments change: they mention only things among the 7 concepts (no `'sync'`, `ctx.effect` and the like) and point to the guide's steps. Whether a template ships a fake `translate` is settled in the PR that writes the guides (#731 leaves this to this design; it uses #670's mechanism).

## 4. Where the advanced content goes

- Plugins as values, the other ways to emit, filters, scopes, cancellation, synchronous methods, strict mode, the rest of each language's test kit, Rust's advanced forms → plugin-api.md, at the advanced locations #692 fixes.
- Running on another machine → nodes.md (moved after #698 rewrites remote configuration).
- How a host installs each language's plugins, Bun's `runtimes.bun` → rutis-host.md (already there).
- Existing Cordis plugins → cordis.md.

Once Rust's advanced examples are in plugin-api.md, both copies of plugin-api.md are included by `rutis`'s `lib.rs` under `#[cfg(doctest)]`, as rust-plugin.md is, so they keep compiling.

The indexes in docs/guide/README.md and the root README are grouped by layer: writing plugins for a host, assembling plugins with rutis-host, building a host open to plugins, reference.

## 5. The host owner's guide "Build a host open to plugins"

`docs/guide/open-host.md` / `open-host.en.md`. Readers: building rutis into their own Rust product and opening its plugin capability to other developers. One example throughout, `jot`: the host provides a `notes` service and opens one extension point, `commands.register(name, handler)`; third parties write commands in TypeScript or Python.

| Section | What it covers | Depends on |
| --- | --- | --- |
| 1 Integrate | Dependencies, a root, providing the host's own services, adding plugins with the loader, waiting until ready (`reconcile` waits for every row's `apply`; what to do when one is stuck), shutting down; what each loader part (`Chain`, `with_shared`, `ServiceCatalog`, `RuntimeResolver`, `RuntimeRowsPlugin`, `Layer`, `Patch`) does and why they combine this way; what paths are relative to, `python_path`, `Launcher::env`, a custom `Resolver` | 0.18 |
| 2 Define the plugin surface | Services with `#[rutis::service]`; callback registration (handlers passed by reference, `&Ctx` is the calling row, registrations go away when the plugin unloads); `caller_row()`; generating types and a contract description for plugin authors — explaining what #684 defines | #684 |
| 3 Keep plugins in check | Explains the options #683 defines: process layout, time limits, which services a plugin may use, and what host and plugin author see on a timeout or refusal | #683 |
| 4 Manage plugins | Installing, upgrading, removing plugins — explaining the install API #691 defines; adding and removing rows, changing config (`rutis_loader::edit`); how a row is doing now (`Loader::status()`). Plugins can manage plugins too: `inject: ['loader']` (#185), as with Cordis's `ctx.loader` | #691, #185 |
| 5 Serve your plugin authors | The template and fakes to hand plugin authors, using the contract description in tests (as #684 defines); a development loop (`RuntimeResolver::invalidate_all`, `Loader::reload`); turning errors into the host's own words (as 0.18 #677 defines) | #684, #670, 0.18 #677 |
| 6 See the state | Which rows run, what they wait for, runtime processes, calls in flight and their callers: `Loader::status()`; listen to `rutis/row`, `rutis/runtime` to act on changes | 0.18 #678 |

- The Rust code compiles as doctests: both open-host.md files are included by `rutis-loader`'s `lib.rs`, as rust-host.md is. Section 1 also uses doc-examples to write `Cargo.toml` and `src/main.rs` and `cargo run` once, so those blocks are written ` ```rust,no_run ` (D7).
- Sections 3, 4 and 5 only explain how to use what those designs define; fields and parameters link to the reference entries in rust-host.md / rutis-host.md and are not repeated in this guide.

## 6. Small code changes

- **Public paths** (D8): remove the `pub(crate) use …::*` for `report`, `edit`, `resolve`, `internal`, `runtime`, `peer` and the like at rutis-loader's crate root; inside the crate, use module paths. The public API does not change (`public-api.txt` is unchanged). From then on doctests make sure every `use` line in the guides compiles.
- **English doc comments** (D9): in the release-train crates' `src/`, `///` and `//!` comments containing Chinese become English, describing what the code does now, without the old design numbers. The files are listed against main at implementation time.
- **Doc example check** (D7): `tools/doc-examples.mjs` accepts attributes after a code block's language (` ```rust,no_run ` is treated as `rust`); `install.yml`'s "the guides' examples" step also runs on `darwin-arm64`, for the Swift examples only (Swift plugins run only on macOS).

## 7. Compatibility and migration

- No change to the plugin API, config format or behaviour; nothing in "Migrating to 0.19". Plugin authors write nothing extra.
- Templates change only in comments, which affects only new projects.
- Removing the `pub(crate)` imports at rutis-loader's root does not change the public API.

## 8. Implementation

| Step | What | Prerequisites |
| --- | --- | --- |
| 1 | §6's public paths and English doc comments | None |
| 2 | doc-examples accepts code blocks with attributes; Swift examples run on macOS | None |
| 3 | plugin-api.md takes the advanced content and is included as a doctest | #692, 0.18 |
| 4 | The six plugin guides rewritten per §3, template comments with them; indexes grouped by layer | Steps 2, 3; #670; #698 (to move "another machine") |
| 5 | rutis-host.md's opening rewritten per D5 | #691, #670 |
| 6 | Both open-host.md files: sections 1 and 6 after 0.18; 2 and 5 after #684; 3 after #683; 4 after #691 and #185 | Steps 1, 2 and each section's issue |
| 7 | Acceptance: in #663's 0.19 evaluation, plugin authors in every language meet at most 7 concepts before step 7; host owners integrate and open their host without reading crate source. If not, the guides change in 0.19.x | 0.19 ships |

#671's "Done" items: 1 → step 4; 2 → step 3; 3 "the doc example check runs the examples" → steps 2, 4, 6; 4 → step 7. #685: "both languages, examples compile and run" → step 6; "the names are usable from public paths" → steps 1 and 6; "doc comments in English" → step 1.
