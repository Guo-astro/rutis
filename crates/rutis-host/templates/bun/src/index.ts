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
