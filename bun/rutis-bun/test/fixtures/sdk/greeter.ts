// A plugin written with the SDK, for its test tool under Bun.
import { definePlugin } from '../../../../../node/rutis/src/index.mjs'

export default definePlugin({
  name: 'greeter',
  inject: ['clock'],
  provides: { greeter: { hello: 'sync' } },
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx: any, config: any) {
    const clock = ctx.use('clock')
    ctx.provide('greeter', { hello: (name: string) => `${config.greeting}, ${name} at ${clock.now()}` })
  },
})
