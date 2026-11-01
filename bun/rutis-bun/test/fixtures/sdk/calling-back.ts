// A plugin that calls a service of the test while it starts up, and that
// service calls the test tool again, from inside the call it answers
// (../sdk.test.ts): the test tool registers its hooks for the test there,
// inside the async context the runtime's session runs calls in (#994).
import { definePlugin } from '../../../../../node/rutis/src/index.mjs'

export default definePlugin({
  name: 'calling-back',
  inject: ['files'],
  provides: { probe: { go: 'async' } },
  async apply(ctx) {
    await ctx.use('files').outer()
    ctx.provide('probe', { go: async () => 1 })
  },
})
