// A Cordis plugin on the logical bus, for test/runner-events.test.mjs: its
// listeners register themselves, and `cordis-probe.run` emits with Cordis's
// own methods, optionally filtered through `thisArg[Context.filter]`.
const FILTER = Symbol.for('cordis.filter')

export const name = 'events-cordis'

export function apply(ctx) {
  const seen = []
  ctx.on('internal/dispatch', (mode, name) => { if (name.startsWith('t/')) seen.push(`dispatch:${mode}:${name}`) })
  ctx.on('t/ask', city => { seen.push(`ask:${city}`); return `cordis:${city}` })
  ctx.provide('cordis-probe', {
    async run(mode, name, args, keepRow) {
      const thisArg = keepRow === undefined ? undefined : Object.assign(Object.create(ctx), {
        [FILTER]: context => { seen.push(`filter:${context.rutis.row}`); return context.rutis.row === keepRow },
      })
      const head = thisArg ? [thisArg, name] : [name]
      const result = await ctx[mode](...head, ...args)
      return result === undefined ? null : result
    },
    rutis: () => ctx.rutis,
    seen: () => seen.splice(0),
  })
}
