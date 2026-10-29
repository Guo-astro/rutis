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
