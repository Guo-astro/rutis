// The other half of a cycle (#933): it provides `later`, which `gated`
// waits for, and waits for `gated`, which `gated` provides.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'cycler',
  inject: ['gated'],
  provides: { later: { now: 'sync' } },
  apply(ctx) {
    ctx.provide('later', { now: () => 7 })
  },
})
