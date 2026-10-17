# Remote leases: takeover, reconnection and revocation (TLA+)

[中文](README.md)

TLC explores every interleaving of a remote runtime's lease, takeover by a newer connection, a link reconnecting and stopping, registration revocation and identity revocation. Issue #210; quality standard Q11.2, Q6.13.3; status §7.2 (risks E4, E5). Two models:

- `RemoteLease.tla`: a listening remote runtime and the two controller links that dial it (the scope of the issue);
- `AccessPoint.tla`: runtimes dialing out to a rutis access point (what landed after the issue): the listener's registration and its generation, hand-over, admission of a self-declared id, takeover by the same id, the access point stopping and starting again, revoking one id or the shared token.

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Controller link: dial, back off and reconnect, wait by failure category, stop, a revoked far end | [remote plugins](../../docs/design-remote-plugins-2026-10-03.en.md) §3.2; [dial-out](../../docs/design-remote-runtime-dial-2026-10-14.en.md) §5 (revocation hook, #273), §6 | `crates/rutis-bridge/src/link.rs`: `dialing`, `start`, `cut` |
| The runtime's lease and takeover order (authenticate → close the old session → clean the old lease → answer the new hello) | remote plugins §4.4 | `python/rutis/rutis/__main__.py` `serve`; `go/rutis/listen.go` `listen`; `node/rutis-runtime/src/serve.mjs` `serve` (Bun's `src/remote.ts` alike) |
| Registrations, generations, the re-check at hand-over | remote plugins §3.2 | `crates/rutis-bridge/src/registration.rs`: `route`, `hand_over` |
| Access point: a connection is an instance, takeover by the same id, the old connection released before the new one is accepted | dial-out §4, §6 | `crates/rutis-bridge/src/access_point.rs`: `Shared::connection`, slots per id |
| Identity layer and revocation | dial-out §5 | `crates/rutis-bridge/src/identity.rs`: `Revocable`, `Revocations`, `SharedToken` |
| The dialer's reconnection rules | dial-out §6 | `python/rutis/rutis/dialing.py` `keep_dialing` (Go, Node and Bun each have the same rules) |

## The models

Common to both: each direction of a connection is delivered in order, with any delay (more general than the issue's "delayed by at most 2 steps"); a side that closed a connection reads nothing more on it, and frames to it are dropped (kept only where a mutation reads them). The environment's actions (revocation, stop, timeout, lost connection, the access point stopping) may happen at any moment; the configuration says which are on.

### `RemoteLease.tla`

- **Participants**: controllers `c1`, `c2` (rutis dialing links, each with an identity that can revoke the far end) and one listening remote runtime. Connection `<<c, n>>` is `c`'s n-th dial.
- **Controller** (`link.rs` `dialing`): at the top of the loop, a revoked far end is not dialed and is waited out as an authentication failure; once a dial succeeds the upgrade is done and the runtime's listener queues the connection; the link waits for the runtime's hello (`start`), then provides the peer and is `Ready`; the loader then loads rows through the peer (`load` frames). A session that ends (replaced, 4002, or ended by the runtime) is retryable: back off, reconnect. On revocation `cut` closes the session and the link waits as after an authentication failure. Stopping (dispose) aborts the task: the session closes, the retry is cancelled. Environment: failed dials, handshake timeout or lost connection (`Lose`), revocation, stop.
- **Runtime** (Python `serve` and the others): one controller at a time. Taking the next connection while a session runs is three steps (§4.4): ① close the old session's channel (4002) and read none of its frames; ② clean the old lease (every row unloaded); ③ start the new session and answer its hello. When a session's far end closes, its lease is cleaned.
- **Reconnects**: each controller dials at most 4 times (3 reconnects).

### `AccessPoint.tla`

- **Participants**: runtimes `d1`, `d2` dialing out, greeting with the same id (`SameId`) or ids of their own; on the rutis side the transport's registrations, the access point and the admission (shared token plus self-declared id, revocable).
- **Dialer** (`dialing.py`): the transport routes the dial by its credential (`route`): nothing registered on the listener is retryable; a revoked shared token gets 403; otherwise the connection gets a ticket of the current registration generation. rutis' hello means "connected"; a closed connection (4002, 4003, lost) is waited out, then dialed again. A stopped dialer dials no more.
- **rutis**:
  - `HandOver`: under the registry lock, hand over only if the ticket's generation is still the current registration's, otherwise close; the access point reads the hello and the admission checks the id (a revoked one is rejected, 4003).
  - `TakeSlot`: lock the id's slot (takeovers of one id run one at a time); a connection that arrived before the current one is superseded (4002); one whose access point stopped is closed; otherwise the current connection is closed (4002) and released, the old instance going first.
  - `Accept`: create the instance (`Accept::accept`), unlock the slot.
  - `EndLive`: an instance whose session closed, whose access point stopped, or whose id was revoked ends, under the slot lock, if it is still current.
  - Environment: the access point stops (registration revoked) and registers again with a new generation; one id revoked; everything revoked (the shared token withdrawn).
- **Reconnects**: each dialer dials at most 4 times (3 reconnects); configurations with lost connections (`lose`) 3 times (2 reconnects), see "Results".

## Simplifications

- No time: backoff, the 30 s wait, the 10 s handshake limit and "a session lasting 60 s resets the backoff" are all just "some later moment". A timeout is a `Lose` that may happen at any moment, more general than real timers.
- The bounded number of reconnects is the model's bound, not the implementation's behaviour. Liveness is checked only in runs without faults; there every behaviour is finite (each dial uses up budget), so `WF_vars(Next)` makes every enabled step happen. Two controllers configured for the same runtime (or two dialers with the same id) keep replacing each other (written in dial-out §6, and in the comments of `leases.rs`); in the model the last one to connect holds the lease once the budget is spent, in the implementation backoff slows the alternation to about once every 30 s each.
- `RemoteLease`: the controller's hello is left out (the runtime reads it when it starts the session; it has no other effect); checking for revocation and dialing are one step (revoked in between, `Revocable::credential` presents nothing and the runtime answers 401: no connection either).
- `AccessPoint`: hand-over, reading the hello and the admission are one step (the access point stopping in between is the same as stopping just after, which `TakeSlot` handles); rutis' hello goes out after the admission (the implementation sends it when the session opens, so a rejected dialer first sees "connected" and then 4003; no invariant depends on it); closing and releasing the old connection are one step. Rows are not modelled (the runtime side's lease is `RemoteLease`'s), only the calls a dialer makes to rutis on its session.
- A static link listening (rutis listens, the far end dials in, one registration per far end) is not modelled on its own: it is the sequential special case of the access point, without self-declared ids and concurrent slots. Its fix and test are under "Counterexamples".
- Each action is atomic. A revocation that lands between "check for revocation" and "provide the peer / create the instance" (a few lines without an await, or while `Accept::accept` runs) counts as a revocation just after: the session just set up is then closed by the revocation, as any running session is.

## Invariants and liveness

| Name | Model | Meaning |
| --- | --- | --- |
| `OneLease` | RemoteLease | At most one active lease on the runtime: the session it serves and the rows not cleaned yet belong to one connection |
| `OldCommandsStayOld` | RemoteLease | A session's commands (`load`) act only on that session's lease; late commands of an old session never act on a new lease |
| `StoppedStaysDown` | both | A stopped link / dialer never dials again and keeps no connection open |
| `ReadyIsCurrent` | RemoteLease | A controller is `Ready` only on its current, open connection: a late hello of a handshake it gave up on never revives it |
| `RevokedStaysOut` | both | Once an identity is revoked no session is set up for it again: the controller is never `Ready` again (no peer provided), the access point creates no instance for that id |
| `OneInstancePerId` | AccessPoint | At most one instance per connection id (the access point's "lease") |
| `OldCallsStayOld` | AccessPoint | A call from a session acts only on that session's instance |
| `NoStaleHandOver` | AccessPoint | A handshake routed to a registration revoked since (the access point stopped, or stopped and registered again with a new generation) never completes hand-over |
| `RevocationScoped` | AccessPoint | A connection ended for a revocation only if its own id was revoked (revoking one id closes only that id's connections) |
| `EventuallyHeld` | both, no faults | Eventually a controller holds the lease (`RemoteLease`: a controller is `Ready` and the runtime serves its connection; `AccessPoint`: each id has an instance and its runtime is connected) |
| `EndsHeld` | both, no faults | `[]<>Held`: in a finite run, "someone holds the lease at the end"; rules out "they replaced each other and nobody got it" |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 17, 16 cores (`-workers auto`). "ok" means the exploration completed and every listed property holds; the state counts of these rows are deterministic.

| Configuration | What | Result | States | Time |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 controllers, 4 dials each, no faults; liveness included | ok | 791 | 1 s |
| `lease-faults` | As above; revocation, stop, failed dials, timeouts / lost connections at any moment | ok | 4,206,661 | 25 s |
| `access-healthy-same` | 2 dialers with the same id, 4 dials each, no faults; liveness included | ok | 24,982 | 1 s |
| `access-healthy-distinct` | Ids of their own; liveness included | ok | 244 | 1 s |
| `access-revoke-same` | Same id, 4 dials each; the id or token revoked, a dialer stopped | ok | 324,889 | 3 s |
| `access-revoke-distinct` | Ids of their own; as above | ok | 27,578 | 1 s |
| `access-registration` | Same id, 4 dials each; the access point stops and starts again (2 registration generations), revocation | ok | 3,109,267 | 18 s |
| `access-lose` | Same id, 3 dials each; timeouts / lost connections, revocation | ok | 4,951,529 | 40 s |
| `access-all` | Same id, 2 dials each; every fault | ok | 1,171,884 | 9 s |

All 21 configurations (with the counterexamples and mutations below) take about 2 minutes; about 3 with `TLC_WORKERS=4`.

The access point's configurations with lost connections are limited to 3 dials each: with 4 (lost connections only) the state count passed 81 million within 10 minutes and was still growing. So the faults run in separate configurations (revocation, registration, lost connections), and the configuration with every fault at a smaller scale.

## Counterexamples in the implementation (fixed)

`lease-before-fix` and `access-before-fix` model the implementation before the fix (`RevokeCheck = FALSE`); TLC finds counterexamples to `RevokedStaysOut`:

1. **Controller link: revoked during the handshake, the peer still provided** (`lease-before-fix`, 5 steps). The controller dials; the runtime starts the session and sends its hello; the controller's identity revokes the runtime; the hello arrives, `start` provides the peer and the link is `Ready`, and only then does `revoked()` in the `select!` cut the session. In between, plugins that depend on the peer (`RuntimePlugin`, the loader) may already mount and load rows, sending the evaluated configuration (possibly with secrets) to a revoked far end. A listening link likewise: `start` provided the peer, then checked `is_revoked()` and cut.
2. **Access point: an id revoked during a takeover still accepted** (`access-before-fix`). A newer connection of an id has passed admission at its hello and waits for the slot or for the old connection's release; the id is revoked; once the release is done `Accept::accept` runs as usual (creating the instance, mounting the runtime), and only then does the revocation watch cut it.

The fix (a commit of its own): `link.rs` `start` checks for revocation after the hello and before providing the peer; a revoked far end's session is closed (a listening link tells the dialer with 4003) and the link waits as after an authentication failure. `access_point.rs` checks after the takeover and before `Accept::accept`; a revoked id is rejected with 4003 and `AccessEvent::Rejected`. The model of the fixed code (every other configuration, `RevokeCheck = TRUE`) passes. Tests: `crates/rutis-bridge/tests/revocation.rs` `a_far_end_revoked_while_it_greets_never_becomes_a_peer` and `an_id_revoked_while_its_connection_takes_over_is_not_accepted`, which fail before the fix (the first 8 times out of 8, the second deterministically).

TLC found no other counterexample to the implementation: the §4.4 takeover order, the generation re-check, reading frames by connection (not by id) and cancelling the retry on stop all agree with the model.

## Mutations: the model is not vacuous

Each invariant and liveness property has a deliberately introduced bug that TLC must catch (`violated:<name>` in `expected.txt`; `violated:liveness` for liveness):

| Configuration | Mutation | Property violated |
| --- | --- | --- |
| `lease-mut-start-before-clean` | The runtime answers the new hello before cleaning the old lease (§4.4 ② and ③ swapped) | `OneLease` |
| `lease-mut-read-old` | The runtime keeps reading a replaced session and applies its commands to the current lease (one shared row table) | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | Stopping a link that waits to reconnect does not cancel the retry | `StoppedStaysDown` |
| `lease-mut-late-hello` | A late hello of a handshake given up on makes the link `Ready` | `ReadyIsCurrent` |
| `lease-before-fix` | The link before the fix (above) | `RevokedStaysOut` |
| `lease-mut-wait-for-old` | A takeover stops reading the old session but does not close it, waiting for the old controller to leave (no faults) | liveness (`EndsHeld`) |
| `access-mut-accept-before-release` | A takeover accepts the new connection before releasing the old one | `OneInstancePerId` |
| `access-mut-route-by-id` | A replaced session is still read and its calls go to the current instance of its id | `OldCallsStayOld` |
| `access-mut-no-gen-check` | Hand-over does not compare the registration generation | `NoStaleHandOver` |
| `access-before-fix` | The access point before the fix (above) | `RevokedStaysOut` |
| `access-mut-revoke-cuts-all` | Any revocation wakes and ends every connection (its own id not checked again) | `RevocationScoped` |
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

When remote plugins §3.2, §4.4 or dial-out §5, §6 change, or `link.rs` (`dialing`, `listening`, `start`), `registration.rs` (`hand_over`), `access_point.rs` (`Shared::connection`), a language's listening takeover order or the dialers' reconnection rules change, update this model and `expected.txt` with them (standard Q11.2.3).
