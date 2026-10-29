# Remote runtimes: one entry configures it all, one connection and many connections behave the same

[中文](design-remote-runtime-consistency-2026-10-26.md)

Status: superseded by [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md). Final, not implemented. Date: 2026-10-26. Baseline: `main` `b4eee45e` (0.17.1). Scheduled for: 0.19.
Issue: [#695](https://github.com/arcships/rutis/issues/695) (implementation: [#696](https://github.com/arcships/rutis/issues/696) loader, [#698](https://github.com/arcships/rutis/issues/698) rutis-host). Decision record: [remote runtime consistency](decision-remote-runtime-consistency-2026-10-26.en.md).
Revises: the rule in §4 of [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md) that "`listen` together with `instances` means dynamic connections"; see §6.

Out of scope: a new hand-over interface or a "connection plugin" layer; opening up authentication (custom credentials, checks that can wait); a relay example (reasons in the decision record); the optional `transport` field in configuration (revised 2026-10-28: there is no custom `Transport` today, and the host building its own connections, [#699](https://github.com/arcships/rutis/issues/699), was decided against, so the field has no use; the replaceable `Transport` interface stays in the code, to be exposed in configuration if a need ever comes); local runtimes; node interconnection; the session protocol and the runtime side in each language.

## 1. Today: the layers are already separated

```text
Plugin management      One connection: RuntimePlugin + RuntimeResolver      Many connections: RuntimeInstances
                          ▲ depends on RuntimeSession#<name>                    ▲ implements the access point's Accept callback
Connection management  One connection: link (rutis dials, or listens and     Many connections: access point (listens, one id per connection)
                          waits for the runtime to connect)
                          ▲                                                     ▲
Transport              Transport#<kind>: WebSocket, local process, in-memory; the host can mount its own
```

- There are two hand-over points between connection management and plugin management: for one connection, the `RuntimeSession#<name>` service; for many connections, the access point's `Accept` callback. Neither the link nor the access point knows about the loader.
- How messages are delivered is decided by `Transport`, which is replaceable. A relay, QUIC and Bluetooth are each a kind of `Transport`.

So "decoupling connections from plugin management" and "the host building its own connections" are already possible in the structure. What is missing is consistency and a configuration entry point.

## 2. Problems

| # | Problem | Today |
| --- | --- | --- |
| P1 | One connection needs configuration in two places | One `runtimes.remote` entry + one `rutis-bridge/peer` row with `runtime`; the two are not checked against each other, and if one is missing the row waits forever |
| P2 | When the remote side lacks a plugin, one connection and many connections report different errors | One connection: `NotFound` at resolution, the whole row fails; many connections: `Unavailable`, unavailable only on that connection |
| P3 | Many connections produce no output | The access point has `AccessEvent`; rutis-host does not subscribe to it |
| P4 | Many connections are hard-wired to WebSocket | rutis-host always gives the access point `"websocket"`; a transport the host builds itself can only be used by a one-connection link (not addressed, see M4) |
| P5 | Discovery and checks are written twice | `RuntimePlugin` and `AcceptInstances` each read the `mount` reply, each check `rows.v2` / `hosts`, and each has its own offline placeholder |

The difference in service visibility (global for one connection, only inside the instance for many connections) is not a problem: that is exactly the difference between one connection and many connections, and `instances` decides it.

## 3. Decisions

| # | Decision |
| --- | --- |
| M1 | Keep the current layers and the two hand-over points; no new hand-over interface, no rewrite of the link or the access point |
| M2 | One `runtimes.remote` entry configures a whole remote runtime; rutis-host generates today's link and runtime wiring internally; the old link-row form keeps working |
| M3 | `instances` decides one connection or many connections; `dial` / `listen` decides who connects; the two are independent |
| M4 | The transport is the existing WebSocket; the replaceable `Transport` interface stays in the code and is not exposed in configuration (revised 2026-10-28; it was an optional `transport` field; reasons under out of scope) |
| M5 | One connection and many connections share one implementation of discovery and checks; a missing plugin is always `Unavailable` |
| M6 | Many connections print connected, disconnected and refused |
| M7 | The runtime side, the session protocol and the Rust API are unchanged |

## 4. Configuration

| `dial` | `listen` | `instances` | Meaning | Inside rutis-host |
| --- | --- | --- | --- | --- |
| address | — | — | One connection, rutis dials | Dialing link + runtime wiring |
| — | listener | — | One connection, waits for the runtime to connect | Listening link + runtime wiring; the peer's `--id` must equal the runtime name |
| — | listener | group | Many connections, one instance per connection | Access point (as today) |
| — | — | — | No connection; waits for `RuntimeSession#<name>` to be provided elsewhere | Nothing (as today, for Rust embedding) |

- `dial` together with `listen`, or `dial` together with `instances`: startup error (what rutis dials can only be one connection).
- The connection is over WebSocket; there is no `transport` field (M4).
- Token rules are unchanged: `RUTIS_TOKEN_<name>`, falling back to `RUTIS_TOKEN` when absent.

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [
    { "name": "gpu",    "language": "python", "dial": "wss://gpu.example.com:7443/rutis" },
    { "name": "mac",    "language": "swift",  "listen": "public" },
    { "name": "worker", "language": "python", "listen": "public", "instances": "worker" }
  ] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "calendar", "name": "mac:calendar" },
    { "id": "worker", "group": true, "instanced": true, "config": [
      { "id": "chunker", "name": "worker:chunker" }
    ] }
  ]
}
```

Today `gpu` and `mac` each need a separate `rutis-bridge/peer` row; with the new form there is only this one `runtimes.remote` entry.

## 5. Behavior

- **Discovery and checks (M5)**: one implementation does "read the `mount` reply → check `rows.v2` / `hosts` → read the bundled plugins, or ask `rows.schema` when there are none". `RuntimePlugin` calls it for one connection, `RuntimeInstances` for many connections.
- **Missing plugin (M5)**: `Unavailable` in both modes, with the message "<runtime> does not contain plugin X"; an edit's dry run does not reject because of it, and other rows are not affected.
- **Output (M6)**:

  ```text
  runtime gpu: link connected
  runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
  runtime worker: connected worker-1
  runtime worker: disconnected worker-1 (connection lost)
  runtime worker: refused (auth-rejected)
  ```

  One connection keeps today's link status lines, with the prefix changed from the link row id to `runtime <name>` (a link row in the old form still uses the row id); many connections subscribe to the access point's `AccessEvent`.
- Disconnection, reconnection, takeover by the same id, and revocation: the same as today.

## 6. Relation to earlier designs

- §4 of [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md), "a `runtimes.remote` entry gains `listen` and `instances`; when both appear it means dynamic connections", is replaced by §4 of this document: the three fields are independent, and `listen` without `instances` is one connection. The rest of that document is unchanged.
- [Remote plugins](design-remote-plugins-2026-10-03.en.md), [the network stack](design-protocol-channel-decoupling-2026-10-03.en.md) and [app integration](design-app-integration-2026-10-14.en.md) are not affected.

## 7. Compatibility

- Old configuration runs without changing a character; the runtime side needs no upgrade; the Rust API is unchanged.
- Behavior changes (written into "Migrating to 0.19"): in one-connection mode, a plugin missing on the remote side changes from a whole-row `NotFound` to `Unavailable`; status lines in the new form are prefixed `runtime <name>`; many connections start producing output.

## 8. Implementation

| Issue | Content | Acceptance |
| --- | --- | --- |
| [#696](https://github.com/arcships/rutis/issues/696) loader | M5 | The same set of row tests runs over three wirings — link dial, link listen, access point — and reports a missing plugin the same way; the `ROW_FEATURES` check exists in one place only; the existing remote tests, `websocket_conformance` and `dial_conformance` pass with unchanged test cases |
| [#698](https://github.com/arcships/rutis/issues/698) rutis-host | M2, M3, M4, M6, documentation and migration guide | End to end for four forms: `dial`, `listen`, `listen` + `instances`, and the old link row (revised 2026-10-28: the acceptance for a custom `Transport` went with the `transport` field) |

Review also found two separate small problems, not covered here: [#701](https://github.com/arcships/rutis/issues/701) rows resolved while connected are not resolved again after reconnection (0.17.2); [#702](https://github.com/arcships/rutis/issues/702) a token conflict on the same listener becomes a startup error (0.18).
