// Listeners of the five modes, for the SDK's test tool under Bun.
import { definePlugin } from '../../../../../node/rutis/src/index.mjs'

export default definePlugin({
  name: 'events',
  apply(ctx: any) {
    ctx.on('ask', (city: string) => `sunny in ${city}`)
    ctx.on('wrap', async (next: () => Promise<string>) => `<${await next()}>`)
  },
})
