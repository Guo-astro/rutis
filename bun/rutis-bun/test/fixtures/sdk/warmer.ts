// A plugin that calls the objects it is given.
import { definePlugin } from '../../../../../node/rutis/src/index.mjs'

export default definePlugin({
  name: 'warmer',
  provides: { warmer: { warm: 'async', back: 'async', plain: 'async', data: 'async' } },
  apply(ctx: any) {
    ctx.provide('warmer', {
      async warm(cache: any) { await cache.set('a', 1); return [String(cache), await cache.get('a')] },
      async back(cache: unknown) { return cache },
      async plain(object: any) { return object.ping() },
      async data(value: any) { value.n = 2; return value.n },
    })
  },
})
