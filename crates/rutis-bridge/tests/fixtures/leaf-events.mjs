import { definePlugin } from '../../../../node/rutis/src/index.mjs'

const seen = []
export default definePlugin({
  provides: { probe: { seen: 'sync' } },
  apply(ctx, { observe }) {
    if (observe) ctx.provide('probe', { seen: () => seen })
    else ctx.on('leaf/changed', async value => {
      await Promise.resolve()
      seen.push(value)
    })
  },
})
