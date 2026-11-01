// A definition that waits for a service nothing provides yet (#1004's
// gating: the row waits, as a row of `load` does).
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'gated',
  inject: ['later'],
  provides: { gated: { ping: 'sync' } },
  apply(ctx) {
    ctx.provide('gated', { ping() { return ctx.get('later').now() } })
  },
})
