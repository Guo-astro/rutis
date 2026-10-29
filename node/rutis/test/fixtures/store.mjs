// A plugin that keeps what it is given, and calls the objects it is given.
import { definePlugin } from '../../src/index.mjs'

const kept = []
export default definePlugin({
  name: 'store',
  provides: { store: { put: 'sync', last: 'sync' } },
  apply(ctx) {
    ctx.provide('store', {
      put(item) { kept.push(item) },
      last() { return kept.at(-1) },
    })
    let first
    ctx.provide('warmer', {
      async warm(cache) {
        const seen = { shown: String(cache), field: typeof cache.size, then: cache.then }
        await cache.set('a', 1)
        seen.missing = await cache.gett().catch(error => [error.message, error.code, error.data])
        return { got: await cache.get('a'), seen }
      },
      async same(cache) { first ??= cache; return first === cache },
      async back(cache) { return cache },
      async plain(object) { return object.ping() },
      async data(value) { value.n = 2; return value.n },
    })
  },
})
