// A plugin definition a plugin hands to a test's service (#1004).
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'part',
  provides: { echo: { say: 'sync' } },
  apply(ctx) {
    ctx.provide('echo', { say() { return 'part' } })
  },
})
