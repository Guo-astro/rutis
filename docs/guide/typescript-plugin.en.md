# Write a TypeScript plugin

[中文](typescript-plugin.md)

From a generated project to a published one, in eight steps. The whole guide is one plugin, `greeter`: it provides the `greeter` service and uses the `translate` service, which another plugin provides, to translate its greeting. JavaScript is written the same way, without the types. You need Node 22 or later.

## 1. Start a project

<!-- example: typescript-en run -->
```bash
npx @arcships/rutis-host new greeter --lang node
cd greeter
npm install
npm test
```

The project:

| File | What it is |
| --- | --- |
| `src/index.ts` | The plugin |
| `test/index.test.ts` | Unit tests; no host needed |
| `rutis.dev.json` | The rows to run locally (step 7) |
| `package.json` | Depends on `@arcships/rutis`; dev dependencies `@arcships/rutis-host`, `@arcships/rutis-runtime` |
| `.github/workflows/publish.yml` | Tests and publishes to npm on a `v*` tag (step 8) |

## 2. Provide a service

The generated `src/index.ts` is a plugin:

<!-- example: typescript-en file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  // Filled from the schema's default when the row leaves it out.
  greeting: string
}

class Greeter {
  constructor(private readonly greeting: string) {}

  hello(name: string): string {
    return `${this.greeting}, ${name}!`
  }
}

// The steps of the TypeScript plugin guide:
// https://github.com/arcships/rutis/blob/main/docs/guide/typescript-plugin.en.md
export default definePlugin<Config>({
  // Services this plugin uses (step 3): `inject: ['translate']`, then
  // `ctx.use('translate')`. It starts once they are all available.
  inject: [],
  // The services it provides and their methods (step 2). Only the methods
  // listed here can be called: add each new one.
  provides: { greeter: { hello: 'async' } },
  // The JSON Schema of its config (step 4): the config apply receives has
  // the defaults filled in.
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(config.greeting))
  },
})
```

- `definePlugin` defines the plugin; it is the module's default export.
- `apply` runs when the plugin starts. `ctx.provide('greeter', object)` provides the object as the service `greeter`. Other plugins, in any language, use it by the name `greeter` and call its methods.
- `provides` says which services the plugin provides and their methods: only the methods listed there can be called, so add each new one there too. Other plugins always call them asynchronously (`'async'`), so `hello` may return a plain string.
- When the plugin stops, the services it provides go with it.

## 3. Use a service

Make `greeter` translate its greeting with the `translate` service: add `translate` to `inject`, and get it with `ctx.use('translate')`. Replace `src/index.ts` with:

<!-- example: typescript-en file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  constructor(
    private readonly translate: Translate,
    private readonly greeting: string,
  ) {}

  async hello(name: string): Promise<string> {
    return `${await this.translate.text(this.greeting)}, ${name}!`
  }
}

export default definePlugin<Config>({
  inject: ['translate'],
  provides: { greeter: { hello: 'async' } },
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(ctx.use<Translate>('translate'), config.greeting))
  },
})
```

The plugin starts once every service in `inject` is available; when one goes, the plugin stops, and it starts again when the service is back. Which plugin, in which language, provides `translate` is not the plugin's concern. Calls to another plugin's methods are asynchronous, hence the `await`.

The template's own test no longer passes now (it gives no `translate`); step 6 replaces it.

## 4. Configuration

`config` is the JSON Schema of the configuration. The `config` that `apply` receives has the schema's defaults filled in: without a `greeting`, it is `'Hello'`. With a configuration that does not match the schema, the plugin does not start.

Where the configuration comes from: in a test, `load(plugin, { config })` passes it (step 6); when it runs, the host's row for this plugin gives it (`rutis.dev.json` in step 7, a host's `rutis.json`).

## 5. Emit and listen to events

After each greeting, emit the event `greeter/greeted` with the name; `greeter` listens to that event itself too, counts the greetings, and returns the count from a new method, `count`. Replace `src/index.ts` with:

<!-- example: typescript-en file=greeter/src/index.ts -->
```ts
import { definePlugin, type Context } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  greeted = 0

  constructor(
    private readonly ctx: Context,
    private readonly translate: Translate,
    private readonly greeting: string,
  ) {}

  async hello(name: string): Promise<string> {
    const text = `${await this.translate.text(this.greeting)}, ${name}!`
    this.ctx.emit('greeter/greeted', name)
    return text
  }

  async count(): Promise<number> {
    return this.greeted
  }
}

export default definePlugin<Config>({
  inject: ['translate'],
  provides: { greeter: { hello: 'async', count: 'async' } },
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    const greeter = new Greeter(ctx, ctx.use<Translate>('translate'), config.greeting)
    ctx.on('greeter/greeted', () => { greeter.greeted += 1 })
    ctx.provide('greeter', greeter)
  },
})
```

- `ctx.emit(name, ...args)` emits an event and returns at once.
- `ctx.on(name, listener)` listens to an event: it arrives whether this plugin emitted it, another plugin in the same process, a plugin in another language, or the host. Nothing to declare; the listener goes when the plugin stops.
- `count` is a new method, so it is added to `provides` too.

The other ways to emit (waiting for every listener, getting a value back) are in [plugin API: events](plugin-api.en.md#events).

## 6. Test

`load` from `@arcships/rutis/testing` needs no host: give the plugin a configuration and a fake `translate`, call the service it provides, listen to the events it emits as a host does, and unload it. Replace `test/index.test.ts` with:

<!-- example: typescript-en file=greeter/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const french: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }
const translate = { text: async (text: string) => french[text] ?? text }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' }, services: { translate } })
  const greeted: string[] = []
  t.on('greeter/greeted', (name: string) => { greeted.push(name) })
  const greeter = t.service('greeter')
  assert.equal(await greeter.hello('Ada'), 'Salut, Ada!')
  assert.deepEqual(greeted, ['Ada'])
  assert.equal(await greeter.count(), 1)
  await t.unload()
})

test('the greeting is Hello when the config leaves it out', async () => {
  const t = await load(plugin, { services: { translate } })
  assert.equal(await t.service('greeter').hello('Ada'), 'Bonjour, Ada!')
  await t.unload()
})
```

<!-- example: typescript-en run -->
```bash
npm test
```

- `services` gives fakes of the services the plugin injects, by name.
- `t.service('greeter')` is the service the plugin provides; only the methods in `provides` can be called, as in a host.
- `t.on(name, listener)` listens to an event as a host does: whatever the plugin emits arrives.
- `load` also checks what a host checks: that the configuration matches the schema, and that the plugin uses only the services in `inject`. Values pass by the rules between processes, so a plugin that passes its tests behaves the same when called from another process.

When the host gives TypeScript declarations of its services (a `.d.ts`), use them in place of your own `Translate` interface, as in `ctx.use<Services['translate']>('translate')`; your editor and `tsc` then check the calls before anything runs, and the fake in the test follows the same interface. The rest of the test tool is in [plugin API: the TypeScript and Bun test tool](plugin-api.en.md#the-typescript-and-bun-test-tool).

## 7. Run it locally and try it

`rutis-host dev` runs the plugin locally and reloads it when its files change. Each plugin in a host is a "row": a row says which plugin to run and what configuration to give it. The rows `dev` runs are in `rutis.dev.json`.

First write a fake `translate`. It is a plugin too, under `dev/`:

<!-- example: typescript-en file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const french: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => french[text] ?? text })
  },
})
```

Replace `rutis.dev.json` with two rows:

<!-- example: typescript-en file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "./dev/fake-translate.ts" }
  ]
}
```

- The row with the plugin's id (`greeter`) is this project's plugin itself; `config` is its configuration.
- The other row is the fake `translate`; `name` says where that plugin is.

<!-- example: typescript-en run until="rutis-host dev: services: greeter (greeter), loader (host), translate (translate)" -->
```bash
npx rutis-host dev
```

```text
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
greeter: running
```

`greeter` runs once `translate` is available; the lines may come in another order. The `rutis-host dev: services:` line lists the services now and the row providing each (`loader` comes from the host). While `dev` runs, in another terminal in the project directory, look at the state and call `greeter` once:

```console
$ npx rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  node: running (pid 63145)
calls:
  none
$ npx rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx rutis-host call greeter.count
1
```

Each argument of `call` is a JSON value, so a string has quotes; without them it says `rutis-host: argument 1 is not JSON: write a string in quotes, as '"Ada"'`.

Change the code: in `src/index.ts`, change `` `, ${name}!` `` to `` `, ${name}! Welcome.` `` and save. `dev` prints:

```text
greeter: reloaded (src/index.ts changed)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

Call it again: it returns `"Bonjour, Ada! Welcome."`. Only the row whose files changed reloads; a change to `rutis.dev.json` also takes effect while it runs. Ctrl-C ends `dev`. All of `dev`'s rules are in [rutis-host: developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev).

To run on another machine, see [the remote section of rutis-host.en.md](rutis-host.en.md#remote-runtimes).

## 8. Publish

First give the package a name of your own: short names like `greeter` were taken on npm long ago. Change `name` in `package.json` to a scoped name, such as `"@your-name/greeter"` (the scope is your npm user name or organization).

```bash
npm publish --access public   # runs prepack first: tsc writes dist/
```

Or push a tag such as `v0.1.0`: the template's `.github/workflows/publish.yml` tests and publishes it (the repository needs an `NPM_TOKEN` secret).

The dependency is a range such as `"@arcships/rutis": "^0.18.0"` (`rutis-host new` writes its own version); when a host has another version installed, npm gives the plugin its own copy, and several copies can coexist.

How a host installs your plugin: [rutis-host: how a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## Debugging

`--debug` opens the Node runtime process's own inspector, on `127.0.0.1:9229`:

<!-- example: typescript-en run until="greeter: running" -->
```bash
npx rutis-host dev --debug
```

The output has `Debugger listening on ws://127.0.0.1:9229/…`. Open `chrome://inspect` in Chrome and click inspect under Remote Target, or attach VS Code to port 9229. Open `src/index.ts` under Sources and set a breakpoint, or write `debugger;` in the code.

- The npm launcher that starts `rutis-host` does not take the port. Do not use `NODE_OPTIONS=--inspect`: it opens an inspector in the launcher too, the launcher starts first and takes the port, and you attach to the launcher; `--inspect-brk` stops the launcher.
- After a reload the module's URL carries `?rutis-reload=<n>` and is a new script: set the breakpoints again in the new script, or use `debugger;`.
- While stopped at a breakpoint the whole Node runtime is stopped: calls to the other rows in that runtime wait too.
- When port 9229 is taken, Node prints `Starting inspector on 127.0.0.1:9229 failed` and runs on without an inspector: end the process holding the port first.
- What `console.log` writes shows in `dev`'s output.

## Advanced: plugin API

Plugins as values, the other ways to emit, objects with methods, cleanup, synchronous methods, versions and more: [plugin API](plugin-api.en.md). Existing Cordis plugins run unchanged: see [Cordis](cordis.en.md).
