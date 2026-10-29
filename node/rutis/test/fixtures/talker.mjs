// A plugin whose service emits: the test hears it (`t.on`).
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'talker',
  apply(ctx, config) {
    ctx.provide('talker', {
      async today() {
        ctx.emit('talker/asked', config.city)
        return 'sunny'
      },
      async ask(place) { return ctx.serial('talker/answer', place) },
      async tell(entry) {
        ctx.emit('talker/told', entry)
        return entry.city
      },
    })
  },
})
