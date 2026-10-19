# @arcships/rutis

Write [rutis](https://github.com/arcships/rutis) plugins in TypeScript or JavaScript.

```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  inject: ['llm'],                              // services it uses
  config: { type: 'object', properties: { city: { type: 'string' } } },
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use('llm'), config.city))
    return () => {}                             // cleanup
  },
})
```

What `ctx.provide` provides reaches rutis by itself, its methods the object's functions, called as `async`. `provides` is optional: `provides: { weather: { unit: 'sync' } }` marks methods `sync` and tells `rutis-host check` what the plugin provides before it runs.

Test it without a host:

```ts
import { load } from '@arcships/rutis/testing'

const t = await load(plugin, { config: { city: 'Oslo' }, services: { llm: { ask: async () => 'sunny' } } })
assert.equal(await t.service('weather').today(), 'sunny in Oslo')
await t.unload()
```

This package has no dependencies. Events (`ctx.on`, and `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`) are one bus across every plugin, runtime and linked node; the testing tool runs them as a runtime does with every listener in its process (`host()` puts several plugins on one bus, each in its scope), so they behave in a test as on the runtime. The host installs the runtime that runs the plugin (`@arcships/rutis-runtime` for Node, `@arcships/rutis-bun` for Bun). Start a project with `npx @arcships/rutis-host new <name> --lang node` (or `--lang bun`).

Guide (Chinese): [docs/guide/typescript-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/typescript-plugin.md).
