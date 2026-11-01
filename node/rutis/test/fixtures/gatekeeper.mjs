// The service the `gated` definition injects, provided by a row of its own
// (#1004): a row it waits for.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'gatekeeper',
  provides: { later: { now: 'sync' } },
  apply(ctx) {
    ctx.provide('later', { now() { return 7 } })
  },
})
