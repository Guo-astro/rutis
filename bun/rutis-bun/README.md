# @arcships/rutis-bun

The Bun runtime of [rutis](https://github.com/arcships/rutis): runs JavaScript and TypeScript plugins in [Bun](https://bun.com) for a rutis host.

A host starts it as one process and runs plugins in it as rows named `bun:<module>`:

```json
{
  "runtimes": { "bun": { "project": "." } },
  "rows": [
    { "id": "weather", "name": "bun:@acme/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

- **Plugins** are written with [`@arcships/rutis`](https://www.npmjs.com/package/@arcships/rutis) (`definePlugin`), or as a module exporting `apply(ctx, config)` with `inject`, `provides` and `config`.
- **Events** are shared by rows in the same process: `ctx.on(name, listener)` returns a disposer and removes the listener when its row unloads; `ctx.emit(name, ...args)` calls listeners synchronously without awaiting their promises. A host can send events with `Process::emit` after declaring the names with `LocalRuntime::emits`, `RuntimePlugin::emits`, or the `Mount.emits` field; host emission waits for all listeners and reports their errors together. Plugin emission stays within the Bun process.
- **Modules** resolve from the project: npm names and subpaths from its `node_modules`, `./paths` relative to it. Install plugins with `bun add`.
- **Nothing is installed or read behind the host's back**: the runtime always runs with `--no-install` (a missing package fails, it is never downloaded) and `--no-env-file` (the environment is what the host gives). The project's `bunfig.toml` applies, as the project's own configuration.
- **Synchronous calls are re-entrant**: while a plugin's synchronous call into rutis waits, calls into this runtime still run, so runtimes that call each other never wait for each other for ever. A plugin's service may therefore be called while that plugin is inside a synchronous call: do not hold a lock across a call into rutis.
- **An uncaught error ends the process**, and with it every service of this runtime.
- **Reloading** a row imports its module again when the file changed; the modules it imports stay loaded (restart the runtime to replace those).
- **Packages installed while the runtime runs** are found after the runtime restarts: a Bun process remembers that a package was missing.

Bun 1.4 or later. Besides running on the host's machine (local channels), it runs as a remote runtime: listening (`listen:ws://…`, `listen:wss://…`) or dialing a rutis access point (`dial:ws://…`, `dial:wss://…`); see the Bun plugin guide.

Embedding hosts start it with `rutis_bridge::runtime::LocalRuntime::bun` (Rust, feature `bun`).
