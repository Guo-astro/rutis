// A service that gives back what it is given.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'echo',
  provides: { echo: { echo: 'sync', async: 'async' } },
  apply(ctx) {
    ctx.provide('echo', { echo: value => value, async: async value => value })
  },
})
