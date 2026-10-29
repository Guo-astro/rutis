// Listeners of each mode, for the test to emit to.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'listener',
  apply(ctx) {
    ctx.on('ask', city => `sunny in ${city}`)
    ctx.on('wrap', async next => `<${await next()}>`)
    ctx.on('tick', () => { throw Object.assign(new Error('boom'), { name: 'Boom' }) })
    ctx.on('fail', () => { throw new TypeError('typed') })
    ctx.on('work', () => new Promise(resolve => ctx.signal.addEventListener('abort', () => setTimeout(resolve, 20))))
  },
})
