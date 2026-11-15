# Row lifecycle: generations, two stages, services and versions, leases, crashes, several runtimes (TLA+)

[中文](README.md)

TLC exhaustively checks a runtime session's rows from resolution and load to cleanup, and their interleavings with a break, a reconnect of the same id, an old generation's unload beside a new one's load, service announcements (with versions), name clashes and the scope of a refusal, host service leases and `hosts.provide`/`hosts.withdraw`, a runtime's crash and restart, two runtimes cold-starting, a refresh that a restart interrupts and that is redone, a volatile `rows.update` committed in place, and a row that ends while it loads. Issue [#613](https://github.com/arcships/rutis/issues/613), its row-lifecycle part (with [#625](https://github.com/arcships/rutis/issues/625) merged in); quality standard Q11.2; design: [remote sessions](../../docs/design-remote-session-2026-10-28.md) §4.2, §4.3, [remote plugins](../../docs/design-remote-plugins-2026-10-03.md) §4.4, [multilanguage runtimes](../../docs/design-multilang-runtimes-2026-10-03.md) §四, §五, §六, [session trace contract](../../docs/formal-trace.md) §3.0.

One model, two parts, switched by constants: the original 22 configurations (9 ok and 13 mutations) turn only the first part on, with the same semantics as before (the three exhaustive ones have identical distinct-state counts: 73,768 / 191,830 / 110; a violated configuration's count depends on the search order anyway); the new configurations turn on two stages, services and versions, the scope of a refusal, dependencies, leases (with the `hosts.provide`/`withdraw` counts), crashes, a second runtime, the refresh task, `rows.update` and a row ending during its load one by one. Event registration and the frame order of `rows.ended` (#613's event table), the call lifecycle and rows hosted by a node are not modelled.

## The design and code it corresponds to

| Model part | Design | Implementation |
| --- | --- | --- |
| The host has one session per runtime and redials only after losing it; rows load through the session and wait for the next one when it ends | remote sessions §4.2; remote plugins §4.4 | `crates/rutis-host/src/connect/dial.rs` `Dialing::run` (redials after `installed.unloaded().await`); `crates/rutis-loader/src/runtime/sessions.rs` `SessionRowsPlugin` |
| The host keys a load by its generation: every load gets a new key, and a late reply of an earlier generation does not act on a newer one | quality standard §11.2; #625 `OwnGenerationOnly` | `crates/rutis-loader/src/runtime.rs:719-727` (`ctx.instance()` + `GENERATION`); `on_row_ended` / `on_row_failed` in `crates/rutis-bridge/src/runtime/process.rs` |
| `rows.ended`: the row ends by itself; the host ends it and unloads it (it is not loaded again) | remote sessions §4.3; `loader/self-dispose` | `crates/rutis-bridge/src/runtime/process.rs:531-541`; `node/rutis-runtime/src/runner.mjs:379-385` (`endRow`) |
| A row that ends while it loads reports its ending only after the load succeeded (54a7e824) | remote sessions §4.3 | `on_row_ended` in `crates/rutis-bridge/src/runtime/process.rs` (registered before the load, sent after its reply); `endRow` in `node/rutis-runtime/src/runner.mjs` waits for apply |
| `rows.failed`: the row's code failed; the host restarts it, the runtime keeps the row loaded | remote sessions §4.3; #688 | `crates/rutis-bridge/src/runtime/process.rs:543-568`; `node/rutis-runtime/src/runner.mjs:1120-1135` |
| `rows.unload` is idempotent: a row already gone is only answered | the `rows.v2` row contract | `crates/rutis-bridge/src/session/plugin/rows.rs:197-207`; `node/rutis-runtime/src/runner.mjs:743-746` |
| Replacement: close the old session, stop reading its frames, clean all its rows, then answer the new hello | remote plugins §4.4; remote sessions §4.2 | the runtimes' listener side (one order in Python `serve`, Go `go/rutis/internal/runtime/listen.go`, Node `node/rutis-runtime/src/serve.mjs`) |
| A session ending (a break or a replacement) cleans all its rows, each once | remote plugins §4.4; `loader/remote-runtime-rows`, `loader/row-unload-leaves-nothing` | the runtimes' `serve`/`listen`; `Server` in `crates/rutis-bridge/src/session/plugin/rows.rs` |
| Two-stage release: the process starts and provides `<language>Runtime`; the host re-resolves its rows with it; then it provides `<language>RuntimeRows` and the rows wait and start with their full dependencies | multilanguage runtimes §四, §六; M1 §6.2; #625 `NoProvisional` | the two stages at the top of `crates/rutis-loader/src/runtime/sessions.rs`; each runtime plugin provides both services; the refresh uses the loader's `entries()` + `reload(id)` |
| The refresh is a task of its own: a runtime restart cancels it, and it is redone after the restart (REM-4.3-3) | multilanguage runtimes §六; #625 | the refresh task in `crates/rutis-loader/src/runtime/sessions.rs` (it does not wait inside the reconcile under way) |
| Announcements carry versions: an older announcement never overrides a newer one; a row may provide a name again (a service update); a row's cleanup withdraws its names before the unload; the host publishes an announcement as a local service in a step of its own, and the cleanup waits for it (#372) | multilanguage runtimes §五; M1 §5.2, §5.3, §14; #625 `NewerWins`, `WithdrawBeforeUnload` | `Imports::update` in `crates/rutis-bridge/src/runtime/process.rs` (drops `version <= entry.1`), `announce`; `publish` / `withdraw` in `crates/rutis-bridge/src/runtime/projection.rs` (`withdraw` waits for the publication under way) |
| A name clash: a second live provider is refused; the refusal fails only the restart it caused, and a restart that waits for a dependency instead is not failed (d94aae9) | multilanguage runtimes §五; M1 §5.2; #625 `RefusalScope` | `Refusals` in `crates/rutis-loader/src/runtime.rs` (per fiber instance: `arm` / `take` / `forget`) |
| Host service leases and the `hosts.provide`/`hosts.withdraw` counts: a row leases the host services it injects while it loads and releases them after `rows.unload`; the first lease provides, the last release withdraws; the runtime holds the service's shape | multilanguage runtimes §五; M1 §5.4; #625 `LeaseWithinRow`, `HostLeases` | `crates/rutis-loader/src/runtime.rs:623-665` (`lease_host_for`, `release` in the cleanup), `:1027-1031`; `Process::lease` / `release_host` and `host_changes` in `crates/rutis-bridge/src/runtime/process.rs` |
| A process crash withdraws both services and does not restart on its own; the application decides. A restart re-resolves the rows | multilanguage runtimes §四 | each runtime plugin; `wait_for_the_runtime` in `crates/rutis-loader/src/runtime.rs` |
| Two runtimes do not wait for each other: one runtime's start, session or rows do not need another | multilanguage runtimes §四; `loader/runtimes-do-not-wait-for-each-other`, SYN-2-14, SYN-4-4 | runtime plugins depend only on their own start; gating lives on the consuming rows (`gated` / `row_inject` in `crates/rutis-loader/src/runtime.rs`) |
| A volatile-only change commits in place with `rows.update`, without an unload or a new generation (SYN-4-8) | multilanguage runtimes §四; the approach of #101 | `Forward::apply` (`update_row`) in `crates/rutis-loader/src/runtime.rs`; `Imports` → `rows.update` in `crates/rutis-bridge/src/runtime/process.rs` |

## The model

- **Participants**: one host (loader rows, its dials, its service table and versions, its host service leases, its publication steps) and 1–2 runtime processes (the listener, the row table, the two release stages, one refresh task). The host has one connection per runtime and redials after losing it; a runtime serves one session at a time and a new connection replaces the old one. The configuration's `RowSet2` says which rows belong to runtime `r2`; the others belong to `r1`.
- **Connections**: `1..MaxConns` per runtime; each direction is FIFO and delayed arbitrarily long; a side that closed a connection reads nothing more on it and frames sent to it are dropped.
- **Two stages and the refresh**: `StartRuntime(rt)` takes the process from `down` to `up` (it provides `<language>Runtime`); the refresh has two shapes: `Refresh(rt)` in one step (the original configurations), or `RefreshBegin(rt)` / `RefreshDone(rt)`, which makes it a task of its own inside a `refreshing` stage (`Refreshes`). A crash cancels a refresh under way (`refresh = "cancelled"`), and after another start `RefreshBegin` runs it again (the `noRefreshRedo` mutation does not). `HLoad(r)` requires its runtime's stage to be `rows` and the row's resolution to be `final`.
- **Host** (`HDial`, `HGreeted`, `HLoad`, `HLoaded`, `HUnloaded`, `HLoadFailed`, `HAnnounce`, `HProvide`, `HPublication`, `HPublicationDone`, `HEnded`, `HFailed`, `HLoadRefused`, `HWaitRefusal`, `HUpdate`, `HUpdated`, `HLose`):
  - `HLoad(r)` sends `rows.load` with the key `<<runtime, session, row, generation>>` and records the host services this generation leases; a row loads at most `MaxLoads` times in a session (a restart uses one). With `HostFrames`, each name's first lease provides it and its last release withdraws it, and the runtime records the shape.
  - `HLoaded` handles the load reply; when this generation's announcement was refused, the row does not become Running but starts unloading. With `Versions` it also requires the names this generation announced to be published (or the publication refused), so "the row is Running" agrees with "its service is published or was refused". `HUnloaded` handles the unload reply: it clears this generation's announcements (by version, see below), releases its leases, and the row goes `idle` (the `leaseLeak` mutation skips the release). `HLoadFailed` handles `loadfailed`: release the lease, back to `idle`.
  - `HAnnounce` (without versions) applies one service announcement: a name another live key holds is refused (the later row fails, the earlier one is unaffected); announcements are processed before the row becomes Running (the `announceAfterLoad` mutation sends the `loaded` reply first, to check `ServicesBeforeRunning`).
  - `HProvide` (`Versions`) writes a versioned announcement into the host's table: one that is not newer than the table's is dropped (the `regardlessOfVersion` mutation applies it anyway, so a late old announcement overrides a newer one and `NewerWins` fails). `HPublication` / `HPublicationDone` are the publication step: they register the names this generation announced that are newer than what was published, and refuse this row when another running row holds a name. `HUnloaded` / `HLoadFailed` withdraw the row's names at "the next version" when it unloads — newer than any announcement in flight, so a late `provide` does not bring them back; the `loseWithdrawal` mutation drops that withdrawal while a publication is still under way (#372), so the row's names stay on the host after it unloaded.
  - `HLoadRefused` fails the one restart the refusal caused, then clears the record (the `refusalSticky` mutation keeps it). With `PreciseRefusal` the refusal is recorded on the row (its fiber): `HLoadRefused` fails only the apply of the first restart after the clash, and `HWaitRefusal` says that restart waits for a dependency instead of applying, so the refusal is forgotten and fails no later generation.
  - `HEnded` ends only the key that is the row's current generation and sends `rows.unload` as the real unload stack does; `HFailed` marks the current generation unloading and waits for the reply. `HLose` breaks the current connection: its rows go `idle`, and its announcements, leases and rows elsewhere that lose a dependency are handled with it.
  - `HUpdate` (`Updates`) sends a volatile-only change to the runtime (same generation, same key) and `HUpdated` takes the reply; the `updateReload` mutation unloads and loads the row instead.
  - A dependency going away: when a provider unloads, the host clears its names in the same action and marks the rows that are `loading`/`loaded` and now miss a dependency as unloading (`Starved`); the row then waits `idle` for the name to come back (the `keepOnDepGone` mutation does not).
- **Runtime** (`TakeUp`, `Answer`, `EndSession`, `RLoad`, `RLoadFinish`, `RProvide`, `RFailLoad`, `RUnload`, `EndSelf`, `EndDuringLoad`, `SendEnding`, `RFail`, `RUpdate`, `Sweep`, `MarkSwept`):
  - `TakeUp(s)` reads a new connection's `hello`, closes the old session (no more reads) and starts cleaning its rows. `Answer(s)` answers the new `hello` only once every earlier session of this runtime is swept; nothing of another runtime is required (the `mutualWait` mutation requires the others to be fully released and swept).
  - Without versions, `RLoad` puts one `announce` frame per name the row provides into the same session's reply queue before the `loaded` reply (the `announceAfterLoad` mutation reverses that). With `Versions`, the first `provide` is part of the load (before the reply, `RProvide`), `RLoad` only answers `loaded`, and a row that runs may provide a name once more (a service update, `MaxProvides` bounds it). `RLoad` / `RLoadFinish` (`LoadSteps`) split "the load was read" and "the reply".
  - `RUnload` is idempotent (a loaded row is cleaned once, one already gone is only answered); `EndSelf` is the plugin ending itself (the row disappears, its `rows.ended` is still to be sent); `EndDuringLoad` (`LoadSteps`) is a plugin ending itself while apply runs: the load still succeeds, and `SendEnding` waits for the `loaded` reply (the `endedDuringLoad` mutation sends it at once and fails `EndingAfterLoad`); `RFail(k)` sends `rows.failed` at most once per generation.
  - `RUpdate` (`Updates`) commits `rows.update` on the same generation and answers `updated`.
- **Crash**: `HCrash(rt)` cleans every row loaded on that runtime (each cleanup count grows once), drops its leases and service announcements, closes all its sessions, takes the stage back to `down`, leaves its rows `idle` and their resolution `provisional`, and marks a refresh under way `cancelled`; `StartRuntime` is limited by `Restarts` and the host's redial by `MaxConns`. One runtime's crash touches only its own state; a configuration checks that another runtime's rows still reach Running.
- **The replacement order**: `TakeUp` closes the old, stops reading, cleans the rows (`Swept`), `Answer` answers the new `hello`. `Answer`'s precondition is still the rule this model checks.

## Simplifications

- The scale is deliberately finite: the old configurations have 1 runtime, 1–2 rows and at most 3 connections; the new ones have at most 2 runtimes, 2 rows per runtime, at most 2 generations of a row per session, 1 crash, at most 2 starts, 2 dials, at most 2 `provide`s per generation and at most 2 `rows.update`s. Repeated crashes, backoff and the 30 s / 60 s timers are not modelled: the environment is only "a break or a crash at some time", which is more general than real timings.
- The handshake is one `hello` each way: version, capabilities, authentication and the 10 s deadline are not modelled (`remote-lease` covers the rest of the handshake and the replacement).
- The only host→runtime frames are `rows.load`, `rows.unload` and `rows.update`; `rows.schema`/`mount`, events and call chains are not modelled. The runtime→host frames are `hello`, `announce`, `provide`, `loaded`, `loadfailed`, `unloaded`, `ended`, `failed` and `updated`.
- At most two service names (`ServiceName` and `GateName`): one name rows fight over and one a refused row may wait for; there is one host service name. Several names and several methods per service are not modelled: the shape travels with `hosts.provide` and with an announcement, and the model checks that a registered name has a shape and that it is the one the first `provide` carried; what a sync or async shape does to a call lives outside (the call lifecycle is #613 and #614).
- A service version is "one increasing counter per name": every `provide` (a service update included) and every withdrawal takes a version, and the host applies only what is newer. The implementation's version is the last one of `Imports.slots[id]`; the comparison rule is the same.
- A row withdrawing a single name while it runs (`ctx.unprovide`) is not modelled on its own: a withdrawal happens only in a row's cleanup (`Dropped` in `HUnloaded` / `HLoadFailed`) with the same version rule; a withdrawn name does not stop consuming rows either (a dependency going away is modelled for a provider's unload, a break and a crash, in `row-deps`).
- With versions a name clash happens at the publication step (a name's local key held by another running row), as `provide_mut_at` does; without versions it happens at the announcement step (the original configurations).
- A load failure (`loadfailed`) only means the runtime failed to load the row; a running row's code failing is `rows.failed`.
- A dependency appearing is only `HLoad`'s precondition; a dependency going away is modelled as the host marking the consuming row unloading in the same action that removes the name (provider unload, broken session or crash). A failed refresh failing only its row (M1-6.2-4) is not modelled.
- A crash is one atomic action (process death, session end and row cleanup together); the per-language unload stacks, `shutdown` deadlines and Node/Bun's forced-kill time are not modelled (the rest of #625).
- A host row is one fiber: `idle | loading | loaded | unloading | gone`, keyed `<<runtime, session, row, generation>>`; the loader's full resolver, config layering and restart timing are not modelled. "A row ending itself disables it, never to load again" follows `loader/self-dispose`.
- Unread frames of an old session stay in the model (the design drops them when the connection closes); `CanRead` never reads a non-current session, and only the `readOld` mutation does.
- The host stops reading a session once it loses it, so a late `rows.ended` of an old session never reaches the host after the new session starts; the host-side rule shows up as the generation race within one session (`OwnFramesOnly`), the defect a88fb65b fixed.
- Memory order, task scheduling, real timers and the frame encoding are not modelled; every action is atomic.
- **Not covered** (still without a formal argument in #625): the frame order of `rows.ended` with `events.*` (#613's event table), the call lifecycle and cross-process synchronous call chains, rows hosted by a node, the per-language apply cancellation and unload-stack differences, the shape negotiation of `rows.schema`/`mount`, the shape refresh when a second lease of one name does not `hosts.provide` again (one name has one service object, so the model's shape never changes), `runtimes_ready` (#314), a row with an `inject` whose apply fails (#584), the specific shapes of #247 and #191, and real timers.

## Invariants and liveness

| Name | Meaning |
| --- | --- |
| `OneRowPerName` | At most one generation of a row name is loaded at the runtime: a new session loads only after the old session's rows are cleaned; a restart waits for the old generation's unload reply |
| `NewSessionOnlyAfterSweep` | When a session's `hello` is answered, every earlier session of this runtime has its rows cleaned |
| `NoRowsAfterSweep` | Once a session was swept, no frame of it loads a row again: the runtime does not read the old session's commands |
| `OwnFramesOnly` | The host acts on a `rows.ended`/`rows.failed` only for the row's current generation |
| `CleanupOnce` | A row generation is cleaned at most once: the session sweep, `rows.unload`, the row ending itself and a crash never dispose the same row twice |
| `NoProvisional` | A row loads and runs only after its runtime finished both stages and its own resolution is final: no row runs on a provisional resolution |
| `ServicesBeforeRunning` | When the host marks a row Running, every announcement the row made while loading has been processed: the announcements arrive in the load's synchronous call chain (before the `loaded` reply in frame order) |
| `OneProviderPerName` | Two rows never run with one name: the later announcement is refused and its load fails (with versions, a provider counts once its name is published) |
| `RefusalOnce` | A refusal fails at most one restart: it is not carried to later generations |
| `RefusalScope` | With `PreciseRefusal`: one refusal neither makes a restart wait for a dependency nor fails its apply — the restart that waited does not count, the refusal is forgotten (d94aae9) |
| `NewerWins` | With `Versions`: the host's table always holds the newest version it applied; a late older announcement never overrides a newer one |
| `WithdrawBeforeUnload` | With `Versions`: once a row is unloaded, its names are no longer on the host (the cleanup withdraws before the unload, and a withdrawal in the middle of a publication is not lost) |
| `HostLeases` | With `HostFrames`: `hosts.provide` and `hosts.withdraw` balance — the runtime holds a name exactly while a generation leases it, one provide is live per registered name, and the runtime has a shape for it |
| `EndingAfterLoad` | With `LoadSteps`: a row that ends while it loads reports its ending only after the load's reply (54a7e824) |
| `UpdateInPlace` | With `Updates`: a volatile-only update commits on the generation the row runs: no unload and no new generation (SYN-4-8) |
| `LeaseWithinRow` | A lease belongs to the row's current generation, is released after the unload reply and before the next generation loads, and a failed load leaves none |
| `LoadedNeedsDeps` | A Running row still has every name it injects: when a name goes away the host synchronously marks the row unloading, so no row runs without its dependencies |
| `EndedLeadsToSwept` | Liveness: a session that ended (a break, a replacement or a crash) ends with its rows cleaned |
| `RestartRecovers` | Liveness: after a crash, while the runtime may start and the host may dial, its rows reach Running again |
| `RuntimesDoNotWait` | Liveness: when one runtime crashes (and is not started again), another runtime's rows still reach Running |
| `ColdStartLoads` | Liveness: once a runtime finished both stages, its resolution is final and its session is ready, its rows reach Running; two runtimes that cold-start together each finish (SYN-2-14) |
| `RefreshRedone` | Liveness: a refresh a runtime restart interrupted is run again (`stage = rows`) (REM-4.3-3) |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 21.0.12 (Temurin), 32 cores, `-workers auto` (8 below). "ok" means the exhaustive search completed and every listed property held; a violated configuration stops at the first counterexample, and its state count depends on the search order. The whole directory takes about 30 seconds.

| Configuration | Content | Result | Distinct states | Time |
| --- | --- | --- | --- | --- |
| `row-healthy` | 2 rows, 1 connection; rows may fail and end themselves; the original 5 safety invariants | ok | 73,768 | 1–2 s |
| `row-reconnect` | 2 rows, 3 connections (2 reconnects), a break may happen | ok | 191,830 | 2–5 s |
| `row-cleanup` | 2 rows, 1 connection, a break may happen; liveness only | ok | 110 | 1 s |
| `row-generations` | 2 rows, 1 connection, two stages, rows may fail (two generations each); `NoProvisional` and others | ok | 419 | 1–2 s |
| `row-services` | both rows provide `s`, two stages, rows may fail; announcements, refusals, one failed restart per refusal | ok | 2,643 | 2 s |
| `row-versions` | both rows provide `s`, announcements carry versions, each generation may provide again (a service update); `NewerWins`, `WithdrawBeforeUnload` | ok | 64,953 | 2–3 s |
| `row-lease` | both rows lease host service `h`; rows and loads may fail; leases | ok | 1,334 | 2 s |
| `row-host-frames` | both rows lease `h`; the `hosts.provide`/`withdraw` frames and counts, shapes, a failed load gives its lease back | ok | 3,156 | 1–2 s |
| `row-deps` | `a` provides `s`, `b` injects it; `a` may fail and restart; the dependency goes away | ok | 422 | 2 s |
| `row-restart` | one runtime crashes once and is started again; the host dials twice; old rows cleaned, new ones load | ok | 1,232 | 1 s |
| `row-runtimes` | two runtimes with two rows each; `r1` crashes and is not started again, `r2` goes on; no mutual wait | ok | 1,988 | 1 s |
| `row-coldstart` | two runtimes, two rows each, cold-starting together; both cold starts finish and the rows reach Running | ok | 784 | 1 s |
| `row-refresh` | one row; the refresh is a task of its own, a crash cancels it and it is run again (liveness) | ok | 358 | 1 s |
| `row-updates` | one row; two `rows.update`s, both committed on the same generation | ok | 23 | 1 s |
| `row-ended-during-load` | one row; the plugin ends itself while apply runs, the ending is reported after the load's reply | ok | 39 | 1 s |
| `row-refusal-scope` | two rows fight over `s`, one also injects `d`; the refused restart waits for `d`, the refusal is forgotten | ok | 628 | 1 s |
| `row-mut-accept-before-clean` | the replacement answers the new `hello` before the old session's rows are cleaned | violated `NewSessionOnlyAfterSweep` | 158 | 2 s |
| `row-mut-accept-before-clean-loads` | the same, then the new session loads a row of the same name | violated `OneRowPerName` | 683 | 1 s |
| `row-mut-read-old` | the runtime keeps reading the old session after the replacement | violated `NoRowsAfterSweep` | 278 | 1 s |
| `row-mut-keyed-by-row` | the host matches `rows.ended`/`rows.failed` by row name, not by key | violated `OwnFramesOnly` | 119 | 1 s |
| `row-mut-reclean` | an unload of a row already gone runs the cleanup again | violated `CleanupOnce` | 53 | 1 s |
| `row-mut-keep-rows` | the runtime closes a lost session but keeps its rows | violated liveness | 29 | 0 s |
| `row-mut-load-provisional` | a row loads on a provisional resolution, before the runtime releases its rows | violated `NoProvisional` | 81 | 1 s |
| `row-mut-announce-after-load` | the runtime sends the `loaded` reply before the service announcements | violated `ServicesBeforeRunning` | 23 | 1 s |
| `row-mut-allow-clash` | the host accepts a second announcement of a name | violated `OneProviderPerName` | 117 | 1 s |
| `row-mut-newer-wins` | the host applies every announcement it reads, without comparing versions | violated `NewerWins` | 16,981 | 1 s |
| `row-mut-projection-close` | the projection closes mid-publish and drops the withdrawal (#372) | violated `WithdrawBeforeUnload` | 21,607 | 1 s |
| `row-mut-lease-leak` | an unload or failed load does not release the host service lease | violated `LeaseWithinRow` | 94 | 1 s |
| `row-mut-lease-orphan` | a lease is dropped while it awaits `hosts.provide`: the count stays, no owner, the service is never withdrawn | violated `HostLeases` | 75 | 1 s |
| `row-mut-keep-on-dep-gone` | the consuming row keeps running after its name goes away | violated `LoadedNeedsDeps` | 211 | 1 s |
| `row-mut-refusal-sticky` | a refusal stays and fails later restarts | violated `RefusalOnce` | 278 | 2 s |
| `row-mut-refusal-scope` | the refusal stays after a restart that waited on a dependency and fails a later apply (d94aae9 before) | violated `RefusalScope` | 465 | 1 s |
| `row-mut-ended-during-load` | a row that ends while it loads reports the ending at once (54a7e824 before) | violated `EndingAfterLoad` | 40 | 0 s |
| `row-mut-update-reload` | a volatile update unloads and loads the row instead of committing in place | violated `UpdateInPlace` | 15 | 1 s |
| `row-mut-no-refresh-redo` | a cancelled refresh is not run again | violated liveness (`RefreshRedone`) | 358 | 1 s |
| `row-mut-mutual-wait` | one runtime's answer waits for another to be fully released and swept | violated liveness (`RuntimesDoNotWait`, `ColdStartLoads`) | 84 | 1 s |

## Mutations and counterexamples

Every checked property has a deliberately introduced error, and TLC must find a counterexample (`violated:<name>` in `expected.txt`; `violated:liveness` for liveness):

| Configuration | Mutation | Violated | Counterexample |
| --- | --- | --- | --- |
| `row-mut-accept-before-clean` | `Answer` does not require the earlier sessions to be `Swept` | `NewSessionOnlyAfterSweep` | session 1 loaded a row, broke, the host dials session 2; `TakeUp(2)` closes session 1 and `Answer(2)` answers at once while `Swept(1)` is still false |
| `row-mut-accept-before-clean-loads` | the same | `OneRowPerName` | session 2 then loads row `a` while session 1's `a` still runs |
| `row-mut-read-old` | `CanRead` allows reading an ended session's frames | `NoRowsAfterSweep` | an unread `rows.load` of session 1 starts a row after the session was swept |
| `row-mut-keyed-by-row` | `HEnded`/`HFailed` match by row name, ignoring the key's generation (a88fb65b before) | `OwnFramesOnly` | the plugin sends `rows.ended` after the unload; the host has loaded a new generation and ends it on the old ending |
| `row-mut-reclean` | a row already gone still runs its cleanup | `CleanupOnce` | the row fails; the plugin ends itself (one cleanup), and the following `rows.unload` finds it gone but the mutation cleans it again: `cleanCount = 2` |
| `row-mut-keep-rows` | `Sweep` is disabled (a break keeps its rows) | liveness (`EndedLeadsToSwept`) | session 1 loaded a row and broke; the runtime never cleans it |
| `row-mut-load-provisional` | `HLoad` requires neither `stage = rows` nor a final resolution (6f8fb5c7 before) | `NoProvisional` | while the stage is `down` and the row is `provisional`, the mutation loads it; `NoProvisional` is false at once |
| `row-mut-announce-after-load` | `RLoad` sends the `loaded` reply before the `announce` frames (2f830f81 before) | `ServicesBeforeRunning` | `sentAnn` already holds `s` while `processedAnn` is empty, and `HLoaded` marks the row `loaded` |
| `row-mut-allow-clash` | an announcement is applied even when another live key holds the name | `OneProviderPerName` | `a` and `b` both provide `s`; both announcements apply, both loads reply `loaded`, and both rows are Running |
| `row-mut-newer-wins` | the host applies every announcement it reads, without comparing versions | `NewerWins` | the row's cleanup withdrew the name at version 3; the `provide` of version 1 still in the channel is applied: the table holds the old version while `ever` is 3 |
| `row-mut-projection-close` | a cleanup that starts while a publication is under way does not wait and its withdrawal is dropped (#372 before) | `WithdrawBeforeUnload` | the row loads, provides again (version 2), the publication starts; the row fails and unloads, the cleanup finishes mid-publication and the name is still this generation's: `hrow = idle` with `claimed` = this generation's key |
| `row-mut-lease-leak` | `HUnloaded`/`HLoadFailed` return the row to `idle` without releasing the lease | `LeaseWithinRow` | a row fails and unloads (or its load fails); it is `idle` while `leaseHeld` is still non-empty |
| `row-mut-lease-orphan` | a restart drops the lease while it awaits `hosts.provide` | `HostLeases` | loading the row raises the count in `aux.hosts` while no generation holds it: `count = 1` with no holder |
| `row-mut-keep-on-dep-gone` | the consuming row is not stopped when the name goes away | `LoadedNeedsDeps` | `a` fails and unloads, `s` leaves the table; `b` is still `loaded` with its dependency gone |
| `row-mut-refusal-sticky` | the refusal is not cleared per generation | `RefusalOnce` | `b` clashes and is refused; the first retry fails (count 1), the record stays, the second retry fails too (count 2) |
| `row-mut-refusal-scope` | the refusal stays after a restart that waited on a dependency | `RefusalScope` | `b` clashes, its restart waits for `d` (`HWaitRefusal`, `waited = 1`), and once `d` is back the apply fails for the same refusal (`failed = 1`) |
| `row-mut-ended-during-load` | a row that ends while it loads reports the ending at once | `EndingAfterLoad` | `EndDuringLoad` puts the key into `pendingEnding` and `SendEnding` does not wait for the `loaded` reply: `endedSent` holds it while `replied` does not |
| `row-mut-update-reload` | a volatile update unloads and loads the row | `UpdateInPlace` | the update starts (`updated` is 1) and the row goes `unloading`: it is no longer `loaded` |
| `row-mut-no-refresh-redo` | `RefreshBegin` refuses a cancelled refresh | liveness (`RefreshRedone`) | the runtime crashes while a refresh runs (`refresh = cancelled`); after the restart the refresh never starts, `stage` stays `up`, the rows never run |
| `row-mut-mutual-wait` | `Answer` also requires the other runtimes at `stage = rows` with all sessions swept | liveness (`RuntimesDoNotWait`, `ColdStartLoads`) | `r1` crashes and is not started again; `r2`'s answer waits for `r1` for ever, so `r2`'s rows never run |

`row-mut-accept-before-clean` and `row-mut-read-old` would also fail `OneRowPerName` or `NewSessionOnlyAfterSweep`; the configurations are split so each counterexample stands for one conclusion (as in `event-close`).

## Conclusion

- The original conclusion is unchanged: under remote plugins §4.4's two rules — stop reading the old session when replacing, answer the new `hello` only after its rows are cleaned — the runtime never has two generations of one row name, the old session's leftover frames never start a row after the sweep, a break's rows are cleaned, and the host acts by key so a row is cleaned once.
- The service part holds in the exhausted range: a row loads only after both stages and a final resolution; announcements made during a load are processed before the `loaded` reply, so the host never marks a row Running while it still owes announcements; two rows never run with one name, and the second is refused; a refusal fails only the restart it caused, and a restart that waited on a dependency does not count (d94aae9); a host service lease belongs to the current generation, is released after the unload or failed-load reply, and the `hosts.provide`/`withdraw` counts agree with what is registered; a crash cleans the runtime's old rows once and they load again after a restart; a consuming row stops as its service goes; one runtime's crash does not stop another runtime's rows; and two runtimes that cold-start together each finish.
- Versions and the later actions hold in the exhausted range: the host's table always holds the newest version it applied, and a late old announcement does not override a newer one (`NewerWins`); once a row is unloaded its names are gone from the host, and a withdrawal in the middle of a publication is not lost (`WithdrawBeforeUnload`, that is #372); a row that ends while it loads reports its ending after the load's reply (54a7e824); a refresh interrupted by a runtime restart is run again (REM-4.3-3); and `rows.update` commits on the generation that runs (SYN-4-8).
- Still open: the last bullet of "Simplifications" — the frame order of `rows.ended` with events (#613), the call lifecycle and cross-process synchronous call chains, rows hosted by a node, the per-language apply cancellation and unload-stack differences, the shape negotiation of `rows.schema`/`mount`, the shape refresh when a second lease of one name does not `hosts.provide` again, `runtimes_ready` (#314), a row with an `inject` whose apply fails (#584), the specific shapes of #247 and #191, and real timers. This directory is not evidence for the whole row protocol.
- "Announcements arrive in the load's synchronous call chain" is modelled as frame order on one session (an announcement before `loaded`), not as a real cross-process call chain; real synchronous ordering lives in `cross-runtime-sync` and later models.

## How to run

Java 11 or later and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`) are needed. Do not commit the jar.

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh                    # every configuration
TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh row-runtimes       # one configuration
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh      # limit TLC's threads
```

`run.sh` compares each configuration's outcome with `expected.txt` and exits 0 when all match; logs and counterexamples go to `TLC_OUT` (a temporary directory by default). CI's `specs` job matrix includes `row-lifecycle` (see [CI](../../docs/ci.en.md) §4).

## Maintenance

When remote sessions §4.2, §4.3, remote plugins §4.4 or multilanguage runtimes §四, §五, §六 change, or when the row key, `Refusals`, leases or projection in `crates/rutis-loader/src/runtime.rs`, `rows.ended`/`rows.failed`/`unload_row` or the service table and `Imports::update`'s version rule in `crates/rutis-bridge/src/runtime/process.rs`, `publish` / `withdraw` in `crates/rutis-bridge/src/runtime/projection.rs`, the runtimes' listener replacement order and two stages, or `endRow`/`unloadRow`/`update` in `node/rutis-runtime/src/runner.mjs` change, update this model and `expected.txt` (standard Q11.2.3). A new feature is added behind a constant (off by default) so that the original configurations keep their semantics and state counts.
