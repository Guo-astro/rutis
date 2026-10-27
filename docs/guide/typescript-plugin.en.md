# Write a TypeScript Plugin

This guide covers creating a project, publishing it, and using it from a host. JavaScript uses the same approach without the types.

## 1. Create a project

<!-- example: typescript-en run -->
```bash
npx @arcships/rutis-host new weather --lang node
cd weather
npm install
```

The generated project contains:

| File | Purpose |
| --- | --- |
| `src/index.ts` | The plugin itself. |
| `test/index.test.ts` | Unit tests that do not need a host. |
| `rutis.dev.json` | Local runtime configuration and other plugins used for testing. |
| `package.json` | Depends on `@arcships/rutis` (a caret range, see §5); development dependencies include `@arcships/rutis-host` and `@arcships/rutis-runtime`; builds before `npm pack` and `npm publish` (`prepack`). |
| `.github/workflows/publish.yml` | Tests and publishes to npm when a `v*` tag is pushed. |

## 2. Write the plugin

The generated `src/index.ts` is a `greeter` that uses no service, `test/index.test.ts` is its test, and `npm test` passes as generated. Here it becomes a `weather` that uses an `llm` service: replace `src/index.ts` with the code below, and `test/index.test.ts` with the one in §3 at the same time (the two files go together; replacing only one makes the template's own test fail).

<!-- example: typescript-en file=weather/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  city?: string
}

interface Llm {
  ask(question: string): Promise<string>
}

class Weather {
  constructor(private readonly llm: Llm, private readonly city: string) {}

  async today(): Promise<string> {
    return `${await this.llm.ask(`weather in ${this.city}`)} in ${this.city}`
  }
}

export default definePlugin<Config>({
  inject: ['llm'],                                 // Services used: start when all are ready; stop if any is revoked
  provides: { weather: { today: 'async' } },      // What it provides: check knows before running; only declared methods are callable
  config: { type: 'object', properties: { city: { type: 'string' } } },  // Configuration JSON Schema
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use<Llm>('llm'), config.city ?? 'Oslo'))
    return () => {}                                // Cleanup; ctx.effect(cleanup) also works, or return nothing
  },
})
```

The plugin depends only on `@arcships/rutis`. It does not need to know which host runs it, which language or machine provides `llm`, or when it starts and stops; the host decides those things. See [Plugin API](plugin-api.en.md) for capabilities and value passing rules.

`provides` says what the plugin provides, so `rutis-host check` knows without running the plugin that this row provides `weather`: when a host checks its configuration with `check --strict` in CI, the rows that use `weather` pass (the template declares it by default). A declared service follows its declaration: only the declared methods can be called, so add each new method here too; `'sync'` marks a method synchronous. It also runs without one: what `ctx.provide` provides still reaches rutis by itself, its methods the object's functions, all called as `async`; `check` then only knows once the plugin runs, and `--strict` reports an error.

A plugin is itself a value: hand the result of `definePlugin(...)` to a service as an ordinary argument (the application's part registry, say), and the receiver can mount it elsewhere, each mount one copy using what your `apply` prepared. See the [plugin API](plugin-api.en.md#plugin-definitions).

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

## 3. Test

`@arcships/rutis/testing` provides fake services and calls the services provided by the plugin without requiring a host:

<!-- example: typescript-en file=weather/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('reports the weather of the configured city', async () => {
  const t = await load(plugin, {
    config: { city: 'Oslo' },
    services: { llm: { ask: async () => 'sunny' } },
  })
  assert.equal(await t.service('weather').today(), 'sunny in Oslo')
  await t.unload()
})
```

<!-- example: typescript-en run -->
```bash
npm test
```

`load` checks the same things as the host: the plugin uses only services declared in `inject`; its provided service has every method declared in `provides` (an undeclared service is called as rutis calls it, by the object's methods, all async; `t.methods(name)` returns the methods rutis sees); and all cleanup functions run when it unloads. In strict mode (the default), values follow cross-process rules: data is copied, functions and objects with methods are passed by reference, and a method declared `sync` cannot return a Promise. Code that works only within one process fails here.

### Events

`ctx.on(name, listener)` listens to an event; `ctx.emit`, `ctx.parallel`, `ctx.serial`, `ctx.waterfall` and `ctx.bail` are the five ways to emit. Listeners register themselves with the host and nothing is declared: whether the emitter is another plugin in the same process, a plugin in another language, Rust code in the host or a linked node, the result is the same. The full rules are in [plugin API: events](plugin-api.en.md#events).

The two plugins below share one bus on the test tool's `host()`: `greeter` takes the first result with `serial`, lets other plugins rewrite the greeting with `waterfall` and notifies with `emit`; `shout` is a link of the waterfall and listens to the notification.

<!-- example: typescript-en file=weather/test/events.test.ts order=2 -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '@arcships/rutis'
import { host } from '@arcships/rutis/testing'

const greeter = definePlugin({
  apply(ctx) {
    // undefined / null / false is "no result": serial asks the next listener
    ctx.on('greeting/pick', (lang: string) => (lang === 'sv' ? 'Hej' : undefined))
    ctx.provide('greeter', {
      async hello(name: string, lang: string) {
        const word = (await ctx.serial<string>('greeting/pick', lang)) ?? 'Hello'
        // every argument is the event's; `with` gives the terminal, the innermost end of the chain
        const text = await ctx.waterfall.with((text: string) => text)<string>('greeting/format', `${word}, ${name}`)
        ctx.emit('greeting/said', name)                  // returns at once, waits for no listener
        return text
      },
    })
  },
})

const said: string[] = []
const shout = definePlugin({
  apply(ctx) {
    // a waterfall listener gets next last: it runs the rest of the chain
    ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
    ctx.on('greeting/said', (name: string) => { said.push(name) })
  },
})

test('plugins hear each other', async () => {
  const h = host()
  const t = await h.load(greeter)
  await h.load(shout)
  assert.equal(await t.service('greeter').hello('Ada', 'sv'), 'Hej, Ada!')
  assert.equal(await t.service('greeter').hello('Bo', 'en'), 'Hello, Bo!')
  assert.deepEqual(said, ['Ada', 'Bo'])
  assert.equal(await h.serial('greeting/pick', 'sv'), 'Hej')   // the test can emit too
  await h.unload()
})
```

- `emit` returns at once; a failing listener reaches neither the emitter nor the other listeners and is reported in one line where it runs (in a test, in `h.reported` / `t.reported`). To wait for every listener and get their errors, use `await ctx.parallel(...)`: failures are an `AggregateError` in registration order.
- `waterfall` returns a Promise whenever a listener is in another process, so always `await` it. `bail` is a synchronous `serial`; an `async` listener fails it with `AsyncListener`.
- `ctx.filter(d => d.row === 'audit').parallel(...)` delivers only to the listeners the filter keeps; `ctx.on(name, fn, { prepend: true })` goes first, `{ global: true }` is exempt from filters.
- Scopes: `h.load(plugin, { scope: 't1/s1' })` loads a plugin into an instance; its listeners carry the scope as a descriptor, and a listener receives every event of the name on the root bus, whatever the emitter's instance. A scope is only a descriptor for filters.
- On unload the plugin's `ctx.signal` is aborted and its running listeners are waited for (up to the shutdown deadline) before the cleanups run.

The `t` that `load(plugin)` returns has the five methods and `filter` too, emitting from that plugin's scope. For a plugin in another language listening to an event a TypeScript plugin emits, see [Python plugins: events](python-plugin.en.md#events).

## 4. Run in a local host

<!-- example: typescript-en run until="weather: running" order=99 -->
```bash
npx rutis-host dev
```

`dev` runs this project as a row using the source file `src/index.ts`, without a build, and reloads it when its files change; only the rows whose files changed reload, and a change to `rutis.dev.json` applies while it runs (see [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev)). Add other required services to `rutis.dev.json`, such as a fake `llm`:

<!-- example: typescript-en file=weather/rutis.dev.json -->
```json
{
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "./dev/fake-llm.ts" }
  ]
}
```

<!-- example: typescript-en file=weather/dev/fake-llm.ts -->
```ts
// dev/fake-llm.ts
import { definePlugin } from '@arcships/rutis'
export default definePlugin({
  provides: { llm: { ask: 'async' } },
  apply(ctx) { ctx.provide('llm', { ask: async () => 'sunny' }) },
})
```

The row with the same ID as the plugin supplies its configuration; the other rows are additional plugins to run. `npx rutis-host check` lists each row's dependencies, provided services, and configuration schema, and exits with a nonzero status if there is a problem. It is suitable for CI.

### Debugging

`--debug` opens the Node runtime process's own inspector, on `127.0.0.1:9229`:

<!-- example: typescript-en run until="weather: running" order=100 -->
```bash
npx rutis-host dev --debug
```

The output has `Debugger listening on ws://127.0.0.1:9229/…`. Open `chrome://inspect` in Chrome and click inspect under Remote Target, or attach VS Code to port 9229. Open `src/index.ts` in Sources and set a breakpoint, or write `debugger;` in the code.

- The npm launcher that starts `rutis-host` does not take the port. Do not use `NODE_OPTIONS=--inspect`: it opens an inspector in the launcher too, which starts first and takes the port, so you would attach to the launcher; `--inspect-brk` stops the launcher.
- After a reload the module's URL ends in `?rutis-reload=<n>`, a new script: set the breakpoint again in it, or use `debugger;`.
- While it is stopped at a breakpoint, the whole Node runtime waits: calls to its other rows wait too.
- When port 9229 is taken, Node prints `Starting inspector on 127.0.0.1:9229 failed` and runs without an inspector: end the process holding the port first.
- What `console.log` prints shows in `dev`'s output.

## 5. Publish

```bash
npm publish --access public   # runs prepack first: tsc generates dist/
```

Alternatively, push a tag such as `v0.1.0`. The generated workflow tests, runs `check`, and publishes (the repository needs an `NPM_TOKEN` secret). `npm pack` and `npm publish` both run `prepack` (`npm run build`) first, so the package always has `dist/`.

Before publishing, give the package a name of your own: short names such as `weather` and `greeter` were taken on npm long ago. Use a scoped name: change `name` in `package.json`, for example to `"@your-name/weather"` (the scope is your npm user or organization; the first publish of a public scoped package needs `--access public`, which the template's workflow passes). A host's row names the plugin by this package name.

Declare a version range such as `"@arcships/rutis": "^0.17.2"` (`rutis-host new` writes its own version). The plugin uses the SDK of the minor version it was written against: when the host has another version, npm installs the plugin a copy of its own, and SDKs of several versions work side by side (plugins are recognized by a `Symbol.for` marker, whichever copy made them). The plugin is tagged with its plugin API version (`definePlugin` adds this automatically). If the host runtime is older, it reports the problem clearly instead of failing later during execution.

## 6. Use from a host

Install the plugin in the host's Node project, alongside `@arcships/rutis-runtime`, then add a row named after the package:

```bash
npm install @your-name/weather
```

```json
{ "id": "weather", "name": "@your-name/weather", "config": { "city": "Oslo" } }
```

See [rutis-host and rutis.json](rutis-host.en.md#how-a-host-installs-plugins), or [Embed in a Rust application](rust-host.en.md) for a Rust host.

## Existing Cordis plugins

No rewrite is needed, and no change to the package: name the package in a row of `rutis.json` (a published package as well):

```json
{ "id": "storage", "name": "@deepseek-ai/dsh-storage" }
```

The plugin runs in the Node runtime's Cordis Context, in the same process as leaf plugins and other Cordis rows: rows of the same runtime use each other's services by Cordis's native rules and receive the objects themselves. The services it provides reach rutis by themselves, and other runtimes and Rust use them by name; the methods are the functions of the service object, called as `async`. To mark some methods `sync`, or to let `rutis-host check` know before running what it provides, declare them in the package's `package.json` (a declaration applies when present):

```json
"rutis": { "provides": { "weather": { "today": "sync" } } }
```

For name clashes and what `check` reports, see [Cordis](cordis.en.md).
