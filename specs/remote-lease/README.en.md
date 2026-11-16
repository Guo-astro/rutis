# Remote leases: takeover, reconnection and start/stop (TLA+)

[中文](README.md)

TLC explores every interleaving of a remote runtime's lease, takeover by a newer connection, a link reconnecting and stopping, and an app hosting its plugins stopping and starting again (several times, a stop during a takeover included). Issues #210 and the items merged from #615; quality standard Q11.2, Q6.13.3; status §7.2 (risks E4, E5). Two models:

- `RemoteLease.tla`: a listening remote runtime and the controller links that dial it;
- `AccessPoint.tla`: far ends dialing a `rutis-host/listen` row: handshakes side by side, one session per id, a newer connection of the same id replacing the older, and the dialers' reconnection and start/stop rules.

**Reconciliation (2026-11-10).** The model now covers these items of [#615](https://github.com/arcships/rutis/issues/615): authentication as a step of the takeover order (`Auth = "step"`), the cleanup deadline (`Cleanup = "deadline"`), app-host stop-then-start (`MaxRounds` / `MaxStops`, properties `RestartIsFresh`, `FinalStopStaysDown`, `RestartRecovers`, including four rounds stopped once each (three starts after a stop) and a stop during a takeover), liveness after faults (`MaxFaults`, `lease-faults-live` / `access-faults-live`), the heartbeat of a new connection starting at the answer to its hello in every runtime (Kotlin #1102; Go, Swift, Node, Bun #1182; `NoHeartbeatBeforeHello`, mutation `heartbeatFromAccept`), the static listen link after revocation (registration kept, 4003, a revocation during the handshake: `access-static-revoke`, `access-mut-revoke-after-handshake`), `RutisApp` start/stop several times and a start/dial after the final stop (`lease-app-restart-3`, `access-app-restart`, `lease-mut-start-after-final-stop`, `access-mut-final-stop-dials`), and the `.accepted` connection arriving after a stop (`lease-mut-late-accept`). The correspondence table now lists Swift / Kotlin / app hosting / the Cordis node link. Still not covered: the Cordis node link's code (`node/rutis-runtime/src/bridge/link.mjs`) is not in this repository, so it can only be checked against the same dial rules; timers (backoff, heartbeat, kill deadline) are not in the model, see "Simplifications". Node / Bun now kill at `RUTIS_SHUTDOWN_TIMEOUT + 2 s` (this branch, f57b4138, `serve.mjs` / `remote.ts`); Kotlin, Go, Swift, Node and Bun all start a connection's heartbeat at the answer to its hello (see "Simplifications").

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Controller (a `rutis-host/dial` row): dial, back off and reconnect, wait by failure category, stop | [remote plugins](../../docs/design-remote-plugins-2026-10-03.en.md) §3.2; [dial-out](../../docs/design-remote-runtime-dial-2026-10-14.en.md) §6; [remote session](../../docs/design-remote-session-2026-10-28.en.md) §4.1 | `crates/rutis-host/src/connect/dial.rs`: dial, handshake, mount the session, wait for it to unload, back off |
| The runtime's lease and takeover order (authenticate → close the old session → clean the old lease → answer the new hello); the cleanup's deadline; where a new connection's heartbeat starts | remote plugins §4.4; [remote session](../../docs/design-remote-session-2026-10-28.en.md) §4.3 | `python/rutis/rutis/__main__.py` `serve`; `go/rutis/internal/runtime/listen.go` `listen`; `node/rutis-runtime/src/serve.mjs` `serve`; `bun/rutis-bun/src/remote.ts`; Swift `swift/rutis/Sources/Rutis/Remote/Listen.swift`, `swift/rutis/Sources/Rutis/Remote/App.swift`; Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Listen.kt`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Frames.kt` |
| App hosting: stop then start (several times), a start during the stop taking effect once it is done; no start or bind after the final round; dials and rounds counted afresh | [Swift SDK](../../docs/design-swift-sdk-2026-10-12.en.md) §7; [Android SDK](../../docs/design-android-runtime-2026-10-12.en.md) §5 | `RutisApp.start` / `stop`: Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt`, Swift `swift/rutis/Sources/Rutis/Remote/App.swift`; this branch's f57b4138 fixes the `run(previous)` thread interleaving |
| The dialing side of an app (`connect`): start / stop, dials counted afresh per round, no dial after a stop | dial-out §6; Android SDK §5 | Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Dialing.kt`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt`; Swift `swift/rutis/Sources/Rutis/Remote/Dial.swift`, `swift/rutis/Sources/Rutis/Remote/App.swift` |
| listen: handshakes side by side, replacement by the same id (old session fully unloaded → old channel closed with 4002 → new session installed); a handshake result of a far end that left is not installed | remote session §4.2 | `crates/rutis-host/src/connect/listen.rs`: `Listening::run`, `Replaceable` |
| The static listen link after its id is revoked: registration kept, `Waiting { AuthRejected }`, later connections of the id closed with 4003 after their hello, one revoked during its handshake included | dial-out §5, §6; [tracking table](../../docs/formal-trace/rules/protocol-remote.md) DIA-5-7 / 5-10 / 5-11 / 5-13 | the revocation hook was removed in 0.19 (#273); no implementation to check against; existing connections are dropped by stopping the row. The half of DIA-5-10 about a revocation during the handshake is in the model |
| The dialer's reconnection rules | dial-out §6 | `python/rutis/rutis/dialing.py` `keep_dialing`; `go/rutis/internal/runtime/dial.go`; `node/rutis-runtime/src/dialing.mjs`; `bun/rutis-bun/src/dialing.ts`; Swift `swift/rutis/Sources/Rutis/Remote/Dial.swift`; Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Dialing.kt` |
| The Cordis node link: backoff (500 ms, up to 30 s, reset after a stable 60 s, 30 s after a refusal) and takeover | remote plugins §4.4, §5; dial-out §6 | `node/rutis-runtime/src/bridge/link.mjs` (code of the Cordis repository, referenced by #615; **not in this repository**, no implementation to check against. Its rules are the same as `node/rutis-runtime/src/dialing.mjs`; the model's controller is the specification) |

**Implementation scope (the static listen link).** The revocation hook of DIA-5-7, DIA-5-10, DIA-5-11 and DIA-5-13 was removed in 0.19 (#273), so there is no current implementation to check against: `access-static-revoke` and `access-mut-revoke-after-handshake` check the rules (registration kept, later connections of the id closed with 4003 after their hello, a connection revoked during its handshake or while waiting to be taken over not installed — the listen side of DIA-5-10). Existing connections are dropped by stopping the row.

## The models

Common to both: each direction of a connection is delivered in order, with any delay; a side that closed a connection reads nothing more on it, and frames to it are dropped (kept only where a mutation reads them). Controllers / dialers count by round: each round pays for `MaxConns` connections; a redial replacing a connection a fault lost does not use the quota (`free`). Transient faults (failed dials, lost connections) are bounded by `MaxFaults`; app rounds by `MaxRounds`, stops by `MaxStops`. These are the model's bounds, not the implementation's behaviour.

### `RemoteLease.tla`

- **Participants**: controllers `c1`, `c2` (rutis `rutis-host/dial` rows) and one listening remote runtime. Connection `<<c, n>>` is `c`'s n-th dial.
- **Controller** (`connect/dial.rs`): once a dial succeeds the upgrade is done and the runtime's listener queues the connection; the row waits for the runtime's hello (the handshake), then mounts the session and is `Ready`; the loader then loads rows through the session (`load` frames). A session that ends (replaced, 4002, or ended by the runtime) is retryable: back off, reconnect. Stopping (dispose) aborts the task: the session closes, the retry is cancelled. Environment: failed dials, handshake timeout or lost connection (`Lose`), stop.
- **Liveness after faults**: failed dials and lost connections together are at most `MaxFaults`; a failed dial does not use up a connection, and the redial after a lost connection is free. Once the faults are spent, the last connection stays, or one free redial remains, so `lease-faults-live` can check `EndsHeld` (without symmetry: TLC may miss a liveness violation with symmetry).
- **Runtime** (Python `serve`, the Swift `RutisApp` listening loop, Kotlin `Listening` and the others): one controller at a time. Taking the next connection while a session runs is the §4.4 order: ① authenticate (`Auth = "step"`: the connection queues first, the runtime checks it before it may enter the takeover queue; refusing closes only that connection and touches no session); ② close the old session's channel (4002) and read none of its frames; ③ clean the old lease (every row unloaded); ④ start the new session and answer its hello. When a session's far end closes, its lease is cleaned.
- **Cleanup deadline** (`Cleanup = "deadline"`): ③ is split into "cleanup starts" and "finishes, or is killed when the deadline passes". A kill is the old process disappearing: the old session's rows go with it, and only then is the new hello answered; at most one kill per run. `Cleanup = "instant"` keeps the old model's one-step cleanup.
- **The new connection's heartbeat**: a connection waiting for the old lease to be cleaned is not "silent"; its heartbeat starts when its hello is answered (`NoHeartbeatBeforeHello`). The mutation `heartbeatFromAccept` starts it at the accept, so the connection is closed with 1001 while the cleanup runs.
- **Stop then start** (app-hosted `RutisApp.listen`): a stop closes the current session and the connections accepted but not served (the one a takeover waits for included), cleans the lease and accepts no dial; a start opens a new round. The stop may come in any phase of a takeover (`closing`, `cleaning`, `starting`); a start asked for while the stop cleans up runs once it is done; the old round's sessions and queued connections are never served in the new one (`RestartIsFresh`). One stop per round, at most `MaxStops` in all; the stop of the last round is final, no start or bind follows (`FinalStopStaysDown`). `lease-app-restart-3` is the interleaving of four rounds, one stop each (three starts after a stop, the last stop final).
- **Reconnects**: each controller pays for at most 4 connections per round (3 reconnects); `lease-faults` uses 3 paid connections and 3 faults, see "Results".

### `AccessPoint.tla`

- **Participants**: far ends `d1`, `d2` dialing out, greeting with the same id (`SameId`) or ids of their own; on the rutis side one `rutis-host/listen` row.
- **Dialer** (`dialing.py` and its twins): a dial delivers the connection to listen and starts its handshake. rutis' hello means "connected"; a closed connection (4002, lost, a failed handshake) is waited out, then dialed again. A stopped dialer dials no more. An app that can `connect` (`RutisApp`) may start again after a stop: the new round numbers its dials afresh, the old round's connections are not read; after the stop of the last round it neither starts nor dials (`FinalStopStaysDown`).
- **Liveness after faults**: as in `RemoteLease`; once `MaxFaults` is spent, `access-faults-live` checks `EndsHeld` for every id (without symmetry).
- **listen** (`Listening::run`):
  - Handshakes run side by side in a `JoinSet`: `HandshakeDone` (answers hello; none of the far end's plugins is registered yet) or `HandshakeFails` (the far end closed, the connection failed).
  - `Join`: the loop takes a connection whose handshake is done. No session of its id: install it. Otherwise mark the old connection's closer replaced, take it out of `live`, start unloading the old session; the loop then sits on `unload().await` and does nothing else. A connection whose far end left is not installed (the hello result goes with the session).
  - `Replace`: once the old session is unloaded (`Unloaded`: every far plugin unloaded, the channel closed last, as 4002 since it is marked replaced), install the new session; one whose far end left meanwhile is dropped.
  - `Register`: an installed session registers the far end's plugins.
  - `EndSelf`, `Unloaded`, `ReadEnding`: a session ending on its own (far end closed, or lost) unloads; when the loop reads its ending, it forgets it only if `live` still holds it (same serial).
- **Revocation** (`Faults = {"revoke"}`): the environment revokes an id. An installed session unloads and its channel closes with 4003 (the model's `"rejected"` frame); the dialer waits as an authentication failure (`outcome = "authRejected"`). The listen row keeps its registration (`registered`), so later connections of the id still complete their handshake, receive rutis' hello, and are then closed with 4003 in `Join` without an installed session or registered plugins. The revocation may come during the handshake or while the connection waits to be taken over (the listen side of DIA-5-10): `Join` / `Replace` check `revoked` again when installing; the mutation `revokeAfterHandshake` only looks at the snapshot taken when the handshake completed and misses that revocation.
- **Reconnects**: each dialer pays for at most 4 connections per round; 3 paid + 1 fault in the configuration with lost connections, 2 + 1 with every fault, 2 with a static revocation, see "Results".

## Simplifications

- No time: backoff, the 30 s wait, the 10 s handshake limit, "a session lasting 60 s resets the backoff", and the 10 / 30 s heartbeat are all just "some later moment". A timeout is a `Lose` that may happen at any moment, more general than real timers. The cleanup's deadline is the same: once cleaning starts, "finished" and "killed at the deadline" are both possible actions, with no clock; at most one kill per run (`ghost.killed`). A kill is the old process disappearing, the old session's rows going with it (`killLeavesRows` is the counterexample). Node / Bun killing at `RUTIS_SHUTDOWN_TIMEOUT + 2 s` is a timer: the model's deadline configuration checks the two outcomes ("finished" or "killed"), while the deadline itself is checked by `serve.test.mjs` and `remote.test.ts`; 5 s and 12 s are the same action in the model.
- The heartbeat: only the order rule "a new connection waiting for the old cleanup is not closed by its heartbeat" is modelled (`NoHeartbeatBeforeHello`). The model is built for "from the answer to its hello"; the mutation `heartbeatFromAccept` is the implementation before the fixes: Kotlin's `FrameConnection` used to set `lastSeen` in its constructor (at the accept), as did Go's `startWSConn`, Swift's `WebSocket.init`, Node's `wrap` and Bun's `wrap`. #1102 fixed Kotlin (`Frames.kt` / `Listen.kt`); #1182 fixed Go, Swift, Node and Bun: every heartbeat starts after the session's hello.
- App-host stop and start: `MaxRounds` rounds, `MaxStops` stops, at most one stop per round; a stop may come in any phase of a takeover. The stop's cleanup is one step (a real `shutdownTimeout` would kill at the deadline, see above); "a start asked for while the stop cleans up" is `pending`, and the next round opens after the cleanup. The stop of the last round is final; `startAfterFinalStop` / `finalStopDials` are the mutations of starting or dialing again. The thread interleaving of `RutisApp.kt` was fixed in f57b4138; `access-app-restart` / `lease-app-restart-3` follow the fixed rules.
- The bounded number of reconnects, faults and start/stops is the model's bound, not the implementation's behaviour. Liveness is checked only in runs without a stop and with the faults bounded (`MaxFaults`); there every behaviour is finite, so `WF_vars(Next)` makes every enabled step happen. `lease-app-restart`'s `RestartRecovers` asks for a lease only while another round remains and a controller still has a connection to spend: one that has spent its connections cannot dial after the start, and the property does not demand it. `lease-app-restart-3` checks safety only: with further stops allowed, a later stop breaks the liveness demanded after an earlier one. Two controllers configured for the same runtime (or two dialers with the same id) keep replacing each other (written in dial-out §6); in the model the last one to connect holds the lease once the budget is spent, in the implementation backoff slows the alternation to about once every 30 s each.
- A lease competing across hosts is the two-controller configuration (`lease-healthy`, `lease-faults`, `lease-faults-live`): each host backs off on its own, no order is promised; the model's arbitrary delays are the interleaving, and `EndsHeld` says the lease lands on someone once the budgets are spent. Clock skew across hosts and network partitions are not modelled separately; a lost connection is `Lose`.
- The Cordis node link (`node/rutis-runtime/src/bridge/link.mjs`) is not in this repository, so there is no code to check; its backoff and takeover follow dial-out §6, and the model's controller is that specification. See "Not covered".
- `RemoteLease`: the controller's hello is left out (the runtime reads it when it starts the session; it has no other effect).
- `AccessPoint`: token routing (`Router`) is left out: a dial reaches the row; `registered` is a Boolean saying whether the row's registration is there. The far plugins are modelled only as "all registered" and "all unloaded", the reverse-order unload one step; calls over the session are left out. `old.closer.close(Replaced)` comes after `Unloaded`, with the channel already closed, so it is no step of its own. There are no close codes: `"close"` is 4002 or a normal close, `"rejected"` is 4003. The listen row has no status variable of its own: `registered` says its registration is still there, and `Waiting { AuthRejected }` is the dialer's `outcome = "authRejected"`.
- Each action is atomic.

### Not covered

- Checking the Cordis node link (`bridge/link.mjs`) against its code: that code is in the Cordis repository, not here. The model only says its rules should be the same as `dialing.mjs` (500 ms, up to 30 s, reset after a stable 60 s, 30 s after a refusal), with the takeover of §4.4; its code was not run.
- Timing: the backoff steps, the 10 s handshake, the 10 / 30 s heartbeat, the cleanup deadline, Node / Bun's kill deadline. The model only has "some moment" and "finished or killed"; the languages' tests cover these (`node/rutis-runtime/test/serve.test.mjs`, `bun/rutis-bun/test/remote.test.ts`, Kotlin `AppTest`, and others).
- Closing the Swift `.accepted` connection that arrives after a stop: only the `lateAccept` mutation exists (queued at once violates `RuntimeStopped`); the Swift drain code was not checked line by line.
- The runtime's thread pools, the network transport and TLS, real scheduling and memory order.

## Invariants and liveness

| Name | Model | Meaning |
| --- | --- | --- |
| `OneLease` | RemoteLease | At most one active lease on the runtime: the session it serves and the rows not cleaned yet belong to one connection |
| `OldCommandsStayOld` | RemoteLease | A session's commands (`load`) act only on that session's lease; late commands of an old session never act on a new lease |
| `CleanBeforeHello` | RemoteLease | When the runtime is about to answer the new hello, the old lease's rows are all gone (a kill included) |
| `StoppedStaysDown` | RemoteLease | A stopped controller link never dials again and keeps no connection open |
| `ReadyIsCurrent` | RemoteLease | A controller is `Ready` only on its current, open connection: a late hello of a handshake it gave up on never revives it |
| `RuntimeStopped` | RemoteLease | A stopped runtime serves no session, holds no lease, and keeps none of the connections it accepted before the stop (including the one arriving after it) |
| `RestartIsFresh` | RemoteLease | The runtime serves, and is about to serve, only connections of its current round; a controller still `Ready` on an old connection must have had it closed by the runtime |
| `FinalStopStaysDown` | both | The stop of the last round is final: the runtime does not start again, the dialer does not start or dial again |
| `NoHeartbeatBeforeHello` | RemoteLease | A new connection is not closed by its heartbeat before its hello is answered: the heartbeat starts at the answer |
| `OnePluginSetPerId` | AccessPoint | Never two sessions of one id with their far plugins registered at once: the new session registers only after the old one fully unloaded |
| `StoppedRoundsStayDown` | AccessPoint | A stopped round never dials again and keeps no connection of it open; a dialer that started again dials in its new round only |
| `StaysRegistered` | AccessPoint | Revoking an id does not drop the listen row's registration: later connections still reach it |
| `RevokedStaysOut` | AccessPoint | A revoked id never has an installed session again: later connections of it (a revocation during the handshake included) are closed with 4003 after their hello |
| `RestartIsFresh` | AccessPoint | The dialer's `ready` belongs to the current round and the connection whose hello it read; a late hello of an earlier round does not make the new round's connection of the same number `ready` |
| `EventuallyRevokedWaits` | AccessPoint, revocation | After an id is revoked, its dialer eventually waits as an authentication failure (`Waiting { AuthRejected }`) |
| `EventuallyHeld` | both, no faults | Eventually a controller / each id holds the lease |
| `EndsHeld` | both, no faults or bounded faults | `[]<>Held`: in a finite run, "someone holds the lease at the end"; rules out "they replaced each other and nobody got it" |
| `RestartRecovers` | RemoteLease, start/stop | After a stop whose round has a successor, while a controller still has a connection to spend, the runtime eventually starts again and someone holds the lease; a controller out of connections is not demanded |

`NoHeartbeatBeforeHello` holds in the base configurations; the model's `HeartbeatOut` can only happen under the mutation `heartbeatFromAccept` — the base configurations do not model a real heartbeat (see "Simplifications").

## Results

TLC 2.19 (tla2tools v1.7.4), Temurin 21.0.12, 32 cores (`-workers 4`), 2026-11-10. "ok" means the exploration completed and every listed property holds; mutations and before-the-fix configurations stop at the first counterexample, so their state counts depend on the search order.

| Configuration | What | Result | States | Time |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 controllers, 4 paid connections each, no faults; liveness included | ok | 791 | 1 s |
| `lease-faults` | As above, 3 paid connections and 3 transient faults each, links may be disposed | ok | 122,217 | 2 s |
| `lease-takeover` | 1 controller, up to 3 connections (2 takeovers), 2 faults; authentication a step before the old session is closed | ok | 2,256 | 1 s |
| `lease-cleanup-deadline` | 1 controller, 2 connections, 1 fault; the cleanup has a deadline and the old session is killed | ok | 45 | 1 s |
| `lease-app-restart` | 1 controller; the runtime stops once and opens the next round, a start can be asked for while it cleans up; `RestartRecovers` | ok | 1,391 | 1 s |
| `lease-app-restart-3` | 4 rounds, 4 stops (one per round, the last final); no start or bind after it | ok | 8,634 | 1 s |
| `lease-stop-during-takeover` | 2 controllers; the runtime stops during a takeover and opens a new round | ok | 79,521 | 2 s |
| `lease-faults-live` | 2 controllers, 2 paid connections and 2 faults each; `EndsHeld` after faults, no symmetry | ok | 15,314 | 2 s |
| `access-healthy-same` | 2 dialers with the same id, 4 connections each, no faults; liveness included | ok | 16,020 | 2 s |
| `access-healthy-distinct` | Ids of their own; liveness included | ok | 64 | 1 s |
| `access-lose` | Same id, 3 paid connections and 1 fault each | ok | 130,710 | 3 s |
| `access-all` | Same id, 2 paid connections and 1 fault each, dialers may stop | ok | 132,693 | 3 s |
| `access-static-revoke` | 1 dialer, 2 dials; its id is revoked; liveness included | ok | 51 | 1 s |
| `access-faults-live` | Ids of their own, 2 paid connections and 2 faults each; `EndsHeld` after faults | ok | 48,745 | 6 s |
| `access-app-restart` | 1 dialer, 4 rounds, 4 stops (the last final); no dial after it | ok | 66,988 | 2 s |

The table is one complete `run.sh` on this machine: all 32 configurations (with the mutations below) match `expected.txt`, about 30 seconds. No larger sizes are run.

## Counterexamples and doubts in the implementation

TLC found two places where revoking an identity during a handshake still set a session up (a link providing the peer, an access point accepting an id during a takeover); they were fixed (#210). Identity revocation (`Revocations` / `Revocable`, #273) was removed in 0.19, and with it the configurations `lease-before-fix`, `access-before-fix`, `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all` and the property `RevocationScoped`. `RevokedStaysOut` came back with #615 from the tracking table's rules (DIA-5-7, DIA-5-10, DIA-5-11, DIA-5-13) (`access-static-revoke`); it checks the rules, and the current implementation has no revocation hook.

0.19 removes the access point (`access_point.rs`) and registration generations (`registration.rs`); `AccessPoint.tla` was rebuilt from the code of `rutis-host/listen`, and the configurations `access-registration`, `access-mut-route-by-id`, `access-mut-no-gen-check` and the properties `OneInstancePerId`, `OldCallsStayOld`, `NoStaleHandOver` went with them.

The new mutations correspond to #615's doubts; their conclusions:

- `lease-mut-heartbeat-from-accept`: the implementation before the fixes — a connection set `lastSeen` in its constructor (at the accept), so a cleanup longer than the heartbeat timeout closed the new connection with 1001 before its hello. Kotlin #1102 and Go, Swift, Node and Bun #1182 now start at the answer to the hello; the mutation stays as the model's counterexample.
- `lease-mut-late-accept`: a `.accepted` connection arriving after a stop that is queued violates `RuntimeStopped` at once. Kotlin `Listening`'s `drain()` closes them; the Swift side was not checked line by line.
- `lease-mut-start-after-final-stop`, `access-mut-final-stop-dials`, `access-mut-match-by-number`: `RutisApp` starting and stopping several times and matching handshake results by number. f57b4138 fixed the thread interleaving of `RutisApp.kt` (nothing is dialed or bound after a stop returned); the model follows the fixed rules, the mutations are the behaviour before the fix.
- `access-mut-revoke-after-handshake`: a revocation that arrives after the handshake completed but before `Join` is missed by a check that only looks at the snapshot taken at the handshake's completion (DIA-5-10 before its fix). The current implementation has no revocation hook; the configuration checks the rule.
- Node / Bun's kill deadline: f57b4138 changes it to `RUTIS_SHUTDOWN_TIMEOUT + 2 s` (`shutdownDeadline() + KILL_MARGIN` in `serve.mjs` / `remote.ts`), tested by `serve.test.mjs` and `remote.test.ts`. The model does not model the timer, see "Simplifications".

TLC found no counterexample to the model otherwise: the §4.4 takeover order (the authentication step included), listen's replacement order, cancelling the retry on stop, a cleanup killed at the deadline leaving no old lease, the new round after an app-host stop never serving old connections, and the lease landing on someone once the fault budget is spent all agree with the model. The new configurations were not run against the implementation; the Swift / Kotlin / app-hosting order was read from the code (see the table above).

## Mutations: the model is not vacuous

Each main invariant and liveness property has a deliberately introduced bug that TLC must catch (`violated:<name>` in `expected.txt`; `violated:liveness` for liveness):

| Configuration | Mutation | Property violated |
| --- | --- | --- |
| `lease-mut-start-before-clean` | The runtime answers the new hello before cleaning the old lease (§4.4 ③ and ④ swapped) | `OneLease` |
| `lease-mut-read-old` | The runtime keeps reading a replaced session and applies its commands to the current lease | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | Stopping a link that waits to reconnect does not cancel the retry | `StoppedStaysDown` |
| `lease-mut-late-hello` | A late hello of a handshake given up on makes the link `Ready` | `ReadyIsCurrent` |
| `lease-mut-wait-for-old` | A takeover stops reading the old session but does not close it, waiting for the old controller to leave | liveness (`EndsHeld`) |
| `lease-mut-kill-leaves-rows` | The cleanup's deadline passes and the old session is killed, but its lease's rows are left behind | `OneLease` |
| `lease-mut-stale-round` | Stopping the runtime keeps the connections it accepted but had not served; the new round serves them | `RestartIsFresh` |
| `lease-mut-start-after-final-stop` | A start after the stop of the last round opens another round | `FinalStopStaysDown` |
| `lease-mut-heartbeat-from-accept` | The new connection's heartbeat starts at the accept; it is closed with 1001 while the cleanup runs | `NoHeartbeatBeforeHello` |
| `lease-mut-late-accept` | A connection arriving after the stop is queued | `RuntimeStopped` |
| `access-mut-accept-before-release` | A replacement installs the new session before unloading the old one (§4.2 step 3 moved before step 1) | `OnePluginSetPerId` |
| `access-mut-stop-keeps-retry` | Stopping a dialer that waits to dial again does not cancel it | `StoppedRoundsStayDown` |
| `access-mut-revoke-installs` | A connection of a revoked id is installed when its handshake completes | `RevokedStaysOut` |
| `access-mut-revoke-unregisters` | Revoking an id also drops the listen row's registration, so later dials are refused at the upgrade | `StaysRegistered` |
| `access-mut-revoke-after-handshake` | Only the revocation snapshot of the handshake's completion is checked, so a connection revoked while waiting to be taken over is installed | `RevokedStaysOut` |
| `access-mut-match-by-number` | A handshake result is matched by connection number, not by round | `RestartIsFresh` |
| `access-mut-final-stop-dials` | A start after the stop of the last round dials again | `FinalStopStaysDown` |

Each mutation's counterexample is found within 2 seconds (tens to some hundreds of states).

## Running it

Needs Java 11+ and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh                     # every configuration
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh lease-faults        # one
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh       # limit TLC's threads
```

`run.sh` picks the model by name (`lease-*` → `RemoteLease.tla`, `access-*` → `AccessPoint.tla`), compares each configuration's outcome with `expected.txt`, and exits 0 when all match; logs and counterexamples go to `TLC_OUT` (a temporary directory by default). CI's `specs` job runs it on pull requests that change `specs/**` and in the nightly full run ([CI](../../docs/ci.en.md) §4).

## Maintenance

When remote plugins §3.2, §4.4, dial-out §5, §6 or remote session §4.2, §4.3 change, or `connect/dial.rs`, `listen.rs` (`Listening::run`), a language's listening takeover order and cleanup deadline, `RutisApp.start/stop`, or the dialers' reconnection rules change, update this model and `expected.txt` with them (standard Q11.2.3). The static listen link's revocation has no implementation; change `access-static-revoke` and `access-mut-revoke-after-handshake` when the tracking table's DIA-5-7 / 5-10 / 5-11 / 5-13 change. When a timing rule changes (the kill deadline, the heartbeat's starting point), change the "Simplifications" section and the tests; the model keeps only the order rules.
