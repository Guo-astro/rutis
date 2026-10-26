# Remote runtimes: dialing out and dynamic connections

[中文](design-remote-runtime-dial-2026-10-14.md)

> Revised (2026-10-26): the rule in §4 that `listen` together with `instances` means dynamic connections is replaced by §4 of [Remote runtime consistency](design-remote-runtime-consistency-2026-10-26.en.md): the `dial`, `listen` and `instances` fields of a `runtimes.remote` entry are independent, and `listen` without `instances` is one connection. Everything else is unchanged.

Status: final; R-P1 (the Rust side) and R-P2 (Python dialing out) implemented, the rest in §7. Date: 2026-10-14. Baseline: `main` `a763cdc` (0.10.0).
Based on: [remote plugins](design-remote-plugins-2026-10-03.en.md) (this document revises how runtimes connect in its §4), [Connect Nodes](guide/nodes.en.md) (link), [creating plugin instances on demand](design-loader-instance-mounts-and-services-2026-10-07.en.md), [service names inside instances](design-instance-services-2026-10-08.en.md), [the Go plugin runtime](design-go-runtime-2026-10-10.en.md), [the Bun runtime](design-bun-runtime-2026-10-09.en.md).
Used by: [app integration](design-app-integration-2026-10-14.en.md), [the Swift SDK](design-swift-sdk-2026-10-12.en.md), [Android app hosting](design-android-runtime-2026-10-12.en.md).

Out of scope: implementing identity, pairing and revocation (it needs a management service, not implemented in this project); how node interconnection (`export` / `import` / `host` / `rows`) connects; code upload.

## 1. Problem

Existing remote runtimes assume "a few fixed servers". Within that assumption they work; beyond it there are three limits:

| Limit | Today (`main` `a763cdc`) | Consequence |
| --- | --- | --- |
| Fixed direction | The Python, Go and Node remote runtimes only implement `listen:`, with rutis dialing in; the Bun runtime does not support remote use yet (it refuses both listening and dialing out) | A remote machine must be directly reachable by rutis (fixed address, not behind NAT); every language implements its own WebSocket server with TLS, and every remote machine manages its own certificates (the Python 3.13.6 TLS deadlock is the price of this) |
| Static topology | One remote runtime = one link row + one `runtimes.remote` entry + one dedicated token; a link inside an instance cannot enable `runtime` | Cases where the count changes (scaling workers, many devices) cannot be handled |
| Identity coupled to the connection | The listening side identifies peers by a per-peer token, written into the connection layer | Identity cannot be handed to a separate management service |

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| R1 | Every remote runtime supports both directions: listening (rutis dials in) and dialing out (the runtime connects to a rutis access point); which one to use is a deployment choice, not a language restriction | 3 |
| R2 | One rutis access point accepts many runtime connections; each connection is an instance (an `instanced` group), created when the connection arrives and closed when it drops | 4 |
| R3 | Connections are told apart by an id the runtime reports itself in the handshake; a new connection with the same id takes over the old one | 5 |
| R4 | Identity is a replaceable layer: during the transition, a shared token + self-reported id (rutis does not verify it); once a management service is connected, a verified id replaces it and nothing else changes | 5 |
| R5 | Dial-out and reconnection behavior is one language-independent specification, enforced by the `dial_conformance` conformance tests | 6 |
| R6 | The existing approach (static configuration, rutis dialing in to listening runtimes) behaves the same; no forced migration | 8 |
| R7 | Transport security rules are unchanged: non-loopback must use TLS; when dialing out, the TLS server is only on the rutis side | 3 |
| R8 | The runtime side is implemented entirely in each platform language, with no Rust dependency (Python, Node, Bun, Go, Swift, Kotlin) | 7 |

## 3. Direction

```text
Existing: rutis ──dial in──▶ runtime (listening, with its own TLS server and certificate)
New:      rutis (access point, the only TLS server) ◀──dial out── runtime × N
```

- When dialing out, the runtime side only needs a WebSocket **client**, which is easier to implement in each language than the existing server.
- rutis has the same control over runtimes that dial in to it as today: loading, configuration, unloading and gating are all driven by rutis (remote plugins §4.1).
- The runtime-side channel is written `dial:wss://…`, symmetric with the existing `listen:wss://…`; every language supports both.
- Choosing: use listening when the remote machine has a fixed address that rutis can reach directly; use dialing out when it is behind NAT, its address changes, or the number of machines changes (devices, scaling workers).
- A listening-side link with `runtime` works today (R-P0, 2026-10-14): once the runtime connects, rutis sends `mount`, loads rows and gates services as usual, and withdraws them when it disconnects; the handshake checks the runtime's self-reported id against the link's peer end to end. No runtime can dial out yet, so the test stands in with a relay that dials both listeners (`rutis-loader/tests/remote_rows.rs::a_runtime_link_can_listen`). R-P1 needs no change to links for this.

## 4. Dynamic connections are instances

- One access point corresponds to one `instanced` group: the rows in the group describe "which plugins to load in a runtime connection, and with what configuration".
- A connection arrives → an instance is created with the connection id; it drops → that instance is closed, all services in it are withdrawn, and consumers stop.
- The runtime reports the plugins it carries in the `mount` reply (`plugins`, each entry the same as a `rows.schema` reply); when a row in the group points, in some instance, to a plugin that connection does not carry, the row fails in that instance with "this connection does not contain plugin X", without affecting other instances. Adding fields such as `engine` to the `mount` reply is not a protocol change (precedent: Bun §3.3).
- Service names inside an instance resolve within that instance (the instance services design, no fallback), so several connections carrying same-named plugins do not conflict, and there are no aliases; rows outside an instance cannot use its services; when needed, put the consumer in the same group.
- Configuration: a `runtimes.remote` entry gains `listen` (the access point name) and `instances` (the `instanced` group id); having both means a dynamic connection. Rows in the group use `<runtime name>:<plugin>`, which in each instance resolves to that instance's connection. No separate link row is needed: rutis-host starts an access point on that listener for such entries (implementation below).

  ```json
  {
    "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
    "runtimes": { "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }] },
    "rows": [
      { "id": "phone", "group": true, "instanced": true, "config": [
        { "id": "contacts", "name": "phone:contacts" },
        { "id": "digest", "name": "py:digest", "inject": ["contacts"] }
      ] }
    ]
  }
  ```

- Three things need to be added:
  1. An access point accepts many connections (today a link is one-to-one);
  2. A connection triggers instance creation and closing (today only business code calls `create_instance`);
  3. Runtime links are allowed inside instances (today `RuntimeSession#<name>` is a global key and `peer.rs` refuses it); runtime keys are distinguished per instance.

- Implementation (R-P1, #382):
  - The access point is `rutis-bridge`'s `AccessPointPlugin` (not a link row; `peer.rs` keeps refusing `runtime` links inside instances). `rutis-loader`'s `RuntimeInstances` handles each connection: it mounts a runtime on the connection's session and calls `create_instance` with the connection (`RuntimeConnection`) as the value; the release calls `remove_instance`. A connection's runtime process belongs to its instance and rows hold it directly, not through a runtime service key, so no per-instance `RuntimeSession` key is needed.
  - The `mount` reply's `plugins` is `{ "<plugin>": <rows.schema reply> }`, keyed by the row name after `<runtime name>:`; it is optional, and without it each plugin is asked for with `rows.schema` (a remote `NotFound` means "not contained"). Each connection is read once.
  - The services a connection's plugins provide are published at `host_key_in(name, instance)`. A shared name resolves, when the instance chain meets a connection that provides it, to that connection's instance key (for Rust rows and rows of every language alike), so local runtime rows (such as `py:digest`) use their own connection's service too, isolated by the instance label (which needs `scopes`); names the connection does not provide stay global.
  - "This connection does not contain plugin X" is a per-copy `Unavailable`: only that copy fails; an edit's dry run is not refused for it, and reconcile does not count it as a new failure to roll back.
  - A `<runtime name>:` row outside the group fails, saying it runs only in that group.

## 5. Identity layer

- On the rutis side, "authenticate and give a connection id" is factored into a layer: input is the credentials and self-reported id from the handshake; output is accept (connection id) or reject (reason).
- Transitional implementation: accept if the credential equals the shared token (environment variable `RUTIS_TOKEN_<runtime name>`, consistent with existing naming), with connection id = self-reported id; the self-reported id must be non-empty and contain only `[a-z0-9-]`. The self-reported id is only used to tell connections apart and name instances, never as an identity credential.
- Future implementation: a management service verifies the credentials and gives the id; access points, instances and configuration do not change.
- The existing static link with a per-peer token (R6) is another implementation of this layer and is kept.
- Revocation hook (#273, implemented): rutis provides only the mechanism; whom to revoke, when, and whether it persists is for the future management service.
  - API (`rutis-bridge`): `Revocations` (clones share) has `revoke(&id)`, `revoke_all()`, `is_revoked(&id)` and `revoked(&id)` (wait until revoked); `Revocable::new(inner, revocations)` wraps any `Identity` (static links) or `Admission` (access points). `Identity` and `Admission` each gain `revocations()` with a default implementation (`None`: revokes nothing), so existing implementations do not change.
  - The "identity" is the id this layer gives: the far end's endpoint id for a static link, the connection id for an access point. With a shared token and self-reported ids, revoking an id closes just that connection, and later connections greeting with that id are rejected with 4003 after the hello; `revoke_all()` amounts to withdrawing the shared token: every connection closes and later ones are refused with 403 at the upgrade. rutis does not verify self-reported ids, so the same token still connects under another id: cutting off a holder of the shared token takes `revoke_all()`.
  - After a revocation: only the connections accepted as that identity close (a link's session, an access point's instance), the session first and then the `Peer` withdrawn / the instance released, so calls in flight fail once with a transport error (as when the network drops); a handshake routed before the revocation but not yet handed over is refused by the re-check in `Registrations::hand_over` (4003); later connections are refused exactly as a wrong token is (403 "not accepted here", or 4003), the dialer classifies them `AuthRejected` and retries every 30 s. Other links, other registrations on the same listener, and local runtimes are untouched. A listening link keeps its registration and reports `Waiting { AuthRejected, "<id>: identity revoked" }`; a dialing link closes its session and dials no more, waiting 30 s at a time as for an authentication failure.
  - Revocations live in memory and are not undone; a restart restores them. rutis-host's transitional implementation (tokens from the environment) cannot revoke at runtime: change the token and restart, or wait for the management service.

## 6. Dial-out and reconnection specification

Language-independent; every language implements it:

| Item | Rule |
| --- | --- |
| Backoff | Retryable failures start at 0.5 seconds and double up to 30 seconds, reset once the connection is stable (same as the existing link) |
| Credentials rejected | Retry every 30 seconds. The dialer tells this from 401 / 403 at the upgrade, or from the access point refusing the id after the hello (WebSocket close code 4003) |
| Incompatible protocol | Stop and report. A hello with no id or an invalid one (`[a-z0-9-]+`) belongs here: retrying cannot fix it |
| Disconnect | The rutis side closes that connection's instance; the runtime side unloads all rows (cleanup runs in reverse order), then reconnects with backoff |
| Reconnect | New session, new instance, load again, independent of any state from the previous connection |
| Takeover by the same id | When a new connection arrives the old one is closed; the old instance finishes cleanup before the new connection is admitted |
| Deliberate stop | The runtime side stops (e.g. the SDK's `stop()`): unload all rows, then close the connection; no reconnect |

Conformance tests: `rutis-bridge/tests/dial_conformance.rs`, shaped like `websocket_conformance`: the rutis side starts an access point, launches the dialing side per language, and checks each row of the table above.

- Implementation (R-P2, #383):
  - What every dialer does (the same in each language; `dial_conformance` checks it): the command line `dial:<address> --id <id> --peer <rutis id> <project>`; the credential in `RUTIS_TOKEN`, a further trusted CA in `RUTIS_CA`; a non-loopback `ws://` and an invalid id are refused at start (non-zero exit status). State lines go to stderr: `rutis: connected to …`, `rutis: cannot connect (…); retrying in <s>s`, `rutis: connection lost (…); retrying in <s>s`, `rutis: refused: auth-rejected (…); retrying in <s>s`, `rutis: refused: incompatible (…); stopping` (then exit status 1). SIGTERM / Ctrl-C is the dialer's own stop, exit status 0.
  - The waits are the defaults of `Retry` in `link.rs`: backoff with ±20% jitter; "stable" is a session lasting 60 s; a fixed 30 s after rejected credentials; an access point that does not greet within 10 s is a retryable failure. Tests shorten them with `RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>`, of a kind with `RUTIS_HEARTBEAT` and for tests only.
  - Failures are classified as the Rust dialer classifies them, by type or status, never by text: 401 / 403 at the upgrade and a server certificate this side refuses are rejected credentials; 400 / 404 / 405 / 426, a subprotocol mismatch and a far hello whose version this side does not speak are incompatible; a far end greeting with an id other than `--peer` is rejected credentials (as `IdentityMismatch`); everything else (cannot connect, timeouts, disconnects, being taken over with 4002) is retryable. The access point rejects an id with 4003 only after the dialer has received rutis's hello, so a `connected to` line may come before the rejection.
  - Same-id takeover has no special case: the one taken over reconnects by its backoff and takes over in turn. Two processes with one id keep displacing each other, at a pace that slows with the backoff to about once every 30 s each (a session shorter than 60 s does not reset it). Self-reported ids are not verified by rutis (§5); unique ids are the deployment's to ensure.
  - Python: `rutis/dialing.py` (the rules and the reconnect loop, pure Python) and `dial` in `rutis/websocket.py`. The websockets asyncio client does all of a connection's reads and writes on one dedicated event loop thread: its threading client reads and writes one TLS socket from two threads, which OpenSSL does not support, and about 5% of `wss://` handshakes lost their response in a test. For the same reason dialing out is not affected by CPython 3.13.6's TLS deadlock (listening still refuses that version).
  - Go: `go/rutis/dial.go` (the rules, the reconnect loop and the WebSocket client), standard library only: the client shares one frame implementation with `listen.go` (`wsConn`; the dialing side masks the frames it sends). A session's end is classified by error type: `internal/peer`'s `ErrIncompatible` and `ErrEndpointMismatch`, and close code 4003. When the access point is incompatible, `Serve` prints `refused: incompatible (…); stopping` as its error and exits with status 1.
  - Node, Bun (#384): as when listening, each session runs in a child runtime process of its own (Node: `spawnSession` in `src/serve.mjs`; Bun: `src/remote.ts`); the parent only relays message by message between the WebSocket and the child's local channel, so a session's rows, proxies and references end with the child, and a reconnection is a new process and a new session. The rules and the reconnect loop are in each runtime's `dialing.mjs` / `dialing.ts` (Bun keeps its own copy and does not import Node's code); the parent reads the access point's hello once on the way through, to say `connected to` or to refuse it by `--peer` and version. Its own stop: the parent sends the child SIGTERM (a terminal's Ctrl-C reaches both), the child unloads every row while the session is still open and ends it, then the parent closes the connection. Node uses `ws`; Bun uses its built-in `ws` client (its `WebSocket` does not report the status of a refused upgrade), and a failed TLS handshake (close code 1015, which is how Bun reports an untrusted certificate) counts as an authentication failure. `RUTIS_CA` is trusted together with the roots each platform trusts by default (a `ca` given alone replaces them in Node and Bun). Bun listens with `Bun.serve`, starting a child runtime per session as well.

## 7. Work per party

| Party | Work | Status |
| --- | --- | --- |
| Rust side | Access point accepting many connections, connection-as-instance, per-instance runtime keys, the identity layer and its transitional implementation, `plugins` in the `mount` reply, rutis-host configuration (`listen` / `instances`), `runtimes.remote[].language` accepting `swift`, `kotlin` / `kt` | Implemented (#396, #399); `swift` and `kotlin` come with app integration |
| Python | Dial-out client (`dial:`); the first existing language (its TLS server has the most problems, so dialing out helps it most) | Implemented (#383); the Python column of `dial_conformance` passes |
| Node | Dial-out client, right after Python | Implemented (#384); the `dial_conformance` Node column passes |
| Go | Dial-out client, right after Python | Implemented (#384); the Go column of `dial_conformance` passes |
| Bun | Both listening and dialing out, right after Python | Implemented (#384); the `dial_conformance` Bun column and `session_matrix::bun_listening_on_a_websocket` pass |
| Swift | Both listening and dialing out (Swift SDK §6) | Not implemented |
| Kotlin | Both listening and dialing out (Android design §5) | Implemented (#391); the Kotlin column of `dial_conformance` and `session_matrix::kotlin_listening_on_a_websocket` pass |
| Docs | Add dialing out and dynamic connections to the Connect Nodes guide; change the recommended approach to dialing out | Dialing out and dynamic connections written (Python, Node, Bun); the recommendation changes once every language dials out |

## 8. Compatibility

- Existing configuration (`runtimes.remote` + link `dial` + `runtime`) behaves the same.
- Runtime-side `listen:` is one of the two directions and is kept long term.

## 9. Phases

| Phase | Work | Acceptance |
| --- | --- | --- |
| R-P0 | Test the current state of a listening-side link + `runtime` | Findings written back into §3 |
| R-P1 | Rust side: the Rust work in §7; the `dial_conformance` framework | Two connections carry same-named plugins, and consumers in each instance only use their own connection's services; dropping one connection closes only its instance; existing remote runtime tests all pass |
| R-P2 | Python dialing out | The Python runtime dials out and connects; the Python column of `dial_conformance` passes |
| R-P3 | Node and Go dialing out; Bun listening and dialing out; Swift and Kotlin connect with apps (Swift A1, Android K-P2) | Every language passes `dial_conformance` (dialing out) and `websocket_conformance` (listening) |
