# Writing a Bun plugin

The plugin runs in [Bun](https://bun.com): the host starts the Bun runtime `@arcships/rutis-bun`, and TypeScript needs no build. Plugins are written as [TypeScript plugins](typescript-plugin.en.md) are, with `bun` commands. The 8 steps below go from a new project to publishing, with the template's `greeter` as the example: it provides the `greeter` service, and uses a `translate` service someone else provides to translate its greeting. Needs Bun 1.4 or later (Windows is not tested yet).

## 1. Start a project

<!-- example: bun-en run -->
```bash
bunx --bun @arcships/rutis-host new greeter --lang bun
cd greeter
bun install
bun test
```

`new` prints:

```text
created greeter/
next: cd greeter && bun install && bun test && bunx --bun rutis-host dev
```

| File | Purpose |
| --- | --- |
| `src/index.ts` | The plugin |
| `test/index.test.ts` | Tests (`bun test`), no host needed |
| `rutis.dev.json` | The rows of the local host (step 7) |
| `package.json` | Depends on `@arcships/rutis`; dev dependencies `@arcships/rutis-bun`, `@arcships/rutis-host` |
| `tsconfig.json` | `bun run check` type checks: Bun runs TypeScript without checking types |
| `.github/workflows/publish.yml` | Tests and publishes to npm on a `v*` tag (step 8) |

## 2. Provide a service

The generated `src/index.ts` is a plugin:

<!-- example: bun-en file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

// Each part below is a step of the Bun plugin guide:
// https://github.com/arcships/rutis/blob/main/docs/guide/bun-plugin.en.md

export interface Config {
  // Filled from the schema's default when the row leaves it out (step 4).
  greeting: string
}

class Greeter {
  constructor(private readonly greeting: string) {}

  hello(name: string): string {
    return `${this.greeting}, ${name}!`
  }
}

export default definePlugin<Config>({
  // Services this plugin uses: `inject: ['translate']`, then
  // `ctx.use('translate')` (step 3).
  inject: [],
  // The services it provides, with their methods (step 2).
  provides: { greeter: { hello: 'async' } },
  // The JSON Schema of its config (step 4).
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(config.greeting))
  },
})
```

- `definePlugin` defines the plugin; the host calls `apply` when it runs it.
- `ctx.provide('greeter', object)` provides the service named `greeter`: other plugins, other languages and the host call its methods by that name.
- `provides` says which services it provides and their methods, so the host knows without running the plugin. Add each new method here too.

## 3. Use a service

`greeter` uses the `translate` service to translate its greeting. Name it in `inject` and get it with `ctx.use`:

<!-- example: bun-en file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  constructor(private readonly translate: Translate, private readonly greeting: string) {}

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

The plugin starts once every service in `inject` is available, and stops when one goes. Which language or row provides `translate` is not the plugin's concern. The template's own test gives no `translate`; step 6 changes it.

## 4. Configuration

The plugin's config is the second argument of `apply`. `config` is its JSON Schema: `greeting` has the `default` `'Hello'`, so when the row leaves `greeting` out, `apply` gets `{ greeting: 'Hello' }`. A config that does not fit the schema makes the row fail with `config: …`, and `apply` is not called.

Where the config comes from: in tests, `load`'s `config` (step 6); when it runs, the `config` of the plugin's row in the host (`rutis.dev.json` in step 7).

## 5. Emit and listen to events

Each time it greets someone, `greeter` emits the event `greeter/greeted`; it also listens to that event itself, counting the greetings, which a new method `count()` returns:

<!-- example: bun-en file=greeter/src/index.ts -->
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
    const greeting = await this.translate.text(this.greeting)
    this.ctx.emit('greeter/greeted', name)
    return `${greeting}, ${name}!`
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
    ctx.on('greeter/greeted', () => { greeter.greeted++ })
    ctx.provide('greeter', greeter)
  },
})
```

- `ctx.emit(name, ...args)` emits an event, `ctx.on(name, listener)` listens. A listener needs no declaration: events of that name from other plugins, other languages and the host reach it. It goes when the plugin unloads.
- The other ways to emit (waiting for results, in order, the first result, …): [plugin API: events](plugin-api.en.md#events).

## 6. Test

`load` from `@arcships/rutis/testing` loads the plugin without a host: give it a config and a fake `translate`, call its service, listen to the events it emits, unload it. Replace `test/index.test.ts` with:

<!-- example: bun-en file=greeter/test/index.test.ts -->
```ts
import { expect, test } from 'bun:test'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, {
    config: { greeting: 'Hi' },
    services: { translate: { text: async (text: string) => words[text] ?? text } },
  })
  const greeted: string[] = []
  t.on('greeter/greeted', (name: string) => { greeted.push(name) })
  expect(await t.service('greeter').hello('Ada')).toBe('Salut, Ada!')
  expect(greeted).toEqual(['Ada'])
  expect(await t.service('greeter').count()).toBe(1)
  await t.unload()
})
```

<!-- example: bun-en run -->
```bash
bun test
bun run check
```

- `services` are the services the plugin `inject`s; `t.service(name)` is a service the plugin provides; `t.on` listens to events as the host does; `t.unload()` unloads the plugin, running its cleanups.
- `load` checks what the host checks: the config fits the schema, the plugin uses only the services in `inject`, and what it provides has every method in `provides`. Values pass by the rules between processes.
- When the host gives you its services' types (a `.d.ts` written by `rutis-host types`), write `ctx.use<Services['translate']>('translate')` and `bun run check` checks the calls before anything runs; see [Building a host open to plugins: types for plugin authors](open-host.en.md#types-for-plugin-authors).
- The rest of the test tool: [plugin API: the TypeScript and Bun test tool](plugin-api.en.md#the-typescript-and-bun-test-tool).

## 7. Run it locally and try it

`rutis-host dev` runs this project in a local host. Each plugin the host runs is a **row**, and `rutis.dev.json` writes the rows:

<!-- example: bun-en file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "bun:./dev/fake-translate.ts" }
  ]
}
```

- The row with the plugin's id (`greeter`) is the plugin itself; its `config` is the plugin's config.
- The other row is a fake `translate`: `name` says where the plugin is, `bun:` that it runs in the Bun runtime, the path relative to the project.

The fake `translate` is a Bun plugin too:

<!-- example: bun-en file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => words[text] ?? text })
  },
})
```

<!-- example: bun-en run until="greeter: running" -->
```bash
bunx --bun rutis-host dev
```

```text
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
greeter: running
```

`greeter` starts once `translate` is available. The `services` line lists the services there are now and the row providing each; the host itself provides `loader`. It is printed once every row is running, waiting or failed, and again whenever the services change, before or after the rows' state lines.

While `dev` runs, in another terminal in the project's directory, see which services there are and which row provides each, then call `greeter.hello` once:

```console
$ bunx --bun rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  bun: running (pid 66997)
calls:
  none
$ bunx --bun rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ bunx --bun rutis-host call greeter.count
1
```

Each argument of `call` is a JSON value, a string with its quotes (`'"Ada"'`); the result is printed as JSON, and an error goes to stderr with exit status 1. See [rutis-host: Seeing the current state](rutis-host.en.md#seeing-the-current-state) and [Calling a service](rutis-host.en.md#calling-a-service).

Change `src/index.ts` and save it (say `${name}!` to `${name}.`): `dev` reloads the row, and the next `call` gets the new code's result:

```text
greeter: reloaded (src/index.ts changed)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

```console
$ bunx --bun rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada."
```

Only the rows whose files changed reload; a change to `rutis.dev.json` applies while it runs too. See [rutis-host: Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev).

To run on another machine, see [the remote section of rutis-host.en.md](rutis-host.en.md#remote-runtimes).

## 8. Publish

- **Package name**: before publishing, change `name` in `package.json` to a scoped name (`"@your-name/greeter"`; a short name such as `greeter` is long taken on npm).
- **Publish by tag**: push a tag such as `v0.1.0`, and the template's workflow runs `bun test`, `bun run check` and `rutis-host check`, then `npm publish` (the repository needs an `NPM_TOKEN` secret). By hand, `npm publish --access public`; it runs `prepack` first, which builds `dist/`.
- **Dependency range**: `@arcships/rutis` takes a caret range (`rutis-host new` writes its own version), and several versions of the SDK can live in one host.

How a host installs your plugin: [rutis-host: How a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## Debugging

`--debug` opens the Bun runtime process's own inspector, always at `ws://127.0.0.1:6499/rutis`:

```bash
bunx --bun rutis-host dev --debug
```

Open the `https://debug.bun.sh/#127.0.0.1:6499/rutis` from the output in a browser, or attach VS Code's Bun extension to `ws://127.0.0.1:6499/rutis`. Set a breakpoint in a source file, or write `debugger;` in the code.

- The address is the same on every start: after the runtime restarts, connect again.
- While it is stopped at a breakpoint, the whole Bun runtime waits: calls to its other rows wait too.
- What `console.log` prints shows in `dev`'s output.

## What the runtime promises

- **Only what the project installed**: the runtime always starts with `--no-install`; a missing package makes loading fail and is never downloaded. Install plugins and their dependencies with `bun add`. By default `bun install` runs install scripts only for dependencies on Bun's built-in trusted list; a project's `trustedDependencies` **replaces** that list, so list only the packages whose install scripts are needed.
- **No `.env`**: the runtime starts with `--no-env-file`, so the environment comes only from the host. The project's `bunfig.toml` applies (including `preload`), as the project's own trusted configuration.
- **Re-entrant synchronous calls**: while a plugin's synchronous call into rutis waits, other calls into this runtime still run. A plugin's service may therefore be called during its own synchronous call: do not hold a lock across a call into rutis.
- **An uncaught error ends the process**, withdrawing every service of the runtime.
- **Reloading** imports the plugin module itself again; the modules it imports are replaced only by restarting the runtime.
- **Packages installed while the runtime runs** (`bun add`) are found only after the runtime restarts: a Bun process remembers that a package was missing.
- `rutis-host dev` runs the project in Bun when it finds `bun.lock`, `runtimes.bun` in `rutis.dev.json`, or a dependency on `@arcships/rutis-bun`.

## Advanced: plugin API

Plugins as values, the other ways to emit, scopes, synchronous methods, objects with methods, the rest of the test tool: [plugin API](plugin-api.en.md).
