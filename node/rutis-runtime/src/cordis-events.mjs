// Cordis plugins on the logical bus (#464; docs/design-events-protocol-
// 2026-10-18.en.md): the Node runtime's Context dispatches every event that
// is not `internal/*` through the bus (./events.mjs), with Cordis's own
// five methods and its listener options.
//
// - Registration: Cordis's `internal/listener` hook takes every `ctx.on`
//   (and `ctx.once`) of a name that is not `internal/*`: the listener goes
//   on the bus, owned by the fiber that registered it, and leaves with it.
// - Dispatch: `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`,
//   `[thisArg,] name, ...args`, run by the bus's rules: on this process's
//   listeners when every listener of the name is here, else through the
//   host. Cordis's `internal/dispatch` is fired first, as Cordis does.
// - Filter: `thisArg[Context.filter]` is called with the listener's own
//   Context for a listener here and `{ rutis: descriptor }` for one
//   elsewhere; every Context has a read-only `ctx.rutis`, its descriptor,
//   so `c => c.rutis.row === 'chat'` works for both. `global` listeners
//   are not filtered.
// - `internal/*` stays Cordis's own: registered and dispatched as before.
//
// A leaf plugin's filter (`ctx.filter(predicate)`) is passed as
// `thisArg[KEEP]`: called with the descriptor alone.

export const KEEP = Symbol.for('rutis.events.keep')

const ORIGINAL = Symbol.for('cordis.original')

const internal = name => typeof name !== 'string' || name.startsWith('internal/')

// Install the bus on `ctx`, the root of a Context. `where(context)` says
// whose a Context is: `{ row, scope, signal }` (row null for a plugin
// loaded by `mount`).
export function install(ctx, Context, bus, where) {
  const events = ctx.events[ORIGINAL] ?? ctx.events
  const original = Object.fromEntries(['emit', 'parallel', 'serial', 'waterfall', 'bail'].map(mode => [mode, events[mode]]))

  bus.dispatched = (mode, name, args) => original.emit.call(events, 'internal/dispatch', mode, name, args, null)

  const describe = context => {
    const current = bus.filtering
    if (current && current.owner === context) return bus.describe(current)
    const { row, scope } = where(context)
    return { name: null, prefix: false, plugin: context.fiber?.name ?? 'root', row: bus.rowId(row), runtime: bus.runtime, node: null, scope, global: false }
  }
  ctx.accessor('rutis', { get() { return describe(this) } })
  ctx.accessor('signal', { get() { return where(this).signal } })

  ctx.on('internal/listener', function (name, listener, options) {
    if (internal(name)) return
    const context = this
    const { row, scope } = where(context)
    const label = `ctx.on(${JSON.stringify(name)})`
    let entry
    const dispose = context.fiber.effect(() => {
      entry = bus.on({ name, callback: listener, prepend: options?.prepend, global: options?.global, row, plugin: context.fiber.name, scope, owner: context })
      return () => bus.off(entry)
    }, label)
    // Cordis's disposer says whether the listener was still registered.
    return () => {
      const live = entry?.live ?? false
      dispose()
      return live
    }
  })

  // Cordis's primitive the five are built on, `events.dispatch(type, args)`:
  // it gives the callbacks to call (some plugins call them themselves). The
  // listeners are the bus's, not Cordis's: it gives one callback that
  // dispatches on the bus (here, or through the host when the name has
  // listeners elsewhere).
  const dispatchCallbacks = events.dispatch
  const throughBus = { emit: 'parallel', serial: 'serial', bail: 'bail' }
  events.dispatch = function (type, args) {
    const hasThis = typeof args[0] === 'object' || typeof args[0] === 'function'
    const name = args[hasThis ? 1 : 0]
    const mode = throughBus[type]
    if (typeof name !== 'string' || internal(name) || !mode) return dispatchCallbacks.call(this, type, args)
    const thisArg = hasThis ? args.shift() : null
    args.shift()
    original.emit.call(this, 'internal/dispatch', type, name, args, thisArg)
    const { row, scope } = where(this.ctx)
    return [(...values) => bus.dispatch(mode, name, values, { row, scope, thisArg })]
  }

  for (const mode of Object.keys(original)) {
    events[mode] = function (...args) {
      const hasThis = typeof args[0] === 'object' || typeof args[0] === 'function'
      const name = args[hasThis ? 1 : 0]
      if (internal(name)) return original[mode].apply(this, args)
      const thisArg = hasThis ? args[0] : null
      const values = args.slice(hasThis ? 2 : 1)
      const terminal = mode === 'waterfall' ? values.pop() : undefined
      // Cordis's own diagnostics hook, once for this dispatch here.
      original.emit.call(this, 'internal/dispatch', mode, name, values, thisArg)
      const { row, scope } = where(this.ctx)
      let keep
      if (typeof thisArg?.[KEEP] === 'function') keep = thisArg[KEEP]
      else if (typeof thisArg?.[Context.filter] === 'function') {
        const filter = thisArg[Context.filter]
        keep = (descriptor, entry) => filter.call(thisArg, entry ? entry.owner : { rutis: descriptor })
      }
      return bus.dispatch(mode, name, values, { row, scope, keep, thisArg, terminal })
    }
  }
}
