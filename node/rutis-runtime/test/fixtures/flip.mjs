// A leaf plugin for test/retired-handle.test.mjs: it provides `flip`,
// whose `swap` withdraws it and provides another object under the name.
import { definePlugin } from '../../../rutis/src/index.mjs'

export default definePlugin({
  name: 'flip',
  apply(ctx) {
    const coin = side => ({
      toss: () => side,
      swap() {
        withdraw()
        withdraw = ctx.provide('flip', coin(side === 'heads' ? 'tails' : 'heads'))
        return null
      },
    })
    let withdraw = ctx.provide('flip', coin('heads'))
  },
})
