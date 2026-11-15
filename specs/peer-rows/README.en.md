# Node-hosted rows: each session's table, the instance key, `link.offers`, hosting operations and two nodes hosting each other (TLA+)

[中文](README.md)

TLC exhausts the interleavings of a node running rows for another node: the old control session breaking, a new one taking over, each session's own table of what it loaded, a row's instance key that outlives a restart, the late `rows.ended`/`rows.failed` notices, and the import and withdrawal of a row's service; plus the rest of #640: `link.offers` epochs and withdrawal, the `PeerRows` gate, the far end being offline, `plugins.describe`/`schema`/`load`/`update`/`unload`, a hosted row cancelled while loading, two nodes hosting each other and starting cold together, and service versions and tombstones. It corresponds to the `specs/peer-rows/` item of the "merged into #640" section of issue [#613](https://github.com/arcships/rutis/issues/613); quality standard Q11.2; the design is [remote plugins](../../docs/design-remote-plugins-2026-10-03.en.md) §4.4, §5.1–§5.3, [remote sessions](../../docs/design-remote-session-2026-10-28.en.md) §4.2, §4.3, and the [session trace contract](../../docs/formal-trace.en.md) §3.0.

This is the second block of the row lifecycle: [`row-lifecycle`](../row-lifecycle/README.en.md) models rows on the runtime's side (loading, unloading and reconnecting within a runtime session); this block models hosting rows for a peer. The directory is split by question into five modules; `PeerRows.tla` is the original session/generation model, the other four add #640's gate, host operations, two nodes hosting each other, and service versions.

`peer:` rows and `PeerRow` are gone from the current code: the 0.19 remote-connection design removes them (remote sessions §8), and the equivalent path is `<connection row>:<plugin>` through `run`; a run row's key is new on every load (`plugin#n`, `run.rs:168`), so two generations never share a key. `PeerRows.tla` writes the old rule of issue #613's doubt — the key is `ctx.instance()`, the same across restarts — exactly to answer what safety rests on when there is no new key per load. That difference is the decision input for #1100, see "The key: the model and today's code" below; no code is changed here.

## The modules

| Module | Content | Participants and scale | Configurations |
| --- | --- | --- | --- |
| `PeerRows.tla` | Each control session's own table, the instance key and generations, late `rows.ended`/`rows.failed`, a row's service following it, cleaning the old session before answering a replacement | 1 host + 1 peer; 2 rows, 2 sessions, 2 generations per row | 7 (`peer-*`) |
| `PeerOffers.tla` | `link.offers` versions, `since`/epochs, older announcements ignored; registering, withdrawing and re-registering `plugins`; the `PeerRows` gate, schema refresh, waiting while offline, one start per row | 1 peer + 1 loader side; 1 row, 2 registrations, the link may break | 7 (`offers-*`) |
| `PeerHosted.tla` | The host's `describe`/`schema`, `load`, `update`, `unload`; a load cancelled in flight and an unload that arrives before its load; `NoOrphan` | 1 peer + 1 host; 1 key, a catalog of 1 plugin | 6 (`hosted-*`) |
| `PeerPair.tla` | Two nodes hosting for each other, starting cold together, breaking and reconnecting; answering `hello` without waiting for one's own row; the old session's rows cleaned before the next answer | 2 nodes, 1 connection; 1 row each | 5 (`pair-*`) |
| `PeerServices.tla` | Service announcement/withdrawal versions, the importer keeping the withdrawn version (a tombstone), an older announcement not restoring the service | 1 provider + 1 importer, 1 service | 3 (`svc-*`) |

## The design and the code it stands for

| Model part | Design | Implementation |
| --- | --- | --- |
| Each control session has its own table of what it loaded; one key is loaded at most once in a session | Remote plugins §4.4 (a runtime instance has one active session) | `crates/rutis-bridge/src/session/plugin/rows.rs:61` (`Server.loaded`), `:110-112` (a key already loaded is refused) |
| A row is loaded by its instance key, which is the same after a restart; the generation is the model's name for one load | Remote plugins §5.3; the doubt of issue #613; #1100 | the old `PeerRow` used `ctx.instance()`; a run row uses `plugin#n` (`crates/rutis-bridge/src/session/plugin/run.rs:168`), and `crates/rutis-bridge/src/session/plugin/rows.rs` keys by it |
| `rows.ended` is sent only for a generation the far end has not unloaded, and not once the session is cancelled | Remote sessions §4.3 | `crates/rutis-bridge/src/session/plugin/rows.rs:138-175` (`follow`: `loaded.remove(&key)`, `ended && !cancelled`) |
| `rows.failed` goes out once per generation; the host keeps the row and the peer unloads it and loads a new one | Remote plugins §4.2; #688 | `crates/rutis-bridge/src/runtime/process.rs:1400-1418`; `crates/rutis-bridge/src/session/plugin/run.rs:171-175` |
| `rows.unload` is idempotent: a row already gone is only answered | the row contract `rows.v2` | `crates/rutis-bridge/src/session/plugin/rows.rs:197-209` ("Already gone: unloading is idempotent") |
| A replacement closes the old session, stops reading it, cleans its rows and services, then answers the new `hello` | Remote plugins §4.4; remote sessions §4.2 | the listener's order in every runtime; each session has its own `Server` in `crates/rutis-bridge/src/session/plugin/rows.rs`; `crates/rutis-bridge/src/session/plugin/mod.rs:334-430` |
| A row stops with its session and runs again on the next one | Remote sessions §4.3, §7 | `RowHandle`/`RunRow` in `crates/rutis-bridge/src/session/plugin/run.rs`; `crates/rutis-loader/tests/session_rows.rs::rows_run_on_the_session_of_their_connection_row`, `::a_far_session_unloading_first_leaves_the_row_waiting` |
| A row's service import follows the row: unloading, ending or losing the session withdraws it | Remote plugins §2.6 decision 1, §5.3 | `crates/rutis-bridge/src/session/plugin/services.rs:232-378` (a session's `Imports`); `crates/rutis-bridge/src/runtime/process.rs:1306-1314` (`forget_exports`), `:1356-1366` (`unload_row` then `forget_exports`/`imports.forget_row`) |
| A late instruction of the old session does not act on the new lease | Remote plugins §4.4 | each session has its own handler; `PeerRows.tla` writes this as matching a generation |
| `link.offers` versions increase and older announcements are ignored; each family carries the number of its registration (`since`), and a family registered again is a new offer | Remote plugins §5.2 (`link.offers`, `since`) | `crates/rutis-bridge/src/peer.rs:44-67` (`Offers`, `epoch`), `:131-153` (`announce`), `:160-178` (older versions ignored), `:253-289` (`register` keeps the handler with its `since`); regression test `:309` `a_family_is_announced_only_with_its_offer` |
| Once the far end offers `plugins`, the loader asks the schema, refreshes resolution and provides `PeerRows`; rows do not start before the declaration is complete; offline rows wait rather than being Unresolved; withdrawing `plugins` withdraws `PeerRows` while Peer and the other features stay | Remote plugins §5.3 | today's equivalent path is the `run` row: `crates/rutis-bridge/src/session/plugin/run.rs:89` describes first, `:168` takes a new key, `:186` loads; `crates/rutis-bridge/src/session/plugin/mod.rs:432-461` waits for the far end's `rows` before creating the process; `crates/rutis-bridge/src/peer.rs` observes the `plugins` epoch in `offers` |
| `plugins.describe`/`rows.schema` returns the plugin's declaration, NotFound when it is not installed; `load` loads by `isolate`/`inject`; `update` and `unload` act only on a loaded key | Remote plugins §5.2 | `crates/rutis-bridge/src/session/plugin/rows.rs:83-97` (`schema`), `:99-135` (`load`), `:177-195` (`update`), `:197-209` (`unload`) |
| A load cancelled in flight, or an unload that arrives before its load, leaves no plugin running unowned (the doubt moved from #625) | Remote plugins §5.1, §5.3 | `crates/rutis-bridge/src/session/plugin/rows.rs:99-135` (the key goes into the table before the apply is awaited; a failed apply removes and disposes it), `:197-209` (unload is idempotent); `crates/rutis-bridge/src/session/plugin/run.rs:42-51` (`RowHandle::drop` aborts the load and disposes the fiber once mounted) |
| Service announcement/withdrawal versions come from the session's single increasing sequence; the importer keeps the withdrawn version and an older announcement does not restore it | Remote plugins §5.2 | `crates/rutis-bridge/src/services.rs:33-135` (`ExportOne`, `next_version`); `crates/rutis-bridge/src/session/plugin/services.rs:239-245` (a version not newer is ignored), `:335-378` (withdrawal and the tombstone) |
| Two nodes host for each other; feature plugins depend only on the session being ready, not on remote rows | Remote plugins §5.1 item 1, §10 | `crates/rutis-bridge/src/session/plugin/mod.rs:334-430` (`mount` registers rows before it answers), `crates/rutis-bridge/src/session/plugin/rows.rs:318-333` (a `Rows` per session) |
| A broken link stops the rows it hosted; a reconnect loads them again | Remote plugins §3.2, §5.3 | the two cases in `crates/rutis-loader/tests/session_rows.rs`; a `run` row follows its session |

## The model

### `PeerRows.tla`: sessions, instance key, generations

- **Participants**: one host (a table per session, the ending and failure notices, the service imports), one peer node with two rows that it loads again on the next session, and the peer's two control sessions: session 1 (old) and session 2 (the replacement). Two rows, `a` and `b`, one instance key each, the key the same on every load; each row has at most two generations (one per session).
- **Sessions**: `1..MaxSessions` (2). The peer opens them in order; each direction of a session is FIFO and delayed arbitrarily long. After a break the peer reads no more of it (`PLoss`) and neither does the host (`CanRead`); a `rows.ended`/`rows.failed` already written but not read may arrive late (`late`).
- **The peer** (`PDial`, `PGreeted`, `PLoad`, `PLoaded`, `PUnload`, `PUnloaded`, `PEnded`, `PFailed`, `PLoss`, `PLateEnded`, `PLateFailed`): `PLoad(r)` loads a row on the session, the key is the instance and the generation goes up by one; `PEnded`/`PFailed` act only on a notice of the row's current generation; `PLoss(s)` stops the rows on that session (`idle`), what it imported goes with it, and undelivered ending/failure notices become late; under the protocol `PLateEnded`/`PLateFailed` change nothing, and the `lateEnding` mutation lets them act by row key.
- **The host** (`HTakeUp`, `HAnswer`, `HEndSession`, `HReadLoad`, `HReadUnload`, `HEndSelf`, `HFail`, `HSweep`, `Break`): `HTakeUp(s)` takes up a new connection's `hello`, closes the old session and starts cleaning it; `HAnswer(s)` answers only once every earlier session's rows are cleaned (`Swept`); `HReadLoad` puts the row into that session's own table; `HReadUnload` is idempotent; `HEndSelf`/`HFail` are a row ending on its own or its code failing; `HEndSession`/`HSweep` end the session and dispose its rows; `Break(s)` is the environment.

### `PeerOffers.tla`: `link.offers`, the gate, offline

- **The far end** (`BOffer`, `BWithdraw`): registering `plugins` puts the family and its `since` into one snapshot; a second registration has a higher `since`; withdrawal is a snapshot without the family. The `twoOffers` mutation is the old shape of #435/#437: a snapshot lists the family without its `since` (epoch 0), the next one carries the real `since`.
- **The loader side** (`ADeliver`, `ARefresh`, `ARowLoaded`, `ALinkDown`, `ALinkUp`): applies only a higher-version snapshot; when the `plugins` epoch changes it stops the old row and sets the gate to `refreshing`; `ARefresh` finishes the schema refresh and opens the gate; `ARowLoaded` lets the row run; withdrawal makes the epoch 0, closes the gate and returns the row to `idle`; a link that goes down makes the row wait and forgets the applied snapshot; when it comes back the far end sends its current snapshot again.
- **Properties**: `EpochWithFamily` (a snapshot that lists the family carries its registration number), `GatedStart` (a row starts only with the gate open, `plugins` offered and a non-zero epoch), `WithdrawalStops`, `NoFailedRow` (#438: withdrawing the gate before the row applies does not leave it Failed), `OfflineWaits` (offline the row waits, it does not run), `ReloadsOnOffer` (a new registration is followed).

### `PeerHosted.tla`: hosting operations and orphans

- **The far end**: `Describe(k)` (`plugins.describe`/`rows.schema`), `Load(k)` (`plugins.load`), `Update(k)`/`UpdateEarly(k)`, `Unload(k)`, `CancelLoad(k)` (the row is cancelled while loading), `Close` (the session ends).
- **The host**: `loading` (accepted, apply not finished), `loaded` (applied), `tomb` (an unload arrived while loading: dispose right after the apply), `orphan` (loaded but the far end no longer wants it), `schema`/`described` (what describe returned), `badLoad`/`badUpdate` (ghosts).
- **A cancelled load**: in the base model `CancelLoad` sends an unload; if the host is still applying it records a tombstone and `FinishLoad` disposes the row. The `cancelNoUnload` mutation is the other half of #625: the far end registered no disposer, sends nothing, and the host finishes with an orphan. In the base model `Unload` for a key still loading records a tombstone; the `noTombstone` mutation forgets it, the later `FinishLoad` inserts the key and leaves an orphan.
- **Properties**: `NoOrphan`, `DescribeExact` (describe agrees with the catalog), `LoadOnlyDescribed` (load only loads what describe found), `UpdatingLoadedOnly` (update does not reach a row whose apply is still out).

### `PeerPair.tla`: two nodes hosting each other, cold together

- **Two nodes** A and B on one connection; each has a row out on the other (`out`) and a table of the other's rows here (`inn`). `Connect` opens the connection and clears the answered state but keeps the old session's rows in `inn`; `Clean(n)` disposes those rows; `Answer(n)` answers the `hello`, and the base rule only requires `inn[n]` to be empty, not `out[n]` to be loaded; `StartOut(n)` loads its own row once the other answered; `Loaded(n)` finishes; `Break` breaks the link and the rows stop waiting.
- **Properties**: `CleanedBeforeAnswer`, `RowsStopOnBreak`, `NoWaitingForOut` (the answer does not wait for one's own row; the `waitOut` mutation records that wait), `EventuallyBothLoaded` (while the link stays up both rows eventually run).

### `PeerServices.tla`: service versions and tombstones

- The provider takes one increasing sequence of versions for `announce`/`withdraw`; the importer applies only a message newer than `seen` and keeps the withdrawn version (`withdrawnAt`); an announcement older than the withdrawal does not restore the service. The `staleWins` mutation lets an old announcement provide anyway; the `noTombstone` mutation forgets the withdrawn version when the withdrawal arrives before any announcement.
- **Property**: `NoStaleRestore`.

## The key: the model and today's code (decision input for #1100)

| | The old `PeerRow` / this model | A current `run` row |
| --- | --- | --- |
| The row's key | the row's instance (`ctx.instance()`), the same across sessions | `plugin#<n>`, taken anew on every load (`run.rs:168`) |
| The generation in a notice | the frame carries `<<kind, row, generation>>`, and the peer acts only on the current generation | the frame carries the key only; the key is already unique, so an old generation's key does not match a new one |
| The peer's per-session table | `Server.loaded` by instance key; a second load of the same key is refused | `Server.loaded` by `plugin#n`; a second load of the same key is refused |
| The identity of one instance across a restart | kept by the instance key; safety rests on matching the generation and cleaning the old session first | not kept; an old `rows.unload`/`rows.update` uses the old key, the new row a new one |
| A late notice | must be dropped by its generation (or by the registration it belongs to); matching the row key alone is the `lateEnding` counterexample | does not match: the key differs; `process.on_row_ended`/`on_row_failed` are keyed (`process.rs:1380`, `:1400`) |

`PeerRows.tla`'s `OwnGenerationOnly` is the condition a stable-key design needs: when the key is the same across restarts, `rows.ended`/`rows.failed` must carry the generation of that load (or an equivalent registration identity), or a late notice of an old generation ends/restarts the new one; `OneInstanceAtATime` rests on the host cleaning the old session before answering the new `hello`, while `Server.loaded` only keeps one load per session. Today's `plugin#n` uses a fresh key instead of matching a generation; the price is that one instance is not identifiable across restarts and `unload`/`update` must hold the current key. If #1100 changes the key to a stable instance, the generation/registration identity must go into the notice (or the match must be by session registration). This model is positive evidence for that decision, not a proof about today's run rows, which have their own evidence (the two cases in `session_rows.rs`).

## The pre-fix configurations (#438, #435/#437)

- **#435 / #437** (two families registered together were announced as two offers and the `peer:` row restarted): the regression case `a_family_is_announced_only_with_its_offer` in today's `peer.rs` (`:309`) is exactly this; the `twoOffers` mutation of `BOffer` reproduces its shape — one snapshot lists `plugins` without its `since` (epoch 0), the next carries the real `since`, so the loader sees two offers of one registration. `offers-mut-two-offers` expects `EpochWithFamily` to be violated. Today's code registers the handler and its `since` in one step (`register`, `:253`), so the mutation is not today's behaviour.
- **#438** (`PeerRows` withdrawn after the row was scheduled and before it applied, leaving the row Failed): `PeerRowsPlugin`/`PeerRow` were removed with 0.19, so **the old configuration cannot be reproduced against today's code**; today's equivalent is a `run` row, without that gate. The model keeps the design rule and pins its shape with `offers-mut-gate-fail`: `plugins` is withdrawn while the row is `starting` (`gateCloseFails`), and setting the row to `failed` violates `NoFailedRow`; the base model returns it to `idle` to wait for the next offer. The current counterpart is `session_rows.rs::a_far_session_unloading_first_leaves_the_row_waiting` (a far session unloading first leaves the row waiting), not the old `PeerRows` configuration.

## Simplifications

- The scale is deliberately finite: `PeerRows` has 1 host + 1 peer, 2 rows, 2 sessions and 2 generations per row; `PeerOffers` 1 row and 2 registrations; `PeerHosted` 1 key and a catalog of 1 plugin; `PeerPair` 2 nodes, 1 connection and at most 2 rounds; `PeerServices` 1 service. Reconnect counts, backoff and timing are not modelled: the environment is only "the link breaks at some point, and comes back", more general than real timing.
- `PeerRows`' two sessions are sequential: the peer opens the new one only after the old one broke. Concurrent handshakes with the same id, the old session alive during a handshake, and an access point's takeover order are not modelled (`remote-lease` and `access-*` cover them); what stays is the one interleaving that matters here — the host answers the new `hello` while the old session's rows may still be there.
- The generation is the model's name for one load: `PeerRows`' frames carry it, a real frame carries only the key. What tells the peer two generations apart in the code is which registration (which session's `Process`) delivered the notice; the model writes that as a generation, and `lateEnding` is exactly "match the row key only". A current `run` row distinguishes them by its fresh key (see the section above).
- No `rows.ended` is sent once the session is cancelled (`!cancelled` in `rows.rs`): the row is treated as disconnected and waits for the next session. `PLateEnded`/`PLateFailed` therefore change nothing under the protocol; a late notice is what the environment may do, not an instruction the protocol allows.
- In `PeerRows` the service announcement and withdrawal are not frames: the import is taken when the load is accepted and withdrawn when the row goes, each in one atomic step. `PeerServices` models only versions and the tombstone, not service calls, their shape, reference counting or real network delays; multi-hop is not modelled (see "Not covered").
- `PeerOffers`' link down/up is atomic: `ALinkDown` forgets the applied snapshot, `ALinkUp` has the far end send its current snapshot again; dialling, reconnect backoff and the handshake deadline are not modelled. `PeerOffers` has one loader-side row and the schema refresh is one step; the fields of `plugins.describe` (version, integrity) are not modelled, only found/missing.
- `PeerHosted` splits one key's load into "accepted" and "apply finished", with unload and update one step each; the catalog is only "has it / does not", not plugin configs, dependencies or real apply time. The end of the session is atomic; `NoOrphan`'s conclusion is about this abstraction.
- `PeerPair`'s one connection, two nodes and one row each are finite; authentication, the 10-second handshake deadline and capabilities are not modelled; "the old rows are cleaned" is one step, where the code disposes row by row. The `waitOut` mutation only records the bad strategy "the answer waits for one's own row"; no timeout.
- `PeerServices` has one session, one service and one sequence of messages; messages may arrive out of order and versions order them; multi-hop, reference forwarding, call-chain routing and multiple links are not modelled (#626).
- Events, proxies, the config differences of `plugins.update`, lease upgrades, memory order, task scheduling and frame encoding are not modelled; every action is atomic.
- A second load of the same key in one session (refused by `rows.rs`) is unreachable in `PeerRows`: the peer loads a row only while it is `idle`, once per session; the model writes that as "already loaded: ignore" and does not model the refusal's reply. `PeerHosted`'s base model requires describe to find the plugin before load; repeated loads are bounded by `loads[k] < MaxLoads`, not by a refusal path.

## Invariants and liveness

| Name | Module | Content |
| --- | --- | --- |
| `OneInstanceAtATime` | PeerRows | A row's instance runs in one session at a time: the new session loads it only after the old session's rows are cleaned |
| `OwnGenerationOnly` | PeerRows | The peer acts only on a `rows.ended`/`rows.failed` of the row's current generation: a late notice of an earlier one neither ends nor restarts the new one |
| `ServicesFollowRows` | PeerRows | A row's service is imported only while the generation providing it is loaded: unloading, ending on its own or losing the session withdraws it; no import outlives the row |
| `CleanupOnce` | PeerRows | A generation is disposed at most once: `rows.unload`, the row's own end and the session's sweep do not dispose it twice |
| `EndedRowsCleaned` | PeerRows | Liveness: once a session ended, the rows it loaded are cleaned (and their services withdrawn) |
| `EpochWithFamily` | PeerOffers | A snapshot that lists `plugins` carries its registration number: no intermediate snapshot with the family and epoch 0 |
| `GatedStart` | PeerOffers | A row starts only with the gate open, `plugins` offered and a non-zero epoch (`starting`/`running`) |
| `WithdrawalStops` | PeerOffers | Withdrawing `plugins` closes the gate and stops the row; Peer is unaffected |
| `NoFailedRow` | PeerOffers | Withdrawing the gate before the row applies does not leave it `failed` (the shape of #438) |
| `OfflineWaits` | PeerOffers | With the link down the gate is closed and the row waits; it does not run |
| `ReloadsOnOffer` | PeerOffers | Liveness: while the link stays up, the loader eventually runs the far end's current `plugins` registration |
| `NoOrphan` | PeerHosted | A row the far end no longer wants does not stay here: a load cancelled in flight is disposed after the apply, an unload that arrived first leaves a tombstone |
| `DescribeExact` | PeerHosted | Describe agrees with the catalog: found if it is here, missing if not |
| `LoadOnlyDescribed` | PeerHosted | Load loads only a plugin describe found |
| `UpdatingLoadedOnly` | PeerHosted | Update does not reach a row whose apply is still out |
| `CleanedBeforeAnswer` | PeerPair | When a `hello` is answered, the old session's rows here are cleaned |
| `RowsStopOnBreak` | PeerPair | A break stops both nodes' outgoing rows |
| `NoWaitingForOut` | PeerPair | The answer does not wait for one's own outgoing row (the `waitOut` mutation sets the ghost) |
| `EventuallyBothLoaded` | PeerPair | Liveness: while the link stays up, both rows eventually run |
| `NoStaleRestore` | PeerServices | A withdrawal is a tombstone: an announcement older than it does not restore the service |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 21.0.12 (Temurin), 32 cores, `-workers 4`, 2026-11-10. `ok` means the exhaustive search completed and every listed property holds; a violated configuration stops at the first counterexample, so its state count depends on the search order and may differ from run to run. Every configuration here is small and all were run; the whole directory takes about 30 seconds.

| Configuration | Module | Content | Result | Distinct states | Time |
| --- | --- | --- | --- | --- | --- |
| `peer-healthy` | PeerRows | 2 rows, 2 sessions, breaks allowed; a row may end on its own; every safety invariant | ok | 36,083 | 1–2 s |
| `peer-failure` | PeerRows | 2 rows, 2 sessions; a row's code may fail and the peer unloads it and loads a new one; every safety invariant | ok | 161,503 | 2 s |
| `peer-clean` | PeerRows | As `peer-healthy`, liveness only | ok | 36,083 | 1–4 s |
| `peer-mut-late-ending` | PeerRows | A late `rows.ended`/`rows.failed` of the old generation acts on the new one by row key | violates `OwnGenerationOnly` | — (stops at the counterexample) | 0–1 s |
| `peer-mut-two-sessions` | PeerRows | A replacement answers the new `hello` before the old session's rows are cleaned | violates `OneInstanceAtATime` | — (stops at the counterexample) | 0–1 s |
| `peer-mut-keep-rows` | PeerRows | The host ends the session but keeps its rows | violates liveness | 36,083 (stops at the counterexample) | 1–3 s |
| `peer-mut-service-residue` | PeerRows | A row's service is not withdrawn when the row goes or its session ends | violates `ServicesFollowRows` | — (stops at the counterexample) | 0–1 s |
| `offers-healthy` | PeerOffers | One registration, schema refresh, gate, row start; link stays up | ok | 5 | 0–1 s |
| `offers-withdraw` | PeerOffers | Register, withdraw, register again; the gate and the row follow, and the last registration is eventually followed | ok | 29 | 0–1 s |
| `offers-offline` | PeerOffers | The link breaks and comes back: the row waits, then loads again | ok | 18 | 0–1 s |
| `offers-mut-two-offers` | PeerOffers | One snapshot lists `plugins` without `since`, one registration becomes two offers (#435/#437) | violates `EpochWithFamily` | — (stops at the counterexample) | 0–1 s |
| `offers-mut-gate-fail` | PeerOffers | The gate is withdrawn before the row applies and the row is set to Failed (the shape of #438) | violates `NoFailedRow` | — (stops at the counterexample) | 0–1 s |
| `offers-mut-keep-row` | PeerOffers | Withdrawing `plugins` leaves the gate open and the row running | violates `WithdrawalStops` | — (stops at the counterexample) | 0–1 s |
| `offers-mut-no-reload` | PeerOffers | A new registration is not followed | violates liveness | — (stops at the counterexample) | 0–1 s |
| `hosted-healthy` | PeerHosted | Describe, load, update, unload, cancel in flight; no orphan | ok | 42 | 0–1 s |
| `hosted-mut-cancel` | PeerHosted | A load cancelled in flight without an unload (no disposer was registered) | violates `NoOrphan` | — (stops at the counterexample) | 0–1 s |
| `hosted-mut-unload-first` | PeerHosted | An unload arrives before its load and the host has no tombstone | violates `NoOrphan` | — (stops at the counterexample) | 0–1 s |
| `hosted-mut-update` | PeerHosted | An update reaches a row whose apply is still out | violates `UpdatingLoadedOnly` | — (stops at the counterexample) | 0–1 s |
| `hosted-mut-load-missing` | PeerHosted | A load without a describe that found the plugin | violates `LoadOnlyDescribed` | — (stops at the counterexample) | 0–1 s |
| `hosted-mut-describe` | PeerHosted | Describe answers found for a plugin the host does not have | violates `DescribeExact` | — (stops at the counterexample) | 0–1 s |
| `pair-healthy` | PeerPair | Two nodes start cold together and host for each other | ok | 26 | 0–1 s |
| `pair-reconnect` | PeerPair | One break and reconnect: rows stop, old rows are cleaned, both load again | ok | 70 | 0–1 s |
| `pair-mut-wait-out` | PeerPair | The answer waits for one's own row, so two cold nodes wait for each other | violates liveness | — (stops at the counterexample) | 0–1 s |
| `pair-mut-keep-rows` | PeerPair | A break leaves the outgoing rows marked loaded | violates `RowsStopOnBreak` | — (stops at the counterexample) | 0–1 s |
| `pair-mut-answer-before-clean` | PeerPair | The next `hello` is answered before the old session's rows are cleaned | violates `CleanedBeforeAnswer` | — (stops at the counterexample) | 0–1 s |
| `svc-healthy` | PeerServices | Announce, withdraw, announce again; out-of-order messages, older ones ignored | ok | 74 | 0–1 s |
| `svc-mut-stale` | PeerServices | An announcement older than the current version still provides | violates `NoStaleRestore` | — (stops at the counterexample) | 0–1 s |
| `svc-mut-no-tombstone` | PeerServices | A withdrawal that arrives first does not keep its version; an older announcement restores the service | violates `NoStaleRestore` | — (stops at the counterexample) | 0–1 s |

## Mutations and counterexamples

Every checked property has a deliberate bug that TLC must find (`violated:<name>` in `expected.txt`; `violated:liveness` for the temporal one):

| Configuration | Mutation | Violated | Counterexample |
| --- | --- | --- | --- |
| `peer-mut-late-ending` | `PLateEnded`/`PLateFailed` ignore the generation and end or restart the row by key | `OwnGenerationOnly` | On session 1, `a` is generation 1; the plugin ends on its own and `rows.ended(a,1)` is written but never read; the connection breaks, the peer opens session 2 and loads `a` as generation 2; the late ending arrives and ends the new generation (`pEndedFrom = 1`, `pkey = 2`) |
| `peer-mut-two-sessions` | `HAnswer` does not require earlier sessions to be `Swept` | `OneInstanceAtATime` | Session 1 has `a` as generation 1; the connection breaks; the peer opens session 2; the host closes the old session but answers without waiting for the cleanup; `loaded[1][a] = 1` and `loaded[2][a] = 2` at once |
| `peer-mut-keep-rows` | `HSweep` is off | liveness | Session 1 loaded a row and broke; the host ends the session, but the row stays in the table forever and `Swept(1)` never holds |
| `peer-mut-service-residue` | The service is not withdrawn when the row is unloaded, ends on its own, or its session is cleaned | `ServicesFollowRows` | Session 1 loads `a`, whose service is imported as `<<1,1>>`; the plugin ends on its own and the host removes the row but keeps the import |
| `offers-mut-two-offers` | A registration snapshot lists the family without `since`, the next one with it (#435/#437) | `EpochWithFamily` | `aFams = {"rows","plugins"}` while `aEpoch = 0`: one registration is seen as two offers |
| `offers-mut-gate-fail` | The gate is withdrawn while the row is `starting` and the row is set to `failed` (#438) | `NoFailedRow` | After the `plugins` registration the loader is `starting`; the withdrawal arrives, the row becomes `failed` and never waits for the next offer |
| `offers-mut-keep-row` | Withdrawing `plugins` does not close the gate or stop the row | `WithdrawalStops` | The snapshot no longer lists `plugins`, but `aGate = "open"` and `aRow = "running"` |
| `offers-mut-no-reload` | A new epoch only changes `aEpoch`; the gate is not refreshed and the row not started | liveness (`ReloadsOnOffer`) | `bSince = 1` and `aEpoch = 1`, but the gate stays `closed` and the row `idle` |
| `hosted-mut-cancel` | A load cancelled in flight and the far end sends no unload (no disposer) | `NoOrphan` | `Load(a)`, `CancelLoad(a)`, `FinishLoad(a)`: the host has `loaded = {"a"}` and `orphan = {"a"}` |
| `hosted-mut-unload-first` | An unload arrives before its load and the host has no tombstone | `NoOrphan` | `Load(a)` is still `loading`, `Unload(a)` is forgotten; `FinishLoad(a)` inserts `a`, `orphan = {"a"}` |
| `hosted-mut-update` | An update reaches a row whose apply is still out | `UpdatingLoadedOnly` | After `Load(a)` and before `FinishLoad(a)`, `UpdateEarly(a)` sets `badUpdate = TRUE` |
| `hosted-mut-load-missing` | A load is allowed without a describe that found the plugin | `LoadOnlyDescribed` | The catalog is empty and `Load(a)` is still accepted; `badLoad = TRUE` |
| `hosted-mut-describe` | Describe answers found for a plugin the host does not have | `DescribeExact` | The catalog is empty; after `Describe(a)`, `schema[a] = "found"` |
| `pair-mut-wait-out` | The answer requires one's own outgoing row to be loaded | liveness (`EventuallyBothLoaded`) | Both nodes are cold and both `Answer` actions wait for their own `out`, while `StartOut` waits for the other's answer; the link stays up and neither row ever loads |
| `pair-mut-keep-rows` | A break leaves the outgoing rows `loaded` | `RowsStopOnBreak` | After `Break`, `out[A] = out[B] = "loaded"` |
| `pair-mut-answer-before-clean` | The answer does not require the old session's rows to be cleaned | `CleanedBeforeAnswer` | After the first round's rows and a break, the second `Answer` happens while `inn` still has the old rows; `badAnswer = TRUE` |
| `svc-mut-stale` | An announcement older than the current version still provides | `NoStaleRestore` | After withdrawal version 2, announcement version 1 arrives and `state = "provided"`, `bad = TRUE` |
| `svc-mut-no-tombstone` | A withdrawal that arrives first does not keep its version | `NoStaleRestore` | Withdrawal version 2 arrives first and `seen` stays 0; announcement version 1 arrives and provides, `bad = TRUE` |

The `peer-mut-two-sessions` mutation also breaks "clean before answering"; the configurations are split so each counterexample carries one conclusion (as in `row-lifecycle` and `event-close`). `CleanupOnce` has no mutation of its own: in `PeerRows` all three disposal paths first check that the generation is still in the table, so a second disposal is unreachable; any path forgetting the check fails it. `PeerHosted` does not duplicate "dispose once"; `PeerRows`' `CleanupOnce` covers it.

## Conclusion

- Under the rules of remote plugins §4.4 — a replacement stops reading the old session and answers the new `hello` only after the old session's rows are cleaned — the exhaustive search holds: one instance never runs in two sessions at once; the new session loads a row only after the old session's rows are cleaned; a row stops with its session and runs on the next one.
- The peer acts on `rows.ended`/`rows.failed` only by the load the notice belongs to: a late ending of an earlier generation does not end the new one, and a late failure does not restart it. With a key that is the same across restarts this must rest on the notice's registration; matching the row key alone is exactly the `lateEnding` counterexample. Today's `run` row avoids this with a fresh key but loses the identity of one instance across restarts (#1100).
- `link.offers`' versions and `since` suffice in the exhaustive search: older announcements are ignored; one registration never appears as a snapshot with the family and no epoch; withdrawal withdraws only `PeerRows` and its rows while Peer stays; a new registration is eventually followed.
- The `PeerRows` gate rules hold: the gate opens only after the schema refresh; offline the row waits and loads again when the link is back; withdrawing the gate before the row applies does not leave it Failed (the shape of #438).
- The host's hosting operations follow the rules: describe agrees with the catalog; load loads only what describe found; update does not reach a row whose apply is still out; a cancelled load or an unload that arrived first leaves no orphan — provided the host keeps a tombstone and the far end unloads a row it cancelled while loading (the two halves of #625).
- Two nodes hosting for each other and starting cold together do not wait for each other: answering `hello` does not depend on one's own outgoing row; a break stops the rows, the old rows are cleaned before the next answer, and both rows run again.
- Service versions and the tombstone hold: an older announcement does not restore a withdrawn service.
- What the model does not prove: concurrent handshakes with the same id and an access point's takeover order (`remote-lease`, `access-*` cover them), real backoff and timing, multi-hop services (#626), the fields of `plugins.describe`, the shape of service calls and reference release, events and proxies, the differences of each language's listener, memory order and real scheduling; and whether today's code can really see the "unload before the load inserts" interleaving — the model allows it and `rows.rs`'s `unload` keeps no tombstone for an unknown key, so this #625 doubt is not closed by the code.

## Running it

Needs Java 11+ and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh                     # every configuration
TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh offers-healthy      # one configuration
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh      # bound TLC's workers
```

`run.sh` picks the module by prefix: `peer-*` → `PeerRows.tla`, `offers-*` → `PeerOffers.tla`, `hosted-*` → `PeerHosted.tla`, `pair-*` → `PeerPair.tla`, `svc-*` → `PeerServices.tla`. It compares each configuration's outcome with `expected.txt` and exits 0 when all match; the logs and counterexamples go to `TLC_OUT` (a temporary directory by default). No configuration here belongs in a weekly run; CI's `specs` job does not list `peer-rows` yet (see [CI](../../docs/ci.en.md) §4; part of #640's close-out).

## Maintenance

When remote plugins §4.4, §5.1–§5.3 or remote sessions §4.2, §4.3 change, or when `peer.rs`'s `Offers`/`since`/`register`, the per-session table and load/unload of `session/plugin/rows.rs`, the row key and `describe` of `session/plugin/run.rs`, the import versions of `session/plugin/services.rs`, `rows.ended`/`rows.failed`/`unload_row` in `runtime/process.rs`, or `ExportOne`/`next_version` in `services.rs` change, change the corresponding model, `expected.txt`, `run.sh` and this file with them (standard Q11.2.3).
