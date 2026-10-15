# Remote runtimes: dialing out and dynamic connections

[中文](design-remote-runtime-dial-2026-10-14.md)

Status: final, not implemented. Date: 2026-10-14. Baseline: `main` `a763cdc` (0.10.0).
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
- Configuration: a `runtimes.remote` entry gains `listen` (the access point name) and `instances` (the `instanced` group id); having both means a dynamic connection. Rows in the group use `<runtime name>:<plugin>`, which in each instance resolves to that instance's connection. No separate link row is needed: rutis-host generates the listening-side link for such entries.

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

## 5. Identity layer

- On the rutis side, "authenticate and give a connection id" is factored into a layer: input is the credentials and self-reported id from the handshake; output is accept (connection id) or reject (reason).
- Transitional implementation: accept if the credential equals the shared token (environment variable `RUTIS_TOKEN_<runtime name>`, consistent with existing naming), with connection id = self-reported id; the self-reported id must be non-empty and contain only `[a-z0-9-]`. The self-reported id is only used to tell connections apart and name instances, never as an identity credential.
- Future implementation: a management service verifies the credentials and gives the id; access points, instances and configuration do not change.
- The existing static link with a per-peer token (R6) is another implementation of this layer and is kept.

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

## 7. Work per party

| Party | Work |
| --- | --- |
| Rust side | Access point accepting many connections, connection-as-instance, per-instance runtime keys, the identity layer and its transitional implementation, `plugins` in the `mount` reply, rutis-host configuration (`listen` / `instances`), `runtimes.remote[].language` accepting `swift`, `kotlin` / `kt` |
| Python | Dial-out client (`dial:`); the first existing language (its TLS server has the most problems, so dialing out helps it most) |
| Node, Go | Dial-out client, right after Python |
| Bun | Both listening and dialing out (neither is supported today), right after Python |
| Swift, Kotlin | Both listening and dialing out (Swift SDK §6, Android design §5) |
| Docs | Add dialing out and dynamic connections to the Connect Nodes guide; change the recommended approach to dialing out |

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
