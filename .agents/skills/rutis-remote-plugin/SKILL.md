---
name: rutis-remote-plugin
description: Use when a rutis plugin must run on another machine, device or app instead of next to the host — "remote plugin", running a Python/Node/Go plugin on a GPU box, connecting two rutis hosts, exposing a phone app's capabilities to rutis. Walks through choosing a node link vs a remote runtime vs app hosting, writing the plugin in the rutis paradigm, deploying, configuring rutis.json, verifying, and the rules that keep it inside the rutis design. Self-contained; reference material is in references/.
---

# Adding a remote plugin to rutis

rutis (https://github.com/arcships/rutis) is a plugin host with a Rust core that takes plugins in several languages. This skill does not depend on the rutis repository; everything it refers to is in `references/`:

| File | Contents |
| --- | --- |
| `references/plugin-api.md` | The plugin API: declarations, ctx, events, lifecycle, how values cross, sync calls and reentrancy, cancellation, versions |
| `references/nodes.md` | Links, running plugins on another node, remote language runtimes, security |
| `references/rutis-json.md` | `rutis.json`: `runtimes`, `listen`, `rows`, credentials, deployment |

Keep one sentence in mind: **rutis has no such thing as a "remote plugin" — only an ordinary plugin plus a runtime or node that runs somewhere else.** The plugin code is exactly what it would be locally; what changes is where its runtime runs and how rutis reaches it. Do not write a separate plugin API, protocol or lifecycle for remote use.

## 1. Decide which kind it is

| What the other side is | Use | Row on the rutis side |
| --- | --- | --- |
| A language runtime on another machine; **this rutis** loads, configures, starts and stops its plugins | Remote runtime (link + `runtime`) | `<runtime name>:<plugin>` |
| Another full rutis host that manages its own plugins and only shares services | Node link (`export` / `import`) | No new row; imported services are available by name |
| Another full rutis host on which this side wants to run rows it writes | Node link (`host: true` there, `rows: true` here) | `peer:<peer>/<plugin>` |
| A phone or desktop app offering its own capabilities as plugins | App hosting (designed, not implemented) | `<runtime name>:<plugin>`, one instance per connection |

When unsure, ask one question: **who decides whether the plugin is loaded and with what configuration?** This rutis → remote runtime / app hosting. The other side → node link.

## 2. What works today (2026-10-14; check the repository before relying on it)

| Language | Remote listening `listen:` (rutis dials in) | Dialing out `dial:` (runtime connects to rutis) |
| --- | --- | --- |
| Python | ✅ | Designed, not implemented ([#383](https://github.com/arcships/rutis/issues/383)) |
| Node / TypeScript | ✅ | Not implemented ([#384](https://github.com/arcships/rutis/issues/384)) |
| Go | ✅ | Not implemented (#384) |
| Bun | ❌ no remote use yet (#384) | ❌ |
| Swift / Kotlin apps | Designed, not implemented ([#389](https://github.com/arcships/rutis/issues/389), [#391](https://github.com/arcships/rutis/issues/391)) | Same |

"One access point accepting many connections, one instance per connection" (many devices, autoscaled workers) is [#382](https://github.com/arcships/rutis/issues/382), not implemented. Today one remote runtime is one static entry in `rutis.json`. Tracking issue: [#392](https://github.com/arcships/rutis/issues/392).

## 3. Write the plugin: the paradigm, same as local

Details in `references/plugin-api.md`. The essentials:

- Write a leaf plugin with the language's SDK: declare `inject` (service names it uses), `provides` (service names it offers, each method `sync` or `async`) and a config schema; in `apply(ctx, config)` use `ctx.use` / `ctx.provide` / `ctx.effect`. The current plugin API version is 2 (adds leaf events, `ctx.on` / `ctx.emit`).
- The plugin does **not** decide when it runs, does not reconnect, does not probe whether the other side is up: it is applied when all its services are ready, stops when any is withdrawn, and starts again when it returns. A remote disconnect is, to the plugin, an ordinary stop.
- Values follow the cross-process rules: JSON-compatible data is copied, functions and objects go by reference, integers within ±(2^53−1), nesting at most 64 levels. Never assume the same process.
- Do not hold locks while calling other services: your service may be called again in the middle of your own synchronous call. Prefer `async` methods for calls that cross runtimes and may cross each other.
- Leaf events are dispatched only **within one runtime process**; do not use events to talk across machines — use services.
- Dependency declarations contain only service names, never languages, machines or addresses.

## 4. Deploy a remote runtime (listening, as of today)

On the remote machine:

1. **Install** the plugin there: Python `pip install` into the interpreter the runtime uses; Node `npm install` in the project; Go, build the binary. A remote runtime loads only plugins already deployed there — **there is no code upload**.
2. Prepare TLS: non-loopback addresses must be `wss://`. Point `RUTIS_CERT` / `RUTIS_KEY` at the certificate and key; with a self-signed certificate, give the issuing CA to `RUTIS_CA` on the rutis side.
3. Set the token: `RUTIS_TOKEN` on the remote side, equal to `RUTIS_TOKEN_<RUNTIME NAME>` on the rutis side.
4. Start it listening. `--id` is this runtime's name, `--peer` is the name of the rutis that controls it:

   ```bash
   # Python
   RUTIS_TOKEN=… RUTIS_CERT=server.pem RUTIS_KEY=server.key \
     python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins

   # Node (inside that machine's Node project)
   node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://0.0.0.0:7443/rutis --id gpu --peer main ./package.json
   ```

   A Go plugin binary accepts `listen:wss://…` as its channel the same way.

## 5. Configure the rutis side

Details in `references/rutis-json.md` and `references/nodes.md`.

```json
{
  "runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
  "rows": [
    { "id": "gpu", "name": "rutis-bridge/peer",
      "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
    { "id": "embedder", "name": "gpu:embedder", "config": { "model": "small" } }
  ]
}
```

- `language`: `python` / `py` / `go` / `node`.
- The link row must have `"runtime": "<name>"`; without it the other side is not treated as a runtime and `<name>:*` rows never become ready.
- Rows are named `<runtime name>:<plugin>`; the plugin is resolved on the remote machine.

## 6. Verify

- `rutis-host check`: remote runtimes are not contacted during checking; services that might come from them are reported as such, not as failures.
- `rutis-host run`: expect `<link row id>: link connected`, then `<row id>: running`.
- `link refused: auth-rejected` means the token or certificate is wrong; `link stopped: incompatible` means the protocol or address form is wrong, or `runtime` is missing.
- Stop the remote process: those rows stop, and so do the rows using their services; when it comes back rutis reconnects and loads them again.

## 7. What not to do

| Don't | Because |
| --- | --- |
| Implement reconnects, heartbeats or offline caches in the plugin to "keep the remote available" | Connections and lifecycle belong to rutis; a plugin only sees whether services are there |
| Upload code to a remote runtime or load plugins by path | A remote runtime loads only deployed plugins |
| Use `ws://` to a non-loopback address | Non-loopback needs TLS; the rutis dialer refuses it outright |
| Model "devices" or "identities" in the protocol or the plugin | Device information is a service some plugin provides; identity belongs to a future management service |
| Alias same-named services to tell machines apart | Same-named plugins on different connections are told apart by instances (#382) |
| Write a separate plugin API for remote use, or make an SDK depend on Rust | The runtime side is implemented entirely in its platform language |
| Handle phone foreground/background or keep-alive in an SDK | That is the app's business logic; the SDK offers only `start` / `stop` |

## Further reading

- Remote plugins design: https://github.com/arcships/rutis/blob/main/docs/design-remote-plugins-2026-10-03.en.md
- Remote runtimes — dialing out and dynamic connections: https://github.com/arcships/rutis/blob/main/docs/design-remote-runtime-dial-2026-10-14.en.md
- App integration: https://github.com/arcships/rutis/blob/main/docs/design-app-integration-2026-10-14.en.md
