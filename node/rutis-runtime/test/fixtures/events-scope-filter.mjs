// A leaf plugin for test/runner-events.test.mjs: a row with `{ listen }`
// listens to that name; a row with `{ probe }` provides `probe-<probe>`,
// whose `filtered(name)` emits in `parallel` through a filter that records
// the scope of each descriptor it is asked about and keeps those of its own
// scope. A row with `{ keep: true }` keeps its context after it unloads,
// and `probe-<probe>.emitKept(name)` emits with that context.
import { definePlugin } from '../../../rutis/src/index.mjs'

let kept

export default definePlugin({
  name: 'scope-filter',
  apply(ctx, config) {
    if (config.listen) ctx.on(config.listen, () => {})
    if (config.keep) kept = ctx
    if (config.probe) {
      ctx.provide(`probe-${config.probe}`, {
        async filtered(name, scope) {
          const seen = []
          await ctx.filter(descriptor => {
            const path = descriptor.scope.map(item => item.id).join('/')
            seen.push(path)
            return path === scope
          }).parallel(name)
          return seen
        },
        async emitKept(name) {
          try {
            kept.emit(name)
            return null
          } catch (error) {
            return `${error.name}: ${error.message}`
          }
        },
      })
    }
  },
})
