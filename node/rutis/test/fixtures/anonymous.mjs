// A plugin that declares no name (#933): its row takes its package's, so a
// test's line says `rutis: …` here rather than `anonymous: …`.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  provides: { anon: { ping: 'sync' } },
  apply(ctx) {
    ctx.provide('anon', { ping: () => 'pong' })
  },
})
