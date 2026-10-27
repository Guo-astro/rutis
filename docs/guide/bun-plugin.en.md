# Writing a Bun plugin

The plugin runs in [Bun](https://bun.com): the host starts the Bun runtime `@arcships/rutis-bun`, and rows are named `bun:<module>`. Plugins are written as [TypeScript plugins](typescript-plugin.en.md) are (`definePlugin`, see the [plugin API](plugin-api.en.md)); the difference is that Bun runs them, so TypeScript needs no build.

## 1. Create a project

```bash
bunx --bun @arcships/rutis-host new greeter --lang bun
cd greeter
bun install
```

| File | Purpose |
| --- | --- |
| `src/index.ts` | The plugin |
| `test/index.test.ts` | Unit tests (`bun test`), no host needed |
| `rutis.dev.json` | Local runtime configuration and other plugins for testing |
| `package.json` | Depends on `@arcships/rutis` (a caret range, see [TypeScript plugins §5](typescript-plugin.en.md#5-publish)); dev dependencies `@arcships/rutis-bun`, `@arcships/rutis-host`; builds before `npm pack` and `npm publish` (`prepack`) |
| `tsconfig.json` | `bun run check` type checks: Bun runs TypeScript without checking types |
| `.github/workflows/publish.yml` | Tests and publishes to npm on a `v*` tag |

## 2. Write and test the plugin

Write it as in sections 2 and 3 of the TypeScript plugin guide. Tests use `bun test`:

```ts
import { expect, test } from 'bun:test'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('greets', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' } })
  expect(await t.service('greeter').hello('Ada')).toBe('Hi, Ada!')
  await t.unload()
})
```

Without the SDK, a module can also export `apply(ctx, config)` directly, with `inject`, `provides` (optional, each method `'sync'` or `'async'`) and `config` (a JSON Schema). `provides` may be left out: what `ctx.provide` provides reaches rutis by itself, its methods the object's functions, called as `async`, by the same rules as on Node (see [Plugin API](plugin-api.en.md)).

The result of `definePlugin(...)` can also be handed to a service as an ordinary argument, and the receiver mounts it elsewhere, as on Node; see the [plugin API](plugin-api.en.md#plugin-definitions).

### Events

As for TypeScript plugins ([TypeScript plugins: events](typescript-plugin.en.md#events)): `ctx.on` listens, `ctx.emit` / `ctx.parallel` / `ctx.serial` / `ctx.waterfall` / `ctx.bail` emit, `ctx.filter` picks listeners. Listeners register themselves with the host: events emitted by Node, Python and Go plugins and by Rust code in the host reach listeners in Bun too, and the other way round. On unload the row's `ctx.signal` is aborted and its running listeners are waited for.

```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    ctx.on('greeting/pick', (lang: string) => (lang === 'sv' ? 'Hej' : undefined))   // undefined when there is no result
    ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
    ctx.provide('greeter', {
      async hello(name: string, lang: string) {
        const word = (await ctx.serial<string>('greeting/pick', lang)) ?? 'Hello'
        const text = await ctx.waterfall<string>('greeting/format', `${word}, ${name}`, (text: string) => text)
        ctx.emit('greeting/said', name)                                               // returns at once
        return text
      },
    })
  },
})
```

Tests use the same test tool and run with `bun test` as usual; `host()` puts several plugins on one bus:

```ts
import { expect, test } from 'bun:test'
import { host } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('greets through events', async () => {
  const h = host()
  const t = await h.load(plugin)
  expect(await t.service('greeter').hello('Ada', 'sv')).toBe('Hej, Ada!')
  expect(await h.serial('greeting/pick', 'en')).toBeUndefined()
  await h.unload()
})
```

To check that the plugin emits an event, listen with `t.on(name, listener, options)` as the host does, at the root scope, as in [TypeScript plugins: testing](typescript-plugin.en.md#3-test): the plugin's emissions and the test's own reach it, and it returns the function that stops it.

### Objects with methods

A class instance, or a plain object with function-valued properties, goes to another language by reference: the far end gets a proxy whose calls reach the original object here. An object another process passes here is a proxy too:

```ts
const cache = await ctx.use('store').cache()
await cache.set('a', 1)          // every method is an asynchronous call of the far end's object
const value = await cache.get('a')
```

- Every name read on a proxy is a method (but `then`, `toJSON`, `toString`, `valueOf` and symbols: so awaiting a proxy gives the proxy itself, and the object's own methods of these names cannot be called through it); it has no data fields: to get data, have a method return it. `console.log` shows `[remote object]`.
- One object passed several times is one proxy on the far end; a proxy passed back to its own process is the original object there.
- An object does not follow its row: after the row that passed it unloads, it can still be called. Once its runtime has exited, calls on it fail with `peer-gone`.
- Calling a method the object lacks fails that call with `no-method`: `Cache has no method gett (methods: get, set)`.
- `undefined` inside an array or object arrives as `null`; only a whole argument or a whole result keeps it.
- Rows in the same runtime get the object itself (its methods synchronous as usual, its fields readable). The testing tool's strict mode gives the cross-process proxy.

## 3. Run it in a host

```bash
bunx --bun rutis-host dev
```

`rutis-host dev` runs the project in Bun when it finds `bun.lock`, `runtimes.bun` in `rutis.dev.json`, or a dependency on `@arcships/rutis-bun`, and reloads it when its files change; only the rows whose files changed reload (see [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev)).

A host's `rutis.json`:

```json
{
  "runtimes": { "bun": { "project": "." } },
  "rows": [
    { "id": "greeter", "name": "bun:greeter" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["greeter"] }
  ]
}
```

- `bun:<npm name or subpath>` resolves from the `node_modules` of `project`; `bun:./path` is relative to `project`. The row `bun:sqlite` names the project's module `sqlite`, not Bun's built-in `bun:sqlite`.
- Fields of `runtimes.bun`: `project` (default `.`), `runtime` (where `@arcships/rutis-bun` is; by default the project's), `program` (the Bun executable; by default `bun` on `PATH`).
- It can be configured together with `runtimes.node` and `runtimes.py`; services are shared across runtimes by name.

### Debugging

`--debug` opens the Bun runtime process's own inspector, always at `ws://127.0.0.1:6499/rutis`:

```bash
bunx --bun rutis-host dev --debug
```

Open the `https://debug.bun.sh/#127.0.0.1:6499/rutis` from the output in a browser, or attach VS Code's Bun extension to `ws://127.0.0.1:6499/rutis`. Set a breakpoint in a source file, or write `debugger;` in the code.

- The address is the same on every start: after the runtime restarts, connect again.
- While it is stopped at a breakpoint, the whole Bun runtime waits: calls to its other rows wait too.
- What `console.log` prints shows in `dev`'s output.

## 4. What the runtime promises

- **Only what the project installed**: the runtime always starts with `--no-install`; a missing package makes loading fail and is never downloaded. Install plugins and their dependencies with `bun add`. By default `bun install` runs install scripts only for dependencies on Bun's built-in trusted list; a project's `trustedDependencies` **replaces** that list, so list only the packages whose install scripts are needed.
- **No `.env`**: the runtime starts with `--no-env-file`, so the environment comes only from the host. The project's `bunfig.toml` applies (including `preload`), as the project's own trusted configuration.
- **Re-entrant synchronous calls**: while a plugin's synchronous call into rutis waits, other calls into this runtime still run. A plugin's service may therefore be called during its own synchronous call: do not hold a lock across a call into rutis.
- **An uncaught error ends the process**, withdrawing every service of the runtime.
- **Reloading** imports the plugin module itself again; the modules it imports are replaced only by restarting the runtime.
- **Packages installed while the runtime runs** (`bun add`) are found only after the runtime restarts: a Bun process remembers that a package was missing.

## 5. Publish

As for TypeScript plugins: `npm publish` (it runs `prepack`, the build, first; the template's workflow publishes on a `v*` tag; give the package a scoped name, see [TypeScript plugins §5](typescript-plugin.en.md#5-publish)). A host installs it with `bun add` and adds a row `{ "id": "greeter", "name": "bun:greeter" }`.

## 6. Run on another machine

A Bun runtime can also run on another machine, its plugins managed by rutis elsewhere, in either direction (see [Connect nodes](nodes.en.md)):

```bash
# Listen, and rutis connects (on the rutis side: a node row and runtimes.remote with language bun)
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://0.0.0.0:7443/rutis --id edge --peer main .
# Dial out to a rutis access point (on the rutis side: runtimes.remote with listen and instances)
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- Plugins are resolved from the last argument, the project directory; rows are named `<runtime-name>:<module>`, such as `edge:greeter` or `edge:./report.ts`.
- Each session runs in a child runtime process: when the session ends (a disconnect, a newer connection taking over) it unloads every row and exits, and the next session starts a new process.
- Dialing out reconnects by fixed rules and reports its state on stderr (`rutis: connected to …` and so on); SIGTERM or Ctrl-C unloads every row and exits. The rules are in [Connect nodes](nodes.en.md#runtimes-that-connect-in). `RUTIS_CA` is trusted together with the roots Bun trusts by default.

## Environment

Bun 1.4 or later. Windows is not tested yet.
