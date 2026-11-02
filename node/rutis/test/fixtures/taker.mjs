// A plugin that provides `echo` without declaring it, for the row that takes
// the name over after another withdrew it (#1030). It announces what it
// provides, as a plugin next to it does.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'taker',
  apply(ctx) {
    ctx.provide('echo', { say() { return 'B' } })
  },
})
