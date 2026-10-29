// Cordis plugins on the logical bus (#464; docs/design-events-protocol-
// 2026-10-18.en.md): the Node runtime's Context dispatches every event that
// is not `internal/*` through the bus (./events.mjs), with Cordis's own
// five methods and its listener options.
//
// - Registration: Cordis's `internal/listener` hook takes every `ctx.on`
//   (and `ctx.once`) of a name that is not `internal/*`: the listener goes
//   on the bus, owned by the fiber that registered it, and leaves with it.
// - Dispatch: `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`,
//   `[thisArg,] name, ...args`, through the host (./events.mjs).
//   Cordis's `internal/dispatch` is fired first, as Cordis does.
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

// A leaf plugin's listener options carry its declared name (null: none,
// and descriptors name the plugin by its row's id).
export const LEAF_NAME = Symbol.for('rutis.events.leaf-name')

const ORIGINAL = Symbol.for('cordis.original')

const internal = name => typeof name !== 'string' || name.startsWith('internal/')

const errorText = error => `${error?.name ?? 'Error'}: ${error?.message ?? String(error)}`

// The callback of Cordis's `dispatch('emit')` runs a `parallel`: Cordis's
// `parallel` awaits it, its `emit` (and a plugin calling it so) does not.
// A failure nobody awaits is reported, as `emit` reports it (§5.6),
// instead of ending the process as an unhandled rejection: the line of an
// emit through the host.
function unawaited(bus, result, name, row) {
  if (!(result instanceof Promise)) return result
  let awaited = false
  result.catch(error => {
    if (awaited) return
    bus.report(`rutis: emit of ${name} from ${row === null || row === undefined ? 'the mount' : `row ${bus.rowId(row)}`} failed: ${errorText(error)}`, { event: name, row, error })
  })
  // Awaiting it, or calling then / catch / finally, takes the failure.
  return new Proxy(result, {
    get(target, key) {
      if (key === 'then' || key === 'catch' || key === 'finally') awaited = true
      const value = Reflect.get(target, key, target)
      return typeof value === 'function' ? value.bind(target) : value
    },
  })
}

// Install the bus on `ctx`, the root of a Context. `where(context)` says
// whose a Context is: `{ row, scope, signal, gone }` (row null for a plugin
// loaded by `mount`; `gone`: of a row unloaded since).
export function install(ctx, Context, bus, where) {
  const events = ctx.events[ORIGINAL] ?? ctx.events
  const original = Object.fromEntries(['emit', 'parallel', 'serial', 'waterfall', 'bail'].map(mode => [mode, events[mode]]))

  bus.dispatched = (mode, name, args) => original.emit.call(events, 'internal/dispatch', mode, name, args, null)

  const describe = context => {
    const current = bus.filtering
    if (current && current.owner === context) return current.descriptor
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
    // The plugin's declared name: Cordis's, or a leaf plugin's own, else
    // its row's id (§4.2).
    const plugin = options && Object.hasOwn(options, LEAF_NAME) ? options[LEAF_NAME] ?? bus.rowId(row) ?? context.fiber.name : context.fiber.name
    let entry
    const dispose = context.fiber.effect(() => {
      entry = bus.on({ name, callback: listener, prepend: options?.prepend, global: options?.global, row, plugin, scope, owner: context })
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
    const { row, gone } = where(this.ctx)
    return [(...values) => {
      const result = bus.dispatch(mode, name, values, { row, thisArg, gone })
      return type === 'emit' ? unawaited(bus, result, name, row) : result
    }]
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
      const { row, gone } = where(this.ctx)
      let keep
      if (typeof thisArg?.[KEEP] === 'function') keep = thisArg[KEEP]
      else if (typeof thisArg?.[Context.filter] === 'function') {
        const filter = thisArg[Context.filter]
        keep = (descriptor, entry) => filter.call(thisArg, entry ? entry.owner : { rutis: descriptor })
      }
      // Cordis's `emit` calls its listeners before it returns, and its
      // `waterfall` returns its value at once: both dispatched synchronously
      // (as `bail` is).
      return bus.dispatch(mode, name, values, { row, keep, thisArg, terminal, gone, sync: mode === 'waterfall' || mode === 'emit' })
    }
  }
}
