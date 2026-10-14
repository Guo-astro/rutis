# @arcships/rutis

Write [rutis](https://github.com/arcships/rutis) plugins in TypeScript or JavaScript.

```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  inject: ['llm'],                              // services it uses
  provides: { weather: { today: 'async' } },    // services it provides
  config: { type: 'object', properties: { city: { type: 'string' } } },
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use('llm'), config.city))
    return () => {}                             // cleanup
  },
})
```

Test it without a host:

```ts
import { load } from '@arcships/rutis/testing'

const t = await load(plugin, { config: { city: 'Oslo' }, services: { llm: { ask: async () => 'sunny' } } })
assert.equal(await t.service('weather').today(), 'sunny in Oslo')
await t.unload()
```

This package depends on `@deepseek-ai/cordis`: its testing tool runs the plugin on Cordis's own event bus, the one the Node and Bun runtimes use, so `ctx.on`, `ctx.emit` and host emission behave in a test as they do on the runtime. The host installs the runtime that runs the plugin (`@arcships/rutis-runtime` for Node, `@arcships/rutis-bun` for Bun). Start a project with `npx @arcships/rutis-host new <name> --lang node` (or `--lang bun`).

Guide (Chinese): [docs/guide/typescript-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/typescript-plugin.md).
