// A leaf plugin for test/retired-handle.test.mjs: it provides `coin`,
// declaring none of its methods (announced), whose `swap` withdraws it and
// provides an object with other methods under the name.
import { definePlugin } from '../../../rutis/src/index.mjs'

export default definePlugin({
  name: 'reshape',
  apply(ctx) {
    let withdraw = ctx.provide('coin', {
      toss: () => 'heads',
      swap() {
        withdraw()
        withdraw = ctx.provide('coin', { spin: () => 'spun' })
        return null
      },
    })
  },
})
