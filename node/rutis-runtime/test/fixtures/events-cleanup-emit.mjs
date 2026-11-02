// A leaf plugin for test/runner-events.test.mjs: a row with `{ listen }`
// listens to that name and logs each event through the host's `events-log`;
// a row with `{ announce: name }` emits that name from a cleanup, while its
// unload stack runs (events protocol §7.1 step 5). Its own listeners have
// left the bus by then, so only the other rows hear it.
import { definePlugin } from '../../../rutis/src/index.mjs'

export default definePlugin({
  name: 'cleanup-emit',
  inject: ['events-log'],
  apply(ctx, config) {
    if (config?.listen) ctx.on(config.listen, (...args) => ctx.use('events-log').add(`${config.listen}:${args[0]}`))
    if (config?.announce) ctx.effect(() => ctx.emit(config.announce, 'bye'))
  },
})
