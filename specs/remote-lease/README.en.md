# Remote leases: takeover and reconnection (TLA+)

[中文](README.md)

TLC explores every interleaving of a remote runtime's lease, takeover by a newer connection, and a link reconnecting and stopping. Issue #210; quality standard Q11.2, Q6.13.3; status §7.2 (risks E4, E5). Two models:

- `RemoteLease.tla`: a listening remote runtime and the two controller links that dial it (the scope of the issue);
- `AccessPoint.tla`: far ends dialing a `rutis-host/listen` row: handshakes side by side, one session per id, a newer connection of the same id replacing the older, and the dialers' reconnection rules.

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Controller (a `rutis-host/dial` row): dial, back off and reconnect, wait by failure category, stop | [remote plugins](../../docs/design-remote-plugins-2026-10-03.en.md) §3.2; [dial-out](../../docs/design-remote-runtime-dial-2026-10-14.en.md) §6; [remote session](../../docs/design-remote-session-2026-10-28.en.md) §4.1 | `crates/rutis-host/src/connect/dial.rs`: dial, handshake, mount the session, wait for it to unload, back off |
| The runtime's lease and takeover order (authenticate → close the old session → clean the old lease → answer the new hello) | remote plugins §4.4 | `python/rutis/rutis/__main__.py` `serve`; `go/rutis/internal/runtime/listen.go` `listen`; `node/rutis-runtime/src/serve.mjs` `serve` (Bun's `src/remote.ts` alike) |
| listen: handshakes side by side, replacement by the same id (old session fully unloaded → old channel closed with 4002 → new session installed) | [remote session](../../docs/design-remote-session-2026-10-28.en.md) §4.2 | `crates/rutis-host/src/connect/listen.rs`: `Listening::run`, `Replaceable` |
| The dialer's reconnection rules | dial-out §6 | `python/rutis/rutis/dialing.py` `keep_dialing` (Go, Node and Bun each have the same rules) |

## The models

Common to both: each direction of a connection is delivered in order, with any delay (more general than the issue's "delayed by at most 2 steps"); a side that closed a connection reads nothing more on it, and frames to it are dropped (kept only where a mutation reads them). The environment's actions (stop, timeout, lost connection) may happen at any moment; the configuration says which are on.

### `RemoteLease.tla`

- **Participants**: controllers `c1`, `c2` (rutis `rutis-host/dial` rows) and one listening remote runtime. Connection `<<c, n>>` is `c`'s n-th dial.
- **Controller** (`connect/dial.rs`): once a dial succeeds the upgrade is done and the runtime's listener queues the connection; the row waits for the runtime's hello (the handshake), then mounts the session and is `Ready`; the loader then loads rows through the session (`load` frames). A session that ends (replaced, 4002, or ended by the runtime) is retryable: back off, reconnect. Stopping (dispose) aborts the task: the session closes, the retry is cancelled. Environment: failed dials, handshake timeout or lost connection (`Lose`), stop.
- **Runtime** (Python `serve` and the others): one controller at a time. Taking the next connection while a session runs is three steps (§4.4): ① close the old session's channel (4002) and read none of its frames; ② clean the old lease (every row unloaded); ③ start the new session and answer its hello. When a session's far end closes, its lease is cleaned.
- **Reconnects**: each controller dials at most 4 times (3 reconnects).

### `AccessPoint.tla`

- **Participants**: far ends `d1`, `d2` dialing out, greeting with the same id (`SameId`) or ids of their own; on the rutis side one `rutis-host/listen` row.
- **Dialer** (`dialing.py`): a dial delivers the connection to listen and starts its handshake. rutis' hello means "connected"; a closed connection (4002, lost, a failed handshake) is waited out, then dialed again. A stopped dialer dials no more.
- **listen** (`Listening::run`):
  - Handshakes run side by side in a `JoinSet`: `HandshakeDone` (answers hello; none of the far end's plugins is registered yet) or `HandshakeFails` (closes that connection and touches nothing else).
  - `Join`: the loop takes a connection whose handshake is done. No session of its id: install it. Otherwise mark the old connection's closer replaced, take it out of `live`, start unloading the old session; the loop then sits on `unload().await` and does nothing else.
  - `Replace`: once the old session is unloaded (`Unloaded`: every far plugin unloaded, the channel closed last, as 4002 since it is marked replaced), install the new session.
  - `Register`: an installed session registers the far end's plugins.
  - `EndSelf`, `Unloaded`, `ReadEnding`: a session ending on its own (far end closed, or lost) unloads; when the loop reads its ending, it forgets it only if `live` still holds it (same serial).
- **Reconnects**: each dialer dials at most 4 times (3 reconnects); 3 in the configuration with lost connections (`lose`), 2 in the one with every fault, see "Results".

## Simplifications

- No time: backoff, the 30 s wait, the 10 s handshake limit and "a session lasting 60 s resets the backoff" are all just "some later moment". A timeout is a `Lose` that may happen at any moment, more general than real timers.
- The bounded number of reconnects is the model's bound, not the implementation's behaviour. Liveness is checked only in runs without faults; there every behaviour is finite (each dial uses up budget), so `WF_vars(Next)` makes every enabled step happen. Two controllers configured for the same runtime (or two dialers with the same id) keep replacing each other (written in dial-out §6); in the model the last one to connect holds the lease once the budget is spent, in the implementation backoff slows the alternation to about once every 30 s each.
- `RemoteLease`: the controller's hello is left out (the runtime reads it when it starts the session; it has no other effect).
- `AccessPoint`: token routing (`Router`) is left out: a dial reaches the row. The far plugins are modelled only as "all registered" and "all unloaded", the reverse-order unload one step; calls over the session are left out. `old.closer.close(Replaced)` comes after `Unloaded`, with the channel already closed, so it is no step of its own.
- Each action is atomic.

## Invariants and liveness

| Name | Model | Meaning |
| --- | --- | --- |
| `OneLease` | RemoteLease | At most one active lease on the runtime: the session it serves and the rows not cleaned yet belong to one connection |
| `OldCommandsStayOld` | RemoteLease | A session's commands (`load`) act only on that session's lease; late commands of an old session never act on a new lease |
| `StoppedStaysDown` | both | A stopped link / dialer never dials again and keeps no connection open |
| `ReadyIsCurrent` | RemoteLease | A controller is `Ready` only on its current, open connection: a late hello of a handshake it gave up on never revives it |
| `OnePluginSetPerId` | AccessPoint | Never two sessions of one id with their far plugins registered at once: the new session registers only after the old one fully unloaded |
| `EventuallyHeld` | both, no faults | Eventually a controller holds the lease (`RemoteLease`: a controller is `Ready` and the runtime serves its connection; `AccessPoint`: each id has an installed session with its far plugins registered, and its dialer is connected) |
| `EndsHeld` | both, no faults | `[]<>Held`: in a finite run, "someone holds the lease at the end"; rules out "they replaced each other and nobody got it" |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 17, 16 cores (`-workers auto`). "ok" means the exploration completed and every listed property holds; the state counts of these rows are deterministic.

| Configuration | What | Result | States | Time |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 controllers, 4 dials each, no faults; liveness included | ok | 791 | 1 s |
| `lease-faults` | As above; stop, failed dials, timeouts / lost connections at any moment | ok | 1,050,999 | 12 s |
| `access-healthy-same` | 2 dialers with the same id, 4 dials each, no faults; liveness included | ok | 16,020 | 4 s |
| `access-healthy-distinct` | Ids of their own; liveness included | ok | 64 | 2 s |
| `access-lose` | Same id, 3 dials each; failed dials and handshakes, lost connections, sessions ending | ok | 6,350,873 | 48 s |
| `access-all` | Same id, 2 dials each; every fault (lost, stopped) | ok | 129,131 | 3 s |

All 13 configurations (with the mutations below) take under 2 minutes.

Configurations with lost connections are limited in dials: 3 each with lost connections only, 2 with lost connections and stops together (3 gives 10.44 million states, 76 s).

## Counterexamples in the implementation (fixed)

TLC found two places where revoking an identity during a handshake still set a session up (a link providing the peer, an access point accepting an id during a takeover); they were fixed (#210). Identity revocation (`Revocations` / `Revocable`, #273) was removed in 0.19, and with it the configurations `lease-before-fix`, `access-before-fix`, `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all` and the properties `RevokedStaysOut`, `RevocationScoped`.

0.19 removes the access point (`access_point.rs`) and registration generations (`registration.rs`); `AccessPoint.tla` was rebuilt from the code of `rutis-host/listen`, and the configurations `access-registration`, `access-mut-route-by-id`, `access-mut-no-gen-check` and the properties `OneInstancePerId`, `OldCallsStayOld`, `NoStaleHandOver` went with them.

TLC found no counterexample to the implementation otherwise: the §4.4 takeover order, listen's replacement order and cancelling the retry on stop all agree with the model.

## Mutations: the model is not vacuous

Each invariant and liveness property has a deliberately introduced bug that TLC must catch (`violated:<name>` in `expected.txt`; `violated:liveness` for liveness):

| Configuration | Mutation | Property violated |
| --- | --- | --- |
| `lease-mut-start-before-clean` | The runtime answers the new hello before cleaning the old lease (§4.4 ② and ③ swapped) | `OneLease` |
| `lease-mut-read-old` | The runtime keeps reading a replaced session and applies its commands to the current lease (one shared row table) | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | Stopping a link that waits to reconnect does not cancel the retry | `StoppedStaysDown` |
| `lease-mut-late-hello` | A late hello of a handshake given up on makes the link `Ready` | `ReadyIsCurrent` |
| `lease-mut-wait-for-old` | A takeover stops reading the old session but does not close it, waiting for the old controller to leave (no faults) | liveness (`EndsHeld`) |
| `access-mut-accept-before-release` | A replacement installs the new session before unloading the old one (§4.2 step 3 moved before step 1) | `OnePluginSetPerId` |
| `access-mut-stop-keeps-retry` | Stopping a dialer that waits to dial again does not cancel it | `StoppedStaysDown` |

Each mutation's counterexample is found within 2 seconds (tens to some thousands of states).

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

When remote plugins §3.2, §4.4, dial-out §6 or remote session §4.2 change, or `connect/dial.rs`, `listen.rs` (`Listening::run`), a language's listening takeover order or the dialers' reconnection rules change, update this model and `expected.txt` with them (standard Q11.2.3).
