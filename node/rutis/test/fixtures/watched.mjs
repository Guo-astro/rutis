// A definition that says what it hears: a listener, and a service to read
// how often it heard (#1004's "its listeners go with the row").
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'watched',
  provides: { watched: { hits: 'sync' } },
  apply(ctx) {
    let heard = 0
    ctx.on('watched/ping', () => { heard++ })
    ctx.provide('watched', { hits() { return heard } })
  },
})
