# Remote leases: takeover and reconnection (TLA+)

[中文](README.md)

TLC explores every interleaving of a remote runtime's lease, takeover by a newer connection, a link reconnecting and stopping, and registration revocation. Issue #210; quality standard Q11.2, Q6.13.3; status §7.2 (risks E4, E5). Two models:

- `RemoteLease.tla`: a listening remote runtime and the two controller links that dial it (the scope of the issue);
- `AccessPoint.tla`: runtimes dialing out to a rutis access point (what landed after the issue): the listener's registration and its generation, hand-over, admission of a self-declared id, takeover by the same id, the access point stopping and starting again.

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Controller link: dial, back off and reconnect, wait by failure category, stop | [remote plugins](../../docs/design-remote-plugins-2026-10-03.en.md) §3.2; [dial-out](../../docs/design-remote-runtime-dial-2026-10-14.en.md) §6 | `crates/rutis-bridge/src/link.rs`: `dialing`, `start` |
| The runtime's lease and takeover order (authenticate → close the old session → clean the old lease → answer the new hello) | remote plugins §4.4 | `python/rutis/rutis/__main__.py` `serve`; `go/rutis/listen.go` `listen`; `node/rutis-runtime/src/serve.mjs` `serve` (Bun's `src/remote.ts` alike) |
| Registrations, generations, the re-check at hand-over | remote plugins §3.2 | `crates/rutis-bridge/src/registration.rs`: `route`, `hand_over` |
| Access point: a connection is an instance, takeover by the same id, the old connection released before the new one is accepted | dial-out §4, §6 | `crates/rutis-bridge/src/access_point.rs`: `Shared::connection`, slots per id |
| Identity layer | dial-out §5 | `crates/rutis-bridge/src/identity.rs`: `SharedToken` |
| The dialer's reconnection rules | dial-out §6 | `python/rutis/rutis/dialing.py` `keep_dialing` (Go, Node and Bun each have the same rules) |

## The models

Common to both: each direction of a connection is delivered in order, with any delay (more general than the issue's "delayed by at most 2 steps"); a side that closed a connection reads nothing more on it, and frames to it are dropped (kept only where a mutation reads them). The environment's actions (stop, timeout, lost connection, the access point stopping) may happen at any moment; the configuration says which are on.

### `RemoteLease.tla`

- **Participants**: controllers `c1`, `c2` (rutis dialing links) and one listening remote runtime. Connection `<<c, n>>` is `c`'s n-th dial.
- **Controller** (`link.rs` `dialing`): once a dial succeeds the upgrade is done and the runtime's listener queues the connection; the link waits for the runtime's hello (`start`), then provides the peer and is `Ready`; the loader then loads rows through the peer (`load` frames). A session that ends (replaced, 4002, or ended by the runtime) is retryable: back off, reconnect. Stopping (dispose) aborts the task: the session closes, the retry is cancelled. Environment: failed dials, handshake timeout or lost connection (`Lose`), stop.
- **Runtime** (Python `serve` and the others): one controller at a time. Taking the next connection while a session runs is three steps (§4.4): ① close the old session's channel (4002) and read none of its frames; ② clean the old lease (every row unloaded); ③ start the new session and answer its hello. When a session's far end closes, its lease is cleaned.
- **Reconnects**: each controller dials at most 4 times (3 reconnects).

### `AccessPoint.tla`

- **Participants**: runtimes `d1`, `d2` dialing out, greeting with the same id (`SameId`) or ids of their own; on the rutis side the transport's registrations, the access point and the admission (shared token plus self-declared id).
- **Dialer** (`dialing.py`): the transport routes the dial by its credential (`route`): nothing registered on the listener is retryable; otherwise the connection gets a ticket of the current registration generation. rutis' hello means "connected"; a closed connection (4002, lost) is waited out, then dialed again. A stopped dialer dials no more.
- **rutis**:
  - `HandOver`: under the registry lock, hand over only if the ticket's generation is still the current registration's, otherwise close; the access point reads the hello and the admission admits the id.
  - `TakeSlot`: lock the id's slot (takeovers of one id run one at a time); a connection that arrived before the current one is superseded (4002); one whose access point stopped is closed; otherwise the current connection is closed (4002) and released, the old instance going first.
  - `Accept`: create the instance (`Accept::accept`), unlock the slot.
  - `EndLive`: an instance whose session closed or whose access point stopped ends, under the slot lock, if it is still current.
  - Environment: the access point stops (registration revoked) and registers again with a new generation.
- **Reconnects**: each dialer dials at most 4 times (3 reconnects); configurations with lost connections (`lose`) 3 times (2 reconnects), see "Results".

## Simplifications

- No time: backoff, the 30 s wait, the 10 s handshake limit and "a session lasting 60 s resets the backoff" are all just "some later moment". A timeout is a `Lose` that may happen at any moment, more general than real timers.
- The bounded number of reconnects is the model's bound, not the implementation's behaviour. Liveness is checked only in runs without faults; there every behaviour is finite (each dial uses up budget), so `WF_vars(Next)` makes every enabled step happen. Two controllers configured for the same runtime (or two dialers with the same id) keep replacing each other (written in dial-out §6, and in the comments of `leases.rs`); in the model the last one to connect holds the lease once the budget is spent, in the implementation backoff slows the alternation to about once every 30 s each.
- `RemoteLease`: the controller's hello is left out (the runtime reads it when it starts the session; it has no other effect).
- `AccessPoint`: hand-over, reading the hello and the admission are one step (the access point stopping in between is the same as stopping just after, which `TakeSlot` handles); closing and releasing the old connection are one step. Rows are not modelled (the runtime side's lease is `RemoteLease`'s), only the calls a dialer makes to rutis on its session.
- A static link listening (rutis listens, the far end dials in, one registration per far end) is not modelled on its own: it is the sequential special case of the access point, without self-declared ids and concurrent slots.
- Each action is atomic.

## Invariants and liveness

| Name | Model | Meaning |
| --- | --- | --- |
| `OneLease` | RemoteLease | At most one active lease on the runtime: the session it serves and the rows not cleaned yet belong to one connection |
| `OldCommandsStayOld` | RemoteLease | A session's commands (`load`) act only on that session's lease; late commands of an old session never act on a new lease |
| `StoppedStaysDown` | both | A stopped link / dialer never dials again and keeps no connection open |
| `ReadyIsCurrent` | RemoteLease | A controller is `Ready` only on its current, open connection: a late hello of a handshake it gave up on never revives it |
| `OneInstancePerId` | AccessPoint | At most one instance per connection id (the access point's "lease") |
| `OldCallsStayOld` | AccessPoint | A call from a session acts only on that session's instance |
| `NoStaleHandOver` | AccessPoint | A handshake routed to a registration revoked since (the access point stopped, or stopped and registered again with a new generation) never completes hand-over |
| `EventuallyHeld` | both, no faults | Eventually a controller holds the lease (`RemoteLease`: a controller is `Ready` and the runtime serves its connection; `AccessPoint`: each id has an instance and its runtime is connected) |
| `EndsHeld` | both, no faults | `[]<>Held`: in a finite run, "someone holds the lease at the end"; rules out "they replaced each other and nobody got it" |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 17, 16 cores (`-workers auto`). "ok" means the exploration completed and every listed property holds; the state counts of these rows are deterministic.

| Configuration | What | Result | States | Time |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 controllers, 4 dials each, no faults; liveness included | ok | 791 | 1 s |
| `lease-faults` | As above; stop, failed dials, timeouts / lost connections at any moment | ok | 1,050,999 | 10 s |
| `access-healthy-same` | 2 dialers with the same id, 4 dials each, no faults; liveness included | ok | 24,982 | 1 s |
| `access-healthy-distinct` | Ids of their own; liveness included | ok | 244 | 1 s |
| `access-registration` | Same id, 4 dials each; the access point stops and starts again (2 registration generations) | ok | 620,591 | 6 s |
| `access-lose` | Same id, 3 dials each; timeouts / lost connections | ok | 1,168,625 | 10 s |
| `access-all` | Same id, 2 dials each; every fault | ok | 288,228 | 3 s |

All 16 configurations (with the mutations below) take under a minute.

The access point's configurations with lost connections are limited to 3 dials each: with 4 (lost connections only) the state count passed 81 million within 10 minutes and was still growing. So the faults run in separate configurations (registration, lost connections), and the configuration with every fault at a smaller scale.

## Counterexamples in the implementation (fixed)

TLC found two places where revoking an identity during a handshake still set a session up (a link providing the peer, an access point accepting an id during a takeover); they were fixed (#210). Identity revocation (`Revocations` / `Revocable`, #273) was removed in 0.19, and with it the configurations `lease-before-fix`, `access-before-fix`, `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all` and the properties `RevokedStaysOut`, `RevocationScoped`.

TLC found no counterexample to the implementation otherwise: the §4.4 takeover order, the generation re-check, reading frames by connection (not by id) and cancelling the retry on stop all agree with the model.

## Mutations: the model is not vacuous

Each invariant and liveness property has a deliberately introduced bug that TLC must catch (`violated:<name>` in `expected.txt`; `violated:liveness` for liveness):

| Configuration | Mutation | Property violated |
| --- | --- | --- |
| `lease-mut-start-before-clean` | The runtime answers the new hello before cleaning the old lease (§4.4 ② and ③ swapped) | `OneLease` |
| `lease-mut-read-old` | The runtime keeps reading a replaced session and applies its commands to the current lease (one shared row table) | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | Stopping a link that waits to reconnect does not cancel the retry | `StoppedStaysDown` |
| `lease-mut-late-hello` | A late hello of a handshake given up on makes the link `Ready` | `ReadyIsCurrent` |
| `lease-mut-wait-for-old` | A takeover stops reading the old session but does not close it, waiting for the old controller to leave (no faults) | liveness (`EndsHeld`) |
| `access-mut-accept-before-release` | A takeover accepts the new connection before releasing the old one | `OneInstancePerId` |
| `access-mut-route-by-id` | A replaced session is still read and its calls go to the current instance of its id | `OldCallsStayOld` |
| `access-mut-no-gen-check` | Hand-over does not compare the registration generation | `NoStaleHandOver` |
| `access-mut-stop-keeps-retry` | Stopping a dialer that waits to dial again does not cancel it | `StoppedStaysDown` |

Each mutation's counterexample is found in about a second (tens to some ten thousand states).

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

When remote plugins §3.2, §4.4 or dial-out §6 change, or `link.rs` (`dialing`, `listening`, `start`), `registration.rs` (`hand_over`), `access_point.rs` (`Shared::connection`), a language's listening takeover order or the dialers' reconnection rules change, update this model and `expected.txt` with them (standard Q11.2.3).
