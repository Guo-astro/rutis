// A plugin whose service emits a little after it returned: the test,
// which hears it (`t.on`), is calling the test tool synchronously by then
// (../testing-heard.test.mjs).
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'teller',
  apply(ctx) {
    ctx.provide('teller', {
      async tell(name) {
        setTimeout(() => ctx.emit('teller/told', name), 100)
        return name
      },
    })
  },
})
