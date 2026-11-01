// A plugin that takes a plugin definition as an argument, to see what a
// definition made in the test's own process meets (#1004).
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'sink',
  provides: { sink: { register: 'async' } },
  apply(ctx) {
    ctx.provide('sink', { async register(definition) { return definition?.name ?? null } })
  },
})
