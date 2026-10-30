// A plugin that hands the host a command, the way open-host.md §5's
// `count` does: an async function whose result comes a while later.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'later',
  inject: ['commands'],
  async apply(ctx) {
    await ctx.use('commands').register('later', async () => {
      await new Promise(resolve => setTimeout(resolve, 200))
      return 'done'
    })
  },
})
