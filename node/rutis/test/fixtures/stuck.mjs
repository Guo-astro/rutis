// A service whose call never returns.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'stuck',
  apply(ctx) {
    ctx.provide('stuck', { wait: () => new Promise(() => {}) })
  },
})
