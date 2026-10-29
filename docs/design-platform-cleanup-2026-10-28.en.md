# 0.19 platform cleanup: cut what does not belong to the platform, by responsibility

[中文](design-platform-cleanup-2026-10-28.md)

Status: final, not implemented. Date: 2026-10-28. Base: `main` `8071a794`. Scheduled: 0.19.
Issue: no separate issue; Refs [#694](https://github.com/arcships/rutis/issues/694) (release planning), [#771](https://github.com/arcships/rutis/issues/771) (0.19 implementation tracking). Source: the five-layer inventory list the user approved on 2026-10-28.
Relation to [Remote connection: keep only one session plugin](https://github.com/arcships/rutis/pull/811) (PR #811, file `docs/design-remote-session-2026-10-28.md`): everything about remote connection — link, access points, admission, revocation, registration, `Transport`, `PeerPlugin`, `RuntimeSession`, `RuntimeInstances`, `runtimes.remote`, the compat session format, switching each language's local mode to endpoint, the link-based implementations of `platform()` — is decided in #811. This design does not repeat it and only cites its section numbers and PR numbers (S1–S6, R1–R6, A1, A2). Where the two designs touch the same code, §13 of this design states which self-contained change it goes into and which comes first.

Out of scope: the frames of the session protocol; Cordis lifecycle semantics; the loader's row model, layering and `instanced` grouping; extending dylib.

Every item in this design was checked on `8071a794` for its definition site and its callers. Where the result differs from the approved list, the section's "Where verification differs from the list" states the facts and the handling; nothing is changed silently.

## 1. Responsibilities and boundaries

The platform has only five responsibilities:

1. **Lifecycle kernel**: plugins, dependencies, services, events, cleanup, including instances and scopes.
2. **Unification across boundaries**: the session protocol, values, errors, object references, one handover point (#811's session).
3. **Leaf SDKs for each language**: very small; they only let plugin authors write code in their own language.
4. **Desired state**: loader reconciliation.
5. **Visibility**: error structure, state snapshots. The platform only makes things visible; it does not make decisions for people.

The platform does not do: isolation and allocation for business logic; connection methods (#811 hands these to connection plugins); business contracts; agent features; long-term compatibility across versions.

Boundaries:

| Layer | What it is | Rule |
| --- | --- | --- |
| Platform | the `rutis` kernel (including `rutis-macros`), `rutis-bridge`, `rutis-loader`, the language SDKs and runtimes | only what the five responsibilities need |
| `rutis-host` and the tool crate `rutis-types` | reference host and tools | may only use the platform's public API; may not push features into the platform |
| Everything else | business | the platform does not cover for it |

Principles:

- A responsibility is justified when **our own design needs it**. Example: instance plugins are justified because we want reused instances to have a lifecycle too.
- What the business can solve, the platform does not cover for.
- No compatibility. Old forms are deleted directly, with no forwarding, old names, fallback branches or switches.
- Cordis compatibility and dsh compatibility are things we want: dsh must be able to install Cordis's dsh plugins. What is kept for it has its reason written down.

## 2. How items are judged

Each item on the list falls into one of these classes:

| Class | Test | Handling |
| --- | --- | --- |
| A Responsibility | belongs to one of the five in §1, and our own design needs it | stays in the platform; duplicates are merged into one |
| B Tool | needed by rutis-host or developers, but not a platform responsibility (how languages start, terminal output layout, restart policy, developer tools) | moves to rutis-host |
| C Cut | no production callers; or compatibility kept for old versions or old hosts; or duplicates another item; or the business can do it itself | deleted, together with its tests, docs and examples |
| User decision | explicitly kept by the user on 2026-10-28 | kept, with the reason in the relevant section: Cordis/dsh compatibility, dylib, synchronous `bail`, `FiberView::set_config`, `greeting` |

"No production callers" means no calls in `crates/`, `examples/`, `tests/e2e/` or any language's source apart from the defining file and tests; an interface used only by tests counts as C.

## 3. Kernel (`crates/rutis`, `crates/rutis-macros`)

### 3.1 Delete

| Name | Location | Callers (verified) | Class | Changed with it |
| --- | --- | --- | --- | --- |
| Service interceptors: `intercept_require_as`, `intercept_set_as`, `ServiceIntercept`, the hook table `ServiceInterceptors`, `ServiceReadFailure::Intercept*`, `ServiceWriteFailure::Intercept*` | `intercept.rs:21`, `:48–326`; `internal.rs:179,187`; `ctx.rs:1602–1616`; `error.rs:14–16,52–54` | No production callers. Tests: `tests/service_intercepts.rs` (the interception part), bridge `marked.rs:1055`, `tests/marked_services.rs:739`, `tests/projection_lifecycle.rs:132,336` | C | The `actor` parameter of `require_provided_as` exists only for interceptors (`ctx.rs:465–468`) and is deleted with them; the macro-generated `__rutis_resolve` (`rutis-macros/src/lib.rs:460–464`), `service::Resolve` and `Entry::resolve` (`service.rs:112–120,190–196`), and the signature in bridge `marked.rs:72–78` change accordingly. The write interception in `ServiceWriter::set` (`intercept.rs:387–402`) is deleted; the rest stays |
| `check()` predicate gating: `provide_checked`, `refresh`, `CheckFn`, `service::Check`, `Exposure::check`, `DependencyStatus::CheckPending`, `CheckRejected`, `CheckPanicked` | `registry.rs:10,36–37,355,385–393`; `internal.rs:133,162`; `ctx.rs:1552,1587`; `diagnostics.rs:191–193`; `service.rs:460–462,493–495` | Production: only bridge `marked.rs:219–223`, which forwards check to the by-name entry; its only possible source is `provide_checked`, and production has no source. The protocol and the six SDKs have no concept of check | C | The list only named `CheckPending`; the other two variants belong to the same mechanism and are deleted together |
| Dispatch observer: `EventBus::observe_dispatch`, `DispatchAttempt`, `DispatchObserver` | `bus.rs:37,45–50,88,504,531,571–632` | No production callers. Tests: `tests/dispatch_observation.rs` (whole file), 5 places in `typed_dispatch.rs`, `bus/transient_tests.rs:128`, bridge `tests/event_hub.rs:984` (used as a probe) | C | `on_selected_for_testing` of the `testing` feature (`bus.rs:668`) is not part of it and stays |
| `take_cleanup_errors` (three places: `CtxHost`, `Ctx`, `FiberView`) | `host.rs:70`, `ctx.rs:1076`, `fiber.rs:1684` | Only `tests/cleanup_errors.rs:20,23,66` | C | The underlying `drained_errors` (`fiber.rs:378`) is merged into the result on unload and stays |
| Cleanup-tree introspection: `FiberView::effects`, `EffectMeta`, `EffectPhase` | `fiber.rs:1667`, `effect.rs:26,35–48,147` | Only `tests/effect_tree.rs` | C | The Stuck hint reads the cleanup's label (`fiber.rs:157` → `effect.rs:141–145`, output at `fiber.rs:174`). The label now lives in `EffectMeta.label`; when `EffectMeta` is deleted it becomes a `label` field on `EffectRecord` itself; `effect_named` stays |
| `CtxHost::plugin_from` | `host.rs:239`, `ctx.rs:1421` | No production callers. Example `examples/development_workflow.rs:134`; tests `instance_subtrees.rs:411`, `lifecycle_order.rs:276`, `typed_plugin.rs:375`; guides `rust-host.md:39,89`, `rust-plugin.md:615` (doctest) | C | See 3.4 item 1 |
| `EventPattern` | `key.rs:318–353`; `internal.rs:21` | No construction or use anywhere in the repo (`on_pattern` was deleted in 0.13) | C | — |
| `waterfall_sync` and the protocol mode `waterfall-sync` | `host.rs:248`, `ctx.rs:1430`, `events/api.rs:400,494–507,691–700`, `events.rs:123–146` (`Mode::WaterfallSync`), `bus.rs:933,1024,1038–1046,1122,1168`, `bus/run.rs:477–610`, `events/remote.rs:113,151–200` | No emitter in Rust production code. Example `examples/sync_decision.rs` (the whole file only demonstrates it), `benches/events.rs:79–83`, loader test `rust_participant.rs:407–409` | C | Receiving branches: bridge `events/hub.rs:1160,1254` (missing from the list), `events/link.rs:495,578` (a file #811 rewrites), the test tool `runtime/testing/events.rs` and the two `waterfall-sync/*` cases in `events.expected.json`; the six SDKs' receiving branches are in §7; "six dispatch modes" in `lib.rs:38` becomes five. All 3 existing `unsafe` blocks in the kernel exist only for synchronous next (`bus/run.rs:589`, `events/remote.rs:163,197`) and go away with it |
| `get_key`, `require_key`, `provide_key` in `internal` | `internal.rs:99,104,122`; `ctx.rs:1488,1521,1543` | Production: the remaining bridge `marked.rs:172,223`, `runtime/rows.rs:317`, the rest of `services.rs`; loader `catalog.rs:120,125`, `runtime.rs:878,1008`, `runtime/rows.rs:122`; kernel `deps.rs:63,97`; the others are in files #811 deletes | C (duplicates `*_at`) | Use `Key::of/named/dynamic(..).instance(..)` with `get_at`/`require_at`/`provide_at`. Callers change to holding a `Key<T>`: `RuntimeRows::key() -> TypeKey` (`rows.rs:51`, public) and the closure signatures in `catalog.rs` change accordingly. Kernel `deps.rs` switches to crate-internal methods |
| Wire error name `"CordisError"` | `service.rs:1024` (`FailCordis`) | Generator `build/exports.rs:479–491,1856–1858,1876`; generated files `examples/rust-services/rows/services.d.ts`, `examples/marked-services/rows/services.{d.ts,py}`; test `marked_services.rs:274`; guide `rust-host.md:145,718` | C | Renamed to `"Error"`; `code` is still the `ErrorKind` variant name; the generated type `CordisErrorThrown` becomes `ErrorThrown`. Reason: the 0.18 [diagnostics design](design-diagnostics-2026-10-26.en.md) §4.5 says text shown to developers uses only what developers wrote, and developers write `rutis::Error`. The generator's recognition of the old path `rutis::CordisError` is deleted too. The six SDKs and specs do not have this name |
| The `anyhow` feature, `from_anyhow`, `From<Error> for anyhow::Error` | `Cargo.toml:22–23,26`; `error.rs:374–379,439–444` | Only `tests/author_api.rs:227–245`; CI `ci.yml:493–495` runs it specifically; no Cargo.toml turns it on | C | The guide gives the replacement `.map_err(\|e\| Error::msg(format!("{e:#}")))`. The code in the generator that recognises `anyhow::Result` is unrelated to this feature and stays (it moves to `rutis-types` with §8) |
| `try_filter`, `try_filter_named` | `events/api.rs` | Only loader test `rust_participant.rs:354` | C | `Ctx::filter` stays, see 3.3 |

### 3.2 Move

| Name | List | Handling after verification |
| --- | --- | --- |
| The loader's Schema validation `rutis_loader::config::check` (594 lines) | not listed (confirmed by the user on 2026-10-28) | **Moves into the kernel** as `rutis::config`; the kernel is the only copy of config validation, see 3.4 item 2 |
| The hint helpers in `internal::config`: `closest`, `distance`, `value_text`, `path_text`, `number_text`, `float_text` | move to loader | **Stay in the kernel**, moved out of `internal` and public in `rutis::config`. See 3.4 item 2 |
| Filter and scope descriptions `row`, `group`, `mark_row`, `mark_scope`, `CtxHost::scope`/`row`, `events::Scope`, `ListenerInfo.row/scope` | move to loader | **Not moved**, see 3.4 item 3 |

### 3.3 Keep

| Name | Location | Reason |
| --- | --- | --- |
| Synchronous `bail` (`Ctx::bail`, `Filtered::bail`) | `events/api.rs:487,668` | User decision: Cordis's `bail`, part of the paradigm; all six SDKs provide it |
| `FiberView::set_config` | `fiber.rs:1964` | User decision: Cordis's volatile fields change config in place. Usage chain (verified): loader `loader/reconcile.rs:968` and `volatile.rs` call it and emit the `loader/volatile-update` event; the dsh plugin `crates/rutis-dsh/dsh/aimux/src/index.ts:19,53` configures with `.volatile()` and listens to this event |
| dylib library generations: `Generation`, `loading`, `current_loading`, `set_generation`, `Ctx::set_code_generation`/`code_generation`, the `code_generation` field | `service.rs:211–285,304,510,523`; `ctx.rs:241–259`; `fiber.rs:408–410` | User decision: dylib is how we support dynamic Rust plugins (design philosophy: "dylibs are all plugins too"). User: `rutis-dylib/src/loader.rs:14,127,596` |
| `Ctx::filter` | `events/api.rs:676` | Cordis's `Context.filter`; all six SDKs provide it (Node `index.d.ts:24`, Python `events.py:544`, Swift `Ctx.swift:161`, Kotlin `Ctx.kt:88`, Go and Bun likewise) |
| `effect_named` | — | The Stuck hint uses its label; production user bridge `marked.rs:233` |
| `provide_mut_at`, `ServiceWriter` | `intercept.rs:331–447` | Used by bridge projection (`runtime/projection.rs:14,219,241`), does not depend on interceptors. Once only this remains in `intercept.rs`, the file is renamed `writer.rs` |
| `CtxInternal::named` | — | Has production callers (loader `reconcile.rs:998`, `runtime.rs:1243`, `binary.rs:2302`; bridge `runtime/rows.rs:312`) |

`DynPlugin::platform()` (`plugin.rs:113`, hidden flag, default false): not decided in this design. Verified: of the 23 implementations that return true, 14 are deleted with #811; after #811, 6 real implementations remain (bridge `RuntimePlugin`, loader `LoaderPlugin`, `RuntimeRowsPlugin`, `BinaryRuntimes`, `BinaryRuntime`, rutis-host `Released`), and the kernel reads it in 7 places (`fiber.rs:135,1048,1492`, `fiber/waiting.rs:91`, `diagnostics.rs:68,107,167`). When to handle it: after #811's S5 is merged, review the remaining implementations in the same PR as §13 CL6, and keep or delete based on the implementations at that time.

`docs/design-rust-port.md` (v5, 285 lines, its header still says "draft"): 29 occurrences of `CordisError` in the text, and it says "bail deleted, four dispatch modes", which does not match the current state; but `guarantees.md:31,117` registers its §3 as the source of `core/D1–D31`, and the README and design philosophy link to it. Handling: do not delete; add a paragraph at the top: "This is a historical design from 2026-08; the current description is the module docs in `crates/rutis/src/lib.rs` and `docs/guarantees.md`."

### 3.4 Where verification differs from the list

1. **`plugin_from` is not a complete duplicate of `plugin_with` + `PluginFactory` and `Definition::new`**. The `FiberView` obtained from `Definition::new` plus `ctx.plugin(def.at(..))` cannot `update(config)` (`fiber.rs:500`, no factory); `plugin_with` requires a hand-written `PluginFactory`, and turning a closure into a factory's `build_fn` (`plugin.rs:241`) is hidden. Handling: delete `plugin_from` as the list says; hosts that need `update` implement the public `host::PluginFactory` (`host.rs:26`); `build_fn` is not made public and no other entry point is added. The two places in the guide and the doctest switch to the `PluginFactory` form.
2. **The hint helpers cannot move to the loader as a whole**: the kernel's `decode` itself uses `closest` and `value_text` (`config/decode.rs:11,105,111,126,214`), `ConfigIssue::at` uses `path_text` (`config/mod.rs:108`), and the loader depends on the kernel, so the kernel cannot call the loader. Another fact: §7 keeps only one Rust copy of Schema validation, but the config of plugin values (`ctx.plugin(def.at(&ctx, config))`) goes straight to `rows.load` from the bridge (`runtime/process.rs:1220–1299`) without passing through the loader; if validation lived only in the loader, nobody would validate this path. The kernel is the common lower layer of loader and bridge, and `check` depends only on `serde_json` and the kernel's own `ConfigError`. Decision (confirmed by the user on 2026-10-28): `rutis_loader::config::check` moves into the kernel as `rutis::config`, next to `ConfigError` and `decode`; the kernel is the only copy of config validation, and config validation, filling defaults, decoding and error text exist only there; the loader's rows and the bridge's plugin values both call it. The hint helpers stay in the kernel, moved out of `internal` and public in `rutis::config` (`closest`, `distance`, `value_text`, `path_text`, `number_text`, `float_text`; loader `loader/waits.rs:439,441` and rutis-host `call.rs:107` use `closest` and `distance` to say "did you mean"). This way neither rutis-host nor the loader uses `rutis::internal::config` any more.
3. **`row`, `group`, `mark_row`, `mark_scope` do not serve only filter descriptions**: the kernel's listener descriptions (`bus.rs:333,336`), error text (`FilterFailed` at `bus.rs:1195`, `fiber.rs:604`), display names in Stuck and diagnostics (`fiber.rs:476`, `diagnostics.rs:60,164,262,284`), the `row` of `ServiceChanged` (`ctx.rs:815–816`) and the mount-range check of `Definition` (`definition.rs:117,151`) all use them; they are also part of the event protocol (the seventh argument `{scope, id}` of `rows.load` in [event protocol](design-events-protocol-2026-10-18.en.md) §3, `row`/`scope` in the descriptions in §4.2), and every language SDK's filter function reads exactly these fields. Handling: not moved, kept in the kernel; the loader is still what writes them (`loader/plugins.rs:97,132`). Reason: rows and scopes are values in the protocol and names in diagnostics, which belong to responsibilities 1 and 5. `Ctx::filter` stays (3.3); only `try_filter*` is deleted.
4. **Tests use `check()` as a hook for holding the gate**: `CheckedProvider` in `tests/load_window.rs:383–410` holds the gate with a blocking check, and 4 tests (`:456,:491,:631,:837`) are the evidence for guarantees `core/D6`, `core/D7`, `core/D14` and `loader/rows-gate-on-services`. Handling: these 4 tests hold the gate instead with a provider plugin that waits for a signal in `apply` and only then calls `provide`; no test hook is added. The check tests in `contract.rs` and `lifecycle_diagnostics.rs::cached_check_diagnostics_do_not_call_user_code` are deleted; `parity.rs:345` treats `refresh` as the counterpart of Cordis's `reflect.notify`, and its parity note is rewritten.
5. **About half of the interceptor test file tests `ServiceWriter`** (`service_intercepts.rs:219–700`): this part moves to a new file `service_writer.rs`, with the cases unchanged. The two projection tests (`a_proxy_held_during_reentrant_replacement_stays_callable`, `a_retried_publication_keeps_its_handle`) use write interception to create reentrancy and rejection: the first instead creates reentrancy with a real re-provide (another provider reenters in Drop); the rejection the second tests can only be created by an interceptor, so it is deleted with the interceptors, and the retry path in `projection.rs` is left with only one entry, a failing `provide_mut_at`, which the existing ownership-check tests cover.
6. **`set_config`**: dsh's Rust code does not call it directly; dsh's plugins reach it through the loader's volatile path (3.3 is already written to the facts).

## 4. rutis-bridge (non-remote parts)

### 4.1 Delete

| Name | Location | Callers (verified) | Class | Changed with it |
| --- | --- | --- | --- | --- |
| The reverse Cordis mount (a Cordis host mounting Rust plugins): `rutis_plugin` | `cordis/build/rust.rs:668` (whole file, 855 lines); re-export `cordis/build.rs:5–6` | Only `examples/native-mount/build.rs:9` | C | Opposite of "rutis is the host"; unrelated to dsh (dsh uses only `from_manifest`, `include_mounts!`, `npm_root`, `ROOT_VARIABLE`, `session::Error`); `examples/native-mount/README.md:12` already says "reverse direction (frozen, no new capabilities)" |
| `cordis::server::{serve, Dispatch}` | `cordis/server.rs` (106 lines, `#[cfg(unix)]`) | code generated by rust.rs; `examples/native-mount/tests/export_identity.rs` | C | — |
| `runtime/unix.rs` (`Connection::connect`, `connect_with`, 75 lines) | — | `cordis/server.rs:94`; test `tests/rpc_callbacks.rs:305` (evidence for guarantee `boundary/5.6`) | C | The test at `rpc_callbacks.rs:305` starts the Node runtime over a local pipe instead (test name unchanged) |
| The reverse-direction parts on the Node side: `Process.launch` in `client.mjs` (`:53–63`), `dispose()` (`:87–95`), the constructor's `executable` parameter; `io-worker.mjs:55–79` (the comment says "The frozen reverse direction") | `node/rutis-runtime/src/` | `test/fixtures/rust-mount.test.mjs` (146 lines), `bench/sync-call.mjs` (26 lines) | C | **`client.mjs` itself is not deleted**, see 4.4 item 1. The test and benchmark are deleted; `bench/sync-call.mjs` is replaced by the event benchmark in §7 |
| The parts of `examples/native-mount` that exist only for the reverse direction | `src/lib.rs` (84), `src/bin/rutis-counter.rs` (14), `tests/cordis_mount.rs` (22), `tests/export_identity.rs` (31), `build.rs:9–14` | — | C | Keep the main direction: `src/main.rs`, `tests/edge.rs`, `from_manifest` in `build.rs`; delete the reverse-direction line from the README |
| Negotiation kept for old runtimes: `events-v2` (`EVENTS_CAPABILITY` `rpc.rs:922`, `without_events` `:1294`, refusal `:2461,:2500`, `session/testing.rs:201`); `objects` (`relay.rs:41`, `object_refused` in `crossing.rs:171–175`); the self-check of `plugins` (`rpc.rs:927,2421`; `Endpoint::rust` always declares it, dead branch); the compatibility condition of `sync-wait` (`rpc.rs:1579,1838`); the empty declarations of `signals` (`rpc.rs:1245`) and `forwarding` (`rpc.rs:1247`, nobody checks them) | `session/` | Declarations and checks throughout the six SDKs (§7 7.2) | C | Guarantee `events/v2-required` is deleted; `events/old-peer-refused` becomes "refused by protocol version", depending on 4.4 item 9 |
| Old feature checks in the runtime mount layer: "A runner older than 0.3.0 reports no features" (`runtime/process.rs:1087`), `require_plugin_values` (`:1265`, called at `:1231,:1251`), the hard-coded "@arcships/rutis-runtime 0.3.0 or later" (`:1517–1524`), the old path when there is no `scopes` (`:1577`, `require("scopes")` `:1329,:1607`) | `runtime/process.rs` | — | C | The text of `check_rows` (`:1494`) is changed by #811 |
| `Connection::independent_future` | `session/rpc.rs:1650` | Only tests `tests/rpc_callbacks.rs:445,499`, neither in guarantees | C | `background()` and the `Background` executor stay (used at `relay.rs:55`) |
| Old process-starting entry points: `Process::launch`, `launch_observed`, `launch_group`, `launch_mount`, `attach` | `runtime/process.rs:832,842,855,867,932` | Only tests (cancellation, concurrent_launch, dsh_baseline, group_mount, rpc_callbacks, projection_lifecycle, error_shape, host_services, live_objects, service_projection, process_exit, session_matrix, event_hub) | C | These tests switch to the process-starting helpers in `rutis-test-support` (§13 CL12a); test names referenced by guarantees stay the same |
| `runtime/spawn.rs` (767 lines) | — | `Process::mount` (`process.rs:912–913`) | C (duplicates `transport/local/spawn.rs`) | Duplicated function by function: `Connect`/`Handover`, `CHANNEL_FD`, `inherit`, `dial_back`, `loopback`, `Child::watch`, `describe`, `peek_exit`, `HANG_GUARD`, `ended_with`/`Exit::wait`, `command`/`Spawn::command`, `traced`/`LocalTransport::trace`. Once `Process::mount` goes through #811's `pipe::spawn`, the whole file is deleted; processes dsh starts then also join the process group and the `RUNNING` registry |

`Process::mount` (`process.rs:891`) **stays**: its only production caller is the code generated from dsh's Cordis type bindings (`node/rutis-runtime/src/generate.mjs:996`), which uses `Mount { plugins, services, observer, hosts, ctx, ..Default }` and relies on the default `launcher: None` (Node). It is aligned with #811 S2b deleting compat (`Format::Compat` at `process.rs:919`), see §13.

### 4.2 Move to rutis-host

| Name | Location | Callers | Changed with it |
| --- | --- | --- | --- |
| Language knowledge: `Launcher::node/python/go/swift/bun` | `runtime/process.rs:333,350,369,377,388` | rutis-host `languages/node.rs:168`, `bun.rs:176`; tests | Placed together with #811's `rutis-host/spawn`. How dsh starts Node: see 4.4 item 2 |
| `spawn::node_inherits` (the compatibility branch that falls back to dial-back when an old Node package lacks `rutisChannels: ["fd"]`) | `runtime/spawn.rs:371` | `process.rs:339` | The dial-back branch is compatibility and is deleted; only the fd handover stays |
| `go_runtime_name`, `swift_runtime_name` | `runtime/mod.rs:70,78` | loader `runtime/go.rs:46`, `swift.rs:45` (row names); rutis-host `go.rs:234,338`, `swift.rs:177` | See 4.4 item 3: move to the **loader** (row names are a loader concept), not to rutis-host |
| Cargo features `node`, `python`, `go`, `bun`, `swift`, `kotlin` (`default = ["node"]`) | `Cargo.toml` | In src they only gate the language items above; 17 test files and CI use them for gating; `cordis = ["node", …]` | The bridge no longer has language features; `cordis` no longer depends on `node`; test gating: see 4.4 item 5. The list did not mention `kotlin` (comment "Tests only"); it is the same kind and is deleted too |
| Line-based output merging and line prefixes: `transport/local/output.rs` (`OutputStream`, `MAX_LINE`, `write_line`, `output_ended`, `prefixed`, `pipe`), `Stdio::Lines` (`transport/local/spawn.rs:59`), `Spawn.output_prefix` (`:103`), `Launcher.output_prefix` (`process.rs:226,323`) | — | rutis-host `outln!`/`errln!` (`lib.rs:19–38`, 93 uses), `host.rs:119–121`, `languages/mod.rs:345`, `binary.rs:184`; loader `binary.rs:1554,2306` | The bridge hands over only the raw pipe; rutis-host splits lines itself and adds the runtime-name prefix for Go/Swift/Kotlin binaries. The test paths of guarantees `host/output-lines-whole` and `host/output-prefix` change accordingly. Line prefixes for Node/Bun/Python are not moved, see 4.4 item 8 |

### 4.3 Keep

| Name | Location | Reason |
| --- | --- | --- |
| The Cordis type bindings and static mounts dsh uses: `cordis::build::from_manifest` (`cordis/build.rs:207`), `Bindings` (`:56`), `include_mounts!` (`cordis/mod.rs:44`), `npm_root` (`:35`), `ROOT_VARIABLE`, `generate.mjs` (1042 lines, called at `cordis/build.rs:151`), `session::{ObjectRef, RemoteFunction, JsError}` (`session/objects.rs:619,662,699`) | `cordis/`, `session/` | Cordis/dsh compatibility: dsh must install Cordis's dsh plugins. Within the bridge, `ObjectRef`, `RemoteFunction` and `JsError` are used only by the generated binding code. This design does not move `cordis` into a separate crate: it depends on `Process::mount` and `Projection`, and moving it out would require making both public to another crate, without reducing code |
| `sync-stack`, `reentrant-sync`, `leaf`, `check_rows`, `rows.v2`, `hosts`, `rows.announce` | `rpc.rs:1579–1581`; loader `runtime.rs:423,1202`; `plugin.rs:510`; `process.rs:1319,1451,1605` | In use: cycle detection for synchronous waits, pure-leaf runtimes, the row conventions (#811 keeps `check_rows`) |
| `events::report`, `headline` | `report.rs:109,161` | Used in loader production code (`loader/status.rs:389–391`), and also by rutis-host `status.rs:38–41`, `call.rs:187` |
| `events::report_line` | `events/errors.rs:122` | See 4.4 item 6 |
| `channel/trace.rs` and `RUTIS_TRACE` | — | See 4.4 item 7 |
| `greeting` (the greeting limit) | `runtime/local.rs:293` | User decision (a debugger with `--inspect-brk` needs the wait). Its carrier `LocalRuntime` is deleted by #811; for its new location see 4.4 item 10 |

### 4.4 Where verification differs from the list

1. **`client.mjs` cannot be deleted as a whole**: the main Node path uses its `Process.connect` (`runner.mjs:6,1073`), and so do test fixtures (`conformance-session.mjs:37`, `rpc-client.mjs:9` — the fixture for guarantee `boundary/5.6`, `sync-heartbeat.mjs:6`, `handshake.test.mjs:19`, `texts.test.mjs:95`). Handling: delete only the reverse-direction `launch`, `dispose`, the `executable` parameter and `io-worker.mjs:55–79`; the file stays.
2. **dsh's static mounts need a way to start Node**: the generated code calls `Process::mount(.., Mount{..Default})`, which by default goes through `Launcher::node` (`runtime/spawn.rs:304–318`); dsh does not depend on rutis-host. Handling: the Node command for Cordis static mounts goes into the bridge's `cordis` module (Cordis plugins can only run on Node; this is part of Cordis compatibility), and the general `Launcher::*` moves to rutis-host.
3. **`go_runtime_name` and `swift_runtime_name` are used by loader production code to determine row names and runtime names** (`runtime/go.rs:46`, `swift.rs:45`). Handling: move to the loader (row-name rules belong to desired state); rutis-host gets them from the loader's public API.
4. **Rust embedders that do not depend on rutis-host would lose the language launchers**: `rust-host.md:426–560` uses `LocalRuntime::node/python/go`; e2e `fixture-tenants` notes "assembled following the rust-host guide, without rutis-host", and so does `fixture-toolhost`; the examples' tests and about 20 loader tests use them too. `LocalRuntime` is deleted by #811. Handling: Rust embedders that need to start local language runtimes depend on the rutis-host library (#811's `rutis-host/spawn` plus the language startup this design moves there); the rust-host guide and the two e2e fixtures change to do this; the tests of loader, bridge and examples switch to the process-starting helpers in `rutis-test-support` (not published), without depending on rutis-host.
5. **After the language features are deleted, tests lose their toolchain gating** (17 bridge test files, about 20 in the loader, a dozen-plus commands in `ci.yml:534–1756`, `stress.yml:214,347,362`). Handling: tests that need a language are decided by `rutis-test-support` based on the environment variable `RUTIS_TEST_RUNTIMES` (comma-separated, for example `node,python`): if a listed language's toolchain is not found, the test fails and says what is missing; tests for languages not listed print one line "skipped: <language> not in RUTIS_TEST_RUNTIMES" and return. Each CI job sets it, replacing the current `--features`. This is a test convention, not a product switch.
6. **`report_line` is not moved**: the bridge's own `events::testing::Tap` (`events/testing.rs:64`, used by loader tests) uses it; it depends on the private `named`; the rust-host guide (`:658`) documents it for embedders, who use it to print one line when their event listener fails. It belongs to the same group of error text as `report` and `headline`, which is responsibility 5. Handling: it stays in the bridge's `events`, and rutis-host keeps calling it.
7. **`RUTIS_TRACE` does not serve only local debugging**: the rust-host guide (`:496,:574`) documents it for Rust embedders, and dsh's mounts also go through it (`runtime/spawn.rs:289–300`). The list's condition was "if it serves only local debugging"; the condition does not hold. Handling: it stays in the bridge and goes with #811's `pipe::spawn` (`pipe::spawn` is in the bridge).
8. **The line prefixes on the Node/Bun/Python runtime side cannot be deleted**: these three runtimes run several plugins in one process, and the prefix `[row name] ` is added according to "the row currently running" (Node `output.mjs:32–81`, Bun `runner.ts:143–147`, Python `_output.py:37–99`); a host reading the pipe cannot tell which row wrote a line. The bridge's `output_prefix` is only for Go/Swift/Kotlin, which run one plugin per process. Handling: the runtime-side prefix stays; rutis-host adds the runtime-name prefix only for binary runtimes, and sets `RUTIS_PREFIX_OUTPUT` so script runtimes add their own.
9. **The session protocol version rises to 4**: after the `events-v2` refusal is deleted, old peers can only be refused by protocol version. The endpoint format version `ENDPOINT_PROTOCOL = 3` (`session/mod.rs:18`) has not changed since 2026-10-06 (`ee5b605c`), and remote runtimes from 0.11–0.18 all speak 3; #811 §7 says "a 0.18 host or runtime: refused by protocol version at handshake", which in itself also requires 0.19's version to differ from 0.18's. Decision (confirmed by the user on 2026-10-28): `ENDPOINT_PROTOCOL` rises to 4 (WebSocket subprotocol `rutis.4`), landing in #811's S2b (the same PR that deletes compat). A host or runtime from 0.18 or earlier that connects is refused at handshake, without retry, and the text says which end to upgrade (the words of a real meeting, as in #811 §7): locally `incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end`; on WebSocket, at the subprotocol step, a 0.18 dialer prints `this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer`, and a 0.18 listener answers `this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (<its package>)`. This design's removal of negotiation (CL11a) is ordered after S2b.
10. **The carrier of `greeting` is deleted by #811**: it hangs on `LocalRuntime` (`local.rs:50,152,293`) and takes effect through `LinkConfig::local_runtime()` (`link.rs:140,154`, default 30 seconds) and `Retry::handshake`; all three are on #811's deletion list, and #811 gives no new location. Handling: `greeting` becomes a Rust construction option of `rutis-host/spawn` (default 30 seconds), available only for Rust embedding as now; no field is added to rutis.json. Same PR as #811 S4.
11. **The `plugins` capability**: Rust's self-check (`rpc.rs:2421`) is a dead branch and is deleted. The six SDKs' "only send plugin values to peers that declared `plugins`" is not old-version compatibility; it is the content of guarantee `bridge/plugin-values`, and #811's `sessions.open` will create runtime-to-runtime sessions; it is not deleted.
12. **Deleting `objects` takes conformance fixtures with it**: the Go fixture `go/rutis/internal/fixtures/session/main.go:193` deliberately does not declare `objects`, `rpc/tests.rs:2676–2706` tests "cannot be sent to py", and the cross-language case `no-objects-declared` at `objects.expected.json:180`. These are deleted together with the removal.

## 5. rutis-loader

### 5.1 Delete

| Name | Location | Callers (verified) | Class | Changed with it |
| --- | --- | --- | --- | --- |
| Branches kept for Go binaries older than 0.18: `config::Options`, `Options::required`, `without_required`; `options` and `older_than_0_18` in `config_step.rs` | `config.rs:19–41,66,405`; `config_step.rs:98–166,223–255` | Production: only `config_step.rs:108`; tests `tests/config_cases.rs:6,45,80,97–100` | C | `config::check` goes from 3 parameters to 2 (done when it moves into the kernel per 3.4 item 2); `rutis-host.md:238` is deleted; "Migrating to 0.19" says "rebuild Go plugins with SDK 0.18 or later" |
| `ServiceCatalog::share_by_name`, `share_registered_only`, the `by_name` field | `catalog.rs:116,134,289,298,426` | `share_by_name`: production rutis-host `host.rs:216`, e2e `fixture-toolhost/src/main.rs:171`, tests `runtime_rows.rs:1036`, `fault_containment.rs:170`, `remote_modes.rs:199`, `runtime_instances.rs:336`, all no-ops (the default is already true). `share_registered_only`: tests `check.rs:106`, `scope.rs:52`, `marked_services.rs:253`, guides `rust-host.md:434`, `open-host.md:129` | C | Overlaps the `share_registered_only` row in #811 §8: this design's CL17 does it all at once, and #811 S5 no longer does that row (#811 did not list the callers of `share_by_name`). Once `shared_key` is always true, the use of `is_exported` at `catalog.rs:426` is deleted |
| `Loader::waiting_for` | `loader/api.rs:178–202` | Only tests `marked_services.rs:458,466` | C (duplicates `waits`/`waits_of`; `WaitReason::Explained` already covers it) | `guarantees.md:279` (the text of `bridge/marked-services`), `rust-host.md:243,487`, `.agents/skills/rutis-remote-plugin/SKILL.md:48` switch to `waits` |
| `Loader::locate` | `api.rs:237–264` | Zero calls | C | — |
| `Loader::schema_of` | `api.rs:267–269` | Tests `loader.rs:458,460`, `peer_rows.rs:230` | C (equals `resolve(name).await?.schema`) | The wording and tests of guarantee `loader/schema` (`guarantees.md:358`) switch to `resolve(..).schema` |
| `register_instance_keyed`, `readable_instance_keyed`, `readable_instance` | `catalog.rs:183,210,200` | The first two are called only by methods in the same file; `readable_instance` only by test `instance_services.rs:170` | C | The body of `register_instance_keyed` is inlined as a private function; dsh uses `readable` and `readable_keyed`, unaffected |
| Old-name aliases for Go/Swift (**8 each**; the list said 7): `GoBinaries`, `GoManifest`, `GoBinary`, `GoResolver`, `GoRuntimes`, `GoRuntimesHandle`, `GoRuntimeState`, `GoRuntimeInfo` and the 8 Swift counterparts | `runtime/go.rs:56–78`, `swift.rs:55–76` | Not used by rutis-host; a dozen-plus test files; guides `rust-host.md:539–547` (doctest), `go-plugin.md:414` | C | `struct Go` and `struct Swift` are not aliases and are used by rutis-host; they stay. The error text `"{}Runtimes restart"` (`binary.rs:1044,1080`) expands to "GoRuntimes restart"; it changes to write the runtime name |
| `read_go_manifest`, `read_swift_manifest` | `go.rs:83`, `swift.rs:81` | The former has zero calls; the latter only tests `swift_rows.rs:18,130` | C | — |
| `GO_MARKER`, `SWIFT_MARKER`, `resolve_entry` in `internal` | `lib.rs:89–105` | No external calls | C | The constants are inlined into the impls of `Go`/`Swift`; `resolve_entry` is deleted with the npm resolution |
| Features `python`, `bun`, `kotlin` that only forward to the bridge | `Cargo.toml` | No code gated in src (`python` only gates the doctest at `lib.rs:36,47`) | C | Test gating per 4.4 item 5; the `features` in rutis-host `Cargo.toml:33`, `examples/marked-services`, `examples/rust-services`, `tests/e2e/fixture-tenants` are deleted in step. `node`, `go`, `swift` gate the loader's own code, are not on the list, and stay |
| The `allowed_services` check | `runtime.rs:1014–1019` | See the #683 section of §4 (4.5) | C | — |
| npm resolution rewritten in Rust: the `npm` module (`runtime.rs:1449–1618`, 170 lines), `package_version` (`:1291–1306`), the local branch of `Naming::Npm`, `pub use npm::resolve_entry`; the optional `url` dependency | `runtime.rs` | `RuntimeResolver::node` (rutis-host's default Node runtime, `languages/node.rs:84`) | C (duplicates the runtime's own resolution) | See 5.4 item 1 |
| The qualified Go/Swift row-name forms `go-<file>:<plugin>`, `swift-<file>:<plugin>` | `binary.rs:990–1012,1084–1094` | See 5.4 item 2 | C | — |
| A second way to state a row's state: `status::RowState` | `loader/status.rs:61` | rutis-host `call.rs:10,137`; tests | C | See 5.4 item 3 |

### 5.2 Move to rutis-host

| Name | Location | Handling |
| --- | --- | --- |
| Restart policy for Go/Swift binary runtimes: `RestartPolicy`, `RuntimeEnd`, `RuntimeEnd::line`, `Supervision`, `supervise`/`unsupervise`, end handling and backoff (`restart_after_end`), the fields `backoffs`/`pending` | `runtime/binary.rs:1231–1289,1398–1411,1600–1650,1960–2075` | Merged into one with rutis-host's `restart.rs` (which already does `pub use rutis_loader::backoff::*`, `restart.rs:17`); `"runtime {name}: restarting"` and `"gave up after {LIMIT} restarts within {} minutes"`, each written on both sides (`restart.rs:55–62`, `binary.rs:1271–1275`), are kept only once. Starting processes belongs to #811's `rutis-host/spawn`, and restarting goes with it |
| The `backoff` module (68 lines, 7 public items) | `backoff.rs` | Moves to rutis-host |
| `BinaryRuntimes::prefix_output` | `binary.rs:1550–1557,2293–2310` | Moves with the output prefix in 4.2 |

The loader keeps only `restart(name)` and state. **On-demand start and idle stop stay in the loader**: both are in the same timer loop `sweep` (`binary.rs:2120–2200`, `idle` default 60 seconds `:1528`, `eager` `:1534`), which reads `loader.entries()` to decide whether any row is in use; this is part of reconciliation. After #811, binary runtimes are rows of `rutis-host/spawn`, and `sweep` starts and stops these rows on demand.

### 5.3 Receive and keep

Receive: per 3.4 items 2 and 3, the things the list would move from the kernel are no longer moved here; the loader switches to calling the kernel's `rutis::config::check`.

| Name | Location | Reason |
| --- | --- | --- |
| Config expressions: `Expressions` (`catalog.rs:526`), `ExprScope` (`:459`), `{"__jsExpr":…}` (`loader/desired.rs:111,161`, `loader/mod.rs:90`), `readable`, `readable_keyed` (`catalog.rs:226,233`), `NotReadable` (`error.rs:103`) | — | Cordis/dsh compatibility: dsh's profile uses them (`profile/mod.rs:23,73`, `profile/expr.rs:19,782`, `profile/yaml.rs:22`). See 5.4 item 4 |
| `patch.rs` (367 lines) | — | Cordis/dsh compatibility: copies the format and behaviour of cordis-plugin-include |
| Layering (`Layer`, `apply_patches`) | — | Used by dsh (its `--patch` is one layer) |

The `internal` module (a back door for outside code, `lib.rs:89–105`) is handled per the table below, and the module is deleted afterwards:

| Item | External callers (verified) | Handling |
| --- | --- | --- |
| `apply_patches` and `Composed`, `ComposedRow`, `Owner`, `PatchWarning` | dsh production code `profile/mod.rs:98`, `profile/layers.rs:19,562`, `profile/include.rs:16,46,141,250` (**not rutis-host**); `Owner` and `PatchWarning` already appear in the public fields `EntryInfo.owner` and `ReconcileReport.warnings` | Becomes public API (`rutis_loader::patch`); the reason is dsh compatibility |
| `is_runtime_name` | rutis-host `project.rs:133`, `dev.rs:611`, `config.rs:608` (the last is deleted by #811) | Public (runtime-name rules are row-name rules, same as item 3) |
| `read_binary_manifest` | rutis-host `languages/go.rs:170` | Public |
| `PLUGIN_API`, `SWIFT_PLUGIN_API` | rutis-host `main.rs:32,33` | Deleted; rutis-host switches to `Go::PLUGIN_API` and `Swift::PLUGIN_API` (public associated constants of `BinaryLanguage`, already used this way at `go.rs:335`) |
| `node_schema`, `REMOVED_EVENTS` | rutis-host `config.rs:861` | Handled by #811 (peer deleted as a whole) |
| `GO_MARKER`, `SWIFT_MARKER`, `resolve_entry` | none | Deleted (5.1) |

The `install` feature (#691) does not go into the loader: verified that the loader has no such feature, and the body of #805 says "rutis-loader unchanged".

### 5.4 Where verification differs from the list

1. **Unprefixed npm package names are the current main form, and the form dsh compatibility needs**. The list says "row names can no longer be unprefixed npm package names". Verified: rutis-host's default Node runtime goes through `RuntimeResolver::node`; the guides `rutis-host.md:75,596`, `typescript-plugin.md:283,293`, `rust-host.md:440,460` all write package names; #805's `add` writes bare package names for Node, and `Installed::of_row` explicitly rejects the `node:` prefix; the persisted samples for 0.8–0.18 contain `weather-plugin` and the like; dsh's Cordis rows (`@deepseek-ai/dsh-storage`, `@rutis/dsh-aimux`) are the Cordis form — a Cordis plugin name is its npm package name. Decision (confirmed by the user on 2026-10-28): **delete the npm resolution in Rust and keep unprefixed row names**; the name is passed unchanged to the Node runtime to resolve (the existing remote branch `runtime.rs:130–133` already does this, and once #811 unifies local and remote this is the only path left anyway). There is only one `RuntimeResolver`, and two forms of Node row names: unprefixed (the default Node runtime; the Cordis/dsh and #805 form) and `<runtime name>:<package or file>` (one process per plugin, #683 H1's `modules`). Cost: losing the Rust-side staleness check based on the `version` in `package.json`; re-resolution after a runtime restart is covered by the existing per-session check (`take_stale`, #701), and a running Node does not switch code because a file changed, so no version check is needed. About 215 lines and the `url` dependency are deleted; no migration is needed.
2. **The two Go/Swift row-name forms serve two purposes**: `go:<plugin>` searches all binaries, and `go-<file>:<plugin>` names the binary when names collide; `dev_rows` in `rutis-host dev` (`languages/go.rs:107–109,164–178`) uses the qualified form to pin rows to the development build. #805's `add`/`installed` and the main form in the guides use `go:<plugin>`. Handling: **keep only the language-prefix forms `go:<plugin>` and `swift:<plugin>`**. Reason: they do not tie row names to file names (renaming a binary does not invalidate the row), and #805 and the guides already use them. When two binaries provide plugins with the same name, startup reports an error naming both files, and the plugin author renames one — consistent with the kernel's "a later one with the same name is refused"; no disambiguating form is offered. Under `dev`, the Go/Swift runtimes look only at the development build directory (`.rutis/go/`), not the installed directory, so `go:<plugin>` hits only the development build. For `go-dns:resolver` and `swift-MacKit:clipboard` in the persisted samples, the same approach as #811 §11 item 8 applies: the samples drop these forms and `rejected/` cases are added.
3. **`report::RowState` is a protocol value** (the values of the `rutis/row` event on the wire, [event protocol](design-events-protocol-2026-10-18.en.md) §8, guarantee `events/framework`) and cannot be merged into a Rust enum with payloads. There is also a fourth one, `check::RowStatus` (`loader/check.rs:79`, the result of `check`, not a running state; this design does not touch it). Handling: `status::RowState` is deleted and state snapshots use `EntryStatus` directly (`Starting` is given by the snapshot phase of `Running`); `report::RowState` is kept as the wire value and renamed `RowEventState`, to avoid two types with the same name. Done when: the output of `rutis-host status` and `call` is unchanged word for word.
4. **The public names of config expressions contain no JS to begin with**: only the wire key `__jsExpr` and the doc comments mention JS. `__jsExpr` is a Cordis/dsh format key, used by dsh's YAML (`!!js` → this node) and by `cordis.patch.expected.json` in persisted samples `0.15.1` and `0.17.2`. Handling: the key name does not change; only the wording of the doc comments changes.
5. **`Loader::set_overlay` is not used by dsh**: dsh's `--patch` is an ordinary application layer (`profile/layers.rs:550–557` makes it `Layer::new("patch:…")`, through `reconcile` at `watch.rs:95`) and does not go through `set_overlay`. Its only production caller is rutis-dev, which is being deleted (`server.rs:285,309`). The list's reason for keeping it does not hold. Decision (confirmed by the user on 2026-10-28): the overlay layer is deleted — `Loader::set_overlay` (`api.rs:115`), `overlays()` (`api.rs:97`), the overlay state (`loader/mod.rs:412,528–536`, `loader/commit.rs:312–332`), the test `tests/overlay.rs` (187 lines), guarantee `loader/overlay` (`guarantees.md:356`); it goes into the same self-contained change as deleting rutis-dev (§13 CL23).
6. **The aliases are 8 per language**, already written with the real count in 5.1.

### 5.5 #683 host limits (rutis-bridge and loader written together)

| Name | Location | Callers (verified) |
| --- | --- | --- |
| `call_limit`, `kill_on_timeout` | bridge `runtime/local.rs:51–54,153–154,251,263`; `runtime/plugin.rs:247,341,356–380,515`; `runtime/rows.rs:15,96–159`; `session/rpc/limits.rs` (188 lines); `session/rpc.rs:307–398,980,1382,1960–1990,2413,3260`; `session/rpc/relay.rs:122–182`; `report.rs:133–135` | No production callers; 6 tests in bridge `tests/plugin_limits.rs` |
| Error code `timeout` | `session/mod.rs:123–126` | Neither the six SDKs nor specs have this code |
| `allowed_services`, `may_use` | `local.rs:278`; `plugin.rs:30,40,348`; `report.rs:117–125` (the `unavailable` branch #793 added only for it); loader `runtime.rs:1015–1019` | No production callers; 3 tests in loader `tests/plugin_limits.rs` |

Handling: delete all of it (class C). Reason: the only user is the hypothetical host deskmate in the [#663 evaluation](https://github.com/arcships/rutis/issues/663) (not in the code; it appears only as an example name in designs and guides); callers set their own time limits (0.18 [diagnostics design](design-diagnostics-2026-10-26.en.md) §6.1, with per-language code at `:211–224`), and after a timeout the host uses `processes().kill()` (`transport/local/spawn.rs:988`) plus `FiberView::restart()` (`crates/rutis/src/fiber.rs:1828`); which services may be used is a business allocation. The `deadline` parameter `#796` added to `request_sync` for call_limit is reverted. `processes()` currently hangs on `LocalRuntime` (`local.rs:306`); after #811 deletes `LocalRuntime` it is on the handle of `rutis-host/spawn`, and the guide documents its location at that time.

Kept: `greeting` (4.4 item 10); #683 H1 "one process per plugin" (`RuntimeResolver::modules`, tests `each_plugin_runs_in_its_own_process`, `a_runtime_waiting_for_a_debugger_stays_starting`). So guarantee `host/plugin-limits` is narrowed (only greeting and one process per plugin remain, and 8 tests are removed from its evidence column), not deleted as a whole row; section 3 of open-host drops the two subsections "call time limit" and "services a plugin may use", and its example code is rewritten per #811.

## 6. rutis-host and tool crates

### 6.1 Delete

| Name | Current state (verified) | Changed with it |
| --- | --- | --- |
| The whole `crates/rutis-dev` crate | src 588 lines, tests 389 lines, 13 public items; version 0.1.0, `publish = false`; no crate depends on it; not in the release train; its features duplicate rutis-host's `control.rs` (`status`, `call`), `dev.rs:376` (`reload`), `status.rs` (state changes) | The loader's overlay layer is deleted in the same self-contained change (5.4 item 5); root `Cargo.toml:3`, `Cargo.lock`, the path filters in `ci.yml:175` and `nightly.yml:112`, `docs/ci.md:80`, lines 59, 71, 105, 142, 411, 413 of `docs/quality-status.md`, the status line of `design-host-dev-mode-2026-09-25.md:4` |
| `crates/rutis-migrate` | src 3354 lines (`lib.rs` 3226, `main.rs` 128), tests 286 lines; only does 0.16→0.17; no crate dependencies | Leaves the release train: `scripts/train.mjs:26–27`, the comment in `release.yml:8`, the table in `docs/release.md:9`, root `Cargo.toml`, `Cargo.lock`. Old versions on crates.io can still be `cargo install`ed, so the pointers in `crates/rutis/README.md:53`, `guide/README.md:32` and `migration-0.17.md` stay. #811 §8 lists "the name table of rutis-migrate" (`lib.rs:2878`) as affected; once the crate is deleted that item is not needed |
| `rutis-host go add` | `main.rs:383–416` (about 35 lines), dispatch `:100`, description `:11`, usage `:76–78`; no dedicated tests | See 6.4 item 1 |

### 6.2 Keep

The whole dylib family: `rutis-dylib` (src 1231 lines), `rutis-dylib-meta` (974), `rutis-dylib-launcher` (292), `rutis-sdk` (243), `rutis-xtask` (1347), and the kernel's library generations (3.3). Reason (user decision): it is how we support dynamic Rust plugins (design philosophy: "dylibs are all plugins too"). Current state (verified): only the example `crates/rutis-cli` (feature `dylib-plugins`) and `tests/dylib-fixtures` depend on it; rutis-host, loader, bridge and dsh do not load dylibs. This design does not extend it.

### 6.3 What rutis-host receives and stops using

Receives (§4, §5): language startup (the language part of `Launcher::*`), line splitting and prefixes for binary runtimes' output, restart policy (merged into one with `restart.rs`). Does not receive: `report_line` (4.4 item 6), trace (4.4 item 7).

rutis-host stops using `rutis::internal` and `rutis_loader::internal`. All verified uses:

| Location | Uses | Changes to |
| --- | --- | --- |
| `call.rs:107` (production) | `rutis::internal::config::closest` | `rutis::config::closest` (3.4 item 2) |
| `status.rs:580` (test) | `rutis::internal::config::decode` | construct the error through the public `Definition` and `ConfigError` |
| `host.rs:1366` (test) | `CtxInternal` | the test uses the public `CtxHost` |
| `config.rs:608`, `dev.rs:611`, `project.rs:133` | `is_runtime_name` | loader public API (5.3) |
| `languages/go.rs:170` | `read_binary_manifest` | loader public API |
| `main.rs:32,33` | `PLUGIN_API`, `SWIFT_PLUGIN_API` | `Go::PLUGIN_API`, `Swift::PLUGIN_API` |
| `config.rs:861` | `REMOVED_EVENTS` | handled by #811 |

### 6.4 Where verification differs from the list

1. **#805 keeps `go add`**: #805 (not merged) makes `go add` call `install_go_module`, and its design's compatibility section and I1 say "`go add` is kept, behaviour unchanged". Difference in behaviour: `add <Go module>` installs with the same `GOBIN=runtimes.go.dir go install`, and additionally writes the `installed` record and the row; `go add` only installs the binary and does not change config. Handling: as the list says, delete `go add` after #805 is merged (§13 CL25); the same PR changes the compatibility section and I1 of `docs/design-plugin-install-2026-10-27.md`, `rutis-host.md:78,96`, `go-plugin.md:411`, `templates/go/README.md:11`, and guarantee `host/go-add`. The "install without adding a row" use goes away: for a plugin to be loaded it needs a row anyway.
2. **`Loader::set_overlay` is not used by dsh**, and is deleted together with rutis-dev (5.4 item 5).
3. **The user of `apply_patches` is dsh, not rutis-host** (the table in 5.3 is already written to the facts).

## 7. Language SDKs

### 7.1 Delete: #692's old-name forwarding

No compatibility: old locations are deleted directly, with no forwarding and no deprecation declarations; the repo's own examples and tests switch to the new locations; [SDK entry convergence](design-sdk-entry-convergence-2026-10-27.en.md) and "Migrating to 0.19" change to "moved directly, no old names".

| Language | Locations and counts (verified) | Changed with it |
| --- | --- | --- |
| TypeScript / Bun | `node/rutis/src/index.mjs:49–51` forwards 9 values; `index.d.ts:48–82` has 16 `@deprecated` (7 types, 9 constants) | `node/rutis/src/testing.mjs:37` and `testing.d.ts:1` import from the old location; in the same PR they change to `./advanced` |
| Python | `__init__.py:15–17,21–42` (`_MOVED`, `__getattr__`, `__dir__`), 7 old names in `__all__` | the deprecated section of `public-api.txt:25–32`; the deprecation cases in `tests/test_advanced.py`; `test_public_api.py:23–25,119` |
| Go | 33 `Deprecated:` in the root package (`values.go` 13, `events.go` 7, `runtime.go` 4, `serve.go` 4, `version.go` 3, `plugin.go` 1, `schema.go` 1); `advanced` is currently a reverse alias (`= rutis.X`) | See 7.6 item 2; the deprecated section of `public-api.txt:52`, `public_api_test.go:85–130`; `scripts/train.mjs:43` and `release.md:18` read `const Version` from `version.go` |
| Kotlin | 19 `@Deprecated typealias` in `Moved.kt`; plus `jvmMain/Serve.kt:35,65` (`Rutis.project`, `Rutis.manifest`), `Rutis.kt:34,38,42` (`VERSION`, `PLUGIN_API`, `IMPLEMENTATION`), 24 in total | The three version constants are defined in the deprecated `Rutis` object, and `advanced/Versions.kt:8–14` refers back to it: the literal values move into `Versions.kt`; `scripts/train.mjs:48`, `release.md:18`; `api/*.api` |
| Swift | 25 `@available(*, deprecated)` in `API/Moved.swift`; the definitions are all already in `Rutis.Advanced` and `_Generated` | the deprecated section of `public-api.txt:190` |

Within the repo (templates, examples, tests, docs, .agents) nothing uses the old names; verified as zero, apart from the SDK files listed in the table above.

### 7.2 Delete: branches kept for old hosts

| Item | Node (rutis-runtime) | Bun | Python | Go | Swift | Kotlin |
| --- | --- | --- | --- | --- | --- | --- |
| The hint when `announce` is missing (hosts older than 0.12; the bridge always sends `announce: true`, `process.rs:1079`, dead branch) | `runner.mjs:65–68,438–444,862`; `leaf.mjs:70–90` | `runner.ts:94–97,168–171,226–235,477` | `runner.py:139–146,347–352,396–405` | `runtime.go:187–192,238–250,264,428–437,717–719`; test `oldhost_test.go` (whole file) | `Runtime.swift:340–342,477–487,685` | `internal/Runtime.kt:41–47,182–187,264–272,578–581` |
| `events-v2` handshake check and error | `events.mjs:63–65` (three copies), `runner.mjs:1083,1090–1095`, `session.mjs:72–82`, `dial.mjs:53–54` | `main.ts:139–148`, `session.ts:73–82,262`, `remote.ts:223–224` | `peer.py:49–58,230,879`, `__main__.py:91–95`, `runner.py:38` | `serve.go:228–231,284–295`, `internal/peer/peer.go:56–58,1584–1590`; test `events_session_test.go:455` | `Serve.swift:3–4,134,148–149`, `Peer.swift:20,97–98,1259–1263` | `Sessions.kt:16–18,68–70`, `Peer.kt:49,101–102,1210–1212` |
| `refuseRemoved` (migration errors for the `events`/`emits` fields) | `events.mjs:55–61`, `runner.mjs:851`, `generate.mjs:43–49,1032–1034`, `testing.mjs:694–702` | `runner.ts:32,474` | `runner.py:102–107,1126–1128`, `testing.py:788–792` (`load(emits=…)`) | none | none | none |
| Loading plugin API 1 and 2 | `api ?? 1` and its comment in `leaf.mjs:15–35` | `plugin.ts:86–89` | `plugin.py:108–111`, `runner.py:1374–1379` | none | none | none |
| `rows.failed` fallback | `runner.mjs:1043–1046` | none | `runner.py:649–652` | none | none | none |
| `objects` fallback | `session.mjs:348–349` | `session.ts:337–338` | `peer.py:1150–1156` | `peer.go:749–750`, `crossing.go:153–166` | `Peer.swift:733–742` | `Peer.kt:500`, `Crossing.kt:46–50` |
| The `sync` field (sent only when the peer declares `sync-wait`) | `session.mjs:573–575` | `session.ts:563–565` | `peer.py:1436–1439` | `peer.go:1113–1116` | `Peer.swift:930–932` | `Peer.kt:738–740` |
| `waterfall-sync` receiving branches (§3) | `events.mjs:524,540` (two copies), `bridge/features.mjs:341,345` | `events.mjs:524,540` | `runner.py:1037–1038` | `internal/events/events.go:105,111,118,349`, `events.go:710` | `EventsCore.swift:111,113`, `RuntimeEvents.swift:478` | `Bus.kt:34,38`, `EventsRuntime.kt:458`, `api/*.api` |

Two more deletions (not on the list, confirmed by the user on 2026-10-28), each in its language's self-contained change:

- Go's `warnChainless` hint, which takes effect only under dev (`go/rutis/runtime.go:1165–1192`, called at `events.go:537`, `runtime.go:1075`; `devMode` exists only for it). `RUTIS_DEV` is still used by Node and Bun row tracking (`runner.mjs:106`, `plugin.ts:125`), and rutis-host `dev.rs:54` still sets it. The line at `design-go-runtime-2026-10-10.md:257` is deleted.
- The "an old host's emit reply is a future" branch in Kotlin and Swift (`EventsRuntime.kt:305–311`, `RuntimeEvents.swift:350–359`): verification found it is not old-host compatibility, see 7.6 item 11.

"Loading plugin API 1 and 2" has no dedicated branch; all three script runtimes only check `api > PLUGIN_API`. Handling: delete the `api ?? 1` default and its comment; plugin objects are produced by `definePlugin` / `plugin(...)` and always carry `api`. No new refusal of `api < 3` is added.

The `Require`/`Refuse` mechanism of `events-v2` is also used in Go (`serve.go:290`) and Kotlin (`Listen.kt:339`) by the `incompatible` stop path of remote dialing: only the `events-v2` requirement is deleted; `Require`/`Refuse` itself stays.

### 7.3 Delete: Python 3.10 and 3.11

The minimum version rises to 3.12. Delete `python/rutis/rutis/_eager.py` (161 lines, callers `events.py:33,126–128`, `peer.py:36,1819–1826`), the fallbacks in `_unhandled.py:56–79,94–99`, the comment at `peer.py:277`, and `tests/test_eager.py`. The three `requires-python` places (`python/rutis/pyproject.toml:7`, `crates/rutis-host/pyproject.toml:13`, `crates/rutis-host/templates/python/pyproject.toml:6`); CI jobs hard-coded to 3.10 (`ci.yml:448,606,690–702,940–952,1042,1142–1151`, `install.yml:19,108,109`, changed in the same PR per the rule in `docs/ci.md:288`); docs: see §12. The generated Python types still use the 3.10 style (`build/exports.rs:82,366,386,1233`); this design does not change that output (§8).

### 7.4 Move: the Kotlin test tool

`RutisTest.kt` (691 lines) is in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/testing/`, and `build.gradle.kts` has both `jvmMain` and `androidMain` include this directory, so it is packaged into every Android app; it imports 17 `internal.*` names and calls `internal.ConfigSchema` directly. Handling: split it into a separate artifact `rutis-testing`; after the rewrite in 7.5 it uses only public API and no longer imports `internal.*`. The dependency coordinates in `kotlin-app.md:67,102,209` and `kotlin/README` change accordingly.

### 7.5 Change: test tools, config validation, event dispatch — leaves are only leaves

These three are interdependent in the same code (the test tools use a local event bus and the SDK's config validation to simulate the host), so they are changed together per language.

**Test tools become "a real runtime + the real rutis kernel".**

- rutis-host adds `rutis-host test`: `rutis-host test <driving plugin> -- <runtime command…> <project>` starts a host with no rows, starts the runtime as a local runtime (`pipe::spawn`: fd 3, or a loopback address with a one-time token on Windows) and sets up a #811 session with it. The host contains an ordinary Rust plugin `rutis-host/test` that uses only the loader's public API and provides the service `rutis-host/test`: `load(name, config)`, `update(id, config)`, `unload(id)`, `reported(id)`. It belongs to rutis-host (reference host and tools); the platform does not change. `rutis-host test <driving plugin> --channel <channel>` (CL27b) takes a runtime the test tool runs in its own process: the test tool hands it the channel as a local runtime gets one (an inherited fd on Unix, `fd:<n>`; on Windows and with `RUTIS_LOCAL_HANDOVER=loopback`, a loopback address with a one-time token, `tcp:<host>:<port>`), for Go, Swift and Kotlin, whose tests call the plugin under test from the test process.
- Each language's test tool runs `rutis-host test` with its language's real runtime command; the plugin under test becomes a row through `load` and is run by the host in this runtime per #811's `run`; the tester's fake services, `t.on` and `emit` are a test plugin in the same runtime, using the public `ctx.provide`, `ctx.on`, `ctx.emit`.
- Delete the parts of each language that simulate host rules: in Node `testing.mjs`, the cross-process value semantics (`:46–360`), the dependency check, and the parts of `config.mjs`/`events.mjs` it uses; in Python `testing.py`, `_cross` and so on (`:93–360`); in Go `rutistest.go`, host dispatch (`:800–931,955–1213,1223–1290,1359–1403,350–391`, about 550 lines); in Swift `RutisTest.swift`, `Harness` (`:61–508`); in Kotlin `RutisTest.kt`, `Harness` (`:101–547`).
- The public API does not change: Node `load`, `host`, and `Loaded`'s `service`/`methods`/`provided`/`reported`/`on`/`update`/`unload`/the five emit modes/`filter`; Python `load`, `Bus`, `Loaded`, `Reported`; Go `Load`, `TryLoad`, `Loaded`; Swift and Kotlin `RutisTest.load`, `scope`, `Loaded`. The Python `load(emits=…)` parameter is deleted with `refuseRemoved`.
- Finding rutis-host: `rutis-host` on `PATH`, or the path given by `RUTIS_HOST`. The Node test tool `@arcships/rutis/testing` dynamically imports `@arcships/rutis-runtime` (the SDK package does not statically depend on the runtime package; the `leaf.mjs` convention does not change); templates add `@arcships/rutis-runtime` and `@arcships/rutis-host` as dev dependencies. Bun uses the same test tool and starts the Bun runtime based on `typeof Bun`.
- In 0.19 test tools use a local runtime only (decided in #875): tests on devices (Android instrumented tests, the iOS simulator) do not use `rutis-host test`; there the application installs its own plugins.
- H4 (the test tool refuses a plugin providing a service name the host already has, #810) is reported by the real kernel; the two hand-written checks in `testing.mjs` and `testing.py` are deleted; the kernel's error text is what counts.
- Test speed: a single test currently takes on the order of 1 millisecond (all 169 tests in `node/rutis` take 1.4 seconds, measured); starting a host once is estimated at 150–500 milliseconds. Approach: start one host per test file, unload rows between cases, and do not restart the host. Acceptable: the tests of a template project (one file) take no more than 2 seconds in total; all tests of `node/rutis` and `python/rutis` take no more than 3 times as long as now. If exceeded, stop and report; do not bring back the simulated host.

**Config Schema validation is kept only once, in Rust.** Validation and filling defaults are in the kernel's `rutis::config::check` (3.4 item 2), and both the loader's rows and the bridge's plugin values call it; the loader sends the config with defaults filled in to the runtime (currently it stores only the original value and does not fill in for rows of other runtimes, [config Schema](design-config-schema-2026-10-27.en.md) §2.1 line 47). SDKs only decode:

| Language | Delete | Keep |
| --- | --- | --- |
| Node / Bun | validation in the three copies of `config.mjs` (476 lines each, identical md5, `config.test.mjs:43–45` checks they match) | `schemaOf` (`definePlugin` rejects Standard Schema, `index.mjs:28,36`), `configError`, `pathText` (`cordis-config.mjs:1` formats Cordis errors), about 20 lines moved into `cordis-config.mjs` and `index.mjs` |
| Python | validation in `config.py` (626 lines) | decoding |
| Go | validation and default-filling in `internal/config` (`config.go` 785, `text.go` 237) | decoding and the rewrite to "is not a value the plugin accepts" (`config.go:22–43`) |
| Swift | `ConfigSchema.swift` (670 lines) | the decoding path of `ConfigSchema.path(of:)` |
| Kotlin | `internal/ConfigSchema.kt` (566 lines) | `DecodePaths` |

An app's own plugins (S4a) do not go through the loader, and no host checks them: their configs are only decoded, a missing key taking the default the author wrote in the type (in Swift, `@RutisConfig` generates a decoding with the defaults, not the synthesized `Decodable`), without Schema validation; a missing required key or a value of another type is a decoding error naming the key, in the words of the kernel's `rutis::config::check` (`config: count is required`, `config: count should be a number, got "x"`). Rows the host loads are still checked by the kernel before the runtime gets them.

`specs/config-schema/cases.json` is run only by the kernel (moved from loader `tests/config_cases.rs`) and `rutis-host check`; each SDK's case runner is deleted, and `specs/config-schema/README.md:5` is rewritten. The exception that Cordis rows are not validated against the converted Schema (§2.3) stays in the loader. The test tools' warning about extra keys is given by the real loader.

The five reasons in §2.1 of the 0.18 [config Schema](design-config-schema-2026-10-27.en.md), one by one: reason 1, "the test tools have no loader, so it can only go in the SDK" — no longer holds once the test tools use the real kernel; reason 2, "remote runtimes are checked by the end that runs them" — the loader already has the Schema for every row (including rows of runtimes that connect in) (`schema: described.config` at `runtime.rs:546`), so it does not hold; reason 3, "typed languages have to decode anyway" — still holds, and only decoding stays; reason 4, "one set of rules kept consistent by a case table" — no longer needed once there is only one implementation; reason 5, "SDKs before 0.18" — no compatibility, void. The conclusion of §2.1 becomes "validation lives once in the kernel; SDKs only decode".

**All events are dispatched by the host.** Delete each language's local dispatch: the local implementations of the five modes, local filtering, local ordering, `events.elsewhere`.

| Language | Locations deleted (verified) |
| --- | --- |
| Node / Bun | in `events.mjs` (three copies, 558 lines): elsewhere (`:273–301`), the five local modes (`:310–455`), `#keeps`, `#unanswered`; `#viaHost` (`:462–509`) and the host callback `call()` (`:515–558`) stay |
| Python | `events.py:355–494`; `runner.py:360–362,895–1020,1122–1123` |
| Go | `Dispatch` and `Queue` in `internal/events/events.go:231–422`; `events.go:403–439,590–617` |
| Swift | `EventsCore.swift:224–391`; `RuntimeEvents.swift:212–310,417–436` |
| Kotlin | `Bus.kt:120–217`; `EventsRuntime.kt:221–266,377–406` |

Benchmark (before the deletion, in its own PR): write a new `tools/bench/events` that measures one case each in the Node and Python runtimes — emitter and listener in the same runtime, using `emit`, `serial` and synchronous `bail` respectively, 10 000 times each, reporting the median and p99; run it before the deletion (local dispatch) and after (through the host). Acceptable: the median of `emit` through the host is accepted at the measured values — Node 71 microseconds, Python 139 microseconds (`docs/performance-events-2026-10-28.en.md`; the original limit was 100 microseconds, Python is over it, and the user decided to remove local dispatch anyway after seeing the numbers); the path through the host can be optimized later. And the time for dsh's baseline (`node/baseline/`) to start and run through once is no more than 1.1 times the current time. If the dsh baseline is over, stop and report the numbers to the user; do not keep a fast path.

Behaviour changes caused by the deletion (written into "Migrating to 0.19"): synchronous `bail` on the Android main thread in Kotlin is always refused (`EventsRuntime.kt:321–327`; currently allowed when there are local listeners); in Go, `emit` after the session ends no longer goes local (`events.go:446–449`), and `emit` during cleanup reports that the session has ended.

### 7.6 Where verification differs from the list

1. **Deleting old names in Python makes already-published plugins fail to import**: plugins and the runtime share the same `rutis` package in the host's environment (SDK entry design §5). The current rules in `release.md:36`, `plugin-api.md:478`, `python-plugin.md:389` say "names are not deleted while `PLUGIN_API` is unchanged; when `PLUGIN_API` increases, the runtime keeps loading the old API". Handling: following "no compatibility", rewrite these three rules as "a version that deletes names says so in the migration guide, and plugins upgrade together with the host"; `PLUGIN_API` is not raised (the plugin API itself did not change; only import locations did). "Migrating to 0.19" says so.
2. **Go cannot simply "move the definitions into `advanced`"**: the root package cannot import `advanced` (cycle); the implementations of `Service`, `ServeArgs`, `ServeConn`, `Manifest`, `Project`, `Rename`, `Schemer`, `Version`, `PluginAPI`, `Implementation` depend on unexported state in the root package. Handling: move these implementations into `go/rutis/internal/runtime`, and have both the root package (keeping only the 19 top-level names plugin authors use) and `advanced` call it; the names in `advanced` are aliases of `internal/runtime`.
3. **After Kotlin deletes the old aliases in the generated direction, code generated by 0.17.2's `rutis-ksp` no longer compiles**, contrary to what the SDK entry design :63 and the migration guide :102 say. Handling: the migration guide says `rutis-ksp` is upgraded to the same version as the SDK.
4. **The "event loop held for 5 seconds" hint is not only for dev**: all three script runtimes have it on by default (Node/Bun `output.mjs:83–159`, Python `_output.py:100–211`), turned off only by `RUTIS_DEBUG=1` (`dev --debug`), and it prints under `run` too; it comes from 0.18 diagnostics design §6.4 and has guarantee `host/blocking-hint`. The list's premise was "only for dev", which does not hold. Handling: not deleted. Go, Swift and Kotlin do not have this hint.
5. **The runtime-side output prefix stays** (4.4 item 8). Go, Swift and Kotlin have no runtime-side prefix.
6. **`refuseRemoved`, plugin API 1/2 and `rows.failed` do not exist in Go, Swift and Kotlin**; the 7.2 table says "none" to reflect the facts.
7. **"Keep config validation only in the loader" has a gap** (plugin values do not go through the loader), so it is placed in the kernel per 3.4 item 2; `config.mjs` cannot be deleted as a whole (the "Keep" column of the table above).
8. **Local event dispatch cannot be fully deleted on Node**: the Cordis node `@arcships/rutis-runtime/bridge` (`src/bridge/*.mjs`, about 810 lines) acts as a host itself, "its bus has no host above it" (`events.mjs:19–22`), and must dispatch locally; it is part of Cordis compatibility, and #811 only changes how it is written. Handling: local dispatch stays only in the Cordis node (split out of `events.mjs` into its own copy), and is deleted entirely from the leaf path; the check that three copies match becomes two (`node/rutis`, Bun).
9. **Order of test tools and fast path**: the test tools depend entirely on local dispatch, so for each language the fast-path deletion and the test tool rewrite go in the same PR (start of 7.5).
10. **The removal of negotiation in the SDKs and the bridge has an order**: first the bridge stops checking the peer's `events-v2`, `objects`, `sync-wait`; then the SDKs stop checking the host's and stop declaring their own; finally the bridge stops declaring. Done the other way round, objects with methods would be refused and wait-cycle detection would stop working. §13 CL11 splits in this order.

11. **The Kotlin and Swift "an old host's emit reply is a future" branch is still reached on the current host**: when an emit is already running on the same row, the bridge's `events.dispatch` queues this emit and answers with a future (the branch where `run_emit` returns false in `crates/rutis-bridge/src/events/hub.rs:809–814`); the comment at the same place in Go, `peer.Settle` (`events.go:541–545`), describes exactly this current behaviour (#677). The comment on this branch (c2c1b7e8, 2026-10-21) says "older host", which does not match the facts. If it were deleted, for consecutive emits on the same row the completion and errors of the later one would be lost, and the Kotlin and Swift tests would fail. Handling: per §13.1 item 2, no working around it — the branch is not deleted; only its comment is changed to "the host answers with a future when it queues", done in the Kotlin and Swift CL26; this item is reported back to the user.

## 8. Generated types: same approach, moved out of the platform

### 8.1 Now

`build::Exports` (`crates/rutis-bridge/src/build/exports.rs` 3612 lines, of which about 830 are tests; `build/crates.rs` 895 lines; `build/mod.rs` 8 lines) reads source code with syn: `crates.rs` runs `cargo metadata` and `rustc --print cfg`, reads the whole module tree from the entry point, resolves `use` and re-exports, and evaluates `#[cfg(feature)]`, without expanding macros; `exports.rs` finds the `impl` blocks marked `#[rutis::service]` and turns method signatures, the fields of referenced structs and enums, doc comments, serde attributes (rename, rename_all, tag, content, untagged, flatten, default, skip, transparent), aliases, generics and error codes into TypeScript and Python. Dependencies `exports = ["dep:proc-macro2", "dep:quote", "dep:syn"]` (bridge `Cargo.toml:33–35,77`).

Callers: `rutis-host types <crate directory>` (`crates/rutis-host/src/main.rs:150–184`, reads source only, no compiling); `examples/rust-services/build.rs`; the snapshot tests of `examples/marked-services` (`tests/rows.rs:175–260,340` compare `rows/services.d.ts` and `services.py`, and run `tsc --strict` and `pyright --strict`). The `cordis` feature does not call Exports; it depends on `exports` only to get syn and quote (for `rust.rs`, which 4.1 deletes, and for one identifier check at `cordis/build.rs:291`).

`src/exports.rs` (228 lines) and `src/marked.rs` (1096 lines) are runtime code (error conversion, exposure by name), not part of generation, and stay.

### 8.2 Decision

The generation approach does not change (user decision, 2026-10-28): keep the `Exports` implementation that reads source code and generates TypeScript and Python. Authors write nothing extra for their data types; only types with hand-written serde implementations use `Exports::declared` / `declared_python` to write out their type. It is only moved out of the platform, out of rutis-bridge: it is a tool for developers (class B in §2), not a platform responsibility.

It moves to a separate tool crate `rutis-types` (`crates/rutis-types`, published with the release train), not into rutis-host. Reasons (by dependency):

- The generator depends only on syn, quote, proc-macro2 and the standard library (verified: the `use`s in `build/exports.rs` and `build/crates.rs` contain nothing from rutis or the bridge), so making it its own crate does not require splitting any dependency.
- Its users include build.rs (`examples/rust-services/build.rs`, and embedders who follow the rust-host guide). If it moved into rutis-host, build.rs would pull rutis-host's whole dependency tree (loader, bridge, tokio, WebSocket, TLS) into build-dependencies; the rutis-host library also states that it makes no semver promise (`lib.rs:4–5`). A separate crate brings only syn.
- rutis-host depends on `rutis-types` to implement `rutis-host types`.

The API does not change: `rutis_types::Exports::new().typescript(..).python(..).declared(..).declared_python(..)`, with `.generate()`, `.declarations()`, `.write(dir)` as before; `rutis-host types <crate directory>` and `--python` as before; build.rs only changes its dependency from `rutis-bridge = { features = ["exports"] }` to `rutis-types`, and the `use` path from `rutis_bridge::build::Exports` to `rutis_types::Exports`. The generated content is unchanged word for word.

### 8.3 Why the approach does not change

The list originally said "the `#[rutis::service]` macro carries the service description into the binary at compile time". After verification, none of three alternative approaches is used:

- **The macro carries descriptions**: an attribute macro only receives the annotated `impl` block — method names, `async`, parameter names, parameter and return types **as the written paths**, method doc comments; the fields of referenced structs and enums, field docs, serde attributes, enum variants (where error codes come from), what a type alias points to, and which crate a path belongs to are all invisible to the macro. The existing example is exactly cross-crate: the data types of `examples/rust-services` are in the dependency crate `rust-services-calendar`, and `build/crates.rs` exists for this. Also, build.rs runs before this crate is compiled, so the description the macro puts into the binary is not available in build.rs.
- **serde-reflection and similar (tracing serde at run time)**: verified as not workable. rutis's value types, `#[serde(tag)]`, `untagged` and `flatten` all go through `deserialize_any`, so tracing cannot recover the structure; types with the same name in different modules are treated as one, giving wrong results; it also does not work when a return value only implements `Serialize`. The current generator supports all of these, with tests.
- **schemars (the `JsonSchema` derive)**: requires authors to add the derive to every type that appears in a parameter, return value or error, including types in dependency crates; this goes against "authors write nothing extra".

### 8.4 Effects

- bridge: the three files in `build/` (4515 lines) move as a whole to `crates/rutis-types/src/`; `pub mod build` (`lib.rs:32`) and the `exports` feature are deleted; `syn::parse_str::<syn::Ident>` at `cordis/build.rs:291` becomes a hand-written check (starts with a letter or underscore, followed by letters, digits or underscores, excluding keywords), and `cordis` no longer depends on `exports` (syn's other user, `cordis/build/rust.rs`, is deleted by CL10). After that the bridge no longer depends on syn, quote, proc-macro2.
- `src/exports.rs` (228 lines) and `src/marked.rs` (1096 lines) are runtime code (error conversion, exposure by name, also used by the loader), not generation, and stay in the bridge.
- rutis-host: the `rutis-bridge` `exports` feature in `Cargo.toml:32` becomes a dependency on `rutis-types`; the `use` in `main.rs:150–184` changes.
- `examples/rust-services`: the build-dependency becomes `rutis-types`; `examples/marked-services`: the dev-dependency becomes `rutis-types`, the snapshot tests do not change.
- The generator's unit tests (about 830 lines inside `exports.rs`) move with the file; `crate::types` and the like inside test fixture strings do not change.
- Release train: `scripts/train.mjs` adds `rutis-types` before `rutis-host`; the table in `docs/release.md:9` gets one row.
- Relation to merged PRs: #798 (Python types) and #810 (docs and H4) are not affected; only the crate holding the code changes; the `CordisError` recognition changed by CL5 is done before the move.
- In the evidence column of guarantee `bridge/marked-services`, the path of the `exports.rs` unit tests changes to the new crate.

## 9. Before and after

The current numbers were measured on `8071a794` with `wc -l` (including unit tests inside files); "public items" uses the same measure as #811 §9: `grep -cE '^\s*pub (fn|struct|enum|trait|type|const|use|mod) '`. All after-change numbers are **estimates**; the "After #811" column is #811 §9's estimate, and this design's reductions are counted on top of it, without double counting.

### 9.1 Rust crates

| crate | Lines now (src) | Public items now | After #811 (estimate) | After this design (estimate) | Main changes in this design |
| --- | ---: | ---: | ---: | ---: | --- |
| `rutis` | 16180 | 408 | 16180 | about 15500 | delete interceptors −390, bus −170, run.rs −150, ctx −90, internal −80 and so on, about −1230; add `config::check` about +560 |
| `rutis-macros` | 1284 | 4 | 1284 | about 1280 | delete the `actor` parameter |
| `rutis-bridge` | 41378 | 750 | about 36100 | about 29000 | reverse mount −1040, `runtime/spawn.rs` −767, `process.rs` −213, `output.rs` −259, `runtime/mod.rs` −71, negotiation −60, #683 about −400; `build/` −4515 (moved to `rutis-types`) |
| `rutis-loader` | 15404 | 297 | about 13610 | about 11850 | `config.rs` moves into the kernel −594, restart policy moves out −250, npm resolution −215, aliases and dead methods about −400, overlay layer about −60 |
| `rutis-host` | 11128 | 176 | about 10760 | about 12000 | receives language startup +200, output +260, restart +330 (merging removes duplicates −100), `rutis-host test` about +300; `go add` −35 |
| `rutis-dev` | 588 | 13 | 588 | 0 | deleted |
| `rutis-migrate` | 3354 | 5 | 3354 | 0 | deleted |
| `rutis-types` (new) | — | — | — | 4515 | moved unchanged from the bridge's `build/` |
| dylib family (`rutis-dylib`, `-meta`, `-launcher`, `rutis-sdk`, `rutis-xtask`) | 4087 | 57 | 4087 | 4087 | unchanged |

Current numbers per file (the files this design touches):

| File | Lines | Public items | After (estimate) |
| --- | ---: | ---: | ---: |
| `rutis/src/intercept.rs` | 492 | 3 | about 100 (renamed `writer.rs`) |
| `rutis/src/bus.rs` | 1209 | 15 | about 1040 |
| `rutis/src/bus/run.rs` | 719 | 7 | about 570 |
| `rutis/src/events/remote.rs` | 317 | 31 | about 255 |
| `rutis/src/events/api.rs` | 702 | 36 | about 645 |
| `rutis/src/effect.rs` | 318 | 5 | about 275 |
| `rutis/src/fiber.rs` | 2140 | 22 | about 2120 |
| `rutis/src/key.rs` | 531 | 27 | about 495 |
| `rutis/src/internal.rs` | 202 | 21 | about 120 |
| `rutis/src/config/mod.rs` + `decode.rs` | 1136 | 18 | about 1700 (takes in `check`) |
| `rutis-bridge/src/build/` (three files) | 4515 | 73 | 0 (moved to `crates/rutis-types`) |
| `rutis-bridge/src/cordis/build/rust.rs` | 855 | 1 | 0 |
| `rutis-bridge/src/cordis/server.rs` | 106 | 2 | 0 |
| `rutis-bridge/src/runtime/spawn.rs` | 767 | 0 | 0 |
| `rutis-bridge/src/runtime/unix.rs` | 75 | 2 | 0 |
| `rutis-bridge/src/runtime/process.rs` | 1933 | 39 | about 1720 |
| `rutis-bridge/src/transport/local/output.rs` | 259 | 4 | 0 |
| `rutis-bridge/src/session/rpc/limits.rs` | 188 | 0 | 0 |
| `rutis-bridge/src/events/errors.rs` | 340 | 2 | 340 (`report_line` stays) |
| `rutis-loader/src/config.rs` | 594 | 6 | 0 (moves into the kernel) |
| `rutis-loader/src/config_step.rs` | 255 | 1 | about 195 |
| `rutis-loader/src/loader/api.rs` | 650 | 11 | about 588 |
| `rutis-loader/src/runtime/go.rs`, `swift.rs` | 85, 83 | 12, 12 | about 45, 45 |
| `rutis-loader/src/runtime/binary.rs` | 2667 | 35 | about 2400 |
| `rutis-loader/src/backoff.rs` | 68 | 7 | 0 (moves to rutis-host) |

- Number of crates 17 → 16 (delete `rutis-dev`, `rutis-migrate`, add `rutis-types`); workspace members 29 → 28; crates.io publish list 10 → 10 (remove `rutis-migrate`, add `rutis-types`).
- Methods of the kernel's `CtxInternal` 18 → 9 (delete `get_key`, `require_key`, `provide_key`, `provide_checked`, `refresh`, `intercept_require_as`, `intercept_set_as`, `try_filter`, `try_filter_named`); methods of `CtxHost` 30 → 27. The kernel's 14 root-exported names do not change.
- rutis-loader root exports 20 items (12 names, 8 public modules) → about 18: remove `backoff`, `internal` (`peer` is removed by #811), add `patch`.
- The bridge's Cargo features go from 11 (`default`, `node`, `python`, `go`, `bun`, `kotlin`, `swift`, `websocket`, `exports`, `cordis`, `testing`) → 4 (`websocket`, `cordis`, `testing`, and an empty `default`); dependencies lose syn, quote, proc-macro2.
- `unsafe` in the kernel 4 places → 1 (only the link_section at `service.rs:1482–1488` remains).

### 9.2 Language SDKs (source files, excluding tests and node_modules)

| Package | Lines now | After (estimate) | Main changes |
| --- | ---: | ---: | --- |
| `node/rutis/src` | 2375 | about 800 | `config.mjs` 476 → about 20; `events.mjs` 558 → 0; `testing.mjs` 720 → about 350; `values.mjs` 261 → about 80; `index.d.ts` 82 → 47 |
| `node/rutis-runtime/src` | 7348 | about 6600 | `config.mjs` −476; `events.mjs` 558 → about 480 (the Cordis node keeps local dispatch); `client.mjs`, `io-worker.mjs` about −50; old-host branches about −80 |
| `bun/rutis-bun/src` | 4542 | about 3800 | `config.mjs` −476; `events.mjs` 558 → about 330; old-host branches about −50 |
| `python/rutis/rutis` | 7725 | about 6300 | `_eager` −161; `config` −626; `events` 553 → about 400; `testing` 826 → about 380; `__init__` 59 → 25 |
| `go/rutis` | 12991 | about 10000 | `internal/config` 1022 → about 120; `rutistest` 1648 → about 700; fast path about −250; old hosts about −80 |
| `swift` | 11488 | about 10200 | `ConfigSchema` −670; `Moved` −74; fast path about −250; `RutisTest` 673 → about 300 |
| `kotlin` (main artifact) | 9661 | about 8300 | `ConfigSchema` −566; `Moved` and others about −75; fast path about −200; `RutisTest` 691 moved out (new artifact about 300) |

Public surface:

| Entry | Now | After |
| --- | ---: | ---: |
| Node `index.mjs` runtime exports | 10 | 1 |
| Node `index.d.ts` `export`s | 20 | 4 |
| Python `__all__` | 12 | 5 |
| Go root package top-level exports | 52 | 19 |
| Swift `public-api.txt` (including deprecated) | 213 | 188 |
| Kotlin old names (typealiases and old constants) | 24 | 0 |

## 10. Effect on merged (and unmerged) 0.19 PRs

| PR | Content | What this design deletes or changes | Not affected |
| --- | --- | --- | --- |
| #767 L1 | `call_limit`, `kill_on_timeout`, `greeting`, the `timeout` code | delete `call_limit`, `kill_on_timeout`, `limits.rs`, `code::TIMEOUT`, the timeout branch in `report.rs`, the 6 time-limit tests in bridge `tests/plugin_limits.rs` | `greeting` and its two tests (the carrier moves to `rutis-host/spawn` with #811 S4) |
| #793 L2 | `allowed_services`; `modules` one process per plugin | delete `allowed_services`, `may_use`, the loader check, the `unavailable` branch in `report.rs`, 3 loader tests | `RuntimeResolver::modules`, `each_plugin_runs_in_its_own_process`, `a_runtime_waiting_for_a_debugger_stays_starting` |
| #796 L3 | docs and guarantees, the merged deadline in `request_sync` | the `deadline` parameter of `request_sync` is reverted; section 3 of open-host drops two subsections, the loader doctest `OpenHostGuide` changes; 3 rows deleted from the rust-host reference table; `host/plugin-limits` narrowed | "One process per plugin" and "Greeting limit" in section 3 of open-host |
| #762 E1 TS/Bun | the `./advanced` entry and forwarding | delete the forwarding at `index.mjs:49–51` and the 16 `@deprecated` in `index.d.ts` | `./advanced` and the public-surface tests |
| #763 E2 Python | `rutis.advanced` and `__getattr__` | delete `_MOVED`, `__getattr__`, the old names in `__all__`, the deprecated section and the deprecation test cases | `rutis.advanced` |
| #769 E3 Go | `advanced` aliases, 33 `Deprecated:` in the root package | delete the 33; implementations move into `internal/runtime`, and `advanced` points to it (7.6 item 2) | the subpackage split |
| #775 E4 Swift | `Moved.swift` | deleted | `Rutis.Advanced`, `_Generated`, the snapshot script |
| #777 E5 Kotlin | `Moved.kt` | deleted; version constants move into `Versions.kt`; "code generated by an old `rutis-ksp` still compiles" no longer holds | the advanced and generated packages |
| #778 E6 | a section of "Migrating to 0.19", the quick reference, ci.md, release.md | the migration section becomes "moved directly, no old names"; the rule at `release.md:36` is rewritten (7.6 item 1) | how the quick reference writes the new locations |
| #765 K1 / #803 K1d | contract descriptions (already deleted) | none | — |
| #798 K2 | Python types | the code moves to `rutis-types` with `build/` (§8) | generated content, the `Exports` API, `--python` |
| #810 K5 / H4 | docs, the test tool refusing service names the host already has | H4 is reported by the real kernel instead, and the two hand-written checks are deleted; the build.rs dependency in rust-host.md becomes `rutis-types` | the usage in python-plugin.md |
| #770 T4 | test tool `t.on` | the implementation becomes `ctx.on` in a test plugin on the real kernel; two tests are changed to run on the real kernel | the `t.on(name, listener, options)` form and the guide |
| #772 C1, #794 C2 | `rutis-host call`, the service rows of `dev` | `call.rs:107` and `dev.rs:611` switch to public API (6.3) | functionality |
| #801, #804 | guides, removing `--allow-call` | none | — |
| #805 P2 (not merged) | installing and `add`/`remove`/`upgrade` | after it is merged, delete `go add` (6.4 item 1); the `rutis_loader::internal::resolve_entry` it uses (diff line 1611) and `LocalRuntime::node/bun/python` (lines 1616, 1621, 1628) are deleted by this design and #811, see the order in §13; Node bare package names and the `go:`/`swift:` row names agree with this design | the design of `add`/`remove`/`upgrade`/`installed` |
| #768 R1, #791 R2 | remote | handled by #811 §10 | — |

Designs to change (add a line at the top, "Partly rewritten by [0.19 platform cleanup](design-platform-cleanup-2026-10-28.en.md)", and change the corresponding sections):

| Design | Sections | What changes |
| --- | --- | --- |
| [SDK entry convergence](design-sdk-entry-convergence-2026-10-27.en.md) | §2 decision table, all of §5, §6, §7, §8, §9 steps 2, 3, 5 | old locations no longer work; delete "why not delete directly in 0.19" and the reason Python cannot delete; the hint-text table becomes compile errors or import errors; in the done conditions, `DeprecationWarning`, SA1019 and "0.17.2's `rutis-ksp` compiles" are deleted |
| [The host keeps plugins in bounds](design-host-plugin-limits-2026-10-27.en.md) | title, header, §2, §3, §4 H2/H3/H5, §4.2, §4.3, §5 items 1–4, §6, §7 items 2, 3, 5, §8 steps 1–3 | only §4.1 (H1 one process per plugin) and §4.4 (greeting) remain; the header states that the call time limit and the service list were added and then deleted within 0.19, never released |
| [Host contract types](design-host-contract-types-2026-10-27.en.md) | §3 H1, §4.1, §5, §7 | the crate holding `Exports` becomes `rutis-types` (API unchanged); H4 becomes a refusal by the real kernel; "the protocol, kernel, loader and language runtimes do not change" is rewritten |
| [Config Schema](design-config-schema-2026-10-27.en.md) | §2.1, §2.2, §2.3, start of §6, §6.1, §7 item 3, §8, §11 steps 2–5 | validation lives once in the kernel, SDKs only decode; the Cordis row exception stays; the special case of `required` for Go before 0.18 is deleted; the case table tests only the kernel and `check` |
| [The host writes its own plugins](design-host-writes-plugins-2026-10-27.en.md) | L3 (lines 41, 148) | "if the host needs limits, use #683's `allowed_services`" becomes "limiting which plugins can use `loader` is the host's business, handled by the host on its own rows (for example `isolate`)" |
| [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) | last sentence of §6.1 | delete "a host that wants to limit third-party plugins' resources belongs to 0.19's #683" |
| [Plugin install](design-plugin-install-2026-10-27.en.md) | compatibility section, I1 | `go add` deleted (with CL25) |
| [Events](design-events-2026-10-18.en.md), [event protocol](design-events-protocol-2026-10-18.en.md) | event protocol §5.1, §5.2, §15 item 5; the sections where `waterfall-sync` appears (:154–192,285,411,434,665–668,883) | delete the local fast path and `events.elsewhere`; delete `waterfall-sync` from the mode table |
| [Kernel design v5](design-rust-port.en.md) | header | the note in the last paragraph of 3.3 |

## 11. Compatibility and "Migrating to 0.19"

No compatibility. Old forms are deleted directly in 0.19, without deprecation marks and without special conversions or hints: deleted Rust APIs are compile errors, deleted SDK names are import errors or compile errors, and deleted row-name forms are reported as "not recognised" under 0.18's rules.

"Migrating to 0.19" gets a "Platform cleanup" section (alongside #811's "Remote connection" section), which only gives the new forms and the mapping:

1. **Rust API**:

   | 0.18 | 0.19 |
   | --- | --- |
   | `ctx.plugin_from(\|cfg\| …)` | implement `rutis::host::PluginFactory`, `ctx.plugin_with(factory, config)` |
   | `waterfall_sync` | use `waterfall` (async) or synchronous `bail` |
   | `take_cleanup_errors()` | cleanup errors are in the unload result (the error `dispose` returns) |
   | `FiberView::effects()`, `EffectMeta`, `EffectPhase` | no replacement; for diagnostics use `diagnostics()` |
   | `EventBus::observe_dispatch` | no replacement; to observe, register an ordinary listener |
   | `internal::intercept_*`, `internal::provide_checked`, `refresh` | no replacement |
   | `internal::{get_key, require_key, provide_key}` | `Key::of/named/dynamic(..).instance(..)` with `get_at`/`require_at`/`provide_at` |
   | `internal::config::{closest, decode, …}` | `rutis::config::closest`; validation `rutis::config::check` |
   | `rutis_loader::config::check(schema, config, options)` | `rutis::config::check(schema, config)` |
   | feature `anyhow`, `Error::from_anyhow` | `.map_err(\|e\| Error::msg(format!("{e:#}")))` |
   | `ServiceCatalog::share_by_name()`, `share_registered_only()` | delete the line (sharing by name is the default) |
   | `Loader::waiting_for`, `locate`, `schema_of` | `waits`/`waits_of`, `row`, `resolve(name).await?.schema` |
   | the 16 aliases `GoBinaries`, `GoResolver`, `GoRuntimes` and so on | `binary::*` plus `Go`, `Swift` |
   | `LocalRuntime::call_limit`, `kill_on_timeout`, `allowed_services`, error code `timeout` | callers set their own time limits; after a timeout, `processes().kill()` plus `FiberView::restart()`; which services may be used is decided by the host itself |
   | `rutis_bridge::build::Exports`, build-dependency `rutis-bridge = { features = ["exports"] }` | `rutis_types::Exports`, build-dependency `rutis-types`; API unchanged |
   | `Loader::set_overlay`, `overlays()` | deleted; to override a row, write it in your own layer |
   | `rutis_bridge::cordis::build::rutis_plugin`, `cordis::server` | deleted (the direction of a Cordis host mounting Rust plugins is no longer supported) |
   | `rutis_bridge::Launcher::node` and the like, the bridge's language features | the rutis-host library (`rutis-host/spawn`); tests use `rutis-test-support` |
   | `rutis_loader::internal::*` | `rutis_loader::patch::apply_patches`, the public `is_runtime_name`, `read_binary_manifest`, `Go::PLUGIN_API` |
   | wire error name `CordisError`, generated type `CordisErrorThrown` | `Error`, `ErrorThrown` |

2. **rutis.json and row names**: the Go/Swift forms `go-<file>:<plugin>` and `swift-<file>:<plugin>` become `go:<plugin>` and `swift:<plugin>`; when two binaries provide plugins with the same name, rename one of them. Node row names do not change.
3. **Go plugins**: rebuild with SDK 0.18 or later (the `required` exception for binaries built before 0.18 is deleted).
4. **Old SDK locations**: the mapping table in the #692 section keeps the "new location" column, and drops the "what hint you see" column and the "it still runs if you change nothing" paragraph; it states that Python plugins must be upgraded together with the host, and that Kotlin's `rutis-ksp` is the same version as the SDK.
5. **Python**: minimum 3.12.
6. **Test tools**: the API does not change; running them needs `rutis-host` (on `PATH` or `RUTIS_HOST`); Node projects add `@arcships/rutis-runtime` and `@arcships/rutis-host` as dev dependencies; the dependency coordinates of the Kotlin test tool become `rutis-testing`; tests on devices go through the application installing its own plugins, not `rutis-host test`.
7. **Events**: in Kotlin, synchronous `bail` on the Android main thread is always refused; in Go, `emit` after the session ends reports that the session has ended.
8. **`rutis-host go add`**: use `rutis-host add <module path>` instead (it also writes a row).
9. **Tools**: `rutis-dev` is deleted; `Exports` for generated types moves to the new crate `rutis-types`, build.rs changes to depend on it, API unchanged; `rutis-migrate` is no longer released with 0.19 (the 0.17 migration can still be done with `cargo install rutis-migrate --version 0.18.0`).
10. **Upgrade together**: same as #811 §11 item 5 — the host, the language runtimes and SDKs all upgrade to 0.19 together; protocol version 4 and 0.18 do not meet.

## 12. Docs to update alongside

| Doc (both languages) | What changes |
| --- | --- |
| `docs/guarantees.md` | Delete: `events/v2-required`, `events/presence`, `host/go-add`, `loader/overlay`. Change: `core/D4`, `core/D16`, `core/D17`, `events/waterfall` (remove `waterfall_sync` and its tests); `core/D11` (remove `take_cleanup_errors`); `core/D14` (evidence becomes the new tests in 3.4 item 4); `core/D18`, `events/reserved`, `events/serial-bail` (replace tests); `events/old-peer-refused`, `bridge/cross-version-meetings` (refused by protocol version 4, together with #811); `events/hub-filter` (filtering always in the host); `events/one-bus`, `sdk/testing-matches-runtime` (the test tool is the real kernel; the differences section is deleted); `sdk/undeclared-use` (reported by the kernel); `sdk/config-schema` (only the kernel and `check`); `sdk/leaf-events` (delete the `refuseRemoved` sentence); `host/output-prefix`, `host/output-lines-whole` (test paths); `loader/go-runtimes`, `loader/swift-runtimes` (only the language-prefix form remains; restart policy is in rutis-host); `host/runtime-restart` (test paths); `loader/schema`, `bridge/marked-services` (the forms of `waiting_for`, `schema_of`, generated types); `host/plugin-limits` (narrowed); `host/check` (remove "0.12 SDK"); `boundary/5.6` (after the fixture and test are rewritten, the test name is unchanged); delete the dangling label `events/waterfall-sync` at `tests/events.rs:602`. Each row's test names change with the tests, and the counts in §15 are recomputed |
| `docs/guide/rust-host.md` | `plugin_from` (:39,89), `take_cleanup_errors` (:146), `CordisError` (:145,718), interception (:238), `share_registered_only` (:434), `LocalRuntime::*` (:426–560, becomes the rutis-host library), the limit options table (:512–523), `waterfall_sync` (:641,654), `waiting_for` (:243,487), Go aliases (:539–547 doctest), the dependency and path of `Exports` become `rutis-types` (:246,280,667–675,731–748, API unchanged), `report_line` (:658, stays), `RUTIS_TRACE` (:496,574, stays) |
| `docs/guide/rust-plugin.md` | `anyhow` (:495), `plugin_from` (:615, doctest) |
| `docs/guide/plugin-api.md` | `waterfall_sync` (:68,72,86,212), events-v2 errors (:227,230), the open-host sentence at :253, test tool differences (:421–432), the Python deprecation rule (:478), Python 3.10 (:265,452) |
| `docs/guide/open-host.md` | section 3 drops "call time limit" and "services a plugin may use"; `share_registered_only` (:129); example code rewritten per #811 |
| `docs/guide/rutis-host.md` | `go add` (:78,96), events-v2 (:179), `required` for Go before 0.18 (:238), Go/Swift row names (:513,596), Python 3.10 (:7), `rutis-host types`, `rutis-host test` |
| `docs/guide/go-plugin.md` | `go add` (:411), qualified row names (:414), `GoResolver` (:414), the description of `rutistest` (:233–296) |
| `docs/guide/typescript-plugin.md`, `bun-plugin.md`, `python-plugin.md`, `swift-plugin.md`, `kotlin-app.md`, `swift-app.md` | the test tool section (real host, dev dependencies, how to do it on devices); Python 3.12 (`python-plugin.md:3`); the Kotlin test artifact coordinates; the deprecation rule at `python-plugin.md:389` |
| `docs/guide/README.md` | Python 3.10 (:38) |
| `docs/migration-0.19.md` | §11 |
| `docs/core-features.md` | :7,23,45,46,48,55,59,63,81 |
| `docs/development-handbook.md`, `development-guide.md` | `plugin_from`, `provide_checked`, `waterfall_sync`, `take_cleanup_errors` |
| `docs/quality-status.md` | `rutis-dev`, Python 3.10 |
| `docs/design-philosophy.md` | §6 Python 3.10+ becomes 3.12+ |
| `docs/release.md` | delete `rutis-migrate` from the publish list and add `rutis-types` (:9); `PLUGIN_API` and the deprecation rule (:36); the paths of the Go/Kotlin version constants (:18) |
| `docs/ci.md` | the `rutis-dev` path filter (:80); the deprecated section of the public-surface check (:99,107); bridge/loader feature combinations (:103) become `RUTIS_TEST_RUNTIMES`; `requires-python` (:276,288,294,342,344); Swift/Kotlin test tools (:108,109); the `anyhow` job |
| `.agents/skills/rutis-new-runtime/SKILL.md` | :72 (test tool), :54,117 (capabilities), :74–77 (new languages no longer add `Launcher`/`LocalRuntime`/a feature in the bridge; they go in rutis-host), :120 (local forwarding) |
| `.agents/skills/rutis-new-runtime/references/runtime-contract.md` | capability table :34–42 (delete `sync-wait`, `signals`, `objects`, `events-v2`; `plugins` stays), :65 (`events`/`emits`), :113–123 (`events.elsewhere`, fast path, `waterfall-sync`), :147 (old hosts), :174 (test tool expectation files) |
| `.agents/skills/*/references/plugin-api.md` (two copies) | `waterfall_sync` (:58,62,76,200), events-v2 (:215,218), test tool (:299–309), Python 3.10 (:329) |
| `.agents/skills/rutis-remote-plugin/SKILL.md` | `waiting_for` (:48) |
| `specs/config-schema/README.md` | :5, :16 |
| `specs/cross-runtime-sync/README.md` | the `sync-wait` capability (:15) |
| templates (`crates/rutis-host/templates/`) | `python/pyproject.toml:6` (3.12); `go/README.md:11` (`add`); the `package.json` of `node` and `bun` add dev dependencies; each template's tests are rerun with the new test tool |
| `README.md`, `CONTRIBUTING.md`, `python/rutis/README.md`, `go/rutis/README.md`, `swift/README.md`, `kotlin/README.md`, `bun/rutis-bun/README.md`, `crates/rutis/README.md`, `crates/rutis-loader/README.md`, `crates/rutis-bridge/README.md`, `examples/native-mount/README.md`, `examples/marked-services/README.md` | Python 3.12; test tool paragraphs; `LocalRuntime` language constructors; the reverse-mount line; `EventPattern`/`waterfall_sync`; the crate that holds `Exports` |

Historical docs (old designs, `review-*`, `research-*`, migration guides and release notes for 0.18 and earlier) are not changed.

## 13. Implementation

### 13.1 Rules

1. **Each PR is self-contained**: when a PR deletes something, it changes every use of it in all crates, language SDKs, tests, docs, examples and templates in the same PR. Each PR below lists everything it covers.
2. **No compatibility added to pass tests or CI**: no forwarding, fallback branches, old-name aliases, feature switches or "keep for now". Tests that fail because of deleted behaviour are rewritten or deleted in the same PR; if that is not possible, stop and report; do not work around it.
3. **When a self-contained change is too large, split it by dependency**: first add the new thing, switch all users to it, then delete the old one; after each step is merged, main works and CI passes. What must change together is marked "same PR".
4. **Aligned with #811**: changes that touch the same code go into the same self-contained change, or their order is stated (the "Depends on" column below). #811's PRs: S1 Channel, S2 session, R1–R6 local endpoint for each language, A1/A2 apps install plugins themselves, S2b delete compat, S3 loader unified resolver, S4 rutis-host's three connection plugins, S5 deletion, S6 docs.
5. SDK changes are one PR per language (Node and Bun share `@arcships/rutis` and the `events.mjs` copies, and count as one self-contained change).
6. Each PR adds a line to #771 before it is opened (rule 2 of #771).

### 13.2 PRs

Numbered CL (cleanup). "Covers" lists the files or directories to change and the tests to delete or rewrite.

**Kernel**

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL1 kernel: interfaces with no production callers | delete `EventPattern`, `observe_dispatch`/`DispatchAttempt`/`DispatchObserver`, `take_cleanup_errors` (three places), `FiberView::effects`/`EffectMeta`/`EffectPhase` (label moves to `EffectRecord`), the `anyhow` feature and `from_anyhow`, `try_filter`/`try_filter_named`; the bridge's `Connection::independent_future` | `crates/rutis/src/{key,bus,host,ctx,fiber,effect,error,internal,events/api}.rs`, `Cargo.toml`, `public-api.txt`; tests: delete `dispatch_observation.rs`, `effect_tree.rs`, change `cleanup_errors.rs`, `author_api.rs`, `typed_dispatch.rs`, `bus/transient_tests.rs`, bridge `tests/event_hub.rs:984` (probe becomes an ordinary listener), bridge `tests/rpc_callbacks.rs:445,499` (delete), loader `events_conformance/rust_participant.rs:354`; `ci.yml:493–495`; docs `core-features`, `development-handbook`, `rust-host`, `rust-plugin` (anyhow), guarantees `core/D11`, `core/D18`, `events/serial-bail` | `git grep` for these names under `crates/ docs/guide .agents` finds nothing; the Stuck hint tests still pass | — |
| CL2 kernel: service interceptors | delete all interceptors and the `actor` of `require_provided_as`; `intercept.rs` → `writer.rs` | kernel `intercept.rs`, `internal.rs`, `ctx.rs`, `error.rs`, `service.rs`; `rutis-macros/src/lib.rs:460–464`; bridge `marked.rs:72–78,1055`; tests: split `service_writer.rs` out of `service_intercepts.rs` and delete the interception part; bridge `marked_services.rs:739`, the two tests in `projection_lifecycle.rs` per 3.4 item 5; guarantees `bridge/marked-services`; `rust-host.md:238`, `core-features.md:63` | macro-generated code compiles; all `ServiceWriter` cases are kept and pass | CL1 |
| CL3 kernel: `check()` gating | delete `provide_checked`, `refresh`, `CheckFn`, `service::Check`, `Exposure::check`, the three `DependencyStatus` variants | kernel `registry.rs`, `internal.rs`, `ctx.rs`, `diagnostics.rs`, `service.rs`, docs at `host.rs:133`, `public-api.txt`; bridge `marked.rs:219–223`, `tests/marked_services.rs:391,431,554,564,569`; tests: rewrite `load_window.rs` per 3.4 item 4, delete the check tests in `contract.rs` and that one in `lifecycle_diagnostics.rs`, change `parity.rs:345`; guarantees `core/D6`, `D7`, `D14`, `loader/rows-gate-on-services` (evidence becomes the new tests), `bridge/marked-services`; `core-features.md:71`, `development-handbook.md:137` | the new evidence tests for the four guarantees exist and pass | CL2 (both change `marked.rs`) |
| CL4 `waterfall_sync` (Rust side) | delete the kernel's `waterfall_sync` and `Mode::WaterfallSync`, synchronous next and the 3 `unsafe`; the branches in bridge `events/hub.rs:1160,1254`, `events/link.rs:495,578`; the test tool `runtime/testing/events.rs` and the `waterfall-sync/*` cases in `events.expected.json` | kernel `host.rs`, `ctx.rs`, `events.rs`, `events/api.rs`, `events/remote.rs`, `bus.rs`, `bus/run.rs`, `lib.rs:38`; delete `examples/sync_decision.rs` (and the links outside `migration-0.3-to-0.5`); `benches/events.rs:79–83`; loader `rust_participant.rs:407–409`; bridge `tests/events_expected.rs:39`; guarantees `core/D4`, `D16`, `D17`, `events/waterfall`, the dangling label at `tests/events.rs:602`; docs `plugin-api.md`, `rust-host.md:641,654`, `core-features.md`, `development-*`, `crates/rutis/README.md`, the mode table of the event protocol, the `plugin-api.md` of the two skills | `grep -ri waterfall.sync crates/ docs/guide .agents` finds nothing; the SDKs' receiving branches then have no sender, and are deleted by CL26-* | — |
| CL5 wire error name `CordisError` → `Error` | change `service.rs:1024`; delete the generator's old-path recognition; generated files and tests | `crates/rutis/src/service.rs`; bridge `build/exports.rs:479–491,1856–1858,1876`; `examples/rust-services/rows/services.d.ts`, `examples/marked-services/rows/services.{d.ts,py}`; bridge `tests/marked_services.rs:274`; `rust-host.md:145,718` | the snapshot tests of both examples and `tsc`/`pyright` pass | — |
| CL6 `*_key` → `Key` + `*_at`; review `platform()` | delete `internal::{get_key, require_key, provide_key}`; callers hold a `Key<T>`; review `platform()` per 3.3 | kernel `internal.rs`, `ctx.rs`, `deps.rs`; bridge `marked.rs:172,223`, `runtime/rows.rs:51,317`, the rest of `services.rs`; loader `catalog.rs:120,125`, `runtime.rs:878,1008`, `runtime/rows.rs:122`; rutis-host `host.rs:1523` (test) | `git grep -E '(get|require|provide)_key' crates/` finds nothing; the handling of `platform()` is written in the PR description | #811 S5 (the deleted files still contain call sites) |
| CL7 `plugin_from` | delete | kernel `host.rs:239`, `ctx.rs:1421`; `examples/development_workflow.rs:134`; tests `instance_subtrees.rs:411`, `lifecycle_order.rs:276`, `typed_plugin.rs:375`; `rust-host.md:39,89`, `rust-plugin.md:615` (doctest switches to `PluginFactory`), `core-features.md:7,23`, `development-handbook.md:98,101,336` | doctests pass | — |
| CL8 config: validation moves into the kernel | move `rutis_loader::config::check` into `rutis::config`, delete `Options`/`required`/`older_than_0_18`; the hint helpers leave `internal` (`closest` public); the loader sends the config with defaults filled in to the runtime; the bridge calls `check` before loading plugin values | kernel `config/`, `internal.rs:13–17`; loader `config.rs` (deleted), `config_step.rs`, `loader/waits.rs:439,441`, `tests/config_cases.rs` → kernel tests; bridge plugin-value loading in `runtime/process.rs`, `events/errors.rs:240` (test); rutis-host `call.rs:107`, `status.rs:580` (test); `rutis-host.md:238`; config Schema design §2.1, §7 item 3 | the 105 cases pass in the kernel; a bad config for a plugin value is refused on the bridge side (new test); the SDKs' existing config tests still pass unchanged (SDK validation is still present at this point, with the same results) | — |

**rutis-bridge**

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL10 reverse Cordis mount | delete `rutis_plugin`, `cordis::server`, `runtime/unix.rs`; on the Node side `launch`/`dispose`/`executable`/`io-worker.mjs:55–79`; the syn check at `cordis/build.rs:291` becomes hand-written; `cordis` no longer depends on `exports` | bridge `cordis/build/rust.rs`, `cordis/build.rs:5–6,291`, `cordis/mod.rs:16`, `cordis/server.rs`, `runtime/unix.rs`, `Cargo.toml`; `node/rutis-runtime/src/client.mjs`, `io-worker.mjs`, `test/fixtures/rust-mount.test.mjs` (deleted), `bench/sync-call.mjs` (deleted); in `examples/native-mount`: `src/lib.rs`, `src/bin/rutis-counter.rs`, `tests/cordis_mount.rs`, `tests/export_identity.rs`, `build.rs:9–14`, README; bridge `tests/rpc_callbacks.rs:305` goes through a local pipe (test name unchanged) | the tests of `boundary/5.6` pass; dsh, `examples/dsh-baseline` and `interop-experiments` build | — |
| CL11a bridge: stop checking the peer's old capabilities | the bridge no longer requires the peer to declare `events-v2`, and no longer changes behaviour based on the peer's `objects` and `sync-wait`; delete the old feature checks in the mount layer (0.3.0, `require_plugin_values`, the old path without `scopes`); delete the self-check of `plugins`; the bridge itself still declares these capabilities | `session/rpc.rs:1294,1579,1838,2421,2461,2500`, `relay.rs:41`, `crossing.rs:171–175`, `runtime/process.rs:1087,1231,1251,1265,1329,1517–1524,1577,1607`; delete tests `rpc/tests.rs::a_far_end_without_events_v2_is_refused`, `event_hub.rs::a_runtime_without_events_v2_is_refused`, `rpc/tests.rs:2676–2706`; guarantees: delete `events/v2-required`, change `events/old-peer-refused` to refusal by protocol version | peers from 0.18 and earlier are refused at handshake (#811 S2b has already raised it to version 4) | #811 S2b |
| CL26-node / bun / py / go / swift / kotlin each language: old-host branches and old capabilities | one PR per language: delete all of this language's branches in the 7.2 table (`announce`, the `events-v2` requirement, `refuseRemoved`, the plugin API default, `rows.failed`, the peer checks of `objects` and `sync`, `waterfall-sync` receiving); stop declaring `events-v2`, `objects`, `sync-wait`, `signals`; the Python PR also raises the minimum version to 3.12 (7.3); the Go PR also deletes `warnChainless`; the Kotlin and Swift PRs change the "older host answers with a future" comment to the current behaviour (7.6 item 11) | the files the 7.2 table lists for this language; tests: delete Go `oldhost_test.go`, `events_session_test.go:455`, delete the old-host cases in Node `runner-events.test.mjs` and Bun `test/old-host.test.ts`, the Go fixture `internal/fixtures/session/main.go:193` and `no-objects-declared` at `objects.expected.json:180` (the case is deleted in the last language's PR); this language's `public-api` snapshot; Python: `_eager.py`, `_unhandled.py`, `tests/test_eager.py`, the three `requires-python`, 3.10 in ci.yml and install.yml, 3.10 in docs; Go: `runtime.go:1165–1192`, `events.go:537`, `runtime.go:1075`, `design-go-runtime-2026-10-10.md:257` (both languages) | all of this language's tests and `runtime_conformance` pass; `git grep` finds no `events-v2` or `waterfall-sync` in this language's source | CL11a; this language's #811 R-x and S2b (they change the same `session` files); Kotlin and Swift also after A1, A2 |
| CL11b bridge: stop declaring old capabilities | the bridge deletes the declarations of `EVENTS_CAPABILITY`, `signals`, `forwarding`, `objects`, `sync-wait`; the capability table in `.agents` | `session/rpc.rs:922,1243–1251`, `session/testing.rs:201`, `rpc/tests.rs:1033`; `runtime-contract.md:34–42` | `grep` finds none of these capability names under `crates/rutis-bridge/src` or the six SDKs' source | all of CL26-* |
| CL12a process-starting helpers for tests | `rutis-test-support` adds helpers to start each language's local runtime and `RUTIS_TEST_RUNTIMES`; the tests of bridge, loader and examples all switch to them; each CI job sets `RUTIS_TEST_RUNTIMES` instead of `--features` | `crates/rutis-test-support`; 17 bridge test files, about 20 in the loader, `tests/common/runtimes.rs:218,243`, the tests of `examples/rust-services` and `marked-services`; `ci.yml`, `stress.yml`; `docs/ci.md:103` | each language job in CI actually runs that language's tests (no skipped in the logs) | #811 S5 (`LocalRuntime` already deleted, tests already go through `pipe::spawn`) |
| CL12b move language knowledge out of the bridge | delete the bridge's `Launcher::node/python/go/swift/bun`, `node_inherits`, `Process::launch*`/`attach`, `runtime/spawn.rs`, language features; `Process::mount` goes through `pipe::spawn`, and the Node command goes into the `cordis` module; `go_runtime_name`/`swift_runtime_name` move to the loader; output line splitting and prefixes (`output.rs`, `Stdio::Lines`, `output_prefix`, `prefix_output`) move to rutis-host; delete the loader's `python`, `bun`, `kotlin` features | bridge `runtime/process.rs`, `runtime/spawn.rs`, `runtime/mod.rs`, `transport/local/output.rs`, `transport/local/spawn.rs`, `Cargo.toml`; loader `Cargo.toml`, `runtime/go.rs`, `swift.rs`, `binary.rs:1550–1557,2293–2310`; rutis-host `lib.rs`, `host.rs:119–121`, `languages/*.rs`, `Cargo.toml:33`; `examples/*/Cargo.toml`, `tests/e2e/fixture-tenants`, `fixture-toolhost` (switch to the rutis-host library); guarantees `host/output-lines-whole`, `host/output-prefix`; `rust-host.md:426–560`, `crates/rutis-bridge/README.md:5`, `crates/rutis-loader/README.md:17`, `bun/rutis-bun/README.md:28`, `rutis-new-runtime/SKILL.md:74–77` | `cargo tree -p rutis-bridge` shows no language features; dsh builds and dsh tests pass; the prefixed output of `rutis-host run` is unchanged word for word | CL12a, #811 S4, S5 |
| CL13 #683 call time limit and service list | delete per 5.5; revert the `deadline` of `request_sync` | bridge `runtime/local.rs`, `plugin.rs`, `rows.rs`, `session/mod.rs`, `session/rpc.rs`, `limits.rs`, `relay.rs`, `report.rs`; loader `runtime.rs:1015–1019`, `lib.rs` (doctest `OpenHostGuide`); tests bridge `tests/plugin_limits.rs` (keep the two greeting tests), loader `tests/plugin_limits.rs` (keep two); guarantees `host/plugin-limits`; section 3 of `open-host.md`, `rust-host.md:512–523`, `plugin-api.md:253`; designs `design-host-plugin-limits`, `design-host-writes-plugins` L3, the last sentence of `design-diagnostics` §6.1 | `git grep -E 'call_limit|kill_on_timeout|allowed_services|may_use' crates/ docs/guide` finds nothing | before #811 S4 (so S4 has less to move when it moves `LocalRuntime`) |

**rutis-loader**

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL17 loader: duplicate and unused interfaces | delete `share_by_name`, `share_registered_only`, `by_name`; `waiting_for`, `locate`, `schema_of`; `register_instance_keyed`, `readable_instance_keyed`, `readable_instance`; the 16 Go/Swift aliases, `read_go_manifest`, `read_swift_manifest`; `GO_MARKER`, `SWIFT_MARKER` in `internal` | loader `catalog.rs`, `loader/api.rs`, `runtime/go.rs`, `swift.rs`, `binary.rs:1044,1080`, `runtime.rs:27,33,49`, `lib.rs`; rutis-host `host.rs:216`; `tests/e2e/fixture-toolhost/src/main.rs:171`; tests `runtime_rows.rs:1036`, `fault_containment.rs:170`, `remote_modes.rs:199`, `runtime_instances.rs:336`, `check.rs:106`, `scope.rs:52`, `marked_services.rs:253,458,466`, `loader.rs:458,460`, `peer_rows.rs:230`, `instance_services.rs:170`, `swift_rows.rs:18,130` and the dozen-plus tests that use the aliases; guarantees `bridge/marked-services`, `loader/schema`; `rust-host.md:243,434,487,539–547`, `open-host.md:129`, `go-plugin.md:414`, `rutis-remote-plugin/SKILL.md:48`, `design-host-writes-plugins` §9 | `git grep` for these names under `crates/ docs/guide .agents tests/e2e` finds nothing | before #811 S5; #811 S5 no longer does the `share_registered_only` row |
| CL18 loader: close up `internal` | make `apply_patches` and `Composed`, `ComposedRow`, `Owner`, `PatchWarning` public as `rutis_loader::patch`; make `is_runtime_name`, `read_binary_manifest` public; rutis-host switches to `Go::PLUGIN_API`, `Swift::PLUGIN_API`; delete the `internal` module | loader `lib.rs:89–105`, `patch.rs`; dsh `profile/mod.rs`, `layers.rs`, `include.rs` and dsh tests; loader tests `loader.rs`, `patch_parity.rs`, `regressions.rs`; rutis-host `config.rs:608`, `dev.rs:611`, `project.rs:133`, `languages/go.rs:170`, `main.rs:32,33` | `git grep 'rutis_loader::internal' ` finds nothing; dsh tests pass | #811 S5 (`REMOVED_EVENTS`, `node_schema`) |
| CL19 loader: delete npm resolution in Rust | per 5.4 item 1: delete the `npm` module, `package_version`, `resolve_entry`, the local branch of `Naming::Npm`, the `url` dependency; unprefixed names are passed to the Node runtime to resolve | loader `runtime.rs`, `Cargo.toml`; rutis-host `languages/node.rs:84`; after #805 is merged, the call to `resolve_entry` in its `installer` is changed to hand off to the runtime (same PR); tests: the unit tests in `runtime.rs`, the loader tests that use local npm resolution | the examples in the `typescript-plugin` guide, the template `node` project, and dsh's Cordis rows pass end to end | #811 S3 (changes the same `runtime.rs`); if #805 is merged first, it is changed here |
| CL20 Go/Swift keep only language-prefix row names | per 5.4 item 2 | loader `binary.rs:990–1012,1084–1094`; rutis-host `languages/go.rs:107–109,164–178`, `swift.rs`, `dev.rs:922–926`, `project.rs:345,428`; persisted samples (`tests/fixtures/persisted/*`) drop the qualified form and add `rejected/` cases; the 33 `"go-…:…"` and 14 `"swift-…:…"` in tests; guarantees `loader/go-runtimes`, `loader/swift-runtimes`; `rutis-host.md:513,596`, `go-plugin.md:414`, `.agents/.../rutis-json.md:212` | under `dev`, changing one Go plugin reloads only it; the error for a same-name collision names both files | CL19 |
| CL21 merge a row's state into one set | delete `status::RowState`, snapshots use `EntryStatus`; rename `report::RowState` to `RowEventState` | loader `loader/status.rs`, `framework.rs:18`, `lib.rs:75`; rutis-host `call.rs:10,137`, `status.rs`; tests `status.rs`, `row_events.rs`, `events_conformance.rs` | the output of `rutis-host status` and `call` is unchanged word for word; the values of the `rutis/row` event do not change | CL13 (`plugin_limits.rs` uses `status::RowState`) |
| CL22 move restart policy to rutis-host | per 5.2; the loader keeps only `restart(name)`, state, on-demand start and idle stop | loader `runtime/binary.rs`, `backoff.rs`, `lib.rs`; rutis-host `restart.rs`, `host.rs:22,450–535,831`, `languages/binary.rs`, `config.rs:350`, `main.rs:329,340`, `dev.rs`; guarantees `loader/go-runtimes`, `host/runtime-restart`; `rust-host.md:557` | the restart output after Go and Node runtimes crash is identical word for word, and there is only one implementation | #811 S4 (binary runtimes become spawn rows), CL12b |

**rutis-host and tools**

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL23 delete rutis-dev and the overlay layer | delete the crate; delete the loader's `set_overlay`, `overlays()` and overlay state (5.4 item 5) | loader `loader/api.rs:97,115`, `loader/mod.rs:412,528–536`, `loader/commit.rs:312–332`, `tests/overlay.rs`; guarantees `loader/overlay`; root `Cargo.toml`, `Cargo.lock`, `ci.yml:175`, `nightly.yml:112`, `docs/ci.md:80`, `docs/quality-status.md`, the status line of `design-host-dev-mode` | the workspace builds; `set_overlay` and `overlays` are not found under `crates/` | — |
| CL24 delete rutis-migrate | delete the crate, leave the release train | root `Cargo.toml`, `Cargo.lock`, `scripts/train.mjs:26–27`, `release.yml:8`, `docs/release.md:9` | `node scripts/train.mjs --crates` no longer lists it; the CI packaging check passes | — |
| CL25 delete `go add` | per 6.4 item 1 | rutis-host `main.rs:11,76–78,100,383–416`; guarantees `host/go-add`; `rutis-host.md:78,96`, `go-plugin.md:411`, `templates/go/README.md:11`; the compatibility section and I1 of `design-plugin-install` | — | #805 merged |
| CL27 `rutis-host test` | add `rutis-host test` (a session over standard input and output, `--listen`) and the plugin `rutis-host/test` | rutis-host `main.rs`, new `test.rs`; end-to-end tests in `tests/`; `rutis-host.md` | minimal Node and Python test plugins run load, on, emit, update, unload through it | #811 S4 (session and listen) |

**Generated types**

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL15 move the generator out of the platform | per §8: the three files in `build/` move unchanged to the new crate `crates/rutis-types`; delete the bridge's `pub mod build`, the `exports` feature and the syn, quote, proc-macro2 dependencies; rutis-host and the two examples change their dependencies | bridge `lib.rs:32`, `build/`, `Cargo.toml:33–35,77`; new `crates/rutis-types` (`Cargo.toml`, README in both languages); the members in root `Cargo.toml`, `Cargo.lock`; rutis-host `Cargo.toml:32`, `main.rs:150–184`; `examples/rust-services` (build-dependency, the `use` in `build.rs`), `examples/marked-services` (dev-dependency, `tests/rows.rs`); `scripts/train.mjs`, `docs/release.md:9`; the test paths of guarantee `bridge/marked-services`; the dependency and paths in `rust-host.md:246,280,667–675,731–748`; `design-host-contract-types` §3, §4.1 | the generated output of both examples is identical word for word to the committed files; the output of `rutis-host types` does not change; `cargo tree -p rutis-bridge` has no syn | CL5, CL10 |

**Language SDKs** (one group per language, in order within the group)

| PR | Content | Covers | Done when | Depends on |
| --- | --- | --- | --- | --- |
| CL28-node, -py, -go, -swift, -kotlin #692 old names | delete this language's forwarding per 7.1; Go first moves the implementations into `internal/runtime` (same PR); Kotlin moves the version constants into `Versions.kt` | the files the 7.1 table lists for this language and the public-surface snapshot; Node `testing.mjs:37`, `testing.d.ts:1`; `scripts/train.mjs:43,48`, `release.md:18` (Go, Kotlin); the first PR merged also changes the rules in `release.md:36`, `plugin-api.md:478`, `python-plugin.md:389` and §5 of the SDK entry design; each PR changes this language's line in the #692 section of "Migrating to 0.19" | this language's public-surface snapshot has no deprecated section; no old names in the repo | — |
| CL29-bench event benchmark | add `tools/bench/events` per 7.5, record the before numbers | `tools/bench/events`, `docs/performance-*` (a new one, both languages) | the before numbers are committed in the doc | — |
| CL30-node, -py, -go, -swift, -kotlin leaves are only leaves | this language's test tool becomes a real runtime + `rutis-host test`; delete the parts simulating host rules, local event dispatch (Node keeps one copy for the Cordis node), the SDK's config validation (decoding stays); the Kotlin test tool is split into the `rutis-testing` artifact | the files the 7.5 tables list for this language; delete this language's tests that run `cases.json`; this language's test tool tests (including #770's `testing-on` and #810's H4 test) run on the real kernel; the dependencies and tests of the template `crates/rutis-host/templates/<language>/`; the test tool section of this language's guide and the README; Kotlin `build.gradle.kts`, `api/*.api`, `MainThreadTest.kt`; the last PR merged changes guarantees `sdk/testing-matches-runtime`, `sdk/config-schema`, `sdk/undeclared-use`, `events/presence`, `events/hub-filter`, `events/one-bus`, `specs/config-schema/README.md`, and §2.1 of the config Schema design | this language's template project `test` passes within the times in 7.5; the benchmark is within the acceptable range in 7.5 (if not, stop and report) | CL8, CL27, CL29-bench, this language's CL26; Kotlin and Swift also after A1, A2 |

### 13.3 Order

```text
Can start now, independent of each other: CL1 → CL2 → CL3; CL4; CL5 → CL15 (also needs CL10); CL7; CL8; CL10; CL13; CL17; CL23; CL24; CL28-*; CL29-bench
After #811 S2b: CL11a → CL26-* (each language, also needs that language's R-x; Kotlin and Swift also need A1, A2) → CL11b
After #811 S3: CL19 → CL20
After #811 S4: CL27 → CL30-* (also needs CL8, CL29-bench, that language's CL26)
After #811 S5: CL6; CL18; CL12a → CL12b → CL22
After #805 is merged: CL25 (if CL19 comes after #805, CL19 changes #805's call)
CL13 before #811 S4; CL17 before #811 S5; CL21 after CL13
```

Once everything is merged, the docs in §12 have been changed along with each PR; before release, run the developer-experience check per #690's release gate; the "Platform cleanup" section of "Migrating to 0.19" is written item by item by each PR, and the last PR checks that all items are present.

